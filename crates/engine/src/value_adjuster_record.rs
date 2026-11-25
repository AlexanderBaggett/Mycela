use crate::{
    BinaryCursor, BinaryError, BinaryWriter, DecimalBits, LimitsRecord, RangeOptionsRecord,
    RangeRecordError,
};
use std::{error::Error, fmt};

/// Concrete receivers whose detached value wire behavior is characterized.
/// Complete catalog factory dispatch and complex payloads remain separate work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueAdjusterKind {
    Number,
    Dual,
    Selection,
    Color,
    Vector,
}
impl ValueAdjusterKind {
    pub const fn tag(self) -> i32 {
        match self {
            Self::Number => 0,
            Self::Dual => 1,
            Self::Selection => 4,
            Self::Color => 6,
            Self::Vector => 14,
        }
    }
}

/// Raw header values. A loaded tag does not change the existing concrete receiver.
#[derive(Clone, Debug)]
pub struct AdjusterMetadataRecord {
    pub name: Option<Vec<u16>>,
    pub group_id: [u8; 16],
    pub type_tag: i32,
    pub order: i32,
    pub loaded: bool,
}
impl AdjusterMetadataRecord {
    pub(crate) fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        identity_consumed: bool,
        version: f64,
    ) -> Result<(), ValueAdjusterError> {
        if !identity_consumed {
            self.name = Some(reader.read_string()?);
            let bytes = reader.read_bytes(16)?;
            self.group_id = bytes.try_into().map_err(|_| ValueAdjusterError::Guid)?;
            self.type_tag = reader.read_i32()?;
        }
        if version > 3.73 {
            self.order = reader.read_i32()?;
        }
        Ok(())
    }
    pub(crate) fn save(&self, writer: &mut BinaryWriter) -> Result<(), ValueAdjusterError> {
        writer.write_optional_utf16(self.name.as_deref())?;
        writer.write_bytes(&self.group_id)?;
        writer.write_i32(self.type_tag)?;
        writer.write_i32(self.order)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectionOptionRecord {
    pub name: Option<Vec<u16>>,
    pub value: f64,
}
impl SelectionOptionRecord {
    fn read(reader: &mut BinaryCursor<'_>) -> Result<Self, ValueAdjusterError> {
        let name = Some(reader.read_string()?);
        let value = reader.read_f64()?;
        Ok(Self { name, value })
    }
    fn save(&self, writer: &mut BinaryWriter) -> Result<(), ValueAdjusterError> {
        writer.write_optional_utf16(self.name.as_deref())?;
        writer.write_f64(self.value)?;
        Ok(())
    }
}

/// Numeric wire state; None preserves a source-null backing Limits reference.
#[derive(Clone, Debug)]
pub struct NumericAdjusterMetadata {
    pub accuracy: i32,
    pub limits: Option<LimitsRecord>,
}
impl Default for NumericAdjusterMetadata {
    fn default() -> Self {
        const ZERO: DecimalBits = match DecimalBits::new([0, 0, 0, 0]) {
            Ok(v) => v,
            Err(_) => panic!("invalid zero"),
        };
        const HUNDRED: DecimalBits = match DecimalBits::new([100, 0, 0, 0]) {
            Ok(v) => v,
            Err(_) => panic!("invalid hundred"),
        };
        let limits = LimitsRecord {
            decimal_places: 1,
            minimum: ZERO,
            maximum: HUNDRED,
            use_range: Some(RangeOptionsRecord::new(ZERO, HUNDRED)),
            ..LimitsRecord::default()
        };
        Self {
            accuracy: 1,
            limits: Some(limits),
        }
    }
}
impl NumericAdjusterMetadata {
    fn load(&mut self, reader: &mut BinaryCursor<'_>) -> Result<(), ValueAdjusterError> {
        self.accuracy = reader.read_i32()?;
        self.limits
            .as_mut()
            .ok_or(ValueAdjusterError::MissingLimits)?
            .load(reader)?;
        Ok(())
    }
    fn save(&self, writer: &mut BinaryWriter) -> Result<(), ValueAdjusterError> {
        writer.write_i32(self.accuracy)?;
        self.limits
            .as_ref()
            .ok_or(ValueAdjusterError::MissingLimits)?
            .save(writer)?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct NumberAdjusterRecord {
    pub numeric: NumericAdjusterMetadata,
    pub value: f64,
    pub previous: f64,
    pub selected: i32,
    pub previous_selected: i32,
    pub options: Vec<SelectionOptionRecord>,
}
#[derive(Clone, Debug)]
pub struct VectorAdjusterRecord<const N: usize> {
    pub numeric: NumericAdjusterMetadata,
    pub value: [f64; N],
    pub previous: [f64; N],
}
#[derive(Debug)]
pub enum ValueAdjusterPayload {
    Number(NumberAdjusterRecord),
    Dual(VectorAdjusterRecord<2>),
    Color { value: [f64; 3], previous: [f64; 3] },
    Vector(VectorAdjusterRecord<3>),
}

/// Owned detached values; source aliases, attached properties and callbacks are
/// separate capabilities. This is not complete cp catalog factory dispatch.
#[derive(Debug)]
pub struct ValueAdjusterRecord {
    pub metadata: AdjusterMetadataRecord,
    pub payload: ValueAdjusterPayload,
}
impl ValueAdjusterRecord {
    pub fn new(kind: ValueAdjusterKind, name: Option<Vec<u16>>, group_id: [u8; 16]) -> Self {
        let metadata = AdjusterMetadataRecord {
            name,
            group_id,
            type_tag: kind.tag(),
            order: 0,
            loaded: false,
        };
        let payload = match kind {
            ValueAdjusterKind::Number | ValueAdjusterKind::Selection => {
                ValueAdjusterPayload::Number(NumberAdjusterRecord {
                    numeric: NumericAdjusterMetadata::default(),
                    value: -1.0,
                    previous: 0.0,
                    selected: -1,
                    previous_selected: -1,
                    options: Vec::new(),
                })
            }
            ValueAdjusterKind::Dual => ValueAdjusterPayload::Dual(VectorAdjusterRecord {
                numeric: NumericAdjusterMetadata::default(),
                value: [0.0; 2],
                previous: [0.0; 2],
            }),
            ValueAdjusterKind::Color => ValueAdjusterPayload::Color {
                value: [0.0; 3],
                previous: [0.0; 3],
            },
            ValueAdjusterKind::Vector => ValueAdjusterPayload::Vector(VectorAdjusterRecord {
                numeric: NumericAdjusterMetadata::default(),
                value: [0.0; 3],
                previous: [0.0; 3],
            }),
        };
        Self { metadata, payload }
    }

    pub fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        identity_consumed: bool,
        version: f64,
    ) -> Result<(), ValueAdjusterError> {
        self.metadata.load(reader, identity_consumed, version)?;
        match &mut self.payload {
            ValueAdjusterPayload::Number(number) => {
                number.numeric.load(reader)?;
                let selected = reader.read_i32()?;
                number.selected = selected;
                number.previous_selected = selected;
                let value = reader.read_f64()?;
                number.value = value;
                number.previous = value;
                let count = reader.read_i32()?;
                for _ in 0..count {
                    let option = SelectionOptionRecord::read(reader)?;
                    if self.metadata.type_tag == ValueAdjusterKind::Selection.tag() {
                        number
                            .options
                            .try_reserve(1)
                            .map_err(|_| BinaryError::Allocation)?;
                        number.options.push(option);
                    }
                }
            }
            ValueAdjusterPayload::Dual(dual) => {
                dual.numeric.load(reader)?;
                let value = read_vector(reader)?;
                dual.value = value;
                dual.previous = value;
            }
            ValueAdjusterPayload::Color { value, .. } => {
                for channel in value {
                    *channel = reader.read_f64()?;
                }
            }
            ValueAdjusterPayload::Vector(vector) => {
                // F068: generic q1.Load mutates a discarded boxed struct. Only
                // after all three reads does source copy retained current to previous.
                for _ in 0..3 {
                    reader.read_f64()?;
                }
                vector.previous = vector.value;
                vector.numeric.load(reader)?;
                let value = read_vector(reader)?;
                vector.value = value;
                vector.previous = value;
            }
        }
        Ok(())
    }

    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), ValueAdjusterError> {
        self.metadata.save(writer)?;
        self.payload.save(writer)
    }
}

impl ValueAdjusterPayload {
    pub(crate) fn save(&self, writer: &mut BinaryWriter) -> Result<(), ValueAdjusterError> {
        match self {
            ValueAdjusterPayload::Number(number) => {
                number.numeric.save(writer)?;
                writer.write_i32(number.selected)?;
                writer.write_f64(number.value)?;
                writer.write_i32(
                    i32::try_from(number.options.len()).map_err(|_| BinaryError::Allocation)?,
                )?;
                for option in &number.options {
                    option.save(writer)?;
                }
            }
            ValueAdjusterPayload::Dual(dual) => {
                dual.numeric.save(writer)?;
                write_vector(writer, &dual.value)?;
            }
            ValueAdjusterPayload::Color { value, .. } => write_vector(writer, value)?,
            ValueAdjusterPayload::Vector(vector) => {
                write_vector(writer, &vector.value)?;
                vector.numeric.save(writer)?;
                write_vector(writer, &vector.value)?;
            }
        }
        Ok(())
    }
}
fn read_vector<const N: usize>(
    reader: &mut BinaryCursor<'_>,
) -> Result<[f64; N], ValueAdjusterError> {
    let mut value = [0.0; N];
    for channel in &mut value {
        *channel = reader.read_f64()?;
    }
    Ok(value)
}
fn write_vector(writer: &mut BinaryWriter, value: &[f64]) -> Result<(), ValueAdjusterError> {
    for channel in value {
        writer.write_f64(*channel)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueAdjusterError {
    Binary(BinaryError),
    Range(RangeRecordError),
    Guid,
    MissingLimits,
}
impl ValueAdjusterError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Binary(error) => error.source_class(),
            Self::Range(error) => error.source_class(),
            Self::Guid => "ArgumentException",
            Self::MissingLimits => "NullReferenceException",
        }
    }
}
impl From<BinaryError> for ValueAdjusterError {
    fn from(error: BinaryError) -> Self {
        Self::Binary(error)
    }
}
impl From<RangeRecordError> for ValueAdjusterError {
    fn from(error: RangeRecordError) -> Self {
        Self::Range(error)
    }
}
impl fmt::Display for ValueAdjusterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Binary(error) => error.fmt(f),
            Self::Range(error) => error.fmt(f),
            Self::Guid => f.write_str("adjuster GUID requires sixteen bytes"),
            Self::MissingLimits => {
                f.write_str("source numeric adjuster requires a Limits reference")
            }
        }
    }
}
impl Error for ValueAdjusterError {}
