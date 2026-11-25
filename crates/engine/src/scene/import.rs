//! Imported model data is immutable. Scene evaluation builds independent mesh
//! owners because placement, material operations and UV effects may mutate them.
mod file;
mod wire;

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

const MISSING_CORNER: i32 = -1;

#[derive(Clone, Debug)]
pub struct ImportedMesh(Arc<Geometry>);

#[derive(Debug)]
struct Geometry {
    id: u64,
    models: Vec<ImportedModel>,
    lights: Vec<ImportedLight>,
}

#[derive(Debug, Default)]
struct ImportedModel {
    positions: Vec<MeshVector>,
    uvs: Vec<MeshUv>,
    normals: Vec<MeshVector>,
    tangents: Vec<MeshVector>,
    groups: Vec<ImportedGroup>,
}

#[derive(Debug)]
struct ImportedGroup {
    kind: i32,
    indices: [Vec<i32>; 4],
    smooth: f64,
}

#[derive(Debug)]
struct ImportedLight {
    enabled: bool,
    position: MeshVector,
    colors: [[f32; 4]; 3],
    attenuation: [f32; 3],
    direction: MeshVector,
    cutoff: f32,
    exponent: f32,
    kind: i32,
}

impl ImportedMesh {
    fn new(models: Vec<ImportedModel>, lights: Vec<ImportedLight>) -> Result<Self, SceneError> {
        for model in &models {
            model.validate()?;
        }
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Ok(Self(Arc::new(Geometry {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            models,
            lights,
        })))
    }

    pub fn model_count(&self) -> usize {
        self.0.models.len()
    }

    pub(crate) fn id(&self) -> u64 {
        self.0.id
    }

    pub(crate) fn has_missing_corner_attributes(&self) -> bool {
        self.0
            .models
            .iter()
            .flat_map(|model| &model.groups)
            .any(|group| {
                group.indices[1].contains(&MISSING_CORNER)
                    || group.indices[2].contains(&MISSING_CORNER)
            })
    }

    pub(crate) fn gtx_attribute_issue(&self) -> Option<&'static str> {
        if self.has_missing_corner_attributes() {
            return Some("Genetica cannot preserve a face with partially supplied UVs or normals.");
        }
        self.0
            .models
            .iter()
            .find_map(ImportedModel::gtx_attribute_issue)
    }

    pub(crate) fn scene(
        &self,
        auto_center: bool,
        auto_scale: bool,
        max_smooth_angle: f64,
        placement: SceneTransform,
        effects: &mut SceneAllocationEffects<'_>,
    ) -> Result<Scene, SceneError> {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for point in self.0.models.iter().flat_map(|model| &model.positions) {
            for (axis, value) in point.values().into_iter().enumerate() {
                min[axis] = min[axis].min(value);
                max[axis] = max[axis].max(value);
            }
        }
        let center = if auto_center && min[0].is_finite() {
            std::array::from_fn(|axis| (min[axis] + max[axis]) / 2.)
        } else {
            [0.; 3]
        };
        let width = (0..3).map(|axis| max[axis] - min[axis]).fold(0., f32::max);
        let scale = if auto_scale && width.is_finite() && width > 0. {
            1. / width
        } else {
            1.
        };
        let lights: Vec<_> = self
            .0
            .lights
            .iter()
            .map(ImportedLight::scene_light)
            .collect();
        let material = SceneMaterial::default();
        {
            let mut data = guard(&material.0);
            data.ambient_enabled = true;
            data.processed_lights = if lights.is_empty() {
                2
            } else {
                i32::try_from(lights.len()).unwrap_or(i32::MAX)
            };
        }
        let [cx, cy, cz] = center;
        let position = |p: MeshVector| {
            placement.point(MeshVector::new(
                (-p.x + cx) * scale,
                (p.y - cy) * scale,
                (p.z - cz) * scale,
            ))
        };
        let models = self
            .0
            .models
            .iter()
            .filter(|model| !model.groups.is_empty())
            .map(|model| {
                model.scene_model(
                    &material,
                    &position,
                    placement,
                    max_smooth_angle.to_radians().cos(),
                    effects,
                )
            })
            .collect::<Result<_, _>>()?;
        Ok(Scene::from_parts(models, lights, None))
    }
}

impl ImportedLight {
    fn scene_light(&self) -> SceneLight {
        let [ambient, diffuse, specular] = self.colors.map(|[r, g, b, _]| [r, g, b]);
        SceneLight::new(SceneLightValues::new(
            self.enabled,
            self.kind,
            self.position,
            ambient,
            diffuse,
            specular,
            self.attenuation,
            self.direction,
            self.cutoff,
            self.exponent,
        ))
    }
}

impl ImportedModel {
    fn gtx_attribute_issue(&self) -> Option<&'static str> {
        // The original camera's normal and tangent preparation assumes that
        // streams regenerated for some groups are available for every group.
        let groups = || {
            self.groups
                .iter()
                .filter(|group| !group.indices[0].is_empty())
        };
        if !groups().any(|group| group.indices[2].is_empty()) {
            return None;
        }
        if groups().any(|group| !group.indices[2].is_empty()) {
            return Some(
                "Genetica cannot render one model with both supplied and generated normals.",
            );
        }
        let has_uvs = groups().any(|group| !group.indices[1].is_empty());
        if has_uvs && self.tangents.is_empty() && groups().any(|group| group.indices[1].is_empty())
        {
            return Some(
                "Genetica cannot render one model with generated normals and UVs on only some faces.",
            );
        }
        if !has_uvs && !self.normals.is_empty() {
            return Some("Genetica cannot render this model's unused normal array with no UVs.");
        }
        None
    }

    fn validate(&self) -> Result<(), SceneError> {
        let lengths = [
            self.positions.len(),
            self.uvs.len(),
            self.normals.len(),
            self.tangents.len(),
        ];
        for group in &self.groups {
            let count = group.indices[0].len();
            match group.kind {
                0 if count.is_multiple_of(3) => {}
                1 | 2 if count >= 3 => {}
                3 if count.is_multiple_of(4) => {}
                4 => return Err(SceneError::UnsupportedAdjustment),
                _ => return Err(SceneError::InvalidProperty),
            }
            for (attribute, (indices, length)) in group.indices.iter().zip(lengths).enumerate() {
                let allows_missing = matches!(attribute, 1 | 2);
                if !indices.is_empty()
                    && (indices.len() != count
                        || indices.iter().any(|&index| {
                            index as usize >= length && !(allows_missing && index == MISSING_CORNER)
                        }))
                {
                    return Err(SceneError::IndexOutOfRange);
                }
            }
        }
        Ok(())
    }

    fn scene_model(
        &self,
        material: &SceneMaterial,
        position: &impl Fn(MeshVector) -> MeshVector,
        placement: SceneTransform,
        smooth: f64,
        effects: &mut SceneAllocationEffects<'_>,
    ) -> Result<MeshModel, SceneError> {
        effects.create::<MeshVector>(self.positions.len(), SceneArrayKind::Vector3F)?;
        let positions = self
            .positions
            .iter()
            .copied()
            .map(position)
            .collect::<Vec<_>>();
        let mut uvs = self.uvs.clone();
        let zero_uv = i32::try_from(uvs.len()).map_err(|_| SceneError::Allocation)?;
        uvs.push(MeshUv::default());
        let mut face_normals = Vec::new();
        let mut face_tangents = Vec::new();
        let mut corners_by_position = vec![Vec::new(); positions.len()];
        let mut groups = Vec::new();
        for group in &self.groups {
            let topology = if group.kind == 3 {
                PolygonTopology::Quads
            } else {
                PolygonTopology::Triangles
            };
            let indices = group.expanded_indices();
            let size = if topology == PolygonTopology::Quads {
                4
            } else {
                3
            };
            let first_face = face_normals.len();
            for (face_index, face) in indices[0].chunks_exact(size).enumerate() {
                let points = std::array::from_fn(|index| positions[face[index] as usize]);
                let [a, b, c] = points;
                face_normals.push(face_normal(a, b, c));
                let offset = face_index * size;
                let tangent = match indices[1].get(offset..offset + size) {
                    Some(uv_indices) if !uv_indices.contains(&MISSING_CORNER) => {
                        let coordinates = std::array::from_fn(|i| uvs[uv_indices[i] as usize]);
                        face_tangent(points, coordinates)
                    }
                    _ => MeshVector::default(),
                };
                face_tangents.push(tangent);
                for &position in face {
                    corners_by_position[position as usize].push(first_face + face_index);
                }
            }
            groups.push((topology, indices, first_face));
        }
        let mut normals: Vec<_> = self
            .normals
            .iter()
            .map(|n| placement.direction(MeshVector::new(-n.x, n.y, n.z)))
            .collect();
        let mut tangents: Vec<_> = self
            .tangents
            .iter()
            .map(|&t| placement.direction(t))
            .collect();
        let mut polygon_groups = Vec::new();
        for (topology, mut indices, first_face) in groups {
            let [positions, uvs, normal_indices, tangent_indices] = &mut indices;
            let count = positions.len();
            if count == 0 {
                continue;
            }
            if uvs.is_empty() {
                uvs.resize(count, zero_uv);
            }
            for index in uvs.iter_mut().filter(|index| **index == MISSING_CORNER) {
                *index = zero_uv;
            }
            if normal_indices.is_empty() {
                normal_indices.resize(count, MISSING_CORNER);
            }
            let size = if topology == PolygonTopology::Quads {
                4
            } else {
                3
            };
            let generate_normals = normal_indices.contains(&MISSING_CORNER);
            let generate_tangents = tangent_indices.is_empty();
            if generate_normals || generate_tangents {
                for (corner, &position) in positions.iter().enumerate() {
                    let face = first_face + corner / size;
                    let base = face_normals[face];
                    let mut normal = MeshVector::default();
                    let mut tangent = MeshVector::default();
                    for &neighbor in &corners_by_position[position as usize] {
                        let candidate = face_normals[neighbor];
                        if neighbor == face
                            || f64::from(base.dot(candidate)) > f64::from(smooth as f32) - 0.001
                        {
                            normal = MeshVector::add(normal, candidate);
                            tangent = MeshVector::add(tangent, face_tangents[neighbor]);
                        }
                    }
                    if normal.length() < 0.0001 {
                        normal = base;
                    }
                    normal.normalize();
                    if normal_indices[corner] == MISSING_CORNER {
                        normal_indices[corner] =
                            i32::try_from(normals.len()).map_err(|_| SceneError::Allocation)?;
                        normals.push(normal);
                    } else {
                        normal = normals[normal_indices[corner] as usize];
                    }
                    if generate_tangents {
                        if tangent.length() < 1e-6 {
                            tangent = face_tangents[face];
                        }
                        tangent = MeshVector::sub(
                            tangent,
                            MeshVector::scale(normal.dot(tangent), normal),
                        );
                        tangent.normalize();
                        tangent_indices.push(
                            i32::try_from(tangents.len()).map_err(|_| SceneError::Allocation)?,
                        );
                        tangents.push(tangent);
                    }
                }
            }
            let count = i32::try_from(count).map_err(|_| SceneError::Allocation)?;
            let polygon = PolygonGroup::with_attributes(topology, count, [false; 3], effects)?;
            for (attribute, values) in [
                MeshAttribute::Position,
                MeshAttribute::Uv,
                MeshAttribute::Normal,
                MeshAttribute::Tangent,
            ]
            .into_iter()
            .zip(indices)
            {
                let store = Array::from_values(values, MeshSeal::Temporary)?;
                polygon.set_indices(attribute, Some(&store))?;
            }
            polygon.set_smooth_angle_cosine(smooth);
            polygon_groups.push(polygon);
        }
        effects.create::<MeshUv>(uvs.len(), SceneArrayKind::UvF32)?;
        effects.create::<MeshVector>(normals.len(), SceneArrayKind::Vector3F)?;
        effects.create::<MeshVector>(tangents.len(), SceneArrayKind::Vector3F)?;
        let positions = Array::from_values(positions, MeshSeal::Temporary)?;
        let uvs = Array::from_values(uvs, MeshSeal::Temporary)?;
        let normals = Array::from_values(normals, MeshSeal::Temporary)?;
        let tangents = Array::from_values(tangents, MeshSeal::Temporary)?;
        Ok(MeshModel(Arc::new(Mutex::new(ModelData {
            positions: Some(positions),
            uvs: Some(uvs),
            normals: Some(normals),
            tangents: Some(tangents),
            groups: polygon_groups,
            material: material.clone(),
        }))))
    }
}

impl ImportedGroup {
    fn expanded_indices(&self) -> [Vec<i32>; 4] {
        if matches!(self.kind, 0 | 3) {
            return self.indices.clone();
        }
        let count = self.indices[0].len();
        std::array::from_fn(|attribute| {
            let indices = &self.indices[attribute];
            if indices.is_empty() {
                return Vec::new();
            }
            (2..count)
                .flat_map(|index| {
                    let start = if self.kind == 1 { 0 } else { index - 2 };
                    [indices[start], indices[start + 1], indices[index]]
                })
                .collect()
        })
    }
}

fn face_normal(a: MeshVector, b: MeshVector, c: MeshVector) -> MeshVector {
    let [ax, ay, az] = a.values().map(f64::from);
    let [bx, by, bz] = b.values().map(f64::from);
    let [cx, cy, cz] = c.values().map(f64::from);
    let [x, y, z] = [bx - ax, by - ay, bz - az];
    let [u, v, w] = [cx - bx, cy - by, cz - bz];
    let [x, y, z] = [y * w - z * v, z * u - x * w, x * v - y * u];
    let length = (x * x + y * y + z * z).sqrt();
    if length == 0. {
        return MeshVector::default();
    }
    // TexTree awx.b negates the normalized cross product for generated faces.
    MeshVector::new(
        (-x / length) as f32,
        (-y / length) as f32,
        (-z / length) as f32,
    )
}

fn face_tangent([a, b, c]: [MeshVector; 3], [uv0, uv1, uv2]: [MeshUv; 3]) -> MeshVector {
    let [u, v] = uv0.values();
    let u1 = f64::from(u - uv1.u());
    let v1 = f64::from(v - uv1.v());
    let u2 = f64::from(u - uv2.u());
    let v2 = f64::from(v - uv2.v());
    let sign = if u1 * v2 - v1 * u2 < 0. { -1. } else { 1. };
    let [x, y, z] = a.values().map(f64::from);
    let [bx, by, bz] = b.values().map(f64::from);
    let [cx, cy, cz] = c.values().map(f64::from);
    let mut tangent = MeshVector::new(
        ((x - bx) * v2 - v1 * (x - cx)) as f32 * sign,
        ((y - by) * v2 - v1 * (y - cy)) as f32 * sign,
        ((z - bz) * v2 - v1 * (z - cz)) as f32 * sign,
    );
    tangent.normalize();
    tangent
}
