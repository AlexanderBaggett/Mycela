use crate::solid_color::normalize_transparency;
use crate::{
    AdjustmentRecord, AnimationRecord, BinaryCursor, BinaryError, BinaryWriter,
    PropertyRecordError, property_prefix_en_us,
};
use std::{error::Error, fmt};

/// Detached, resolved Solid Color wire values and owned fixed-layout metadata.
///
/// Saving writes the source's current node format and can create animation or
/// adjustment curves. Values are already resolved stored values; writing does
/// not apply UI rounding, evaluate bindings, or sample animation. Body/property
/// loading preserves the characterized fixed source names/limits. Full GTX
/// dispatch/container loading and property evaluation remain separate.
// Rust value copying of this owned wire DTO is distinct from cloning a live
// source node, which also has retained Limits, callbacks and parent identities.
#[derive(Clone, Debug, Default)]
pub struct SolidColorWireRecord {
    pub color: [f64; 3],
    pub transparency: f64,
    pub color_metadata: SolidColorPropertyMetadata,
    pub transparency_metadata: SolidColorPropertyMetadata,
}

#[derive(Clone, Debug, Default)]
pub struct SolidColorPropertyMetadata {
    pub binding: PropertyBindingRecord,
    pub animation: Option<AnimationRecord>,
    pub adjustment: Option<AdjustmentRecord>,
}

/// A retained parent binding. Only index -1 suppresses its wire payload.
/// Names retain UTF-16 units; malformed surrogate sequences are writer errors.
#[derive(Clone, Debug)]
pub struct PropertyBindingRecord {
    pub index: i32,
    pub name: Option<Vec<u16>>,
}

impl Default for PropertyBindingRecord {
    fn default() -> Self {
        Self {
            index: -1,
            name: None,
        }
    }
}

impl SolidColorWireRecord {
    /// Read a detached node body after dispatch has consumed its identity.
    /// Uses the characterized default reader, names, limits and en-US profile.
    #[allow(clippy::approx_constant)] // 3.14 is the original format revision.
    pub fn load_body(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), SolidColorRecordError> {
        let count = reader.read_u8()?;
        if version < 3.14 {
            reader.read_string()?;
            let color = [reader.read_f64()?, reader.read_f64()?, reader.read_f64()?];
            let transparency = reader.read_f64()?;
            self.assign_color(color);
            self.assign_transparency(transparency);
            reader.read_bool()?;
            reader.read_bool()?;
        } else {
            for index in 0..count {
                let property = match index {
                    0 => SolidColorProperty::Color,
                    1 => SolidColorProperty::Transparency,
                    _ => return Err(SolidColorRecordError::PropertyIndex),
                };
                self.load_property(reader, version, property)?;
            }
        }
        if reader.read_u8()? != 0 {
            return Err(SolidColorRecordError::Slots);
        }
        Ok(())
    }

    /// Read one fixed property at its code string, keeping partial mutation.
    pub fn load_property(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
        property: SolidColorProperty,
    ) -> Result<(), SolidColorRecordError> {
        let code = reader.read_string()?;
        let name: &[u16] = match property {
            SolidColorProperty::Color => &[67, 111, 108, 111, 114],
            SolidColorProperty::Transparency => {
                &[84, 114, 97, 110, 115, 112, 97, 114, 101, 110, 99, 121]
            }
        };
        if !property_prefix_en_us(name, &code) {
            return Err(SolidColorRecordError::Code);
        }
        match property {
            SolidColorProperty::Color => {
                let color = [reader.read_f64()?, reader.read_f64()?, reader.read_f64()?];
                self.assign_color(color);
            }
            SolidColorProperty::Transparency => {
                let value = reader.read_f64()?;
                validate_transparency(value)?;
                self.assign_transparency(value);
            }
        }
        let metadata = match property {
            SolidColorProperty::Color => &mut self.color_metadata,
            SolidColorProperty::Transparency => &mut self.transparency_metadata,
        };
        if reader.read_bool()? {
            metadata.binding.index = i32::from(reader.read_i16()?);
            let name = reader.read_string()?;
            if metadata.binding.name.is_some() || !name.is_empty() {
                metadata.binding.name = Some(name);
            }
        }
        metadata
            .animation
            .get_or_insert_with(|| AnimationRecord::new(1))
            .load(reader, version)?;
        if version >= 3.72 && reader.read_bool()? {
            metadata
                .adjustment
                .get_or_insert_with(|| {
                    let (bounds, kind) = match property {
                        SolidColorProperty::Color => ([-1000.0, 1000.0], 6),
                        SolidColorProperty::Transparency => ([0.0, 1.0], 0),
                    };
                    let mut adjustment = AdjustmentRecord::new(&[bounds]);
                    adjustment.property_type = kind;
                    adjustment
                })
                .load(reader, version)?;
        }
        if (3.54..=3.71).contains(&version) && reader.read_bool()? {
            // The fresh obsolete c6 object's setters have no observers; only
            // its reads/failures are observable before it is discarded.
            reader.read_f64()?;
            reader.read_f64()?;
            reader.read_i32()?;
            reader.read_string()?;
            reader.read_string()?;
            if version >= 3.55 {
                reader.read_f64()?;
            }
        }
        Ok(())
    }
    fn assign_color(&mut self, color: [f64; 3]) {
        if self.color != color {
            self.color = color;
        }
    }
    fn assign_transparency(&mut self, value: f64) {
        let value = normalize_transparency(value);
        if self.transparency != value {
            self.transparency = value;
        }
    }
    pub fn save(&mut self, writer: &mut BinaryWriter) -> Result<(), PropertyRecordError> {
        writer.write_utf16(&[83, 111, 108, 105, 100, 32, 67, 111, 108, 111, 114])?;
        writer.write_u8(2)?;
        self.color_metadata
            .save(writer, &[67, 111, 108], &self.color)?;
        self.transparency_metadata
            .save(writer, &[84, 114, 97], &[self.transparency])?;
        writer.write_u8(0)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SolidColorProperty {
    Color,
    Transparency,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SolidColorRecordError {
    Binary(BinaryError),
    Property(PropertyRecordError),
    Code,
    NumericBounds,
    DecimalOverflow,
    PropertyIndex,
    Slots,
}
impl From<BinaryError> for SolidColorRecordError {
    fn from(e: BinaryError) -> Self {
        Self::Binary(e)
    }
}
impl From<PropertyRecordError> for SolidColorRecordError {
    fn from(e: PropertyRecordError) -> Self {
        Self::Property(e)
    }
}
impl SolidColorRecordError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Binary(e) => e.source_class(),
            Self::Property(e) => e.source_class(),
            Self::DecimalOverflow => "OverflowException",
            Self::PropertyIndex => "ArgumentOutOfRangeException",
            Self::Code | Self::NumericBounds | Self::Slots => "Exception",
        }
    }
}
impl fmt::Display for SolidColorRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Solid Color wire {:?}: {}",
            self,
            self.source_class()
        )
    }
}
impl Error for SolidColorRecordError {}

// Limits.IsValid(double) converts to Decimal before comparing. Only magnitude
// is needed for these symmetric fixed bounds; this validator does not replace
// the original input or claim a general Decimal conversion/formatting API.
fn validate_transparency(value: f64) -> Result<(), SolidColorRecordError> {
    const POWERS: [f64; 29] = [
        1.0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
        1e17, 1e18, 1e19, 1e20, 1e21, 1e22, 1e23, 1e24, 1e25, 1e26, 1e27, 1e28,
    ];
    let exponent = ((value.to_bits() >> 52) & 2047) as i32 - 1022;
    if exponent < -94 {
        return Ok(());
    }
    if exponent > 96 {
        return Err(SolidColorRecordError::DecimalOverflow);
    }
    let mut scale = 14 - ((exponent * 19728) >> 16);
    let mut scaled = value.abs();
    if scale >= 0 {
        scale = scale.min(28);
        scaled *= POWERS[scale as usize];
    } else if scale != -1 || scaled >= 1e15 {
        scaled /= POWERS[(-scale) as usize];
    } else {
        scale = 0;
    }
    if scaled < 1e14 && scale < 28 {
        scaled *= 10.0;
        scale += 1;
    }
    let mut coefficient = scaled as u128;
    let remainder = scaled - coefficient as f64;
    if remainder > 0.5 || (remainder == 0.5 && coefficient & 1 != 0) {
        coefficient += 1;
    }
    if coefficient == 0 {
        return Ok(());
    }
    if scale < 0 {
        coefficient *= 10_u128.pow((-scale) as u32);
        scale = 0;
    }
    if coefficient > (1_u128 << 96) - 1 {
        return Err(SolidColorRecordError::DecimalOverflow);
    }
    if coefficient > 1_000_000_u128 * 10_u128.pow(scale as u32) {
        return Err(SolidColorRecordError::NumericBounds);
    }
    Ok(())
}

impl SolidColorPropertyMetadata {
    fn save(
        &mut self,
        writer: &mut BinaryWriter,
        code: &[u16],
        values: &[f64],
    ) -> Result<(), PropertyRecordError> {
        writer.write_utf16(code)?;
        for value in values {
            writer.write_f64(*value)?;
        }
        let bound = self.binding.index != -1;
        writer.write_bool(bound)?;
        if bound {
            // Source os.e deliberately narrows without validating the i32.
            writer.write_i16(self.binding.index as i16)?;
            writer.write_utf16(self.binding.name.as_deref().unwrap_or(&[]))?;
        }
        self.animation
            .get_or_insert_with(|| AnimationRecord::new(1))
            .save(writer)?;
        let adjusted = self
            .adjustment
            .as_ref()
            .is_some_and(|a| a.components().iter().any(|c| c.active));
        writer.write_bool(adjusted)?;
        if let Some(adjustment) = self.adjustment.as_mut().filter(|_| adjusted) {
            adjustment.save(writer)?;
        }
        Ok(())
    }
}
