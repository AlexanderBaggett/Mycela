use crate::{BinaryCursor, BinaryError, BinaryWriter};

/// Flame h4's data-only wire state. Parameter meanings and rendering are separate.
///
/// Lookup data is nonwire context. This owned projection retains its allocation
/// during loading/saving; it does not implement caller-shared array mutations.
#[derive(Debug)]
pub struct FlamePostprocessRecord {
    pub enabled: bool,
    pub parameters: [f64; 3],
    pub lookup: Option<Vec<f32>>,
}

impl Default for FlamePostprocessRecord {
    fn default() -> Self {
        Self {
            enabled: true,
            parameters: [0.3, 1.0, 1.5],
            lookup: Some(Vec::new()),
        }
    }
}

impl FlamePostprocessRecord {
    pub fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        _encoding_version: f64,
    ) -> Result<(), BinaryError> {
        self.enabled = reader.read_bool()?;
        for parameter in &mut self.parameters {
            *parameter = reader.read_f64()?;
        }
        Ok(())
    }

    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), BinaryError> {
        writer.write_bool(self.enabled)?;
        for parameter in self.parameters {
            writer.write_f64(parameter)?;
        }
        Ok(())
    }
}
