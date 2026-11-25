use super::*;
#[path = "primitive_cone.rs"]
mod cone;
#[path = "primitive_cube.rs"]
mod cube;
#[path = "primitive_cylinder.rs"]
mod cylinder;
#[path = "primitive_disc.rs"]
mod disc;
#[path = "primitive_plane.rs"]
mod plane;
#[path = "primitive_sphere.rs"]
mod sphere;
#[path = "primitive_torus.rs"]
mod torus;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScenePrimitiveKind {
    Cone,
    Cube,
    Cylinder,
    Disc,
    Plane,
    Sphere,
    Torus,
}
impl ScenePrimitiveKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cone => "Cone",
            Self::Cube => "Cube",
            Self::Cylinder => "Cylinder",
            Self::Disc => "Disc",
            Self::Plane => "Plane",
            Self::Sphere => "Sphere",
            Self::Torus => "Torus",
        }
    }
    pub const fn all() -> [Self; 7] {
        [
            Self::Cone,
            Self::Cube,
            Self::Cylinder,
            Self::Disc,
            Self::Plane,
            Self::Sphere,
            Self::Torus,
        ]
    }
}
/// Explicit sampled kernel inputs, not an editable property root or graph model.
#[derive(Clone, Copy, Debug)]
pub struct ScenePrimitiveParameters {
    kind: ScenePrimitiveKind,
    segments: [i32; 3],
    number: f64,
    flag: bool,
}
impl ScenePrimitiveParameters {
    pub const fn cone(length: i32, rotational: i32, cap: i32, caps: bool) -> Self {
        Self {
            kind: ScenePrimitiveKind::Cone,
            segments: [length, rotational, cap],
            number: 0.,
            flag: caps,
        }
    }
    pub const fn cube(x: i32, y: i32, z: i32) -> Self {
        Self {
            kind: ScenePrimitiveKind::Cube,
            segments: [x, y, z],
            number: 0.,
            flag: false,
        }
    }
    pub const fn cylinder(length: i32, rotational: i32, cap: i32, taper: f64, caps: bool) -> Self {
        Self {
            kind: ScenePrimitiveKind::Cylinder,
            segments: [length, rotational, cap],
            number: taper,
            flag: caps,
        }
    }
    pub const fn disc(circular: i32, radial: i32, inner_radius: f64, two_sided: bool) -> Self {
        Self {
            kind: ScenePrimitiveKind::Disc,
            segments: [circular, radial, 0],
            number: inner_radius,
            flag: two_sided,
        }
    }
    pub const fn plane(x: i32, y: i32, two_sided: bool) -> Self {
        Self {
            kind: ScenePrimitiveKind::Plane,
            segments: [x, y, 0],
            number: 0.,
            flag: two_sided,
        }
    }
    pub const fn sphere(latitude: i32, longitude: i32) -> Self {
        Self {
            kind: ScenePrimitiveKind::Sphere,
            segments: [latitude, longitude, 0],
            number: 0.,
            flag: false,
        }
    }
    pub const fn torus(ring: i32, pipe: i32, pipe_radius: f64) -> Self {
        Self {
            kind: ScenePrimitiveKind::Torus,
            segments: [ring, pipe, 0],
            number: pipe_radius,
            flag: false,
        }
    }
    pub const fn kind(self) -> ScenePrimitiveKind {
        self.kind
    }
}
/// Actual unit-space aiy/ql mesh cohort. Allocation effects are required at the
/// source SafelyCreateArray sites; this function does not fabricate ambient GC.
pub fn generate_primitive(
    p: ScenePrimitiveParameters,
    effects: &mut SceneAllocationEffects<'_>,
) -> Result<Scene, SceneError> {
    let [a, b, c] = p.segments;
    let model = match p.kind {
        ScenePrimitiveKind::Cone => cone::cone(a, b, c, p.flag, effects)?,
        ScenePrimitiveKind::Cube => cube::cube(a, b, c, effects)?,
        ScenePrimitiveKind::Cylinder => cylinder::cylinder(a, b, c, p.number, p.flag, effects)?,
        ScenePrimitiveKind::Disc => disc::disc(a, b, p.number / 200., p.flag, effects)?,
        ScenePrimitiveKind::Plane => plane::plane(a, b, p.flag, effects)?,
        ScenePrimitiveKind::Sphere => sphere::sphere(a, b, effects)?,
        ScenePrimitiveKind::Torus => torus::torus(a, b, p.number / 100., effects)?,
    };
    let base = SceneTransform::from_components(
        SceneVector::new(1., 1., 1.),
        SceneVector::default(),
        SceneVector::default(),
    );
    match p.kind {
        ScenePrimitiveKind::Cube => model
            .transform(SceneTransform::translation(SceneVector::new(-0.5, -0.5, -0.5)).then(base)),
        ScenePrimitiveKind::Plane => model
            .transform(SceneTransform::translation(SceneVector::new(-0.5, 0., -0.5)).then(base)),
        _ => model.transform(base),
    }?;
    // All these concrete kernels supply both normal and tangent channels, so the
    // reached ue.a OR-condition returns before generating missing face channels.
    Ok(Scene::from_parts(vec![model], Vec::new(), None))
}
struct SourceList<T> {
    values: Vec<T>,
}
impl<T: Copy> SourceList<T> {
    fn new() -> Self {
        Self { values: Vec::new() }
    }
    fn from_values(values: Vec<T>) -> Self {
        Self { values }
    }
    fn count(&self) -> i32 {
        self.values.len() as i32
    }
    fn push(&mut self, value: T) -> Result<(), SceneError> {
        self.values
            .try_reserve(1)
            .map_err(|_| SceneError::Allocation)?;
        self.values.push(value);
        Ok(())
    }
    fn at(&self, ix: i32) -> Result<T, SceneError> {
        self.values
            .get(usize::try_from(ix).map_err(|_| SceneError::IndexOutOfRange)?)
            .copied()
            .ok_or(SceneError::IndexOutOfRange)
    }
}
#[derive(Clone)]
struct Indices(Arc<Mutex<Vec<i32>>>);
impl Indices {
    fn zeros(n: i32) -> Result<Self, SceneError> {
        let n = usize::try_from(n).map_err(|_| SceneError::NegativeCount)?;
        let mut a = Vec::new();
        a.try_reserve_exact(n).map_err(|_| SceneError::Allocation)?;
        a.resize(n, 0);
        Ok(Self::values(a))
    }
    fn values(a: Vec<i32>) -> Self {
        Self(Arc::new(Mutex::new(a)))
    }
    fn at(&self, ix: i32) -> Result<i32, SceneError> {
        guard(&self.0)
            .get(usize::try_from(ix).map_err(|_| SceneError::IndexOutOfRange)?)
            .copied()
            .ok_or(SceneError::IndexOutOfRange)
    }
    fn put(&self, ix: i32, value: i32) -> Result<(), SceneError> {
        *guard(&self.0)
            .get_mut(usize::try_from(ix).map_err(|_| SceneError::IndexOutOfRange)?)
            .ok_or(SceneError::IndexOutOfRange)? = value;
        Ok(())
    }
}
const REVERSE: Reverse = Reverse;
struct Reverse;
impl Reverse {
    fn at(&self, ix: i32) -> Result<i32, SceneError> {
        [1, 0, 3, 2]
            .get(usize::try_from(ix).map_err(|_| SceneError::IndexOutOfRange)?)
            .copied()
            .ok_or(SceneError::IndexOutOfRange)
    }
}
struct MeshBuild {
    groups: Vec<PolygonGroup>,
}
impl MeshBuild {
    fn new() -> Self {
        Self { groups: Vec::new() }
    }
    fn group_count(&self) -> i32 {
        self.groups.len() as i32
    }
    fn group(
        &mut self,
        topology: PolygonTopology,
        count: i32,
        effects: &mut SceneAllocationEffects<'_>,
    ) -> Result<(), SceneError> {
        self.groups.push(PolygonGroup::with_attributes(
            topology, count, [true; 3], effects,
        )?);
        Ok(())
    }
    fn emit(&mut self, group: i32, cursor: &mut i32, attrs: &[&Indices]) -> Result<(), SceneError> {
        let group = self
            .groups
            .get(group as usize)
            .ok_or(SceneError::IndexOutOfRange)?;
        let roots = guard(&group.0).indices.clone();
        let mut buffers = Vec::new();
        for root in roots {
            buffers.push(root.map(|a| a.data(false)).transpose()?);
        }
        let n = guard(&attrs[0].0).len();
        for ix in 0..n {
            for (channel, source) in attrs.iter().enumerate() {
                let value = source.at(ix as i32)?;
                buffers[channel]
                    .as_ref()
                    .ok_or(SceneError::NullReference)?
                    .write(*cursor, value)?;
            }
            *cursor = cursor.wrapping_add(1);
        }
        Ok(())
    }
    fn finish(
        self,
        positions: SourceList<MeshVector>,
        uvs: SourceList<MeshUv>,
        normals: SourceList<MeshVector>,
        tangents: SourceList<MeshVector>,
        safe_directions: bool,
        effects: &mut SceneAllocationEffects<'_>,
    ) -> Result<MeshModel, SceneError> {
        fn copied<T: MeshArrayElement>(
            list: SourceList<T>,
            safe: bool,
            kind: SceneArrayKind,
            effects: &mut SceneAllocationEffects<'_>,
        ) -> Result<Array<T>, SceneError> {
            let n = list.values.len();
            if safe {
                effects.create::<T>(n, kind)?;
            }
            if n == 0 {
                return Err(SceneError::ZeroLengthArray);
            }
            let mut values = Vec::new();
            values
                .try_reserve_exact(n)
                .map_err(|_| SceneError::Allocation)?;
            values.extend_from_slice(&list.values);
            Array::from_values(values, MeshSeal::Temporary)
        }
        let positions = copied(positions, true, SceneArrayKind::Vector3F, effects)?;
        let uvs = copied(uvs, true, SceneArrayKind::UvF32, effects)?;
        let normals = copied(normals, safe_directions, SceneArrayKind::Vector3F, effects)?;
        let tangents = copied(tangents, safe_directions, SceneArrayKind::Vector3F, effects)?;
        for group in &self.groups {
            for array in guard(&group.0).indices.iter().flatten() {
                array.seal(true);
            }
        }
        Ok(MeshModel(Arc::new(Mutex::new(ModelData {
            positions: Some(positions),
            uvs: Some(uvs),
            normals: Some(normals),
            tangents: Some(tangents),
            groups: self.groups,
            material: SceneMaterial::default(),
        }))))
    }
}
#[derive(Clone, Copy)]
struct Sample {
    position: MeshVector,
    uv: MeshUv,
    normal: MeshVector,
    tangent: MeshVector,
}
fn cos(x: f64) -> f64 {
    x.cos()
}
fn sin(x: f64) -> f64 {
    x.sin()
}
trait Cast: Copy {
    fn sf(self) -> f32;
    fn sd(self) -> f64;
    fn sign(self) -> Result<i32, SceneError>;
}
macro_rules! casts {
    ($t:ty) => {
        impl Cast for $t {
            fn sf(self) -> f32 {
                self as f32
            }
            fn sd(self) -> f64 {
                self as f64
            }
            fn sign(self) -> Result<i32, SceneError> {
                if self.is_nan() {
                    Err(SceneError::Arithmetic)
                } else {
                    Ok(if self > 0. {
                        1
                    } else if self < 0. {
                        -1
                    } else {
                        0
                    })
                }
            }
        }
    };
}
casts!(f32);
casts!(f64);
impl Cast for i32 {
    fn sf(self) -> f32 {
        self as f32
    }
    fn sd(self) -> f64 {
        f64::from(self)
    }
    fn sign(self) -> Result<i32, SceneError> {
        Ok(self.signum())
    }
}
fn sf<T: Cast>(x: T) -> f32 {
    x.sf()
}
fn sd<T: Cast>(x: T) -> f64 {
    x.sd()
}
fn sgn<T: Cast>(x: T) -> Result<i32, SceneError> {
    x.sign()
}
fn tangent(n: MeshVector) -> Result<MeshVector, SceneError> {
    Ok(if f64::from(n.z.abs()) > 0.0001 {
        MeshVector::new(-sgn(n.z)? as f32, 0., sgn(n.x)? as f32 * (n.x / n.z).abs()).unit()
    } else {
        MeshVector::new(0., 0., sgn(n.x)? as f32).unit()
    })
}
fn cone_side(y: f64, radius: f64, angle: f64, caps: bool) -> Result<Sample, SceneError> {
    let a = angle / 180. * std::f64::consts::PI;
    let c = cos(a);
    let s = sin(a);
    let v = (0.5 - y) as f32;
    let v = if caps {
        0.01 + v * 0.48 + 0.5
    } else {
        0.01 + v * 0.98
    };
    let normal = MeshVector::new((0.5 * c) as f32, 0.5, (0.5 * s) as f32).unit();
    Ok(Sample {
        position: MeshVector::new((radius * c) as f32, y as f32, (radius * s) as f32),
        uv: MeshUv::new(0.01 + (angle / 360.) as f32 * 0.98, v),
        normal,
        tangent: tangent(normal)?,
    })
}
fn cone_cap(y: f64, radius: f64, angle: f64) -> Result<Sample, SceneError> {
    cap_sample(y, radius, angle)
}
fn cylinder_cap(y: f64, radius: f64, angle: f64) -> Result<Sample, SceneError> {
    cap_sample(y, radius, angle)
}
fn cap_sample(y: f64, radius: f64, angle: f64) -> Result<Sample, SceneError> {
    let a = angle / 180. * std::f64::consts::PI;
    let x = (radius * cos(a)) as f32;
    let z = (radius * sin(a)) as f32;
    let position = MeshVector::new(x, y as f32, z);
    Ok(Sample {
        position,
        uv: MeshUv::new(
            if y < 0. {
                0.75 - x * 0.48
            } else {
                0.25 + x * 0.48
            },
            -z * 0.48 + 0.25,
        ),
        normal: MeshVector::new(0., sgn(y)? as f32, 0.),
        tangent: tangent(position)?,
    })
}
fn cylinder_side(
    y: f64,
    radius: f64,
    angle: f64,
    taper: f64,
    caps: bool,
) -> Result<Sample, SceneError> {
    let a = angle / 180. * std::f64::consts::PI;
    let c = cos(a);
    let s = sin(a);
    let v = (0.5 - y) as f32;
    let t = taper / 100.;
    let normal = if t > 0. {
        let q = t * t / (t * t + 1.);
        let r = t / (t * t + 1.);
        MeshVector::new((r * c) as f32, q as f32, (r * s) as f32).unit()
    } else {
        MeshVector::new(c as f32, 0., s as f32).unit()
    };
    Ok(Sample {
        position: MeshVector::new((radius * c) as f32, y as f32, (radius * s) as f32),
        uv: MeshUv::new(
            0.01 + (angle / 360.) as f32 * 0.98,
            if caps {
                0.01 + v * 0.48 + 0.5
            } else {
                0.01 + v * 0.98
            },
        ),
        normal,
        tangent: tangent(normal)?,
    })
}
fn sphere_sample(latitude: f64, longitude: f64) -> Result<Sample, SceneError> {
    let a = latitude / 180. * std::f64::consts::PI;
    let b = longitude / 180. * std::f64::consts::PI;
    let position = MeshVector::new(
        (0.5 * cos(b) * sin(a)) as f32,
        (0.5 * cos(a)) as f32,
        (0.5 * sin(b) * sin(a)) as f32,
    );
    let normal = position.unit();
    let tangent = if f64::from(normal.z.abs()) > 0.0001 {
        MeshVector::new(
            -sgn(normal.z)? as f32,
            0.,
            sgn(normal.x)? as f32 * (normal.x / normal.z).abs(),
        )
        .unit()
    } else {
        MeshVector::new(0., 0., sgn(normal.x)? as f32)
    };
    Ok(Sample {
        position,
        uv: MeshUv::new((longitude / 360.) as f32, (latitude / 180.) as f32),
        normal,
        tangent,
    })
}
fn torus_add(
    positions: &mut SourceList<MeshVector>,
    normals: &mut SourceList<MeshVector>,
    tangents: &mut SourceList<MeshVector>,
    ring: f64,
    pipe: f64,
    radius: f64,
) -> Result<i32, SceneError> {
    let a = ring / 180. * std::f64::consts::PI;
    let b = pipe / 180. * std::f64::consts::PI;
    let c = cos(a);
    let s = sin(a);
    let u = cos(-b);
    let v = sin(-b);
    let r = 0.5_f32 + (u * radius) as f32;
    positions.push(MeshVector::new(
        (f64::from(r) * c) as f32,
        (radius * v) as f32,
        (f64::from(r) * s) as f32,
    ))?;
    normals.push(MeshVector::new((u * c) as f32, v as f32, (u * s) as f32))?;
    tangents.push(MeshVector::new(-s as f32, 0., c as f32))?;
    Ok(positions.count().wrapping_sub(1))
}
fn torus_uv_add(uvs: &mut SourceList<MeshUv>, ring: f64, pipe: f64) -> Result<i32, SceneError> {
    uvs.push(MeshUv::new(
        0.01 + 0.98 * (ring / 360.) as f32,
        0.01 + 0.98 * (pipe / 360.) as f32,
    ))?;
    Ok(uvs.count().wrapping_sub(1))
}
fn disc_add(
    positions: &mut SourceList<MeshVector>,
    uvs: &mut SourceList<MeshUv>,
    radius: f64,
    angle: f64,
    two: bool,
) -> Result<(i32, i32), SceneError> {
    let a = angle / 180. * std::f64::consts::PI;
    let x = (radius * cos(a)) as f32;
    let z = (radius * sin(a)) as f32;
    uvs.push(MeshUv::new(x * 0.98 + 0.5, -z * 0.98 + 0.5))?;
    let ix = positions.count();
    if two {
        positions.push(MeshVector::new(x, 0.00005, z))?;
        positions.push(MeshVector::new(x, -0.00005, z))?;
    } else {
        positions.push(MeshVector::new(x, 0., z))?;
    }
    Ok((ix, uvs.count().wrapping_sub(1)))
}

#[cfg(test)]
mod signed_group_laws {
    use super::*;
    #[test]
    fn signed_nonpositive_group_has_absent_attributes_and_null_write() {
        for count in [0, -8] {
            let mut requests = Vec::new();
            let mut admit = |r| {
                requests.push(r);
                Ok(())
            };
            let mut build = MeshBuild::new();
            build
                .group(
                    PolygonTopology::Quads,
                    count,
                    &mut SceneAllocationEffects::new(&mut admit),
                )
                .unwrap();
            let group = &build.groups[0];
            assert_eq!(group.signed_corner_count(), count);
            for a in [
                MeshAttribute::Position,
                MeshAttribute::Uv,
                MeshAttribute::Normal,
                MeshAttribute::Tangent,
            ] {
                assert!(!group.has_attribute(a));
                assert_eq!(group.index_len(a), 0);
                assert_eq!(group.seal(a), None);
            }
            let input = Indices::values(vec![0, 0, 0, 0]);
            let mut cursor = 0;
            assert_eq!(
                build
                    .emit(0, &mut cursor, &[&input, &input, &input, &input])
                    .unwrap_err(),
                SceneError::NullReference
            );
            assert_eq!(cursor, 0);
            assert!(requests.is_empty());
        }
        assert_eq!(Indices::zeros(-1).err(), Some(SceneError::NegativeCount));
    }
}
