//! Outer-context wire children of the actual retained Invert selection owners.
//! Current/previous choices, options and binding metadata remain in Selection.
use super::*;
use crate::pointwise::property::capture::capture_values::Comparison;
use crate::pointwise::property::wire::{
    AdjustmentSpec, LegacyWireMetadata, LivePropertyWireError, SharedPropertyAdjustment,
    check_code, save_code,
};
use crate::{
    BinaryCursor, BinaryWriter, DecimalBits, GradientAnimation, LimitsArithmeticProfile,
    LimitsDefinition, LimitsStep, NodeWireError, RenderCaptureSession, SharedLimits,
};
use std::sync::Mutex;
fn guard<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
struct InvertWireProperty<E> {
    metadata: LegacyWireMetadata<E>,
    animation: Mutex<Option<GradientAnimation<E>>>,
}
pub(in crate::pointwise) struct InvertWireOwner<E> {
    properties: [InvertWireProperty<E>; 4],
}
impl<E: 'static> InvertWireOwner<E> {
    pub(in crate::pointwise) fn new() -> Arc<Self> {
        Arc::new(Self {
            properties: std::array::from_fn(|_| InvertWireProperty {
                metadata: LegacyWireMetadata::without_binding(true, true),
                animation: Mutex::new(None),
            }),
        })
    }
    pub(in crate::pointwise) fn source_copy(&self) -> Result<Arc<Self>, PointwiseError<E>> {
        let mut properties = Vec::with_capacity(4);
        for p in &self.properties {
            let animation = guard(&p.animation).clone();
            let animation = animation.map(|a| a.source_clone()).transpose()?;
            let metadata = p.metadata.source_clone()?;
            properties.push(InvertWireProperty {
                metadata,
                animation: Mutex::new(animation),
            });
        }
        Ok(Arc::new(Self {
            properties: properties
                .try_into()
                .map_err(|_| PointwiseError::InvalidCast)?,
        }))
    }
    pub(in crate::pointwise) fn capture_with(
        &self,
        session: &mut RenderCaptureSession<E>,
    ) -> Result<Arc<Self>, PointwiseError<E>> {
        let mut properties = Vec::with_capacity(4);
        for p in &self.properties {
            let animation = guard(&p.animation).clone();
            properties.push(InvertWireProperty {
                metadata: p.metadata.capture_with(|a| session.copy_adjustment(a))?,
                animation: Mutex::new(
                    animation
                        .as_ref()
                        .map(|a| session.copy_animation(a))
                        .transpose()?,
                ),
            });
        }
        Ok(Arc::new(Self {
            properties: properties
                .try_into()
                .map_err(|_| PointwiseError::InvalidCast)?,
        }))
    }
    fn ensure_property_supported(&self, ix: usize, timed: bool) -> Result<(), PointwiseError<E>> {
        let p = &self.properties[ix];
        p.metadata.ensure_evaluation_supported()?;
        if timed {
            return Ok(());
        }
        let animation = guard(&p.animation).clone();
        if animation
            .map(|a| a.is_active())
            .transpose()?
            .unwrap_or(false)
        {
            return Err(PointwiseError::UnsupportedCapability(
                "positive live Invert animation evaluation",
            ));
        }
        Ok(())
    }
    pub(in crate::pointwise) fn ensure_evaluation_supported(
        &self,
        timed: bool,
    ) -> Result<(), PointwiseError<E>> {
        for ix in 0..4 {
            self.ensure_property_supported(ix, timed)?;
        }
        Ok(())
    }
    pub(in crate::pointwise) fn capture_values(
        &self,
        comparison: &mut Comparison<E>,
        out: &mut Vec<u64>,
    ) -> Result<(), PointwiseError<E>> {
        for p in &self.properties {
            comparison.metadata(&p.metadata, out)?;
            comparison.animation(guard(&p.animation).clone(), out)?;
        }
        Ok(())
    }
}
impl<E: 'static> PointwiseNode<E> {
    fn invert_wire(&self) -> Result<Arc<InvertWireOwner<E>>, PointwiseError<E>> {
        match &self.core().payload {
            Payload::Invert { wire, .. } => Ok(wire.clone()),
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    pub(in crate::pointwise) fn ensure_invert_wire_property_evaluation_supported(
        &self,
        index: usize,
    ) -> Result<(), PointwiseError<E>> {
        self.invert_wire()?
            .ensure_property_supported(index, false)?;
        self.invert_property(index as i32)
            .map_err(graph_error)?
            .ensure_binding_supported()
    }
    pub(in crate::pointwise) fn ensure_invert_wire_evaluation_supported(
        &self,
    ) -> Result<(), PointwiseError<E>> {
        self.ensure_invert_evaluation_supported(false)
    }
    pub(in crate::pointwise) fn ensure_invert_evaluation_supported(
        &self,
        timed: bool,
    ) -> Result<(), PointwiseError<E>> {
        let wire = self.invert_wire()?;
        wire.ensure_evaluation_supported(timed)?;
        for property in self.invert_properties().map_err(graph_error)? {
            property.ensure_binding_supported()?;
        }
        Ok(())
    }

    pub(in crate::pointwise) fn invert_guarantee_selection(
        &self,
        index: usize,
    ) -> Result<Option<i32>, PointwiseError<E>> {
        self.invert_wire()?.ensure_property_supported(index, true)?;
        let property = self.invert_property(index as i32).map_err(graph_error)?;
        property.ensure_binding_supported()?;
        if property.is_animated()? {
            return Ok(None);
        }
        property.current().map(Some).map_err(graph_error)
    }
    pub(crate) fn save_invert_wire_property(
        &self,
        index: usize,
        writer: &mut BinaryWriter,
    ) -> Result<(), NodeWireError<E>> {
        if index >= 4 {
            return Err(NodeWireError::PropertyIndex);
        }
        let property = self
            .invert_property(index as i32)
            .map_err(NodeWireError::Invert)?;
        save_code(
            writer,
            property.snapshot().map_err(NodeWireError::Invert)?.name(),
        )?;
        writer.write_optional_utf16(property.text().map_err(NodeWireError::Invert)?.as_deref())?;
        let bound = property
            .snapshot()
            .map_err(NodeWireError::Invert)?
            .binding_index()
            != -1;
        writer.write_bool(bound)?;
        if bound {
            writer.write_i16(
                property
                    .snapshot()
                    .map_err(NodeWireError::Invert)?
                    .binding_index() as i16,
            )?;
            writer.write_utf16(
                property
                    .snapshot()
                    .map_err(NodeWireError::Invert)?
                    .binding_name()
                    .as_deref()
                    .unwrap_or_default(),
            )?;
        }
        property
            .wire_property()?
            .metadata
            .save_animation_adjustment_tail(writer, || property.animation())?;
        Ok(())
    }
    pub(crate) fn load_invert_wire_property(
        &self,
        index: usize,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), NodeWireError<E>> {
        if index >= 4 {
            return Err(NodeWireError::PropertyIndex);
        }
        let property = self
            .invert_property(index as i32)
            .map_err(NodeWireError::Invert)?;
        check_code(
            reader,
            property.snapshot().map_err(NodeWireError::Invert)?.name(),
            false,
        )?;
        property
            .set_text(Some(reader.read_string()?))
            .map_err(NodeWireError::Invert)?;
        if reader.read_bool()? {
            property
                .set_binding_index(i32::from(reader.read_i16()?))
                .map_err(NodeWireError::Invert)?;
            property
                .set_binding_name(Some(reader.read_string()?))
                .map_err(NodeWireError::Invert)?;
        }
        property
            .wire_property()?
            .metadata
            .load_animation_adjustment_tail(
                reader,
                version,
                || property.animation(),
                || {
                    Ok(AdjustmentSpec {
                        property_type: 0,
                        count: 1,
                        limits: Some(property.choice_limits()?),
                    })
                },
            )?;
        Ok(())
    }
}
impl<E: 'static> PointwiseInvertProperty<E> {
    fn ensure_binding_supported(&self) -> Result<(), PointwiseError<E>> {
        if self.snapshot().map_err(graph_error)?.binding_index() >= 0 {
            return Err(PointwiseError::UnsupportedCapability(
                "positive Invert document binding evaluation",
            ));
        }
        Ok(())
    }
    pub(in crate::pointwise) fn is_animated(&self) -> Result<bool, PointwiseError<E>> {
        super::super::parameter_ops::is_animated(|| self.retained_animation())
    }

    pub(in crate::pointwise) fn effective_index_at(
        &self,
        option_count: usize,
        time: f64,
    ) -> Result<i32, PointwiseError<E>> {
        if self.is_animated()? {
            let curve = super::super::parameter_ops::curve(|| self.retained_animation(), 0)?;
            super::super::parameter_ops::animated_choice_index(curve, option_count, time)
        } else {
            self.current().map_err(graph_error)
        }
    }

    fn wire_owner(&self) -> Result<Arc<InvertWireOwner<E>>, PointwiseError<E>> {
        self.owner.invert_wire()
    }
    fn wire_property(&self) -> Result<InvertWirePropertyHandle<E>, PointwiseError<E>> {
        Ok(InvertWirePropertyHandle {
            owner: self.wire_owner()?,
            index: self.channel as usize,
        })
    }
    pub(in crate::pointwise) fn choice_limits(
        &self,
    ) -> Result<SharedLimits, LivePropertyWireError<E>> {
        let count = self.options().map_err(graph_error)?.entries().len();
        if count > i32::MAX as usize {
            return Err(PointwiseError::Image(crate::ImageError::Overflow).into());
        }
        let max = (count as u128)
            .checked_mul(10)
            .and_then(|v| v.checked_add(5))
            .ok_or(PointwiseError::Arithmetic)?;
        if max > ((1_u128 << 96) - 1) {
            return Err(PointwiseError::Arithmetic.into());
        }
        let maximum =
            DecimalBits::new([max as u32, (max >> 32) as u32, (max >> 64) as u32, 1 << 16])?;
        SharedLimits::from_definition_with_profile(
            LimitsDefinition {
                decimal_places: 1,
                step: LimitsStep::Derived(DecimalBits::new([1, 0, 0, 1 << 16])?),
                minimum: DecimalBits::new([5, 0, 0, 1 << 16])?,
                maximum,
                ..LimitsDefinition::default()
            },
            LimitsArithmeticProfile::Framework64OleAut32TenFirst,
        )
        .map_err(|_| PointwiseError::Arithmetic.into())
    }
    pub fn retained_animation(&self) -> Result<Option<GradientAnimation<E>>, PointwiseError<E>> {
        let p = self.wire_property()?;
        let value = guard(&p.animation).clone();
        Ok(value)
    }
    /// Actual lazy choice bh creation; an existing root survives disabling.
    pub fn animation(&self) -> Result<Option<GradientAnimation<E>>, LivePropertyWireError<E>> {
        let p = self.wire_property()?;
        let retained = guard(&p.animation).clone();
        if retained.is_some() || !p.metadata.animatable() {
            return Ok(retained);
        }
        let animation = GradientAnimation::new(1, Some(self.choice_limits()?))?;
        *guard(&p.animation) = Some(animation.clone());
        Ok(Some(animation))
    }
    pub fn adjustment(
        &self,
    ) -> Result<Option<SharedPropertyAdjustment<E>>, LivePropertyWireError<E>> {
        let p = self.wire_property()?;
        if !p.metadata.adjustable() {
            return Ok(p.metadata.retained_adjustment());
        }
        if let Some(value) = p.metadata.retained_adjustment() {
            return Ok(Some(value));
        }
        p.metadata.adjustment(0, 1, Some(&self.choice_limits()?))
    }
    pub(crate) fn retained_adjustment(&self) -> Option<SharedPropertyAdjustment<E>> {
        self.wire_property().ok()?.metadata.retained_adjustment()
    }
    pub(in crate::pointwise) fn adjustment_metadata(&self) -> Option<LegacyWireMetadata<E>> {
        Some(self.wire_property().ok()?.metadata.clone())
    }
    pub fn animatable(&self) -> Result<bool, PointwiseError<E>> {
        Ok(self.wire_property()?.metadata.animatable())
    }
    pub fn adjustable(&self) -> Result<bool, PointwiseError<E>> {
        Ok(self.wire_property()?.metadata.adjustable())
    }
}
// A borrowed field view retains its actual outer-context owner; no duplicate
// choice, binding or adjustment state is projected into this adapter.
struct InvertWirePropertyHandle<E> {
    owner: Arc<InvertWireOwner<E>>,
    index: usize,
}
impl<E> std::ops::Deref for InvertWirePropertyHandle<E> {
    type Target = InvertWireProperty<E>;
    fn deref(&self) -> &Self::Target {
        &self.owner.properties[self.index]
    }
}

#[cfg(test)]
mod laws {
    use super::*;
    #[test]
    fn failed_binding_transport_does_not_reach_lazy_animation() {
        let node = PointwiseNode::<()>::invert();
        let p = node.invert_property(0).unwrap();
        p.set_binding_index(7).unwrap();
        p.set_binding_name(Some(vec![65, 66])).unwrap();
        let mut prefix = BinaryWriter::default();
        save_code::<()>(&mut prefix, p.snapshot().unwrap().name()).unwrap();
        prefix
            .write_optional_utf16(p.text().unwrap().as_deref())
            .unwrap();
        let mut writer = BinaryWriter::default();
        writer.set_write_failure_at(Some(prefix.bytes().len()));
        assert!(matches!(
            node.save_invert_wire_property(0, &mut writer),
            Err(NodeWireError::Binary(crate::BinaryError::CallerWrite))
        ));
        assert_eq!(writer.bytes(), prefix.bytes());
        assert!(p.retained_animation().unwrap().is_none());
        assert!(
            p.wire_property()
                .unwrap()
                .metadata
                .retained_adjustment()
                .is_none()
        );
    }
    #[test]
    fn bound_null_name_is_an_actual_empty_string_roundtrip() {
        let node = PointwiseNode::<()>::invert();
        let property = node.invert_property(0).unwrap();
        property.set_binding_index(4).unwrap();
        property.set_binding_name(None).unwrap();
        let mut writer = BinaryWriter::default();
        node.save_binary(&mut writer).unwrap();
        let mut reader = BinaryCursor::new(writer.bytes()).unwrap();
        let name = reader.read_string().unwrap();
        let loaded = crate::load_node_binary::<()>(&mut reader, 3.75, Some(&name)).unwrap();
        let property = loaded.invert_property(0).unwrap();
        assert_eq!(property.snapshot().unwrap().binding_index(), 4);
        assert_eq!(property.snapshot().unwrap().binding_name(), None);
        let mut again = BinaryWriter::default();
        loaded.save_binary(&mut again).unwrap();
        assert_eq!(again.bytes(), writer.bytes());
    }
    #[test]
    fn actual_all_choice_tails_and_owned_capture_remain_distinct_from_source_copy() {
        let node = PointwiseNode::<()>::invert();
        let mut writer = BinaryWriter::default();
        node.save_binary(&mut writer).unwrap();
        let mut reader = BinaryCursor::new(writer.bytes()).unwrap();
        let name = reader.read_string().unwrap();
        let loaded = crate::load_node_binary::<()>(&mut reader, 3.75, Some(&name)).unwrap();
        assert_eq!(reader.position().unwrap() as usize, writer.bytes().len());
        for i in 0..4 {
            let a = node.invert_property(i).unwrap();
            let b = loaded.invert_property(i).unwrap();
            assert_eq!(a.current().unwrap(), b.current().unwrap());
            assert!(a.retained_animation().unwrap().is_some());
            assert!(
                !a.retained_animation()
                    .unwrap()
                    .unwrap()
                    .same_reference(&b.retained_animation().unwrap().unwrap())
            );
        }
        let live = node
            .invert_property(0)
            .unwrap()
            .retained_animation()
            .unwrap()
            .unwrap();
        let source_copy = node.invert_wire().unwrap().source_copy().unwrap();
        let mut session = RenderCaptureSession::new();
        let capture = node
            .invert_wire()
            .unwrap()
            .capture_with(&mut session)
            .unwrap();
        drop(session);
        let cloned = guard(&source_copy.properties[0].animation).clone().unwrap();
        let owned = guard(&capture.properties[0].animation).clone().unwrap();
        assert!(
            live.limits_for(0)
                .unwrap()
                .unwrap()
                .same_reference(&cloned.limits_for(0).unwrap().unwrap())
        );
        assert!(
            !live
                .limits_for(0)
                .unwrap()
                .unwrap()
                .same_reference(&owned.limits_for(0).unwrap().unwrap())
        );
        live.flags().unwrap().set(0, true).unwrap();
        assert!(!cloned.is_active().unwrap());
        assert!(!owned.is_active().unwrap());
        assert!(node.ensure_invert_wire_evaluation_supported().is_err());
    }
}
