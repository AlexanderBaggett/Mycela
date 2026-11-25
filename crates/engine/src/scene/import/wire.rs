use super::{ImportedGroup, ImportedLight, ImportedMesh, ImportedModel};
use crate::{BinaryCursor, BinaryError, BinaryWriter, MeshUv, MeshVector};

fn count(reader: &mut BinaryCursor<'_>, minimum_bytes: i32) -> Result<usize, BinaryError> {
    let count = reader.read_i32()?;
    if count < 0 {
        return Err(BinaryError::NegativeLength);
    }
    if count > (reader.length()? - reader.position()?) / minimum_bytes {
        return Err(BinaryError::End);
    }
    Ok(count as usize)
}

fn fixed<T, const N: usize>(
    mut read: impl FnMut() -> Result<T, BinaryError>,
) -> Result<[T; N], BinaryError> {
    let mut values = [const { None }; N];
    for value in &mut values {
        *value = Some(read()?);
    }
    Ok(values.map(|value| value.expect("each element was read")))
}

fn array<T>(
    count: usize,
    mut read: impl FnMut() -> Result<T, BinaryError>,
) -> Result<Vec<T>, BinaryError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| BinaryError::Allocation)?;
    for _ in 0..count {
        values.push(read()?);
    }
    Ok(values)
}

fn vector(reader: &mut BinaryCursor<'_>) -> Result<MeshVector, BinaryError> {
    let [x, y, z] = fixed(|| reader.read_f32())?;
    Ok(MeshVector::new(x, y, z))
}

fn put_count(writer: &mut BinaryWriter, count: usize) -> Result<(), BinaryError> {
    writer.write_i32(i32::try_from(count).map_err(|_| BinaryError::Count)?)
}

fn put_floats(writer: &mut BinaryWriter, floats: &[f32]) -> Result<(), BinaryError> {
    for &value in floats {
        writer.write_f32(value)?;
    }
    Ok(())
}

fn put_vectors(writer: &mut BinaryWriter, values: &[MeshVector]) -> Result<(), BinaryError> {
    put_count(writer, values.len())?;
    for value in values {
        put_floats(writer, &value.values())?;
    }
    Ok(())
}

impl ImportedMesh {
    pub(crate) fn read_wire(
        reader: &mut BinaryCursor<'_>,
        project: bool,
    ) -> Result<Self, BinaryError> {
        let models = array(count(reader, 20)?, || ImportedModel::read(reader))?;
        let lights = array(count(reader, 93)?, || ImportedLight::read(reader))?;
        let mesh = Self::new(models, lights).map_err(|_| BinaryError::Index)?;
        if !project && mesh.has_missing_corner_attributes() {
            return Err(BinaryError::Index);
        }
        Ok(mesh)
    }

    pub(crate) fn write_wire(
        &self,
        writer: &mut BinaryWriter,
        project: bool,
    ) -> Result<(), BinaryError> {
        if !project && self.has_missing_corner_attributes() {
            return Err(BinaryError::Index);
        }
        put_count(writer, self.0.models.len())?;
        for model in &self.0.models {
            model.write(writer)?;
        }
        put_count(writer, self.0.lights.len())?;
        for light in &self.0.lights {
            light.write(writer)?;
        }
        Ok(())
    }
}

impl ImportedModel {
    fn read(reader: &mut BinaryCursor<'_>) -> Result<Self, BinaryError> {
        let groups = array(count(reader, 19)?, || ImportedGroup::read(reader))?;
        let positions = array(count(reader, 12)?, || vector(reader))?;
        let uvs = array(count(reader, 8)?, || {
            Ok(MeshUv::new(reader.read_f32()?, reader.read_f32()?))
        })?;
        let normals = array(count(reader, 12)?, || vector(reader))?;
        let tangents = array(count(reader, 12)?, || vector(reader))?;
        Ok(Self {
            groups,
            positions,
            uvs,
            normals,
            tangents,
        })
    }

    fn write(&self, writer: &mut BinaryWriter) -> Result<(), BinaryError> {
        put_count(writer, self.groups.len())?;
        for group in &self.groups {
            group.write(writer)?;
        }
        put_vectors(writer, &self.positions)?;
        put_count(writer, self.uvs.len())?;
        for uv in &self.uvs {
            put_floats(writer, &uv.values())?;
        }
        put_vectors(writer, &self.normals)?;
        put_vectors(writer, &self.tangents)
    }
}

impl ImportedGroup {
    fn read(reader: &mut BinaryCursor<'_>) -> Result<Self, BinaryError> {
        let kind = reader.read_i32()?;
        let count = count(reader, 4)?;
        let mut indices = std::array::from_fn(|_| Vec::new());
        for (attribute, values) in indices.iter_mut().enumerate() {
            if attribute == 0 || reader.read_bool()? {
                if count > ((reader.length()? - reader.position()?) / 4) as usize {
                    return Err(BinaryError::End);
                }
                *values = array(count, || reader.read_i32())?;
            }
        }
        Ok(Self {
            kind,
            indices,
            smooth: reader.read_f64()?,
        })
    }

    fn write(&self, writer: &mut BinaryWriter) -> Result<(), BinaryError> {
        writer.write_i32(self.kind)?;
        put_count(writer, self.indices[0].len())?;
        for (attribute, indices) in self.indices.iter().enumerate() {
            if attribute != 0 {
                writer.write_bool(!indices.is_empty())?;
            }
            for &index in indices {
                writer.write_i32(index)?;
            }
        }
        writer.write_f64(self.smooth)
    }
}

impl ImportedLight {
    fn read(reader: &mut BinaryCursor<'_>) -> Result<Self, BinaryError> {
        Ok(Self {
            enabled: reader.read_bool()?,
            position: vector(reader)?,
            colors: fixed(|| fixed(|| reader.read_f32()))?,
            attenuation: fixed(|| reader.read_f32())?,
            direction: vector(reader)?,
            cutoff: reader.read_f32()?,
            exponent: reader.read_f32()?,
            // zd.Save omits the light type. The loaded object uses its constructor default.
            kind: 0,
        })
    }

    fn write(&self, writer: &mut BinaryWriter) -> Result<(), BinaryError> {
        writer.write_bool(self.enabled)?;
        put_floats(writer, &self.position.values())?;
        for color in self.colors {
            put_floats(writer, &color)?;
        }
        put_floats(writer, &self.attenuation)?;
        put_floats(writer, &self.direction.values())?;
        writer.write_f32(self.cutoff)?;
        writer.write_f32(self.exponent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_native_uv_and_normal_streams_may_encode_an_absent_corner() {
        for (attribute, index) in [(0, -1), (1, -1), (2, -1), (3, -1), (2, -2)] {
            let mut indices = std::array::from_fn(|_| vec![0; 3]);
            indices[attribute][1] = index;
            let model = ImportedModel {
                groups: vec![ImportedGroup {
                    kind: 0,
                    indices,
                    smooth: 1.,
                }],
                positions: vec![MeshVector::default()],
                uvs: vec![MeshUv::default()],
                normals: vec![MeshVector::default()],
                tangents: vec![MeshVector::default()],
            };
            let mut writer = BinaryWriter::default();
            put_count(&mut writer, 1).unwrap();
            model.write(&mut writer).unwrap();
            put_count(&mut writer, 0).unwrap();
            let read = |project| {
                ImportedMesh::read_wire(&mut BinaryCursor::new(writer.bytes()).unwrap(), project)
            };
            assert!(matches!(read(false), Err(BinaryError::Index)));
            if matches!(attribute, 1 | 2) && index == -1 {
                assert!(read(true).unwrap().has_missing_corner_attributes());
            } else {
                assert!(matches!(read(true), Err(BinaryError::Index)));
            }
        }
    }
}
