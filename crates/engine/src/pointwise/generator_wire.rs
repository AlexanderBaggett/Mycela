//! Legacy os wire IO over real generator property storage.
use super::*;
use crate::pointwise::property::wire::{
    self, AdjustmentSpec, LegacyWireMetadata, LivePropertyWireError, SharedPropertyAdjustment,
    check_code, save_code, validate_wire_number,
};
impl<E> GradientGeneratorParameter<E> {
    pub(crate) fn wire_metadata(&self) -> LegacyWireMetadata<E> {
        guard(&self.owner.state()).wire[self.kind.ix()].clone()
    }
    pub fn binding_index(&self) -> i32 {
        self.wire_metadata().binding_index()
    }
    pub fn binding_name(&self) -> Option<Vec<u16>> {
        self.wire_metadata().binding_name()
    }
    pub fn set_binding_index(&self, index: i32) {
        self.wire_metadata().set_binding_index(index);
    }
    pub fn set_binding_name(&self, name: Option<&[u16]>) {
        self.wire_metadata().set_binding_name(name);
    }
    pub fn is_animatable(&self) -> bool {
        self.wire_metadata().animatable()
    }
    pub fn is_adjustable(&self) -> bool {
        self.wire_metadata().adjustable()
    }
    pub fn retained_adjustment(&self) -> Option<SharedPropertyAdjustment<E>> {
        self.wire_metadata().retained_adjustment()
    }
}
impl<E: 'static> GradientGenerator<E> {
    pub fn ensure_property_evaluation_supported(&self) -> Result<(), PointwiseError<E>> {
        for p in self.parameters() {
            p.ensure_property_evaluation_supported()?;
        }
        Ok(())
    }
}
impl<E: 'static> GradientGeneratorParameter<E> {
    pub fn ensure_property_evaluation_supported(&self) -> Result<(), PointwiseError<E>> {
        if self.binding_index() < 0 || !self.animated_binding_supported()? {
            self.wire_metadata().ensure_evaluation_supported()?;
        }
        if self.is_choice()
            && let Some(animation) = self.retained_animation()?
        {
            // Validate the flags just as the choice getter does, including a
            // malformed retained animation whose flags are absent.
            animation.is_active()?;
        }
        if self.kind == GradientGeneratorParameterKind::Image {
            let animation = guard(&self.owner.state()).animations[self.kind.ix()].clone();
            if animation
                .map(|a| a.is_active())
                .transpose()?
                .unwrap_or(false)
            {
                return Err(PointwiseError::UnsupportedCapability(
                    "positive generator custom animation evaluation",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn wire_limits(&self) -> Result<Option<SharedLimits>, PointwiseError<E>> {
        if self.is_choice() {
            wire::choice_limits(self.options()?.entries().len()).map(Some)
        } else if self.kind == GradientGeneratorParameterKind::Image {
            Ok(None)
        } else {
            self.retained_limits()
        }
    }
    fn wire_animation(&self) -> Result<Option<GradientAnimation<E>>, LivePropertyWireError<E>> {
        let ix = self.kind.ix();
        if let Some(a) = guard(&self.owner.state()).animations[ix].clone() {
            return Ok(Some(a));
        }
        if !self.is_animatable() {
            return Ok(None);
        }
        let a = GradientAnimation::new(if self.is_paired() { 2 } else { 1 }, self.wire_limits()?)?;
        guard(&self.owner.state()).animations[ix] = Some(a.clone());
        Ok(Some(a))
    }
    pub(in crate::pointwise) fn adjustment_spec(
        &self,
    ) -> Result<AdjustmentSpec, LivePropertyWireError<E>> {
        Ok(AdjustmentSpec {
            property_type: if self.is_paired() {
                1
            } else if self.kind == GradientGeneratorParameterKind::Image {
                3
            } else {
                0
            },
            count: if self.is_paired() { 2 } else { 1 },
            limits: self.wire_limits()?,
        })
    }
    pub fn adjustment(
        &self,
    ) -> Result<Option<SharedPropertyAdjustment<E>>, LivePropertyWireError<E>> {
        let s = self.adjustment_spec()?;
        self.wire_metadata()
            .adjustment(s.property_type, s.count, s.limits.as_ref())
    }
    pub fn save_binary(
        &self,
        writer: &mut crate::BinaryWriter,
    ) -> Result<(), LivePropertyWireError<E>> {
        let name: Vec<u16> = self.name().encode_utf16().collect();
        save_code(writer, &name)?;
        if self.is_paired() {
            writer.write_f64(self.pair()?[0])?;
            writer.write_f64(self.pair()?[1])?;
        } else if self.is_choice() {
            writer.write_optional_utf16(self.text()?.as_deref())?;
        } else if self.kind == GradientGeneratorParameterKind::Image {
            wire::save_custom(
                self.custom_value()?
                    .as_ref()
                    .ok_or(PointwiseError::NullReference)?,
                writer,
                false,
            )?;
        } else {
            writer.write_f64(self.number()?)?;
        }
        self.wire_metadata()
            .save_tail(writer, || self.wire_animation())
    }
    pub fn load_binary(
        &self,
        reader: &mut crate::BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), LivePropertyWireError<E>> {
        let name: Vec<u16> = self.name().encode_utf16().collect();
        check_code(reader, &name, false)?;
        if self.is_paired() {
            for axis in 0..2 {
                let value = reader.read_f64()?;
                validate_wire_number(value, &self.limits()?)?;
                self.set_axis(axis, value)?;
            }
        } else if self.is_choice() {
            self.set_text(Some(&reader.read_string()?))?;
        } else if self.kind == GradientGeneratorParameterKind::Image {
            wire::load_custom(
                self.custom_value()?
                    .as_ref()
                    .ok_or(PointwiseError::NullReference)?,
                reader,
                version,
            )?;
            self.refresh_wire_previous()?;
        } else {
            let value = reader.read_f64()?;
            validate_wire_number(value, &self.limits()?)?;
            self.set_number(value)?;
        }
        self.wire_metadata().load_tail(
            reader,
            version,
            || self.wire_animation(),
            || self.adjustment_spec(),
        )
    }
}
