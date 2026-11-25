//! Detached Imported owner capture. Source Clone and public setters are never
//! used to populate the frame; all intrinsic receivers point at captured bodies.
use super::*;
use crate::ordinary_events::DelegateCore;
use crate::pointwise::property::capture::capture_values::Comparison;

impl<E: 'static> GradientGenerator<E> {
    pub(in crate::pointwise::generator) fn capture_imported_with(
        &self,
        session: &mut crate::RenderCaptureSession<E>,
    ) -> Result<Self, PointwiseError<E>> {
        let (
            kind,
            factor,
            mut scalars,
            mut pairs,
            selection,
            animations,
            wire,
            allow_changes,
            current,
            previous,
            choices,
        ) = {
            let owner = self.state();
            let state = guard(&owner);
            let imported = state.imported.as_ref().ok_or(PointwiseError::InvalidCast)?;
            let choice = |v: &Selection| Selection {
                current: v.current,
                previous: v.previous,
                options: v.options.clone(),
            };
            (
                state.kind,
                state.factor,
                state.scalars.clone(),
                state.pairs.clone(),
                choice(&state.selection),
                state.animations.clone(),
                state.wire.clone(),
                state.allow_changes,
                imported.current.clone(),
                imported.previous.clone(),
                [choice(&imported.choices[0]), choice(&imported.choices[1])],
            )
        };
        for pair in &mut pairs {
            pair.limits = session.copy_limits(&pair.limits);
        }
        let selection = Selection {
            options: session.copy_options(&selection.options),
            ..selection
        };
        for scalar in scalars.iter_mut().flatten() {
            scalar.limits = scalar.limits.as_ref().map(|v| session.copy_limits(v));
        }
        let mut copied_animations = std::array::from_fn(|_| None);
        for (ix, animation) in animations.iter().enumerate() {
            copied_animations[ix] = animation
                .as_ref()
                .map(|v| session.copy_animation(v))
                .transpose()?;
        }
        let mut copied_wire = Vec::with_capacity(wire.len());
        for metadata in &wire {
            copied_wire.push(metadata.capture_with(|a| session.copy_adjustment(a))?);
        }
        let copied_wire = copied_wire
            .try_into()
            .map_err(|_| PointwiseError::InvalidCast)?;
        let current = current
            .as_ref()
            .map(|v| session.copy_custom_object(v))
            .transpose()?;
        let previous = previous
            .as_ref()
            .map(|v| session.copy_custom_object(v))
            .transpose()?;
        let choices = choices.map(|choice| Selection {
            options: session.copy_options(&choice.options),
            ..choice
        });
        let copy = Self {
            node: PointwiseNode::publish_image_generator(Arc::new(Mutex::new(GeneratorState {
                kind,
                factor,
                scalars,
                pairs,
                selection,
                animations: copied_animations,
                wire: copied_wire,
                allow_changes,
                handlers: std::array::from_fn(|_| DelegateCore::Absent),
                cache: None,
                render_caches: super::cached_render::GradientRenderCaches::new(),
                final_clone: false,
                final_cache: super::final_render::FinalImageCache::default(),
                imported: Some(ImportedState {
                    current,
                    previous,
                    binding: None,
                    choices,
                }),
            }))),
        };
        copy.bind_imported();
        Ok(copy)
    }
    pub(in crate::pointwise::generator) fn imported_capture_values(
        &self,
        comparison: &mut Comparison<E>,
    ) -> Result<Vec<u64>, PointwiseError<E>> {
        let (
            kind,
            factor,
            scalars,
            pairs,
            selection,
            animations,
            wire,
            allow,
            current,
            previous,
            choices,
        ) = {
            let owner = self.state();
            let state = guard(&owner);
            let imported = state.imported.as_ref().ok_or(PointwiseError::InvalidCast)?;
            let choice = |v: &Selection| Selection {
                current: v.current,
                previous: v.previous,
                options: v.options.clone(),
            };
            (
                state.kind,
                state.factor,
                state.scalars.clone(),
                state.pairs.clone(),
                choice(&state.selection),
                state.animations.clone(),
                state.wire.clone(),
                state.allow_changes,
                imported.current.clone(),
                imported.previous.clone(),
                [choice(&imported.choices[0]), choice(&imported.choices[1])],
            )
        };
        let mut out = vec![kind as u64, factor.to_bits(), pairs.len() as u64];
        for pair in pairs {
            out.extend(pair.current.map(f64::to_bits));
            out.extend(pair.previous.map(f64::to_bits));
            out.extend(pair.normalized.map(f64::to_bits));
            out.push(pair.scale.to_bits());
            comparison.limits(&pair.limits, &mut out);
        }
        for scalar in scalars {
            match scalar {
                None => out.push(0),
                Some(s) => {
                    out.extend([
                        1,
                        s.current.to_bits(),
                        s.previous.to_bits(),
                        s.modifier.to_bits(),
                        s.derived.to_bits(),
                    ]);
                    match s.limits {
                        None => out.push(0),
                        Some(l) => {
                            out.push(1);
                            comparison.limits(&l, &mut out);
                        }
                    }
                }
            }
        }
        for selection in std::iter::once(selection).chain(choices) {
            out.extend([selection.current as u64, selection.previous as u64]);
            comparison.options(&selection.options, &mut out);
        }
        out.extend(allow.map(u64::from));
        for animation in animations {
            comparison.animation(animation, &mut out)?;
        }
        for metadata in &wire {
            comparison.metadata(metadata, &mut out)?;
        }
        comparison.object(&current, &mut out)?;
        comparison.object(&previous, &mut out)?;
        Ok(out)
    }
    pub(in crate::pointwise::generator) fn capture_imported_frame(
        &self,
        time: f64,
    ) -> Result<GradientRenderSnapshot, PointwiseError<E>> {
        let mut comparison = vec![time.to_bits()];
        comparison.extend(self.raw_capture_values(&mut Comparison::default())?);
        let frame = ImportedFrame {
            renderer: Arc::new(OwnedRenderer {
                owner: self.clone(),
                time,
            }),
            comparison: comparison.into(),
        };
        Ok(GradientRenderSnapshot {
            imported: Some(frame),
            selection: None,
            family: None,
            mode: None,
            size: [0.0; 2],
            location: [0.0; 2],
        })
    }
}
trait RenderImported: Send + Sync {
    fn render(
        &self,
        width: i32,
        height: i32,
        execution: ProceduralExecution,
    ) -> Result<SharedImageMap, PointwiseError<()>>;
}
struct OwnedRenderer<E> {
    owner: GradientGenerator<E>,
    time: f64,
}
impl<E: 'static> RenderImported for OwnedRenderer<E> {
    fn render(
        &self,
        width: i32,
        height: i32,
        execution: ProceduralExecution,
    ) -> Result<SharedImageMap, PointwiseError<()>> {
        self.owner
            .render_imported(
                width,
                height,
                self.time,
                PointwiseRenderOptions::new(false, false),
                execution,
            )
            .map_err(|error| convert_error(&error))
    }
}
/// Type erasure retains the actual detached owner; no live source, decoded
/// ImageMap-valued source or pre-evaluated fallible property result is stored.
#[derive(Clone)]
pub(in crate::pointwise::generator) struct ImportedFrame {
    renderer: Arc<dyn RenderImported>,
    comparison: Arc<[u64]>,
}
impl fmt::Debug for ImportedFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportedFrame")
            .field("operands", &self.comparison.len())
            .finish_non_exhaustive()
    }
}
impl ImportedFrame {
    pub(in crate::pointwise::generator) fn same_render_values(&self, other: &Self) -> bool {
        self.comparison == other.comparison
    }
    pub(in crate::pointwise::generator) fn render<E>(
        &self,
        width: i32,
        height: i32,
        execution: ProceduralExecution,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        self.renderer
            .render(width, height, execution)
            .map_err(|error| convert_error(&error))
    }
}
// No host callback survives capture. Reconstruct every actual non-observer
// error cause across the nongeneric snapshot seam; an impossible observer arm
// is refused explicitly rather than assigned a source class or successful value.
fn curve_error<A, B>(error: &crate::CurveError<A>) -> Option<crate::CurveError<B>> {
    use crate::CurveError as C;
    Some(match error {
        C::NullReference => C::NullReference,
        C::Index => C::Index,
        C::ArgumentIndex => C::ArgumentIndex,
        C::InvalidBlend => C::InvalidBlend,
        C::WrongControl => C::WrongControl,
        C::Overflow => C::Overflow,
        C::Allocation => C::Allocation,
        C::Binary(v) => C::Binary(*v),
        C::InvalidData => C::InvalidData,
        C::UnsupportedCapability => C::UnsupportedCapability,
        C::Observer(_) => return None,
    })
}
fn animation_error<A, B>(error: &crate::AnimationError<A>) -> Option<crate::AnimationError<B>> {
    use crate::AnimationError as AErr;
    Some(match error {
        AErr::NullReference => AErr::NullReference,
        AErr::Index => AErr::Index,
        AErr::Overflow => AErr::Overflow,
        AErr::Allocation => AErr::Allocation,
        AErr::Binary(v) => AErr::Binary(*v),
        AErr::Gradient(v) => AErr::Gradient(*v),
        AErr::Curve(v) => AErr::Curve(curve_error(v)?),
        AErr::Bounds(v) => AErr::Bounds(*v),
    })
}
fn history_error<A, B>(error: &crate::HistoryError<A>) -> Option<crate::HistoryError<B>> {
    use crate::HistoryError as H;
    Some(match error {
        H::MissingDependency(v) => H::MissingDependency(*v),
        H::Clock { site, reason } => H::Clock { site, reason },
        H::NullUndo => H::NullUndo,
        H::NullTarget => H::NullTarget,
        H::DetachedCommand => H::DetachedCommand,
        H::InputChanged => H::InputChanged,
        H::BranchChanged => H::BranchChanged,
        H::HistoryRetired => H::HistoryRetired,
        H::NullDocument => H::NullDocument,
        H::NullContributorList => H::NullContributorList,
        H::ContributorIndex => H::ContributorIndex,
        H::ContributorCollectionModified => H::ContributorCollectionModified,
        H::ListIndex {
            operation,
            index,
            length,
        } => H::ListIndex {
            operation,
            index: *index,
            length: *length,
        },
        H::Index {
            operation,
            index,
            length,
        } => H::Index {
            operation,
            index: *index,
            length: *length,
        },
        H::CrossHistory => H::CrossHistory,
        H::Property(v) => H::Property(Box::new(convert_error(v))),
        H::Service(_) => return None,
    })
}
fn convert_error<A, B>(error: &PointwiseError<A>) -> PointwiseError<B> {
    let observer = || {
        PointwiseError::UnsupportedCapability(
            "detached Imported frame unexpectedly reached an observer",
        )
    };
    match error {
        PointwiseError::Image(v) => PointwiseError::Image(*v),
        PointwiseError::Scene(v) => PointwiseError::Scene(*v),
        PointwiseError::ModernProperty(v) => PointwiseError::ModernProperty(*v),
        PointwiseError::PropertyNotification(v) => PointwiseError::PropertyNotification(*v),
        PointwiseError::DecimalRounding(v) => PointwiseError::DecimalRounding(*v),
        PointwiseError::ImportedImage(v) => PointwiseError::ImportedImage(*v),
        PointwiseError::Gradient(v) => PointwiseError::Gradient(*v),
        PointwiseError::Curve(v) => curve_error(v.as_ref())
            .map(|v| PointwiseError::Curve(Arc::new(v)))
            .unwrap_or_else(observer),
        PointwiseError::Animation(v) => animation_error(v.as_ref())
            .map(|v| PointwiseError::Animation(Arc::new(v)))
            .unwrap_or_else(observer),
        PointwiseError::Selection(v) => match v {
            crate::SelectionError::Index => PointwiseError::Selection(crate::SelectionError::Index),
            crate::SelectionError::NullReference => {
                PointwiseError::Selection(crate::SelectionError::NullReference)
            }
            crate::SelectionError::InvalidCast => {
                PointwiseError::Selection(crate::SelectionError::InvalidCast)
            }
            crate::SelectionError::UnrecognizedText => {
                PointwiseError::Selection(crate::SelectionError::UnrecognizedText)
            }
            crate::SelectionError::Observer(_) => observer(),
        },
        PointwiseError::Interpolation(v) => PointwiseError::Interpolation(*v),
        PointwiseError::Index => PointwiseError::Index,
        PointwiseError::ArgumentIndex => PointwiseError::ArgumentIndex,
        PointwiseError::NullReference => PointwiseError::NullReference,
        PointwiseError::Arithmetic => PointwiseError::Arithmetic,
        PointwiseError::DivideByZero => PointwiseError::DivideByZero,
        PointwiseError::Argument => PointwiseError::Argument,
        PointwiseError::ArgumentNull => PointwiseError::ArgumentNull,
        PointwiseError::InvalidOperation => PointwiseError::InvalidOperation,
        PointwiseError::InvalidCast => PointwiseError::InvalidCast,
        PointwiseError::Exception => PointwiseError::Exception,
        PointwiseError::Observer(_) => observer(),
        PointwiseError::FinalRender(error) => {
            PointwiseError::FinalRender(Box::new(convert_error(error)))
        }
        PointwiseError::History(v) => history_error(v)
            .map(|v| PointwiseError::History(Box::new(v)))
            .unwrap_or_else(observer),
        PointwiseError::CapacityQuery(v) => PointwiseError::CapacityQuery(v.clone()),
        PointwiseError::UnsupportedCapability(v) => PointwiseError::UnsupportedCapability(v),
        PointwiseError::Cancelled => PointwiseError::Cancelled,
        PointwiseError::TemplateResource(path) => PointwiseError::TemplateResource(path.clone()),
        PointwiseError::Shader(message) => PointwiseError::Shader(message.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ByteBuffer, ImageGeneratorParameterKind as P, RenderCaptureSession};
    #[test]
    fn history_error_projection_keeps_pure_nested_causes_and_refuses_foreign_service_payload() {
        use crate::HistoryError as H;
        let error: PointwiseError<()> = PointwiseError::History(Box::new(H::Property(Box::new(
            PointwiseError::History(Box::new(H::NullTarget)),
        ))));
        let converted: PointwiseError<String> = convert_error(&error);
        assert!(
            matches!(converted, PointwiseError::History(v) if matches!(*v, H::Property(ref p) if matches!(**p, PointwiseError::History(ref h) if matches!(**h, H::NullTarget))))
        );
        let error = PointwiseError::History(Box::new(H::Service(42)));
        let converted: PointwiseError<()> = convert_error(&error);
        assert!(matches!(
            converted,
            PointwiseError::UnsupportedCapability(_)
        ));
    }
    fn bmp() -> Vec<u8> {
        let mut v = vec![0; 62];
        v[0..2].copy_from_slice(b"BM");
        v[2..6].copy_from_slice(&62u32.to_le_bytes());
        v[10..14].copy_from_slice(&54u32.to_le_bytes());
        v[14..18].copy_from_slice(&40u32.to_le_bytes());
        v[18..22].copy_from_slice(&2i32.to_le_bytes());
        v[22..26].copy_from_slice(&1i32.to_le_bytes());
        v[26..28].copy_from_slice(&1u16.to_le_bytes());
        v[28..30].copy_from_slice(&24u16.to_le_bytes());
        v[34..38].copy_from_slice(&8u32.to_le_bytes());
        v[54..60].copy_from_slice(&[30, 20, 10, 60, 50, 40]);
        v
    }
    fn payload(node: &ImportedImageNode<()>) -> PointwiseImportedImage<()> {
        node.parameter(P::Image)
            .custom_value()
            .unwrap()
            .unwrap()
            .imported_image()
            .unwrap()
    }
    #[test]
    fn captured_receivers_publish_only_into_captured_bodies_even_after_live_drop() {
        let live = ImportedImageNode::<()>::new().unwrap();
        let original = payload(&live);
        let bytes = ByteBuffer::new(bmp());
        original.set_bytes(Some(bytes.clone())).unwrap();
        let shared = original.source_clone();
        live.parameter(P::Image)
            .set_custom_value(Some(&shared.custom_object()), false)
            .unwrap();
        // Actual null setter retains its old receiver entry even though that
        // receiver no longer has a current container of its own.
        original.set_bytes(None).unwrap();
        let mut session = RenderCaptureSession::new();
        let copied = live.generator.capture_imported_with(&mut session).unwrap();
        let captured = copied
            .parameter(P::Image)
            .custom_value()
            .unwrap()
            .unwrap()
            .imported_image()
            .unwrap();
        let container = captured.container().unwrap();
        let receivers = container.dimension_listeners();
        assert_eq!(receivers.len(), 1);
        let receiver = receivers[0].imported_image().unwrap();
        assert!(!receiver.same_reference(&original));
        assert!(receiver.container().is_none());
        assert!(!captured.bytes().unwrap().unwrap().same_reference(&bytes));
        assert_eq!(original.cached_dimensions(), [0, 0]);
        assert_eq!(receiver.cached_dimensions(), [0, 0]);
        bytes.write(54, 200).unwrap();
        drop(session);
        drop(live);
        drop(shared);
        drop(original);
        drop(bytes);
        let image = captured.image().unwrap().unwrap();
        assert_eq!(image.geometry().width(), 2);
        assert_eq!(receiver.cached_dimensions(), [2, 1]);
        assert!(receiver.has_valid_dimensions());
        assert_eq!(captured.cached_dimensions(), [0, 0]);
        assert!(captured.has_valid_dimensions());
        assert_eq!(captured.bytes().unwrap().unwrap().read(54).unwrap(), 30);
    }
    #[test]
    fn shared_container_and_distinct_store_same_array_relations_survive_one_session() {
        let a = PointwiseImportedImage::<()>::new();
        let bytes = ByteBuffer::new(bmp());
        a.set_bytes(Some(bytes.clone())).unwrap();
        let b = a.source_clone();
        let c = PointwiseImportedImage::<()>::new();
        c.set_bytes(Some(bytes.clone())).unwrap();
        let mut session = RenderCaptureSession::new();
        let ca = session
            .copy_custom_object(&a.custom_object())
            .unwrap()
            .imported_image()
            .unwrap();
        let cb = session
            .copy_custom_object(&b.custom_object())
            .unwrap()
            .imported_image()
            .unwrap();
        let cc = session
            .copy_custom_object(&c.custom_object())
            .unwrap()
            .imported_image()
            .unwrap();
        assert!(
            ca.container()
                .unwrap()
                .same_reference(&cb.container().unwrap())
        );
        assert!(
            !ca.container()
                .unwrap()
                .same_reference(&cc.container().unwrap())
        );
        assert!(
            !ca.container()
                .unwrap()
                .store()
                .unwrap()
                .same_reference(&cc.container().unwrap().store().unwrap())
        );
        assert!(
            ca.bytes()
                .unwrap()
                .unwrap()
                .same_reference(&cc.bytes().unwrap().unwrap())
        );
        assert!(!ca.bytes().unwrap().unwrap().same_reference(&bytes));
        let old = ca.bytes().unwrap().unwrap().read(54).unwrap();
        bytes.write(54, 99).unwrap();
        assert_eq!(ca.bytes().unwrap().unwrap().read(54).unwrap(), old);
        a.container().unwrap().store().unwrap().dispose();
        assert!(!ca.container().unwrap().store().unwrap().is_disposed());
    }
    #[test]
    fn frame_comparison_tracks_silent_warm_bytes_and_cache_cells_without_callbacks() {
        let live = ImportedImageNode::<()>::new().unwrap();
        let image = payload(&live);
        let bytes = ByteBuffer::new(bmp());
        image.set_bytes(Some(bytes.clone())).unwrap();
        let map = image.image().unwrap().unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        image.add_handler(crate::PointwiseMatrixHandler::new(move |_, _| {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
        let first = RenderCaptureSession::new()
            .capture_generator(&live.generator, 0.)
            .unwrap();
        let same = RenderCaptureSession::new()
            .capture_generator(&live.generator, 0.)
            .unwrap();
        assert!(first.same_render_values(&same));
        bytes.write(54, 199).unwrap();
        let changed = RenderCaptureSession::new()
            .capture_generator(&live.generator, 0.)
            .unwrap();
        assert!(!first.same_render_values(&changed));
        let values = map.channel(0, true).unwrap();
        values.write(0, 0.875).unwrap();
        let next = RenderCaptureSession::new()
            .capture_generator(&live.generator, 0.)
            .unwrap();
        assert!(!changed.same_render_values(&next));
        let before = first
            .render(2, 1)
            .unwrap()
            .channel(0, true)
            .unwrap()
            .values();
        drop(live);
        drop(image);
        drop(bytes);
        drop(map);
        assert_eq!(
            first
                .render(2, 1)
                .unwrap()
                .channel(0, true)
                .unwrap()
                .values(),
            before
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    #[test]
    fn disposed_encoded_owner_and_wrong_current_kind_are_captured_without_getter_or_repair() {
        let image = PointwiseImportedImage::<()>::new();
        let bytes = ByteBuffer::new(bmp());
        image.set_bytes(Some(bytes.clone())).unwrap();
        image.container().unwrap().store().unwrap().dispose();
        let mut session = RenderCaptureSession::new();
        let copy = session
            .copy_custom_object(&image.custom_object())
            .unwrap()
            .imported_image()
            .unwrap();
        assert!(copy.container().unwrap().store().unwrap().is_disposed());
        assert!(matches!(
            copy.bytes(),
            Err(crate::ImportedImageError::ObjectDisposed)
        ));
        assert!(
            copy.container()
                .unwrap()
                .store()
                .unwrap()
                .raw_snapshot()
                .unwrap()
                .resident
                .unwrap()
                .values()
                == bytes.values()
        );
        let live = ImportedImageNode::<()>::new().unwrap();
        let wrong = crate::PointwiseMatrix::<()>::new().unwrap().custom_object();
        assert!(matches!(
            live.parameter(P::Image)
                .set_custom_value(Some(&wrong), false),
            Ok(())
        ));
        let frame = RenderCaptureSession::new()
            .capture_generator(&live.generator, 0.)
            .unwrap();
        assert!(matches!(
            frame.render(2, 1),
            Err(PointwiseError::InvalidCast)
        ));
    }
}

#[cfg(test)]
mod scene_error_projection_laws {
    use super::*;
    #[test]
    fn detached_projection_preserves_new_scene_property_causes() {
        assert!(matches!(
            convert_error::<(), u32>(&PointwiseError::Scene(crate::SceneError::NullReference)),
            PointwiseError::Scene(crate::SceneError::NullReference)
        ));
        assert!(matches!(
            convert_error::<(), u32>(&PointwiseError::ModernProperty(
                crate::ModernPropertyError::AnimationUnavailable
            )),
            PointwiseError::ModernProperty(crate::ModernPropertyError::AnimationUnavailable)
        ));
        assert!(matches!(
            convert_error::<(), u32>(&PointwiseError::PropertyNotification(
                crate::ChangeNotificationFailure::EmptyInvocation
            )),
            PointwiseError::PropertyNotification(crate::ChangeNotificationFailure::EmptyInvocation)
        ));
        assert!(matches!(
            convert_error::<(), u32>(&PointwiseError::DecimalRounding(
                crate::DecimalRoundingError::Precision
            )),
            PointwiseError::DecimalRounding(crate::DecimalRoundingError::Precision)
        ));
    }
}
