//! Actual os rr value and its existing bh/Gradient evaluation authority.
use super::*;
impl<E: 'static> PointwiseParameter<E> {
    /// Read the linked Lab swatch when active, or the property's owned Color.
    pub fn color(&self) -> Result<crate::ContextColorValue<E>, PointwiseError<E>> {
        let owned = self.owned_color()?;
        let wire = self.wire_metadata();
        if let Some(color) = wire.resolved_lab_color()
            && wire.is_adjusted()?
        {
            return Ok(crate::ContextColorValue::unlinked(color));
        }
        Ok(owned)
    }

    fn owned_color(&self) -> Result<crate::ContextColorValue<E>, PointwiseError<E>> {
        match self.raw_value() {
            PointwiseParameterValue::Color(value) => Ok(value),
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    /// os.e(rr): component equality ignores APV. Equal RGB leaves the old
    /// reference intact; publication precedes notification outside the guard.
    pub fn set_color(&self, value: crate::ContextColorValue<E>) -> Result<(), PointwiseError<E>> {
        self.set_color_with_temporary(value, false)
    }
    /// os.e(rr,bool): the caller's temporary flag belongs to the real undo
    /// payload; the committed value and its input reference remain authoritative.
    pub fn set_color_with_temporary(
        &self,
        value: crate::ContextColorValue<E>,
        temporary: bool,
    ) -> Result<(), PointwiseError<E>> {
        let old = {
            let mut entries = guard(&self.owner.core.filters);
            let PointwiseParameterValue::Color(current) =
                &mut entries[self.owner.ix].parameters[self.ix].value
            else {
                return Err(PointwiseError::InvalidCast);
            };
            if current.equals_components(&value) {
                return Ok(());
            }
            std::mem::replace(current, value)
        };
        let undo = if self.can_create_property_undo()? {
            Some(Arc::new(crate::ContextColorUndoInfo::with_values(
                Some(self.name()),
                6,
                old,
                self.owned_color()?,
                temporary,
            )))
        } else {
            None
        };
        self.notify_change(Arc::new(crate::ContextChangeArgument::Property(
            crate::ContextPropertyChangeArgs::with_color_undo(undo),
        )))
    }
    /// Actual os.a(apv)/rr.lh reference assignment has no component-change
    /// event. Only genuine existing SharedColorInput owners can be supplied.
    pub fn set_color_input_reference(
        &self,
        input: Option<&crate::SharedColorInput<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let current = self.owned_color()?;
        let value = input.map_or_else(
            || crate::ContextColorValue::unlinked(current.components()),
            |i| current.with_input_reference(i),
        );
        let old = {
            let mut entries = guard(&self.owner.core.filters);
            let PointwiseParameterValue::Color(current) =
                &mut entries[self.owner.ix].parameters[self.ix].value
            else {
                return Err(PointwiseError::InvalidCast);
            };
            std::mem::replace(current, value)
        };
        drop(old);
        Ok(())
    }
    pub fn effective_color_at(
        &self,
        time: f64,
    ) -> Result<crate::ContextColorValue<E>, PointwiseError<E>> {
        if let Some(animation) = self.retained_animation()?
            && animation.is_active()?
        {
            return Ok(crate::ContextColorValue::unlinked(
                animation.sample_color(time)?,
            ));
        }
        self.ensure_property_evaluation_supported()?;
        if let Some(value) = self.animated_binding_value(time)? {
            return Ok(crate::ContextColorValue::unlinked(value.color()));
        }
        self.color()
    }
}
impl<E: 'static> PointwiseFilter<E> {
    /// ek.ds dispatches a real native APV editor only on the unadjusted,
    /// unanimated, positive-reference branch. That host route is unported.
    pub fn open_selector_color_editor(&self) -> Result<(), PointwiseError<E>> {
        if self.kind() != PointwiseKind::Selector {
            return Err(PointwiseError::InvalidCast);
        }
        let p = self.parameter(0)?;
        if p.retained_adjustment()
            .map(|a| a.is_active())
            .transpose()?
            .unwrap_or(false)
        {
            return Ok(());
        }
        if super::super::parameter_ops::is_animated(|| p.retained_animation())? {
            return Ok(());
        }
        if p.color()?.input_reference().is_some() {
            return Err(PointwiseError::UnsupportedCapability(
                "Selector APV native color editor",
            ));
        }
        Ok(())
    }
}
