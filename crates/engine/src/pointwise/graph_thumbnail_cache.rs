use super::{Payload, PointwiseKind, PointwiseNode};
use crate::{
    ImageGeneratorKind, ImageGeometry, PointwiseRenderOptions, SharedImageMap,
    pointwise::region::NodeId,
};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

const RASTER_BUDGET: usize = 32 * 1024 * 1024;
type RasterKey = (NodeId, Option<u64>);

/// Shared image intermediates for one captured document revision and resource
/// configuration. Create a new cache when the graph or its resources change.
pub struct PointwiseThumbnailCache {
    geometry: ImageGeometry,
    options: PointwiseRenderOptions,
    capacity: usize,
    images: HashMap<RasterKey, SharedImageMap>,
    order: VecDeque<RasterKey>,
}

impl PointwiseThumbnailCache {
    pub fn new(geometry: ImageGeometry, options: PointwiseRenderOptions) -> Self {
        // Charge four complete float planes even when the image is scalar.
        let bytes = (geometry.width() as usize)
            .saturating_mul(geometry.height() as usize)
            .saturating_mul(4 * size_of::<f32>());
        Self {
            geometry,
            options,
            capacity: RASTER_BUDGET.checked_div(bytes).unwrap_or(0).min(4096),
            images: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub(in crate::pointwise) fn geometry(&self) -> ImageGeometry {
        self.geometry
    }

    pub(in crate::pointwise) fn options(&self) -> PointwiseRenderOptions {
        self.options
    }

    fn insert(&mut self, key: RasterKey, image: &SharedImageMap) {
        if self.capacity == 0 || self.images.contains_key(&key) {
            return;
        }
        if self.images.len() == self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.images.remove(&oldest);
        }
        self.images.insert(key, image.share_pixel_backing());
        self.order.push_back(key);
    }
}

/// Proof of when a private final occurrence can share its raster with another
/// card. The cache key uses the original ID, which survives detached captures.
pub(in crate::pointwise) struct ThumbnailReuse<'a> {
    cache: &'a mut PointwiseThumbnailCache,
    origins: &'a BTreeMap<NodeId, NodeId>,
    ambiguous_sources: &'a HashSet<NodeId>,
    time: f64,
    reusable: HashMap<NodeId, bool>,
}

impl<'a> ThumbnailReuse<'a> {
    pub(in crate::pointwise) fn new(
        cache: &'a mut PointwiseThumbnailCache,
        origins: &'a BTreeMap<NodeId, NodeId>,
        ambiguous_sources: &'a HashSet<NodeId>,
        time: f64,
    ) -> Self {
        Self {
            cache,
            origins,
            ambiguous_sources,
            time,
            reusable: HashMap::new(),
        }
    }

    fn origin(&self, source: NodeId) -> Option<NodeId> {
        if self.ambiguous_sources.contains(&source) {
            return None;
        }
        self.origins.get(&source).copied()
    }

    pub(super) fn lookup(&self, source: NodeId) -> Option<SharedImageMap> {
        let origin = self.origin(source)?;
        self.cache
            .images
            .get(&(origin, None))
            .or_else(|| self.cache.images.get(&(origin, Some(self.time.to_bits()))))
            .map(SharedImageMap::share_pixel_backing)
    }

    pub(super) fn mark_reused(&mut self, node: NodeId, source: NodeId) {
        if let Some(origin) = self.origin(source) {
            self.reusable
                .insert(node, self.cache.images.contains_key(&(origin, None)));
        }
    }

    pub(super) fn record<E: 'static>(
        &mut self,
        node: &PointwiseNode<E>,
        source: NodeId,
        image: &SharedImageMap,
    ) {
        let Some(origin) = self.origin(source) else {
            return;
        };
        if image.geometry() != self.cache.geometry
            || image.region_store().is_some()
            || !image.named_channels().is_empty()
        {
            return;
        }
        let Some(inputs_static) = self.inputs_static(node) else {
            return;
        };
        let is_static = inputs_static
            && node
                .local_animated_preview_needed()
                .is_ok_and(|animated| !animated);
        self.reusable.insert(node.id, is_static);
        self.cache
            .insert((origin, (!is_static).then(|| self.time.to_bits())), image);
    }

    fn inputs_static<E: 'static>(&self, node: &PointwiseNode<E>) -> Option<bool> {
        if node.kind() == Some(PointwiseKind::Reference) {
            // A card rendered on its own uses the stored slot. A prepared Group
            // can use the sampled slot. Share only where these agree.
            let saved = node.reference_slot_at(None).ok()??;
            if node.reference_slot_at(Some(self.time)).ok()? != Some(saved) {
                return None;
            }
            let target = node.reference_target().ok()??;
            return self.reusable.get(&target.id).copied();
        }
        let allowed = match &node.core().payload {
            Payload::Solid { .. } | Payload::Invert { .. } => true,
            Payload::ImageGenerator { .. } => {
                node.image_generator_handle().is_some_and(|generator| {
                    matches!(
                        generator.kind(),
                        ImageGeneratorKind::Gradient
                            | ImageGeneratorKind::Noise
                            | ImageGeneratorKind::Cells
                            | ImageGeneratorKind::WoodRings
                    )
                })
            }
            Payload::Filter { kind, .. } => matches!(
                kind,
                PointwiseKind::Checkerboard
                    | PointwiseKind::ChangeBci
                    | PointwiseKind::ChangeHsb
                    | PointwiseKind::Combine
                    | PointwiseKind::Add
                    | PointwiseKind::Multiply
                    | PointwiseKind::ToneCurve
                    | PointwiseKind::Frequency
                    | PointwiseKind::Fractalize
                    | PointwiseKind::GaussianBlur
                    | PointwiseKind::Distort
                    | PointwiseKind::FreeTransform
                    | PointwiseKind::MoveRepeat
                    | PointwiseKind::RotateTile
                    | PointwiseKind::Pixelate
                    | PointwiseKind::Kaleidoscope
                    | PointwiseKind::WizardKaleidoscope
            ),
            _ => false,
        };
        if !allowed {
            return None;
        }
        // Require a recorded pure result for every connected input. A skipped
        // or special-purpose input makes reuse conservative for this node.
        let mut all_static = true;
        for input in node.slots().into_iter().filter_map(|slot| slot.node()) {
            all_static &= *self.reusable.get(&input.id)?;
        }
        Some(all_static)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AnalyticCurveKind, CurveLayerHandle, GradientGenerator, GradientGeneratorParameterKind,
        PointwiseFilter, PointwiseRenderEffects, ProceduralExecution, RenderCaptureSession,
        RenderedImage,
    };

    #[test]
    fn a_frozen_thumbnail_keeps_its_pixels_while_a_gradient_edit_invalidates_its_consumers() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        let source = gradient.node();
        let consumer = PointwiseNode::invert();
        consumer.slot(0).unwrap().set_node(Some(&source)).unwrap();
        let sibling = PointwiseNode::<()>::solid_color().unwrap();
        let revisions = || {
            [
                source.render_revision(),
                consumer.render_revision(),
                sibling.render_revision(),
            ]
        };
        let original = revisions();

        let mut capture = crate::RenderFreezeSession::new();
        let pending = capture.freeze_thumbnail(&consumer, 0.).unwrap();
        let frozen = capture.finish().unwrap();
        gradient
            .parameter(GradientGeneratorParameterKind::Location)
            .set_axis(0, 0.35)
            .unwrap();
        let changed = revisions();
        assert_ne!(original[0], changed[0]);
        assert_ne!(original[1], changed[1]);
        assert_eq!(original[2], changed[2]);

        let geometry = ImageGeometry::new(24, 16);
        let options = PointwiseRenderOptions::new(false, true);
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let execution = ProceduralExecution::CallingThread;
        let ready = frozen.materialize().unwrap();
        let old = pending
            .complete(&ready)
            .unwrap()
            .render_sample(
                0.,
                false,
                execution,
                &mut PointwiseThumbnailCache::new(geometry, options),
                &mut effects,
            )
            .unwrap()
            .image()
            .unwrap();
        let newest = RenderCaptureSession::new()
            .capture_node(&consumer, 0.)
            .unwrap()
            .render_export_with_effects(geometry, execution, options, &mut effects)
            .unwrap()
            .image()
            .unwrap();
        assert_ne!(
            old.materialize(0).unwrap().values(),
            newest.materialize(0).unwrap().values()
        );
    }

    #[test]
    fn canvas_samples_reuse_upstream_rasters_and_match_export_playback() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        let animation = gradient
            .parameter(GradientGeneratorParameterKind::Location)
            .animation()
            .unwrap();
        animation.ensure_curve(0).unwrap();
        let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
        curve.set_minimum(0.);
        curve.set_maximum(1.);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        animation.flags().unwrap().set(0, true).unwrap();
        let source = gradient.node();

        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        group.parameter(0).unwrap().set_number(1.).unwrap();
        let group = group.node();
        group
            .slot(0)
            .unwrap()
            .set_node_quiet(Some(&source))
            .unwrap();
        let reference = PointwiseFilter::new(PointwiseKind::Reference).unwrap();
        reference.parameter(0).unwrap().set_number(1.).unwrap();
        let reference = reference.node();
        let downstream = PointwiseNode::invert();
        downstream
            .slot(0)
            .unwrap()
            .set_node_quiet(Some(&reference))
            .unwrap();
        group
            .content(0)
            .unwrap()
            .set_node_quiet(Some(&downstream))
            .unwrap();
        let source_card = RenderCaptureSession::new()
            .capture_node(&source, 0.)
            .unwrap()
            .prepare_thumbnail()
            .unwrap();
        let group_frame = RenderCaptureSession::new()
            .capture_node(&group, 0.)
            .unwrap();
        let downstream_card = group_frame.clone().prepare_thumbnail().unwrap();
        let geometry = ImageGeometry::new(24, 16);
        let options = PointwiseRenderOptions::new(false, true);
        let mut cache = PointwiseThumbnailCache::new(geometry, options);
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let execution = ProceduralExecution::CallingThread;
        let baseline = group_frame
            .render_export_playback_with_effects(geometry, execution, options, &mut effects)
            .unwrap();
        let frames = baseline.output().unwrap();
        assert_eq!(frames.frames().len(), RenderedImage::PREVIEW_FRAME_COUNT);

        for index in [0, 13, 31] {
            let time = index as f64 / RenderedImage::PREVIEW_FRAME_COUNT as f64;
            let image = source_card
                .render_sample(time, true, execution, &mut cache, &mut effects)
                .unwrap()
                .image()
                .unwrap();
            let sample = (source.id, Some(time.to_bits()));
            assert!(
                cache.images[&sample]
                    .materialize(0)
                    .unwrap()
                    .shares_read_only_backing(&image.materialize(0).unwrap())
            );
            let operations = cache.order.len();
            let reread = source_card
                .render_sample(time, true, execution, &mut cache, &mut effects)
                .unwrap()
                .image()
                .unwrap();
            assert_eq!(cache.order.len(), operations);
            assert!(
                reread
                    .materialize(0)
                    .unwrap()
                    .shares_read_only_backing(&image.materialize(0).unwrap())
            );
            let actual = downstream_card
                .render_sample(time, true, execution, &mut cache, &mut effects)
                .unwrap()
                .image()
                .unwrap();
            for channel in 0..4 {
                let expected = frames.frames()[index]
                    .materialize(channel)
                    .unwrap()
                    .values();
                assert_eq!(actual.materialize(channel).unwrap().values(), expected);
            }
        }
    }
}
