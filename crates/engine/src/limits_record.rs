use crate::{BinaryCursor, BinaryError, BinaryWriter, DecimalBits};
use std::{error::Error, fmt};

// Constructor data is checked at compile time; these are source decimal bits.
const fn integer(magnitude: u32, negative: bool) -> DecimalBits {
    match DecimalBits::new([magnitude, 0, 0, if negative { 0x8000_0000 } else { 0 }]) {
        Ok(value) => value,
        Err(_) => panic!("invalid built-in decimal"),
    }
}
const ZERO: DecimalBits = integer(0, false);
const ONE: DecimalBits = integer(1, false);
const TEN: DecimalBits = integer(10, false);
const MINIMUM: DecimalBits = integer(1_000_000, true);
const MAXIMUM: DecimalBits = integer(1_000_000, false);

/// Owned range option wire values from Controls.ad4.
/// Flag order matches the source fields f/g/h; GUI meaning remains separate.
#[derive(Clone, Debug)]
pub struct RangeOptionsRecord {
    pub minimum: DecimalBits,
    pub maximum: DecimalBits,
    pub decimal_places: i32,
    pub multiplier: f64,
    pub flags: [bool; 3],
}
impl Default for RangeOptionsRecord {
    fn default() -> Self {
        Self::new(ZERO, ONE)
    }
}
impl RangeOptionsRecord {
    pub fn new(minimum: DecimalBits, maximum: DecimalBits) -> Self {
        Self {
            minimum,
            maximum,
            decimal_places: 3,
            multiplier: 1.0,
            flags: [false, false, true],
        }
    }

    pub fn load(&mut self, reader: &mut BinaryCursor<'_>) -> Result<(), RangeRecordError> {
        self.minimum = reader.read_decimal()?;
        self.maximum = reader.read_decimal()?;
        self.decimal_places = reader.read_i32()?;
        self.multiplier = reader.read_f64()?;
        for flag in &mut self.flags {
            *flag = reader.read_bool()?;
        }
        Ok(())
    }

    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), RangeRecordError> {
        writer.write_decimal(self.minimum)?;
        writer.write_decimal(self.maximum)?;
        writer.write_i32(self.decimal_places)?;
        writer.write_f64(self.multiplier)?;
        for flag in self.flags {
            writer.write_bool(flag)?;
        }
        Ok(())
    }
}

/// Exact curve range wire values; ordering is not validated by source Load.
#[derive(Clone, Debug)]
pub struct CurveRangeRecord {
    pub minimum: DecimalBits,
    pub maximum: DecimalBits,
    pub wrap_y: bool,
}
impl Default for CurveRangeRecord {
    fn default() -> Self {
        Self {
            minimum: ZERO,
            maximum: ONE,
            wrap_y: false,
        }
    }
}
impl CurveRangeRecord {
    pub fn load(&mut self, reader: &mut BinaryCursor<'_>) -> Result<(), RangeRecordError> {
        self.minimum = reader.read_decimal()?;
        self.maximum = reader.read_decimal()?;
        self.wrap_y = reader.read_bool()?;
        Ok(())
    }

    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), RangeRecordError> {
        writer.write_decimal(self.minimum)?;
        writer.write_decimal(self.maximum)?;
        writer.write_bool(self.wrap_y)?;
        Ok(())
    }
}

/// Owned Limits wire state. BigIncrement is retained but not serialized.
/// Optional children preserve null backing state and partial source loads;
/// constructor/clone sharing and validation arithmetic are separate capabilities.
#[derive(Clone, Debug)]
pub struct LimitsRecord {
    pub decimal_places: i32,
    pub increment: DecimalBits,
    pub big_increment: DecimalBits,
    pub minimum: DecimalBits,
    pub maximum: DecimalBits,
    pub used: bool,
    pub use_range: Option<RangeOptionsRecord>,
    pub curve_range: Option<CurveRangeRecord>,
}
impl Default for LimitsRecord {
    fn default() -> Self {
        Self {
            decimal_places: 3,
            increment: ONE,
            big_increment: TEN,
            minimum: MINIMUM,
            maximum: MAXIMUM,
            used: true,
            use_range: Some(RangeOptionsRecord::new(MINIMUM, MAXIMUM)),
            curve_range: None,
        }
    }
}
impl LimitsRecord {
    pub fn load(&mut self, reader: &mut BinaryCursor<'_>) -> Result<(), RangeRecordError> {
        self.decimal_places = reader.read_i32()?;
        self.increment = reader.read_decimal()?;
        self.minimum = reader.read_decimal()?;
        self.maximum = reader.read_decimal()?;
        self.used = reader.read_bool()?;
        if reader.read_bool()? {
            // Replacement must survive even a failure in the first child read.
            let range = self.use_range.insert(RangeOptionsRecord::default());
            range.load(reader)?;
        }
        if reader.read_bool()? {
            let range = self.curve_range.insert(CurveRangeRecord::default());
            range.load(reader)?;
        }
        Ok(())
    }

    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), RangeRecordError> {
        writer.write_i32(self.decimal_places)?;
        writer.write_decimal(self.increment)?;
        writer.write_decimal(self.minimum)?;
        writer.write_decimal(self.maximum)?;
        writer.write_bool(self.used)?;
        writer.write_bool(self.use_range.is_some())?;
        // F065: source dereferences unconditionally after the presence byte.
        self.use_range
            .as_ref()
            .ok_or(RangeRecordError::MissingUseRange)?
            .save(writer)?;
        writer.write_bool(self.curve_range.is_some())?;
        if let Some(range) = &self.curve_range {
            range.save(writer)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RangeRecordError {
    Binary(BinaryError),
    MissingUseRange,
}
impl RangeRecordError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Binary(error) => error.source_class(),
            Self::MissingUseRange => "NullReferenceException",
        }
    }
}
impl From<BinaryError> for RangeRecordError {
    fn from(error: BinaryError) -> Self {
        Self::Binary(error)
    }
}
impl fmt::Display for RangeRecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Binary(error) => error.fmt(f),
            Self::MissingUseRange => {
                f.write_str("source Limits save requires a range after writing its presence flag")
            }
        }
    }
}
impl Error for RangeRecordError {}
