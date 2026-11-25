//! Live Solid Color property IO over genuine stored values and retained children.
//! Wire projections in solid_color_record remain a separate historical API.
use super::*;
use crate::pointwise::property::capture::capture_values::Comparison;
use crate::pointwise::property::wire::{
    AdjustmentSpec, LivePropertyWireError, SharedPropertyAdjustment, check_code, save_code,
    validate_wire_number,
};
use crate::{BinaryCursor, BinaryWriter, NodeWireError, PointwiseSolidEditError};

impl<E: 'static> SharedSolidColorNode<E> {
    pub(crate) fn raw_wire_values(
        &self,
        comparison: &mut Comparison<E>,
    ) -> Result<Vec<u64>, PointwiseError<E>> {
        let state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut out = vec![self.0.mode as u64, state.transparency.to_bits()];
        out.extend(state.color.map(f64::to_bits));
        for ix in 0..2 {
            let (previous, normalized, modifier) = if ix == 0 {
                (0.0_f64, 0.0_f64, 0.0_f64)
            } else {
                (state.previous, state.normalized, state.modifier)
            };
            out.extend([
                previous.to_bits(),
                normalized.to_bits(),
                modifier.to_bits(),
                state.allow[ix] as u64,
            ]);
            comparison.limits(&self.0.limits[ix], &mut out);
            let (binding, name, animatable, adjustable) = state.wire[ix].capture_values();
            out.extend([binding as u64, animatable as u64, adjustable as u64]);
            match name {
                None => out.push(0),
                Some(name) => {
                    out.push(name.len() as u64 + 1);
                    out.extend(name.into_iter().map(u64::from));
                }
            }
            comparison.animation(state.animations[ix].clone(), &mut out)?;
            comparison.adjustment(state.wire[ix].retained_adjustment(), &mut out)?;
        }
        comparison.color_input(self.raw_color_input(), &mut out);
        Ok(out)
    }
}
impl<E: 'static> SharedSolidColorProperty<E> {
    pub(crate) fn wire_metadata(&self) -> LegacyWireMetadata<E> {
        self.owner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .wire[index(self.kind)]
        .clone()
    }
    pub fn binding_index(&self) -> i32 {
        self.wire_metadata().binding_index()
    }
    pub fn binding_name(&self) -> Option<Vec<u16>> {
        self.wire_metadata().binding_name()
    }
    /// Raw unbound metadata edit; attached document binding resolution remains separate.
    pub fn set_binding_index(&self, value: i32) {
        self.wire_metadata().set_binding_index(value);
    }
    pub fn set_binding_name(&self, value: Option<&[u16]>) {
        self.wire_metadata().set_binding_name(value);
    }
    pub fn retained_animation(&self) -> Option<GradientAnimation<E>> {
        self.owner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .animations[index(self.kind)]
        .clone()
    }
    pub(crate) fn is_animated(&self) -> Result<bool, PointwiseError<E>> {
        Ok(self
            .retained_animation()
            .as_ref()
            .map(GradientAnimation::is_active)
            .transpose()?
            .unwrap_or(false))
    }

    pub(crate) fn is_guaranteed_constant(&self) -> Result<bool, PointwiseError<E>> {
        self.is_animated().map(|active| !active)
    }
    /// Source lazy bh getter. Color uses its own 0..100 animation Limits;
    /// transparency retains its actual mutable numeric Limits root.
    pub fn animation(&self) -> Result<GradientAnimation<E>, LivePropertyWireError<E>> {
        self.ensure_animation().map_err(LivePropertyWireError::from)
    }
    pub(crate) fn ensure_animation(&self) -> Result<GradientAnimation<E>, PointwiseError<E>> {
        if let Some(animation) = self.retained_animation() {
            return Ok(animation);
        }
        let limits = match self.kind {
            SolidColorProperty::Color => {
                let decimal = |bits| DecimalBits::new(bits).map_err(|_| PointwiseError::Arithmetic);
                let increment = decimal([1, 0, 0, 1 << 16])?;
                let maximum = decimal([100, 0, 0, 0])?;
                SharedLimits::from_definition(LimitsDefinition {
                    decimal_places: 1,
                    step: LimitsStep::Derived(increment),
                    minimum: decimal([0; 4])?,
                    maximum,
                    ..LimitsDefinition::default()
                })
                .map_err(|_| PointwiseError::Arithmetic)?
            }
            SolidColorProperty::Transparency => self.limits(),
        };
        let animation = GradientAnimation::new(1, Some(limits))?;
        Ok(self
            .owner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .animations[index(self.kind)]
        .get_or_insert(animation)
        .clone())
    }
    /// os.AnimationProperty writes the replacement before checking undo eligibility.
    pub(crate) fn replace_animation(
        &self,
        animation: Option<GradientAnimation<E>>,
        has_history_target: bool,
    ) -> Result<Option<Arc<ContextChangeArgument<E>>>, LiveSolidColorError<E>> {
        let old = {
            let mut state = self
                .owner
                .state
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            std::mem::replace(&mut state.animations[index(self.kind)], animation.clone())
        };
        self.emit_edit(has_history_target, |name| {
            let undo = crate::AnimationUndoInfo::replacement(Some(name), old, animation);
            ContextPropertyChangeArgs::with_animation_undo(Arc::new(undo))
        })
    }
    pub fn adjustment(
        &self,
    ) -> Result<Option<SharedPropertyAdjustment<E>>, LivePropertyWireError<E>> {
        let kind = if self.kind == SolidColorProperty::Color {
            6
        } else {
            0
        };
        self.wire_metadata()
            .adjustment(kind, 1, Some(&self.limits()))
    }
    pub(crate) fn save_wire(&self, writer: &mut BinaryWriter) -> Result<(), NodeWireError<E>> {
        let snapshot = self.snapshot();
        save_code(writer, &snapshot.name)?;
        match self.kind {
            SolidColorProperty::Color => {
                // os writes each component using a fresh stored rr getter.
                for channel in 0..3 {
                    writer.write_f64(self.snapshot().color[channel])?;
                }
            }
            SolidColorProperty::Transparency => writer.write_f64(self.snapshot().current)?,
        }
        self.wire_metadata()
            .save_tail(writer, || self.animation().map(Some))?;
        Ok(())
    }
    pub(crate) fn load_wire(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), NodeWireError<E>> {
        check_code(reader, &self.snapshot().name, false)?;
        match self.kind {
            SolidColorProperty::Color => {
                let color = [reader.read_f64()?, reader.read_f64()?, reader.read_f64()?];
                self.set_color(color)
                    .map_err(|e| NodeWireError::Solid(PointwiseSolidEditError::Source(e)))?;
            }
            SolidColorProperty::Transparency => {
                let value = reader.read_f64()?;
                validate_wire_number(value, &self.limits())?;
                self.set_transparency(value)
                    .map_err(|e| NodeWireError::Solid(PointwiseSolidEditError::Source(e)))?;
            }
        }
        self.wire_metadata().load_tail(
            reader,
            version,
            || self.animation().map(Some),
            || {
                Ok(AdjustmentSpec {
                    property_type: if self.kind == SolidColorProperty::Color {
                        6
                    } else {
                        0
                    },
                    count: 1,
                    limits: Some(self.limits()),
                })
            },
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod laws {
    use super::*;
    use crate::{AnimationFlags, BinaryWriter, SolidColorCloneMode};

    #[test]
    fn animation_replacement_commits_with_controls_only_when_changes_are_disabled() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let node =
            SharedSolidColorNode::<()>::new_unattached(SolidColorCloneMode::Original).unwrap();
        let property = node.color_property();
        let original = property.ensure_animation().unwrap();
        let replacement = original.source_clone().unwrap();
        let expected = replacement.clone();
        property.set_allow_changes(false);
        let visited = Arc::new(AtomicBool::new(false));
        let seen = visited.clone();
        let observer = SolidColorPropertyHandler::new(move |sender, argument| {
            assert!(
                sender
                    .retained_animation()
                    .unwrap()
                    .same_reference(&expected)
            );
            let Some(ContextChangeArgument::Property(args)) = argument.as_deref() else {
                panic!("expected a property change");
            };
            assert!(args.update_controls_only());
            assert!(args.typed_undo_info().is_none());
            seen.store(true, Ordering::Release);
            Ok(())
        });
        property.add(&SolidColorPropertyDelegate::from_handlers(&[observer]));
        assert!(
            property
                .replace_animation(Some(replacement), true)
                .unwrap()
                .is_none()
        );
        assert!(visited.load(Ordering::Acquire));
    }

    #[test]
    fn failed_binding_write_does_not_eagerly_create_animation() {
        let node =
            SharedSolidColorNode::<()>::new_unattached(SolidColorCloneMode::Original).unwrap();
        let property = node.color_property();
        property.set_color([0.25, 0.5, 0.75]).unwrap();
        property.set_binding_index(2);
        property.set_binding_name(Some(&[65, 66]));
        // Code prefix + code + RGB precede the binding marker.
        let mut value_prefix = BinaryWriter::default();
        save_code::<()>(&mut value_prefix, &property.snapshot().name).unwrap();
        for v in property.snapshot().color {
            value_prefix.write_f64(v).unwrap();
        }
        let mut writer = BinaryWriter::default();
        writer.set_write_failure_at(Some(value_prefix.bytes().len()));
        let error = property.save_wire(&mut writer).unwrap_err();
        assert_eq!(error.source_class_with(|_| "observer"), Some("IOException"));
        assert_eq!(writer.bytes(), value_prefix.bytes());
        assert!(property.retained_animation().is_none());
        assert!(property.wire_metadata().retained_adjustment().is_none());
    }

    #[test]
    fn save_creates_real_animation_and_clone_and_capture_split_mutable_roots() {
        let node =
            SharedSolidColorNode::<()>::new_unattached(SolidColorCloneMode::Original).unwrap();
        let property = node.color_property();
        let mut writer = BinaryWriter::default();
        property.save_wire(&mut writer).unwrap();
        let animation = property.retained_animation().unwrap();
        assert!(!animation.is_active().unwrap());
        let cloned = node.clone_unadjusted_for_graph().unwrap().unwrap();
        let cloned_animation = cloned.color_property().retained_animation().unwrap();
        assert!(!animation.same_reference(&cloned_animation));
        assert!(
            !animation
                .flags()
                .unwrap()
                .same_reference(&cloned_animation.flags().unwrap())
        );
        assert!(
            animation
                .limits_for(0)
                .unwrap()
                .unwrap()
                .same_reference(&cloned_animation.limits_for(0).unwrap().unwrap())
        );
        let mut session = RenderCaptureSession::new();
        let mut inputs = crate::color_region::ColorInputCapture::default();
        let captured = node.capture_raw(&mut inputs, &mut session).unwrap();
        drop(session);
        let captured_animation = captured.color_property().retained_animation().unwrap();
        assert!(!animation.same_reference(&captured_animation));
        assert!(
            !animation
                .limits_for(0)
                .unwrap()
                .unwrap()
                .same_reference(&captured_animation.limits_for(0).unwrap().unwrap())
        );
        animation.set_flags(Some(AnimationFlags::from_values(vec![true])));
        assert!(!cloned_animation.is_active().unwrap());
        assert!(!captured_animation.is_active().unwrap());
        // Source kx.a6 uses raw stored getters even when k8 evaluation is animated.
        assert_eq!(node.constant_channel(0).unwrap(), 0.0);
        assert_eq!(node.constant_channel(4).unwrap(), 0.0);
        assert!(matches!(
            node.render(1, 1, 0.0),
            Err(LiveSolidColorError::Property(
                PointwiseError::Animation(error)
            )) if matches!(error.as_ref(), crate::AnimationError::NullReference)
        ));
        captured.render(1, 1, 0.0).unwrap();
    }

    #[test]
    fn color_commit_survives_later_binding_read_failure() {
        let producer =
            SharedSolidColorNode::<()>::new_unattached(SolidColorCloneMode::Original).unwrap();
        producer
            .color_property()
            .set_color([0.125, 0.25, 0.5])
            .unwrap();
        producer.color_property().set_binding_index(4);
        producer
            .color_property()
            .set_binding_name(Some(&[65, 66, 67]));
        let mut writer = BinaryWriter::default();
        producer.color_property().save_wire(&mut writer).unwrap();
        let mut prefix = BinaryWriter::default();
        save_code::<()>(&mut prefix, &producer.color_property().snapshot().name).unwrap();
        for v in producer.snapshot().color {
            prefix.write_f64(v).unwrap();
        }
        prefix.write_bool(true).unwrap();
        prefix.write_i16(4).unwrap();
        let target =
            SharedSolidColorNode::<()>::new_unattached(SolidColorCloneMode::Original).unwrap();
        target.color_property().set_binding_name(Some(&[90]));
        let mut reader = BinaryCursor::new(&writer.bytes()[..prefix.bytes().len()]).unwrap();
        assert!(
            target
                .color_property()
                .load_wire(&mut reader, 3.75)
                .is_err()
        );
        assert_eq!(target.snapshot().color, [0.125, 0.25, 0.5]);
        assert_eq!(target.color_property().binding_index(), 4);
        assert_eq!(target.color_property().binding_name(), Some(vec![90]));
        assert!(target.color_property().retained_animation().is_none());
    }
}
