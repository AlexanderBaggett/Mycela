use super::{
    ImportedGroup, ImportedLight, ImportedMesh, ImportedModel, MISSING_CORNER, MeshUv, MeshVector,
};
use crate::{BinaryCursor, BinaryError};

impl ImportedMesh {
    pub fn from_obj(source: &str) -> Result<Self, String> {
        let mut model = ImportedModel::default();
        let mut triangles = Vec::new();
        let mut quads = Vec::new();
        for (line_number, line) in source.trim_start_matches('\u{feff}').lines().enumerate() {
            let mut fields = line
                .split('#')
                .next()
                .unwrap_or("")
                .split_ascii_whitespace();
            let Some(head) = fields.next() else {
                continue;
            };
            let result = (|| {
                match head.to_ascii_lowercase().as_str() {
                    "v" => model.positions.push(obj_vector(&mut fields)?),
                    "vn" => model.normals.push(obj_vector(&mut fields)?),
                    "vt" => model.uvs.push(MeshUv::new(
                        obj_float(fields.next())?,
                        1. - obj_float(fields.next())?,
                    )),
                    "f" => {
                        let lengths = [model.positions.len(), model.uvs.len(), model.normals.len()];
                        let face = fields
                            .map(|field| obj_corner(field, lengths))
                            .collect::<Result<Vec<_>, _>>()?;
                        match face.len() {
                            0..=2 => return Err("A face needs at least three corners"),
                            4 => add_obj_face(&mut quads, 3, &face),
                            _ => {
                                for index in 2..face.len() {
                                    add_obj_face(
                                        &mut triangles,
                                        0,
                                        &[face[0], face[index - 1], face[index]],
                                    );
                                }
                            }
                        }
                    }
                    _ => {}
                }
                Ok(())
            })();
            result.map_err(|error| format!("{error} on line {}", line_number + 1))?;
        }
        model.groups.extend(triangles.into_iter().chain(quads));
        if model.groups.is_empty() {
            return Err("The OBJ file contains no faces".into());
        }
        Self::new(vec![model], Vec::new()).map_err(|error| error.to_string())
    }

    pub fn from_3ds(source: &[u8]) -> Result<Self, String> {
        read_3ds(source).map_err(|error| format!("Invalid 3DS file ({error})"))
    }
}

fn obj_float(field: Option<&str>) -> Result<f32, &'static str> {
    field
        .and_then(|value| value.parse::<f64>().ok())
        .map(|value| value as f32)
        .filter(|value| value.is_finite())
        .ok_or("Invalid coordinate")
}

fn obj_vector(fields: &mut dyn Iterator<Item = &str>) -> Result<MeshVector, &'static str> {
    Ok(MeshVector::new(
        obj_float(fields.next())?,
        obj_float(fields.next())?,
        obj_float(fields.next())?,
    ))
}

fn obj_corner(field: &str, lengths: [usize; 3]) -> Result<[Option<i32>; 3], &'static str> {
    let mut indices = [None; 3];
    let mut fields = field.split('/');
    for (attribute, length) in lengths.into_iter().enumerate() {
        let Some(field) = fields.next().filter(|field| !field.is_empty()) else {
            if attribute == 0 {
                return Err("Missing position index");
            }
            continue;
        };
        let index: i32 = field.parse().map_err(|_| "Invalid face index")?;
        let at = match index {
            1.. => index - 1,
            ..=-1 => i32::try_from(length)
                .unwrap_or(i32::MAX)
                .checked_add(index)
                .filter(|&at| at >= 0)
                .ok_or("Relative index exceeds the current coordinates")?,
            0 => return Err("OBJ indices start at one"),
        };
        indices[attribute] = Some(at);
    }
    if fields.next().is_some() {
        return Err("Too many indices for a face corner");
    }
    Ok(indices)
}

impl ImportedGroup {
    fn new(kind: i32) -> Self {
        Self {
            kind,
            indices: std::array::from_fn(|_| Vec::new()),
            smooth: 1.,
        }
    }
}

fn add_obj_face(groups: &mut Vec<ImportedGroup>, kind: i32, face: &[[Option<i32>; 3]]) {
    // Keep the source face order within each topology. A group whose faces
    // omit an entire stream can use the original, portable GTX representation.
    let streams: [bool; 3] =
        std::array::from_fn(|attribute| face.iter().any(|corner| corner[attribute].is_some()));
    if groups.last().is_none_or(|group| {
        group
            .indices
            .iter()
            .zip(streams)
            .any(|(indices, present)| indices.is_empty() == present)
    }) {
        groups.push(ImportedGroup::new(kind));
    }
    let group = groups.last_mut().expect("a group for the face");
    for (attribute, present) in streams.into_iter().enumerate() {
        if present {
            group.indices[attribute].extend(
                face.iter()
                    .map(|corner| corner[attribute].unwrap_or(MISSING_CORNER)),
            );
        }
    }
}

fn read_3ds(source: &[u8]) -> Result<ImportedMesh, BinaryError> {
    let mut source = source;
    let (kind, mut contents) = chunk(&mut source)?;
    if kind != 0x4d4d {
        return Err(BinaryError::LengthEncoding);
    }
    let mut models = Vec::new();
    let mut lights = Vec::new();
    while !contents.is_empty() {
        let (kind, body) = chunk(&mut contents)?;
        match kind {
            0x0002 if BinaryCursor::new(body)?.read_i32()? < 3 => {
                return Err(BinaryError::LengthEncoding);
            }
            0x3d3d => read_objects(body, &mut models, &mut lights)?,
            0x4000 => read_object(body, &mut models, &mut lights)?,
            _ => {}
        }
    }
    if models.is_empty() && lights.is_empty() {
        return Err(BinaryError::Count);
    }
    ImportedMesh::new(models, lights).map_err(|_| BinaryError::Index)
}

fn chunk<'a>(source: &mut &'a [u8]) -> Result<(u16, &'a [u8]), BinaryError> {
    let header = source.first_chunk::<6>().ok_or(BinaryError::End)?;
    let kind = u16::from_le_bytes([header[0], header[1]]);
    let length = u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as usize;
    let body = source.get(6..length).ok_or(BinaryError::LengthEncoding)?;
    *source = &source[length..];
    Ok((kind, body))
}

fn read_objects(
    mut contents: &[u8],
    models: &mut Vec<ImportedModel>,
    lights: &mut Vec<ImportedLight>,
) -> Result<(), BinaryError> {
    while !contents.is_empty() {
        let (kind, body) = chunk(&mut contents)?;
        if kind == 0x4000 {
            read_object(body, models, lights)?;
        }
    }
    Ok(())
}

fn read_object(
    contents: &[u8],
    models: &mut Vec<ImportedModel>,
    lights: &mut Vec<ImportedLight>,
) -> Result<(), BinaryError> {
    let name_end = contents
        .iter()
        .position(|&byte| byte == 0)
        .ok_or(BinaryError::End)?;
    let mut contents = &contents[name_end + 1..];
    while !contents.is_empty() {
        let (kind, body) = chunk(&mut contents)?;
        match kind {
            0x4100 => {
                let model = read_model(body)?;
                if !model.groups.is_empty() {
                    models.push(model);
                }
            }
            0x4600 => lights.push(read_light(body)?),
            _ => {}
        }
    }
    Ok(())
}

fn read_vector(reader: &mut BinaryCursor<'_>) -> Result<MeshVector, BinaryError> {
    Ok(MeshVector::new(
        reader.read_f32()?,
        reader.read_f32()?,
        reader.read_f32()?,
    ))
}

fn read_model(mut contents: &[u8]) -> Result<ImportedModel, BinaryError> {
    let mut model = ImportedModel::default();
    let mut faces = ImportedGroup::new(0);
    let mut has_uv_chunk = false;
    while !contents.is_empty() {
        let (kind, body) = chunk(&mut contents)?;
        let mut reader = BinaryCursor::new(body)?;
        match kind {
            0x4110 => {
                let count = reader.read_i16()? as u16;
                for _ in 0..count {
                    model.positions.push(read_vector(&mut reader)?);
                }
            }
            0x4120 => {
                let count = reader.read_i16()? as u16;
                for _ in 0..count {
                    for _ in 0..3 {
                        faces.indices[0].push(i32::from(reader.read_i16()? as u16));
                    }
                    reader.read_i16()?;
                }
            }
            0x4140 => {
                has_uv_chunk = true;
                let count = reader.read_i16()? as u16;
                for _ in 0..count {
                    model
                        .uvs
                        .push(MeshUv::new(reader.read_f32()?, reader.read_f32()?));
                }
            }
            _ => {}
        }
    }
    // Mapping coordinates follow position indices in 3DS. Both lists may come
    // after the faces, so resolve them only after all siblings have been read.
    if has_uv_chunk {
        if model.uvs.len() != model.positions.len() {
            return Err(BinaryError::Count);
        }
        faces.indices[1] = faces.indices[0].clone();
    }
    if !faces.indices[0].is_empty() {
        model.groups.push(faces);
    }
    Ok(model)
}

fn read_light(contents: &[u8]) -> Result<ImportedLight, BinaryError> {
    let position = read_vector(&mut BinaryCursor::new(contents)?)?;
    let mut contents = contents.get(12..).ok_or(BinaryError::End)?;
    let (kind, color) = chunk(&mut contents)?;
    if kind != 0x0010 {
        return Err(BinaryError::LengthEncoding);
    }
    let [r, g, b] = read_vector(&mut BinaryCursor::new(color)?)?.values();
    let mut light = ImportedLight {
        enabled: true,
        position,
        colors: [[r, g, b, 1.]; 3],
        attenuation: [1., 0., 0.],
        direction: MeshVector::default(),
        cutoff: 180.,
        exponent: 1.,
        kind: 0,
    };
    while !contents.is_empty() {
        let (kind, body) = chunk(&mut contents)?;
        if kind == 0x4610 {
            let mut reader = BinaryCursor::new(body)?;
            light.direction = read_vector(&mut reader)?;
            reader.read_f32()?;
            light.cutoff = reader.read_f32()?;
            light.kind = 2;
        } else {
            light.kind = 1;
        }
    }
    Ok(light)
}
