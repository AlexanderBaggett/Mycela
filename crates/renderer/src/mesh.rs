//! Transient uploads from the actual independently indexed mesh owners.
use super::{SceneRenderError, SceneRenderRequest, admit};
use mycela_engine::{
    MeshAttribute, MeshModel, MeshVector, PolygonTopology, Scene, SceneLightValues, SceneMaterial,
    SceneTransform, UvLightSettings, UvLightShadow,
};
fn put(bytes: &mut Vec<u8>, values: impl IntoIterator<Item = f32>) {
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}
fn allocate<E>(size: usize) -> Result<Vec<u8>, SceneRenderError<E>> {
    admit(size as u64)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| SceneRenderError::Allocation)?;
    Ok(bytes)
}
fn add_size<E>(total: &mut usize, count: usize, width: usize) -> Result<(), SceneRenderError<E>> {
    *total = total
        .checked_add(
            count
                .checked_mul(width)
                .ok_or(SceneRenderError::InvalidMesh)?,
        )
        .ok_or(SceneRenderError::InvalidMesh)?;
    Ok(())
}
pub(super) fn vertices<E>(
    model: &MeshModel,
    request: &SceneRenderRequest<'_>,
) -> Result<Vec<u8>, SceneRenderError<E>> {
    request.check()?;
    let groups = model.groups();
    let mut working = 0usize;
    let mut corners_count = 0usize;
    for (attribute, width) in [
        (MeshAttribute::Position, 12),
        (MeshAttribute::Normal, 12),
        (MeshAttribute::Tangent, 12),
        (MeshAttribute::Uv, 8),
    ] {
        add_size(&mut working, model.attribute_len(attribute), width)?;
    }
    for group in &groups {
        request.check()?;
        for attribute in [
            MeshAttribute::Position,
            MeshAttribute::Normal,
            MeshAttribute::Tangent,
            MeshAttribute::Uv,
        ] {
            add_size(&mut working, group.index_len(attribute), 4)?;
        }
        let n = group.index_len(MeshAttribute::Position);
        let triangles = match group.topology() {
            PolygonTopology::Triangles => n,
            PolygonTopology::Quads => (n / 4)
                .checked_mul(6)
                .ok_or(SceneRenderError::InvalidMesh)?,
        };
        corners_count = corners_count
            .checked_add(triangles)
            .ok_or(SceneRenderError::InvalidMesh)?;
    }
    add_size(&mut working, corners_count, 48)?; // actual corner i32 list plus expanded 44-byte vertices
    admit(working as u64)?;
    request.check()?;
    // These are fallible readonly copies of the existing roots. They do not source-copy or promote seals.
    let positions = model.try_positions().map_err(SceneRenderError::Scene)?;
    request.check()?;
    let normals = model.try_normals().map_err(SceneRenderError::Scene)?;
    request.check()?;
    let tangents = model.try_tangents().map_err(SceneRenderError::Scene)?;
    request.check()?;
    let uvs = model.try_uvs().map_err(SceneRenderError::Scene)?;
    request.check()?;
    let mut bytes = allocate(
        corners_count
            .checked_mul(44)
            .ok_or(SceneRenderError::InvalidMesh)?,
    )?;
    for group in groups {
        request.check()?;
        let mut corners = group.triangle_corners().map_err(SceneRenderError::Scene)?;
        if group.topology() == PolygonTopology::Quads {
            // Source OpenGL camera coverage uses the second-to-fourth corner diagonal.
            for quad in corners.chunks_exact_mut(6) {
                quad[2] = quad[5];
                quad[3] = quad[1];
            }
        }
        let position_indices = group
            .try_indices(MeshAttribute::Position)
            .map_err(SceneRenderError::Scene)?;
        request.check()?;
        let normal_indices = group
            .try_indices(MeshAttribute::Normal)
            .map_err(SceneRenderError::Scene)?;
        request.check()?;
        let tangent_indices = group
            .try_indices(MeshAttribute::Tangent)
            .map_err(SceneRenderError::Scene)?;
        request.check()?;
        let uv_indices = group
            .try_indices(MeshAttribute::Uv)
            .map_err(SceneRenderError::Scene)?;
        request.check()?;
        let attributes = [
            position_indices,
            normal_indices,
            tangent_indices,
            uv_indices,
        ];
        for (ix, corner) in corners.into_iter().enumerate() {
            if ix % 1024 == 0 {
                request.check()?;
            }
            let corner = usize::try_from(corner).map_err(|_| SceneRenderError::InvalidMesh)?;
            let mut indices = [0usize; 4];
            for (source, destination) in attributes.iter().zip(&mut indices) {
                *destination =
                    usize::try_from(*source.get(corner).ok_or(SceneRenderError::InvalidMesh)?)
                        .map_err(|_| SceneRenderError::InvalidMesh)?;
            }
            put(
                &mut bytes,
                positions
                    .get(indices[0])
                    .ok_or(SceneRenderError::InvalidMesh)?
                    .values(),
            );
            put(
                &mut bytes,
                normals
                    .get(indices[1])
                    .ok_or(SceneRenderError::InvalidMesh)?
                    .values(),
            );
            put(
                &mut bytes,
                tangents
                    .get(indices[2])
                    .ok_or(SceneRenderError::InvalidMesh)?
                    .values(),
            );
            put(
                &mut bytes,
                uvs.get(indices[3])
                    .ok_or(SceneRenderError::InvalidMesh)?
                    .values(),
            );
        }
    }
    request.check()?;
    Ok(bytes)
}
pub(super) fn frame<E>(
    material: &SceneMaterial,
    camera: &super::CameraUpload,
    previous: bool,
    textured: bool,
    uv_light: bool,
    quantize_source_rgb: bool,
) -> Result<Vec<u8>, SceneRenderError<E>> {
    let count = material.processed_lights();
    if count <= 0 {
        return Err(SceneRenderError::UnsupportedFeature(
            "nonpositive processed-light configuration",
        ));
    }
    let mut bytes = allocate(256)?;
    put(&mut bytes, camera.view.values().map(|x| x as f32));
    put(&mut bytes, camera.composed.values().map(|x| x as f32));
    for row in camera.normal.chunks_exact(3) {
        put(&mut bytes, row.iter().map(|x| *x as f32));
        put(&mut bytes, [0.]);
    }
    put(&mut bytes, material.ambient_rgba());
    put(&mut bytes, material.diffuse_rgba());
    put(&mut bytes, material.specular_rgba());
    put(
        &mut bytes,
        [
            material.shininess(),
            material.transparency(),
            f32::from(quantize_source_rgb),
            0.,
        ],
    );
    for value in [
        count as u32,
        u32::from(previous && material.is_depth_peeling_enabled()),
        u32::from(textured),
        u32::from(uv_light),
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    debug_assert_eq!(bytes.len(), 256);
    Ok(bytes)
}

pub(super) fn uv_light<E>(
    light: UvLightSettings,
    shadow: Option<&UvLightShadow>,
    view: SceneTransform,
) -> Result<Vec<u8>, SceneRenderError<E>> {
    let mut bytes = allocate(112)?;
    let [x, y, z] = light.position().map(f64::from);
    let [x, y, z, w] = view.transform_homogeneous([x, y, z, 1.]);
    put(
        &mut bytes,
        [(x / w) as f32, (y / w) as f32, (z / w) as f32, 1.],
    );
    let shadow_strength = shadow.map_or(0., |_| light.shadow_strength());
    put(
        &mut bytes,
        [light.global_illumination(), shadow_strength, 0., 0.],
    );
    // UV Light uses the next four storage vectors for the shadow matrix.
    let transform = shadow.map(UvLightShadow::transform).unwrap_or_default();
    put(&mut bytes, transform.values().map(|x| x as f32));
    put(&mut bytes, [0.; 4]);
    Ok(bytes)
}
pub(super) fn lights<E>(
    scene: &Scene,
    material: &SceneMaterial,
    view: SceneTransform,
    request: &SceneRenderRequest<'_>,
) -> Result<Vec<u8>, SceneRenderError<E>> {
    let count = usize::try_from(material.processed_lights())
        .map_err(|_| SceneRenderError::InvalidGeometry)?;
    if count == 0 {
        return Err(SceneRenderError::UnsupportedFeature(
            "nonpositive processed-light configuration",
        ));
    }
    let mut bytes = allocate(
        count
            .checked_mul(112)
            .ok_or(SceneRenderError::InvalidGeometry)?,
    )?;
    let source = scene.lights();
    let default = source.is_empty().then(|| {
        SceneLightValues::new(
            true,
            0,
            MeshVector::new(0.2, 0.7, -1.5),
            [0.1; 3],
            [0.7; 3],
            [0.2; 3],
            [1., 0., 0.],
            MeshVector::new(-0.2, -0.7, 1.5),
            25.,
            1.,
        )
    });
    let mut active = 0;
    for light in default
        .into_iter()
        .chain(source.iter().map(|light| light.values()))
    {
        request.check()?;
        if !light.is_enabled() {
            continue;
        }
        if active == count {
            break;
        }
        active += 1;
        let p = light.position().values().map(f64::from);
        let directional = light.kind() == 0;
        let mut p =
            view.transform_homogeneous([p[0], p[1], p[2], if directional { 0. } else { 1. }]);
        if !directional {
            let scale = 1. / p[3];
            p[0] *= scale;
            p[1] *= scale;
            p[2] *= scale;
            p[3] = 1.;
        }
        put(&mut bytes, p.map(|x| x as f32));
        for rgb in [light.ambient(), light.diffuse(), light.specular()] {
            put(&mut bytes, rgb);
            put(&mut bytes, [1.]);
        }
        let d = light.spot_direction().values().map(f64::from);
        let d = view.transform_homogeneous([d[0], d[1], d[2], 0.]);
        let spot = light.kind() == 2;
        put(
            &mut bytes,
            [
                d[0] as f32,
                d[1] as f32,
                d[2] as f32,
                if spot { light.spot_cutoff() } else { 180. },
            ],
        );
        let [constant, linear, quadratic] = light.attenuation();
        put(
            &mut bytes,
            [constant, linear, quadratic, light.spot_exponent()],
        );
        put(
            &mut bytes,
            [
                if spot {
                    (f64::from(light.spot_cutoff()) * std::f64::consts::PI / 180.).cos() as f32
                } else {
                    -1.
                },
                0.,
                0.,
                0.,
            ],
        );
    }
    // Unwritten uniforms on this newly supplied program frame remain zero.
    let total = count
        .checked_mul(112)
        .ok_or(SceneRenderError::InvalidGeometry)?;
    while bytes.len() < total {
        request.check()?;
        let end = bytes.len().saturating_add(112 * 1024).min(total);
        bytes.resize(end, 0);
    }
    request.check()?;
    Ok(bytes)
}
