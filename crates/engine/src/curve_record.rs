use crate::{BinaryCursor, BinaryError, BinaryWriter};
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurveKind {
    Sine,
    Linear,
    Arc,
    Bell,
    Gain,
    Noise,
    Steps,
    Custom,
}
impl CurveKind {
    pub const fn tag(self) -> i32 {
        self as i32
    }
    pub fn from_tag(tag: i32) -> Result<Self, CurveRecordError> {
        match tag {
            0 => Ok(Self::Sine),
            1 => Ok(Self::Linear),
            2 => Ok(Self::Arc),
            3 => Ok(Self::Bell),
            4 => Ok(Self::Gain),
            5 => Ok(Self::Noise),
            6 => Ok(Self::Steps),
            7 => Ok(Self::Custom),
            _ => Err(CurveRecordError::TypeIndex),
        }
    }
}

/// Persisted composite data. Sampling, source clone/edit operations, caches and
/// callback/object-graph aliasing are separate capabilities.
#[derive(Clone, Debug)]
pub struct CurveRecord {
    pub layers: Vec<CurveLayerRecord>,
    pub minimum: f32,
    pub maximum: f32,
    pub wrap: bool,
}
impl CurveRecord {
    pub const fn new(minimum: f32, maximum: f32, wrap: bool) -> Self {
        Self {
            layers: Vec::new(),
            minimum,
            maximum,
            wrap,
        }
    }
    pub fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), CurveRecordError> {
        self.layers.clear();
        let count = reader.read_i32()?;
        if count < 0 {
            return Err(CurveRecordError::Validation);
        }
        for _ in 0..count {
            let mut layer = CurveLayerRecord::new(CurveKind::from_tag(reader.read_i32()?)?);
            layer.load(reader, version)?;
            self.layers
                .try_reserve(1)
                .map_err(|_| CurveRecordError::Allocation)?;
            self.layers.push(layer);
        }
        self.minimum = scalar(reader, version)?;
        self.maximum = scalar(reader, version)?;
        self.wrap = reader.read_bool()?;
        Ok(())
    }
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), CurveRecordError> {
        writer.write_i32(
            i32::try_from(self.layers.len()).map_err(|_| CurveRecordError::Allocation)?,
        )?;
        for layer in &self.layers {
            writer.write_i32(layer.kind.tag())?;
            layer.save(writer)?;
        }
        writer.write_f32(self.minimum)?;
        writer.write_f32(self.maximum)?;
        writer.write_bool(self.wrap)?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct CurveLayerRecord {
    kind: CurveKind,
    pub mode: i32,
    controls: Vec<CurveControlRecord>,
    pub loop_points: bool,
    pub points: Vec<CurvePointRecord>,
}
impl CurveLayerRecord {
    pub fn new(kind: CurveKind) -> Self {
        let mut controls = vec![
            control(1, "Mirror", CurveControlValue::Toggle(false)),
            numeric(2, "Frequency", 1.0, 1.0, 100.0),
            numeric(3, "Initial Delay", 0.0, 0.0, 100.0),
            numeric(4, "Repeat Delay", 0.0, 0.0, 100.0),
            numeric(5, "Mirror Delay", 0.0, 0.0, 100.0),
            modulated(6, "Phase", 0.0, 0),
            modulated(7, "Amplitude", 100.0, 2),
            modulated(8, "Shift", 50.0, 2),
            modulated(9, "Remap X", 0.0, 0),
            modulated(10, "Remap Y", 0.0, 2),
            numeric(11, "Minimum", 0.0, 0.0, 100.0),
            numeric(12, "Maximum", 100.0, 0.0, 100.0),
            control(
                13,
                "Mode",
                CurveControlValue::Enumeration {
                    value: 0,
                    maximum: 7,
                },
            ),
            modulated(14, "Strength", 100.0, 1),
        ];
        match kind {
            CurveKind::Linear => controls.push(control(
                15,
                "Direction",
                CurveControlValue::Enumeration {
                    value: 0,
                    maximum: 1,
                },
            )),
            CurveKind::Arc => controls.push(numeric(16, "Curve", 75.0, 0.0, 100.0)),
            CurveKind::Bell => controls.extend([
                modulated(17, "Width", 50.0, 1),
                modulated(18, "Skew", 50.0, 1),
            ]),
            CurveKind::Gain => controls.push(modulated(19, "Gain", 75.0, 1)),
            CurveKind::Noise => controls.extend([
                numeric(20, "Base Frequency", 10.0, 1.0, 1000.0),
                modulated(21, "Gain", 50.0, 1),
                modulated(22, "Jitter Amplitude", 0.0, 1),
                numeric(23, "Jitter Frequency", 10.0, 1.0, 100.0),
                numeric(24, "Seed", 0.0, 0.0, 1_000_000.0),
                control(25, "Loop", CurveControlValue::Toggle(true)),
            ]),
            CurveKind::Steps => controls.extend([
                numeric(26, "Steps", 4.0, 1.0, 100.0),
                modulated(27, "Smoothness", 50.0, 1),
            ]),
            CurveKind::Sine | CurveKind::Custom => {}
        }
        Self {
            kind,
            mode: 2,
            controls,
            loop_points: true,
            points: Vec::new(),
        }
    }
    pub const fn kind(&self) -> CurveKind {
        self.kind
    }
    pub fn controls(&self) -> &[CurveControlRecord] {
        &self.controls
    }
    pub fn controls_mut(&mut self) -> &mut [CurveControlRecord] {
        &mut self.controls
    }
    pub(crate) fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), CurveRecordError> {
        self.mode = reader.read_i32()?;
        enumeration(self.mode, 3)?;
        for control in &mut self.controls {
            control.load(reader, version)?;
        }
        if self.kind == CurveKind::Custom {
            self.loop_points = reader.read_bool()?;
            let count = reader.read_i32()?;
            if count < 0 {
                return Err(CurveRecordError::Validation);
            }
            self.points.clear();
            for _ in 0..count {
                let x = reader.read_f64()? as f32;
                let y = reader.read_f64()? as f32;
                let left = reader.read_i32()?;
                let right = reader.read_i32()?;
                range(x, 0.0, 1.0)?;
                range(y, -1000.0, 1000.0)?;
                enumeration(left, 1)?;
                enumeration(right, 1)?;
                self.points
                    .try_reserve(1)
                    .map_err(|_| CurveRecordError::Allocation)?;
                self.points.push(CurvePointRecord { x, y, left, right });
            }
        }
        if self.kind == CurveKind::Noise {
            // Source rebuild follows all reads. These malformed scalar values
            // fail before a fresh outer layer can commit. Persisted DTOs don't
            // create sampling caches; those are required by the evaluator.
            if self.controls[18].scalar().is_nan() {
                return Err(CurveRecordError::NoiseSeed);
            }
            if self.controls[14].scalar().is_nan() || self.controls[17].scalar().is_nan() {
                return Err(CurveRecordError::Allocation);
            }
        }
        Ok(())
    }
    pub(crate) fn save(&self, writer: &mut BinaryWriter) -> Result<(), CurveRecordError> {
        writer.write_i32(self.mode)?;
        for control in &self.controls {
            control.save(writer)?;
        }
        if self.kind == CurveKind::Custom {
            writer.write_bool(self.loop_points)?;
            writer.write_i32(
                i32::try_from(self.points.len()).map_err(|_| CurveRecordError::Allocation)?,
            )?;
            for point in &self.points {
                writer.write_f64(f64::from(point.x))?;
                writer.write_f64(f64::from(point.y))?;
                writer.write_i32(point.left)?;
                writer.write_i32(point.right)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct CurvePointRecord {
    pub x: f32,
    pub y: f32,
    pub left: i32,
    pub right: i32,
}
#[derive(Clone, Debug)]
pub struct CurveControlRecord {
    pub enabled: bool,
    pub name: Vec<u16>,
    pub id: i32,
    pub value: CurveControlValue,
}
#[derive(Clone, Debug)]
pub enum CurveControlValue {
    Toggle(bool),
    Enumeration {
        value: i32,
        maximum: i32,
    },
    Scalar {
        value: f32,
        minimum: f32,
        maximum: f32,
    },
    Modulated {
        mode: i32,
        scalar: f32,
        minimum: f32,
        maximum: f32,
        child: Option<Box<CurveLayerRecord>>,
    },
}
impl CurveControlRecord {
    fn scalar(&self) -> f32 {
        match self.value {
            CurveControlValue::Scalar { value, .. } => value,
            CurveControlValue::Modulated { scalar, .. } => scalar,
            _ => 0.0,
        }
    }
    fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), CurveRecordError> {
        self.enabled = reader.read_bool()?;
        self.name = reader.read_string()?;
        self.id = reader.read_i32()?;
        match &mut self.value {
            CurveControlValue::Toggle(value) => *value = reader.read_bool()?,
            CurveControlValue::Enumeration { value, maximum } => {
                *value = reader.read_i32()?;
                enumeration(*value, *maximum)?;
            }
            CurveControlValue::Scalar {
                value,
                minimum,
                maximum,
            } => {
                *value = scalar(reader, version)?;
                range(*value, *minimum, *maximum)?;
            }
            CurveControlValue::Modulated {
                mode,
                scalar: value,
                minimum,
                maximum,
                child,
            } => {
                *mode = reader.read_i32()?;
                enumeration(*mode, 3)?;
                if self.enabled {
                    let mut nested =
                        CurveLayerRecord::new(CurveKind::from_tag(reader.read_i32()?)?);
                    nested.load(reader, version)?;
                    *child = Some(Box::new(nested));
                } else {
                    *value = scalar(reader, version)?;
                    range(*value, *minimum, *maximum)?;
                }
            }
        }
        Ok(())
    }
    fn save(&self, writer: &mut BinaryWriter) -> Result<(), CurveRecordError> {
        writer.write_bool(self.enabled)?;
        writer.write_utf16(&self.name)?;
        writer.write_i32(self.id)?;
        match &self.value {
            CurveControlValue::Toggle(value) => writer.write_bool(*value)?,
            CurveControlValue::Enumeration { value, .. } => writer.write_i32(*value)?,
            CurveControlValue::Scalar { value, .. } => writer.write_f32(*value)?,
            CurveControlValue::Modulated {
                mode,
                scalar,
                child,
                ..
            } => {
                writer.write_i32(*mode)?;
                if self.enabled {
                    let child = child.as_ref().ok_or(CurveRecordError::MissingChild)?;
                    writer.write_i32(child.kind.tag())?;
                    child.save(writer)?;
                } else {
                    writer.write_f32(*scalar)?;
                }
            }
        }
        Ok(())
    }
}
fn control(id: i32, name: &str, value: CurveControlValue) -> CurveControlRecord {
    CurveControlRecord {
        enabled: false,
        name: name.encode_utf16().collect(),
        id,
        value,
    }
}
fn numeric(id: i32, name: &str, value: f32, minimum: f32, maximum: f32) -> CurveControlRecord {
    control(
        id,
        name,
        CurveControlValue::Scalar {
            value,
            minimum,
            maximum,
        },
    )
}
fn modulated(id: i32, name: &str, scalar: f32, mode: i32) -> CurveControlRecord {
    control(
        id,
        name,
        CurveControlValue::Modulated {
            mode,
            scalar,
            minimum: 0.0,
            maximum: 100.0,
            child: None,
        },
    )
}
fn range(value: f32, min: f32, max: f32) -> Result<(), CurveRecordError> {
    if value < min || value > max {
        Err(CurveRecordError::Validation)
    } else {
        Ok(())
    }
}
fn enumeration(value: i32, max: i32) -> Result<(), CurveRecordError> {
    if value < 0 || value > max {
        Err(CurveRecordError::Validation)
    } else {
        Ok(())
    }
}
fn scalar(reader: &mut BinaryCursor<'_>, version: f64) -> Result<f32, CurveRecordError> {
    if version >= 3.12 || version.is_nan() {
        return Ok(reader.read_f32()?);
    }
    Ok(reader.read_decimal()?.to_f64() as f32)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurveRecordError {
    Binary(BinaryError),
    Validation,
    TypeIndex,
    MissingChild,
    NoiseSeed,
    Allocation,
}
impl From<BinaryError> for CurveRecordError {
    fn from(error: BinaryError) -> Self {
        Self::Binary(error)
    }
}
impl CurveRecordError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Binary(error) => error.source_class(),
            Self::Validation => "Exception",
            Self::TypeIndex => "IndexOutOfRangeException",
            Self::MissingChild => "NullReferenceException",
            Self::NoiseSeed => "OverflowException",
            Self::Allocation => "OutOfMemoryException",
        }
    }
}
impl fmt::Display for CurveRecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "curve record {:?}: {}", self, self.source_class())
    }
}
impl Error for CurveRecordError {}
