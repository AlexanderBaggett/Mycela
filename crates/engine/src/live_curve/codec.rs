//! Sequential live layer/control Load. Fields commit as their reads succeed;
//! composite loading and current-format saving share the live wire module.
use super::*;
use crate::BinaryCursor;
// rl/ac3/ef call Decimal.op_Explicit returning Single, not ToDouble.
// Use the independently recovered Framework64 Single arithmetic, including
// its separate high/low divisions and final narrowing. Decimal storage and
// the distinct Mono ToDouble law remain unchanged.
pub(super) fn read_scalar(
    reader: &mut BinaryCursor<'_>,
    version: f64,
) -> Result<f32, crate::BinaryError> {
    if version < 3.12 {
        Ok(reader.read_decimal()?.to_framework_f32())
    } else {
        reader.read_f32()
    }
}
impl<E> From<crate::BinaryError> for CurveError<E> {
    fn from(error: crate::BinaryError) -> Self {
        Self::Binary(error)
    }
}
impl<E> CurveLayerHandle<E> {
    pub fn load(&self, reader: &mut BinaryCursor<'_>, version: f64) -> Result<(), CurveError<E>> {
        self.load_with_depth(reader, version, usize::MAX)
    }

    pub(super) fn load_with_depth(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
        depth_remaining: usize,
    ) -> Result<(), CurveError<E>> {
        let control_depth = depth_remaining
            .checked_sub(1)
            .ok_or(CurveError::UnsupportedCapability)?;
        let mode = reader.read_i32()?;
        self.set_mode(mode);
        if !(0..=3).contains(&mode) {
            return Err(CurveError::InvalidData);
        }
        for ix in 0..14 {
            self.common(ix)?
                .load_with_depth(reader, version, control_depth)?;
        }
        for control in self.shape_controls() {
            control.load_with_depth(reader, version, control_depth)?;
        }
        if self.kind() == CurveLayerKind::Custom {
            self.load_custom(reader)?;
        }
        if self.kind() == CurveLayerKind::Noise {
            self.rebuild_noise(false)?
        }
        Ok(())
    }
}
impl<E> CurveControlHandle<E> {
    pub fn load(&self, reader: &mut BinaryCursor<'_>, version: f64) -> Result<(), CurveError<E>> {
        self.load_with_depth(reader, version, usize::MAX)
    }

    fn load_with_depth(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
        depth_remaining: usize,
    ) -> Result<(), CurveError<E>> {
        let active = reader.read_bool()?;
        self.set_active(active);
        let name = reader.read_string()?;
        self.0.write(|s| {
            if let Object::Control(c) = s {
                c.name = Some(name)
            }
        });
        let id = reader.read_i32()?;
        self.0.write(|s| {
            if let Object::Control(c) = s {
                c.id = id
            }
        });
        match self.kind() {
            CurveControlKind::Toggle => {
                let value = reader.read_bool()?;
                self.0.write(|s| {
                    if let Object::Control(c) = s {
                        c.value = Value::Toggle(value)
                    }
                });
            }
            CurveControlKind::Direction | CurveControlKind::Blend => {
                let value = reader.read_i32()?;
                let direction = self.kind() == CurveControlKind::Direction;
                self.0.write(|s| {
                    if let Object::Control(c) = s {
                        c.value = if direction {
                            Value::Direction(value)
                        } else {
                            Value::Blend(value)
                        }
                    }
                });
                if !(0..=if direction { 1 } else { 7 }).contains(&value) {
                    return Err(CurveError::InvalidData);
                }
            }
            CurveControlKind::Scalar | CurveControlKind::Modulated => {
                if self.kind() == CurveControlKind::Modulated {
                    let mode = reader.read_i32()?;
                    self.0.write(|s| {
                        if let Object::Control(Control {
                            value: Value::Modulated { mode: slot, .. },
                            ..
                        }) = s
                        {
                            *slot = mode
                        }
                    });
                    if !(0..=3).contains(&mode) {
                        return Err(CurveError::InvalidData);
                    }
                    if active {
                        let kind = reader.read_i32()?;
                        let child = CurveLayerHandle::from_source_tag(kind)?;
                        self.attach_loaded_child(&child);
                        return child.load_with_depth(reader, version, depth_remaining);
                    }
                }
                let value = read_scalar(reader, version)?;
                self.0.write(|s| {
                    if let Object::Control(c) = s {
                        match &mut c.value {
                            Value::Scalar(v) | Value::Modulated { scalar: v, .. } => *v = value,
                            _ => unreachable!(),
                        }
                    }
                });
                let [minimum, maximum] = self.bounds().expect("Scalar bounds");
                if value < minimum || value > maximum {
                    return Err(CurveError::InvalidData);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BinaryWriter, DecimalBits};

    // Source-body instance audit, not additional original runtime goldens:
    // ac3 and inactive ef reach the same original Single conversion operation.
    #[test]
    fn native_single_zero_is_local_to_legacy_scalar_and_modulated_load() {
        for scale in 0..=28 {
            let bits = [0, 0, 0, 0x8000_0000 | (scale << 16)];
            let value = DecimalBits::new(bits).unwrap();
            assert_eq!(value.bits(), bits);
            assert_eq!(value.to_f64().to_bits(), (-0.0f64).to_bits());
            for id in [11, 6] {
                let layer = CurveLayerHandle::<()>::new(AnalyticCurveKind::Linear);
                let control = layer.control(id).unwrap().unwrap();
                let modulated = control.kind() == CurveControlKind::Modulated;
                let mut writer = BinaryWriter::default();
                writer.write_bool(false).unwrap();
                writer.write_utf16(&[]).unwrap();
                writer.write_i32(id).unwrap();
                if modulated {
                    writer.write_i32(1).unwrap();
                }
                writer.write_decimal(value).unwrap();
                let mut reader = BinaryCursor::new(writer.bytes()).unwrap();
                control.load(&mut reader, 3.0).unwrap();
                assert_eq!(control.scalar().unwrap().to_bits(), 0);
                assert_eq!(reader.position().unwrap() as usize, writer.bytes().len());
                // Modern IEEE wire keeps its sign; this is no global zero rule.
                let mut writer = BinaryWriter::default();
                writer.write_bool(false).unwrap();
                writer.write_utf16(&[]).unwrap();
                writer.write_i32(id).unwrap();
                if modulated {
                    writer.write_i32(1).unwrap();
                }
                writer.write_f32(-0.0).unwrap();
                control
                    .load(&mut BinaryCursor::new(writer.bytes()).unwrap(), 3.12)
                    .unwrap();
                assert_eq!(control.scalar().unwrap().to_bits(), (-0.0f32).to_bits());
            }
        }
    }
}
