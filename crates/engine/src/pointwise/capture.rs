//! Click-time export-frame capture over this closed render domain. This is not
//! source Clone, serialization, a graph clone, or a second editable UI model.
use super::super::{GradientGenerator, GradientRenderSnapshot, PointwiseRenderOptions};
use super::retained_spatial::matrix_object::CapturedJpeg;
use super::*;
use crate::GradientAnimation;
use crate::pointwise::region::NodeId;
#[path = "capture_values.rs"]
pub(crate) mod capture_values;
use crate::live_curve::frozen::CurveCapture;
use std::collections::BTreeMap;
#[path = "export_final_cache.rs"]
mod final_cache;
pub use final_cache::PointwiseFinalExportCache;
#[path = "capture_pending.rs"]
mod pending;
pub use pending::{
    EnvironmentRenderOverride, FrozenRenderBatch, FrozenRenderValues, MaterializedRenderBatch,
    PendingFilterRender, PendingGradientRender, PendingNodeRender, PendingThumbnailRender,
    RenderFreezeSession,
};

/// The after-values of a staged editor edit, keyed by source graph occurrence.
/// These values are used only by the raw copier for a working preview.
#[derive(Default)]
pub(crate) struct DraftRenderOverrides {
    pub(crate) colors: BTreeMap<(u64, usize), [f64; 3]>,
    pub(crate) solids: BTreeMap<u64, [f64; 3]>,
    pub(crate) environments: BTreeMap<(u64, usize), Arc<crate::EnvironmentRecord>>,
    pub(crate) synthesis: BTreeMap<u64, crate::SynthesisRecord>,
    pub(in crate::pointwise) canvases: BTreeMap<u64, Arc<super::super::canvas::CanvasDocument>>,
    pub(in crate::pointwise) styles: BTreeMap<u64, Arc<super::super::canvas::CanvasStyle>>,
}

impl DraftRenderOverrides {
    // A sampled query follows the Reference route and its saved fallback. The copier uses
    // the unsampled query to decide whether a committed cache is reusable
    // across every time captured by this session.
    fn affects<E: 'static>(&self, node: &crate::PointwiseNode<E>, time: Option<f64>) -> bool {
        let mut pending = vec![node.clone()];
        let mut visited = std::collections::HashSet::new();
        while let Some(node) = pending.pop() {
            let id = node.occurrence_id();
            if !visited.insert(id) {
                continue;
            }
            if self.solids.contains_key(&id)
                || self.synthesis.contains_key(&id)
                || self.canvases.contains_key(&id)
                || self.styles.contains_key(&id)
                || self
                    .colors
                    .range((id, 0)..=(id, usize::MAX))
                    .next()
                    .is_some()
                || self
                    .environments
                    .range((id, 0)..=(id, usize::MAX))
                    .next()
                    .is_some()
            {
                return true;
            }
            if node.kind() == Some(PointwiseKind::Reference) {
                if let Some(time) = time {
                    pending.extend(node.reference_target_at(time).ok().flatten());
                    // A final graph with no prepared Group demand may use
                    // the saved slot even when the sampled selector differs.
                    pending.extend(node.reference_target().ok().flatten());
                } else if reference_has_dynamic_selector(&node).unwrap_or(true) {
                    if let Ok(Some(group)) = node.reference_group() {
                        pending.extend(group.slots().into_iter().filter_map(|slot| slot.node()));
                    }
                } else {
                    pending.extend(node.reference_target().ok().flatten());
                }
            }
            pending.extend(
                node.slots()
                    .into_iter()
                    .chain(node.contents())
                    .filter_map(|slot| slot.node()),
            );
        }
        false
    }
}

fn reference_has_dynamic_selector<E: 'static>(
    node: &crate::PointwiseNode<E>,
) -> Result<bool, PointwiseError<E>> {
    let selector = node
        .filter()
        .ok_or(PointwiseError::InvalidCast)?
        .parameter(0)?;
    Ok(selector.axis_is_animated(0)? || selector.has_animated_binding()?)
}

type CapturedInvertNode<E> =
    crate::SharedInvertNode<super::super::invert_node::InvertCallbackError<E>>;
type NodeCopies<E> = Vec<(
    super::super::PointwiseNode<E>,
    super::super::PointwiseNode<E>,
)>;
type CapturedNodeIdentities = Arc<BTreeMap<NodeId, NodeId>>;

pub(in crate::pointwise) fn node_payload_values<E: 'static>(
    node: &super::super::PointwiseNode<E>,
    time: f64,
    comparison: &mut capture_values::Comparison<E>,
) -> Result<(u64, Vec<u64>), PointwiseError<E>> {
    use super::super::graph::Payload;
    let core = node.core();
    Ok(match &core.payload {
        Payload::Input { image, .. } => {
            let mut values = Vec::new();
            comparison.image(&guard(image).clone(), &mut values);
            (8, values)
        }
        Payload::SceneClearLights => (5, Vec::new()),
        Payload::SceneCombine => (7, Vec::new()),
        Payload::UvTexture { kind, .. } => (
            kind.capture_tag(),
            crate::pointwise::scene_primitive::capture_values::properties(
                node.uv_texture_properties()
                    .ok_or(PointwiseError::InvalidCast)?,
                comparison,
            )?,
        ),
        Payload::SceneTransform { .. } => (
            6,
            node.scene_transform_3d()
                .ok_or(PointwiseError::InvalidCast)?
                .raw_capture_values(comparison)?,
        ),
        Payload::ScenePrimitive { .. } => (
            4,
            node.scene_primitive()
                .ok_or(PointwiseError::InvalidCast)?
                .raw_capture_values(comparison)?,
        ),
        Payload::SceneImport { state } => {
            let mut values = crate::pointwise::scene_primitive::capture_values::properties(
                node.import_3d_properties()
                    .ok_or(PointwiseError::InvalidCast)?,
                comparison,
            )?;
            values.push(
                guard(&state.mesh)
                    .as_ref()
                    .map_or(0, crate::ImportedMesh::id),
            );
            (15, values)
        }
        Payload::Filter { .. } => (
            0,
            comparison
                .filter(&node.filter().ok_or(PointwiseError::InvalidCast)?, time)?
                .to_vec(),
        ),
        Payload::ImageGenerator { .. } => (
            1,
            node.image_generator_handle()
                .ok_or(PointwiseError::InvalidCast)?
                .raw_capture_values(comparison)?,
        ),
        Payload::Invert { model, wire, .. } => {
            let mut values = model.raw_capture_values(&mut |o, v| comparison.options(o, v));
            wire.capture_values(comparison, &mut values)?;
            (2, values)
        }
        Payload::Solid { model, .. } => (3, model.raw_wire_values(comparison)?),
    })
}

/// One scene capture preserves shared mutable roots across source and stages.
/// Capture on the model owner thread, then drop this session before dispatching
/// snapshots. No host delegate, source setter, lazy getter or rebuild runs here.
/// Concurrent external edits are not an atomic multi-object transaction.
/// Any returned capture error retires this session: create a fresh session for
/// another attempt. Previously returned snapshots remain usable and unchanged.
pub struct RenderCaptureSession<E = ()> {
    failed: bool,
    deferred_files: bool,
    final_graph: bool,
    environment_override: Option<Arc<EnvironmentRenderOverride<E>>>,
    draft_override: Option<DraftRenderOverrides>,
    curves: CurveCapture<E>,
    preset_curves: Vec<(crate::SharedCurveRecord, crate::SharedCurveRecord)>,
    color_inputs: crate::color_region::ColorInputCapture<E>,
    matrix_objects: Vec<(PointwiseCustomObject<E>, PointwiseCustomObject<E>)>,
    region_owners: Vec<(
        PointwiseCustomObject<E>,
        super::super::PointwiseNode<E>,
        usize,
    )>,
    matrix_arrays: Vec<(crate::MatrixCoefficients, crate::MatrixCoefficients)>,
    imported_containers: Vec<(ImportedImageContainer<E>, ImportedImageContainer<E>)>,
    captured_jpegs: BTreeMap<usize, Arc<CapturedJpeg>>,
    encoded_stores: Vec<(crate::EncodedByteStore, crate::EncodedByteStore)>,
    encoded_bytes: Vec<(crate::ByteBuffer, crate::ByteBuffer)>,
    images: Vec<(SharedImageMap, SharedImageMap)>,
    image_data: crate::image_map::ImageCapture,
    comparison: capture_values::Comparison<E>,
    gradients: crate::live_gradient::GradientCapture,
    options: Vec<(SharedSelectionOptions, SharedSelectionOptions)>,
    limits: Vec<(SharedLimits, SharedLimits)>,
    ranges: Vec<(SharedRangeOptionsRecord, SharedRangeOptionsRecord)>,
    curve_ranges: Vec<(SharedCurveRangeRecord, SharedCurveRangeRecord)>,
    histograms: Vec<(Histogram, Histogram)>,
    animations: Vec<(GradientAnimation<E>, GradientAnimation<E>)>,
    animation_flags: Vec<(crate::AnimationFlags, crate::AnimationFlags)>,
    animation_curves: Vec<(crate::AnimationCurves<E>, crate::AnimationCurves<E>)>,
    animation_limits: Vec<(crate::AnimationLimits, crate::AnimationLimits)>,
    adjustments: Vec<(
        super::wire::SharedPropertyAdjustment<E>,
        super::wire::SharedPropertyAdjustment<E>,
    )>,
    filters: Vec<(PointwiseFilter<E>, PointwiseFilter<E>)>,
    nodes: std::collections::HashMap<NodeId, (crate::PointwiseNode<E>, crate::PointwiseNode<E>)>,
    inverts: Vec<(CapturedInvertNode<E>, CapturedInvertNode<E>)>,
}
impl<E: 'static> Default for RenderCaptureSession<E> {
    fn default() -> Self {
        Self::new()
    }
}
impl<E: 'static> RenderCaptureSession<E> {
    pub fn new() -> Self {
        Self {
            failed: false,
            deferred_files: false,
            final_graph: false,
            environment_override: None,
            draft_override: None,
            curves: CurveCapture::default(),
            preset_curves: Vec::new(),
            color_inputs: crate::color_region::ColorInputCapture::default(),
            matrix_objects: Vec::new(),
            matrix_arrays: Vec::new(),
            region_owners: Vec::new(),
            imported_containers: Vec::new(),
            captured_jpegs: BTreeMap::new(),
            encoded_stores: Vec::new(),
            encoded_bytes: Vec::new(),
            images: Vec::new(),
            image_data: crate::image_map::ImageCapture::default(),
            comparison: capture_values::Comparison::default(),
            gradients: crate::live_gradient::GradientCapture::default(),
            options: Vec::new(),
            limits: Vec::new(),
            ranges: Vec::new(),
            curve_ranges: Vec::new(),
            histograms: Vec::new(),
            animations: Vec::new(),
            animation_flags: Vec::new(),
            animation_curves: Vec::new(),
            animation_limits: Vec::new(),
            adjustments: Vec::new(),
            filters: Vec::new(),
            nodes: std::collections::HashMap::new(),
            inverts: Vec::new(),
        }
    }
    fn ensure_usable(&self) -> Result<(), PointwiseError<E>> {
        if self.failed {
            Err(PointwiseError::UnsupportedCapability(
                "render capture session previously failed; create a new session",
            ))
        } else {
            Ok(())
        }
    }
    fn finish_capture<T>(
        &mut self,
        result: Result<T, PointwiseError<E>>,
    ) -> Result<T, PointwiseError<E>> {
        // Raw graph copying registers owners early to preserve cycles. On any
        // failure an unfinished memo may exist, so this session can never
        // publish another frame. Already returned snapshots remain independent.
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub(crate) fn copy_curve(
        &mut self,
        value: &SharedCurve<E>,
    ) -> Result<SharedCurve<E>, crate::CurveError<E>> {
        self.curves.curve(value)
    }
    pub(crate) fn copy_gradient(&mut self, value: &SharedGradient) -> SharedGradient {
        self.gradients.gradient(value)
    }
    pub(crate) fn copy_options(
        &mut self,
        value: &SharedSelectionOptions,
    ) -> SharedSelectionOptions {
        if let Some((_, copy)) = self
            .options
            .iter()
            .find(|(old, _)| old.same_reference(value))
        {
            return copy.clone();
        }
        let copy = SharedSelectionOptions::new(value.entries());
        self.options.push((value.clone(), copy.clone()));
        copy
    }
    pub(crate) fn copy_limits(&mut self, value: &SharedLimits) -> SharedLimits {
        if let Some((_, copy)) = self
            .limits
            .iter()
            .find(|(old, _)| old.same_reference(value))
        {
            return copy.clone();
        }
        let mut values = value.snapshot();
        values.use_range = values.use_range.as_ref().map(|v| {
            if let Some((_, copy)) = self.ranges.iter().find(|(old, _)| old.same_reference(v)) {
                return copy.clone();
            }
            let copy = SharedRangeOptionsRecord::new(v.copy_wire_value());
            self.ranges.push((v.clone(), copy.clone()));
            copy
        });
        values.curve_range = values.curve_range.as_ref().map(|v| {
            if let Some((_, copy)) = self
                .curve_ranges
                .iter()
                .find(|(old, _)| old.same_reference(v))
            {
                return copy.clone();
            }
            let copy = SharedCurveRangeRecord::new(v.copy_wire_value());
            self.curve_ranges.push((v.clone(), copy.clone()));
            copy
        });
        let copy = SharedLimits::from_backing(values);
        self.limits.push((value.clone(), copy.clone()));
        copy
    }
    fn copy_histogram(&mut self, value: &Histogram) -> Histogram {
        if let Some((_, copy)) = self
            .histograms
            .iter()
            .find(|(old, _)| old.same_reference(value))
        {
            return copy.clone();
        }
        let (arrays, selected, disabled) = {
            let state = guard(&value.0);
            (state.arrays.clone(), state.selected, state.disabled)
        };
        let arrays = arrays.map(|v| self.image_data.buffer(&v));
        let copy = Histogram(Arc::new(Mutex::new(HistogramState {
            arrays,
            selected,
            disabled,
        })));
        self.histograms.push((value.clone(), copy.clone()));
        copy
    }
    pub(crate) fn copy_animation(
        &mut self,
        value: &GradientAnimation<E>,
    ) -> Result<GradientAnimation<E>, crate::AnimationError<E>>
    where
        E: 'static,
    {
        if let Some((_, copy)) = self
            .animations
            .iter()
            .find(|(old, _)| old.same_reference(value))
        {
            return Ok(copy.clone());
        }
        let copy = value.capture_render_owner(self)?;
        self.animations.push((value.clone(), copy.clone()));
        Ok(copy)
    }
    pub(crate) fn copy_animation_flags(
        &mut self,
        old: &crate::AnimationFlags,
    ) -> Result<crate::AnimationFlags, crate::AnimationError<E>> {
        if let Some((_, copy)) = self
            .animation_flags
            .iter()
            .find(|(v, _)| v.same_reference(old))
        {
            return Ok(copy.clone());
        }
        let values = (0..old.len())
            .map(|i| {
                old.value(i as i32)
                    .map_err(|_| crate::AnimationError::Index)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let copy = crate::AnimationFlags::from_values(values);
        self.animation_flags.push((old.clone(), copy.clone()));
        Ok(copy)
    }
    pub(crate) fn copy_animation_curves(
        &mut self,
        old: &crate::AnimationCurves<E>,
    ) -> Result<crate::AnimationCurves<E>, crate::AnimationError<E>> {
        if let Some((_, copy)) = self
            .animation_curves
            .iter()
            .find(|(v, _)| v.same_reference(old))
        {
            return Ok(copy.clone());
        }
        let roots = (0..old.len())
            .map(|i| old.value(i as i32))
            .collect::<Result<Vec<_>, _>>()?;
        let values = roots
            .iter()
            .map(|v| {
                v.as_ref()
                    .map(|v| self.copy_curve(v))
                    .transpose()
                    .map_err(crate::AnimationError::Curve)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let copy = crate::AnimationCurves::from_values(values);
        self.animation_curves.push((old.clone(), copy.clone()));
        Ok(copy)
    }
    pub(crate) fn copy_animation_limits(
        &mut self,
        old: &crate::AnimationLimits,
    ) -> Result<crate::AnimationLimits, crate::AnimationError<E>> {
        if let Some((_, copy)) = self
            .animation_limits
            .iter()
            .find(|(v, _)| v.same_reference(old))
        {
            return Ok(copy.clone());
        }
        let roots = (0..old.len())
            .map(|i| {
                old.value(i as i32)
                    .map_err(|_| crate::AnimationError::Index)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let copy = crate::AnimationLimits::from_values(
            roots
                .iter()
                .map(|v| v.as_ref().map(|v| self.copy_limits(v)))
                .collect(),
        );
        self.animation_limits.push((old.clone(), copy.clone()));
        Ok(copy)
    }
    pub(crate) fn copy_adjustment(
        &mut self,
        old: &super::wire::SharedPropertyAdjustment<E>,
    ) -> Result<super::wire::SharedPropertyAdjustment<E>, PointwiseError<E>> {
        if let Some((_, copy)) = self.adjustments.iter().find(|(v, _)| v.same_reference(old)) {
            return Ok(copy.clone());
        }
        let mut parts = old.capture_parts();
        parts.names = parts.names.as_ref().map(|v| self.copy_options(v));
        parts.flags = parts
            .flags
            .as_ref()
            .map(|v| self.copy_animation_flags(v))
            .transpose()?;
        parts.curves = parts
            .curves
            .as_ref()
            .map(|v| self.copy_animation_curves(v))
            .transpose()?;
        let copy = super::wire::SharedPropertyAdjustment::from_capture_parts(parts);
        self.adjustments.push((old.clone(), copy.clone()));
        Ok(copy)
    }
    pub fn capture_generator(
        &mut self,
        value: &super::super::ImageGenerator<E>,
        time: f64,
    ) -> Result<super::super::ImageGeneratorRenderSnapshot, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_usable()?;
        let result = value.capture_with(self, time);
        self.finish_capture(result)
    }
    pub fn capture_gradient(
        &mut self,
        value: &GradientGenerator<E>,
        time: f64,
    ) -> Result<GradientRenderSnapshot, PointwiseError<E>>
    where
        E: 'static,
    {
        self.ensure_usable()?;
        let result = value.capture_with(self, time);
        self.finish_capture(result)
    }
    pub(in crate::pointwise) fn copy_custom_object(
        &mut self,
        old: &PointwiseCustomObject<E>,
    ) -> Result<PointwiseCustomObject<E>, PointwiseError<E>> {
        if let Some((_, copy)) = self
            .matrix_objects
            .iter()
            .find(|(o, _)| o.same_reference(old))
        {
            return Ok(copy.clone());
        }
        if let Ok(fields) = old.light_position() {
            let copy = PointwiseCustomObject::new_light_position();
            copy.set_light_position_x(fields[0])?;
            copy.set_light_position_y(fields[1])?;
            self.matrix_objects.push((old.clone(), copy.raw()));
            return Ok(copy.raw());
        }
        if let Ok(fields) = old.legacy_tone_curve() {
            let copy = PointwiseCustomObject::new_legacy_tone_curve(fields);
            self.matrix_objects.push((old.clone(), copy.raw()));
            return Ok(copy.raw());
        }
        if let Ok(value) = old.legacy_gradient() {
            let copy = PointwiseCustomObject::new_legacy_gradient(value);
            self.matrix_objects.push((old.clone(), copy.raw()));
            return Ok(copy.raw());
        }
        if let Ok(region) = old.region() {
            let copy = PointwiseCustomObject::captured_region(region.fields());
            self.matrix_objects.push((old.clone(), copy.raw()));
            if let Some((owner, parameter)) = region.capture_owner() {
                self.region_owners.push((copy.raw(), owner, parameter));
            }
            return Ok(copy.raw());
        }
        if let Ok(image) = old.imported_image() {
            // Publish/memo the fresh aoi body before any container receiver can
            // recurse to it. No live listener or source-copy operation runs.
            let copy = PointwiseImportedImage::new();
            self.matrix_objects
                .push((old.clone(), copy.custom_object().raw()));
            let (dimensions, valid, container) = image.capture_fields();
            let container = container
                .as_ref()
                .map(|v| self.copy_imported_container(v, &copy))
                .transpose()?;
            copy.install_captured_fields(dimensions, valid, container.as_ref());
            return Ok(copy.custom_object().raw());
        }
        let copy = match old.matrix() {
            Ok(m) => {
                let k = m.captured_payload();
                let array = k.coefficients().map(|a| {
                    if let Some((_, copy)) = self
                        .matrix_arrays
                        .iter()
                        .find(|(old, _)| old.same_reference(&a))
                    {
                        copy.clone()
                    } else {
                        let copy = crate::MatrixCoefficients::new(a.values());
                        self.matrix_arrays.push((a, copy.clone()));
                        copy
                    }
                });
                PointwiseCustomObject::captured_matrix(crate::MatrixKernel::captured_fields(
                    k.angle(),
                    k.radius(),
                    array,
                ))
            }
            Err(_) => PointwiseCustomObject::captured_histogram(
                self.copy_histogram(&old.histogram().ok_or(PointwiseError::InvalidCast)?),
            ),
        };
        self.matrix_objects.push((old.clone(), copy.raw()));
        Ok(copy.raw())
    }
    fn copy_bytes(&mut self, old: &crate::ByteBuffer) -> crate::ByteBuffer {
        if let Some((_, copy)) = self
            .encoded_bytes
            .iter()
            .find(|(v, _)| v.same_reference(old))
        {
            return copy.clone();
        }
        let copy = old.frozen_copy();
        self.encoded_bytes.push((old.clone(), copy.clone()));
        copy
    }
    fn copy_encoded_store(
        &mut self,
        old: &crate::EncodedByteStore,
    ) -> Result<crate::EncodedByteStore, PointwiseError<E>> {
        if let Some((_, copy)) = self
            .encoded_stores
            .iter()
            .find(|(v, _)| v.same_reference(old))
        {
            return Ok(copy.clone());
        }
        let copy = if self.deferred_files {
            old.freeze_with(|bytes| self.copy_bytes(bytes))
        } else {
            old.capture_with(|bytes| self.copy_bytes(bytes))
        }
        .map_err(|e| PointwiseError::ImportedImage(crate::ImportedImageDecodeError::Storage(e)))?;
        self.encoded_stores.push((old.clone(), copy.clone()));
        Ok(copy)
    }
    fn copy_image(&mut self, old: &SharedImageMap) -> SharedImageMap {
        if let Some((_, copy)) = self.images.iter().find(|(v, _)| v.same_reference(old)) {
            return copy.clone();
        }
        let fields = old.capture_fields();
        let copy = SharedImageMap::from_image(self.image_data.image(fields));
        self.images.push((old.clone(), copy.clone()));
        copy
    }
    fn copy_imported_container(
        &mut self,
        old: &ImportedImageContainer<E>,
        owner: &PointwiseImportedImage<E>,
    ) -> Result<ImportedImageContainer<E>, PointwiseError<E>> {
        if let Some((_, copy)) = self
            .imported_containers
            .iter()
            .find(|(v, _)| v.same_reference(old))
        {
            return Ok(copy.clone());
        }
        let copy = owner.empty_captured_container();
        // A receiver may itself retain this container, another container, or no
        // current container. The placeholder closes the genuine cyclic graph.
        self.imported_containers.push((old.clone(), copy.clone()));
        let store = old
            .store()
            .as_ref()
            .map(|v| self.copy_encoded_store(v))
            .transpose()?;
        let image = old.cached_image().as_ref().map(|v| self.copy_image(v));
        let jpeg = if image.is_none() {
            store
                .as_ref()
                .and_then(crate::EncodedByteStore::resident_bytes)
                .map(|bytes| bytes.generation())
                .filter(|encoded| encoded.starts_with(b"\xff\xd8"))
                .map(|encoded| {
                    self.captured_jpegs
                        .entry(Arc::as_ptr(&encoded) as usize)
                        .or_insert_with(|| Arc::new(CapturedJpeg::new(&encoded)))
                        .clone()
                })
        } else {
            None
        };
        let receivers = old
            .dimension_listeners()
            .iter()
            .map(|v| self.copy_custom_object(v))
            .collect::<Result<Vec<_>, _>>()?;
        copy.install_captured_fields(store, image, jpeg, &receivers);
        Ok(copy)
    }
    pub fn capture_filter_at_time(
        &mut self,
        value: &PointwiseFilter<E>,
        time: f64,
    ) -> Result<PointwiseRenderSnapshot<E>, PointwiseError<E>> {
        self.ensure_usable()?;
        let result = (|| {
            let mut frame = self.capture_filter(value)?;
            frame.time = time;
            frame.comparison = self.compare_node(&frame.model.node(), time)?;
            Ok(frame)
        })();
        self.finish_capture(result)
    }
    pub fn capture_filter(
        &mut self,
        value: &PointwiseFilter<E>,
    ) -> Result<PointwiseRenderSnapshot<E>, PointwiseError<E>> {
        self.ensure_usable()?;
        let result = (|| {
            let node = self.copy_node_raw(&value.node())?;
            let model = node.filter().ok_or(PointwiseError::InvalidCast)?;
            let comparison = self.compare_node(&node, 0.0)?;
            Ok(PointwiseRenderSnapshot {
                model,
                time: 0.0,
                comparison,
            })
        })();
        self.finish_capture(result)
    }
    fn copy_filter_raw(
        &mut self,
        value: &PointwiseFilter<E>,
    ) -> Result<PointwiseFilter<E>, PointwiseError<E>> {
        if let Some((_, copy)) = self
            .filters
            .iter()
            .find(|(old, _)| old.same_reference(value))
        {
            return Ok(copy.clone());
        }
        let owner_id = value.node().occurrence_id();
        let mut synthesis = value.synthesis_texture();
        let synthesis_wire = value
            .synthesis_wire()
            .map(|wire| wire.capture_with(|adjustment| self.copy_adjustment(adjustment)))
            .transpose()?;
        let property_count = self.final_graph.then(|| value.parameters().len());
        let (
            kind,
            factor,
            lab_inheritance,
            mut canvas,
            mut style,
            wizard_template,
            mut clone_mode,
            wizard_dynamic,
            wizard_label,
            wizard_positions,
            group_guid,
            mut parameters,
        ) = {
            let entries = guard(&value.core.filters);
            let state = &entries[value.ix];
            (
                state.kind,
                state.factor,
                state.lab_inheritance,
                state.canvas.clone(),
                state.style.clone(),
                state.wizard_template,
                state.clone_mode,
                if self.final_graph {
                    None
                } else {
                    state.wizard_dynamic.clone()
                },
                state.wizard_label.clone(),
                state.wizard_positions.clone(),
                state.group_guid,
                state
                    .parameters
                    .iter()
                    .take(property_count.unwrap_or(state.parameters.len()))
                    .map(|p| ParameterState {
                        extras: ParameterExtras {
                            ordinary_limits: p.extras.ordinary_limits.clone(),
                            size: p.extras.size.clone(),
                            seed_modifier: p.extras.seed_modifier,
                            wire: p.extras.wire.clone(),
                            animation: p.extras.animation.clone(),
                            allow_changes: p.extras.allow_changes,
                            ..ParameterExtras::default()
                        },
                        choice_memory: p.choice_memory.clone(),
                        name: p.name.clone(),
                        value: p.value.clone(),
                    })
                    .collect::<Vec<_>>(),
            )
        };
        if let Some(edit) = &self.environment_override {
            edit.replace_style(value, &mut canvas, &mut style)?;
        }
        if let Some(edit) = &self.draft_override {
            if let Some(record) = edit.synthesis.get(&owner_id) {
                synthesis = Some(record.texture().clone());
            }
            if let Some(replacement) = edit.canvases.get(&owner_id) {
                canvas = Some(replacement.clone());
            }
            if let Some(replacement) = edit.styles.get(&owner_id) {
                style = Some(replacement.clone());
            }
        }
        let group_presets = value
            .group_presets()
            .map(|presets| {
                let copy = guard(&presets).copy_with(false, &mut |reference| {
                    Ok::<_, PointwiseError<E>>(match reference {
                        crate::ReferenceValueHandle::Gradient(gradient) => {
                            crate::ReferenceValueHandle::Gradient(
                                crate::SharedGradientRecord::from_live(
                                    self.copy_gradient(&gradient.live()),
                                ),
                            )
                        }
                        crate::ReferenceValueHandle::Curve(curve) => {
                            let copy = if let Some((_, copy)) = self
                                .preset_curves
                                .iter()
                                .find(|(old, _)| old.same_reference(curve))
                            {
                                copy.clone()
                            } else {
                                let copy = curve.detached_copy();
                                self.preset_curves.push((curve.clone(), copy.clone()));
                                copy
                            };
                            crate::ReferenceValueHandle::Curve(copy)
                        }
                    })
                })?;
                Ok::<_, PointwiseError<E>>(Arc::new(Mutex::new(copy)))
            })
            .transpose()?;
        // These filters inherit zl.s8 and os.CloneFrom. Their constructors
        // supply fresh limits and literal options before assigning source values.
        let mut final_defaults = if self.final_graph
            && matches!(
                kind,
                PointwiseKind::ChangeBci
                    | PointwiseKind::ChangeHsb
                    | PointwiseKind::Colorize
                    | PointwiseKind::Levels
                    | PointwiseKind::GetChannel
                    | PointwiseKind::Combine
                    | PointwiseKind::Add
                    | PointwiseKind::Multiply
                    | PointwiseKind::FromHsb
                    | PointwiseKind::ToneCurve
                    | PointwiseKind::Matrix
                    | PointwiseKind::ClearMaps
                    | PointwiseKind::StoreMap
                    | PointwiseKind::MergeMaps
                    | PointwiseKind::FixMapContrast
                    | PointwiseKind::LayerMaps
            ) {
            clone_mode = PointwiseCloneMode::Clone;
            PointwiseFilter::<E>::new_in_mode(kind, clone_mode)?.parameters()
        } else {
            Vec::new()
        }
        .into_iter();
        // All child operations follow release of the owning cohort guard.
        for (index, p) in parameters.iter_mut().enumerate() {
            let default = final_defaults.next();
            let default_value = default.as_ref().map(|parameter| parameter.value());
            p.extras.ordinary_limits = if let Some(parameter) = &default {
                let entries = guard(&parameter.owner.core.filters);
                entries[parameter.owner.ix].parameters[parameter.ix]
                    .extras
                    .ordinary_limits
                    .clone()
            } else {
                p.extras
                    .ordinary_limits
                    .as_ref()
                    .map(|limits| self.copy_limits(limits))
            };
            p.extras.wire = p.extras.wire.capture_with(|a| self.copy_adjustment(a))?;
            if let Some(rgb) = self
                .draft_override
                .as_ref()
                .and_then(|edit| edit.colors.get(&(owner_id, index)))
            {
                let wire = &p.extras.wire;
                wire.replace_resolved_lab_color(wire.resolved_lab_color(), Some(*rgb));
            }
            p.extras.animation = p
                .extras
                .animation
                .as_ref()
                .map(|a| self.copy_animation(a))
                .transpose()?;
            p.value = match &p.value {
                PointwiseParameterValue::Number {
                    current,
                    previous,
                    limits,
                } => PointwiseParameterValue::Number {
                    current: *current,
                    previous: if default_value.is_some() {
                        *current
                    } else {
                        *previous
                    },
                    limits: match default_value {
                        Some(PointwiseParameterValue::Number { limits, .. }) => limits,
                        _ => self.copy_limits(limits),
                    },
                },
                PointwiseParameterValue::Choice {
                    current,
                    previous,
                    options,
                } => PointwiseParameterValue::Choice {
                    current: *current,
                    previous: if default_value.is_some() {
                        *current
                    } else {
                        *previous
                    },
                    options: match default_value {
                        Some(PointwiseParameterValue::Choice { options, .. }) => options,
                        _ => self.copy_options(options),
                    },
                },
                PointwiseParameterValue::Color(v) => {
                    let copy = crate::ContextColorValue::unlinked(v.components());
                    PointwiseParameterValue::Color(match v.input_reference() {
                        Some(i) => copy.with_input_reference(&self.color_inputs.copy(&i)),
                        None => copy,
                    })
                }
                PointwiseParameterValue::Text(v) => PointwiseParameterValue::Text(v.clone()),
                PointwiseParameterValue::Environment(environment) => {
                    PointwiseParameterValue::Environment(if self.final_graph {
                        // Final clones have no Wizard Lab owner. Resolve the
                        // authoritative source before detaching its storage.
                        value.parameter(index as i32)?.effective_environment()?
                    } else if let Some(map) = self
                        .draft_override
                        .as_ref()
                        .and_then(|edit| edit.environments.get(&(owner_id, index)))
                    {
                        map.clone()
                    } else {
                        self.environment_override
                            .as_ref()
                            .filter(|edit| edit.matches_parameter(value, index))
                            .map_or_else(|| environment.clone(), |edit| edit.map().clone())
                    })
                }
                PointwiseParameterValue::Variable(v) => {
                    PointwiseParameterValue::Variable(v.clone())
                }
                PointwiseParameterValue::Gradient(g) => {
                    let ramp = if let Some(parameter) = &default {
                        // CloneFrom copies the points through Gradient.k.
                        Some(match g {
                            Some(ramp) => ramp.source_clone()?,
                            None => parameter.gradient()?,
                        })
                    } else {
                        g.as_ref().map(|ramp| self.copy_gradient(ramp))
                    };
                    PointwiseParameterValue::Gradient(ramp)
                }
                PointwiseParameterValue::Curve(c) => {
                    PointwiseParameterValue::Curve(if let Some(parameter) = &default {
                        // os.CloneFrom creates a new composite curve and each
                        // new layer using rl's source copy constructor.
                        Some(match c {
                            Some(curve) => curve.source_clone()?,
                            None => parameter.curve()?,
                        })
                    } else {
                        c.as_ref().map(|c| self.copy_curve(c)).transpose()?
                    })
                }
                PointwiseParameterValue::Pair {
                    current,
                    previous,
                    limits,
                } => PointwiseParameterValue::Pair {
                    current: *current,
                    previous: *previous,
                    limits: self.copy_limits(limits),
                },
                PointwiseParameterValue::Region { current, previous } => {
                    PointwiseParameterValue::Region {
                        current: current
                            .as_ref()
                            .map(|o| self.copy_custom_object(o))
                            .transpose()?,
                        previous: previous
                            .as_ref()
                            .map(|o| self.copy_custom_object(o))
                            .transpose()?,
                    }
                }
                PointwiseParameterValue::Matrix { current, .. }
                    if kind == PointwiseKind::Levels && self.final_graph =>
                {
                    // ku.cj copies only histogram values into the fresh owner.
                    // os.ba then captures those values as the previous object.
                    let current = current
                        .as_ref()
                        .filter(|object| object.histogram().is_some())
                        .map(PointwiseCustomObject::source_clone)
                        .transpose()?
                        .unwrap_or_else(PointwiseCustomObject::new_histogram);
                    let previous = current.source_clone()?;
                    PointwiseParameterValue::Matrix {
                        current: Some(current.raw()),
                        previous: Some(previous.raw()),
                    }
                }
                PointwiseParameterValue::Matrix { current, .. }
                    if kind == PointwiseKind::Matrix && self.final_graph =>
                {
                    let current = match current {
                        Some(matrix) => matrix.source_clone()?,
                        None => crate::PointwiseMatrix::new()?.custom_object(),
                    };
                    let previous = current.source_clone()?;
                    PointwiseParameterValue::Matrix {
                        current: Some(current.raw()),
                        previous: Some(previous.raw()),
                    }
                }
                PointwiseParameterValue::Matrix { current, previous } => {
                    PointwiseParameterValue::Matrix {
                        current: current
                            .as_ref()
                            .map(|o| self.copy_custom_object(o))
                            .transpose()?,
                        previous: previous
                            .as_ref()
                            .map(|o| self.copy_custom_object(o))
                            .transpose()?,
                    }
                }
                PointwiseParameterValue::Histogram(h) => {
                    PointwiseParameterValue::Histogram(if default.is_some() {
                        h.source_clone()
                    } else {
                        self.copy_histogram(h)
                    })
                }
            };
        }
        let core = Arc::new(Cohort {
            filters: Mutex::new(vec![FilterState {
                factor,
                lab_inheritance,
                canvas,
                style,
                synthesis,
                synthesis_wire,
                synthesis_catalog: None,
                wizard_template,
                previous_templates: Default::default(),
                clone_mode,
                kind,
                parameters,
                wizard_dynamic,
                wizard_label,
                wizard_positions,
                group_guid,
                group_presets,
                cache: None,
                handlers: Vec::new(),
            }]),
        });
        let model = PointwiseFilter::publish(core, 0);
        if let Some(program) = value.node().shader_program() {
            model.node().load_shader_program(program);
        }
        self.filters.push((value.clone(), model.clone()));
        Ok(model)
    }
    fn copy_node_value(
        &mut self,
        value: &super::super::PointwiseNode<E>,
    ) -> Result<super::super::PointwiseNode<E>, PointwiseError<E>> {
        use super::super::graph::Payload;
        let core = value.core();
        Ok(match &core.payload {
            Payload::Input { image, .. } => {
                let copy = super::super::PointwiseNode::input();
                let image = guard(image)
                    .clone()
                    .as_ref()
                    .map(|image| self.copy_image(image));
                copy.set_input_image(image)?;
                copy
            }
            Payload::SceneClearLights => super::super::PointwiseNode::publish_scene_clear_lights(),
            Payload::SceneCombine => super::super::PointwiseNode::publish_scene_combine(),
            Payload::UvTexture { kind, properties } => {
                super::super::PointwiseNode::publish_uv_texture(
                    *kind,
                    properties
                        .iter()
                        .map(|property| property.copied(false, Some(self)))
                        .collect::<Result<_, _>>()?,
                )
            }
            Payload::SceneTransform { .. } => value
                .scene_transform_3d()
                .ok_or(PointwiseError::InvalidCast)?
                .capture_with(self)?
                .node(),
            Payload::ScenePrimitive { .. } => value
                .scene_primitive()
                .ok_or(PointwiseError::InvalidCast)?
                .capture_with(self)?
                .node(),
            Payload::SceneImport { state } => {
                state.copy(guard(&state.mesh).clone(), false, Some(self))?
            }
            Payload::Filter { .. } => self
                .copy_filter_raw(&value.filter().ok_or(PointwiseError::InvalidCast)?)?
                .node(),
            Payload::ImageGenerator { .. } if self.final_graph => value
                .image_generator_handle()
                .ok_or(PointwiseError::InvalidCast)?
                .copy_for_final_render(false)?
                .node(),
            Payload::ImageGenerator { .. } => value
                .image_generator_handle()
                .ok_or(PointwiseError::InvalidCast)?
                .copy_raw_with(self)?
                .node(),
            Payload::Invert { model, wire, .. } if self.final_graph => {
                super::super::PointwiseNode::publish_invert_with_wire(
                    model.copy_for_final_render(),
                    wire.source_copy()?,
                )
            }
            Payload::Invert { model, wire, .. } => {
                let captured_wire = wire.capture_with(self)?;
                let mut memo = std::mem::take(&mut self.inverts);
                let copy = model.capture_raw(&mut memo, &mut |o| self.copy_options(o));
                self.inverts = memo;
                super::super::PointwiseNode::publish_invert_with_wire(copy, captured_wire)
            }
            Payload::Solid { model, .. } => {
                let mut inputs = std::mem::take(&mut self.color_inputs);
                let copy = if self.final_graph {
                    model.copy_for_final_render(&mut inputs)
                } else {
                    model.capture_raw(&mut inputs, self)
                };
                self.color_inputs = inputs;
                let copy = copy.map_err(|error| match error {
                    crate::LiveSolidColorError::Property(error) => error,
                    _ => PointwiseError::UnsupportedCapability("Solid Color frame construction"),
                })?;
                if let Some(rgb) = self
                    .draft_override
                    .as_ref()
                    .and_then(|edit| edit.solids.get(&value.occurrence_id()))
                {
                    let property = copy.color_property();
                    let before = property
                        .color_value()
                        .map_err(super::super::solid::solid_property_error)?;
                    property
                        .replace_adjusted_color(&before, &crate::ContextColorValue::unlinked(*rgb));
                }
                super::super::PointwiseNode::publish_solid(copy)
            }
        })
    }

    pub(in crate::pointwise) fn copy_node_raw(
        &mut self,
        value: &super::super::PointwiseNode<E>,
    ) -> Result<super::super::PointwiseNode<E>, PointwiseError<E>> {
        if let Some((_, copy)) = self.nodes.get(&value.id) {
            return Ok(copy.clone());
        }
        let copy = self.copy_node_value(value)?;
        // Publish/memo BEFORE traversing Parent or Slots: child-first capture
        // must converge through its parent Median's slot back to this owner.
        self.nodes.insert(value.id, (value.clone(), copy.clone()));
        if value.kind() == Some(PointwiseKind::Reference) {
            let property = value
                .filter()
                .ok_or(PointwiseError::InvalidCast)?
                .parameter(0)?;
            let animated = property.axis_is_animated(0)?;
            let target = value.reference_target()?;
            let affected = self
                .draft_override
                .as_ref()
                .is_some_and(|edit| edit.affects(value, None));
            let own_cache = (!affected).then(|| value.cached_image()).flatten();
            if animated || target.is_none() {
                let image = own_cache.as_ref().map(|image| self.copy_image(image));
                if image.is_some() {
                    copy.set_cached_image(image)?;
                }
            }
            if !affected
                && (!animated || own_cache.is_none())
                && let Some(target) = target
                && let Some(output) = target.cached_rendered_output()
            {
                let target_copy = self.copy_node_raw(&target)?;
                let output = match output {
                    crate::RenderedImage::Still(image) => {
                        crate::RenderedImage::Still(self.copy_image(&image))
                    }
                    crate::RenderedImage::Animated(frames) => {
                        crate::RenderedImage::Animated(Arc::new(std::array::from_fn(|index| {
                            self.copy_image(&frames[index])
                        })))
                    }
                };
                target_copy.install_captured_rendered_output(output)?;
            }
        }
        while let Some((object, owner, parameter)) = self.region_owners.pop() {
            let owner = self.copy_node_raw(&owner)?;
            copy.merge_region(&owner);
            object.install_region_owner(Some(PointwiseCustomObject::bind_node(&owner, parameter)));
        }
        let (parent, slots, names) = value.capture_links();
        let parent = parent.as_ref().map(|n| self.copy_node_raw(n)).transpose()?;
        let slots = slots
            .iter()
            .map(|n| n.as_ref().map(|n| self.copy_node_raw(n)).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        copy.install_captured_links(parent, slots, names);
        let (is_filter, is_default_input, contents, names) = value.capture_mode_links();
        let contents = contents
            .iter()
            .map(|node| {
                node.as_ref()
                    .map(|node| self.copy_node_raw(node))
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        copy.install_captured_mode_links(is_filter, is_default_input, contents, names);
        copy.install_captured_default_preparation(value.default_input_preparation());
        Ok(copy)
    }

    pub(in crate::pointwise) fn copy_final_graph_nodes(
        root: &super::super::PointwiseNode<E>,
    ) -> Result<NodeCopies<E>, PointwiseError<E>> {
        use super::super::PointwiseNode;
        use std::collections::BTreeSet;

        fn copy_occurrence<E: 'static>(
            source: &PointwiseNode<E>,
            parent: Option<&PointwiseNode<E>>,
            copies: &mut NodeCopies<E>,
            active: &mut BTreeSet<NodeId>,
        ) -> Result<PointwiseNode<E>, PointwiseError<E>> {
            if !active.insert(source.id) {
                return Err(PointwiseError::UnsupportedCapability(
                    "cyclic final graph topology",
                ));
            }
            // zl.s8 starts a new property owner per occurrence. The normal
            // detached capture above instead preserves source graph aliases.
            let copy = {
                let mut values = RenderCaptureSession::new();
                values.final_graph = true;
                values.copy_node_value(source)?
            };
            copy.mark_final_graph_clone();
            let mut copy_children = |children: Vec<Option<PointwiseNode<E>>>| {
                children
                    .into_iter()
                    .map(|node| {
                        node.as_ref()
                            .map(|child| copy_occurrence(child, Some(&copy), copies, active))
                            .transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()
            };
            let (_, slots, names) = source.capture_links();
            copy.install_captured_links(parent.cloned(), copy_children(slots)?, names);
            let (_, _, contents, names) = source.capture_mode_links();
            // z7 becomes an ordinary Imported Image and the editor flags start clear.
            copy.install_captured_mode_links(false, false, copy_children(contents)?, names);
            copies.push((source.clone(), copy.clone()));
            active.remove(&source.id);
            Ok(copy)
        }

        let mut copies = Vec::new();
        copy_occurrence(root, None, &mut copies, &mut BTreeSet::new())?;
        Ok(copies)
    }
    fn captured_node_identities(&self) -> CapturedNodeIdentities {
        Arc::new(
            self.nodes
                .values()
                .map(|(source, copy)| (copy.id, source.id))
                .collect(),
        )
    }

    fn compare_node(
        &mut self,
        node: &super::super::PointwiseNode<E>,
        time: f64,
    ) -> Result<Arc<[u64]>, PointwiseError<E>> {
        fn visit<E: 'static>(
            node: &super::super::PointwiseNode<E>,
            time: f64,
            seen: &mut Vec<super::super::PointwiseNode<E>>,
            comparison: &mut capture_values::Comparison<E>,
            out: &mut Vec<u64>,
        ) -> Result<(), PointwiseError<E>> {
            if let Some(id) = seen.iter().position(|old| old.same_reference(node)) {
                out.extend([0, id as u64]);
                return Ok(());
            }
            seen.push(node.clone());
            out.extend([1, (seen.len() - 1) as u64]);
            let (kind, values) = node_payload_values(node, time, comparison)?;
            out.extend([kind, values.len() as u64]);
            out.extend(values);
            if let Some(shader) = node.shader_program() {
                out.extend([
                    u64::from(shader.is_imported_pixel_script()),
                    shader.source().len() as u64,
                ]);
                out.extend(shader.source().bytes().map(u64::from));
            }
            let frames = node.cached_rendered_output();
            let frames = frames
                .as_ref()
                .map(crate::RenderedImage::frames)
                .unwrap_or_default();
            out.push(frames.len() as u64);
            for frame in frames {
                comparison.image(&Some(frame.clone()), out);
            }
            if let Some(filter) = node.filter() {
                for p in filter.parameters() {
                    if let PointwiseParameterValue::Region { current, previous } = p.raw_value() {
                        for object in [current, previous] {
                            let owner = object
                                .as_ref()
                                .and_then(|o| o.region().ok())
                                .and_then(|r| r.capture_owner());
                            if let Some((owner, parameter)) = owner {
                                out.extend([1, parameter as u64]);
                                visit(&owner, time, seen, comparison, out)?;
                            } else {
                                out.push(0);
                            }
                        }
                    }
                }
            }
            let (parent, slots, names) = node.capture_links();
            match parent {
                None => out.push(0),
                Some(p) => {
                    out.push(1);
                    visit(&p, time, seen, comparison, out)?;
                }
            }
            out.push(slots.len() as u64);
            for (slot, name) in slots.into_iter().zip(names) {
                out.push(name.len() as u64);
                out.extend(name.into_iter().map(u64::from));
                match slot {
                    None => out.push(0),
                    Some(n) => {
                        out.push(1);
                        visit(&n, time, seen, comparison, out)?;
                    }
                }
            }
            let (is_filter, is_default_input, contents, names) = node.capture_mode_links();
            out.extend([
                u64::from(is_filter),
                u64::from(is_default_input),
                contents.len() as u64,
            ]);
            for (child, name) in contents.into_iter().zip(names) {
                out.push(name.len() as u64);
                out.extend(name.into_iter().map(u64::from));
                match child {
                    None => out.push(0),
                    Some(child) => {
                        out.push(1);
                        visit(&child, time, seen, comparison, out)?;
                    }
                }
            }
            match node.default_input_preparation() {
                None => out.push(0),
                Some((flags, mode, first, factors, second)) => {
                    out.push(1);
                    out.extend(flags.map(u64::from));
                    out.push(mode as u64);
                    out.extend(first.map(u64::from));
                    out.extend(factors.map(|v| u64::from(v.to_bits())));
                    out.extend(second.map(u64::from));
                }
            }
            Ok(())
        }
        let mut out = vec![time.to_bits()];
        visit(node, time, &mut Vec::new(), &mut self.comparison, &mut out)?;
        Ok(out.into())
    }
    /// Retains actual raw node owners and links; no child parameter evaluation
    /// or provider runs while capturing a supplied filter's presence topology.
    pub fn capture_node(
        &mut self,
        node: &super::super::PointwiseNode<E>,
        time: f64,
    ) -> Result<PointwiseNodeRenderSnapshot<E>, PointwiseError<E>> {
        self.ensure_usable()?;
        let result = (|| {
            let node = self.copy_node_raw(node)?;
            let comparison = self.compare_node(&node, time)?;
            Ok(PointwiseNodeRenderSnapshot {
                node,
                origins: self.captured_node_identities(),
                time,
                comparison,
            })
        })();
        self.finish_capture(result)
    }
}
/// Owned Original-node render frame. Raw topology uses sponsored IDs rather
/// than owning Arc backedges; comparison operands never read worker caches.
pub struct PointwiseNodeRenderSnapshot<E = ()> {
    node: super::super::PointwiseNode<E>,
    origins: CapturedNodeIdentities,
    time: f64,
    comparison: Arc<[u64]>,
}
impl<E> Clone for PointwiseNodeRenderSnapshot<E> {
    fn clone(&self) -> Self {
        Self {
            node: self.node.clone(),
            origins: self.origins.clone(),
            time: self.time,
            comparison: self.comparison.clone(),
        }
    }
}
/// A captured canvas card, with one reusable final graph across its samples.
pub struct PointwiseThumbnailRender<E = ()> {
    snapshot: PointwiseNodeRenderSnapshot<E>,
    graph: Option<crate::PointwiseFinalGraph<E>>,
    ambiguous_sources: std::collections::HashSet<NodeId>,
}

impl<E: 'static> PointwiseThumbnailRender<E> {
    /// Render one frame at the requested time. A legacy-only animated graph may
    /// return its full retained sequence when it cannot render a final sample.
    pub fn render_sample(
        &self,
        time: f64,
        animated: bool,
        execution: crate::ProceduralExecution,
        cache: &mut crate::PointwiseThumbnailCache,
        effects: &mut crate::PointwiseRenderEffects<'_, E>,
    ) -> Result<crate::PointwiseRenderOutcome<E>, PointwiseError<E>> {
        use crate::pointwise::graph::{FinalImageReuse, ThumbnailReuse};
        let geometry = cache.geometry();
        let options = cache.options();
        let result = if let Some(graph) = &self.graph {
            let reuse =
                ThumbnailReuse::new(cache, &self.snapshot.origins, &self.ambiguous_sources, time);
            graph.render_frame(
                geometry,
                time,
                execution,
                options,
                FinalImageReuse::Thumbnail(reuse),
                effects,
            )
        } else {
            self.snapshot
                .render_export_frame(None, geometry, time, execution, options, effects)
        };
        match result {
            Err(error) if animated && self.graph.is_none() && error.is_unsupported_capability() => {
                self.snapshot
                    .render_preview_with_effects(geometry, execution, effects)
            }
            other => other,
        }
    }
}

impl<E: 'static> PointwiseNodeRenderSnapshot<E> {
    pub fn prepare_thumbnail(self) -> Result<PointwiseThumbnailRender<E>, PointwiseError<E>> {
        let graph = self.node.clone_supported_final_graph()?;
        let mut seen = std::collections::HashSet::new();
        let mut ambiguous_sources = std::collections::HashSet::new();
        if let Some(graph) = &graph {
            for source in graph.source_nodes() {
                if !seen.insert(source) {
                    ambiguous_sources.insert(source);
                }
            }
        }
        Ok(PointwiseThumbnailRender {
            snapshot: self,
            graph,
            ambiguous_sources,
        })
    }

    pub fn render_scene_with_effects(
        &self,
        effects: &mut crate::SceneAllocationEffects<'_>,
    ) -> Result<crate::Scene, PointwiseError<E>> {
        self.node.render_scene(self.time, effects)
    }

    pub fn same_render_values(&self, other: &Self) -> bool {
        self.comparison == other.comparison
    }
    pub fn render_with_execution_and_effects(
        &self,
        width: i32,
        height: i32,
        execution: crate::ProceduralExecution,
        options: PointwiseRenderOptions,
        effects: &mut crate::PointwiseRenderEffects<'_, E>,
    ) -> Result<super::super::PointwiseRenderOutcome<E>, PointwiseError<E>> {
        self.node
            .render_original_with_effects(width, height, options, self.time, execution, effects)
    }

    /// Prepare the detached graph's Preview output with the caller's render services.
    pub fn render_preview_with_effects(
        &self,
        geometry: crate::ImageGeometry,
        execution: crate::ProceduralExecution,
        effects: &mut crate::PointwiseRenderEffects<'_, E>,
    ) -> Result<super::super::PointwiseRenderOutcome<E>, PointwiseError<E>> {
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        // The retained Preview mode publishes sequences on its graph nodes.
        // Keep those caches in this request so a reusable snapshot can also
        // render stills and build a sequence at a different resolution.
        let preview = RenderCaptureSession::new().copy_node_raw(&self.node)?;
        preview.render_cached_graph_with_effects(
            crate::PointwiseCachedRenderMode::Preview,
            geometry,
            self.time,
            execution,
            PointwiseRenderOptions::new(false, false),
            effects,
        )
    }

    /// Use final-render semantics for supported graphs in an export.
    /// The originals for this operation are the job's frozen owners.
    pub fn render_export_with_effects(
        &self,
        geometry: crate::ImageGeometry,
        execution: crate::ProceduralExecution,
        options: PointwiseRenderOptions,
        effects: &mut crate::PointwiseRenderEffects<'_, E>,
    ) -> Result<super::super::PointwiseRenderOutcome<E>, PointwiseError<E>> {
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        let graph = self.node.clone_supported_final_graph()?;
        self.render_export_frame(
            graph.as_ref(),
            geometry,
            self.time,
            execution,
            options,
            effects,
        )
    }

    /// Render the selected Canvas's authored tile for the drawing workspace.
    /// Its input branches keep their own render settings.
    pub fn render_canvas_tile_with_effects(
        &self,
        geometry: crate::ImageGeometry,
        execution: crate::ProceduralExecution,
        options: PointwiseRenderOptions,
        effects: &mut crate::PointwiseRenderEffects<'_, E>,
    ) -> Result<super::super::PointwiseRenderOutcome<E>, PointwiseError<E>> {
        if self.node.kind() != Some(PointwiseKind::Canvas) {
            return Err(PointwiseError::InvalidCast);
        }
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        let graph = self.node.clone_supported_final_graph()?;
        let target = graph
            .as_ref()
            .map_or(self.node.id, |graph| graph.selected_id());
        self.render_export_frame(
            graph.as_ref(),
            geometry,
            self.time,
            execution,
            options.with_canvas_tile(target),
            effects,
        )
    }

    /// Render one standalone Synthesis layer from this frozen graph.
    /// The paint retains its own source, transparency, Scale and Seed.
    /// Layer blending and master color and sharpening apply to the whole texture.
    pub fn render_synthesis_layer_with_effects(
        &self,
        layer: usize,
        geometry: crate::ImageGeometry,
        execution: crate::ProceduralExecution,
        options: PointwiseRenderOptions,
        effects: &mut crate::PointwiseRenderEffects<'_, E>,
    ) -> Result<super::super::PointwiseRenderOutcome<E>, PointwiseError<E>> {
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        let texture = self
            .node
            .filter()
            .and_then(|filter| filter.synthesis_texture())
            .ok_or(PointwiseError::InvalidCast)?;
        if layer >= texture.layer_count() {
            return Err(PointwiseError::Argument);
        }
        let graph = self.node.clone_supported_final_graph()?;
        let target = graph
            .as_ref()
            .map_or(self.node.id, |graph| graph.selected_id());
        self.render_export_frame(
            graph.as_ref(),
            geometry,
            self.time,
            execution,
            options.with_synthesis_layer(target, layer),
            effects,
        )
    }

    /// Reads a Texture Synthesis paint from this frozen Style or Canvas owner.
    /// A layer identity selects a Canvas shape's Style, otherwise the owner is a Style node.
    pub fn synthesis_editor_record(
        &self,
        canvas_layer: Option<usize>,
        pass: usize,
    ) -> Option<crate::SynthesisRecord> {
        let filter = self.node.filter()?;
        let style = if let Some(identity) = canvas_layer {
            filter
                .canvas_layer_editors()
                .into_iter()
                .find(|layer| layer.identity() == identity)?
                .style_edit()?
        } else {
            filter.style_edit()?
        };
        Some(style.synthesis_edit(pass)?.record())
    }

    /// Evaluate desktop playback at the source's 32 sample times using the
    /// export renderer. Reuse the frozen final clone and eligible static
    /// raster results while keeping each frame's evaluation state independent.
    pub fn render_export_playback_with_effects(
        &self,
        geometry: crate::ImageGeometry,
        execution: crate::ProceduralExecution,
        options: PointwiseRenderOptions,
        effects: &mut crate::PointwiseRenderEffects<'_, E>,
    ) -> Result<super::super::PointwiseRenderOutcome<E>, PointwiseError<E>> {
        use crate::{
            PointwiseRenderOutcome, RenderedImage,
            pointwise::graph::{FinalImageReuse, PlaybackImageCache},
        };

        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        if !self.node.has_animated_preview()? {
            return self.render_export_with_effects(geometry, execution, options, effects);
        }
        let graph = self.node.clone_supported_final_graph()?;
        let mut cache = PlaybackImageCache::new();
        let mut frames = Vec::with_capacity(RenderedImage::PREVIEW_FRAME_COUNT);
        for index in 0..RenderedImage::PREVIEW_FRAME_COUNT {
            let time = index as f64 / RenderedImage::PREVIEW_FRAME_COUNT as f64;
            let result = match &graph {
                Some(graph) => graph.render_frame(
                    geometry,
                    time,
                    execution,
                    options,
                    FinalImageReuse::Playback(&mut cache),
                    effects,
                ),
                None => self.render_export_frame(None, geometry, time, execution, options, effects),
            };
            let frame = match result {
                // Some graph families only have a timed preview evaluator. Keep
                // playback available for them when no export route can render.
                Err(error) if graph.is_none() && error.is_unsupported_capability() => {
                    return self.render_preview_with_effects(geometry, execution, effects);
                }
                other => other?,
            };
            if !frame.caught_errors().is_empty() {
                return Ok(frame);
            }
            frames.push(frame.image().ok_or(PointwiseError::NullReference)?);
        }
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        let frames = frames
            .try_into()
            .map_err(|_| PointwiseError::InvalidOperation)?;
        Ok(PointwiseRenderOutcome {
            image: Some(RenderedImage::Animated(Arc::new(frames))),
            caught: Vec::new(),
        })
    }

    fn render_export_frame(
        &self,
        graph: Option<&crate::PointwiseFinalGraph<E>>,
        geometry: crate::ImageGeometry,
        time: f64,
        execution: crate::ProceduralExecution,
        options: PointwiseRenderOptions,
        effects: &mut crate::PointwiseRenderEffects<'_, E>,
    ) -> Result<super::super::PointwiseRenderOutcome<E>, PointwiseError<E>> {
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        if let Some(graph) = graph {
            graph.render_with_effects(geometry, time, execution, options, false, effects)
        } else {
            self.node.render_original_with_effects(
                geometry.width(),
                geometry.height(),
                options,
                time,
                execution,
                effects,
            )
        }
    }
}
/// A private detached rendering authority. No mutable parameter/model handle is
/// exposed; rendering may update only this job's real lazy/cache/option state.
/// Use one capture session for all stages to retain their source alias relations.
pub struct PointwiseRenderSnapshot<E = ()> {
    pub(super) model: PointwiseFilter<E>,
    time: f64,
    comparison: Arc<[u64]>,
}
impl<E> Clone for PointwiseRenderSnapshot<E> {
    fn clone(&self) -> Self {
        Self {
            model: self.model.clone(),
            time: self.time,
            comparison: self.comparison.clone(),
        }
    }
}
impl<E: 'static> PointwiseRenderSnapshot<E> {
    pub fn same_render_values(&self, other: &Self) -> bool {
        self.comparison == other.comparison
    }
    pub fn render_with_execution_and_effects(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
        execution: crate::ProceduralExecution,
        options: PointwiseRenderOptions,
        effects: &mut crate::PointwiseRenderEffects<'_, E>,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        self.model.render_supplied_at_time_with_effects(
            inputs, width, height, self.time, execution, options, effects,
        )
    }
    pub fn render_with_execution_and_capacity_query(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
        execution: crate::ProceduralExecution,
        options: PointwiseRenderOptions,
        query: &mut impl FnMut(i32) -> Result<bool, PointwiseError<E>>,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        self.model.render_supplied_at_time_with_capacity_query(
            inputs, width, height, self.time, execution, options, query,
        )
    }
    pub fn kind(&self) -> PointwiseKind {
        self.model.kind()
    }
    pub fn render_supplied_with_options(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
        options: PointwiseRenderOptions,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        self.model.render_supplied_at_time(
            inputs,
            width,
            height,
            self.time,
            crate::ProceduralExecution::CallingThread,
            options,
        )
    }
    /// The immutable model keeps the live supplied seam's lazy ambient timing;
    /// the capacity provider is supplied at render time, not captured as a bool.
    pub fn render_supplied_with_capacity_query(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
        options: PointwiseRenderOptions,
        query: &mut impl FnMut(i32) -> Result<bool, PointwiseError<E>>,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        self.render_with_execution_and_capacity_query(
            inputs,
            width,
            height,
            crate::ProceduralExecution::CallingThread,
            options,
            query,
        )
    }
    pub fn refresh_layer_map_options(
        &self,
        input: Option<&SharedImageMap>,
    ) -> Result<[i32; 2], PointwiseError<E>> {
        self.model.refresh_layer_map_options(input)
    }
}
impl<E: 'static> PointwiseFilter<E> {
    /// Standalone capture. Scene callers must share RenderCaptureSession to
    /// preserve children shared with another filter or generator animation.
    pub fn capture_render(&self) -> Result<PointwiseRenderSnapshot<E>, PointwiseError<E>> {
        RenderCaptureSession::new().capture_filter(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_copy_and_detached_capture_keep_distinct_actual_property_owners() {
        for kind in [PointwiseKind::Frequency, PointwiseKind::Fractalize] {
            let model = PointwiseFilter::<()>::new(kind).unwrap();
            let old = model.parameter(0).unwrap().number();
            let copied = model.source_clone().unwrap();
            let frame = model.capture_render().unwrap();
            model.parameter(0).unwrap().set_number(3.).unwrap();
            assert_eq!(copied.parameter(0).unwrap().number(), old);
            assert_eq!(frame.model.parameter(0).unwrap().number(), old);
            assert!(!copied.same_reference(&model));
            assert!(!frame.model.same_reference(&model));
        }
    }

    use crate::{AnalyticCurveKind, CurveLayerHandle, ImageGeometry, LimitsEdit, RgbColor};
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn input() -> SharedImageMap {
        let out = SharedImageMap::new(ImageGeometry::new(7, 5));
        for (ch, v) in [0.2, 0.4, 0.6, 0.25].into_iter().enumerate() {
            out.set_constant(ch as i32, v).unwrap();
        }
        out
    }
    fn bits(image: &SharedImageMap) -> Vec<Vec<u32>> {
        (0..4)
            .map(|ch| {
                image
                    .channel(ch, true)
                    .unwrap()
                    .values()
                    .into_iter()
                    .map(f32::to_bits)
                    .collect()
            })
            .collect()
    }
    #[test]
    fn all_current_filter_kinds_capture_the_real_kernel_at_non_preview_geometry() {
        use PointwiseKind::*;
        for kind in [
            ChangeBci,
            ChangeHsb,
            Colorize,
            Levels,
            GetChannel,
            Add,
            Multiply,
            FromHsb,
            ClearMaps,
            StoreMap,
            MergeMaps,
            FixMapContrast,
            LayerMaps,
            ToneCurve,
            Combine,
        ] {
            let filter = PointwiseFilter::<()>::new(kind).unwrap();
            let capture = filter.capture_render().unwrap();
            assert_eq!(capture.kind(), kind);
            let image = input();
            let inputs = vec![Some(image); kind.input_count()];
            let options = PointwiseRenderOptions::new(false, true);
            let expected = filter.render_supplied_with_options(&inputs, 7, 5, options);
            let actual = capture.render_supplied_with_options(&inputs, 7, 5, options);
            match (expected, actual) {
                (Ok(Some(a)), Ok(Some(b))) => {
                    assert_eq!(b.geometry(), ImageGeometry::new(7, 5));
                    assert_eq!(bits(&a), bits(&b), "{kind:?}");
                }
                (Ok(None), Ok(None)) => (),
                (Err(a), Err(b)) => assert_eq!(a.source_class(), b.source_class(), "{kind:?}"),
                _ => panic!("capture changed render outcome for {kind:?}"),
            }
        }
    }
    #[test]
    fn capture_preserves_raw_values_and_detaches_options_limits_histogram_without_observers() {
        let f = PointwiseFilter::<()>::new(PointwiseKind::ChangeBci).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        f.add_cache_handler(PointwiseCacheHandler::new(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        let mut session = RenderCaptureSession::new();
        let captured = session.capture_filter(&f).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let original = f.parameter(0).unwrap().options().unwrap();
        let frozen = captured.model.parameter(0).unwrap().options().unwrap();
        assert!(!original.same_reference(&frozen));
        original.set_entry(1, Some(vec![0xd800])).unwrap();
        assert_eq!(
            captured.model.parameter(0).unwrap().text().unwrap(),
            Some("No".encode_utf16().collect())
        );
        let PointwiseParameterValue::Number { limits, .. } = f.parameter(1).unwrap().value() else {
            panic!()
        };
        let PointwiseParameterValue::Number {
            limits: frozen_limits,
            ..
        } = captured.model.parameter(1).unwrap().value()
        else {
            panic!()
        };
        limits.edit(LimitsEdit::DecimalPlaces(17));
        assert_ne!(frozen_limits.snapshot().decimal_places, 17);
        let histogram = PointwiseFilter::<()>::new(PointwiseKind::Levels).unwrap();
        let snap = session.capture_filter(&histogram).unwrap();
        let live = histogram.parameter(0).unwrap().histogram().unwrap();
        let frozen = snap.model.parameter(0).unwrap().histogram().unwrap();
        live.arrays()[0]
            .write(1, f32::from_bits(0x8000_0000))
            .unwrap();
        assert_eq!(frozen.arrays()[0].read(1).unwrap().to_bits(), 0);
        assert!(!live.arrays()[0].shares_storage(&frozen.arrays()[0]));
    }
    #[test]
    fn distinct_gradient_roots_keep_shared_lists_points_and_cache_inside_the_job() {
        let a = SharedGradient::new();
        let b = SharedGradient::new();
        a.add_color(0, RgbColor::new(0.25, 0.5, 0.75, 1.0)).unwrap();
        a.rebuild().unwrap();
        b.set_color_points(a.color_points());
        b.set_cache(a.cache());
        let fa = PointwiseFilter::<()>::new(PointwiseKind::Colorize).unwrap();
        let fb = PointwiseFilter::<()>::new(PointwiseKind::Colorize).unwrap();
        fa.parameter(0)
            .unwrap()
            .set_gradient(Some(a.clone()))
            .unwrap();
        fb.parameter(0)
            .unwrap()
            .set_gradient(Some(b.clone()))
            .unwrap();
        let mut session = RenderCaptureSession::new();
        let ca = session.capture_filter(&fa).unwrap();
        let cb = session.capture_filter(&fb).unwrap();
        let ga = ca.model.parameter(0).unwrap().gradient().unwrap();
        let gb = cb.model.parameter(0).unwrap().gradient().unwrap();
        assert!(!ga.same_reference(&gb));
        assert!(
            ga.color_points()
                .unwrap()
                .same_reference(&gb.color_points().unwrap())
        );
        assert!(
            ga.color_point(0)
                .unwrap()
                .unwrap()
                .same_reference(&gb.color_point(0).unwrap().unwrap())
        );
        assert!(ga.cache().unwrap().same_reference(&gb.cache().unwrap()));
        assert!(
            !ga.color_points()
                .unwrap()
                .same_reference(&a.color_points().unwrap())
        );
        assert!(!ga.cache().unwrap().same_reference(&a.cache().unwrap()));
        a.color_point(0)
            .unwrap()
            .unwrap()
            .set_color(RgbColor::new(1.0, 0.0, 0.0, 1.0));
        assert_eq!(ga.color_point(0).unwrap().unwrap().color().argb()[1], 0.25);
    }
    #[test]
    fn lazy_children_stay_lazy_and_render_rebuilds_only_detached_curve_cache() {
        let tone = PointwiseFilter::<()>::new(PointwiseKind::ToneCurve).unwrap();
        let _ = tone.capture_render().unwrap();
        assert!(matches!(
            tone.parameter(0).unwrap().value(),
            PointwiseParameterValue::Curve(None)
        ));
        let layer = CurveLayerHandle::new_custom();
        layer.add_custom_point(0.0, 0.0).unwrap();
        layer.add_custom_point(1.0, 100.0).unwrap();
        let curve = SharedCurve::new(0.0, 1.0, false);
        curve.append(&[Some(layer.clone())]);
        tone.parameter(0)
            .unwrap()
            .set_curve(Some(curve.clone()))
            .unwrap();
        let frozen = tone.capture_render().unwrap();
        assert!(layer.custom_cache().unwrap().is_none());
        frozen
            .render_supplied_with_options(
                &[Some(input())],
                7,
                5,
                PointwiseRenderOptions::new(false, true),
            )
            .unwrap();
        assert!(layer.custom_cache().unwrap().is_none());
        assert!(layer.is_custom_dirty().unwrap());
        let copied = frozen.model.parameter(0).unwrap().curve().unwrap();
        assert!(
            copied.layers()[0]
                .as_ref()
                .unwrap()
                .custom_cache()
                .unwrap()
                .is_some()
        );
        curve.clear();
        assert_eq!(copied.layers().len(), 1);
    }
    #[test]
    fn scene_curve_sharing_and_null_errors_survive_without_source_clone() {
        let a = PointwiseFilter::<()>::new(PointwiseKind::ToneCurve).unwrap();
        let b = PointwiseFilter::<()>::new(PointwiseKind::ToneCurve).unwrap();
        let curve = SharedCurve::new(0.0, 1.0, false);
        curve.append(&[None]);
        a.parameter(0)
            .unwrap()
            .set_curve(Some(curve.clone()))
            .unwrap();
        b.parameter(0).unwrap().set_curve(Some(curve)).unwrap();
        let mut session = RenderCaptureSession::new();
        let ca = session.capture_filter(&a).unwrap();
        let cb = session.capture_filter(&b).unwrap();
        assert!(
            ca.model
                .parameter(0)
                .unwrap()
                .curve()
                .unwrap()
                .same_reference(&cb.model.parameter(0).unwrap().curve().unwrap())
        );
        let result = ca.render_supplied_with_options(
            &[Some(input())],
            7,
            5,
            PointwiseRenderOptions::new(false, true),
        );
        assert_eq!(result.unwrap_err().source_class(), "NullReferenceException");
    }
    #[test]
    fn active_time_capture_freezes_evaluated_source_and_never_notifies_original() {
        use crate::GradientGeneratorParameterKind::{Mode, Size};
        let generator = GradientGenerator::<()>::new().unwrap();
        let size = generator.parameter(Size);
        let owner = size.animation().unwrap();
        owner.ensure_curve(0).unwrap();
        let curve = owner.curves().unwrap().value(0).unwrap().unwrap();
        curve.set_minimum(1.0);
        curve.set_maximum(3.0);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        owner.flags().unwrap().set(0, true).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        size.add_changed_handler(&crate::GradientGeneratorChangeHandler::new(move |_, _| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        let frozen = generator.capture_render(0.25).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let expected = generator.render_unbound(11, 7, 0.25).unwrap();
        generator.parameter(Mode).set_index(1).unwrap();
        curve.set_maximum(8.0);
        assert_eq!(bits(&frozen.render(11, 7).unwrap()), bits(&expected));
    }
    #[test]
    fn invert_and_solid_snapshots_freeze_bits_without_setter_rounding() {
        let node = crate::SharedInvertNode::<()>::new(crate::SelectionCloneMode::Original);
        let captured = node.capture_render().unwrap();
        node.property(0).unwrap().set_index(1).unwrap();
        let image = input();
        let out = captured.render_shared(Some(&image), 7, 5).unwrap();
        assert_eq!(
            out.channel(0, true).unwrap().read(0).unwrap().to_bits(),
            (1.0f32 - 0.2).to_bits()
        );
        let solid = crate::SolidColorNodeSnapshot {
            mode: crate::SolidColorCloneMode::Original,
            color: [-0.0, 1.125, f64::from_bits(0x7ff8_0000_0000_0001)],
            transparency: -0.0,
        };
        let mut out = solid.render(11, 7).unwrap();
        assert_eq!(
            out.channel(0, true).unwrap().read(0).unwrap().to_bits(),
            0x8000_0000
        );
        assert_eq!(out.channel(1, true).unwrap().read(0).unwrap(), 1.125);
        assert_eq!(
            out.channel(3, true).unwrap().read(0).unwrap().to_bits(),
            0x8000_0000
        );
    }
    #[test]
    fn snapshots_are_transferable_without_ui_models() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<PointwiseRenderSnapshot<()>>();
        send_sync::<GradientRenderSnapshot>();
        send_sync::<crate::InvertRenderSnapshot>();
        send_sync::<crate::SolidColorNodeSnapshot>();
    }
}

#[cfg(test)]
#[path = "capture_node_test.rs"]
mod node_tests;

#[cfg(test)]
mod adaptive_constructor_mode_laws {
    use super::*;

    #[test]
    fn fresh_smear_constructor_mode_preserves_raw_normalized_flags_copy_and_capture() {
        for (mode, normalized, flags_present) in [
            (PointwiseCloneMode::Original, [6.; 2], true),
            (PointwiseCloneMode::Clone, [0.; 2], false),
        ] {
            let live = PointwiseFilter::<()>::with_clone_mode(PointwiseKind::Smear, mode).unwrap();
            let property = live.parameter(0).unwrap();
            assert_eq!(property.pair().unwrap(), [6.; 2]);
            assert_eq!(property.previous_pair().unwrap(), [6.; 2]);
            assert_eq!(property.normalized_pair().unwrap(), normalized);
            assert_eq!(property.value_at(0, 0.).unwrap(), normalized[0]);
            let source_copy = live.source_clone().unwrap();
            let frame = RenderCaptureSession::new().capture_filter(&live).unwrap();
            for model in [&live, &source_copy, &frame.model] {
                let p = model.parameter(0).unwrap();
                assert_eq!(p.pair().unwrap(), [6.; 2]);
                assert_eq!(p.previous_pair().unwrap(), [6.; 2]);
                assert_eq!(p.normalized_pair().unwrap(), normalized);
                let entries = guard(&model.core.filters);
                let size = entries[model.ix].parameters[0]
                    .extras
                    .size
                    .as_ref()
                    .unwrap();
                assert_eq!(size.flags.is_some(), flags_present);
                if let Some(flags) = &size.flags {
                    assert_eq!(flags.len(), 20);
                    assert!(flags.iter().all(|flag| !flag));
                }
            }
            property.set_pair([2., 3.]).unwrap();
            assert_eq!(
                frame.model.parameter(0).unwrap().normalized_pair().unwrap(),
                normalized
            );
        }
    }
}

#[cfg(test)]
mod slot_occurrence_capture_laws {
    use super::*;

    #[test]
    fn detached_capture_owns_new_occurrences_and_preserves_actual_child_aliases() {
        let node = PointwiseFilter::<()>::new(PointwiseKind::Add)
            .unwrap()
            .node();
        let child = super::super::super::PointwiseNode::solid_color().unwrap();
        let first = node.slot(0).unwrap();
        let second = node.slot(1).unwrap();
        first.set_node(Some(&child)).unwrap();
        second.set_node(Some(&child)).unwrap();
        let frame = RenderCaptureSession::new().capture_node(&node, 0.).unwrap();
        let captured_first = frame.node.slot(0).unwrap();
        let captured_second = frame.node.slot(1).unwrap();
        assert!(!captured_first.same_reference(&first));
        assert!(!captured_second.same_reference(&second));
        assert!(!captured_first.same_reference(&captured_second));
        let captured_child = captured_first.node().unwrap();
        assert!(captured_child.same_reference(&captured_second.node().unwrap()));
        assert!(!captured_child.same_reference(&child));
        assert!(captured_first.parent().same_reference(&frame.node));
        assert!(captured_child.parent().unwrap().same_reference(&frame.node));
        first.set_node(None).unwrap();
        assert!(
            captured_first
                .node()
                .unwrap()
                .same_reference(&captured_child)
        );
        assert!(
            captured_second
                .node()
                .unwrap()
                .same_reference(&captured_child)
        );
    }
}

#[cfg(test)]
mod imported_topology_tests {
    use super::*;
    #[test]
    fn imported_child_and_parent_first_capture_converge_through_fresh_slot_owners() {
        let live = crate::ImportedImageNode::<()>::new().unwrap();
        let parent = PointwiseFilter::<()>::new(PointwiseKind::MedianBlur).unwrap();
        parent
            .node()
            .slot(1)
            .unwrap()
            .set_node(Some(&live.node()))
            .unwrap();
        for child_first in [true, false] {
            let mut session = RenderCaptureSession::new();
            let (child, parent) = if child_first {
                let child = session.capture_node(&live.node(), 0.25).unwrap();
                let parent = session.capture_filter_at_time(&parent, 0.25).unwrap();
                (child, parent)
            } else {
                let parent = session.capture_filter_at_time(&parent, 0.25).unwrap();
                let child = session.capture_node(&live.node(), 0.25).unwrap();
                (child, parent)
            };
            assert!(
                child
                    .node
                    .parent()
                    .unwrap()
                    .same_reference(&parent.model.node())
            );
            assert!(
                parent
                    .model
                    .node()
                    .slot(1)
                    .unwrap()
                    .node()
                    .unwrap()
                    .same_reference(&child.node)
            );
            assert!(!child.node.same_reference(&live.node()));
            assert!(child.node.imported_image_handle().is_some());
        }
    }
    #[test]
    fn failure_after_imported_child_registration_retires_session_and_keeps_completed_frame() {
        let imported = crate::ImportedImageNode::<()>::new().unwrap();
        let mut session = RenderCaptureSession::new();
        let completed = session.capture_node(&imported.node(), 0.).unwrap();
        let original = completed.comparison.clone();
        let parent = PointwiseFilter::<()>::new(PointwiseKind::Add).unwrap();
        parent
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&imported.node()))
            .unwrap();
        let bad = PointwiseFilter::<()>::new(PointwiseKind::ToneCurve).unwrap();
        let curve = bad.parameter(0).unwrap().curve().unwrap();
        curve.clear();
        let layer = crate::CurveLayerHandle::new(crate::AnalyticCurveKind::Linear);
        layer
            .control_at(6)
            .unwrap()
            .unwrap()
            .set_child(Some(&layer))
            .unwrap();
        curve.insert(0, Some(&layer)).unwrap();
        parent
            .node()
            .slot(1)
            .unwrap()
            .set_node(Some(&bad.node()))
            .unwrap();
        assert!(matches!(
            session.capture_node(&parent.node(), 0.),
            Err(PointwiseError::Curve(_))
        ));
        let generator = imported.node().image_generator_handle().unwrap();
        assert!(matches!(
            session.capture_generator(&generator, 0.),
            Err(PointwiseError::UnsupportedCapability(_))
        ));
        assert!(matches!(
            session.capture_node(&imported.node(), 0.),
            Err(PointwiseError::UnsupportedCapability(_))
        ));
        assert_eq!(completed.comparison, original);
        curve.clear();
        assert!(
            RenderCaptureSession::new()
                .capture_node(&parent.node(), 0.)
                .is_ok()
        );
    }
}

#[cfg(test)]
mod wire_capture_laws {
    use super::*;
    #[test]
    fn adjustment_and_animation_keep_one_owned_array_graph_across_owner_types() {
        let animation = GradientAnimation::<()>::new(1, Some(SharedLimits::default())).unwrap();
        let curve = SharedCurve::<()>::new(0., 1., false);
        let flags = crate::AnimationFlags::from_values(vec![false]);
        let curves = crate::AnimationCurves::from_values(vec![Some(curve.clone())]);
        animation.set_flags(Some(flags.clone()));
        animation.set_curves(Some(curves.clone()));
        let metadata = super::super::wire::LegacyWireMetadata::<()>::new(true, true);
        let adjusted = metadata
            .adjustment(0, 1, Some(&SharedLimits::default()))
            .unwrap()
            .unwrap();
        let mut parts = adjusted.capture_parts();
        parts.flags = Some(flags.clone());
        parts.curves = Some(curves.clone());
        let adjusted = super::super::wire::SharedPropertyAdjustment::from_capture_parts(parts);
        let mut session = RenderCaptureSession::new();
        let a = session.copy_animation(&animation).unwrap();
        let b = session.copy_adjustment(&adjusted).unwrap();
        let again = session.copy_adjustment(&adjusted).unwrap();
        drop(session);
        assert!(again.same_reference(&b));
        assert!(!b.same_reference(&adjusted));
        let parts = b.capture_parts();
        let owned_flags = parts.flags.unwrap();
        let owned_curves = parts.curves.unwrap();
        assert!(a.flags().unwrap().same_reference(&owned_flags));
        assert!(a.curves().unwrap().same_reference(&owned_curves));
        assert!(!owned_flags.same_reference(&flags));
        assert!(!owned_curves.same_reference(&curves));
        assert!(
            !owned_curves
                .value(0)
                .unwrap()
                .unwrap()
                .same_reference(&curve)
        );
        flags.set(0, true).unwrap();
        assert!(!owned_flags.value(0).unwrap());
        drop(animation);
        drop(adjusted);
        drop(curves);
        drop(curve);
        drop(flags);
        assert!(a.curves().unwrap().value(0).unwrap().is_some());
    }
}

#[cfg(test)]
mod color_capture_laws {
    use super::*;
    #[test]
    fn color_inputs_share_one_captured_root_across_filters_and_detach_live_edits() {
        let a = PointwiseFilter::<()>::new(PointwiseKind::Selector).unwrap();
        let b = PointwiseFilter::<()>::new(PointwiseKind::Selector).unwrap();
        let live = crate::SharedColorInput::new_uncreated();
        for f in [&a, &b] {
            f.parameter(0)
                .unwrap()
                .set_color_input_reference(Some(&live))
                .unwrap();
        }
        let mut session = RenderCaptureSession::new();
        let ca = session.capture_filter(&a).unwrap();
        let cb = session.capture_filter(&b).unwrap();
        drop(session);
        let ai = ca
            .model
            .parameter(0)
            .unwrap()
            .color()
            .unwrap()
            .input_reference()
            .unwrap();
        let bi = cb
            .model
            .parameter(0)
            .unwrap()
            .color()
            .unwrap()
            .input_reference()
            .unwrap();
        assert!(ai.same_reference(&bi));
        assert!(!ai.same_reference(&live));
        live.set_value(&crate::ContextColorValue::unlinked([0.5, 0.25, 0.75]))
            .unwrap();
        assert_eq!(ai.value().components(), [0.; 3]);
        let fresh = RenderCaptureSession::new().capture_filter(&a).unwrap();
        assert!(!ca.same_render_values(&fresh));
        drop(a);
        drop(b);
        drop(live);
        assert_eq!(ai.value().components(), [0.; 3]);
    }
    #[test]
    fn captured_color_animation_and_component_state_have_independent_authorities() {
        let a = PointwiseFilter::<()>::new(PointwiseKind::Selector).unwrap();
        let p = a.parameter(0).unwrap();
        p.set_color(crate::ContextColorValue::unlinked([0.25, 0.5, 0.75]))
            .unwrap();
        let animation = p.animation().unwrap();
        let mut session = RenderCaptureSession::new();
        let frame = session.capture_filter(&a).unwrap();
        drop(session);
        let copy = frame.model.parameter(0).unwrap();
        assert!(!copy.animation().unwrap().same_reference(&animation));
        p.set_color(crate::ContextColorValue::unlinked([1., 0., 0.]))
            .unwrap();
        assert_eq!(copy.color().unwrap().components(), [0.25, 0.5, 0.75]);
        assert_eq!(
            copy.effective_color_at(0.).unwrap().components(),
            [0.25, 0.5, 0.75]
        );
    }
}

#[cfg(test)]
mod color_property_limits_capture_law {
    use super::*;
    #[test]
    fn ordinary_limits_capture_and_worker_freeze_preserve_shared_roots_and_detach_mutation() {
        let live = PointwiseFilter::<()>::new(PointwiseKind::Selector).unwrap();
        let original = live.parameter(0).unwrap().limits().unwrap();
        // Public Limits.Load installs a real non-null CurveRange child. The
        // property field itself stays the same readonly ordinary Limits owner.
        let supplied = SharedLimits::new(crate::LimitsValues {
            curve_range: Some(crate::SharedCurveRangeRecord::from_bounds(
                crate::DecimalBits::new([2, 0, 0, 0x8000_0000]).unwrap(),
                crate::DecimalBits::new([3, 0, 0, 0]).unwrap(),
                false,
            )),
            ..crate::LimitsValues::default()
        });
        let mut supplied_wire = crate::BinaryWriter::default();
        supplied.save(&mut supplied_wire).unwrap();
        original
            .load(&mut crate::BinaryCursor::new(supplied_wire.bytes()).unwrap())
            .unwrap();
        let copied = live.source_clone().unwrap();
        let range = original.snapshot().use_range.unwrap();
        let curve = original.snapshot().curve_range.unwrap();
        let mut capture = RenderCaptureSession::new();
        let first = capture.capture_filter(&live).unwrap();
        let second = capture.capture_filter(&copied).unwrap();
        drop(capture);
        let a = first.model.parameter(0).unwrap().limits().unwrap();
        let b = second.model.parameter(0).unwrap().limits().unwrap();
        assert!(a.same_reference(&b));
        assert!(!a.same_reference(&original));
        let ar = a.snapshot().use_range.unwrap();
        assert!(ar.same_reference(&b.snapshot().use_range.unwrap()));
        assert!(!ar.same_reference(&range));
        let ac = a.snapshot().curve_range.unwrap();
        assert!(ac.same_reference(&b.snapshot().curve_range.unwrap()));
        assert!(!ac.same_reference(&curve));
        let old_curve = ac.copy_wire_value().minimum;
        let old_range = ar.copy_wire_value().minimum;
        let mut freeze = super::super::RenderFreezeSession::new();
        let pending = freeze.freeze_filter_at_time(&live, 0.).unwrap();
        let token = pending.frozen_values().clone();
        let batch = freeze.finish().unwrap();
        original.edit(crate::LimitsEdit::Minimum(
            crate::DecimalBits::new([13, 0, 0, 0x8000_0000]).unwrap(),
        ));
        let mut replacement = range.copy_wire_value();
        replacement.minimum = crate::DecimalBits::new([31, 0, 0, 0x8000_0000]).unwrap();
        let mut wire = crate::BinaryWriter::default();
        replacement.save(&mut wire).unwrap();
        range
            .load(&mut crate::BinaryCursor::new(wire.bytes()).unwrap())
            .unwrap();
        let mut changed_curve = curve.copy_wire_value();
        changed_curve.minimum = crate::DecimalBits::new([7, 0, 0, 0x8000_0000]).unwrap();
        let mut curve_wire = crate::BinaryWriter::default();
        changed_curve.save(&mut curve_wire).unwrap();
        curve
            .load(&mut crate::BinaryCursor::new(curve_wire.bytes()).unwrap())
            .unwrap();
        assert_ne!(original.snapshot().minimum, a.snapshot().minimum);
        assert_eq!(ar.copy_wire_value().minimum, old_range);
        assert_eq!(ac.copy_wire_value().minimum, old_curve);
        let newer = RenderCaptureSession::new().capture_filter(&live).unwrap();
        assert!(
            !first.same_render_values(&newer),
            "silent actual limits/child edits invalidate semantic capture values"
        );
        let mut newer_freeze = super::super::RenderFreezeSession::new();
        let newer_pending = newer_freeze.freeze_filter_at_time(&live, 0.).unwrap();
        assert_ne!(&token, newer_pending.frozen_values());
        drop(newer_pending);
        drop(newer_freeze);
        let mut materialized = batch.materialize::<()>().unwrap();
        let completed = pending.complete(&mut materialized).unwrap();
        let owned = completed.model.parameter(0).unwrap().limits().unwrap();
        assert!(!owned.same_reference(&original));
        assert_eq!(owned.snapshot().minimum, a.snapshot().minimum);
        assert_eq!(
            owned
                .snapshot()
                .use_range
                .unwrap()
                .copy_wire_value()
                .minimum,
            old_range
        );
        assert_eq!(
            owned
                .snapshot()
                .curve_range
                .unwrap()
                .copy_wire_value()
                .minimum,
            old_curve
        );
        drop(live);
        drop(copied);
        drop(original);
        drop(range);
        drop(curve);
        drop(supplied);
        assert_eq!(a.snapshot().minimum, owned.snapshot().minimum);
    }
}

#[cfg(test)]
#[path = "scene_primitive/capture_laws.rs"]
mod scene_capture_laws;

#[cfg(test)]
mod group_preset_capture_tests {
    use super::*;
    #[test]
    fn capture_detaches_group_preset_references_from_live_values() {
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let guid = group.group_guid().unwrap();
        let gradient = crate::GtxPresetRecord::new(
            crate::GtxPresetKind::Gradient,
            Some("G".encode_utf16().collect()),
            guid,
        );
        if let crate::GtxPresetValue::Reference(crate::ReferenceValueHandle::Gradient(value)) =
            gradient.value()
        {
            value.live().set_width(200);
        }
        let curve = crate::GtxPresetRecord::new(
            crate::GtxPresetKind::Curve,
            Some("C".encode_utf16().collect()),
            guid,
        );
        let mut writer = crate::BinaryWriter::default();
        writer.write_i32(1).unwrap();
        writer
            .write_utf16(&"P".encode_utf16().collect::<Vec<_>>())
            .unwrap();
        writer.write_bytes(&guid).unwrap();
        writer.write_i32(2).unwrap();
        gradient.save(&mut writer).unwrap();
        curve.save(&mut writer).unwrap();
        let live = group.group_presets().unwrap();
        guard(&live)
            .load(&mut crate::BinaryCursor::new(writer.bytes()).unwrap(), 3.75)
            .unwrap();
        let captured = RenderCaptureSession::new().capture_filter(&group).unwrap();
        let frozen = captured.model.group_presets().unwrap();
        let get = |root: &Arc<Mutex<crate::GtxPresetCollection>>, index: usize| {
            let locked = guard(root);
            match locked.groups()[0].presets()[index].value() {
                crate::GtxPresetValue::Reference(value) => value.clone(),
                _ => panic!("reference expected"),
            }
        };
        let original_gradient = get(&live, 0);
        let captured_gradient = get(&frozen, 0);
        assert!(!original_gradient.same_reference(&captured_gradient));
        assert!(!get(&live, 1).same_reference(&get(&frozen, 1)));
        if let crate::ReferenceValueHandle::Gradient(value) = original_gradient {
            value.live().set_width(400);
        }
        if let crate::ReferenceValueHandle::Gradient(value) = captured_gradient {
            assert_eq!(value.live().width(), 200);
        }
    }
}
