//! Memory-only freeze boundary over the existing raw copier. Pending owners are
//! private, detached and cannot render. All original-reference memo roots die
//! when finish consumes the session, before a worker receives the batch.
use super::*;
use crate::EnvironmentRecord;
use crate::pointwise::canvas::{CanvasDocument, CanvasStyle};
use std::sync::atomic::{AtomicBool, Ordering};

/// A working map substituted for specific owners in one detached render batch.
pub struct EnvironmentRenderOverride<E> {
    owner: EnvironmentOwner<E>,
    map: Arc<EnvironmentRecord>,
}

enum EnvironmentOwner<E> {
    Properties {
        parameters: Vec<PointwiseParameter<E>>,
        adjuster: Option<([u8; 16], Vec<u16>)>,
    },
    Style {
        node: crate::PointwiseNode<E>,
        canvas_layer: Option<usize>,
        pass: usize,
    },
}

impl<E: 'static> EnvironmentRenderOverride<E> {
    pub fn for_properties(
        properties: Vec<PointwiseParameter<E>>,
        map: Arc<EnvironmentRecord>,
    ) -> Self {
        Self {
            owner: EnvironmentOwner::Properties {
                parameters: properties,
                adjuster: None,
            },
            map,
        }
    }

    pub(crate) fn for_adjuster(
        parameters: Vec<PointwiseParameter<E>>,
        group_id: [u8; 16],
        name: Vec<u16>,
        map: Arc<EnvironmentRecord>,
    ) -> Self {
        Self {
            owner: EnvironmentOwner::Properties {
                parameters,
                adjuster: Some((group_id, name)),
            },
            map,
        }
    }

    pub fn for_style(
        node: crate::PointwiseNode<E>,
        canvas_layer: Option<&crate::CanvasLayerEditor<E>>,
        pass: usize,
        map: Arc<EnvironmentRecord>,
    ) -> Self {
        Self {
            owner: EnvironmentOwner::Style {
                node,
                canvas_layer: canvas_layer.map(crate::CanvasLayerEditor::identity),
                pass,
            },
            map,
        }
    }

    pub fn map(&self) -> &Arc<EnvironmentRecord> {
        &self.map
    }

    pub(super) fn matches_parameter(&self, filter: &PointwiseFilter<E>, index: usize) -> bool {
        match &self.owner {
            EnvironmentOwner::Properties { parameters, .. } => parameters
                .iter()
                .any(|property| property.ix == index && property.owner.same_reference(filter)),
            EnvironmentOwner::Style { .. } => false,
        }
    }

    pub(super) fn replace_style(
        &self,
        filter: &PointwiseFilter<E>,
        canvas: &mut Option<Arc<CanvasDocument>>,
        style: &mut Option<Arc<CanvasStyle>>,
    ) -> Result<(), PointwiseError<E>> {
        if let EnvironmentOwner::Properties {
            adjuster: Some((group, name)),
            ..
        } = &self.owner
        {
            let mut map = |_: &Arc<EnvironmentRecord>, key: Option<([u8; 16], &[u16])>| {
                Ok::<_, PointwiseError<E>>(
                    (key == Some((*group, name.as_slice()))).then(|| self.map.clone()),
                )
            };
            if let Some(current) = canvas.as_ref()
                && let Some(replacement) = current.map_environments(&mut map)?
            {
                *canvas = Some(replacement);
            }
            if let Some(current) = style.as_ref()
                && let Some(replacement) = current.map_environments(&mut map)?
            {
                *style = Some(replacement);
            }
            return Ok(());
        }
        let EnvironmentOwner::Style {
            node,
            canvas_layer,
            pass,
        } = &self.owner
        else {
            return Ok(());
        };
        if !node.same_reference(&filter.node()) {
            return Ok(());
        }
        if let Some(identity) = *canvas_layer {
            let current = canvas.as_ref().ok_or(PointwiseError::InvalidOperation)?;
            *canvas = Some(
                current
                    .preview_environment(identity, *pass, &self.map)
                    .ok_or(PointwiseError::InvalidOperation)?,
            );
        } else {
            let current = style.as_ref().ok_or(PointwiseError::InvalidOperation)?;
            *style = Some(
                current
                    .preview_environment(*pass, &self.map)
                    .ok_or(PointwiseError::InvalidOperation)?,
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenRenderValues(Arc<[u64]>);
pub struct RenderFreezeSession<E = ()> {
    inner: RenderCaptureSession<E>,
    ready: Arc<AtomicBool>,
}
pub struct FrozenRenderBatch {
    stores: Vec<crate::EncodedByteStore>,
    ready: Arc<AtomicBool>,
}
pub struct MaterializedRenderBatch<E = ()> {
    inner: RenderCaptureSession<E>,
    ready: Arc<AtomicBool>,
}
pub struct PendingNodeRender<E = ()> {
    node: super::super::super::PointwiseNode<E>,
    origins: CapturedNodeIdentities,
    time: f64,
    values: FrozenRenderValues,
    ready: Arc<AtomicBool>,
}
pub struct PendingThumbnailRender<E = ()> {
    node: crate::PointwiseNode<E>,
    time: f64,
    origins: CapturedNodeIdentities,
    ready: Arc<AtomicBool>,
}
pub struct PendingFilterRender<E = ()> {
    node: PendingNodeRender<E>,
}
pub struct PendingGradientRender<E = ()> {
    model: GradientGenerator<E>,
    time: f64,
    values: FrozenRenderValues,
    ready: Arc<AtomicBool>,
}
impl<E: 'static> Default for RenderFreezeSession<E> {
    fn default() -> Self {
        Self::new()
    }
}
impl<E: 'static> RenderFreezeSession<E> {
    pub fn new() -> Self {
        let mut inner = RenderCaptureSession::new();
        inner.deferred_files = true;
        inner.comparison.memory_only = true;
        Self {
            inner,
            ready: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn with_environment_override(edit: Arc<EnvironmentRenderOverride<E>>) -> Self {
        let mut session = Self::new();
        session.inner.environment_override = Some(edit);
        session
    }

    pub(crate) fn with_draft_override(edit: DraftRenderOverrides) -> Self {
        let mut session = Self::new();
        session.inner.draft_override = Some(edit);
        session
    }

    /// Identify a draft receiver or a consumer that would display its value.
    pub fn affects_draft_preview(&self, node: &crate::PointwiseNode<E>, time: f64) -> bool {
        self.inner
            .draft_override
            .as_ref()
            .is_some_and(|edit| edit.affects(node, Some(time)))
    }

    pub fn affects_lab_color_preview(&self, node: &crate::PointwiseNode<E>, time: f64) -> bool {
        self.affects_draft_preview(node, time)
    }

    pub fn freeze_node(
        &mut self,
        node: &super::super::super::PointwiseNode<E>,
        time: f64,
    ) -> Result<PendingNodeRender<E>, PointwiseError<E>> {
        self.inner.ensure_usable()?;
        let result = (|| {
            let node = self.inner.copy_node_raw(node)?;
            let values = FrozenRenderValues(self.inner.compare_node(&node, time)?);
            Ok(PendingNodeRender {
                node,
                origins: self.inner.captured_node_identities(),
                time,
                values,
                ready: self.ready.clone(),
            })
        })();
        self.inner.finish_capture(result)
    }
    /// Detach canvas geometry and properties without a comparison fingerprint.
    /// The document's revision owns cancellation and replacement of these frames.
    pub fn freeze_thumbnail(
        &mut self,
        node: &crate::PointwiseNode<E>,
        time: f64,
    ) -> Result<PendingThumbnailRender<E>, PointwiseError<E>> {
        self.inner.ensure_usable()?;
        let result = self
            .inner
            .copy_node_raw(node)
            .map(|node| PendingThumbnailRender {
                node,
                time,
                origins: self.inner.captured_node_identities(),
                ready: self.ready.clone(),
            });
        self.inner.finish_capture(result)
    }

    pub fn freeze_filter_at_time(
        &mut self,
        value: &PointwiseFilter<E>,
        time: f64,
    ) -> Result<PendingFilterRender<E>, PointwiseError<E>> {
        self.freeze_node(&value.node(), time)
            .map(|node| PendingFilterRender { node })
    }
    pub fn freeze_gradient(
        &mut self,
        model: &GradientGenerator<E>,
        time: f64,
    ) -> Result<PendingGradientRender<E>, PointwiseError<E>> {
        self.inner.ensure_usable()?;
        let result = (|| {
            let model = model.copy_raw_with(&mut self.inner)?;
            let values = FrozenRenderValues(self.inner.compare_node(&model.node(), time)?);
            Ok(PendingGradientRender {
                model,
                time,
                values,
                ready: self.ready.clone(),
            })
        })();
        self.inner.finish_capture(result)
    }
    pub fn finish(self) -> Result<FrozenRenderBatch, PointwiseError<E>> {
        self.inner.ensure_usable()?;
        let stores = self
            .inner
            .encoded_stores
            .iter()
            .map(|(_, copy)| copy.clone())
            .collect();
        // self (including all original-reference memos) is dropped here.
        Ok(FrozenRenderBatch {
            stores,
            ready: self.ready.clone(),
        })
    }
}
impl FrozenRenderBatch {
    /// Run off the model/UI thread. On error the entire batch is retired;
    /// existing independent completed frames remain usable.
    pub fn materialize<E: 'static>(self) -> Result<MaterializedRenderBatch<E>, PointwiseError<E>> {
        for store in &self.stores {
            store.materialize_frozen().map_err(|e| {
                PointwiseError::ImportedImage(crate::ImportedImageDecodeError::Storage(e))
            })?;
        }
        self.ready.store(true, Ordering::Release);
        Ok(MaterializedRenderBatch {
            inner: RenderCaptureSession::new(),
            ready: self.ready.clone(),
        })
    }
}
impl<E: 'static> PendingNodeRender<E> {
    pub fn frozen_values(&self) -> &FrozenRenderValues {
        &self.values
    }
    pub fn complete(
        self,
        batch: &mut MaterializedRenderBatch<E>,
    ) -> Result<PointwiseNodeRenderSnapshot<E>, PointwiseError<E>> {
        batch.check(&self.ready)?;
        let result = batch
            .inner
            .compare_node(&self.node, self.time)
            .map(|comparison| PointwiseNodeRenderSnapshot {
                node: self.node,
                origins: self.origins,
                time: self.time,
                comparison,
            });
        batch.inner.finish_capture(result)
    }
}
impl<E: 'static> PendingThumbnailRender<E> {
    pub fn complete(
        self,
        batch: &MaterializedRenderBatch<E>,
    ) -> Result<PointwiseThumbnailRender<E>, PointwiseError<E>> {
        batch.check(&self.ready)?;
        PointwiseNodeRenderSnapshot {
            node: self.node,
            origins: self.origins,
            time: self.time,
            comparison: Arc::from([]),
        }
        .prepare_thumbnail()
    }
}

impl<E: 'static> PendingFilterRender<E> {
    pub fn frozen_values(&self) -> &FrozenRenderValues {
        self.node.frozen_values()
    }
    pub fn complete(
        self,
        batch: &mut MaterializedRenderBatch<E>,
    ) -> Result<PointwiseRenderSnapshot<E>, PointwiseError<E>> {
        batch.check(&self.node.ready)?;
        let result = (|| {
            let model = self.node.node.filter().ok_or(PointwiseError::InvalidCast)?;
            let comparison = batch.inner.compare_node(&self.node.node, self.node.time)?;
            Ok(PointwiseRenderSnapshot {
                model,
                time: self.node.time,
                comparison,
            })
        })();
        batch.inner.finish_capture(result)
    }
}
impl<E: 'static> PendingGradientRender<E> {
    pub fn frozen_values(&self) -> &FrozenRenderValues {
        &self.values
    }
    pub fn complete(
        self,
        batch: &mut MaterializedRenderBatch<E>,
    ) -> Result<GradientRenderSnapshot, PointwiseError<E>> {
        batch.check(&self.ready)?;
        let result = self.model.complete_captured_operands(self.time);
        batch.inner.finish_capture(result)
    }
}

impl<E: 'static> MaterializedRenderBatch<E> {
    fn check(&self, ready: &Arc<AtomicBool>) -> Result<(), PointwiseError<E>> {
        self.inner.ensure_usable()?;
        if !Arc::ptr_eq(&self.ready, ready) || !ready.load(Ordering::Acquire) {
            Err(PointwiseError::UnsupportedCapability(
                "pending frame belongs to another or unfinished materialization batch",
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod laws {
    use super::*;
    use crate::{ByteBuffer, ImageGeneratorKind, ImageGeneratorParameterKind as P, PointwiseNode};

    #[test]
    fn synthesis_draft_reference_follows_the_sample_and_never_reuses_committed_pixels() {
        use crate::{
            AnalyticCurveKind, CurveLayerHandle, ImageGeometry, PointwiseRenderEffects,
            PointwiseRenderOptions, ProceduralExecution, SynthesisColorTarget, SynthesisLayerKind,
        };

        let first = PointwiseNode::<()>::solid_color().unwrap();
        first.set_solid_color([0.2; 3]).unwrap();
        let filter = PointwiseFilter::new(PointwiseKind::Synthesis).unwrap();
        let texture = filter.node();
        let color = SynthesisColorTarget::Solid(0);
        let mut before = filter.synthesis_edit().unwrap();
        before.add_layer(SynthesisLayerKind::Solid);
        assert!(before.set_color_value(color, [0.4; 3]));
        filter.edit_synthesis(before, "Texture").unwrap();
        let mut draft = filter.synthesis_edit().unwrap();
        assert!(draft.set_color_value(color, [0.35; 3]));

        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        group.parameter(0).unwrap().set_number(2.).unwrap();
        group
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&first))
            .unwrap();
        group
            .node()
            .slot(1)
            .unwrap()
            .set_node(Some(&texture))
            .unwrap();
        let reference = PointwiseFilter::new(PointwiseKind::Reference).unwrap();
        let node = reference.node();
        let other = PointwiseFilter::new(PointwiseKind::Reference).unwrap();
        other.parameter(0).unwrap().set_number(2.).unwrap();
        let sum = PointwiseFilter::new(PointwiseKind::Add).unwrap().node();
        for (index, input) in [&node, &other.node()].into_iter().enumerate() {
            sum.slot(index as i32)
                .unwrap()
                .set_node(Some(input))
                .unwrap();
        }
        let output = group.node();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&sum))
            .unwrap();
        let selector = reference.parameter(0).unwrap();
        selector.set_number(1.).unwrap();
        selector.set_axis_animated(0, true).unwrap();
        let curve = selector.animation_curve(0).unwrap();
        curve.set_minimum(1.);
        curve.set_maximum(2.);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        assert!(
            node.reference_target_at(0.)
                .unwrap()
                .unwrap()
                .same_reference(&first)
        );
        assert!(
            node.reference_target_at(1.)
                .unwrap()
                .unwrap()
                .same_reference(&texture)
        );

        let geometry = ImageGeometry::new(16, 16);
        let cached = first.render_original(16, 16).unwrap().image().unwrap();
        first.set_cached_image(Some(cached.clone())).unwrap();
        node.set_cached_image(Some(cached)).unwrap();
        let ordinary = RenderFreezeSession::new().freeze_node(&node, 0.).unwrap();
        assert!(ordinary.node.cached_image().is_some());
        let mut overrides = DraftRenderOverrides::default();
        overrides
            .synthesis
            .insert(texture.occurrence_id(), draft.record());
        let mut session = RenderFreezeSession::with_draft_override(overrides);
        assert!(!session.affects_draft_preview(&node, 0.));
        assert!(session.affects_draft_preview(&node, 1.));
        let direct = session.freeze_node(&node, 1.).unwrap();
        assert!(direct.node.cached_image().is_none());
        let early = session.freeze_node(&output, 0.).unwrap();
        let late = session.freeze_node(&output, 1.).unwrap();
        let mut batch = session.finish().unwrap().materialize().unwrap();
        // The Group uses its sampled, prepared exterior result. Rendering
        // the selected Reference alone still falls back to its saved slot.
        for (sample, expected) in [(early, 0.55), (late, 0.7), (direct, 0.2)] {
            let frame = sample.complete(&mut batch).unwrap();
            let mut capacity = |_| Ok(true);
            let mut preflight = |_, _| Ok(());
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            let rendered = frame
                .render_export_with_effects(
                    geometry,
                    ProceduralExecution::CallingThread,
                    PointwiseRenderOptions::new(false, false),
                    &mut effects,
                )
                .unwrap();
            assert!(rendered.caught_errors().is_empty());
            let value = rendered
                .image()
                .unwrap()
                .channel(0, true)
                .unwrap()
                .read(0)
                .unwrap();
            assert!((value - expected).abs() < 1e-6, "{value} != {expected}");
        }
        assert!(node.cached_image().is_some());
        assert_eq!(
            filter.synthesis_edit().unwrap().color_value(color),
            Some([0.4; 3])
        );
    }

    #[test]
    fn environment_preview_uses_property_identity_even_when_maps_are_shared() {
        let make = || PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        let edited = make();
        let unrelated = make();
        let map = Arc::new(EnvironmentRecord::default());
        for filter in [&edited, &unrelated] {
            filter
                .parameter(0)
                .unwrap()
                .set_environment(map.clone())
                .unwrap();
        }
        let mut draft = EnvironmentRecord::default();
        draft
            .set_field(crate::EnvironmentField::Exposure, 1.5)
            .unwrap();
        let draft = Arc::new(draft);
        let edit = EnvironmentRenderOverride::for_properties(
            vec![edited.parameter(0).unwrap()],
            draft.clone(),
        );
        let mut session = RenderFreezeSession::with_environment_override(Arc::new(edit));
        let edited_frame = session.freeze_node(&edited.node(), 0.).unwrap();
        let unrelated_frame = session.freeze_node(&unrelated.node(), 0.).unwrap();
        let mut batch = session.finish().unwrap().materialize().unwrap();
        let edited_frame = edited_frame.complete(&mut batch).unwrap();
        let unrelated_frame = unrelated_frame.complete(&mut batch).unwrap();
        let value = |node: &PointwiseNode<()>| {
            node.filter()
                .unwrap()
                .parameter(0)
                .unwrap()
                .effective_environment()
                .unwrap()
        };
        assert!(Arc::ptr_eq(&value(&edited.node()), &map));
        assert!(Arc::ptr_eq(&value(&unrelated.node()), &map));
        assert!(Arc::ptr_eq(&value(&edited_frame.node), &draft));
        assert!(Arc::ptr_eq(&value(&unrelated_frame.node), &map));
        assert!(!edited_frame.same_render_values(&unrelated_frame));
    }

    fn bmp() -> Vec<u8> {
        let mut bytes = vec![0; 62];
        bytes[..2].copy_from_slice(b"BM");
        bytes[2..6].copy_from_slice(&62u32.to_le_bytes());
        bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
        bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
        bytes[18..22].copy_from_slice(&2i32.to_le_bytes());
        bytes[22..26].copy_from_slice(&1i32.to_le_bytes());
        bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
        bytes[54..60].copy_from_slice(&[30, 20, 10, 60, 50, 40]);
        bytes
    }
    fn image(node: &PointwiseNode<()>) -> PointwiseImportedImage<()> {
        node.image_generator_handle()
            .unwrap()
            .parameter(P::Image)
            .custom_value()
            .unwrap()
            .unwrap()
            .imported_image()
            .unwrap()
    }
    #[test]
    fn pending_frame_owns_click_bytes_warm_cells_and_shared_node_memo() {
        let live = PointwiseNode::<()>::image_generator(ImageGeneratorKind::ImportedImage).unwrap();
        let payload = image(&live);
        let bytes = ByteBuffer::new(bmp());
        payload.set_bytes(Some(bytes.clone())).unwrap();
        let warm = payload.image().unwrap().unwrap();
        let cells = warm.channel(0, true).unwrap();
        let before = cells.values();
        let mut session = RenderFreezeSession::new();
        let first = session.freeze_node(&live, 0.).unwrap();
        let second = session.freeze_node(&live, 0.).unwrap();
        assert!(first.node.same_reference(&second.node));
        let detached = image(&first.node);
        let captured = detached.image().unwrap().unwrap();
        assert!(!captured.same_reference(&warm));
        assert!(!captured.channel(0, true).unwrap().shares_storage(&cells));
        assert!(!detached.bytes().unwrap().unwrap().same_reference(&bytes));
        let token = first.frozen_values().clone();
        let batch = session.finish().unwrap();
        bytes.write(54, 199).unwrap();
        cells.write(0, 0.875).unwrap();
        let mut current = RenderFreezeSession::new();
        assert_ne!(
            &token,
            current.freeze_node(&live, 0.).unwrap().frozen_values()
        );
        drop(current);
        payload.container().unwrap().store().unwrap().dispose();
        drop(payload);
        drop(live);
        drop(bytes);
        drop(warm);
        drop(cells);
        let mut batch = batch.materialize::<()>().unwrap();
        let first = first.complete(&mut batch).unwrap();
        let second = second.complete(&mut batch).unwrap();
        assert!(first.node.same_reference(&second.node));
        assert!(first.same_render_values(&second));
        drop(batch);
        assert_eq!(
            image(&first.node)
                .image()
                .unwrap()
                .unwrap()
                .channel(0, true)
                .unwrap()
                .values(),
            before
        );
    }
    #[test]
    fn pending_cannot_complete_with_unrelated_materialization_authority() {
        let node = PointwiseNode::<()>::solid_color().unwrap();
        let mut session = RenderFreezeSession::new();
        let pending = session.freeze_node(&node, 0.).unwrap();
        let mut unrelated = RenderFreezeSession::<()>::new()
            .finish()
            .unwrap()
            .materialize()
            .unwrap();
        assert!(matches!(
            pending.complete(&mut unrelated),
            Err(PointwiseError::UnsupportedCapability(_))
        ));
        // Cancelling the genuine unmaterialized batch releases only detached
        // roots; the live outward authority remains independently editable.
        drop(session);
        node.set_solid_color([0.25, 0.5, 0.75]).unwrap();
        assert_eq!(node.solid_color_values().unwrap(), [0.25, 0.5, 0.75]);
    }
}
