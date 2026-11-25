//! Final graph clones, source guarantee shortcuts and original-owned caches.
use super::{
    Payload, PointwiseChannelGuarantees, PointwiseKind, PointwiseNode, PointwiseRenderOptions,
    PointwiseRenderOutcome, group_skips_color, guard,
};
use crate::pointwise::{
    check_render_cancelled, generator::FinalImageCache, parameter_ops, region::NodeId,
};
use crate::{
    ImageGeneratorKind, ImageGeometry, PointwiseError, PointwiseRenderEffects, ProceduralExecution,
    RenderCaptureSession, Scene, SharedImageMap,
};
use std::collections::{BTreeMap, BTreeSet};
#[path = "graph_final_cache_inputs.rs"]
mod cache_inputs;
pub(in crate::pointwise) use cache_inputs::FinalExportCacheTree;

/// The private final-render copies of a graph and its enclosing groups.
/// Each original accepts cache writes from the latest eligible copy by identity.
pub struct PointwiseFinalGraph<E = ()> {
    tree: PointwiseNode<E>,
    selected: PointwiseNode<E>,
    originals: BTreeMap<NodeId, PointwiseNode<E>>,
}

impl<E> PointwiseFinalGraph<E> {
    pub(in crate::pointwise) fn selected_id(&self) -> NodeId {
        self.selected.id
    }
}

#[derive(Default)]
pub(in crate::pointwise) struct StillExportCachePolicy {
    settings: Option<(ImageGeometry, PointwiseRenderOptions)>,
    last_placement_count: usize,
}

impl StillExportCachePolicy {
    pub(in crate::pointwise) fn matches(
        &self,
        geometry: ImageGeometry,
        options: PointwiseRenderOptions,
    ) -> bool {
        self.settings == Some((geometry, options))
    }
}

impl<E: 'static> PointwiseNode<E> {
    fn with_final_cache<T>(&self, f: impl FnOnce(&mut FinalImageCache) -> T) -> T {
        let core = self.core();
        match &core.payload {
            Payload::ImageGenerator { model } => f(&mut guard(model).final_cache),
            _ => f(&mut guard(&core.mode_events).final_cache),
        }
    }

    pub fn set_should_final_cache(&self, enabled: bool) {
        self.with_final_cache(|cache| cache.enabled = enabled);
    }

    pub fn set_final_cache_destruct_mode(&self, enabled: bool) {
        self.with_final_cache(|cache| cache.destruct_mode = enabled);
    }

    pub fn cached_final_image(&self) -> Option<SharedImageMap> {
        self.with_final_cache(|cache| cache.image.clone())
    }

    /// Clone the whole owning tree and retain the copy of this selected node.
    pub fn final_graph_clone(&self) -> Result<PointwiseFinalGraph<E>, PointwiseError<E>> {
        self.clone_supported_final_graph()?
            .ok_or(PointwiseError::UnsupportedCapability(
                "final graph node or topology",
            ))
    }

    // s8 starts at the outer tree. A final render requested for any contained
    // node still needs the real Parent path for Variables and References.
    pub(in crate::pointwise) fn clone_supported_final_graph(
        &self,
    ) -> Result<Option<PointwiseFinalGraph<E>>, PointwiseError<E>> {
        let mut tree = self.clone();
        let mut ancestors = BTreeSet::from([tree.id]);
        while let Some(parent) = tree.parent() {
            if !ancestors.insert(parent.id) {
                return Err(PointwiseError::UnsupportedCapability(
                    "cyclic final graph ancestry",
                ));
            }
            tree = parent;
        }
        let mut pending = vec![tree.clone()];
        let mut revisions = BTreeMap::new();
        while let Some(node) = pending.pop() {
            if revisions.contains_key(&node.id) {
                continue;
            }
            if node.is_clone_mode() {
                return Ok(None);
            }
            match &node.core().payload {
                Payload::ImageGenerator { .. }
                    if node.is_guaranteed_scalar_source()
                        || node.imported_image_handle().is_some() => {}
                Payload::Invert { .. } | Payload::Solid { .. } | Payload::Input { .. } => {}
                Payload::Filter { kind, .. }
                    if kind.is_group()
                        || matches!(
                            kind,
                            PointwiseKind::Reference
                                | PointwiseKind::Canvas
                                | PointwiseKind::Style
                                | PointwiseKind::Synthesis
                                | PointwiseKind::ChangeBci
                                | PointwiseKind::ChangeHsb
                                | PointwiseKind::Colorize
                                | PointwiseKind::ColorizeG2
                                | PointwiseKind::LegacyBricksBoards
                                | PointwiseKind::Levels
                                | PointwiseKind::GetChannel
                                | PointwiseKind::Combine
                                | PointwiseKind::Add
                                | PointwiseKind::Multiply
                                | PointwiseKind::FromHsb
                                | PointwiseKind::OklabColor
                                | PointwiseKind::Shader
                                | PointwiseKind::Reflect
                                | PointwiseKind::Refract
                                | PointwiseKind::ReflectEnvironment
                                | PointwiseKind::RefractEnvironment
                                | PointwiseKind::Exposure
                                | PointwiseKind::Selector
                                | PointwiseKind::PointLight
                                | PointwiseKind::IlluminateG2
                                | PointwiseKind::Shadow
                                | PointwiseKind::ShadowG2
                                | PointwiseKind::FullLight
                                | PointwiseKind::Hair
                                | PointwiseKind::Frequency
                                | PointwiseKind::Fractalize
                                | PointwiseKind::Regions
                                | PointwiseKind::GaussianBlur
                                | PointwiseKind::MotionBlur
                                | PointwiseKind::ShapeBlur
                                | PointwiseKind::Smear
                                | PointwiseKind::VariableBlur
                                | PointwiseKind::Distort
                                | PointwiseKind::FreeTransform
                                | PointwiseKind::Kaleidoscope
                                | PointwiseKind::WizardKaleidoscope
                                | PointwiseKind::MoveRepeat
                                | PointwiseKind::Remap
                                | PointwiseKind::Rotate
                                | PointwiseKind::RotateTile
                                | PointwiseKind::Sharpen
                                | PointwiseKind::Polygon
                                | PointwiseKind::MakeEnvironment
                                | PointwiseKind::RotateEnvironment
                                | PointwiseKind::DiffuseEnvironment
                                | PointwiseKind::ScatterEnvironment
                                | PointwiseKind::ConvertEnvironment
                                | PointwiseKind::Stereogram
                                | PointwiseKind::Caustics
                                | PointwiseKind::Transmit
                                | PointwiseKind::Atmosphere
                                | PointwiseKind::ToneCurve
                                | PointwiseKind::ToneCurveG2
                                | PointwiseKind::WizardColor
                                | PointwiseKind::WizardFiveColor
                                | PointwiseKind::WizardSlideColor
                                | PointwiseKind::WizardToneCurve
                                | PointwiseKind::WizardMatrix
                                | PointwiseKind::Matrix
                                | PointwiseKind::ClearMaps
                                | PointwiseKind::StoreMap
                                | PointwiseKind::MergeMaps
                                | PointwiseKind::FixMapContrast
                                | PointwiseKind::LayerMaps
                        ) => {}
                _ if node.is_scene_node() => {}
                _ => return Ok(None),
            }
            let revision = node.with_final_cache(|cache| cache.revision);
            revisions.insert(node.id, revision);
            // Reparenting detaches only the first matching occurrence.
            // s8 follows every remaining slot and assigns the copy's parent.
            pending.extend(
                node.slots()
                    .into_iter()
                    .chain(node.contents())
                    .filter_map(|slot| slot.node()),
            );
        }
        if !revisions.contains_key(&self.id) {
            return Ok(None);
        }
        let copies = RenderCaptureSession::copy_final_graph_nodes(&tree)?;
        let Some((_, tree_copy)) = copies
            .iter()
            .find(|(original, _)| original.same_reference(&tree))
        else {
            return Err(PointwiseError::InvalidOperation);
        };
        let Some((_, selected)) = copies
            .iter()
            .rfind(|(original, _)| original.same_reference(self))
        else {
            return Err(PointwiseError::InvalidOperation);
        };
        let tree = tree_copy.clone();
        let selected = selected.clone();
        let mut recipients = BTreeMap::new();
        let originals = copies
            .into_iter()
            .map(|(original, clone)| {
                recipients.insert(original.id, clone.id);
                (clone.id, original)
            })
            .collect::<BTreeMap<_, _>>();
        // The source assigns the recipient in s8 visitation order. Publish
        // only the last copy of each original after all cloning succeeds.
        for (original, clone) in recipients {
            originals[&clone].with_final_cache(|cache| {
                if revisions.get(&original) == Some(&cache.revision) {
                    cache.recipient = Some(clone);
                }
            });
        }
        Ok(Some(PointwiseFinalGraph {
            tree,
            selected,
            originals,
        }))
    }
}

impl<E: 'static> PointwiseFinalGraph<E> {
    /// Recompute the source chain-length cache placements for a still graph.
    /// The caller owns memory and output-size policy for electing this pass.
    pub fn prepare_automatic_caches(&self, time: f64) -> Result<usize, PointwiseError<E>> {
        let guarantees = prepare_guarantees(&self.tree, &|| false)?;
        self.place_final_caches(time, &guarantees)
    }

    fn place_final_caches(
        &self,
        time: f64,
        guarantees: &BTreeMap<NodeId, Prepared>,
    ) -> Result<usize, PointwiseError<E>> {
        let mut chains = BTreeMap::new();
        let total = measure_chain(
            &self.tree,
            time,
            guarantees,
            &mut chains,
            &mut BTreeSet::new(),
        )?;
        let spacing = (total / 10).max(3);
        let mut marked = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut pending = vec![(self.tree.clone(), total)];
        while let Some((node, mut previous)) = pending.pop() {
            if !visited.insert(node.id) {
                continue;
            }
            let (counted, children) = chain_children(&node, time, guarantees)?;
            let length = chains[&node.id];
            if counted
                && length >= spacing
                && total.saturating_sub(length) >= spacing
                && previous.saturating_sub(length) >= spacing
            {
                marked.insert(self.originals[&node.id].id);
                previous = length;
            }
            pending.extend(children.into_iter().rev().map(|child| (child, previous)));
        }
        let mut enabled = 0;
        for (&clone, original) in &self.originals {
            enabled += original.with_final_cache(|cache| {
                if cache.recipient != Some(clone) {
                    return 0;
                }
                if marked.contains(&original.id) {
                    cache.enabled = true;
                    cache.destruct_mode = false;
                    1
                } else {
                    if cache.enabled {
                        cache.destruct_mode = true;
                        cache.enabled = false;
                    }
                    0
                }
            });
        }
        Ok(enabled)
    }

    pub(in crate::pointwise) fn prepare_still_export_caches(
        &self,
        geometry: ImageGeometry,
        options: PointwiseRenderOptions,
        policy: &mut StillExportCachePolicy,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, PointwiseError<E>> {
        check_render_cancelled(cancelled)?;
        // aq9.wj receives an effective square render size. The PNG host
        // uses the longer side of a rectangular frame. cg defaults to 46000.
        let size = geometry.width().max(geometry.height());
        const SOURCE_RESOLUTION_LIMIT: f64 = 46_000.0;
        let edition_cache_limit = (SOURCE_RESOLUTION_LIMIT.powi(2) * 0.5).sqrt() as i32;
        if size > 2100 || size >= edition_cache_limit {
            self.reset_host_caches(true);
            return Ok(false);
        }
        let settings = (geometry, options);
        if policy.settings != Some(settings) {
            self.reset_host_caches(false);
            policy.settings = Some(settings);
        }

        let guarantees = prepare_guarantees(&self.tree, cancelled).map_err(final_render_error)?;
        let (count, active_caches) = self.count_cache_candidates(&guarantees, cancelled)?;
        if count <= 5 {
            self.reset_host_caches(true);
            return Ok(false);
        }

        let previous = policy.last_placement_count as f64;
        if active_caches < (count / 8).min(4)
            || active_caches > 12
            || (count as f64) < previous * 0.8
            || (count as f64) > previous * 1.2
        {
            // The export host always places caches at time zero. Final
            // traversal still samples its own requested time.
            self.place_final_caches(0.0, &guarantees)?;
            policy.last_placement_count = count;
        }
        Ok(true)
    }

    fn count_cache_candidates(
        &self,
        guarantees: &BTreeMap<NodeId, Prepared>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(usize, usize), PointwiseError<E>> {
        // zl.g(out count, out caches) counts the physical tree through
        // resolved passthrough slots. It does not follow Reference backedges.
        let mut pending = vec![self.tree.clone()];
        let mut count = 0;
        let mut active_caches = 0;
        while let Some(node) = pending.pop() {
            check_render_cancelled(cancelled)?;
            active_caches += self.originals[&node.id].with_final_cache(|cache| {
                // zl.g counts the source's cache flag at every occurrence.
                // Only its authorized clone will actually use that cache.
                usize::from(
                    cache
                        .recipient
                        .is_some_and(|id| self.originals.contains_key(&id))
                        && cache.enabled
                        && !cache.destruct_mode,
                )
            });
            if let Some(index) = guarantees[&node.id].channels.passthrough_slot() {
                pending.extend(node.slot(index as i32)?.node());
            } else {
                count += usize::from(
                    !node
                        .kind()
                        .is_some_and(|kind| kind.is_group() || kind == PointwiseKind::Reference),
                );
                pending.extend(
                    node.slots()
                        .into_iter()
                        .chain(node.contents())
                        .filter_map(|slot| slot.node()),
                );
            }
        }
        Ok((count, active_caches))
    }

    fn reset_host_caches(&self, disable: bool) {
        for (&clone, original) in &self.originals {
            let old = original.with_final_cache(|cache| {
                if cache.recipient != Some(clone) {
                    return None;
                }
                cache.destruct_mode = false;
                if disable {
                    cache.enabled = false;
                }
                cache.image.take()
            });
            drop(old);
        }
    }

    pub(in crate::pointwise) fn retire_destructive_caches(&self) {
        for (&clone, original) in &self.originals {
            let old = original.with_final_cache(|cache| {
                if cache.recipient != Some(clone) || !cache.destruct_mode {
                    return None;
                }
                cache.destruct_mode = false;
                cache.enabled = false;
                cache.image.take()
            });
            drop(old);
        }
    }

    /// Final evaluation runs a still frame on the private graph. The cache
    /// switch requests reuse and publication on matching source recipients.
    pub fn render_with_effects(
        &self,
        geometry: ImageGeometry,
        time: f64,
        execution: ProceduralExecution,
        options: PointwiseRenderOptions,
        use_final_cache: bool,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>> {
        let reuse = if use_final_cache {
            FinalImageReuse::Original
        } else {
            FinalImageReuse::None
        };
        self.render_frame(geometry, time, execution, options, reuse, effects)
    }

    pub(in crate::pointwise) fn render_frame(
        &self,
        geometry: ImageGeometry,
        time: f64,
        execution: ProceduralExecution,
        options: PointwiseRenderOptions,
        reuse: FinalImageReuse<'_>,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>> {
        check_render_cancelled(effects.cancel_check())?;
        let guarantees =
            prepare_guarantees(&self.tree, effects.cancel_check()).map_err(final_render_error)?;
        let mut state = FinalGraphExecution {
            originals: &self.originals,
            guarantees,
            images: BTreeMap::new(),
            scenes: BTreeMap::new(),
            prepared_groups: BTreeMap::new(),
            reuse,
        };
        self.selected
            .render_graph_in_effects(
                geometry.width(),
                geometry.height(),
                options,
                Some((time, execution)),
                effects,
                Some(&mut state),
            )
            .map_err(final_render_error)
    }
}

fn chain_children<E: 'static>(
    node: &PointwiseNode<E>,
    time: f64,
    guarantees: &BTreeMap<NodeId, Prepared>,
) -> Result<(bool, Vec<PointwiseNode<E>>), PointwiseError<E>> {
    let shortcut = guarantees[&node.id].channels.passthrough_slot();
    if node.kind() == Some(PointwiseKind::Reference) {
        Ok((false, node.reference_target_at(time)?.into_iter().collect()))
    } else if let Some(index) = shortcut {
        Ok((false, node.slot(index as i32)?.node().into_iter().collect()))
    } else if node.kind().is_some_and(PointwiseKind::is_group) {
        Ok((false, node.content(0)?.node().into_iter().collect()))
    } else {
        Ok((
            true,
            node.slots()
                .into_iter()
                .filter_map(|slot| slot.node())
                .collect(),
        ))
    }
}

fn measure_chain<E: 'static>(
    node: &PointwiseNode<E>,
    time: f64,
    guarantees: &BTreeMap<NodeId, Prepared>,
    chains: &mut BTreeMap<NodeId, usize>,
    active: &mut BTreeSet<NodeId>,
) -> Result<usize, PointwiseError<E>> {
    if !active.insert(node.id) {
        return Err(PointwiseError::UnsupportedCapability(
            "cyclic final cache placement",
        ));
    }
    let (counted, children) = chain_children(node, time, guarantees)?;
    let mut length = usize::from(counted);
    for child in children {
        if node.kind() != Some(PointwiseKind::Reference) || !chains.contains_key(&child.id) {
            length = length
                .checked_add(measure_chain(&child, time, guarantees, chains, active)?)
                .ok_or(PointwiseError::Arithmetic)?;
        }
    }
    active.remove(&node.id);
    chains.insert(node.id, length);
    Ok(length)
}

pub(super) fn final_render_error<E>(error: PointwiseError<E>) -> PointwiseError<E> {
    match error {
        PointwiseError::Cancelled
        | PointwiseError::Image(crate::ImageError::Allocation)
        | PointwiseError::Scene(crate::SceneError::Allocation)
        | PointwiseError::FinalRender(_) => error,
        error if error.is_unsupported_capability() => error,
        error => PointwiseError::FinalRender(Box::new(error)),
    }
}

#[derive(Clone, Copy)]
struct Prepared {
    channels: PointwiseChannelGuarantees,
    // zl.cy forwards a static Reference number through passthrough slots.
    reference_slot: Option<u32>,
    // zl.k records AllGuaranteed before looking for an identically valued input.
    uniform_before_input_match: bool,
    needs_region_render: bool,
}

impl Prepared {
    fn new(channels: PointwiseChannelGuarantees, reference_slot: Option<u32>) -> Self {
        Self {
            channels,
            reference_slot,
            uniform_before_input_match: channels.passthrough_slot().is_none()
                && !channels.has_scene()
                && channels.constants().iter().all(Option::is_some),
            needs_region_render: false,
        }
    }
}

pub(in crate::pointwise) struct LabSimplifications(BTreeMap<NodeId, Prepared>);

impl LabSimplifications {
    pub(in crate::pointwise) fn prepare<E: 'static>(
        root: &PointwiseNode<E>,
    ) -> Result<Self, PointwiseError<E>> {
        prepare_guarantees_for_use(root, &|| false, true).map(Self)
    }

    pub(in crate::pointwise) fn uniform(&self, id: NodeId) -> Option<[f32; 4]> {
        let value = self.0.get(&id)?;
        match value.channels.constants() {
            [Some(r), Some(g), Some(b), Some(a)] if value.uniform_before_input_match => {
                Some([r, g, b, a])
            }
            _ => None,
        }
    }

    pub(in crate::pointwise) fn channels(&self, id: NodeId) -> Option<PointwiseChannelGuarantees> {
        self.0.get(&id).map(|value| value.channels)
    }
}

fn prepare_guarantees<E: 'static>(
    root: &PointwiseNode<E>,
    cancelled: &dyn Fn() -> bool,
) -> Result<BTreeMap<NodeId, Prepared>, PointwiseError<E>> {
    prepare_guarantees_for_use(root, cancelled, false)
}

fn prepare_guarantees_for_use<E: 'static>(
    root: &PointwiseNode<E>,
    cancelled: &dyn Fn() -> bool,
    lab_expansion: bool,
) -> Result<BTreeMap<NodeId, Prepared>, PointwiseError<E>> {
    // zl.ao follows the saved Reference target and any passthrough slot to the
    // original producer. Combine uses this identity to detect two equal inputs.
    fn image_root<E: 'static>(
        mut node: Option<PointwiseNode<E>>,
        prepare: &mut impl FnMut(Option<PointwiseNode<E>>) -> Result<Prepared, PointwiseError<E>>,
    ) -> Result<Option<NodeId>, PointwiseError<E>> {
        let mut visited = BTreeSet::new();
        while let Some(current) = node {
            if !visited.insert(current.id) {
                return Err(PointwiseError::UnsupportedCapability(
                    "cyclic final image identity",
                ));
            }
            node = if current.kind() == Some(PointwiseKind::Reference) {
                current.reference_target()?
            } else if let Some(index) = prepare(Some(current.clone()))?.channels.passthrough_slot()
            {
                current.slot(index as i32)?.node()
            } else {
                return Ok(Some(current.id));
            };
        }
        Ok(None)
    }

    fn prepare_for_expansion<E: 'static>(
        node: &PointwiseNode<E>,
        values: &mut BTreeMap<NodeId, Prepared>,
        active: &mut BTreeSet<NodeId>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Prepared, PointwiseError<E>> {
        // A branch that cannot be prepared may produce image or auxiliary data.
        // Retain it and use an unknown result in its consumers.
        let unknown = || {
            Prepared::new(
                PointwiseChannelGuarantees::new([None; 4], [false; 4], true, None),
                None,
            )
        };
        if active.contains(&node.id) {
            return Ok(unknown());
        }
        match prepare(node, values, active, cancelled, true) {
            Ok(prepared) => Ok(prepared),
            Err(PointwiseError::Cancelled) => Err(PointwiseError::Cancelled),
            Err(_) => {
                active.remove(&node.id);
                let prepared = unknown();
                values.insert(node.id, prepared);
                Ok(prepared)
            }
        }
    }

    fn prepare<E: 'static>(
        node: &PointwiseNode<E>,
        values: &mut BTreeMap<NodeId, Prepared>,
        active: &mut BTreeSet<NodeId>,
        cancelled: &dyn Fn() -> bool,
        lab_expansion: bool,
    ) -> Result<Prepared, PointwiseError<E>> {
        check_render_cancelled(cancelled)?;
        if let Some(prepared) = values.get(&node.id) {
            return Ok(*prepared);
        }
        if !active.insert(node.id) {
            return Err(PointwiseError::UnsupportedCapability(
                "cyclic final graph guarantees",
            ));
        }
        let missing = Prepared::new(PointwiseChannelGuarantees::missing(), None);
        let mut child = |node: Option<PointwiseNode<E>>| {
            node.map_or(Ok(missing), |node| {
                if lab_expansion {
                    prepare_for_expansion(&node, values, active, cancelled)
                } else {
                    prepare(&node, values, active, cancelled, false)
                }
            })
        };
        let prepared = match node.kind() {
            Some(PointwiseKind::Reference) => {
                let property = node
                    .filter()
                    .ok_or(PointwiseError::InvalidCast)?
                    .parameter(0)?;
                let animated = property.axis_is_animated(0)? || property.has_animated_binding()?;
                if animated {
                    Prepared::new(
                        PointwiseChannelGuarantees::new([None; 4], [false; 4], false, None),
                        None,
                    )
                } else {
                    let target = node.reference_target()?;
                    let (constants, ranges) = match target {
                        Some(target) => {
                            let crosses_lab_input = target.parent().is_some_and(|parent| {
                                matches!(parent.kind(), Some(PointwiseKind::TemplateLab(_)))
                            });
                            let channels = child(Some(target))?.channels;
                            (
                                if crosses_lab_input {
                                    [None; 4]
                                } else {
                                    channels.constants()
                                },
                                std::array::from_fn(|channel| channels.effective_range(channel)),
                            )
                        }
                        None => ([None; 4], [true; 4]),
                    };
                    Prepared {
                        // ark.a7 suppresses constants for Lab exterior inputs.
                        // Its inherited de() has no physical slots to inspect.
                        channels: PointwiseChannelGuarantees::new(constants, ranges, false, None),
                        reference_slot: u32::try_from((property.number() as i32).wrapping_sub(1))
                            .ok(),
                        uniform_before_input_match: false,
                        needs_region_render: false,
                    }
                }
            }
            Some(kind) if kind.is_group() => {
                let contents = child(node.content(0)?.node())?;
                let slots = node.slots();
                let skip = group_skips_color(node, PointwiseRenderOptions::new(true, false))?;
                let mut passthrough = contents
                    .reference_slot
                    .map(|index| index as usize)
                    .filter(|&slot| slot < slots.len() && (!skip || slot == 0));
                let uniform_before_input_match = passthrough.is_none()
                    && !contents.needs_region_render
                    && !contents.channels.has_scene()
                    && contents.channels.constants().iter().all(Option::is_some);
                if uniform_before_input_match {
                    for (index, slot) in slots.iter().enumerate() {
                        if child(slot.node())?.channels.constants() == contents.channels.constants()
                        {
                            passthrough = Some(index);
                            break;
                        }
                    }
                }
                let source = match passthrough {
                    Some(slot) => child(slots[slot].node())?,
                    None => contents,
                };
                Prepared {
                    channels: PointwiseChannelGuarantees::new(
                        source.channels.constants(),
                        source.channels.in_range(),
                        source.channels.has_scene(),
                        passthrough,
                    ),
                    reference_slot: passthrough.and(source.reference_slot),
                    uniform_before_input_match,
                    needs_region_render: source.needs_region_render,
                }
            }
            _ if node.is_scene_node() => Prepared::new(
                PointwiseChannelGuarantees::new([None; 4], [false; 4], true, None),
                None,
            ),
            Some(PointwiseKind::Canvas | PointwiseKind::Style | PointwiseKind::Synthesis) => {
                Prepared::new(
                    PointwiseChannelGuarantees::new([None; 4], [false; 4], false, None),
                    None,
                )
            }
            Some(PointwiseKind::Combine) => {
                let slots = [
                    node.slot(0)?.node(),
                    node.slot(1)?.node(),
                    node.slot(2)?.node(),
                ];
                let inputs = [
                    child(slots[0].clone())?,
                    child(slots[1].clone())?,
                    child(slots[2].clone())?,
                ];
                let filter = node.filter().ok_or(PointwiseError::InvalidCast)?;
                let (channels, uniform_before_input_match) = super::super::combine::guarantees(
                    &filter,
                    inputs.map(|input| input.channels),
                    |a, b| match (&slots[a], &slots[b]) {
                        (None, None) => Ok(true),
                        (Some(a), Some(b)) => Ok(image_root(Some(a.clone()), &mut child)?
                            == image_root(Some(b.clone()), &mut child)?),
                        _ => Ok(false),
                    },
                )?;
                Prepared {
                    channels,
                    reference_slot: channels
                        .passthrough_slot()
                        .and_then(|index| inputs[index].reference_slot),
                    uniform_before_input_match,
                    needs_region_render: false,
                }
            }
            Some(
                PointwiseKind::Add
                | PointwiseKind::Multiply
                | PointwiseKind::FromHsb
                | PointwiseKind::OklabColor
                | PointwiseKind::Shader
                | PointwiseKind::Reflect
                | PointwiseKind::Refract
                | PointwiseKind::ReflectEnvironment
                | PointwiseKind::RefractEnvironment
                | PointwiseKind::Transmit
                | PointwiseKind::FullLight
                | PointwiseKind::Hair
                | PointwiseKind::Regions
                | PointwiseKind::Remap
                | PointwiseKind::MotionBlur
                | PointwiseKind::ShapeBlur
                | PointwiseKind::Smear
                | PointwiseKind::VariableBlur
                | PointwiseKind::Distort,
            ) => {
                let inputs = node
                    .slots()
                    .iter()
                    .map(|slot| child(slot.node()))
                    .collect::<Result<Vec<_>, _>>()?;
                let channels = inputs
                    .iter()
                    .map(|input| Some(input.channels))
                    .collect::<Vec<_>>();
                let (mut channels, uniform_before_input_match) =
                    node.calculate_image_guarantees_for_inputs_with_uniformity(&channels)?;
                let needs_region_render = node.kind() == Some(PointwiseKind::Regions)
                    && super::super::regions::retains_gap_map(
                        &node.filter().ok_or(PointwiseError::InvalidCast)?,
                    )?;
                if needs_region_render {
                    // Uniform colors can still carry a computed RegionsMap. Keep this
                    // result when rendering the node without imposing a scene demand on consumers.
                    channels = PointwiseChannelGuarantees::new(
                        channels.constants(),
                        channels.in_range(),
                        channels.has_scene(),
                        None,
                    );
                }
                Prepared {
                    channels,
                    reference_slot: channels
                        .passthrough_slot()
                        .and_then(|index| inputs[index].reference_slot),
                    uniform_before_input_match: uniform_before_input_match && !needs_region_render,
                    needs_region_render,
                }
            }
            _ => {
                let input = node
                    .slots()
                    .first()
                    .map(|slot| child(slot.node()))
                    .transpose()?;
                let (channels, uniform_before_input_match) = node
                    .calculate_image_guarantees_with_uniformity(
                        input.map(|input| input.channels),
                    )?;
                Prepared {
                    channels,
                    reference_slot: channels
                        .passthrough_slot()
                        .and(input.and_then(|input| input.reference_slot)),
                    uniform_before_input_match,
                    needs_region_render: false,
                }
            }
        };
        active.remove(&node.id);
        values.insert(node.id, prepared);
        Ok(prepared)
    }

    let mut values = BTreeMap::new();
    let mut pending = vec![root.clone()];
    let mut active = BTreeSet::new();
    let mut visited = BTreeSet::new();
    while let Some(node) = pending.pop() {
        if !visited.insert(node.id) {
            continue;
        }
        if lab_expansion {
            prepare_for_expansion(&node, &mut values, &mut active, cancelled)?;
        } else {
            prepare(&node, &mut values, &mut active, cancelled, false)?;
        }
        pending.extend(
            node.slots()
                .into_iter()
                .chain(node.contents())
                .filter_map(|slot| slot.node()),
        );
    }
    Ok(values)
}

pub(super) struct FinalGraphExecution<'a, E> {
    originals: &'a BTreeMap<NodeId, PointwiseNode<E>>,
    guarantees: BTreeMap<NodeId, Prepared>,
    images: BTreeMap<NodeId, SharedImageMap>,
    scenes: BTreeMap<NodeId, Option<Scene>>,
    prepared_groups: BTreeMap<NodeId, Vec<FinalSlotDemand>>,
    reuse: FinalImageReuse<'a>,
}

pub(in crate::pointwise) enum FinalImageReuse<'a> {
    None,
    Original,
    Playback(&'a mut PlaybackImageCache),
}

/// Only used for a single playback request on one detached final graph.
pub(in crate::pointwise) struct PlaybackImageCache {
    remaining_bytes: usize,
    images: BTreeMap<NodeId, SharedImageMap>,
    static_nodes: BTreeSet<NodeId>,
}

impl PlaybackImageCache {
    pub(in crate::pointwise) fn new() -> Self {
        Self {
            remaining_bytes: 32 * 1024 * 1024,
            images: BTreeMap::new(),
            static_nodes: BTreeSet::new(),
        }
    }

    fn record<E: 'static>(
        &mut self,
        node: &PointwiseNode<E>,
        image: &SharedImageMap,
        frame_images: &BTreeMap<NodeId, SharedImageMap>,
    ) {
        if self.images.contains_key(&node.id) || !self.is_static_image(node, image, frame_images) {
            return;
        }
        self.static_nodes.insert(node.id);
        // Charge for four full planes even when the image uses fewer.
        let geometry = image.geometry();
        let bytes = (geometry.width() as usize)
            .saturating_mul(geometry.height() as usize)
            .saturating_mul(4 * size_of::<f32>());
        if bytes == 0 || bytes > self.remaining_bytes {
            return;
        }
        self.remaining_bytes -= bytes;
        self.images.insert(node.id, image.share_pixel_backing());
    }

    fn is_static_image<E: 'static>(
        &self,
        node: &PointwiseNode<E>,
        image: &SharedImageMap,
        frame_images: &BTreeMap<NodeId, SharedImageMap>,
    ) -> bool {
        if let Some(generator) = node.image_generator_handle() {
            return matches!(
                generator.kind(),
                ImageGeneratorKind::Gradient
                    | ImageGeneratorKind::Noise
                    | ImageGeneratorKind::Cells
                    | ImageGeneratorKind::WoodRings
            ) && generator
                .parameters()
                .iter()
                .all(|property| property.binding_index() < 0)
                && generator
                    .animated_preview_needed()
                    .is_ok_and(|animated| !animated);
        }
        // Only admit workers with no external resources or context-dependent
        // inputs. Their actual results must own their raster and have no
        // auxiliary maps, so the pixel budget and per-frame identities hold.
        let Some(filter) = node
            .filter()
            .filter(|filter| filter.kind() == PointwiseKind::GaussianBlur)
        else {
            return false;
        };
        node.slot(0)
            .ok()
            .and_then(|slot| slot.node())
            .is_some_and(|source| {
                self.static_nodes.contains(&source.id)
                    && frame_images
                        .get(&source.id)
                        .is_some_and(|input| !image.same_reference(input))
            })
            && image.region_store().is_none()
            && image.named_channels().is_empty()
            && filter.parameters().iter().all(|property| {
                property.binding_index() < 0
                    && parameter_ops::is_animated(|| property.retained_animation())
                        .is_ok_and(|animated| !animated)
            })
    }
}

pub(super) enum FinalImageShortcut {
    Uniform([f32; 4]),
    Slot(usize),
    Kernel,
}

#[derive(Clone, Copy, Default)]
pub(super) struct FinalSlotDemand {
    pub(super) image: bool,
    pub(super) scene: bool,
}

impl<E: 'static> FinalGraphExecution<'_, E> {
    pub(super) fn image_shortcut(&self, node: &PointwiseNode<E>) -> FinalImageShortcut {
        let prepared = self.guarantees[&node.id];
        let guarantees = prepared.channels;
        if !prepared.needs_region_render
            && !guarantees.has_scene()
            && node.kind() != Some(PointwiseKind::Reference)
            && let [Some(r), Some(g), Some(b), Some(a)] = guarantees.constants()
        {
            FinalImageShortcut::Uniform([r, g, b, a])
        } else if let Some(index) = guarantees.passthrough_slot() {
            FinalImageShortcut::Slot(index)
        } else {
            FinalImageShortcut::Kernel
        }
    }

    pub(super) fn lookup(
        &mut self,
        node: &PointwiseNode<E>,
        geometry: ImageGeometry,
    ) -> Option<SharedImageMap> {
        if let Some(image) = self.images.get(&node.id) {
            return Some(image.clone());
        }
        let image = match &self.reuse {
            FinalImageReuse::None => None,
            FinalImageReuse::Original => {
                self.originals[&node.id].with_final_cache(|cache| cache.read(node.id, geometry))
            }
            FinalImageReuse::Playback(cache) => cache
                .images
                .get(&node.id)
                .filter(|image| image.geometry() == geometry)
                .map(SharedImageMap::share_pixel_backing),
        }?;
        self.images.insert(node.id, image.clone());
        Some(image)
    }

    pub(super) fn record(&mut self, node: &PointwiseNode<E>, image: &SharedImageMap) {
        self.images.insert(node.id, image.clone());
        match &mut self.reuse {
            FinalImageReuse::None => {}
            FinalImageReuse::Original => {
                let old = self.originals[&node.id]
                    .with_final_cache(|cache| cache.publish(node.id, image));
                drop(old);
            }
            FinalImageReuse::Playback(cache) => cache.record(node, image, &self.images),
        }
    }

    pub(super) fn record_scene(&mut self, node: &PointwiseNode<E>, scene: Option<Scene>) {
        self.scenes.insert(node.id, scene);
    }

    pub(super) fn scene(&self, node: &PointwiseNode<E>) -> Option<Option<Scene>> {
        self.scenes.get(&node.id).cloned()
    }

    pub(super) fn group_inputs_prepared(
        &mut self,
        node: &PointwiseNode<E>,
        demands: Vec<FinalSlotDemand>,
    ) {
        self.prepared_groups.insert(node.id, demands);
    }

    pub(super) fn reference_target(
        &self,
        node: &PointwiseNode<E>,
        time: f64,
        need_scene: bool,
    ) -> Result<Option<PointwiseNode<E>>, PointwiseError<E>> {
        // A prepared Group supplies its sampled exterior result, including
        // empty slots. A stand-alone Reference falls back to its saved slot.
        if let Some(group) = node.reference_group()?
            && let Some(demands) = self.prepared_groups.get(&group.id)
            && let Some(index) = node.reference_slot_at(Some(time))?
            && let Some(demand) = demands.get(index)
        {
            let target = group.slot(index as i32)?.node();
            let prepared = if need_scene {
                demand.scene
                    && target.as_ref().is_some_and(|target| {
                        self.scenes.get(&target.id).is_some_and(Option::is_some)
                    })
            } else {
                demand.image
                    || (demand.scene
                        && target
                            .as_ref()
                            .is_none_or(|target| self.images.contains_key(&target.id)))
            };
            if prepared {
                return Ok(target);
            }
        }
        node.reference_target()
    }

    /// Plan the exterior inputs reached from the requested Group contents.
    /// Keep image and Model demands separate when a slot serves both.
    pub(super) fn group_slot_demands(
        &self,
        group: &PointwiseNode<E>,
        need_scene: bool,
        geometry: ImageGeometry,
        time: f64,
        options: PointwiseRenderOptions,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<FinalSlotDemand>, PointwiseError<E>> {
        let mut demands = vec![FinalSlotDemand::default(); group.slots().len()];
        if !need_scene
            && group_skips_color(group, options)?
            && let Some(first) = demands.first_mut()
        {
            first.image = true;
        }
        // zl.i discovers References at input slots while walking Contents[0].
        // A Reference that is itself the contents root has no input slots.
        let mut pending: Vec<_> = group
            .content(0)?
            .node()
            .map(|child| (child, need_scene, false))
            .into_iter()
            .collect();
        let mut nested_groups = BTreeSet::from([group.id]);
        let mut visited = BTreeSet::new();
        while let Some((node, need_scene, from_slot)) = pending.pop() {
            check_render_cancelled(cancelled)?;
            if !visited.insert((node.id, need_scene, from_slot))
                || (need_scene && self.scenes.contains_key(&node.id))
                || (!need_scene && self.has_image(&node, geometry))
            {
                continue;
            }
            match self.image_shortcut(&node) {
                FinalImageShortcut::Uniform(_) => continue,
                FinalImageShortcut::Slot(index) => {
                    if let Some(child) = node.slot(index as i32)?.node() {
                        pending.push((child, need_scene, true));
                    }
                    continue;
                }
                FinalImageShortcut::Kernel => {}
            }
            match node.kind() {
                Some(PointwiseKind::Reference) => {
                    if !from_slot {
                        continue;
                    }
                    let Some(owner) = node.reference_group()? else {
                        continue;
                    };
                    if !nested_groups.contains(&owner.id) {
                        if let Some(child) = self.reference_target(&node, time, need_scene)? {
                            pending.push((child, need_scene, true));
                        }
                        continue;
                    }
                    let slots = owner.slots();
                    // zl.i reads ark.l (stored slot). zl.g checks the sample
                    // only when handing over a prepared exterior result.
                    let Some(index) = node
                        .reference_slot_at(None)?
                        .filter(|&index| index < slots.len())
                    else {
                        continue;
                    };
                    if owner.same_reference(group) {
                        let demand = &mut demands[index];
                        demand.scene |= need_scene;
                        demand.image |= !need_scene;
                    } else if let Some(child) = slots[index].node() {
                        pending.push((child, need_scene, true));
                    }
                }
                Some(kind) if kind.is_group() => {
                    nested_groups.insert(node.id);
                    let content = node.content(0)?.node();
                    let need_scene = need_scene && content.is_some();
                    if !need_scene
                        && group_skips_color(&node, options)?
                        && let Some(child) = node.slot(0)?.node()
                    {
                        pending.push((child, false, true));
                    }
                    pending.extend(content.map(|child| (child, need_scene, false)));
                }
                _ => {
                    let scene_slots = node.is_scene_node();
                    let uv_apply = node.uv_texture_output().is_some();
                    pending.extend(node.slots().into_iter().enumerate().filter_map(
                        |(index, slot)| {
                            slot.node().map(|child| {
                                (child, scene_slots && (!uv_apply || index == 0), true)
                            })
                        },
                    ));
                }
            }
        }
        Ok(demands)
    }

    fn has_image(&self, node: &PointwiseNode<E>, geometry: ImageGeometry) -> bool {
        // A playback hit still needs to enter this frame's normal Group
        // schedule so References see the same prepared-slot state.
        self.images.contains_key(&node.id)
            || (matches!(self.reuse, FinalImageReuse::Original)
                && self.originals[&node.id].with_final_cache(|cache| {
                    cache.recipient == Some(node.id)
                        && cache.image.as_ref().is_some_and(|image| {
                            !image.is_disposed() && image.geometry() == geometry
                        })
                }))
    }

    /// A final passthrough forwards the paired result. Otherwise zl.o uses
    /// the first contents scene, or ark.kv follows its prepared Group input.
    pub(super) fn scene_forward_input(
        &self,
        node: &PointwiseNode<E>,
        time: f64,
    ) -> Result<Option<PointwiseNode<E>>, PointwiseError<E>> {
        match self.image_shortcut(node) {
            FinalImageShortcut::Uniform(_) => Ok(None),
            FinalImageShortcut::Slot(index) => Ok(node.slot(index as i32)?.node()),
            FinalImageShortcut::Kernel => match node.kind() {
                Some(PointwiseKind::Reference) => {
                    let Some(target) = self.reference_target(node, time, true)? else {
                        return Ok(None);
                    };
                    if self.scene(&target).is_some()
                        || target.is_scene_node()
                        || target.scene_forward_target()?.is_some()
                    {
                        Ok(Some(target))
                    } else {
                        Ok(None)
                    }
                }
                _ => Ok(None),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        GradientGenerator, GradientGeneratorParameterKind::Location,
        PointwiseCachedRenderMode::Preview, PointwiseFilter, ProceduralExecution::CallingThread,
    };

    fn render(graph: &PointwiseFinalGraph, geometry: ImageGeometry, cache: bool) -> SharedImageMap {
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let outcome = graph
            .render_with_effects(
                geometry,
                0.375,
                CallingThread,
                PointwiseRenderOptions::new(false, false),
                cache,
                &mut effects,
            )
            .unwrap();
        assert!(outcome.caught_errors().is_empty());
        let image = outcome.image().unwrap();
        assert!(image.is_permanently_sealed());
        image
    }

    fn first_pixel(image: &SharedImageMap) -> f32 {
        image.channel(0, false).unwrap().read(0).unwrap()
    }

    fn assert_native_tone_matrix(stage: &str, image: &SharedImageMap) {
        assert_native_frame(
            include_str!("../../../../../port/evidence/gradient-tone-matrix-final-original.tsv"),
            stage,
            image,
        );
    }

    fn assert_native_frame(recording: &str, stage: &str, image: &SharedImageMap) {
        let mut fields = recording
            .lines()
            .map(|line| line.split('\t'))
            .find(|fields| fields.clone().next() == Some(stage))
            .unwrap();
        assert_eq!(fields.next(), Some(stage));
        assert_eq!(
            fields.next().unwrap().parse::<i32>().unwrap(),
            image.geometry().width()
        );
        assert_eq!(
            fields.next().unwrap().parse::<i32>().unwrap(),
            image.geometry().height()
        );
        let words = |values: &[f32]| {
            values
                .iter()
                .map(|value| format!("{:08x}", value.to_bits()))
                .collect::<Vec<_>>()
                .join(",")
        };
        for channel in 0..4 {
            let values = image.materialize(channel).unwrap().values();
            assert_eq!(
                Some(words(&values).as_str()),
                fields.next(),
                "{stage}, channel {channel}"
            );
        }
        for channel in image.named_channels() {
            let channel = channel.unwrap();
            let name = String::from_utf16(channel.name_utf16()).unwrap();
            let values = channel.data().unwrap().data(true).unwrap().values();
            let expected = fields.next().unwrap().split_once('=').unwrap();
            assert_eq!(name, expected.0, "{stage}, named channel");
            assert_eq!(words(&values), expected.1, "{stage}, named channel {name}");
        }
        assert!(fields.next().is_none());
    }

    #[test]
    fn final_clone_rebuilds_default_input_as_an_imported_image() {
        use crate::GradientGeneratorParameterKind::{ForceSquareRatio, Image};

        let default = PointwiseNode::<()>::default_input_prototype(0)
            .unwrap()
            .source_clone()
            .unwrap();
        let source = default.imported_image_handle().unwrap();
        source.parameter(ForceSquareRatio).set_index(0).unwrap();
        let graph = default.final_graph_clone().unwrap();
        assert!(default.is_default_input());
        assert!(!graph.selected.is_default_input());
        assert!(
            !default
                .retained_nodes()
                .iter()
                .any(|node| node.same_reference(&graph.selected))
        );
        let clone = graph.selected.imported_image_handle().unwrap();
        let source_image = source.parameter(Image).custom_value().unwrap().unwrap();
        let copy_property = clone.parameter(Image);
        let copy_image = copy_property.custom_value().unwrap().unwrap();
        let previous = copy_property.previous_custom_value().unwrap().unwrap();
        assert!(!copy_image.same_reference(&source_image));
        assert!(!copy_image.same_reference(&previous));
        for image in [source_image, copy_image] {
            assert!(image.imported_image().unwrap().bytes().unwrap().is_none());
        }
        let source_choice = source.parameter(ForceSquareRatio);
        let copy_choice = clone.parameter(ForceSquareRatio);
        assert_eq!(copy_choice.index().unwrap(), 0);
        assert!(
            !copy_choice
                .options()
                .unwrap()
                .same_reference(&source_choice.options().unwrap())
        );
        source_choice.set_index(1).unwrap();
        assert_eq!(copy_choice.index().unwrap(), 0);
    }

    #[test]
    fn playback_reuses_static_group_subgraph_and_resamples_animated_nodes() {
        use crate::{
            AnalyticCurveKind, CurveLayerHandle, GradientGeneratorParameterKind::Iterations,
        };

        let noise = GradientGenerator::<()>::with_kind(ImageGeneratorKind::Noise).unwrap();
        noise.parameter(Iterations).set_number(3.0).unwrap();
        let blur = PointwiseFilter::new(PointwiseKind::GaussianBlur).unwrap();
        let radius = blur.parameter(1).unwrap();
        radius.set_number(0.1).unwrap();
        blur.node()
            .slot(0)
            .unwrap()
            .set_node(Some(&noise.node()))
            .unwrap();
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap().node();
        group.slot(0).unwrap().set_node(Some(&blur.node())).unwrap();
        let reference = PointwiseFilter::new(PointwiseKind::Reference)
            .unwrap()
            .node();
        let invert = PointwiseNode::invert();
        invert.slot(0).unwrap().set_node(Some(&reference)).unwrap();
        invert.set_invert_red_only().unwrap();
        group.content(0).unwrap().set_node(Some(&invert)).unwrap();

        let animate = |animation: crate::GradientAnimation<()>, minimum, maximum| {
            animation.ensure_curve(0).unwrap();
            let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
            curve.set_minimum(minimum);
            curve.set_maximum(maximum);
            curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
            animation.flags().unwrap().set(0, true).unwrap();
        };
        let red = invert.invert_property(0).unwrap();
        animate(red.animation().unwrap().unwrap(), 1.0, 2.0);

        let geometry = ImageGeometry::new(32, 24);
        let options = PointwiseRenderOptions::new(false, false);
        let run = || {
            let snapshot = RenderCaptureSession::new()
                .capture_node(&group, 0.0)
                .unwrap();
            let mut capacity = |_| unreachable!();
            let mut preflights = 0;
            let mut preflight = |_, _| {
                preflights += 1;
                Ok(())
            };
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            let playback = snapshot
                .render_export_playback_with_effects(geometry, CallingThread, options, &mut effects)
                .unwrap()
                .output()
                .unwrap();
            let playback_preflights = preflights;
            let graph = group.final_graph_clone().unwrap();
            let mut still_preflights = 0;
            let mut preflight = |_, _| {
                still_preflights += 1;
                Ok(())
            };
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            let frames = playback.frames();
            assert_eq!(frames.len(), 32);
            for (index, frame) in frames.iter().enumerate() {
                let time = index as f64 / frames.len() as f64;
                let still = graph
                    .render_with_effects(
                        geometry,
                        time,
                        CallingThread,
                        options,
                        false,
                        &mut effects,
                    )
                    .unwrap()
                    .image()
                    .unwrap();
                for ch in 0..4 {
                    assert_eq!(
                        frame.materialize(ch).unwrap().values(),
                        still.materialize(ch).unwrap().values(),
                        "frame {index}, channel {ch}"
                    );
                }
            }
            assert!(playback_preflights > 0);
            assert!(!frames[0].same_reference(&frames[1]));
            assert_ne!(first_pixel(&frames[0]), first_pixel(&frames[31]));
            (playback_preflights, still_preflights, frames.len())
        };

        let (playback, still, frames) = run();
        assert_eq!(playback * frames, still);

        let radius_animation = radius.animation().unwrap();
        animate(radius_animation.clone(), 0.04, 0.12);
        let (animated_blur, still, _) = run();
        assert!(animated_blur > playback);
        assert!(animated_blur < still);

        radius_animation.flags().unwrap().set(0, false).unwrap();
        animate(noise.parameter(Location).animation().unwrap(), 0.0, 0.5);
        let (animated_noise, still, _) = run();
        assert_eq!(animated_noise, still);
    }

    #[test]
    fn selected_final_clone_uses_real_group_input_and_original_cache_recipients() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        let group = PointwiseFilter::new(PointwiseKind::WizardGroup).unwrap();
        group.parameter(0).unwrap().set_number(1.).unwrap();
        group
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let reference = PointwiseFilter::new(PointwiseKind::Reference)
            .unwrap()
            .node();
        let selected = PointwiseNode::invert();
        selected
            .slot(0)
            .unwrap()
            .set_node(Some(&reference))
            .unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&selected))
            .unwrap();
        let preview = gradient
            .render_cached(Preview, 2, 2, 0., CallingThread)
            .unwrap()
            .image()
            .unwrap();
        gradient.set_should_final_cache(true);
        selected.set_should_final_cache(true);

        let geometry = ImageGeometry::new(5, 3);
        let first = selected.final_graph_clone().unwrap();
        let image = render(&first, geometry, true);
        assert_eq!(first_pixel(&image), 0.75);
        assert!(image.same_reference(&selected.cached_final_image().unwrap()));
        let child_image = gradient.cached_final_image().unwrap();
        assert_eq!(child_image.geometry(), geometry);
        assert!(!child_image.same_reference(&preview));
        assert!(render(&first, geometry, true).same_reference(&image));
        assert!(!render(&first, geometry, false).same_reference(&image));

        let latest = selected.final_graph_clone().unwrap();
        assert!(!render(&first, geometry, true).same_reference(&image));
        assert!(render(&latest, geometry, true).same_reference(&image));
        selected.set_final_cache_destruct_mode(true);
        assert!(render(&latest, geometry, true).same_reference(&image));
        assert!(selected.cached_final_image().is_none());

        gradient.parameter(Location).set_axis(0, 0.5).unwrap();
        assert!(gradient.cached_final_image().is_none());
        assert!(selected.cached_final_image().is_none());
        assert_eq!(first_pixel(&render(&latest, geometry, true)), 0.75);
        assert!(selected.cached_final_image().is_none());
        let fresh = selected.final_graph_clone().unwrap();
        assert_eq!(first_pixel(&render(&fresh, geometry, true)), 0.5);
        assert!(selected.cached_final_image().is_some());
    }

    #[test]
    fn automatic_final_cache_spacing_honors_group_passthrough_and_chain_lengths() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        let mut chain = vec![gradient.node()];
        for _ in 0..9 {
            let invert = PointwiseNode::invert();
            invert.slot(0).unwrap().set_node(chain.last()).unwrap();
            chain.push(invert);
        }
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap().node();
        group.slot(0).unwrap().set_node(chain.last()).unwrap();
        let reference = PointwiseFilter::new(PointwiseKind::Reference)
            .unwrap()
            .node();
        group
            .content(0)
            .unwrap()
            .set_node(Some(&reference))
            .unwrap();
        let graph = group.final_graph_clone().unwrap();
        assert_eq!(graph.prepare_automatic_caches(0.).unwrap(), 2);
        let image = render(&graph, ImageGeometry::new(5, 2), true);
        assert_eq!(first_pixel(&image), 1.);
        let indexes: Vec<_> = chain
            .iter()
            .enumerate()
            .filter_map(|(index, node)| node.cached_final_image().is_some().then_some(index))
            .collect();
        assert_eq!(indexes, [3, 6]);
        assert!(group.cached_final_image().is_none());
        assert!(reference.cached_final_image().is_none());
    }

    #[test]
    fn still_host_replaces_excess_cache_placements_and_releases_unread_images() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        let mut nodes = vec![gradient.node()];
        for _ in 0..15 {
            let invert = PointwiseNode::invert();
            invert.slot(0).unwrap().set_node(nodes.last()).unwrap();
            nodes.push(invert);
        }
        let root = nodes.last().unwrap();
        let graph = root.final_graph_clone().unwrap();
        let geometry = ImageGeometry::new(7, 3);
        let options = PointwiseRenderOptions::new(false, true);
        let mut policy = StillExportCachePolicy::default();
        assert!(
            graph
                .prepare_still_export_caches(geometry, options, &mut policy, &|| false)
                .unwrap()
        );

        for node in &nodes {
            node.set_should_final_cache(true);
        }
        let result = render(&graph, geometry, true);
        assert!(root.cached_final_image().unwrap().same_reference(&result));
        assert!(
            graph
                .prepare_still_export_caches(geometry, options, &mut policy, &|| false)
                .unwrap()
        );
        assert!(root.with_final_cache(|cache| cache.destruct_mode && !cache.enabled));
        assert!(root.cached_final_image().is_some());
        graph.retire_destructive_caches();
        assert!(root.cached_final_image().is_none());
        assert!(nodes[3].cached_final_image().is_some());
        assert!(
            graph
                .prepare_still_export_caches(
                    ImageGeometry::new(7, 2100),
                    options,
                    &mut policy,
                    &|| false
                )
                .unwrap()
        );
        assert!(nodes[3].cached_final_image().is_none());
        assert!(nodes[3].with_final_cache(|cache| cache.enabled));
        assert!(
            !graph
                .prepare_still_export_caches(
                    ImageGeometry::new(7, 2101),
                    options,
                    &mut policy,
                    &|| false
                )
                .unwrap()
        );
        assert!(
            nodes
                .iter()
                .all(|node| node.with_final_cache(|cache| !cache.enabled))
        );
    }

    #[test]
    fn final_shared_gradient_copies_each_slot_and_keeps_latest_cache_recipient() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        gradient.set_should_final_cache(true);
        let source = gradient.node();
        let combine = crate::SceneCombineNode::new().node();
        for index in 0..2 {
            combine
                .slot(index)
                .unwrap()
                .set_node(Some(&source))
                .unwrap();
        }

        let mut graph = source.final_graph_clone().unwrap();
        let first = graph.tree.slot(0).unwrap().node().unwrap();
        let last = graph.tree.slot(1).unwrap().node().unwrap();
        assert!(!first.same_reference(&last));
        assert!(first.parent().unwrap().same_reference(&graph.tree));
        assert!(last.parent().unwrap().same_reference(&graph.tree));
        assert!(graph.selected.same_reference(&last));
        first
            .gradient_generator_handle()
            .unwrap()
            .parameter(Location)
            .set_axis(0, 0.5)
            .unwrap();

        let geometry = ImageGeometry::new(5, 3);
        let cached = render(&graph, geometry, true);
        assert_eq!(first_pixel(&cached), 0.25);
        assert!(
            gradient
                .cached_final_image()
                .unwrap()
                .same_reference(&cached)
        );
        graph.selected = first;
        let earlier = render(&graph, geometry, true);
        assert_eq!(first_pixel(&earlier), 0.5);
        assert!(!earlier.same_reference(&cached));
        assert!(
            gradient
                .cached_final_image()
                .unwrap()
                .same_reference(&cached)
        );
    }

    #[test]
    fn final_shared_gradient_uses_each_slot_parent_after_reparenting() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        gradient.set_should_final_cache(true);
        let source = gradient.node();
        let combine = crate::SceneCombineNode::new().node();
        let former_first = combine.slot(0).unwrap();
        let direct_slot = combine.slot(1).unwrap();
        former_first.set_node(Some(&source)).unwrap();
        direct_slot.set_node(Some(&source)).unwrap();

        let invert = PointwiseNode::invert();
        invert.slot(0).unwrap().set_node(Some(&source)).unwrap();
        assert!(former_first.node().is_none());
        assert!(direct_slot.node().unwrap().same_reference(&source));
        former_first.set_node(Some(&invert)).unwrap();
        assert!(source.parent().unwrap().same_reference(&invert));

        let mut graph = source.final_graph_clone().unwrap();
        let invert_copy = graph.tree.slot(0).unwrap().node().unwrap();
        let nested = invert_copy.slot(0).unwrap().node().unwrap();
        let direct = graph.tree.slot(1).unwrap().node().unwrap();
        assert!(!nested.same_reference(&direct));
        assert!(nested.parent().unwrap().same_reference(&invert_copy));
        assert!(direct.parent().unwrap().same_reference(&graph.tree));
        assert!(graph.selected.same_reference(&direct));

        nested
            .gradient_generator_handle()
            .unwrap()
            .parameter(Location)
            .set_axis(0, 0.5)
            .unwrap();
        let geometry = ImageGeometry::new(5, 3);
        let cached = render(&graph, geometry, true);
        assert_eq!(first_pixel(&cached), 0.25);
        graph.selected = nested;
        assert_eq!(first_pixel(&render(&graph, geometry, true)), 0.5);
        assert!(
            gradient
                .cached_final_image()
                .unwrap()
                .same_reference(&cached)
        );
    }

    #[test]
    fn export_cache_keeps_shared_occurrences_distinct_across_frozen_graphs() {
        let source = GradientGenerator::<()>::new().unwrap().node();
        let combine = crate::SceneCombineNode::new().node();
        for index in 0..2 {
            combine
                .slot(index)
                .unwrap()
                .set_node(Some(&source))
                .unwrap();
        }
        let freeze = || {
            let copy = RenderCaptureSession::new().copy_node_raw(&source).unwrap();
            let origins = BTreeMap::from([
                (copy.id, source.id),
                (copy.parent().unwrap().id, combine.id),
            ]);
            let graph = copy.final_graph_clone().unwrap();
            let cache = graph.export_cache_tree(&origins, 0., &|| false).unwrap();
            (copy, graph, cache)
        };
        let (old_source, old_graph, old_cache) = freeze();
        old_source.set_should_final_cache(true);
        let geometry = ImageGeometry::new(5, 3);
        let cached = render(&old_graph, geometry, true);
        let guarantees = prepare_guarantees(&old_graph.tree, &|| false).unwrap();
        assert_eq!(
            old_graph
                .count_cache_candidates(&guarantees, &|| false)
                .unwrap(),
            (3, 2)
        );
        let (_, mut graph, next_cache) = freeze();
        next_cache
            .inherit(&old_cache, true, true, &|| false)
            .unwrap();
        assert!(render(&graph, geometry, true).same_reference(&cached));
        graph.selected = graph.tree.slot(0).unwrap().node().unwrap();
        assert!(!render(&graph, geometry, true).same_reference(&cached));
    }

    #[test]
    fn final_group_prepares_image_and_model_slots_before_only_its_first_contents() {
        #[derive(Default)]
        struct Materials(Vec<Vec<f32>>);
        impl crate::SceneImageRenderer<()> for Materials {
            fn preflight_array(
                &mut self,
                _: crate::SceneAllocationRequest,
            ) -> Result<(), crate::SceneError> {
                Ok(())
            }
            fn material_texture_limit(&mut self) -> Result<u32, PointwiseError<()>> {
                Ok(4096)
            }
            fn render_scene_image(
                &mut self,
                scene: &Scene,
                width: i32,
                height: i32,
            ) -> Result<SharedImageMap, PointwiseError<()>> {
                let values: Vec<_> = scene
                    .models()
                    .iter()
                    .map(|model| first_pixel(&model.material().map().unwrap()))
                    .collect();
                let image = SharedImageMap::new(ImageGeometry::new(width, height));
                image.set_constant(0, values[0])?;
                self.0.push(values);
                Ok(image)
            }
        }

        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        group.parameter(0).unwrap().set_number(3.).unwrap();
        let group = group.node();
        for (index, color) in [0.125, 0.625, 0.875].into_iter().enumerate() {
            let image = GradientGenerator::new().unwrap();
            image.parameter(Location).set_axis(0, color).unwrap();
            if index == 2 {
                image
                    .parameter(crate::GradientGeneratorParameterKind::Mode)
                    .set_index(99)
                    .unwrap();
            }
            let scene = crate::SceneClearLightsNode::new().node();
            scene
                .slot(0)
                .unwrap()
                .set_node(Some(&image.node()))
                .unwrap();
            group
                .slot(index as i32)
                .unwrap()
                .set_node(Some(&scene))
                .unwrap();
        }
        let reference = |number| {
            let node = PointwiseFilter::new(PointwiseKind::Reference).unwrap();
            node.parameter(0).unwrap().set_number(number).unwrap();
            node.node()
        };
        let invert = |node: &PointwiseNode<()>| {
            let filter = PointwiseNode::invert();
            filter.slot(0).unwrap().set_node(Some(node)).unwrap();
            filter
        };
        let combine = |first: &PointwiseNode<()>, second: &PointwiseNode<()>| {
            let node = crate::SceneCombineNode::new().node();
            node.slot(0).unwrap().set_node(Some(first)).unwrap();
            node.slot(1).unwrap().set_node(Some(second)).unwrap();
            node
        };
        let animated = reference(1.);
        let selector = animated.filter().unwrap().parameter(0).unwrap();
        selector.set_axis_animated(0, true).unwrap();
        let curve = selector.animation_curve(0).unwrap();
        curve.set_minimum(2.);
        curve.set_maximum(2.);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        // The second image input independently demands the sampled slot.
        let images = combine(&invert(&animated), &invert(&reference(2.)));
        let models = combine(&reference(2.), &reference(1.));
        let primary = combine(&images, &models);
        group.content(0).unwrap().set_node(Some(&primary)).unwrap();
        // Rendering this auxiliary branch would fail before the primary contents.
        let extra = GradientGenerator::new().unwrap();
        extra
            .parameter(crate::GradientGeneratorParameterKind::Mode)
            .set_index(99)
            .unwrap();
        group
            .append_content(Vec::new())
            .set_node(Some(&extra.node()))
            .unwrap();

        let render = |node: &PointwiseNode<()>| {
            let graph = node.final_graph_clone().unwrap();
            let mut backend = Materials::default();
            let mut capacity = |_| unreachable!();
            let mut preflight = |_, _| unreachable!();
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
                .with_scene_renderer(&mut backend);
            let outcome = graph
                .render_with_effects(
                    ImageGeometry::new(5, 3),
                    0.5,
                    CallingThread,
                    PointwiseRenderOptions::new(false, true),
                    false,
                    &mut effects,
                )
                .unwrap();
            assert!(outcome.caught_errors().is_empty());
            assert_eq!(
                first_pixel(&outcome.image().unwrap()),
                backend.0.last().unwrap()[0]
            );
            backend.0
        };
        let expected = [vec![0.125], vec![0.625], vec![0.375, 0.375, 0.625, 0.125]];
        assert_eq!(render(&group), expected);
        let parent = crate::SceneClearLightsNode::new().node();
        parent.slot(0).unwrap().set_node(Some(&group)).unwrap();
        assert_eq!(render(&parent), expected);

        // The same demand still hands an empty image to the animated Reference
        // when the slot is disconnected. Its saved source remains nonempty.
        group.slot(1).unwrap().set_node(None).unwrap();
        assert_eq!(render(&parent), [vec![0.125], vec![1., 1., 0., 0.125]]);
    }

    #[test]
    fn final_reference_falls_back_to_the_saved_slot_when_its_sample_is_unprepared() {
        use crate::{AnalyticCurveKind, CurveLayerHandle};
        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        group.parameter(0).unwrap().set_number(2.).unwrap();
        let group = group.node();
        group
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let reference = PointwiseFilter::new(PointwiseKind::Reference).unwrap();
        let slot = reference.parameter(0).unwrap();
        let animation = slot.animation().unwrap();
        animation.ensure_curve(0).unwrap();
        let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
        curve.set_minimum(2.);
        curve.set_maximum(2.);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        animation.flags().unwrap().set(0, true).unwrap();
        assert_eq!(slot.number(), 1.);
        let invert = PointwiseNode::invert();
        invert
            .slot(0)
            .unwrap()
            .set_node(Some(&reference.node()))
            .unwrap();
        group.content(0).unwrap().set_node(Some(&invert)).unwrap();
        let geometry = ImageGeometry::new(5, 2);
        let graph = group.final_graph_clone().unwrap();
        assert_eq!(first_pixel(&render(&graph, geometry, false)), 0.75);

        let other = GradientGenerator::new().unwrap();
        other
            .parameter(crate::GradientGeneratorParameterKind::Mode)
            .set_index(99)
            .unwrap();
        group
            .slot(1)
            .unwrap()
            .set_node(Some(&other.node()))
            .unwrap();
        let graph = group.final_graph_clone().unwrap();
        assert_eq!(first_pixel(&render(&graph, geometry, false)), 0.75);

        invert.slot(0).unwrap().set_node(None).unwrap();
        group
            .content(0)
            .unwrap()
            .set_node(Some(&reference.node()))
            .unwrap();
        let graph = group.final_graph_clone().unwrap();
        assert_eq!(first_pixel(&render(&graph, geometry, false)), 0.25);
    }

    #[test]
    fn remap_keeps_its_original_chain_on_the_final_evaluator() {
        let input = GradientGenerator::<()>::new().unwrap();
        let hsb = PointwiseFilter::new(PointwiseKind::ChangeHsb)
            .unwrap()
            .node();
        hsb.slot(0).unwrap().set_node(Some(&input.node())).unwrap();
        let remap = PointwiseFilter::new(PointwiseKind::Remap).unwrap().node();
        remap.slot(0).unwrap().set_node(Some(&hsb)).unwrap();
        for index in [1, 2] {
            let coordinate = GradientGenerator::new().unwrap();
            remap
                .slot(index)
                .unwrap()
                .set_node(Some(&coordinate.node()))
                .unwrap();
        }

        hsb.set_should_final_cache(true);
        input.node().set_should_final_cache(true);
        let graph = remap.final_graph_clone().unwrap();
        let output = render(&graph, ImageGeometry::new(8, 6), true);
        let red = output.channel(0, false).unwrap().values();
        assert!(red.iter().any(|sample| *sample != red[0]));
        let source = input.node().cached_final_image().unwrap();
        assert!(hsb.cached_final_image().unwrap().same_reference(&source));
    }

    #[test]
    fn directional_blurs_keep_the_upstream_change_hsb_final_shortcut() {
        use crate::{RenderCaptureSession, RgbColor};

        for kind in [
            PointwiseKind::MotionBlur,
            PointwiseKind::ShapeBlur,
            PointwiseKind::Smear,
            PointwiseKind::VariableBlur,
        ] {
            let noise = crate::create_node_by_identity::<()>(Some(
                &"Noise".encode_utf16().collect::<Vec<_>>(),
            ))
            .unwrap();
            let colorize = PointwiseFilter::new(PointwiseKind::Colorize).unwrap();
            let gradient = colorize.parameter(0).unwrap().gradient().unwrap();
            gradient
                .set_color_at(0, RgbColor::new(0.2, 0.45, 0.8, 1.))
                .unwrap();
            gradient
                .set_color_at(1, RgbColor::new(0.9, 0.39, 0.18, 1.))
                .unwrap();
            let colorize = colorize.node();
            colorize.slot(0).unwrap().set_node(Some(&noise)).unwrap();
            let change = PointwiseFilter::new(PointwiseKind::ChangeHsb)
                .unwrap()
                .node();
            let blur = PointwiseFilter::new(kind).unwrap().node();
            if kind == PointwiseKind::Smear {
                let distance = PointwiseNode::solid_color().unwrap();
                distance.set_solid_color([0.5, 0., 0.]).unwrap();
                blur.slot(2).unwrap().set_node(Some(&distance)).unwrap();
            }
            let geometry = ImageGeometry::new(24, 16);
            let render = || {
                let mut capacity = |_| Ok(true);
                let mut preflight = |_, _| Ok(());
                let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
                let outcome = RenderCaptureSession::new()
                    .capture_node(&blur, 0.)
                    .unwrap()
                    .render_export_with_effects(
                        geometry,
                        CallingThread,
                        PointwiseRenderOptions::new(false, true),
                        &mut effects,
                    )
                    .unwrap();
                assert!(outcome.caught_errors().is_empty());
                outcome.image().unwrap()
            };
            let input = blur.slot(0).unwrap();
            input.set_node(Some(&colorize)).unwrap();
            let direct = render();
            input.set_node(Some(&change)).unwrap();
            change.slot(0).unwrap().set_node(Some(&colorize)).unwrap();
            let through_hsb = render();
            for index in 0..4 {
                assert_eq!(
                    direct.materialize(index).unwrap().values(),
                    through_hsb.materialize(index).unwrap().values()
                );
            }
        }
    }

    #[test]
    fn remap_renders_scene_coordinates_even_with_a_uniform_original() {
        #[derive(Default)]
        struct CoordinateRasters(Vec<f32>);

        impl crate::SceneImageRenderer<()> for CoordinateRasters {
            fn preflight_array(
                &mut self,
                _: crate::SceneAllocationRequest,
            ) -> Result<(), crate::SceneError> {
                Ok(())
            }

            fn material_texture_limit(&mut self) -> Result<u32, PointwiseError<()>> {
                Ok(4096)
            }

            fn render_scene_image(
                &mut self,
                scene: &Scene,
                width: i32,
                height: i32,
            ) -> Result<SharedImageMap, PointwiseError<()>> {
                let coordinate = first_pixel(&scene.models()[0].material().map().unwrap());
                self.0.push(coordinate);
                let image = SharedImageMap::new(ImageGeometry::new(width, height));
                image.set_constant(0, coordinate)?;
                Ok(image)
            }
        }

        let remap = PointwiseFilter::new(PointwiseKind::Remap).unwrap().node();
        let original = GradientGenerator::<()>::new().unwrap().node();
        remap.slot(0).unwrap().set_node(Some(&original)).unwrap();
        for (slot, red) in [(1, 0.5), (2, 0.25)] {
            let color = PointwiseNode::solid_color().unwrap();
            color.set_solid_color([red, 0., 0.]).unwrap();
            let scene = crate::SceneClearLightsNode::new().node();
            scene.slot(0).unwrap().set_node(Some(&color)).unwrap();
            remap.slot(slot).unwrap().set_node(Some(&scene)).unwrap();
        }

        let geometry = ImageGeometry::new(8, 4);
        let source = render(&original.final_graph_clone().unwrap(), geometry, false);
        let expected = source.channel(0, false).unwrap().read(12).unwrap();
        let render_remap = || {
            let mut backend = CoordinateRasters::default();
            let mut capacity = |_| unreachable!();
            let mut preflight = |_, _| unreachable!();
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
                .with_scene_renderer(&mut backend);
            let outcome = remap
                .final_graph_clone()
                .unwrap()
                .render_with_effects(
                    geometry,
                    0.375,
                    CallingThread,
                    PointwiseRenderOptions::new(false, false),
                    false,
                    &mut effects,
                )
                .unwrap();
            assert!(outcome.caught_errors().is_empty());
            assert_eq!(backend.0, [0.5, 0.25]);
            outcome.image().unwrap()
        };
        let red = render_remap().materialize(0).unwrap().values();
        assert_eq!(red, vec![expected; 32]);

        let solid = PointwiseNode::solid_color().unwrap();
        solid.set_solid_color([0.375, 0., 0.]).unwrap();
        remap.slot(0).unwrap().set_node(Some(&solid)).unwrap();
        assert_eq!(first_pixel(&render_remap()), 0.375);
    }

    #[test]
    fn final_color_filters_clone_the_ramp_and_sample_hue_at_render_time() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        let colorize = PointwiseFilter::new(PointwiseKind::Colorize).unwrap();
        let ramp = colorize.parameter(0).unwrap().gradient().unwrap();
        ramp.clear_points().unwrap();
        ramp.add_normalized_color(0., 0.25, 0., 0.).unwrap();
        ramp.add_normalized_color(1., 1., 0., 0.).unwrap();
        colorize
            .parameter(1)
            .unwrap()
            .options()
            .unwrap()
            .set_entry(0, None)
            .unwrap();
        let color = colorize.node();
        color
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        color.set_should_final_cache(true);
        let hsb = PointwiseFilter::new(PointwiseKind::ChangeHsb).unwrap();
        let node = hsb.node();
        node.slot(0).unwrap().set_node(Some(&color)).unwrap();
        let geometry = ImageGeometry::new(5, 3);
        let still = node.final_graph_clone().unwrap();
        let cloned_color = still
            .selected
            .slot(0)
            .unwrap()
            .node()
            .unwrap()
            .filter()
            .unwrap();
        assert!(
            !cloned_color
                .parameter(0)
                .unwrap()
                .gradient()
                .unwrap()
                .same_reference(&ramp)
        );
        assert_eq!(
            cloned_color.parameter(1).unwrap().text().unwrap(),
            Some("Yes".encode_utf16().collect())
        );
        let unchanged = render(&still, geometry, true);
        let original_red = unchanged.channel(0, false).unwrap().values();
        assert!(original_red.iter().all(|red| (0.25..=1.0).contains(red)));
        assert!(unchanged.same_reference(&color.cached_final_image().unwrap()));

        let hue = hsb.parameter(0).unwrap();
        let animation = hue.animation().unwrap();
        animation.ensure_curve(0).unwrap();
        let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
        curve.set_minimum(0.);
        curve.set_maximum(320.);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        animation.flags().unwrap().set(0, true).unwrap();
        let frame = render(&node.final_graph_clone().unwrap(), geometry, false);
        assert!(
            frame
                .channel(0, false)
                .unwrap()
                .values()
                .iter()
                .all(|red| red.abs() < 1e-6)
        );
        let green = frame.channel(1, false).unwrap().values();
        assert_eq!(green, original_red);
        assert_eq!(hue.number(), 0.);
    }

    #[test]
    fn change_hsb_brightens_black_to_neutral_gray() {
        let filter = PointwiseFilter::<()>::new(PointwiseKind::ChangeHsb).unwrap();
        filter.parameter(0).unwrap().set_number(90.).unwrap();
        filter.parameter(1).unwrap().set_number(100.).unwrap();
        filter.parameter(2).unwrap().set_number(50.).unwrap();
        let node = filter.node();
        let geometry = ImageGeometry::new(3, 2);
        let assert_frame = |transparency| {
            let output = render(&node.final_graph_clone().unwrap(), geometry, false);
            for channel in 0..3 {
                assert_eq!(output.materialize(channel).unwrap().values(), [0.5; 6]);
            }
            assert_eq!(output.materialize(3).unwrap().values(), [transparency; 6]);
        };
        assert_frame(0.0);

        let black = PointwiseNode::solid_color().unwrap();
        black.set_solid_color([0.; 3]).unwrap();
        black.set_solid_transparency(0.625).unwrap();
        node.slot(0).unwrap().set_node(Some(&black)).unwrap();
        assert_frame(0.625);
    }

    #[test]
    fn final_levels_and_get_channel_use_owned_settings_at_the_sample_time() {
        use crate::{AnalyticCurveKind, CurveLayerHandle};

        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        gradient.set_should_final_cache(true);
        let channel = PointwiseFilter::new(PointwiseKind::GetChannel).unwrap();
        let root = channel.node();
        root.slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let geometry = ImageGeometry::new(5, 2);
        let red = render(&root.final_graph_clone().unwrap(), geometry, true);
        assert!(red.same_reference(&gradient.cached_final_image().unwrap()));

        let levels = PointwiseFilter::new(PointwiseKind::Levels).unwrap();
        let histogram = levels.parameter(0).unwrap().histogram().unwrap();
        histogram.set_channel(1);
        histogram.set_component(1, 0.25).unwrap();
        histogram.set_component(2, 0.5).unwrap();
        histogram.set_disabled(true);
        root.slot(0)
            .unwrap()
            .set_node(Some(&levels.node()))
            .unwrap();
        levels
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();

        let selection = channel.parameter(0).unwrap();
        selection.set_index(1).unwrap();
        selection.options().unwrap().set_entry(6, None).unwrap();
        selection.set_axis_animated(0, true).unwrap();
        let curve = selection.animation_curve(0).unwrap();
        curve.set_minimum(6.25);
        curve.set_maximum(8.25);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);

        let frozen = root.final_graph_clone().unwrap();
        let cloned_histogram = frozen
            .selected
            .slot(0)
            .unwrap()
            .node()
            .unwrap()
            .filter()
            .unwrap()
            .parameter(0)
            .unwrap()
            .histogram()
            .unwrap();
        assert!(!cloned_histogram.same_reference(&histogram));
        assert_eq!(cloned_histogram.selected_channel(), 1);
        assert!(!cloned_histogram.is_disabled());
        histogram.arrays()[1].write(1, 0.5).unwrap();
        histogram.arrays()[2].write(1, 1.).unwrap();

        let old = render(&frozen, geometry, false);
        assert_eq!(first_pixel(&old), 0.5);
        for ch in 1..4 {
            assert_eq!(old.channel(ch, false).unwrap().values(), [0.]);
        }
        let fresh = render(&root.final_graph_clone().unwrap(), geometry, false);
        assert_eq!(first_pixel(&fresh), 0.25);
        assert_eq!(selection.index(), 1);
    }

    #[test]
    fn get_channel_from_black_produces_a_usable_hue() {
        let black = PointwiseNode::<()>::solid_color().unwrap();
        black.set_solid_color([0.; 3]).unwrap();
        let hue = PointwiseFilter::new(PointwiseKind::GetChannel).unwrap();
        hue.parameter(0).unwrap().set_index(4).unwrap();
        hue.node().slot(0).unwrap().set_node(Some(&black)).unwrap();
        let geometry = ImageGeometry::new(5, 3);
        let extracted = render(&hue.node().final_graph_clone().unwrap(), geometry, false);
        assert_eq!(extracted.channel(0, false).unwrap().values(), [0.]);

        let color = PointwiseFilter::new(PointwiseKind::FromHsb).unwrap().node();
        color.slot(0).unwrap().set_node(Some(&hue.node())).unwrap();
        for slot in [1, 2] {
            let full = PointwiseNode::solid_color().unwrap();
            full.set_solid_color([1., 0., 0.]).unwrap();
            color.slot(slot).unwrap().set_node(Some(&full)).unwrap();
        }
        let extracted = render(&color.final_graph_clone().unwrap(), geometry, false);
        for (index, expected) in [1., 0., 0., 0.].into_iter().enumerate() {
            assert!(
                extracted
                    .channel(index as i32, false)
                    .unwrap()
                    .values()
                    .into_iter()
                    .all(|value| value == expected)
            );
        }
    }

    #[test]
    fn nongradient_export_and_preview_sample_native_invert_choices() {
        use crate::{AnalyticCurveKind, CurveLayerHandle, PointwiseFinalExportCache};
        let recording =
            include_str!("../../../../../port/evidence/solid-invert-final-original.tsv");
        let solid = PointwiseNode::<()>::solid_color().unwrap();
        solid.set_solid_color([0.25, 0.5, 0.75]).unwrap();
        solid.set_solid_transparency(0.125).unwrap();
        let invert = PointwiseNode::invert();
        invert.slot(0).unwrap().set_node(Some(&solid)).unwrap();
        invert.set_invert_red_only().unwrap();
        let red = invert.invert_property(0).unwrap();
        red.set_index(1).unwrap();
        let animation = red.animation().unwrap().unwrap();
        animation.ensure_curve(0).unwrap();
        let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
        curve.set_minimum(1.0);
        curve.set_maximum(2.0);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        animation.flags().unwrap().set(0, true).unwrap();

        let floating = invert
            .render_cached_graph(
                crate::PointwiseCachedRenderMode::Floating,
                5,
                3,
                0.375,
                CallingThread,
            )
            .unwrap();
        assert!(floating.caught_errors().is_empty());
        assert_native_frame(recording, "animated-red", &floating.image().unwrap());
        let preview = invert
            .render_cached_graph(Preview, 5, 3, 0.375, CallingThread)
            .unwrap();
        assert!(preview.caught_errors().is_empty());
        let preview = preview.output().unwrap();
        assert_eq!(preview.frames().len(), 32);
        for (stage, time) in [
            ("animated-red", 0.375),
            ("tie-to-even", 0.5),
            ("disabled-red", 0.75),
        ] {
            assert_native_frame(recording, stage, &preview.frame_at_time(time).unwrap());
        }
        assert_eq!(red.current().unwrap(), 1);
        invert.set_cached_image(None).unwrap();

        // xe's constructor restores this label on a final clone.
        red.options()
            .unwrap()
            .set_entry(0, Some("No".encode_utf16().collect()))
            .unwrap();
        let snapshot = RenderCaptureSession::new()
            .capture_node(&invert, 0.375)
            .unwrap();
        let frozen = invert.final_graph_clone().unwrap();
        let geometry = ImageGeometry::new(5, 3);
        let options = PointwiseRenderOptions::new(false, true);
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let exported = PointwiseFinalExportCache::default()
            .render_snapshot_with_effects(&snapshot, geometry, CallingThread, options, &mut effects)
            .unwrap();
        assert!(exported.caught_errors().is_empty());
        assert_native_frame(recording, "animated-red", &exported.image().unwrap());
        let exported = snapshot
            .render_export_with_effects(geometry, CallingThread, options, &mut effects)
            .unwrap();
        assert!(exported.caught_errors().is_empty());
        assert_native_frame(recording, "animated-red", &exported.image().unwrap());

        curve.set_maximum(1.0);
        let mut render = |graph: &PointwiseFinalGraph| {
            let result = graph
                .render_with_effects(geometry, 0.75, CallingThread, options, false, &mut effects)
                .unwrap();
            assert!(result.caught_errors().is_empty());
            result.image().unwrap()
        };
        assert_native_frame(recording, "frozen-red", &render(&frozen));
        assert_native_frame(
            recording,
            "fresh-red",
            &render(&invert.final_graph_clone().unwrap()),
        );
    }

    #[test]
    fn solid_animation_matches_original_timed_frames_and_frozen_exports() {
        use crate::{
            AnalyticCurveKind, CurveLayerHandle, LimitsEdit, PointwiseFinalExportCache,
            SharedSolidColorNode, SolidColorCloneMode,
        };

        let recording =
            include_str!("../../../../../port/evidence/solid-animation-final-original.tsv");
        let solid =
            SharedSolidColorNode::<()>::new_unattached(SolidColorCloneMode::Original).unwrap();
        let color = solid.color_property();
        color.set_color([0.125, 0.375, 0.625]).unwrap();
        let transparency = solid.transparency_property();
        transparency.set_transparency(0.375).unwrap();
        let node = PointwiseNode::publish_solid(solid.clone());
        let color_animation = color.animation().unwrap();
        let transparency_animation = transparency.animation().unwrap();
        let animate = |animation: &crate::GradientAnimation<()>, minimum, maximum| {
            animation.ensure_curve(0).unwrap();
            let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
            curve.set_minimum(minimum);
            curve.set_maximum(maximum);
            curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
            animation.flags().unwrap().set(0, true).unwrap();
            curve
        };
        animate(&color_animation, -25.0, 125.0);
        let transparency_curve = animate(&transparency_animation, -0.25, 1.25);
        let ramp = color_animation.gradient().unwrap();
        ramp.clear_points().unwrap();
        ramp.add_normalized_color(0.0, 0.125, 0.25, 0.75).unwrap();
        ramp.add_normalized_color(1.0, 0.875, 0.625, 0.125).unwrap();

        let floating = node
            .render_cached_graph(
                crate::PointwiseCachedRenderMode::Floating,
                5,
                3,
                0.375,
                CallingThread,
            )
            .unwrap();
        assert!(floating.caught_errors().is_empty());
        assert_native_frame(recording, "solid-0.375", &floating.image().unwrap());

        let preview = node
            .render_cached_graph(Preview, 5, 3, 0.375, CallingThread)
            .unwrap();
        assert!(preview.caught_errors().is_empty());
        let preview = preview.output().unwrap();
        assert_eq!(preview.frames().len(), 32);
        for (stage, time) in [("solid-0.375", 0.375), ("solid-0.75", 0.75)] {
            assert_native_frame(recording, stage, &preview.frame_at_time(time).unwrap());
        }
        node.set_cached_image(None).unwrap();

        let geometry = ImageGeometry::new(5, 3);
        let options = PointwiseRenderOptions::new(false, true);
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let render = |graph: &PointwiseFinalGraph, stage, time, effects: &mut _| {
            let result = graph
                .render_with_effects(geometry, time, CallingThread, options, false, effects)
                .unwrap();
            assert!(result.caught_errors().is_empty());
            assert_native_frame(recording, stage, &result.image().unwrap());
        };
        let frozen = node.final_graph_clone().unwrap();
        for (stage, time) in [
            ("solid-0", 0.0),
            ("solid-0.375", 0.375),
            ("solid-0.75", 0.75),
            ("solid-1", 1.0),
        ] {
            render(&frozen, stage, time, &mut effects);
        }
        let snapshot = RenderCaptureSession::new()
            .capture_node(&node, 0.75)
            .unwrap();

        ramp.clear_points().unwrap();
        ramp.add_normalized_color(0.0, 0.0, 0.25, 0.75).unwrap();
        ramp.add_normalized_color(1.0, 1.0, 0.5, 0.125).unwrap();
        transparency_curve.set_maximum(0.75);
        render(&frozen, "frozen-palette", 0.75, &mut effects);
        let exported = PointwiseFinalExportCache::default()
            .render_snapshot_with_effects(&snapshot, geometry, CallingThread, options, &mut effects)
            .unwrap();
        assert!(exported.caught_errors().is_empty());
        assert_native_frame(recording, "frozen-palette", &exported.image().unwrap());
        render(
            &node.final_graph_clone().unwrap(),
            "edited-palette",
            0.75,
            &mut effects,
        );

        transparency.limits().edit(LimitsEdit::DecimalPlaces(0));
        let original = node
            .render_original_with_effects(5, 3, options, 0.5, CallingThread, &mut effects)
            .unwrap();
        assert!(original.caught_errors().is_empty());
        assert_native_frame(recording, "original-floor", &original.image().unwrap());
        let preview = solid.capture_preview_at(0.5).unwrap().render(5, 3).unwrap();
        assert_native_frame(
            recording,
            "original-floor",
            &SharedImageMap::from_image(preview),
        );
        let export = solid.capture_final_at(0.5).unwrap().render(5, 3).unwrap();
        assert_native_frame(
            recording,
            "final-default-precision",
            &SharedImageMap::from_image(export),
        );
        render(
            &node.final_graph_clone().unwrap(),
            "final-default-precision",
            0.5,
            &mut effects,
        );
    }

    #[test]
    fn solid_map_variable_bindings_match_original_render_and_frozen_export() {
        use crate::{
            AnalyticCurveKind, CurveLayerHandle, PointwiseFinalExportCache, SolidColorProperty,
        };
        let recording =
            include_str!("../../../../../port/evidence/solid-binding-final-original.tsv");
        let map = PointwiseFilter::<()>::new(crate::PointwiseKind::MapVariable).unwrap();
        let root = map.node();
        let solid = PointwiseNode::solid_color().unwrap();
        root.content(0).unwrap().set_node(Some(&solid)).unwrap();
        let original = map.parameter(3).unwrap();
        original.set_number(25.0).unwrap();
        map.parameter(4)
            .unwrap()
            .set_color(crate::ContextColorValue::unlinked([0.125, 0.25, 0.75]))
            .unwrap();
        let color = SolidColorProperty::Color;
        let transparency = SolidColorProperty::Transparency;
        solid.set_solid_binding_index(color, 0).unwrap();
        solid.set_solid_binding_index(transparency, 0).unwrap();
        let mapping: Vec<u16> = "0>0;100>1".encode_utf16().collect();
        solid
            .set_solid_binding_mapping(transparency, Some(&mapping))
            .unwrap();

        let geometry = ImageGeometry::new(5, 3);
        let options = PointwiseRenderOptions::new(false, true);
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let render = |graph: &PointwiseFinalGraph, stage, time, effects: &mut _| {
            let frame = graph
                .render_with_effects(geometry, time, CallingThread, options, false, effects)
                .unwrap();
            assert!(frame.caught_errors().is_empty());
            assert_native_frame(recording, stage, &frame.image().unwrap());
        };
        render(
            &root.final_graph_clone().unwrap(),
            "static-binding",
            0.375,
            &mut effects,
        );
        let mapping: Vec<u16> = "0>20;100>80".encode_utf16().collect();
        solid
            .set_solid_binding_mapping(color, Some(&mapping))
            .unwrap();
        original.set_axis_animated(0, true).unwrap();
        let curve = original.animation_curve(0).unwrap();
        curve.set_minimum(0.0);
        curve.set_maximum(100.0);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        let animated = root.final_graph_clone().unwrap();
        render(&animated, "animated-binding-0.375", 0.375, &mut effects);
        render(&animated, "animated-binding-0.75", 0.75, &mut effects);
        let snapshot = RenderCaptureSession::new()
            .capture_node(&root, 0.75)
            .unwrap();

        let floating = solid
            .render_cached_graph(
                crate::PointwiseCachedRenderMode::Floating,
                5,
                3,
                0.375,
                CallingThread,
            )
            .unwrap();
        assert!(floating.caught_errors().is_empty());
        assert_native_frame(
            recording,
            "animated-binding-0.375",
            &floating.image().unwrap(),
        );
        let preview = solid
            .render_cached_graph(Preview, 5, 3, 0.75, CallingThread)
            .unwrap();
        assert!(preview.caught_errors().is_empty());
        let preview = preview.output().unwrap();
        assert_eq!(preview.frames().len(), 32);
        assert_native_frame(
            recording,
            "animated-binding-0.75",
            &preview.frame_at_time(0.75).unwrap(),
        );
        solid.set_cached_image(None).unwrap();

        curve.set_maximum(50.0);
        render(&animated, "frozen-binding", 0.75, &mut effects);
        let export = PointwiseFinalExportCache::default()
            .render_snapshot_with_effects(&snapshot, geometry, CallingThread, options, &mut effects)
            .unwrap();
        assert!(export.caught_errors().is_empty());
        assert_native_frame(recording, "frozen-binding", &export.image().unwrap());
        render(
            &root.final_graph_clone().unwrap(),
            "fresh-binding",
            0.75,
            &mut effects,
        );

        let animation = solid.solid_animation(transparency).unwrap();
        animation.ensure_curve(0).unwrap();
        let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
        curve.set_minimum(0.125);
        curve.set_maximum(0.625);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        animation.flags().unwrap().set(0, true).unwrap();
        solid
            .set_solid_animation(transparency, Some(animation))
            .unwrap();
        render(
            &root.final_graph_clone().unwrap(),
            "local-transparency",
            0.375,
            &mut effects,
        );
    }

    #[test]
    fn wizard_bindings_refresh_solid_values_and_keep_final_clone_storage() {
        use crate::{AnalyticCurveKind, CurveLayerHandle, PointwiseCachedRenderMode};

        let recording =
            include_str!("../../../../../port/evidence/solid-binding-final-original.tsv");

        let wizard = PointwiseFilter::<()>::new(crate::PointwiseKind::WizardGroup).unwrap();
        let root = wizard.node();
        let mut number = crate::VariableSpec::new(crate::VARIABLE_NUMERIC);
        number.set_name(Some(&"Alpha".encode_utf16().collect::<Vec<_>>()));
        wizard.parameter(3).unwrap().set_variable(number).unwrap();
        let mut rgb = crate::VariableSpec::new(crate::VARIABLE_COLOR);
        rgb.set_name(Some(&"Tint".encode_utf16().collect::<Vec<_>>()));
        wizard.parameter(4).unwrap().set_variable(rgb).unwrap();
        let controls = wizard.wizard_dynamic_properties().unwrap();
        let solid = PointwiseNode::solid_color().unwrap();
        root.content(0).unwrap().set_node(Some(&solid)).unwrap();
        let color = crate::SolidColorProperty::Color;
        let transparency = crate::SolidColorProperty::Transparency;
        solid.set_solid_binding_index(color, 1).unwrap();
        solid.set_solid_binding_index(transparency, 0).unwrap();
        let mapping: Vec<u16> = "0>0;100>1".encode_utf16().collect();
        solid
            .set_solid_binding_mapping(transparency, Some(&mapping))
            .unwrap();
        controls[1]
            .set_color(crate::ContextColorValue::unlinked([0.125, 0.5, 0.875]))
            .unwrap();
        controls[0].set_number(25.0).unwrap();
        assert_eq!(solid.solid_color_values().unwrap(), [0.125, 0.5, 0.875]);
        assert_eq!(solid.solid_transparency().unwrap(), 0.25);

        let animation = controls[0].animation().unwrap();
        animation.ensure_curve(0).unwrap();
        let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
        curve.set_minimum(25.0);
        curve.set_maximum(75.0);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        animation.flags().unwrap().set(0, true).unwrap();
        controls[0].set_animation(Some(animation)).unwrap();
        let preview = solid
            .render_cached_graph(
                PointwiseCachedRenderMode::Floating,
                3,
                2,
                0.75,
                CallingThread,
            )
            .unwrap();
        assert!(preview.caught_errors().is_empty());
        assert_native_frame(recording, "wizard-live", &preview.image().unwrap());
        assert_eq!(solid.solid_transparency().unwrap(), 0.25);
        let final_graph = root.final_graph_clone().unwrap();
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let rendered = final_graph
            .render_with_effects(
                crate::ImageGeometry::new(3, 2),
                0.75,
                CallingThread,
                crate::PointwiseRenderOptions::new(false, true),
                false,
                &mut effects,
            )
            .unwrap();
        assert!(rendered.caught_errors().is_empty());
        assert_native_frame(recording, "wizard-final", &rendered.image().unwrap());
    }

    #[test]
    fn final_effect_map_chain_retains_names_and_samples_layer_settings() {
        use crate::{AnalyticCurveKind, CurveLayerHandle};

        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        let attach = |kind, input: &PointwiseNode<()>| {
            let filter = PointwiseFilter::new(kind).unwrap();
            filter
                .node()
                .slot(0)
                .unwrap()
                .set_node(Some(input))
                .unwrap();
            filter
        };
        let name = |filter: &PointwiseFilter<()>, index, value: &str| {
            filter
                .parameter(index)
                .unwrap()
                .set_text(Some(&value.encode_utf16().collect::<Vec<_>>()))
                .unwrap();
        };
        let a = attach(PointwiseKind::StoreMap, &gradient.node());
        name(&a, 0, "A");
        let invert = PointwiseNode::invert();
        invert.set_invert_red_only().unwrap();
        invert.slot(0).unwrap().set_node(Some(&a.node())).unwrap();
        let b = attach(PointwiseKind::StoreMap, &invert);
        name(&b, 0, "B");
        let layer = attach(PointwiseKind::LayerMaps, &b.node());
        name(&layer, 4, "Layered");
        layer.parameter(2).unwrap().set_index(1).unwrap(); // stored Add
        layer.parameter(3).unwrap().set_number(0.0).unwrap();
        for (index, value) in [(2, 4.0), (3, 0.5)] {
            let property = layer.parameter(index).unwrap();
            property.set_axis_animated(0, true).unwrap();
            let curve = property.animation_curve(0).unwrap();
            curve.set_minimum(value);
            curve.set_maximum(value);
            curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        }
        let chosen = layer.parameter(0).unwrap();
        let options = crate::SharedSelectionOptions::new(vec![Some(vec![65]), Some(vec![66])]);
        chosen.set_options(Some(options)).unwrap();
        chosen.set_index(1).unwrap();
        chosen.set_index(0).unwrap(); // remembers A independently of the options list
        let layer_graph = layer.node().final_graph_clone().unwrap();
        let selection = layer_graph.selected.filter().unwrap().parameter(0).unwrap();
        assert_eq!(
            selection.remembered_selection(),
            chosen.remembered_selection()
        );
        assert_eq!(
            selection.text().unwrap(),
            Some("None Found".encode_utf16().collect())
        );

        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let geometry = ImageGeometry::new(7, 3);
        let render = |graph: &PointwiseFinalGraph,
                      channels,
                      effects: &mut PointwiseRenderEffects<'_, ()>| {
            let outcome = graph
                .render_with_effects(
                    geometry,
                    0.375,
                    CallingThread,
                    PointwiseRenderOptions::new(false, channels),
                    false,
                    effects,
                )
                .unwrap();
            assert!(outcome.caught_errors().is_empty());
            outcome.image().unwrap()
        };
        let recording =
            include_str!("../../../../../port/evidence/gradient-effect-maps-final-original.tsv");
        assert_native_frame(
            recording,
            "layered",
            &render(&layer_graph, true, &mut effects),
        );
        let preview_width = ImageGeometry::new(168, 3);
        let wider = layer_graph
            .render_with_effects(
                preview_width,
                0.375,
                CallingThread,
                PointwiseRenderOptions::new(false, true),
                false,
                &mut effects,
            )
            .unwrap();
        assert!(wider.caught_errors().is_empty());
        assert_eq!(wider.image().unwrap().named_channels().len(), 3);
        assert_eq!(selection.text().unwrap(), Some(vec![65]));

        let merged = attach(PointwiseKind::MergeMaps, &layer.node());
        merged.parameter(1).unwrap().set_index(0).unwrap();
        name(&merged, 0, "Merged");
        let contrast = attach(PointwiseKind::FixMapContrast, &merged.node());
        let cleared = attach(PointwiseKind::ClearMaps, &contrast.node());
        cleared.parameter(1).unwrap().set_index(1).unwrap();
        name(&cleared, 2, "  Export  ");
        let graph = cleared.node().final_graph_clone().unwrap();
        assert_native_frame(recording, "export", &render(&graph, true, &mut effects));
        assert_native_frame(recording, "base-only", &render(&graph, false, &mut effects));

        let solid = PointwiseNode::solid_color().unwrap();
        solid.set_solid_color([0.25, 0., 0.]).unwrap();
        let store = attach(PointwiseKind::StoreMap, &solid);
        name(&store, 0, "Constant");
        let map = render(
            &store.node().final_graph_clone().unwrap(),
            true,
            &mut effects,
        );
        assert_native_frame(recording, "constant", &map);
    }

    #[test]
    fn wizard_tone_curve_renders_through_still_preview_and_final_paths() {
        let tone = PointwiseFilter::<()>::new(PointwiseKind::WizardToneCurve).unwrap();
        tone.parameter(0).unwrap().set_number(0.).unwrap();
        tone.parameter(1).unwrap().set_number(0.25).unwrap();
        tone.parameter(2).unwrap().set_number(0.75).unwrap();
        let source = PointwiseNode::solid_color().unwrap();
        source.set_solid_color([0.625, 0.25, 0.125]).unwrap();
        let node = tone.node();
        node.slot(0).unwrap().set_node(Some(&source)).unwrap();
        let geometry = ImageGeometry::new(9, 5);
        let width = geometry.width();
        let height = geometry.height();
        let still = node.render_original(width, height).unwrap();
        assert!(still.caught_errors().is_empty());
        let preview = node
            .render_cached_graph(
                crate::PointwiseCachedRenderMode::Preview,
                width,
                height,
                0.,
                CallingThread,
            )
            .unwrap();
        assert!(preview.caught_errors().is_empty());
        let final_image = render(&node.final_graph_clone().unwrap(), geometry, false);
        let expected = first_pixel(&still.image().unwrap());
        assert!((expected - 0.5625).abs() < 1e-6);
        assert_eq!(first_pixel(&preview.image().unwrap()), expected);
        assert_eq!(first_pixel(&final_image), expected);

        tone.parameter(1).unwrap().set_number(0.).unwrap();
        tone.parameter(2).unwrap().set_number(1.).unwrap();
        let warm = SharedImageMap::new(geometry);
        warm.set_constant(0, 0.625).unwrap();
        source.set_cached_image(Some(warm.clone())).unwrap();
        let forwarded = node.render_original(width, height).unwrap();
        assert!(forwarded.image().unwrap().same_reference(&warm));
    }

    #[test]
    fn wizard_kaleidoscope_preview_and_export_match_original_modes_and_sizes() {
        use crate::{
            AnalyticCurveKind, BinaryWriter, CurveLayerHandle, Gtx375GraphDocument,
            NodeWireReadPolicy, create_node_by_identity,
        };

        fn digest(values: &[f32]) -> String {
            let hash = values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
                });
            format!("{hash:016x}")
        }

        let cases = [
            ("squares-0", 0, 0., false, false),
            ("eight-0", 1, 0., false, false),
            ("twenty-four-0", 2, 0., false, false),
            ("forty-eight-0", 3, 0., false, false),
            ("constant-0", 3, 0., false, true),
            ("animated-0.25", 2, 0.25, true, false),
            ("animated-0.75", 2, 0.75, true, false),
        ];
        let recording =
            include_str!("../../../../../port/evidence/wizard-kaleidoscope-final-original.tsv");
        let geometry = ImageGeometry::new(12, 12);
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let mut check = |node: &PointwiseNode, time, label| {
            let (_, expected) = recording
                .lines()
                .filter_map(|line| line.split_once('\t'))
                .find(|(case, _)| *case == label)
                .unwrap();
            let mut expected = expected.split('\t');
            assert!(node.clone_supported_final_graph().unwrap().is_some());
            let snapshot = RenderCaptureSession::new()
                .capture_node(node, time)
                .unwrap();
            let preview = snapshot
                .render_preview_with_effects(geometry, CallingThread, &mut effects)
                .unwrap();
            let output = snapshot
                .render_export_with_effects(
                    geometry,
                    CallingThread,
                    PointwiseRenderOptions::new(false, true),
                    &mut effects,
                )
                .unwrap();
            assert!(preview.caught_errors().is_empty(), "{label}");
            assert!(output.caught_errors().is_empty(), "{label}");
            let preview = preview.output().unwrap().frame_at_time(time).unwrap();
            let output = output.image().unwrap();
            for channel in 0..4 {
                let final_values = output.materialize(channel).unwrap().values();
                assert_eq!(digest(&final_values), expected.next().unwrap(), "{label}");
                assert_eq!(
                    preview.materialize(channel).unwrap().values(),
                    final_values,
                    "{label}, preview channel {channel}"
                );
            }
            let maps = output.named_channels();
            if let Some(expected_map) = expected.next() {
                let [Some(map)] = maps.as_slice() else {
                    panic!("{label}: expected the connected Source Pattern map");
                };
                assert_eq!(String::from_utf16_lossy(map.name_utf16()), "Source Pattern");
                assert_eq!(
                    digest(&map.data().unwrap().data(true).unwrap().values()),
                    expected_map,
                    "{label}"
                );
            } else {
                assert!(maps.is_empty(), "{label}");
            }
        };
        for (label, mode, time, animated, constant) in cases {
            let wizard = PointwiseFilter::<()>::new(PointwiseKind::WizardKaleidoscope).unwrap();
            wizard
                .parameter(2)
                .unwrap()
                .set_number(f64::from(mode))
                .unwrap();
            let map = PointwiseFilter::new(PointwiseKind::StoreMap).unwrap();
            map.parameter(0)
                .unwrap()
                .set_text(Some(&"Source Pattern".encode_utf16().collect::<Vec<_>>()))
                .unwrap();
            let source = if constant {
                let node = PointwiseNode::solid_color().unwrap();
                node.set_solid_color([0.25, 0.5, 0.75]).unwrap();
                node
            } else {
                create_node_by_identity(Some(&"Noise".encode_utf16().collect::<Vec<_>>())).unwrap()
            };
            map.node().slot(0).unwrap().set_node(Some(&source)).unwrap();
            let node = wizard.node();
            node.slot(0).unwrap().set_node(Some(&map.node())).unwrap();
            if animated {
                let size = wizard.parameter(0).unwrap();
                size.set_axis_animated(0, true).unwrap();
                let curve = size.animation_curve(0).unwrap();
                curve.set_minimum(1.);
                curve.set_maximum(2.);
                curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
            }

            check(&node, time, label);
            if label == "animated-0.75" {
                let mut doc = Gtx375GraphDocument::new(vec![Some(node)]).unwrap();
                let policy = NodeWireReadPolicy::new(128, 8192);
                let mut gtx = BinaryWriter::default();
                doc.save_compatible_gtx(&mut gtx).unwrap();
                let mut native = BinaryWriter::default();
                doc.save_project(&mut native).unwrap();
                for reopened in [
                    Gtx375GraphDocument::load_project(native.bytes(), policy).unwrap(),
                    Gtx375GraphDocument::load_bounded(gtx.bytes(), policy).unwrap(),
                ] {
                    check(&reopened.branch(0).unwrap().unwrap(), time, label);
                }
            }
        }
        let factory =
            include_bytes!("../../../../../genetica/Wgroups/Noise Types/Kaleidoscope.wtx");
        let doc =
            Gtx375GraphDocument::load_bounded(factory, NodeWireReadPolicy::new(128, 8192)).unwrap();
        let group = doc.branch(0).unwrap().unwrap();
        check(&group, 0., "factory-root");
        check(&group.content(0).unwrap().node().unwrap(), 0., "factory-a");
    }

    #[test]
    fn final_tone_curve_uses_its_constant_lookup_offset() {
        let tone = PointwiseFilter::<()>::new(PointwiseKind::ToneCurve).unwrap();
        tone.set_tone_curve_red_only().unwrap();
        let curve = crate::SharedCurve::new(0.125, 0.625, false);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        tone.parameter(0).unwrap().set_curve(Some(curve)).unwrap();
        let root = tone.node();
        let source = PointwiseNode::solid_color().unwrap();
        source.set_solid_color([0.375, 0.5, 0.25]).unwrap();
        root.slot(0).unwrap().set_node(Some(&source)).unwrap();

        let geometry = ImageGeometry::new(5, 3);
        let final_image = render(&root.final_graph_clone().unwrap(), geometry, false);
        assert_native_tone_matrix("constant-tone", &final_image);
        let input = SharedImageMap::new(geometry);
        input.set_constant(0, 0.375).unwrap();
        let worker = tone
            .render_supplied(&[Some(input)], geometry.width(), geometry.height())
            .unwrap()
            .unwrap();
        assert_eq!(first_pixel(&worker), 0.3125);
    }

    #[test]
    fn final_tone_and_matrix_sample_the_frozen_graph() {
        use crate::{AnalyticCurveKind, CurveLayerHandle, PointwiseFinalExportCache};

        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        let tone = PointwiseFilter::new(PointwiseKind::ToneCurve).unwrap();
        tone.set_tone_curve_red_only().unwrap();
        let curve = crate::SharedCurve::new(0.125, 0.625, false);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        tone.parameter(0)
            .unwrap()
            .set_curve(Some(curve.clone()))
            .unwrap();
        let red = tone.parameter(1).unwrap();
        red.set_index(1).unwrap();
        red.set_axis_animated(0, true).unwrap();
        let red_curve = red.animation_curve(0).unwrap();
        red_curve.set_minimum(1.);
        red_curve.set_maximum(2.);
        red_curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        red.options()
            .unwrap()
            .set_entry(0, Some("No".encode_utf16().collect()))
            .unwrap();
        let tone_node = tone.node();
        tone_node
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();

        let matrix = PointwiseFilter::new(PointwiseKind::Matrix).unwrap();
        let object = matrix.parameter(0).unwrap().matrix().unwrap().unwrap();
        let coefficients = object.payload().coefficients().unwrap();
        coefficients.write(0, 2.).unwrap();
        let iterations = matrix.parameter(1).unwrap();
        iterations.set_number(0.).unwrap();
        iterations.set_axis_animated(0, true).unwrap();
        let count_curve = iterations.animation_curve(0).unwrap();
        count_curve.set_minimum(2.);
        count_curve.set_maximum(2.);
        count_curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        let root = matrix.node();
        root.slot(0).unwrap().set_node(Some(&tone_node)).unwrap();

        let sample = 0.375;
        let geometry = ImageGeometry::new(5, 3);
        let frozen = RenderCaptureSession::new()
            .capture_node(&root, sample)
            .unwrap();
        let clone = root.final_graph_clone().unwrap();
        let parameter = clone.selected.filter().unwrap().parameter(0).unwrap();
        let current = parameter
            .matrix()
            .unwrap()
            .unwrap()
            .payload()
            .coefficients()
            .unwrap();
        let previous = parameter
            .previous_matrix()
            .unwrap()
            .unwrap()
            .payload()
            .coefficients()
            .unwrap();
        assert!(!current.same_reference(&previous));
        assert_eq!(previous.read(0).unwrap(), 2.);
        coefficients.write(0, 1.).unwrap();
        curve.clear();
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let outcome = PointwiseFinalExportCache::default()
            .render_snapshot_with_effects(
                &frozen,
                geometry,
                CallingThread,
                PointwiseRenderOptions::new(false, false),
                &mut effects,
            )
            .unwrap();
        assert!(outcome.caught_errors().is_empty());
        let image = outcome.image().unwrap();
        assert_native_tone_matrix("frozen-settings-0.375", &image);
        assert_native_tone_matrix("frozen-settings-0.375", &render(&clone, geometry, false));
        let disabled = clone
            .render_with_effects(
                geometry,
                0.75,
                CallingThread,
                PointwiseRenderOptions::new(false, false),
                false,
                &mut effects,
            )
            .unwrap();
        assert!(disabled.caught_errors().is_empty());
        assert_native_tone_matrix("disabled-red-0.75", &disabled.image().unwrap());
        assert_native_tone_matrix(
            "fresh-settings-0.375",
            &render(&root.final_graph_clone().unwrap(), geometry, false),
        );
    }

    #[test]
    fn final_arithmetic_shortcuts_preserve_scene_demands_and_factor_bounds() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        gradient.set_should_final_cache(true);
        let gradient = gradient.node();
        let multiply = PointwiseFilter::new(PointwiseKind::Multiply)
            .unwrap()
            .node();
        let solid = PointwiseNode::solid_color().unwrap();
        solid.set_solid_color([1.0005, 0., 0.]).unwrap();
        multiply.slot(0).unwrap().set_node(Some(&gradient)).unwrap();
        multiply.slot(1).unwrap().set_node(Some(&solid)).unwrap();
        let add = PointwiseFilter::new(PointwiseKind::Add).unwrap().node();
        add.slot(1).unwrap().set_node(Some(&multiply)).unwrap();
        let geometry = ImageGeometry::new(5, 3);
        let image = render(&add.final_graph_clone().unwrap(), geometry, true);
        assert!(image.same_reference(&gradient.cached_final_image().unwrap()));
        assert_eq!(first_pixel(&image), 0.25);

        let gradient_channels = PointwiseChannelGuarantees::new(
            [None, Some(0.0), Some(0.0), Some(0.0)],
            [true; 4],
            false,
            None,
        );
        let factor = |red, has_scene| {
            PointwiseChannelGuarantees::new(
                [Some(red), Some(0.0), Some(0.0), Some(0.0)],
                [false; 4],
                has_scene,
                None,
            )
        };
        for (red, has_scene, slot) in [
            (1.0, false, Some(0)),
            (1.001, false, None),
            (0.999, false, None),
            (1.0, true, None),
        ] {
            let output = multiply
                .calculate_image_guarantees_for_inputs(&[
                    Some(gradient_channels),
                    Some(factor(red, has_scene)),
                ])
                .unwrap();
            assert_eq!(output.passthrough_slot(), slot);
            assert_eq!(output.has_scene(), has_scene);
        }
        let zero_scene = factor(0.0, true);
        let output = add
            .calculate_image_guarantees_for_inputs(&[Some(gradient_channels), Some(zero_scene)])
            .unwrap();
        assert_eq!(output.passthrough_slot(), None);
        assert!(output.has_scene());

        let hue = GradientGenerator::new().unwrap();
        hue.parameter(crate::GradientGeneratorParameterKind::Mode)
            .set_index(99)
            .unwrap();
        let hsb = PointwiseFilter::new(PointwiseKind::FromHsb).unwrap().node();
        hsb.slot(0).unwrap().set_node(Some(&hue.node())).unwrap();
        // Brightness is disconnected, so the final render need not evaluate Hue.
        let black = render(&hsb.final_graph_clone().unwrap(), geometry, false);
        for channel in 0..4 {
            assert_eq!(black.channel(channel, false).unwrap().values(), [0.]);
        }
    }

    #[test]
    fn frozen_combine_export_samples_its_choices_opacity_and_selector() {
        use crate::{AnalyticCurveKind, CurveLayerHandle, PointwiseFinalExportCache};

        let combine = PointwiseFilter::<()>::new(PointwiseKind::Combine).unwrap();
        let node = combine.node();
        let top = GradientGenerator::new().unwrap();
        let bottom = GradientGenerator::new().unwrap();
        let selector = GradientGenerator::new().unwrap();
        top.parameter(Location).set_axis(0, 0.75).unwrap();
        bottom.parameter(Location).set_axis(0, 0.5).unwrap();
        selector.parameter(Location).set_axis(0, 0.2).unwrap();
        let colorize = PointwiseFilter::new(PointwiseKind::Colorize).unwrap();
        let ramp = colorize.parameter(0).unwrap().gradient().unwrap();
        ramp.clear_points().unwrap();
        ramp.add_normalized_color(0., 0., 0., 0.).unwrap();
        ramp.add_normalized_color(1., 1., 0., 0.).unwrap();
        ramp.add_normalized_alpha(0., 0.75).unwrap();
        ramp.add_normalized_alpha(1., 0.75).unwrap();
        colorize.parameter(1).unwrap().set_index(1).unwrap();
        let colorize = colorize.node();
        colorize
            .slot(0)
            .unwrap()
            .set_node(Some(&top.node()))
            .unwrap();
        for (index, input) in [colorize, bottom.node(), selector.node()]
            .iter()
            .enumerate()
        {
            node.slot(index as i32)
                .unwrap()
                .set_node(Some(input))
                .unwrap();
        }

        let geometry = ImageGeometry::new(5, 3);
        let time = 0.375;
        for (index, selected) in [(0, 3), (1, 1), (2, 5), (4, 0)] {
            combine
                .parameter(index)
                .unwrap()
                .set_index(selected)
                .unwrap();
        }
        let opacity = combine.parameter(3).unwrap();
        opacity.set_number(time).unwrap();
        let expected = render(&node.final_graph_clone().unwrap(), geometry, false);
        assert_eq!(expected.channel(3, false).unwrap().values(), [0.25]);

        for (index, stored) in [(0, 0), (1, 0), (2, 0)] {
            combine.parameter(index).unwrap().set_index(stored).unwrap();
        }
        opacity.set_number(0.).unwrap();
        for (index, min, max) in [(0, 4., 5.), (1, 2., 2.5), (2, 6., 7.), (3, 0., 1.)] {
            let property = combine.parameter(index).unwrap();
            property.set_axis_animated(0, true).unwrap();
            let curve = property.animation_curve(0).unwrap();
            curve.set_minimum(min);
            curve.set_maximum(max);
            curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        }

        let preview = node
            .render_cached_graph(Preview, 5, 3, time, CallingThread)
            .unwrap();
        assert!(preview.caught_errors().is_empty());
        let sequence = preview.output().unwrap();
        assert_eq!(sequence.frames().len(), 32);
        let preview = sequence.frame_at_time(time).unwrap();
        for channel in 0..4 {
            assert_eq!(
                preview.channel(channel, false).unwrap().values(),
                expected.channel(channel, false).unwrap().values()
            );
        }
        combine
            .parameter(0)
            .unwrap()
            .options()
            .unwrap()
            .set_entry(3, Some("Normal".encode_utf16().collect()))
            .unwrap();
        let snapshot = RenderCaptureSession::new()
            .capture_node(&node, time)
            .unwrap();
        opacity.animation_curve(0).unwrap().set_maximum(0.5);
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let frozen = PointwiseFinalExportCache::default()
            .render_snapshot_with_effects(
                &snapshot,
                geometry,
                CallingThread,
                PointwiseRenderOptions::new(false, true),
                &mut effects,
            )
            .unwrap();
        assert!(frozen.caught_errors().is_empty());
        let frozen = frozen.image().unwrap();
        for channel in 0..4 {
            assert_eq!(
                frozen.channel(channel, false).unwrap().values(),
                expected.channel(channel, false).unwrap().values()
            );
        }
        let current = render(&node.final_graph_clone().unwrap(), geometry, false);
        assert_ne!(
            current.channel(0, false).unwrap().values(),
            frozen.channel(0, false).unwrap().values()
        );
        assert_eq!(opacity.number(), 0.);
    }

    #[test]
    fn selector_in_exterior_slots_preserves_shared_named_map_identity() {
        let node = |kind| PointwiseFilter::<()>::new(kind).unwrap().node();
        let connect = |parent: &PointwiseNode, slot, child: PointwiseNode| {
            parent.slot(slot).unwrap().set_node(Some(&child)).unwrap();
        };
        let store_substance = || {
            let map = PointwiseFilter::<()>::new(PointwiseKind::StoreMap).unwrap();
            map.parameter(0)
                .unwrap()
                .set_text(Some(&"Substance".encode_utf16().collect::<Vec<_>>()))
                .unwrap();
            let map = map.node();
            connect(&map, 0, GradientGenerator::new().unwrap().node());
            map
        };
        let exported_maps = |root: &PointwiseNode| {
            let snapshot = crate::RenderCaptureSession::new()
                .capture_node(root, 0.)
                .unwrap();
            let mut capacity = |_| Ok(true);
            let mut preflight = |_, _| Ok(());
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            let result = snapshot
                .render_export_with_effects(
                    ImageGeometry::new(32, 32),
                    CallingThread,
                    PointwiseRenderOptions::new(false, true),
                    &mut effects,
                )
                .unwrap();
            assert!(result.caught_errors().is_empty());
            result
                .image()
                .unwrap()
                .named_channels()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
        };

        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        group.parameter(0).unwrap().set_number(2.).unwrap();
        let group = group.node();
        connect(&group, 0, store_substance());
        // This unused input still participates in final graph admission.
        connect(&group, 1, node(PointwiseKind::Selector));
        let sum = node(PointwiseKind::Add);
        for slot in [0, 1] {
            let inner = node(PointwiseKind::Group);
            connect(&inner, 0, node(PointwiseKind::Reference));
            inner
                .content(0)
                .unwrap()
                .set_node(Some(&node(PointwiseKind::Reference)))
                .unwrap();
            connect(&sum, slot, inner);
        }
        group.content(0).unwrap().set_node(Some(&sum)).unwrap();
        let maps = exported_maps(&group);
        assert_eq!(maps.len(), 1);
        assert_eq!(String::from_utf16_lossy(maps[0].name_utf16()), "Substance");

        let independent = node(PointwiseKind::Add);
        for slot in [0, 1] {
            connect(&independent, slot, store_substance());
        }
        let maps = exported_maps(&independent);
        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0].name_utf16(), maps[1].name_utf16());
        let first = maps[0].data().unwrap();
        let second = maps[1].data().unwrap();
        assert!(!first.same_store(&second));
        assert_eq!(
            first.data(false).unwrap().values(),
            second.data(false).unwrap().values()
        );
    }

    #[test]
    fn final_combine_passthrough_matches_saved_reference_source_identity() {
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        group.parameter(0).unwrap().set_number(2.).unwrap();
        let group = group.node();
        for (index, location) in [(0, 0.25), (1, 0.75)] {
            let source = GradientGenerator::new().unwrap();
            source.parameter(Location).set_axis(0, location).unwrap();
            group
                .slot(index)
                .unwrap()
                .set_node(Some(&source.node()))
                .unwrap();
        }
        let combine = PointwiseFilter::new(PointwiseKind::Combine).unwrap();
        combine.parameter(3).unwrap().set_number(0.375).unwrap();
        let combine = combine.node();
        let top = PointwiseFilter::new(PointwiseKind::Reference).unwrap();
        let bottom = PointwiseFilter::new(PointwiseKind::Reference).unwrap();
        combine
            .slot(0)
            .unwrap()
            .set_node(Some(&top.node()))
            .unwrap();
        combine
            .slot(1)
            .unwrap()
            .set_node(Some(&bottom.node()))
            .unwrap();
        group.content(0).unwrap().set_node(Some(&combine)).unwrap();
        let slot = || {
            let graph = combine.final_graph_clone().unwrap();
            prepare_guarantees(&graph.tree, &|| false).unwrap()[&graph.selected.id]
                .channels
                .passthrough_slot()
        };
        assert_eq!(slot(), Some(0));
        bottom.parameter(0).unwrap().set_number(2.).unwrap();
        assert_eq!(slot(), None);
    }

    #[test]
    fn final_change_bci_uses_constructor_options_and_channel_guarantees() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        let filter = PointwiseFilter::new(PointwiseKind::ChangeBci).unwrap();
        let root = filter.node();
        root.slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let geometry = ImageGeometry::new(5, 2);
        filter.parameter(1).unwrap().set_number(25.).unwrap();
        let clean = root.final_graph_clone().unwrap();
        let expected = render(&clean, geometry, false);
        assert_eq!(first_pixel(&expected), 0.5);
        filter
            .parameter(0)
            .unwrap()
            .options()
            .unwrap()
            .set_entry(1, Some("Yes".encode_utf16().collect()))
            .unwrap();
        let copy = root.final_graph_clone().unwrap();
        let rendered = render(&copy, geometry, false);
        assert_eq!(
            expected.channel(0, false).unwrap().values(),
            rendered.channel(0, false).unwrap().values()
        );

        filter.parameter(2).unwrap().set_number(-100.).unwrap();
        gradient.set_should_final_cache(true);
        let graph = root.final_graph_clone().unwrap();
        let rendered = render(&graph, geometry, true);
        for channel in 0..3 {
            assert_eq!(rendered.channel(channel, false).unwrap().values(), [0.5]);
        }
        assert_eq!(rendered.channel(3, false).unwrap().values(), [0.]);
        assert!(gradient.cached_final_image().is_none());
    }

    #[test]
    fn combine_can_bypass_regions_in_an_unused_image_input() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        let regions = PointwiseFilter::new(PointwiseKind::Regions).unwrap();
        regions
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        regions.parameter(1).unwrap().set_index(0).unwrap();
        let bottom = GradientGenerator::new().unwrap();
        let combine = PointwiseFilter::new(PointwiseKind::Combine).unwrap();
        combine.parameter(2).unwrap().set_index(5).unwrap(); // Bottom transparency.
        combine.parameter(3).unwrap().set_number(0.).unwrap();
        combine
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&regions.node()))
            .unwrap();
        combine
            .node()
            .slot(1)
            .unwrap()
            .set_node(Some(&bottom.node()))
            .unwrap();

        let geometry = ImageGeometry::new(32, 32);
        assert!(
            render(
                &regions.node().final_graph_clone().unwrap(),
                geometry,
                false
            )
            .region_store()
            .is_some()
        );
        let image = render(
            &combine.node().final_graph_clone().unwrap(),
            geometry,
            false,
        );
        assert!(image.region_store().is_none());
        let bottom = render(&bottom.node().final_graph_clone().unwrap(), geometry, false);
        assert_eq!(
            image.materialize(0).unwrap().values(),
            bottom.materialize(0).unwrap().values()
        );
    }

    #[test]
    fn final_free_transform_and_kaleidoscope_keep_upstream_passthrough() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        let solid = PointwiseNode::solid_color().unwrap();
        solid.set_solid_color([0.75; 3]).unwrap();
        let add = PointwiseFilter::new(PointwiseKind::Add).unwrap().node();
        add.slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        add.slot(1).unwrap().set_node(Some(&solid)).unwrap();
        let bci = PointwiseFilter::new(PointwiseKind::ChangeBci).unwrap();
        bci.parameter(1).unwrap().set_binding_index(0);
        bci.node().slot(0).unwrap().set_node(Some(&add)).unwrap();
        let transform = PointwiseFilter::new(PointwiseKind::FreeTransform)
            .unwrap()
            .node();
        transform
            .slot(0)
            .unwrap()
            .set_node(Some(&bci.node()))
            .unwrap();
        let kaleidoscope = PointwiseFilter::new(PointwiseKind::Kaleidoscope).unwrap();
        kaleidoscope
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&transform))
            .unwrap();
        let region = kaleidoscope.parameter(1).unwrap().region().unwrap();
        region.set_x(0.2);
        region.set_size(0.5);

        let geometry = ImageGeometry::new(32, 32);
        let input = render(&add.final_graph_clone().unwrap(), geometry, false);
        assert!(
            input
                .channel(0, false)
                .unwrap()
                .values()
                .iter()
                .any(|value| *value > 1.)
        );
        let expected = kaleidoscope
            .render_supplied_at_time(
                &[Some(input)],
                geometry.width(),
                geometry.height(),
                0.375,
                CallingThread,
                PointwiseRenderOptions::new(false, false),
            )
            .unwrap()
            .unwrap();
        let graph = kaleidoscope.node().final_graph_clone().unwrap();
        let actual = render(&graph, geometry, false);
        for index in 0..4 {
            assert_eq!(
                actual.materialize(index).unwrap().values(),
                expected.materialize(index).unwrap().values()
            );
        }
    }

    #[test]
    fn final_change_bci_samples_auto_fix_and_all_three_adjustments() {
        use crate::{AnalyticCurveKind, CurveLayerHandle};

        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        let filter = PointwiseFilter::new(PointwiseKind::ChangeBci).unwrap();
        let node = filter.node();
        node.slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let geometry = ImageGeometry::new(5, 2);

        // The final test renderer samples at 0.375. Round 1.375 to the first
        // Auto Fix option and compare the other curves with their stored equivalents.
        filter.parameter(0).unwrap().set_index(0).unwrap();
        for (ix, sample) in [(1, 12.), (2, 24.), (3, 6.)] {
            filter.parameter(ix).unwrap().set_number(sample).unwrap();
        }
        let expected = render(&node.final_graph_clone().unwrap(), geometry, false);
        filter.parameter(0).unwrap().set_index(1).unwrap();
        for ix in 1..=3 {
            filter.parameter(ix).unwrap().set_number(0.).unwrap();
        }
        for (ix, min, max) in [(0, 1., 2.), (1, 0., 32.), (2, 0., 64.), (3, 0., 16.)] {
            let property = filter.parameter(ix).unwrap();
            property.set_axis_animated(0, true).unwrap();
            let curve = property.animation_curve(0).unwrap();
            curve.set_minimum(min);
            curve.set_maximum(max);
            curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        }

        let actual = render(&node.final_graph_clone().unwrap(), geometry, false);
        for ch in 0..4 {
            assert_eq!(
                actual.channel(ch, false).unwrap().values(),
                expected.channel(ch, false).unwrap().values()
            );
        }
        assert_eq!(filter.parameter(0).unwrap().index(), 1);
        for ix in 1..=3 {
            assert_eq!(filter.parameter(ix).unwrap().number(), 0.);
        }

        let input = SharedImageMap::new(geometry);
        input.set_constant(0, 0.25).unwrap();
        filter
            .parameter(0)
            .unwrap()
            .set_axis_animated(0, false)
            .unwrap();
        let direct = filter
            .render_supplied(&[Some(input.clone())], 5, 2)
            .unwrap()
            .unwrap();
        assert!(input.same_reference(&direct));
        let sampled = filter
            .render_supplied_at_time(
                &[Some(input.clone())],
                5,
                2,
                0.,
                CallingThread,
                PointwiseRenderOptions::new(false, false),
            )
            .unwrap()
            .unwrap();
        assert_eq!(first_pixel(&sampled), 0.25);
        assert!(!input.same_reference(&sampled));
    }

    #[test]
    fn final_graph_render_wraps_ordinary_failure_and_keeps_cache_cold() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.set_should_final_cache(true);
        gradient
            .parameter(crate::GradientGeneratorParameterKind::Mode)
            .set_index(99)
            .unwrap();
        let invert = PointwiseNode::invert();
        invert
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        invert.set_should_final_cache(true);
        let graph = invert.final_graph_clone().unwrap();
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        assert!(matches!(graph.render_with_effects(
            ImageGeometry::new(4, 2), 0., CallingThread,
            PointwiseRenderOptions::new(false, false), true, &mut effects,
        ), Err(PointwiseError::FinalRender(error)) if matches!(*error, PointwiseError::Index)));
        assert!(gradient.cached_final_image().is_none());
        assert!(invert.cached_final_image().is_none());
    }
}
