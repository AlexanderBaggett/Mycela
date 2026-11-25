use crate::{
    AdjustmentRecord, AnimationRecord, BinaryCursor, BinaryError, BinaryWriter,
    PropertyRecordError, ReferenceAdjusterError, SharedCurveRecord, SharedGradientRecord,
};
use std::{error::Error, fmt};

/// Typed primitive state and shared nullable curve/gradient roots.
/// Wire reads bypass setters and preserve all IEEE bits and the Color flag.
#[derive(Clone, Debug)]
pub enum ModernPropertyValue {
    Double {
        current: f64,
        previous: f64,
    },
    Float {
        current: f32,
        previous: f32,
    },
    Int32 {
        current: i32,
        previous: i32,
    },
    Selected {
        current: i32,
        previous: i32,
    },
    Boolean(bool),
    Curve(Option<SharedCurveRecord>),
    Gradient(Option<SharedGradientRecord>),
    Color {
        argb: [f64; 4],
        grayscale: bool,
    },
    DualDouble {
        current: [f64; 2],
        previous: [f64; 2],
    },
    DualFloat {
        current: [f32; 2],
        previous: [f32; 2],
    },
    DualInt32 {
        current: [i32; 2],
        previous: [i32; 2],
    },
    Vector {
        current: [f64; 3],
        previous: [f64; 3],
    },
}
impl ModernPropertyValue {
    pub const fn component_count(&self) -> usize {
        match self {
            Self::DualDouble { .. } | Self::DualFloat { .. } | Self::DualInt32 { .. } => 2,
            Self::Vector { .. } => 3,
            _ => 1,
        }
    }
}

/// Resolved concrete getter configuration. Source flag exceptions and Limits
/// resolution belong to the caller's characterized property definition.
#[derive(Clone, Debug)]
pub struct ModernAdjustmentConfiguration {
    pub property_type: i32,
    pub bounds: Vec<[f32; 2]>,
}
#[derive(Clone, Debug)]
pub struct ModernPropertyConfiguration {
    pub can_create_animation: bool,
    pub adjustment: Option<ModernAdjustmentConfiguration>,
}

/// Owned fixed-layout modern property metadata with typed wire values. Full setters, internal aliases, dynamic
/// Limits/options and parameter-set enumeration are separate capabilities.
#[derive(Debug)]
pub struct ModernPropertyRecord {
    pub name: Option<Vec<u16>>,
    pub adjustable: bool,
    pub animatable: bool,
    pub animation: Option<AnimationRecord>,
    pub adjustment: Option<AdjustmentRecord>,
    value: ModernPropertyValue,
    configuration: ModernPropertyConfiguration,
}
impl ModernPropertyRecord {
    pub fn new(
        value: ModernPropertyValue,
        configuration: ModernPropertyConfiguration,
    ) -> Result<Self, ModernPropertyError> {
        if configuration
            .adjustment
            .as_ref()
            .is_some_and(|a| a.bounds.len() != value.component_count())
        {
            return Err(ModernPropertyError::ConfigurationLayout);
        }
        Ok(Self {
            name: None,
            adjustable: false,
            animatable: false,
            animation: None,
            adjustment: None,
            value,
            configuration,
        })
    }
    /// Owned read snapshot; not source Clone, live-root replacement or Save.
    /// Owned metadata is copied; existing shared Curve/Gradient handles retain
    /// their identity. No lazy getter or serialization side effect is invoked.
    pub fn copy_record_snapshot(&self) -> Self {
        Self {
            name: self.name.clone(),
            adjustable: self.adjustable,
            animatable: self.animatable,
            animation: self.animation.clone(),
            adjustment: self.adjustment.clone(),
            value: self.value.clone(),
            configuration: self.configuration.clone(),
        }
    }
    pub fn value(&self) -> &ModernPropertyValue {
        &self.value
    }
    pub(crate) fn double_pair(&self) -> Option<(f64, f64)> {
        match self.value {
            ModernPropertyValue::Double { current, previous } => Some((current, previous)),
            _ => None,
        }
    }
    // Post-callback assignment must read current now, after any synchronous
    // reentry, rather than publish the value captured before dispatch.
    pub(crate) fn copy_double_current_to_previous(&mut self) -> bool {
        if let ModernPropertyValue::Double { current, previous } = &mut self.value {
            *previous = *current;
            true
        } else {
            false
        }
    }
    // Crate-private backing edit for recorded fixture/data operations. Keep the
    // concrete value kind and previous value immutable through this operation.
    pub(crate) fn overwrite_double_current(&mut self, current: f64) -> bool {
        if let ModernPropertyValue::Double { current: value, .. } = &mut self.value {
            *value = current;
            true
        } else {
            false
        }
    }
    pub(crate) fn set_adjustment_bounds(&mut self, bounds: Vec<[f32; 2]>) {
        if let Some(configuration) = &mut self.configuration.adjustment {
            configuration.bounds = bounds;
        }
    }
    pub(crate) fn value_mut(&mut self) -> &mut ModernPropertyValue {
        &mut self.value
    }
    pub fn configuration(&self) -> &ModernPropertyConfiguration {
        &self.configuration
    }
    pub fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), ModernPropertyError> {
        // F073: exact historical special versions; NaN skips these flags.
        if version >= 3.72 || version == 3.71 || version == 3.7 {
            if reader.read_bool()? {
                if self.animation.is_none() {
                    if !self.configuration.can_create_animation {
                        return Err(ModernPropertyError::AnimationUnavailable);
                    }
                    self.animation = Some(AnimationRecord::new(self.value.component_count()));
                }
                if let Some(animation) = &mut self.animation {
                    animation.load(reader, version)?;
                }
            } else {
                self.animation = None;
            }
            let present = reader.read_bool()?;
            self.load_adjustment_body(reader, version, present)?;
        }
        self.load_value(reader, version)
    }
    pub(crate) fn load_adjustment_body(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
        present: bool,
    ) -> Result<(), ModernPropertyError> {
        if present {
            if self.adjustment.is_none() {
                let config = self
                    .configuration
                    .adjustment
                    .as_ref()
                    .ok_or(ModernPropertyError::AdjustmentUnavailable)?;
                let mut adjustment = AdjustmentRecord::new(&config.bounds);
                adjustment.property_type = config.property_type;
                self.adjustment = Some(adjustment);
            }
            if let Some(adjustment) = &mut self.adjustment {
                adjustment.load(reader, version)?;
            }
        } else {
            self.adjustment = None;
        }
        Ok(())
    }
    pub(crate) fn load_value(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), ModernPropertyError> {
        match &mut self.value {
            ModernPropertyValue::Double { current, previous } => {
                let value = reader.read_f64()?;
                *current = value;
                *previous = value;
            }
            ModernPropertyValue::Float { current, previous } => {
                let value = reader.read_f32()?;
                *current = value;
                *previous = value;
            }
            ModernPropertyValue::Int32 { current, previous }
            | ModernPropertyValue::Selected { current, previous } => {
                let value = reader.read_i32()?;
                *current = value;
                *previous = value;
            }
            ModernPropertyValue::Boolean(value) => *value = reader.read_bool()?,
            ModernPropertyValue::Curve(value) => {
                if let Some(value) = value {
                    value.load(reader, version)?;
                }
            }
            ModernPropertyValue::Gradient(value) => {
                if let Some(value) = value {
                    value.load(reader)?;
                }
            }
            ModernPropertyValue::Color { argb, .. } => {
                for value in argb {
                    *value = reader.read_f64()?;
                }
            }
            ModernPropertyValue::DualDouble { current, previous } => {
                let value = [reader.read_f64()?, reader.read_f64()?];
                *current = value;
                *previous = value;
            }
            ModernPropertyValue::DualFloat { current, previous } => {
                let value = [reader.read_f32()?, reader.read_f32()?];
                *current = value;
                *previous = value;
            }
            ModernPropertyValue::DualInt32 { current, previous } => {
                let value = [reader.read_i32()?, reader.read_i32()?];
                *current = value;
                *previous = value;
            }
            ModernPropertyValue::Vector { current, .. } => {
                // Generic q1.Load boxes ae4 and discards the first copy. It
                // does not update oldValue; the derived copy commits channels.
                for _ in 0..3 {
                    reader.read_f64()?;
                }
                for value in current {
                    *value = reader.read_f64()?;
                }
            }
        }
        Ok(())
    }
    pub fn save(&mut self, writer: &mut BinaryWriter) -> Result<(), ModernPropertyError> {
        let animated = self
            .animation
            .as_ref()
            .is_some_and(|a| a.components().iter().any(|c| c.active));
        writer.write_bool(animated)?;
        if animated && let Some(animation) = &self.animation {
            animation.save(writer)?;
        }
        self.save_tail(writer)
    }
    pub(crate) fn save_tail(
        &mut self,
        writer: &mut BinaryWriter,
    ) -> Result<(), ModernPropertyError> {
        let adjusted = self
            .adjustment
            .as_ref()
            .is_some_and(|a| a.components().iter().any(|c| c.active));
        writer.write_bool(adjusted)?;
        if adjusted && let Some(adjustment) = &mut self.adjustment {
            adjustment.save(writer)?;
        }
        match &self.value {
            ModernPropertyValue::Double { current, .. } => writer.write_f64(*current)?,
            ModernPropertyValue::Float { current, .. } => writer.write_f32(*current)?,
            ModernPropertyValue::Int32 { current, .. }
            | ModernPropertyValue::Selected { current, .. } => writer.write_i32(*current)?,
            ModernPropertyValue::Boolean(value) => writer.write_bool(*value)?,
            ModernPropertyValue::Curve(value) => {
                if let Some(value) = value {
                    value.save(writer)?;
                }
            }
            ModernPropertyValue::Gradient(value) => {
                if let Some(value) = value {
                    value.save(writer)?;
                }
            }
            ModernPropertyValue::Color { argb, .. } => {
                for value in argb {
                    writer.write_f64(*value)?;
                }
            }
            ModernPropertyValue::DualDouble { current, .. } => {
                for value in current {
                    writer.write_f64(*value)?;
                }
            }
            ModernPropertyValue::DualFloat { current, .. } => {
                for value in current {
                    writer.write_f32(*value)?;
                }
            }
            ModernPropertyValue::DualInt32 { current, .. } => {
                for value in current {
                    writer.write_i32(*value)?;
                }
            }
            ModernPropertyValue::Vector { current, .. } => {
                for _ in 0..2 {
                    for value in current {
                        writer.write_f64(*value)?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModernPropertyError {
    Binary(BinaryError),
    Wrapper(PropertyRecordError),
    Reference(ReferenceAdjusterError),
    AnimationUnavailable,
    AdjustmentUnavailable,
    ConfigurationLayout,
}
impl ModernPropertyError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Binary(e) => e.source_class(),
            Self::Wrapper(e) => e.source_class(),
            Self::Reference(e) => e.source_class(),
            Self::AnimationUnavailable | Self::AdjustmentUnavailable => "NullReferenceException",
            Self::ConfigurationLayout => "ArgumentException",
        }
    }
}
impl From<BinaryError> for ModernPropertyError {
    fn from(e: BinaryError) -> Self {
        Self::Binary(e)
    }
}
impl From<PropertyRecordError> for ModernPropertyError {
    fn from(e: PropertyRecordError) -> Self {
        Self::Wrapper(e)
    }
}
impl From<ReferenceAdjusterError> for ModernPropertyError {
    fn from(e: ReferenceAdjusterError) -> Self {
        Self::Reference(e)
    }
}
impl fmt::Display for ModernPropertyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Binary(e) => e.fmt(f),
            Self::Wrapper(e) => e.fmt(f),
            Self::Reference(e) => e.fmt(f),
            Self::AnimationUnavailable => f.write_str("source animation getter returned null"),
            Self::AdjustmentUnavailable => f.write_str("source adjustment getter returned null"),
            Self::ConfigurationLayout => {
                f.write_str("resolved adjustment layout does not match the concrete property")
            }
        }
    }
}
impl Error for ModernPropertyError {}
