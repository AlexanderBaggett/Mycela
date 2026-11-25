//! Sequential live composite loading and current-format saving.
// Source rl.cs:312-343 / primary0600048d. The same factory should also serve
// ef.Load's child creation. Source tags differ from Rust enum declaration order.
use super::*;
use crate::{BinaryCursor, BinaryWriter};

const PRESET_SIGNATURE: &[u8] = b"\x10CompositeCurve&#";
const PRESET_VERSION: i32 = 383;
const PRESET_LAYER_DEPTH: usize = 128;

impl<E> CurveLayerHandle<E> {
    pub(super) fn from_source_tag(tag: i32) -> Result<Self, CurveError<E>> {
        Ok(match tag {
            0 => Self::new(AnalyticCurveKind::Sine),
            1 => Self::new(AnalyticCurveKind::Linear),
            2 => Self::new(AnalyticCurveKind::Arc),
            3 => Self::new(AnalyticCurveKind::Bell),
            4 => Self::new(AnalyticCurveKind::Gain),
            5 => Self::new_noise()?,
            6 => Self::new(AnalyticCurveKind::Steps),
            7 => Self::new_custom(),
            _ => return Err(CurveError::Index),
        })
    }
}

impl<E> SharedCurve<E> {
    /// Read a standalone Genetica `.curv` file into a detached composite.
    pub fn read_preset(reader: &mut BinaryCursor<'_>) -> Result<Self, CurveError<E>> {
        if reader.read_bytes(PRESET_SIGNATURE.len() as i32)? != PRESET_SIGNATURE {
            return Err(CurveError::InvalidData);
        }
        let version = reader.read_i32()?;
        if version > PRESET_VERSION {
            return Err(CurveError::UnsupportedCapability);
        }
        if version < 0 {
            return Err(CurveError::InvalidData);
        }
        let curve = Self::new(0.0, 1.0, false);
        curve.load_with_depth(reader, f64::from(version) / 100.0, PRESET_LAYER_DEPTH)?;
        Ok(curve)
    }

    /// Write the standalone wrapper and the regular composite curve record.
    pub fn save_preset(&self, writer: &mut BinaryWriter) -> Result<(), CurveError<E>> {
        writer.write_bytes(PRESET_SIGNATURE)?;
        writer.write_i32(PRESET_VERSION)?;
        self.save_with_depth(writer, PRESET_LAYER_DEPTH)
    }

    pub fn load(&self, reader: &mut BinaryCursor<'_>, version: f64) -> Result<(), CurveError<E>> {
        self.load_with_depth(reader, version, usize::MAX)
    }

    fn load_with_depth(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
        depth_remaining: usize,
    ) -> Result<(), CurveError<E>> {
        // Source clears old entries before reading even the count. Caller-held
        // old layers/controls remain actual retained objects in their cohort.
        self.clear();
        let count = reader.read_i32()?;
        if count < 0 {
            return Err(CurveError::InvalidData);
        }
        for _ in 0..count {
            let layer = CurveLayerHandle::from_source_tag(reader.read_i32()?)?;
            // A failing Load never publishes this particular layer to the
            // composite; previously completed layers stay committed.
            layer.load_with_depth(reader, version, depth_remaining)?;
            self.append(&[Some(layer)]);
        }
        // Each field commits separately. This is deliberately not DTO parsing
        // followed by replacing the complete model on success.
        let minimum = codec::read_scalar(reader, version)?;
        self.set_minimum(minimum);
        let maximum = codec::read_scalar(reader, version)?;
        self.set_maximum(maximum);
        self.set_wrapped(reader.read_bool()?);
        Ok(())
    }
}

impl<E> CurveLayerHandle<E> {
    pub fn source_tag(&self) -> i32 {
        match self.kind() {
            CurveLayerKind::Sine => 0,
            CurveLayerKind::Linear => 1,
            CurveLayerKind::Arc => 2,
            CurveLayerKind::Bell => 3,
            CurveLayerKind::Gain => 4,
            CurveLayerKind::Noise => 5,
            CurveLayerKind::Steps => 6,
            CurveLayerKind::Custom => 7,
        }
    }
    pub fn save(&self, writer: &mut crate::BinaryWriter) -> Result<(), CurveError<E>> {
        self.save_with_depth(writer, usize::MAX)
    }

    fn save_with_depth(
        &self,
        writer: &mut BinaryWriter,
        depth_remaining: usize,
    ) -> Result<(), CurveError<E>> {
        let control_depth = depth_remaining
            .checked_sub(1)
            .ok_or(CurveError::UnsupportedCapability)?;
        writer.write_i32(self.mode())?;
        for ix in 0..14 {
            self.common(ix)?.save_with_depth(writer, control_depth)?;
        }
        for control in self.shape_controls() {
            control.save_with_depth(writer, control_depth)?;
        }
        if self.kind() == CurveLayerKind::Custom {
            self.save_custom(writer)?;
        }
        Ok(())
    }
}
impl<E> SharedCurve<E> {
    pub fn save(&self, writer: &mut crate::BinaryWriter) -> Result<(), CurveError<E>> {
        self.save_with_depth(writer, usize::MAX)
    }

    fn save_with_depth(
        &self,
        writer: &mut BinaryWriter,
        depth_remaining: usize,
    ) -> Result<(), CurveError<E>> {
        let layers = self.layers();
        writer.write_i32(i32::try_from(layers.len()).map_err(|_| CurveError::Overflow)?)?;
        for layer in layers {
            let layer = layer.ok_or(CurveError::NullReference)?;
            writer.write_i32(layer.source_tag())?;
            layer.save_with_depth(writer, depth_remaining)?;
        }
        writer.write_f32(self.minimum())?;
        writer.write_f32(self.maximum())?;
        writer.write_bool(self.is_wrapped())?;
        Ok(())
    }
}
impl<E> CurveControlHandle<E> {
    pub fn save(&self, writer: &mut crate::BinaryWriter) -> Result<(), CurveError<E>> {
        self.save_with_depth(writer, usize::MAX)
    }

    fn save_with_depth(
        &self,
        writer: &mut BinaryWriter,
        depth_remaining: usize,
    ) -> Result<(), CurveError<E>> {
        writer.write_bool(self.is_active())?;
        writer.write_optional_utf16(self.name().as_deref())?;
        writer.write_i32(self.id())?;
        match self.value() {
            Value::Toggle(value) => writer.write_bool(value)?,
            Value::Scalar(value) => writer.write_f32(value)?,
            Value::Direction(value) | Value::Blend(value) => writer.write_i32(value)?,
            Value::Modulated { mode, .. } => {
                writer.write_i32(mode)?;
                if self.is_active() {
                    let child = self.retained_child().ok_or(CurveError::NullReference)?;
                    writer.write_i32(child.source_tag())?;
                    child.save_with_depth(writer, depth_remaining)?;
                } else {
                    writer.write_f32(self.scalar()?)?;
                }
            }
        }
        Ok(())
    }
}

impl<E> SharedCurve<E> {
    pub fn from_source(source: Option<&Self>) -> Result<Self, CurveError<E>> {
        source.ok_or(CurveError::NullReference)?.source_clone()
    }
    pub fn save_nullable(&self, writer: Option<&mut BinaryWriter>) -> Result<(), CurveError<E>> {
        match writer {
            Some(w) => self.save(w),
            None => self.save(&mut BinaryWriter::null_receiver()),
        }
    }
    pub fn load_nullable(
        &self,
        reader: Option<&mut BinaryCursor<'_>>,
        version: f64,
    ) -> Result<(), CurveError<E>> {
        match reader {
            Some(r) => self.load(r, version),
            None => self.load(&mut BinaryCursor::null_receiver(), version),
        }
    }
    /// rl.Equals: OTHER Save first, then SELF Save, even when roots are identical.
    pub fn source_equals_object(
        &self,
        other: crate::SourceObject<'_, E>,
    ) -> Result<bool, CurveError<E>> {
        let crate::SourceObject::Curve(other) = other else {
            return Ok(false);
        };
        let mut right = BinaryWriter::default();
        other.save(&mut right)?;
        let mut left = BinaryWriter::default();
        self.save(&mut left)?;
        Ok(right.bytes() == left.bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BinaryError, BinaryWriter};
    // Static source-body ownership/partial-state laws supplement, and do not
    // add original runtime goldens to, the437/3975 replay denominators.
    #[test]
    fn source_law_reload_keeps_retained_old_noise_receiver_separate() {
        let curve = SharedCurve::<()>::new(-3.0, 7.0, true);
        let old = CurveLayerHandle::new_noise().unwrap();
        let control = old.shape_controls()[0].clone();
        curve.append(&[Some(old.clone())]);
        let other = SharedCurve::<()>::new(0.0, 1.0, false);
        other.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        let mut writer = BinaryWriter::default();
        other.save(&mut writer).unwrap();
        curve
            .load(&mut BinaryCursor::new(writer.bytes()).unwrap(), 3.75)
            .unwrap();
        let loaded = curve.layers()[0].clone().unwrap();
        assert!(!loaded.same_reference(&old));
        assert_eq!(loaded.kind(), CurveLayerKind::Linear);
        let before = old.noise_caches().unwrap()[0].clone().unwrap();
        control.set_scalar(2.0).unwrap();
        let rebuilt = old.noise_caches().unwrap()[0].clone().unwrap();
        assert!(!before.same_reference(&rebuilt));
        assert_eq!(rebuilt.len(), 3);
        assert!(curve.layers()[0].as_ref().unwrap().same_reference(&loaded));
    }
    #[test]
    fn source_law_failed_fresh_layer_is_not_published_and_bounds_commit_separately() {
        let curve = SharedCurve::<()>::new(-3.0, 7.0, true);
        let old = CurveLayerHandle::new(AnalyticCurveKind::Sine);
        curve.append(&[Some(old.clone())]);
        let mut writer = BinaryWriter::default();
        writer.write_i32(2).unwrap();
        writer.write_i32(0).unwrap();
        old.save(&mut writer).unwrap();
        writer.write_i32(8).unwrap();
        assert_eq!(
            curve.load(&mut BinaryCursor::new(writer.bytes()).unwrap(), 3.75),
            Err(CurveError::Index)
        );
        assert_eq!(curve.layers().len(), 1);
        assert!(!curve.layers()[0].as_ref().unwrap().same_reference(&old));
        assert_eq!(curve.minimum(), -3.0);
        assert_eq!(curve.maximum(), 7.0);
        assert!(curve.is_wrapped());
        let mut writer = BinaryWriter::default();
        writer.write_i32(0).unwrap();
        writer.write_f32(2.0).unwrap();
        writer.write_u8(0).unwrap();
        assert_eq!(
            curve.load(&mut BinaryCursor::new(writer.bytes()).unwrap(), 3.75),
            Err(CurveError::Binary(BinaryError::End))
        );
        assert!(curve.layers().is_empty());
        assert_eq!(curve.minimum(), 2.0);
        assert_eq!(curve.maximum(), 7.0);
        assert!(curve.is_wrapped());
    }
    #[test]
    fn source_law_save_preserves_lazy_custom_and_null_layer_leaves_written_prefix() {
        let layer = CurveLayerHandle::<()>::new_custom();
        layer.add_custom_point(0.5, 25.0).unwrap();
        let raw = layer.custom_points().unwrap();
        let point = raw.entries()[0].clone();
        let mut writer = BinaryWriter::default();
        layer.save(&mut writer).unwrap();
        assert!(layer.custom_cache().unwrap().is_none());
        assert!(layer.is_custom_dirty().unwrap());
        assert!(raw.same_reference(&layer.custom_points().unwrap()));
        assert!(point.same_reference(&raw.entries()[0]));
        let curve = SharedCurve::<()>::new(0.0, 1.0, false);
        curve.append(&[None]);
        let mut writer = BinaryWriter::default();
        assert_eq!(curve.save(&mut writer), Err(CurveError::NullReference));
        assert_eq!(writer.bytes(), 1i32.to_le_bytes());
    }
}
