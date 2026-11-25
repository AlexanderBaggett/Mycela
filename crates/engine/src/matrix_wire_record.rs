use crate::{BinaryCursor, BinaryError, BinaryWriter};
use std::{error::Error, fmt};

/// Data from the transform's six-double matrix wire helper.
/// Elements are M11, M12, M21, M22, OffsetX, OffsetY. Characterized arithmetic
/// and typed shared-root operations are provided by the matrix module.
#[derive(Debug)]
pub struct MatrixWireRecord {
    pub elements: [f64; 6],
}

impl Default for MatrixWireRecord {
    fn default() -> Self {
        Self {
            elements: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }
}

impl MatrixWireRecord {
    /// Read a fresh value. A caller can publish it after success; a failed read
    /// consumes available bytes without modifying the caller's existing value.
    pub fn read(
        reader: &mut BinaryCursor<'_>,
        _encoding_version: f64,
    ) -> Result<Self, BinaryError> {
        let mut elements = [0.0; 6];
        for element in &mut elements {
            *element = reader.read_f64()?;
        }
        Ok(Self { elements })
    }

    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), BinaryError> {
        for element in self.elements {
            writer.write_f64(element)?;
        }
        Ok(())
    }

    /// The original private helper accepts a nullable matrix argument. Its
    /// first property access fails before any write when that argument is null.
    pub fn save_optional(
        value: Option<&Self>,
        writer: &mut BinaryWriter,
    ) -> Result<(), MatrixWireError> {
        value.ok_or(MatrixWireError::NullValue)?.save(writer)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatrixWireError {
    NullValue,
    Binary(BinaryError),
}

impl MatrixWireError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::NullValue => "NullReferenceException",
            Self::Binary(error) => error.source_class(),
        }
    }
}
impl From<BinaryError> for MatrixWireError {
    fn from(value: BinaryError) -> Self {
        Self::Binary(value)
    }
}
impl fmt::Display for MatrixWireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "matrix wire: {}", self.source_class())
    }
}
impl Error for MatrixWireError {}
