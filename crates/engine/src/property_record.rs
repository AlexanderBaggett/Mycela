use crate::{
    BinaryCursor, BinaryError, BinaryWriter, CurveKind, CurveLayerRecord, CurveRecord,
    CurveRecordError,
};
use std::{error::Error, fmt};

/// Gradient wire data; sampling caches and editor references are separate.
#[derive(Clone, Debug)]
pub struct GradientRecord {
    pub width: i32,
    pub colors: Vec<GradientColorPoint>,
    pub transparency: Vec<GradientTransparencyPoint>,
}
#[derive(Clone, Debug)]
pub struct GradientColorPoint {
    pub coordinate: i32,
    pub argb: [f64; 4],
}
#[derive(Clone, Debug)]
pub struct GradientTransparencyPoint {
    pub coordinate: i32,
    pub value: f64,
}
impl Default for GradientRecord {
    fn default() -> Self {
        Self {
            width: 1000,
            colors: Vec::new(),
            transparency: Vec::new(),
        }
    }
}
impl GradientRecord {
    pub fn load(&mut self, reader: &mut BinaryCursor<'_>) -> Result<(), PropertyRecordError> {
        self.colors.clear();
        self.transparency.clear();
        self.width = reader.read_i32()?;
        let colors = reader.read_i32()?;
        let alpha = reader.read_i32()?;
        for _ in 0..colors {
            let coordinate = reader.read_i32()?;
            let argb = [
                reader.read_f64()?,
                reader.read_f64()?,
                reader.read_f64()?,
                reader.read_f64()?,
            ];
            self.colors
                .try_reserve(1)
                .map_err(|_| PropertyRecordError::Allocation)?;
            self.colors.push(GradientColorPoint { coordinate, argb });
        }
        for _ in 0..alpha {
            let coordinate = reader.read_i32()?;
            let value = reader.read_f64()?;
            self.transparency
                .try_reserve(1)
                .map_err(|_| PropertyRecordError::Allocation)?;
            self.transparency
                .push(GradientTransparencyPoint { coordinate, value });
        }
        Ok(())
    }
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), PropertyRecordError> {
        writer.write_i32(self.width)?;
        writer.write_i32(
            i32::try_from(self.colors.len()).map_err(|_| PropertyRecordError::Allocation)?,
        )?;
        writer.write_i32(
            i32::try_from(self.transparency.len()).map_err(|_| PropertyRecordError::Allocation)?,
        )?;
        for point in &self.colors {
            writer.write_i32(point.coordinate)?;
            for value in point.argb {
                writer.write_f64(value)?;
            }
        }
        for point in &self.transparency {
            writer.write_i32(point.coordinate)?;
            writer.write_f64(point.value)?;
        }
        Ok(())
    }
}

/// Fixed-layout animation wire data. Inactive slots retain their curves, and
/// omitted gradient flags retain the existing gradient. Timeline sampling and
/// source shared-array/clone behavior are not implemented by this DTO.
#[derive(Clone, Debug)]
pub struct AnimationRecord {
    components: Vec<AnimationComponentRecord>,
    pub gradient: Option<GradientRecord>,
}
#[derive(Clone, Debug)]
pub struct AnimationComponentRecord {
    pub active: bool,
    pub curve: Option<CurveRecord>,
}
impl AnimationRecord {
    pub fn new(count: usize) -> Self {
        Self {
            components: (0..count)
                .map(|_| AnimationComponentRecord {
                    active: false,
                    curve: None,
                })
                .collect(),
            gradient: None,
        }
    }
    pub fn components(&self) -> &[AnimationComponentRecord] {
        &self.components
    }
    pub fn components_mut(&mut self) -> &mut [AnimationComponentRecord] {
        &mut self.components
    }
    pub fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), PropertyRecordError> {
        for component in &mut self.components {
            component.active = reader.read_bool()?;
            if component.active {
                component
                    .curve
                    .get_or_insert_with(|| CurveRecord::new(0.0, 0.0, false))
                    .load(reader, version)?;
            }
        }
        if version > 3.15 && reader.read_bool()? {
            self.gradient
                .get_or_insert_with(GradientRecord::default)
                .load(reader)?;
        }
        Ok(())
    }
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), PropertyRecordError> {
        for component in &self.components {
            writer.write_bool(component.active)?;
            if component.active {
                component
                    .curve
                    .as_ref()
                    .ok_or(PropertyRecordError::MissingCurve)?
                    .save(writer)?;
            }
        }
        let included = self.gradient.is_some() && self.components.first().is_some_and(|c| c.active);
        writer.write_bool(included)?;
        if included {
            self.gradient
                .as_ref()
                .ok_or(PropertyRecordError::MissingGradient)?
                .save(writer)?;
        }
        Ok(())
    }
}

/// Adjustment wire data with explicit resolved component bounds. Full source
/// Limits/UseRange resolution and mutable component-array layouts are separate.
#[derive(Clone, Debug)]
pub struct AdjustmentRecord {
    pub property_type: i32,
    pub mode: i32,
    pub guid: [u8; 16],
    components: Vec<AdjustmentComponentRecord>,
}
#[derive(Clone, Debug)]
pub struct AdjustmentComponentRecord {
    pub name: Vec<u16>,
    pub active: bool,
    pub curve: Option<CurveRecord>,
    default_bounds: [f32; 2],
}
impl AdjustmentComponentRecord {
    pub const fn default_bounds(&self) -> [f32; 2] {
        self.default_bounds
    }
    pub fn curve_mut(&mut self) -> &mut CurveRecord {
        self.curve.get_or_insert_with(|| {
            let mut c = CurveRecord::new(self.default_bounds[0], self.default_bounds[1], false);
            c.layers.push(CurveLayerRecord::new(CurveKind::Linear));
            c
        })
    }
}
impl AdjustmentRecord {
    pub fn new(bounds: &[[f32; 2]]) -> Self {
        Self {
            property_type: 0,
            mode: 1,
            guid: [0; 16],
            components: bounds
                .iter()
                .map(|bounds| AdjustmentComponentRecord {
                    name: Vec::new(),
                    active: false,
                    curve: None,
                    default_bounds: *bounds,
                })
                .collect(),
        }
    }
    pub fn components(&self) -> &[AdjustmentComponentRecord] {
        &self.components
    }
    pub fn components_mut(&mut self) -> &mut [AdjustmentComponentRecord] {
        &mut self.components
    }
    pub fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), PropertyRecordError> {
        self.property_type = reader.read_i32()?;
        self.mode = reader.read_i32()?;
        let guid = reader.read_bytes(16)?;
        if guid.len() != 16 {
            return Err(PropertyRecordError::GuidLength);
        }
        self.guid.copy_from_slice(&guid);
        for component in &mut self.components {
            component.name = reader.read_string()?;
            component.active = reader.read_bool()?;
            component.curve_mut().load(reader, version)?;
        }
        Ok(())
    }
    /// Writing can materialize a missing default Linear curve, matching akf.Save.
    pub fn save(&mut self, writer: &mut BinaryWriter) -> Result<(), PropertyRecordError> {
        writer.write_i32(self.property_type)?;
        writer.write_i32(self.mode)?;
        writer.write_bytes(&self.guid)?;
        for component in &mut self.components {
            writer.write_utf16(&component.name)?;
            writer.write_bool(component.active)?;
            component.curve_mut().save(writer)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PropertyRecordError {
    Binary(BinaryError),
    Curve(CurveRecordError),
    GuidLength,
    MissingCurve,
    MissingGradient,
    Allocation,
}
impl From<BinaryError> for PropertyRecordError {
    fn from(e: BinaryError) -> Self {
        Self::Binary(e)
    }
}
impl From<CurveRecordError> for PropertyRecordError {
    fn from(e: CurveRecordError) -> Self {
        Self::Curve(e)
    }
}
impl PropertyRecordError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Binary(e) => e.source_class(),
            Self::Curve(e) => e.source_class(),
            Self::GuidLength => "ArgumentException",
            Self::MissingCurve | Self::MissingGradient => "NullReferenceException",
            Self::Allocation => "OutOfMemoryException",
        }
    }
}
impl fmt::Display for PropertyRecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "property record {:?}: {}", self, self.source_class())
    }
}
impl Error for PropertyRecordError {}
