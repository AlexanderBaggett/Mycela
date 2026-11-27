//! Retained CPU image graph. Slots and Parent use node IDs in a merged
//! region: stale source backedges and repeated children cannot create Arc cycles.
//! Original and timed graph traversal share the supplied-image bodies.
//! Final clones reuse the still-image traversal and prepared image kernels.
use super::property::Cohort;
use super::region::{NodeId, NodeRoute, Sponsor};
use super::{PointwiseError, PointwiseFilter, PointwiseKind, SharedImageMap, guard};
use crate::{ImageGeometry, SharedSolidColorNode, SolidColorCloneMode};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, Mutex},
};
#[path = "graph_guarantees.rs"]
mod guarantees;
pub use guarantees::{PointwiseChannelGuarantees, PointwisePreparationHandler};
#[path = "graph_final_render.rs"]
mod final_render;
#[path = "graph_mode.rs"]
mod mode;
#[path = "graph_progress.rs"]
mod progress;
#[path = "graph_scale.rs"]
mod scale;
pub(in crate::pointwise) use scale::reaches_scaled_reference;
#[path = "graph_cached_render.rs"]
mod timed;
pub(in crate::pointwise) use final_render::ThumbnailReuse;
pub(in crate::pointwise) use final_render::{
    FinalExportCacheTree, FinalImageReuse, LabSimplifications, PlaybackImageCache,
    SequenceOutputCache, StillExportCachePolicy,
};
pub use final_render::{PointwiseFinalGraph, PointwiseThumbnailCache};
pub use mode::{
    PointwiseNodeChangeHandler, PointwiseSlotChange, PointwiseSlotChangeHandler,
    SharedDefaultInputMapping,
};
pub use timed::PointwiseCachedRenderMode;

impl<E> PointwiseNode<E> {
    pub(super) fn is_scene_node(&self) -> bool {
        matches!(
            &self.core().payload,
            Payload::ScenePrimitive { .. }
                | Payload::SceneImport { .. }
                | Payload::SceneClearLights
                | Payload::SceneCombine
                | Payload::SceneTransform { .. }
                | Payload::UvTexture { .. }
        )
    }

    pub(super) fn is_scene_generator(&self) -> bool {
        matches!(
            &self.core().payload,
            Payload::ScenePrimitive { .. } | Payload::SceneImport { .. }
        )
    }
}

pub(super) fn validate_group_render<E: 'static>(
    node: &PointwiseNode<E>,
) -> Result<(), PointwiseError<E>> {
    if node.is_clone_mode() {
        return Err(PointwiseError::UnsupportedCapability(
            "Group clone scheduling",
        ));
    }
    let filter = node.filter().ok_or(PointwiseError::InvalidCast)?;
    filter.ensure_property_evaluation_supported()?;
    // hj.k8, hj.ku and hj.kv do not read Iterations in Original mode.
    // Its retained source limits mark it unused; clone scheduling remains
    // separately rejected above rather than inventing repeated image passes.
    Ok(())
}

fn group_skips_color<E>(
    node: &PointwiseNode<E>,
    options: PointwiseRenderOptions,
) -> Result<bool, PointwiseError<E>> {
    let property = match node.kind() {
        Some(PointwiseKind::Group) => 3,
        Some(PointwiseKind::WizardGroup) => 23,
        _ => return Ok(false),
    };
    Ok(options.is_color_only()
        && node
            .filter()
            .ok_or(PointwiseError::InvalidCast)?
            .parameter(property)?
            .index()
            == 0)
}

fn merge_group_color_and_channels<E>(
    geometry: ImageGeometry,
    color: &SharedImageMap,
    effects: Option<&SharedImageMap>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let result = SharedImageMap::new(geometry);
    for channel in 0..color.channel_count()? {
        let channel = i32::try_from(channel).map_err(|_| PointwiseError::Arithmetic)?;
        result.copy_channel_from(channel, color, channel)?;
    }
    if let Some(effects) = effects {
        result.merge_effects(&[Some(effects.clone())])?;
    }
    Ok(result)
}

pub(super) fn validate_reference_render<E: 'static>(
    node: &PointwiseNode<E>,
) -> Result<(), PointwiseError<E>> {
    if node.is_clone_mode() {
        return Err(PointwiseError::UnsupportedCapability(
            "Reference clone scheduling",
        ));
    }
    let property = node
        .filter()
        .ok_or(PointwiseError::InvalidCast)?
        .parameter(0)?;
    property.ensure_property_evaluation_supported()?;
    Ok(())
}

// Follow zl.i's ordinary image dependency walk. A Reference encountered in
// a node's input marks its stored slot in the surrounding Group; a Reference
// that is itself a contents root has no inputs and marks nothing. Nested
// Groups first discover their own inputs, then map those inputs outward.
// Scene requirements need separate image/scene flags and are left to the
// existing scene traversal.
pub(super) fn group_image_dependencies<E: 'static>(
    group: &PointwiseNode<E>,
) -> Result<Option<Vec<usize>>, PointwiseError<E>> {
    enum Frame<E> {
        Enter(PointwiseNode<E>),
        GroupContents(PointwiseNode<E>),
        Slots(PointwiseNode<E>, Vec<usize>, usize, BTreeSet<usize>),
        Merge(PointwiseNode<E>, Vec<usize>, usize, BTreeSet<usize>),
    }
    let Some(content) = group.content(0)?.node() else {
        return Ok(Some(Vec::new()));
    };
    let mut frames = vec![Frame::Enter(content)];
    let mut active = BTreeSet::new();
    let mut results: Vec<BTreeSet<usize>> = Vec::new();
    while let Some(frame) = frames.pop() {
        match frame {
            Frame::Enter(node) => {
                if !active.insert(node.id) {
                    return Err(PointwiseError::UnsupportedCapability(
                        "cyclic source graph scheduling",
                    ));
                }
                if node.cached_image().is_some() {
                    active.remove(&node.id);
                    results.push(BTreeSet::new());
                } else if node.is_scene_generator()
                    || node.scene_clear_lights().is_some()
                    || node.scene_transform_3d().is_some()
                    || node.scene_combine_3d().is_some()
                    || node.uv_texture_output().is_some()
                {
                    return Ok(None);
                } else if node.kind().is_some_and(PointwiseKind::is_group) {
                    if let Some(content) = node.content(0)?.node() {
                        frames.push(Frame::GroupContents(node));
                        frames.push(Frame::Enter(content));
                    } else {
                        active.remove(&node.id);
                        results.push(BTreeSet::new());
                    }
                } else {
                    let selected = (0..node.slots().len()).collect();
                    frames.push(Frame::Slots(node, selected, 0, BTreeSet::new()));
                }
            }
            Frame::GroupContents(node) => {
                let selected = results
                    .pop()
                    .expect("completed Group contents")
                    .into_iter()
                    .filter(|i| *i < node.slots().len())
                    .collect();
                frames.push(Frame::Slots(node, selected, 0, BTreeSet::new()));
            }
            Frame::Slots(node, selected, cursor, mut result) => {
                if cursor == selected.len() {
                    active.remove(&node.id);
                    results.push(result);
                    continue;
                }
                let child = node.slot(selected[cursor] as i32)?.node();
                if let Some(child) = child {
                    if child.kind() == Some(PointwiseKind::Reference) {
                        let number = child
                            .filter()
                            .ok_or(PointwiseError::InvalidCast)?
                            .parameter(0)?
                            .number() as i32;
                        if let Ok(index) = usize::try_from(number.wrapping_sub(1)) {
                            result.insert(index);
                        }
                    } else {
                        frames.push(Frame::Merge(node, selected, cursor + 1, result));
                        frames.push(Frame::Enter(child));
                        continue;
                    }
                }
                frames.push(Frame::Slots(node, selected, cursor + 1, result));
            }
            Frame::Merge(node, selected, cursor, mut result) => {
                result.extend(results.pop().expect("completed input dependency"));
                frames.push(Frame::Slots(node, selected, cursor, result));
            }
        }
    }
    let indices = results.pop().expect("completed dependency discovery");
    Ok(Some(
        indices
            .into_iter()
            .filter(|i| *i < group.slots().len())
            .collect(),
    ))
}

/// Actual wo caller options for retained Original image-only graphs. The same
/// flags reach every protected body; they do not request a separate scene or
/// clone-mode evaluation. Effect-map bodies use render_channels directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointwiseRenderOptions {
    color_only: bool,
    render_channels: bool,
    supplied_slot_presence: Option<[bool; 4]>,
    canvas_tile: Option<NodeId>,
    synthesis_layer: Option<(NodeId, usize)>,
}
impl PointwiseRenderOptions {
    pub const fn new(color_only: bool, render_channels: bool) -> Self {
        Self {
            color_only,
            render_channels,
            supplied_slot_presence: None,
            canvas_tile: None,
            synthesis_layer: None,
        }
    }
    /// Describe connections for a caller-supplied image pipeline. When omitted,
    /// kernels inspect their retained graph slots instead.
    pub fn with_supplied_slot_presence(mut self, slots: [bool; 4]) -> Self {
        self.supplied_slot_presence = Some(slots);
        self
    }
    pub(super) fn supplied_slot_connected(&self, slot: usize) -> Option<bool> {
        self.supplied_slot_presence.map(|slots| slots[slot])
    }
    pub fn is_color_only(&self) -> bool {
        self.color_only
    }
    pub fn is_channel_rendering(&self) -> bool {
        self.render_channels
    }
    pub(in crate::pointwise) fn with_canvas_tile(mut self, target: NodeId) -> Self {
        self.canvas_tile = Some(target);
        self
    }
    pub(in crate::pointwise) fn is_canvas_tile(&self, target: NodeId) -> bool {
        self.canvas_tile == Some(target)
    }

    pub(in crate::pointwise) fn with_synthesis_layer(
        mut self,
        target: NodeId,
        layer: usize,
    ) -> Self {
        self.synthesis_layer = Some((target, layer));
        self
    }

    pub(in crate::pointwise) fn synthesis_layer_for(&self, target: NodeId) -> Option<usize> {
        self.synthesis_layer
            .filter(|(node, _)| *node == target)
            .map(|(_, layer)| layer)
    }
}
pub(super) enum Payload<E> {
    Input {
        image: Mutex<Option<SharedImageMap>>,
        cache: Mutex<Option<RenderedImage>>,
    },
    SceneClearLights,
    SceneCombine,
    UvTexture {
        kind: super::uv_texture::UvTextureKind,
        properties: Vec<Arc<super::scene_primitive::property::PropertyRoot<E>>>,
    },
    SceneTransform {
        state: Arc<super::scene_modifier::TransformState<E>>,
    },
    ScenePrimitive {
        state: Arc<super::scene_primitive::PrimitiveState<E>>,
    },
    SceneImport {
        state: super::scene_import::ImportState<E>,
    },
    Filter {
        core: Arc<Cohort<E>>,
        ix: usize,
        kind: PointwiseKind,
    },
    ImageGenerator {
        model: Arc<Mutex<super::generator::GeneratorState<E>>>,
    },
    Invert {
        model: crate::SharedInvertNode<super::invert_node::InvertCallbackError<E>>,
        wire: Arc<super::invert_node::InvertWireOwner<E>>,
        cache: Mutex<Option<RenderedImage>>,
    },
    Solid {
        model: SharedSolidColorNode<E>,
        cache: Mutex<Option<RenderedImage>>,
    },
}
// A real qi occurrence outlives its membership in the current node list.
// IDs create no sponsor/Arc backedge; outward handles sponsor the whole region.
pub(super) struct SlotCore {
    owner: NodeId,
    pub(super) name: Vec<u16>,
    child: Mutex<Option<NodeId>>,
}
impl SlotCore {
    fn new(owner: NodeId, name: Vec<u16>, child: Option<NodeId>) -> Arc<Self> {
        Arc::new(Self {
            owner,
            name,
            child: Mutex::new(child),
        })
    }
}
struct Links {
    shader: Option<Arc<super::ShaderProgram>>,
    parent: Option<NodeId>,
    slots: Vec<Arc<SlotCore>>,
    contents: Vec<Arc<SlotCore>>,
    is_filter: bool,
    default_input: bool,
}
pub(super) struct NodeCore<E> {
    pub(super) id: NodeId,
    pub(super) payload: Payload<E>,
    pub(super) route: NodeRoute<E>,
    links: Mutex<Links>,
    mode_events: Mutex<mode::ModeEvents<E>>,
    pub(super) history_binding: Mutex<super::history_binding::NodeBinding<E>>,
}
impl<E> NodeCore<E> {
    pub(super) fn filter(
        id: NodeId,
        core: Arc<Cohort<E>>,
        ix: usize,
        kind: PointwiseKind,
    ) -> Arc<Self> {
        let names: &[&str] = match kind {
            PointwiseKind::WizardGroup
            | PointwiseKind::Group
            | PointwiseKind::MapVariable
            | PointwiseKind::WizardReference
            | PointwiseKind::Canvas
            | PointwiseKind::TemplateLab(_) => &[
                "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15",
                "16", "17", "18", "19", "20",
            ],
            PointwiseKind::Style => &["Design", "Background", "Input Texture"],
            PointwiseKind::Synthesis => &["Input"],
            PointwiseKind::Polygon
            | PointwiseKind::Reference
            | PointwiseKind::MakeEnvironment
            | PointwiseKind::Checkerboard
            | PointwiseKind::Shader => &[],
            PointwiseKind::Frequency
            | PointwiseKind::Equalize
            | PointwiseKind::GaussianBlur
            | PointwiseKind::Sharpen
            | PointwiseKind::ChangeBci
            | PointwiseKind::ChangeHsb
            | PointwiseKind::ChangeOklab
            | PointwiseKind::Pixelate
            | PointwiseKind::Rotate => &["Original"],
            PointwiseKind::WizardColor
            | PointwiseKind::WizardFiveColor
            | PointwiseKind::WizardSlideColor => &["Original"],
            PointwiseKind::Colorize
            | PointwiseKind::ColorizeG2
            | PointwiseKind::Levels
            | PointwiseKind::Matrix
            | PointwiseKind::WizardMatrix => &["Input"],
            PointwiseKind::Reflect | PointwiseKind::Refract => &["Base", "Heightmap"],
            PointwiseKind::Stereogram => &["Background", "Heightmap"],
            PointwiseKind::Regions => &["Regions", "Image"],
            PointwiseKind::LegacyBricksBoards => &["Color"],
            PointwiseKind::Transmit => &["Original", "Heightmap"],
            PointwiseKind::ReflectEnvironment | PointwiseKind::RefractEnvironment => {
                &["Heightmap", "Environment"]
            }
            PointwiseKind::Caustics => &["Heightmap"],
            PointwiseKind::Atmosphere => &["Density"],
            PointwiseKind::Hair => &["Heightmap", "Hair Density", "Color"],
            PointwiseKind::FullLight => &["Material", "Heightmap", "Environment", "Reflectivity"],
            PointwiseKind::ConvertEnvironment
            | PointwiseKind::RotateEnvironment
            | PointwiseKind::DiffuseEnvironment
            | PointwiseKind::ScatterEnvironment => &["Environment"],
            PointwiseKind::ToneCurve
            | PointwiseKind::ToneCurveG2
            | PointwiseKind::WizardToneCurve
            | PointwiseKind::PointLight
            | PointwiseKind::Shadow
            | PointwiseKind::ShadowG2
            | PointwiseKind::IlluminateG2 => &["Heightmap"],
            PointwiseKind::Fractalize => &["Noise"],
            PointwiseKind::Exposure => &["Environment"],
            PointwiseKind::Selector => &["Image"],
            PointwiseKind::MatchColor => &["Original", "Match To"],
            PointwiseKind::ArtBlur => &["Original", "Blur Radius", "Driver"],
            PointwiseKind::Distort => &["Original", "Direction", "Distance"],
            PointwiseKind::Remap => &["Original", "X", "Y"],
            PointwiseKind::FreeTransform
            | PointwiseKind::Kaleidoscope
            | PointwiseKind::WizardKaleidoscope
            | PointwiseKind::MoveRepeat
            | PointwiseKind::RotateTile => &["Original"],
            PointwiseKind::GetChannel
            | PointwiseKind::ClearMaps
            | PointwiseKind::StoreMap
            | PointwiseKind::MergeMaps
            | PointwiseKind::FixMapContrast
            | PointwiseKind::LayerMaps => &["Image"],
            PointwiseKind::MedianBlur | PointwiseKind::SmartBlur | PointwiseKind::VariableBlur => {
                &["Original", "Blur Radius"]
            }
            PointwiseKind::Smear => &["Original", "Direction", "Distance"],
            PointwiseKind::MotionBlur => &["Original", "Blur Radius", "Angle"],
            PointwiseKind::ShapeBlur => &["Original", "Blur Radius", "Shape"],
            PointwiseKind::Add | PointwiseKind::Multiply => &["A", "B"],
            PointwiseKind::FromHsb => &["Hue", "Saturation", "Brightness"],
            PointwiseKind::OklabColor => &["Lightness", "a", "b", "Opacity"],
            PointwiseKind::Combine => &["Top", "Bottom", "Selector"],
        };
        let lab = match kind {
            PointwiseKind::TemplateLab(lab) => Some(lab.template()),
            _ => None,
        };
        Arc::new(Self {
            id,
            payload: Payload::Filter { core, ix, kind },
            route: NodeRoute::new(),
            history_binding: Mutex::new(super::history_binding::NodeBinding::new()),
            mode_events: Mutex::new(mode::ModeEvents::new()),
            links: Mutex::new(Links {
                shader: None,
                parent: None,
                contents: Vec::new(),
                is_filter: false,
                default_input: false,
                slots: names
                    .iter()
                    .enumerate()
                    .map(|(index, name)| {
                        let name = lab
                            .as_ref()
                            .and_then(|spec| spec.inputs.get(index))
                            .unwrap_or(name);
                        SlotCore::new(id, name.encode_utf16().collect(), None)
                    })
                    .collect(),
            }),
        })
    }
}
/// One real graph node. Outward handles retain the complete connected region;
/// source Parent is independent of the ordered slot list and may be stale.
pub struct PointwiseNode<E = ()> {
    pub(super) id: NodeId,
    pub(super) sponsor: Arc<Sponsor<E>>,
}
impl<E> Clone for PointwiseNode<E> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            sponsor: Arc::clone(&self.sponsor),
        }
    }
}
impl<E> fmt::Debug for PointwiseNode<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PointwiseNode({:?})", self.id)
    }
}
/// One actual qi occurrence. Fresh Slots arrays share these occurrences;
/// replacing the current list never retargets an already returned handle.
pub struct PointwiseSlot<E = ()> {
    owner: PointwiseNode<E>,
    pub(super) core: Arc<SlotCore>,
}
impl<E> Clone for PointwiseSlot<E> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            core: Arc::clone(&self.core),
        }
    }
}
impl<E> fmt::Debug for PointwiseSlot<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PointwiseSlot({:?}/{:p})",
            self.core.owner,
            Arc::as_ptr(&self.core)
        )
    }
}
/// A retained still map or the original 32-frame Preview sequence.
#[derive(Clone, Debug)]
pub enum RenderedImage {
    Still(SharedImageMap),
    Animated(Arc<[SharedImageMap; Self::PREVIEW_FRAME_COUNT]>),
}
impl RenderedImage {
    pub const PREVIEW_FRAME_COUNT: usize = 32;

    pub fn frames(&self) -> &[SharedImageMap] {
        match self {
            Self::Still(image) => std::slice::from_ref(image),
            Self::Animated(frames) => frames.as_slice(),
        }
    }

    pub fn still_image(&self) -> Option<SharedImageMap> {
        match self {
            Self::Still(image) => Some(image.clone()),
            Self::Animated(_) => None,
        }
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Still(a), Self::Still(b)) => a.same_reference(b),
            (Self::Animated(a), Self::Animated(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Source zl.q selects by rounding time times frame count to even, with
    /// no wrap or clamp when that rounds past the last frame.
    pub fn frame_at_time(&self, time: f64) -> Option<SharedImageMap> {
        match self {
            Self::Still(image) => Some(image.clone()),
            Self::Animated(frames) => {
                let index =
                    crate::framework_cast::integer((time * frames.len() as f64).round_ties_even());
                usize::try_from(index)
                    .ok()
                    .and_then(|index| frames.get(index))
                    .cloned()
            }
        }
    }
}

/// Public graph evaluation catches ordinary kernel errors and returns a new
/// empty map. The cause is retained separately, never reported as a kernel pass.
pub struct PointwiseRenderOutcome<E = ()> {
    pub(super) image: Option<RenderedImage>,
    pub(super) caught: Vec<PointwiseError<E>>,
}
impl<E> PointwiseRenderOutcome<E> {
    /// Return the single map for a still render. Use output() to read a sequence.
    pub fn image(&self) -> Option<SharedImageMap> {
        self.image.as_ref()?.still_image()
    }
    pub fn output(&self) -> Option<RenderedImage> {
        self.image.clone()
    }
    pub fn caught_errors(&self) -> &[PointwiseError<E>] {
        &self.caught
    }
}
impl<E> PointwiseNode<E> {
    pub(super) fn restore_content_links(&self, contents: &[(Arc<SlotCore>, Option<NodeId>)]) {
        for (slot, child) in contents {
            *guard(&slot.child) = *child;
            if let Some(child) = child {
                guard(&self.at(*child).core().links).parent = Some(self.id);
            }
        }
        guard(&self.core().links).contents =
            contents.iter().map(|(slot, _)| slot.clone()).collect();
    }
    pub(super) fn restore_input_links(&self, filter: bool, children: &[Option<NodeId>]) {
        let core = self.core();
        let mut links = guard(&core.links);
        if links.is_filter != filter {
            links.slots = links
                .slots
                .iter()
                .map(|slot| SlotCore::new(self.id, slot.name.clone(), None))
                .collect();
            links.is_filter = filter;
        }
        for (slot, child) in links.slots.iter().zip(children) {
            *guard(&slot.child) = *child;
        }
        drop(links);
        for child in children.iter().flatten() {
            guard(&self.at(*child).core().links).parent = Some(self.id);
        }
    }

    pub(super) fn from_filter(filter: &PointwiseFilter<E>) -> Self {
        Self {
            id: filter.graph_id,
            sponsor: Arc::clone(&filter.sponsor),
        }
    }
    pub(super) fn from_core(core: &Arc<NodeCore<E>>) -> Option<Self> {
        Some(Self {
            id: core.id,
            sponsor: Sponsor::for_node(core)?,
        })
    }
    pub(super) fn core(&self) -> Arc<NodeCore<E>> {
        self.sponsor.node(self.id)
    }
    fn at(&self, id: NodeId) -> Self {
        Self {
            id,
            sponsor: Arc::clone(&self.sponsor),
        }
    }
    pub(super) fn merge_region(&self, other: &Self) {
        self.sponsor.merge(&other.sponsor);
    }
    pub(super) fn retained_nodes(&self) -> Vec<Self> {
        self.sponsor
            .nodes()
            .into_iter()
            .map(|core| self.at(core.id))
            .collect()
    }
    /// Capture reads and installs links directly; public slot assignment would
    /// reparent, invalidate and destroy a stale source Parent relationship.
    pub(super) fn capture_links(&self) -> (Option<Self>, Vec<Option<Self>>, Vec<Vec<u16>>) {
        let (parent, slots) = {
            let core = self.core();
            let links = guard(&core.links);
            (links.parent, links.slots.clone())
        };
        (
            parent.map(|id| self.at(id)),
            slots
                .iter()
                .map(|slot| guard(&slot.child).map(|id| self.at(id)))
                .collect(),
            slots.iter().map(|slot| slot.name.clone()).collect(),
        )
    }
    pub(super) fn install_captured_links(
        &self,
        parent: Option<Self>,
        slots: Vec<Option<Self>>,
        names: Vec<Vec<u16>>,
    ) {
        assert_eq!(slots.len(), names.len(), "captured slot/name layout");
        for node in parent.iter().chain(slots.iter().flatten()) {
            self.merge_region(node);
        }
        let slots = slots
            .into_iter()
            .zip(names)
            .map(|(child, name)| SlotCore::new(self.id, name, child.map(|node| node.id)))
            .collect();
        let core = self.core();
        let mut links = guard(&core.links);
        links.parent = parent.map(|v| v.id);
        // Fresh occurrences belong to this detached owner. Previously returned
        // handles keep their occurrence/name/child and the region sponsor.
        links.slots = slots;
    }
    /// Real ordinary original-mode Invert with one retained Original slot.
    pub fn invert() -> Self
    where
        E: 'static,
    {
        Self::publish_invert(crate::SharedInvertNode::new(
            crate::SelectionCloneMode::Original,
        ))
    }
    pub(super) fn publish_invert(
        model: crate::SharedInvertNode<super::invert_node::InvertCallbackError<E>>,
    ) -> Self
    where
        E: 'static,
    {
        Self::publish_invert_with_wire(model, super::invert_node::InvertWireOwner::new())
    }
    pub(in crate::pointwise) fn publish_invert_with_wire(
        model: crate::SharedInvertNode<super::invert_node::InvertCallbackError<E>>,
        wire: Arc<super::invert_node::InvertWireOwner<E>>,
    ) -> Self
    where
        E: 'static,
    {
        let id = NodeId::new();
        let core = Arc::new(NodeCore {
            id,
            payload: Payload::Invert {
                model,
                wire,
                cache: Mutex::new(None),
            },
            route: NodeRoute::new(),
            history_binding: Mutex::new(super::history_binding::NodeBinding::new()),
            mode_events: Mutex::new(mode::ModeEvents::new()),
            links: Mutex::new(Links {
                shader: None,
                parent: None,
                contents: Vec::new(),
                is_filter: false,
                default_input: false,
                slots: vec![SlotCore::new(id, "Original".encode_utf16().collect(), None)],
            }),
        });
        let node = Self {
            id,
            sponsor: Sponsor::publish(&core),
        };
        node.install_invert_invalidation();
        node
    }
    /// Real unadjusted Solid Color constructor. Graph editing stays on this
    /// sponsored node; underlying unsponsored property handles never escape.
    pub fn solid_color() -> Result<Self, PointwiseError<E>>
    where
        E: 'static,
    {
        let model =
            SharedSolidColorNode::new_unattached(SolidColorCloneMode::Original).map_err(|_| {
                PointwiseError::UnsupportedCapability("Solid Color constructor dependency")
            })?;
        Ok(Self::publish_solid(model))
    }
    pub(super) fn publish_solid(model: SharedSolidColorNode<E>) -> Self
    where
        E: 'static,
    {
        let id = NodeId::new();
        let core = Arc::new(NodeCore {
            id,
            payload: Payload::Solid {
                model,
                cache: Mutex::new(None),
            },
            route: NodeRoute::new(),
            history_binding: Mutex::new(super::history_binding::NodeBinding::new()),
            mode_events: Mutex::new(mode::ModeEvents::new()),
            links: Mutex::new(Links {
                shader: None,
                parent: None,
                contents: Vec::new(),
                is_filter: false,
                default_input: false,
                slots: Vec::new(),
            }),
        });
        let node = Self {
            id,
            sponsor: Sponsor::publish(&core),
        };
        node.install_solid_invalidation();
        node
    }
    /// Source x: a hidden host-supplied image leaf with no properties or slots.
    pub fn input() -> Self {
        let id = NodeId::new();
        let core = Arc::new(NodeCore {
            id,
            payload: Payload::Input {
                image: Mutex::new(None),
                cache: Mutex::new(None),
            },
            route: NodeRoute::new(),
            history_binding: Mutex::new(super::history_binding::NodeBinding::new()),
            mode_events: Mutex::new(mode::ModeEvents::new()),
            links: Mutex::new(Links {
                shader: None,
                parent: None,
                contents: Vec::new(),
                is_filter: false,
                default_input: false,
                slots: Vec::new(),
            }),
        });
        Self {
            id,
            sponsor: Sponsor::publish(&core),
        }
    }
    /// The source field is an image reference; setting it does not send a property event.
    pub fn input_image(&self) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        match &self.core().payload {
            Payload::Input { image, .. } => Ok(guard(image).clone()),
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    pub fn set_input_image(&self, value: Option<SharedImageMap>) -> Result<(), PointwiseError<E>> {
        match &self.core().payload {
            Payload::Input { image, .. } => {
                let old = std::mem::replace(&mut *guard(image), value);
                drop(old);
                Ok(())
            }
            _ => Err(PointwiseError::InvalidCast),
        }
    }
    /// The original unbound Gradient scalar generator, with all thirteen modes.
    pub fn gradient_generator() -> Result<Self, PointwiseError<E>> {
        Ok(super::GradientGenerator::new()?.node())
    }
    pub fn gradient_generator_handle(&self) -> Option<super::GradientGenerator<E>> {
        self.image_generator_handle()
            .filter(|g| g.kind() == super::ImageGeneratorKind::Gradient)
    }
    pub fn image_generator(kind: super::ImageGeneratorKind) -> Result<Self, PointwiseError<E>> {
        Ok(super::ImageGenerator::with_kind(kind)?.node())
    }
    pub fn imported_image_handle(&self) -> Option<super::ImportedImageNode<E>>
    where
        E: 'static,
    {
        super::ImportedImageNode::from_node(self)
    }
    pub fn image_generator_handle(&self) -> Option<super::ImageGenerator<E>> {
        super::ImageGenerator::from_node(self)
    }

    pub(super) fn publish_image_generator(
        model: Arc<Mutex<super::generator::GeneratorState<E>>>,
    ) -> Self {
        let id = NodeId::new();
        let core = Arc::new(NodeCore {
            id,
            payload: Payload::ImageGenerator { model },
            route: NodeRoute::new(),
            history_binding: Mutex::new(super::history_binding::NodeBinding::new()),
            mode_events: Mutex::new(mode::ModeEvents::new()),
            links: Mutex::new(Links {
                shader: None,
                parent: None,
                contents: Vec::new(),
                is_filter: false,
                default_input: false,
                slots: Vec::new(),
            }),
        });
        Self {
            id,
            sponsor: Sponsor::publish(&core),
        }
    }
    pub(super) fn publish_scene_primitive(
        state: Arc<super::scene_primitive::PrimitiveState<E>>,
    ) -> Self
    where
        E: 'static,
    {
        let id = NodeId::new();
        let core = Arc::new(NodeCore {
            id,
            payload: Payload::ScenePrimitive {
                state: state.clone(),
            },
            route: NodeRoute::new(),
            history_binding: Mutex::new(super::history_binding::NodeBinding::new()),
            mode_events: Mutex::new(mode::ModeEvents::new()),
            links: Mutex::new(Links {
                shader: None,
                parent: None,
                slots: Vec::new(),
                contents: Vec::new(),
                is_filter: false,
                default_input: false,
            }),
        });
        state.bind(&core);
        let sponsor = Sponsor::publish(&core);
        Self { id, sponsor }
    }
    pub(super) fn publish_scene_import(
        properties: Vec<Arc<super::scene_primitive::property::PropertyRoot<E>>>,
        mesh: Option<crate::ImportedMesh>,
    ) -> Self
    where
        E: 'static,
    {
        let id = NodeId::new();
        let core = Arc::new(NodeCore {
            id,
            payload: Payload::SceneImport {
                state: super::scene_import::ImportState {
                    properties,
                    mesh: Mutex::new(mesh),
                },
            },
            route: NodeRoute::new(),
            history_binding: Mutex::new(super::history_binding::NodeBinding::new()),
            mode_events: Mutex::new(mode::ModeEvents::new()),
            links: Mutex::new(Links {
                shader: None,
                parent: None,
                slots: Vec::new(),
                contents: Vec::new(),
                is_filter: false,
                default_input: false,
            }),
        });
        if let Payload::SceneImport { state } = &core.payload {
            for property in &state.properties {
                property.bind(&core);
            }
        }
        Self {
            id,
            sponsor: Sponsor::publish(&core),
        }
    }
    pub(super) fn publish_scene_clear_lights() -> Self {
        let id = NodeId::new();
        let core = Arc::new(NodeCore {
            id,
            payload: Payload::SceneClearLights,
            route: NodeRoute::new(),
            history_binding: Mutex::new(super::history_binding::NodeBinding::new()),
            mode_events: Mutex::new(mode::ModeEvents::new()),
            links: Mutex::new(Links {
                shader: None,
                parent: None,
                slots: vec![SlotCore::new(id, "Model".encode_utf16().collect(), None)],
                contents: Vec::new(),
                is_filter: false,
                default_input: false,
            }),
        });
        Self {
            id,
            sponsor: Sponsor::publish(&core),
        }
    }
    pub(super) fn publish_scene_combine() -> Self {
        let id = NodeId::new();
        let core = Arc::new(NodeCore {
            id,
            payload: Payload::SceneCombine,
            route: NodeRoute::new(),
            history_binding: Mutex::new(super::history_binding::NodeBinding::new()),
            mode_events: Mutex::new(mode::ModeEvents::new()),
            links: Mutex::new(Links {
                shader: None,
                parent: None,
                slots: vec![
                    SlotCore::new(id, "Model A".encode_utf16().collect(), None),
                    SlotCore::new(id, "Model B".encode_utf16().collect(), None),
                ],
                contents: Vec::new(),
                is_filter: false,
                default_input: false,
            }),
        });
        Self {
            id,
            sponsor: Sponsor::publish(&core),
        }
    }
    pub(super) fn publish_scene_transform(
        state: Arc<super::scene_modifier::TransformState<E>>,
    ) -> Self
    where
        E: 'static,
    {
        let id = NodeId::new();
        let core = Arc::new(NodeCore {
            id,
            payload: Payload::SceneTransform {
                state: state.clone(),
            },
            route: NodeRoute::new(),
            history_binding: Mutex::new(super::history_binding::NodeBinding::new()),
            mode_events: Mutex::new(mode::ModeEvents::new()),
            links: Mutex::new(Links {
                shader: None,
                parent: None,
                slots: vec![SlotCore::new(id, "Model".encode_utf16().collect(), None)],
                contents: Vec::new(),
                is_filter: false,
                default_input: false,
            }),
        });
        state.bind(&core);
        Self {
            id,
            sponsor: Sponsor::publish(&core),
        }
    }
    pub(super) fn publish_uv_texture(
        kind: super::uv_texture::UvTextureKind,
        properties: Vec<Arc<super::scene_primitive::property::PropertyRoot<E>>>,
    ) -> Self
    where
        E: 'static,
    {
        let id = NodeId::new();
        let core = Arc::new(NodeCore {
            id,
            payload: Payload::UvTexture { kind, properties },
            route: NodeRoute::new(),
            history_binding: Mutex::new(super::history_binding::NodeBinding::new()),
            mode_events: Mutex::new(mode::ModeEvents::new()),
            links: Mutex::new(Links {
                shader: None,
                parent: None,
                slots: std::iter::once("Model")
                    .chain(kind.image_port())
                    .map(|name| SlotCore::new(id, name.encode_utf16().collect(), None))
                    .collect(),
                contents: Vec::new(),
                is_filter: false,
                default_input: false,
            }),
        });
        if let Payload::UvTexture { properties, .. } = &core.payload {
            for property in properties {
                property.bind(&core);
            }
        }
        Self {
            id,
            sponsor: Sponsor::publish(&core),
        }
    }
    pub fn filter(&self) -> Option<PointwiseFilter<E>> {
        match &self.core().payload {
            Payload::Filter { core, ix, .. } => Some(PointwiseFilter::from_graph(
                Arc::clone(core),
                *ix,
                self.id,
                Arc::clone(&self.sponsor),
            )),
            Payload::Input { .. }
            | Payload::Solid { .. }
            | Payload::ImageGenerator { .. }
            | Payload::Invert { .. }
            | Payload::SceneClearLights
            | Payload::SceneCombine
            | Payload::SceneTransform { .. }
            | Payload::UvTexture { .. }
            | Payload::ScenePrimitive { .. }
            | Payload::SceneImport { .. } => None,
        }
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.id == other.id
    }
    /// Process-local identity of this retained graph occurrence.
    pub fn occurrence_id(&self) -> u64 {
        self.id.get()
    }
    pub(crate) fn graph_identity(&self) -> impl Ord + use<E> {
        self.id
    }
    pub fn kind(&self) -> Option<PointwiseKind> {
        match &self.core().payload {
            Payload::Filter { kind, .. } => Some(*kind),
            _ => None,
        }
    }
    pub fn parent(&self) -> Option<Self> {
        guard(&self.core().links).parent.map(|id| self.at(id))
    }
    pub fn slots(&self) -> Vec<PointwiseSlot<E>> {
        let slots = guard(&self.core().links).slots.clone();
        slots
            .into_iter()
            .map(|core| PointwiseSlot {
                owner: self.at(core.owner),
                core,
            })
            .collect()
    }
    pub fn slot(&self, ix: i32) -> Result<PointwiseSlot<E>, PointwiseError<E>> {
        let ix = usize::try_from(ix).map_err(|_| PointwiseError::Index)?;
        let core = guard(&self.core().links)
            .slots
            .get(ix)
            .cloned()
            .ok_or(PointwiseError::Index)?;
        Ok(PointwiseSlot {
            owner: self.at(core.owner),
            core,
        })
    }
    /// zl.Parent first asks the OLD parent to detach its first matching slot.
    /// Removing a slot alone intentionally does not clear the old child's Parent.
    pub fn set_parent(&self, parent: Option<&Self>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let current = self.parent();
        if current.as_ref().map(|n| n.id) == parent.map(|n| n.id) {
            return Ok(());
        }
        // History can wrap existing input trees. A direct Parent change would
        // leave the editor slot and render occurrence pointing to different nodes.
        if self.has_live_property_history_binding()
            || parent.is_some_and(Self::has_live_property_history_binding)
            || current
                .as_ref()
                .is_some_and(Self::has_live_property_history_binding)
        {
            return Err(PointwiseError::UnsupportedCapability(
                "property history child/group ownership",
            ));
        }
        self.assign_parent(parent)
    }

    pub(super) fn assign_parent(&self, parent: Option<&Self>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        let current = self.parent();
        if current.as_ref().map(|n| n.id) == parent.map(|n| n.id) {
            return Ok(());
        }
        if let Some(parent) = parent {
            self.sponsor.merge(&parent.sponsor)
        }
        if let Some(old) = current {
            let mut detached = false;
            for slot in old.slots() {
                if slot.node().is_some_and(|n| n.id == self.id) {
                    slot.set_node(None)?;
                    detached = true;
                    break;
                }
            }
            if !detached {
                for slot in old.contents() {
                    if slot.node().is_some_and(|n| n.id == self.id) {
                        slot.set_node(None)?;
                        break;
                    }
                }
            }
        }
        guard(&self.core().links).parent = parent.map(|p| p.id);
        Ok(())
    }
    // History already exchanged all the affected input occurrences.
    pub(super) fn promote_restored_branch(&self) {
        guard(&self.core().links).parent = None;
    }
    pub fn cached_image(&self) -> Option<SharedImageMap> {
        self.cached_rendered_output()?.still_image()
    }

    /// RenderedImageMap may retain an animated Preview from a public render.
    pub fn cached_rendered_output(&self) -> Option<RenderedImage> {
        match &self.core().payload {
            Payload::Filter { .. } => self.filter()?.cached_rendered_output(),
            Payload::Input { cache, .. }
            | Payload::Solid { cache, .. }
            | Payload::Invert { cache, .. } => guard(cache).clone(),
            Payload::ImageGenerator { model } => guard(model).cache.clone(),
            Payload::ScenePrimitive { .. }
            | Payload::SceneImport { .. }
            | Payload::SceneClearLights
            | Payload::SceneCombine
            | Payload::SceneTransform { .. }
            | Payload::UvTexture { .. } => {
                guard(&self.core().mode_events).scene_rendered_image.clone()
            }
        }
    }

    pub fn set_cached_image(&self, image: Option<SharedImageMap>) -> Result<(), PointwiseError<E>> {
        self.set_rendered_output(image.map(RenderedImage::Still))
    }

    fn set_rendered_output(&self, image: Option<RenderedImage>) -> Result<(), PointwiseError<E>> {
        match &self.core().payload {
            Payload::ScenePrimitive { .. }
            | Payload::SceneImport { .. }
            | Payload::SceneClearLights
            | Payload::SceneCombine
            | Payload::SceneTransform { .. }
            | Payload::UvTexture { .. } => {
                let old = std::mem::replace(
                    &mut guard(&self.core().mode_events).scene_rendered_image,
                    image,
                );
                drop(old);
                self.notify_rendered_image_changed()
            }
            Payload::Filter { .. } => self
                .filter()
                .ok_or(PointwiseError::NullReference)?
                .set_rendered_output(image),
            Payload::ImageGenerator { model } => {
                let old = std::mem::replace(&mut guard(model).cache, image);
                drop(old);
                self.notify_rendered_image_changed()
            }
            Payload::Input { cache, .. }
            | Payload::Solid { cache, .. }
            | Payload::Invert { cache, .. } => {
                let old = std::mem::replace(&mut *guard(cache), image);
                drop(old);
                self.notify_rendered_image_changed()
            }
        }
    }

    pub(in crate::pointwise) fn install_captured_rendered_output(
        &self,
        output: RenderedImage,
    ) -> Result<(), PointwiseError<E>> {
        self.set_rendered_output(Some(output))
    }
    // sh invalidation is not a RenderedImageMap setter: no cache-change callback.
    pub(super) fn invalidate(&self) {
        let mut next = Some(self.clone());
        let mut seen = BTreeSet::new();
        while let Some(node) = next {
            if !seen.insert(node.id) {
                break;
            }
            node.clear_local_mode_cache();
            next = node.parent();
        }
    }
    /// Original og.a cached-child traversal, including missing-slot empty maps,
    /// source ordered supplied-map kernels and ordinary error-to-empty handling.
    /// Scene/group/clone/optimized-guarantee scheduling and positive editor
    /// contexts remain separate capabilities; only Original image nodes exist
    /// in this graph authority, so those roles cannot be silently substituted.
    pub fn render_original(
        &self,
        width: i32,
        height: i32,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        self.render_original_with_options(width, height, PointwiseRenderOptions::new(false, false))
    }
    pub fn render_original_with_options(
        &self,
        width: i32,
        height: i32,
        options: PointwiseRenderOptions,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        self.render_original_in_context(width, height, options, None, &mut |_, _| {
            Err(PointwiseError::UnsupportedCapability(
                "Noise memory preflight context",
            ))
        })
    }
    /// Explicit caller execution/time and real memory preflight; no source host
    /// or fixed-success provider is synthesized for procedural graph leaves.
    pub fn render_original_with_generator_context(
        &self,
        width: i32,
        height: i32,
        options: PointwiseRenderOptions,
        time: f64,
        execution: crate::ProceduralExecution,
        preflight: &mut impl FnMut(i32, i32) -> Result<(), PointwiseError<E>>,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        self.render_original_in_context(width, height, options, Some((time, execution)), preflight)
    }
    fn render_original_in_context(
        &self,
        width: i32,
        height: i32,
        options: PointwiseRenderOptions,
        context: Option<(f64, crate::ProceduralExecution)>,
        preflight: &mut impl FnMut(i32, i32) -> Result<(), PointwiseError<E>>,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        let mut capacity = |_| {
            Err(PointwiseError::UnsupportedCapability(
                "Combine ambient capacity predicate",
            ))
        };
        let mut effects = super::PointwiseRenderEffects::new(&mut capacity, preflight);
        self.render_graph_in_effects(width, height, options, context, &mut effects, None, None)
    }
    /// Same Original graph evaluator with both real memory effects supplied.
    #[allow(clippy::too_many_arguments)]
    pub fn render_original_with_effects(
        &self,
        width: i32,
        height: i32,
        options: PointwiseRenderOptions,
        time: f64,
        execution: crate::ProceduralExecution,
        effects: &mut super::PointwiseRenderEffects<'_, E>,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        self.render_graph_in_effects(
            width,
            height,
            options,
            Some((time, execution)),
            effects,
            None,
            None,
        )
    }
    pub(in crate::pointwise) fn render_original_sequence_frame(
        &self,
        geometry: ImageGeometry,
        time: f64,
        execution: crate::ProceduralExecution,
        options: PointwiseRenderOptions,
        cache: &mut SequenceOutputCache,
        effects: &mut super::PointwiseRenderEffects<'_, E>,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        self.render_graph_in_effects(
            geometry.width(),
            geometry.height(),
            options,
            Some((time, execution)),
            effects,
            None,
            Some(&mut final_render::OriginalSequenceFrame::new(cache)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_graph_in_effects(
        &self,
        width: i32,
        height: i32,
        options: PointwiseRenderOptions,
        context: Option<(f64, crate::ProceduralExecution)>,
        effects: &mut super::PointwiseRenderEffects<'_, E>,
        mut final_render: Option<&mut final_render::FinalGraphExecution<'_, E>>,
        mut original_sequence: Option<&mut final_render::OriginalSequenceFrame<'_>>,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        // Source zl.b2 -> k prepares guarantees over the reachable topology
        // before the render traversal. This graph authority has no retained
        // general b0/b1/b2 or a7/a6 preparation and cannot substitute eager
        // child rendering: parents can change their decisions from those values.
        // Reject before any graph child, cache, or kernel is consumed. Visit
        // Contents and cached subtrees too: source k traverses both independent
        // of RenderedImageMap. Supplied-map n7.k8 remains separately available.
        let mut pending = vec![self.clone()];
        let mut inspected = BTreeSet::new();
        while let Some(node) = pending.pop() {
            super::check_render_cancelled(effects.cancel_check())?;
            if !inspected.insert(node.id) {
                continue;
            }
            if node.kind().is_some_and(PointwiseKind::is_group) && final_render.is_none() {
                validate_group_render(&node)?;
            }
            if node.kind() == Some(PointwiseKind::Reference) && final_render.is_none() {
                validate_reference_render(&node)?;
                let property = node
                    .filter()
                    .ok_or(PointwiseError::InvalidCast)?
                    .parameter(0)?;
                let animated = property.axis_is_animated(0)?;
                let has_own_cache = animated && node.cached_image().is_some();
                if !has_own_cache && let Some(target) = node.reference_target()? {
                    pending.push(target);
                }
                if animated
                    && !has_own_cache
                    && let Some(target) =
                        node.reference_target_at(context.map_or(0.0, |(time, _)| time))?
                {
                    pending.push(target);
                }
            }
            for slot in node.slots().into_iter().chain(node.contents()) {
                if let Some(child) = slot.node() {
                    pending.push(child);
                }
            }
        }
        enum FinalGroupOutput {
            Image { skip: bool, channels: bool },
            Scene,
        }
        enum Frame<E> {
            Enter(PointwiseNode<E>),
            Leave(PointwiseNode<E>, usize),
            LeaveGroup(PointwiseNode<E>, bool, bool),
            PrepareGroup(
                PointwiseNode<E>,
                bool,
                bool,
                Vec<(usize, Option<PointwiseNode<E>>)>,
            ),
            EnterFinalGroup(PointwiseNode<E>, FinalGroupOutput),
            PrepareFinalGroupContents(
                PointwiseNode<E>,
                FinalGroupOutput,
                Vec<final_render::FinalSlotDemand>,
            ),
            LeaveReference(PointwiseNode<E>),
            LeaveEmpty(i32, i32),
            LeaveDisconnected,
            EnterFinalScene(PointwiseNode<E>),
            LeaveFinalScene(PointwiseNode<E>, usize),
            EmptyFinalScene,
            PrepareFinalSceneFromImage(PointwiseNode<E>),
            ForwardFinalScene(PointwiseNode<E>),
            FinishFinalSceneImage(PointwiseNode<E>),
            PrepareUvTexture(PointwiseNode<E>, bool),
            FinishUvTexture(PointwiseNode<E>, bool),
        }
        impl<E: 'static> Frame<E> {
            fn node(&self) -> Option<&PointwiseNode<E>> {
                match self {
                    Self::Enter(node)
                    | Self::Leave(node, _)
                    | Self::LeaveReference(node)
                    | Self::LeaveGroup(node, _, _)
                    | Self::PrepareGroup(node, _, _, _)
                    | Self::EnterFinalGroup(node, _)
                    | Self::PrepareFinalGroupContents(node, _, _)
                    | Self::EnterFinalScene(node)
                    | Self::LeaveFinalScene(node, _)
                    | Self::ForwardFinalScene(node)
                    | Self::PrepareFinalSceneFromImage(node)
                    | Self::FinishFinalSceneImage(node)
                    | Self::PrepareUvTexture(node, _)
                    | Self::FinishUvTexture(node, _) => Some(node),
                    Self::LeaveEmpty(..) | Self::LeaveDisconnected | Self::EmptyFinalScene => None,
                }
            }

            fn weight(&self, progress: &progress::GraphProgress) -> u64 {
                let Some(node) = self.node() else {
                    return 1;
                };
                match self {
                    Self::Enter(_)
                    | Self::EnterFinalGroup(..)
                    | Self::EnterFinalScene(_)
                    | Self::PrepareUvTexture(..) => progress.branch(node),
                    Self::PrepareGroup(..) | Self::PrepareFinalGroupContents(..) => node
                        .content(0)
                        .ok()
                        .and_then(|slot| slot.node())
                        .map_or(1, |content| progress.branch(&content)),
                    _ => progress::GraphProgress::kernel(node),
                }
            }
        }
        let observer = effects.graph_progress();
        let mut progress = observer.map(|_| progress::GraphProgress::new(self));
        let mut stack = vec![Frame::Enter(self.clone())];
        let mut images: Vec<Option<SharedImageMap>> = Vec::new();
        let mut scenes: Vec<Option<crate::Scene>> = Vec::new();
        let mut active = BTreeSet::new();
        let mut active_scenes = BTreeSet::new();
        let mut prepared_inputs: BTreeMap<NodeId, BTreeMap<NodeId, SharedImageMap>> =
            BTreeMap::new();
        let mut caught = Vec::new();
        let geometry = ImageGeometry::new(width, height);
        let time = context.map_or(0., |(time, _)| time);
        loop {
            if let Some(progress) = &mut progress {
                progress.resolve(&stack, |progress, frame| frame.weight(progress));
            }
            let Some(frame) = stack.pop() else {
                break;
            };
            super::check_render_cancelled(effects.cancel_check())?;
            if let Some(progress) = &mut progress {
                let fraction = progress.start(stack.len());
                if let Some(observer) = observer
                    && let Some(node) = frame.node()
                {
                    observer(fraction, &String::from_utf16_lossy(&node.identity_name()));
                }
            }
            match frame {
                Frame::Enter(node) => {
                    // Covers topology changes since the initial read. As with
                    // other graph preparation, concurrent topology edits are
                    // not an atomic transaction and their ordering is unproved.
                    if let Some(state) = &mut original_sequence
                        && let Some(image) = state.image(node.id, geometry)
                    {
                        images.push(Some(image));
                        continue;
                    }
                    if let Some(final_render) = final_render.as_deref_mut() {
                        if let Some(image) = final_render.lookup(&node, geometry) {
                            images.push(Some(image));
                            continue;
                        }
                        match final_render.image_shortcut(&node) {
                            final_render::FinalImageShortcut::Uniform(constants) => {
                                let image = SharedImageMap::new(geometry);
                                for (index, value) in constants.into_iter().enumerate() {
                                    image.set_constant(index as i32, value)?;
                                }
                                image.finish_graph_render()?;
                                final_render.record(&node, &image);
                                images.push(Some(image));
                                continue;
                            }
                            final_render::FinalImageShortcut::Slot(index) => {
                                if !active.insert(node.id) {
                                    return Err(PointwiseError::UnsupportedCapability(
                                        "cyclic final graph scheduling",
                                    ));
                                }
                                let child = node.slot(index as i32)?.node();
                                stack.push(Frame::LeaveGroup(node, false, false));
                                stack.push(
                                    child.map_or(Frame::LeaveEmpty(width, height), Frame::Enter),
                                );
                                continue;
                            }
                            final_render::FinalImageShortcut::Kernel => {}
                        }
                        if node.uv_texture_output().is_some() && node.uv_bakes_map(time)? {
                            if !active.insert(node.id) {
                                return Err(PointwiseError::UnsupportedCapability(
                                    "cyclic UV texture image scheduling",
                                ));
                            }
                            stack.push(Frame::PrepareUvTexture(node, false));
                            continue;
                        }
                        if node.is_scene_node() {
                            stack.push(Frame::FinishFinalSceneImage(node.clone()));
                            stack.push(Frame::EnterFinalScene(node));
                            continue;
                        }
                    } else if node.kind().is_some_and(PointwiseKind::is_group) {
                        validate_group_render(&node)?;
                    }
                    if node.kind() == Some(PointwiseKind::Reference) && final_render.is_none() {
                        validate_reference_render(&node)?;
                    }
                    if node.kind() == Some(PointwiseKind::Reference) {
                        let property = node
                            .filter()
                            .ok_or(PointwiseError::InvalidCast)?
                            .parameter(0)?;
                        if property.axis_is_animated(0)?
                            && let Some(image) = node.cached_image()
                        {
                            images.push(Some(image));
                            continue;
                        }
                    }
                    if node.kind() == Some(PointwiseKind::Reference)
                        && let Some(target) = match &final_render {
                            Some(final_render) => {
                                final_render.reference_target(&node, time, false)?
                            }
                            None => node.reference_target()?,
                        }
                    {
                        if let Some(output @ RenderedImage::Animated(_)) =
                            target.cached_rendered_output()
                        {
                            // ark.k8 unwraps the target sequence before its
                            // width comparison. An out-of-range index is caught
                            // by the public Original renderer.
                            let time = context.map_or(0., |(time, _)| time);
                            let Some(frame) = output.frame_at_time(time) else {
                                caught.push(PointwiseError::Index);
                                images.push(Some(SharedImageMap::new(ImageGeometry::new(
                                    width, height,
                                ))));
                                continue;
                            };
                            if !active.insert(node.id) {
                                return Err(PointwiseError::UnsupportedCapability(
                                    "cyclic source graph scheduling",
                                ));
                            }
                            images.push(Some(frame));
                            stack.push(Frame::LeaveReference(node));
                            continue;
                        }
                        if !active.insert(node.id) {
                            return Err(PointwiseError::UnsupportedCapability(
                                "cyclic source graph scheduling",
                            ));
                        }
                        // zl.o supplies the Group's prepared slot images to
                        // the References in its contents. Reuse them within
                        // this Group evaluation without publishing a cache.
                        let prepared = if final_render.is_none() {
                            node.reference_group()?
                                .and_then(|group| prepared_inputs.get(&group.id))
                                .and_then(|images| images.get(&target.id))
                                .cloned()
                        } else {
                            None
                        };
                        stack.push(Frame::LeaveReference(node));
                        if let Some(image) = prepared {
                            images.push(Some(image));
                        } else {
                            stack.push(Frame::Enter(target));
                        }
                        continue;
                    }
                    if let Some(final_render) = final_render.as_deref_mut()
                        && node.kind() == Some(PointwiseKind::Reference)
                    {
                        let image = SharedImageMap::new(geometry);
                        image.finish_graph_render()?;
                        final_render.record(&node, &image);
                        images.push(Some(image));
                        continue;
                    }
                    if let Some(output) = node.cached_rendered_output() {
                        match output {
                            RenderedImage::Still(image) => images.push(Some(image)),
                            RenderedImage::Animated(_) if node.same_reference(self) => {
                                return Ok(PointwiseRenderOutcome {
                                    image: Some(output),
                                    caught,
                                });
                            }
                            RenderedImage::Animated(_) => {
                                return Err(PointwiseError::UnsupportedCapability(
                                    "animated cached image in Original graph input",
                                ));
                            }
                        }
                        continue;
                    }
                    if node.kind().is_some_and(PointwiseKind::is_group) {
                        if !active.insert(node.id) {
                            return Err(PointwiseError::UnsupportedCapability(
                                "cyclic source graph scheduling",
                            ));
                        }
                        let skip = group_skips_color(&node, options)?;
                        let channels = options.is_channel_rendering();
                        if final_render.is_some() {
                            stack.push(Frame::EnterFinalGroup(
                                node,
                                FinalGroupOutput::Image { skip, channels },
                            ));
                            continue;
                        }
                        let dependencies = group_image_dependencies(&node)?
                            .unwrap_or_default()
                            .into_iter()
                            .map(|index| Ok((index, node.slot(index as i32)?.node())))
                            .collect::<Result<Vec<_>, PointwiseError<E>>>()?;
                        stack.push(Frame::PrepareGroup(
                            node.clone(),
                            skip,
                            channels,
                            dependencies.clone(),
                        ));
                        for (_, child) in dependencies.into_iter().rev() {
                            stack
                                .push(child.map_or(Frame::LeaveEmpty(width, height), Frame::Enter));
                        }
                        continue;
                    }
                    if !node.contents().is_empty() && node.kind() != Some(PointwiseKind::Canvas) {
                        return Err(PointwiseError::UnsupportedCapability(
                            "Contents image/scene evaluation context",
                        ));
                    }
                    if !active.insert(node.id) {
                        return Err(PointwiseError::UnsupportedCapability(
                            "cyclic source graph scheduling",
                        ));
                    }
                    // zl.i/m5 for these concrete image nodes: no Contents or
                    // scene override; each slot requests its real Image. wo is
                    // unchanged across frames (it does not select dependencies).
                    // Cached nodes above bypass both traversal and finish.
                    // zl.o renders required children before entering the node's
                    // k8. aip's Iterations-before-g ordering is local to k8.
                    // A scene input is evaluated as an nt scene, not as an
                    // already-rendered image. Its owner performs that traversal.
                    let slots = if matches!(
                        &node.core().payload,
                        Payload::SceneClearLights
                            | Payload::SceneCombine
                            | Payload::SceneTransform { .. }
                            | Payload::SceneImport { .. }
                            | Payload::UvTexture { .. }
                    ) {
                        Vec::new()
                    } else {
                        node.slots()
                    };
                    let uses_fallbacks = matches!(
                        node.kind(),
                        Some(
                            PointwiseKind::OklabColor
                                | PointwiseKind::Shader
                                | PointwiseKind::Canvas
                                | PointwiseKind::Style
                                | PointwiseKind::Synthesis
                        )
                    );
                    stack.push(Frame::Leave(node.clone(), slots.len()));
                    let synthesis_layer = options.synthesis_layer_for(node.id);
                    for (index, slot) in slots.into_iter().enumerate().rev() {
                        let uses_slot = match synthesis_layer {
                            Some(layer) => {
                                index == 0
                                    && node
                                        .filter()
                                        .and_then(|filter| filter.synthesis_texture())
                                        .is_some_and(|texture| texture.layer_uses_input(layer))
                            }
                            None => node.uses_image_slot(index),
                        };
                        let child = uses_slot.then(|| slot.node()).flatten();
                        match child {
                            Some(child) => stack.push(Frame::Enter(child)),
                            None if uses_fallbacks => stack.push(Frame::LeaveDisconnected),
                            None => {
                                // Missing entries need their own ordered stack result; use a
                                // real empty image leaf instead of an invented cached node.
                                stack.push(Frame::LeaveEmpty(width, height));
                            }
                        }
                    }
                }
                Frame::PrepareGroup(node, skip, channels, dependencies) => {
                    let prepared = images.split_off(images.len() - dependencies.len());
                    let first = if skip {
                        dependencies
                            .iter()
                            .position(|(index, _)| *index == 0)
                            .and_then(|index| prepared[index].clone())
                    } else {
                        None
                    };
                    prepared_inputs.insert(
                        node.id,
                        dependencies
                            .into_iter()
                            .zip(prepared)
                            .filter_map(|((_, child), image)| Some((child?.id, image?)))
                            .collect(),
                    );
                    let content = node.content(0)?.node();
                    stack.push(Frame::LeaveGroup(node.clone(), skip, channels));
                    stack.push(content.map_or(Frame::LeaveEmpty(width, height), Frame::Enter));
                    if skip {
                        if let Some(first) = first {
                            images.push(Some(first));
                        } else {
                            stack.push(
                                node.slot(0)?
                                    .node()
                                    .map_or(Frame::LeaveEmpty(width, height), Frame::Enter),
                            );
                        }
                    }
                }
                Frame::EnterFinalGroup(node, output) => {
                    let state = final_render.as_deref_mut().expect("final Group frame");
                    let demands = state.group_slot_demands(
                        &node,
                        matches!(output, FinalGroupOutput::Scene),
                        geometry,
                        time,
                        options,
                        effects.cancel_check(),
                    )?;
                    let before_inputs = stack.len();
                    for (index, demand) in demands.iter().enumerate().rev() {
                        let child = node.slot(index as i32)?.node();
                        if demand.scene {
                            stack.push(
                                child
                                    .clone()
                                    .map_or(Frame::EmptyFinalScene, Frame::EnterFinalScene),
                            );
                        }
                        if demand.image {
                            stack
                                .push(child.map_or(Frame::LeaveEmpty(width, height), Frame::Enter));
                        }
                    }
                    stack.insert(
                        before_inputs,
                        Frame::PrepareFinalGroupContents(node, output, demands),
                    );
                }
                Frame::PrepareFinalGroupContents(node, output, demands) => {
                    let image_count = demands.iter().filter(|demand| demand.image).count();
                    let scene_count = demands.iter().filter(|demand| demand.scene).count();
                    let exterior = images.split_off(images.len() - image_count);
                    scenes.truncate(scenes.len() - scene_count);
                    final_render
                        .as_deref_mut()
                        .expect("final Group frame")
                        .group_inputs_prepared(&node, demands);
                    // zl.o visits only Contents[0] in the still render mode
                    // used by the final clone, for both image and Model output.
                    let content = node.content(0)?.node();
                    match output {
                        FinalGroupOutput::Image { skip, channels } => {
                            if skip {
                                // Slot zero is required for a skipping Group's
                                // image and is the first exterior image result.
                                let first = exterior
                                    .into_iter()
                                    .next()
                                    .flatten()
                                    .ok_or(PointwiseError::Index)?;
                                images.push(Some(first));
                            }
                            stack.push(Frame::LeaveGroup(node, skip, channels));
                            stack.push(
                                content.map_or(Frame::LeaveEmpty(width, height), Frame::Enter),
                            );
                        }
                        FinalGroupOutput::Scene => {
                            stack.push(Frame::ForwardFinalScene(node));
                            stack.push(
                                content.map_or(Frame::EmptyFinalScene, Frame::EnterFinalScene),
                            );
                        }
                    }
                }
                Frame::LeaveReference(node) => {
                    let image = images
                        .pop()
                        .flatten()
                        .ok_or(PointwiseError::NullReference)?;
                    let image = if image.geometry().width() != width {
                        let execution = context
                            .map(|(_, execution)| execution)
                            .unwrap_or(crate::ProceduralExecution::CallingThread);
                        super::image_resize::resize(&image, width, height, execution)?
                    } else {
                        image
                    };
                    image.finish_graph_render()?;
                    if let Some(final_render) = final_render.as_deref_mut() {
                        final_render.record(&node, &image);
                    } else if caught.is_empty()
                        && let Some(state) = &mut original_sequence
                    {
                        state.record(&node, &image);
                    }
                    images.push(Some(image));
                    active.remove(&node.id);
                }
                Frame::LeaveGroup(node, skip, channels) => {
                    prepared_inputs.remove(&node.id);
                    let mut inputs = images.split_off(images.len() - if skip { 2 } else { 1 });
                    let content = inputs
                        .pop()
                        .flatten()
                        .ok_or(PointwiseError::NullReference)?;
                    let image = if skip {
                        let first = inputs
                            .pop()
                            .flatten()
                            .ok_or(PointwiseError::NullReference)?;
                        if channels {
                            merge_group_color_and_channels(geometry, &first, Some(&content))?
                        } else {
                            first
                        }
                    } else {
                        content
                    };
                    // Final planar optics can forward Base without entering its
                    // kernel. Apply the same rectangular grid in that case.
                    let image = if width != height
                        && matches!(
                            node.kind(),
                            Some(PointwiseKind::Reflect | PointwiseKind::Refract)
                        ) {
                        let execution = context.map_or(
                            crate::ProceduralExecution::CallingThread,
                            |(_, execution)| execution,
                        );
                        super::image_resize::resize(&image, width, height, execution)?
                    } else {
                        image
                    };
                    image.finish_graph_render()?;
                    if let Some(final_render) = final_render.as_deref_mut() {
                        final_render.record(&node, &image);
                    } else if caught.is_empty()
                        && let Some(state) = &mut original_sequence
                    {
                        state.record(&node, &image);
                    }
                    images.push(Some(image));
                    active.remove(&node.id);
                }
                Frame::Leave(node, count) => {
                    let inputs = images.split_off(images.len() - count);
                    node.validate_supplied_graph_frame()?;
                    let rendered = node.render_supplied_graph_frame(
                        &inputs, width, height, context, options, effects,
                    );
                    let rendered = rendered.and_then(|image| {
                        super::check_render_cancelled(effects.cancel_check())?;
                        let image = image.ok_or(PointwiseError::NullReference)?;
                        image.finish_graph_render()?;
                        Ok(Some(image))
                    });
                    let image = match rendered {
                        Ok(image) => image,
                        Err(PointwiseError::Cancelled) => return Err(PointwiseError::Cancelled),
                        // An unimplemented target operation is not an original
                        // exception. Preserve it even when a child owner wraps it.
                        Err(error) if error.is_unsupported_capability() => return Err(error),
                        Err(error @ PointwiseError::Image(crate::ImageError::Allocation)) => {
                            return Err(error);
                        }
                        Err(error @ PointwiseError::Scene(crate::SceneError::Allocation)) => {
                            return Err(error);
                        }
                        Err(error) if final_render.is_some() => return Err(error),
                        Err(error) => {
                            caught.push(error);
                            Some(SharedImageMap::new(ImageGeometry::new(width, height)))
                        }
                    };
                    // zl.o does not publish original-mode RenderedImageMap. Child
                    // caches keep their actual identity and aliasing unchanged.
                    if let Some(final_render) = final_render.as_deref_mut()
                        && let Some(image) = &image
                    {
                        final_render.record(&node, image);
                    } else if caught.is_empty()
                        && let Some(state) = &mut original_sequence
                        && let Some(image) = &image
                    {
                        state.record(&node, image);
                    }
                    images.push(image);
                    active.remove(&node.id);
                }
                Frame::LeaveEmpty(w, h) => {
                    images.push(Some(SharedImageMap::new(ImageGeometry::new(w, h))))
                }
                Frame::LeaveDisconnected => images.push(None),
                Frame::EmptyFinalScene => scenes.push(None),
                Frame::EnterFinalScene(node) => {
                    let state = final_render.as_deref_mut().expect("final scene frame");
                    if let Some(scene) = state.scene(&node) {
                        scenes.push(scene);
                        continue;
                    }
                    if !active_scenes.insert(node.id) {
                        return Err(PointwiseError::UnsupportedCapability(
                            "cyclic final scene scheduling",
                        ));
                    }
                    if node.uv_texture_output().is_some() {
                        if node.uv_bakes_map(time)? {
                            stack.push(Frame::PrepareFinalSceneFromImage(node.clone()));
                            stack.push(Frame::Enter(node));
                        } else {
                            stack.push(Frame::PrepareUvTexture(node, true));
                        }
                    } else if node.is_scene_node() {
                        let slots = node.slots();
                        stack.push(Frame::LeaveFinalScene(node, slots.len()));
                        for slot in slots.into_iter().rev() {
                            stack.push(
                                slot.node()
                                    .map_or(Frame::EmptyFinalScene, Frame::EnterFinalScene),
                            );
                        }
                    } else if node.kind().is_some_and(PointwiseKind::is_group)
                        && matches!(
                            state.image_shortcut(&node),
                            final_render::FinalImageShortcut::Kernel
                        )
                        && node.content(0)?.node().is_some()
                    {
                        stack.push(Frame::EnterFinalGroup(node, FinalGroupOutput::Scene));
                    } else if let Some(target) = state.scene_forward_input(&node, time)? {
                        stack.push(Frame::ForwardFinalScene(node));
                        stack.push(Frame::EnterFinalScene(target));
                    } else {
                        stack.push(Frame::PrepareFinalSceneFromImage(node.clone()));
                        stack.push(Frame::Enter(node));
                    }
                }
                Frame::PrepareFinalSceneFromImage(node) => {
                    let image = images
                        .pop()
                        .flatten()
                        .ok_or(PointwiseError::NullReference)?;
                    let state = final_render.as_deref_mut().expect("final scene frame");
                    if let Some(target) = state.scene_forward_input(&node, time)? {
                        stack.push(Frame::ForwardFinalScene(node));
                        stack.push(Frame::EnterFinalScene(target));
                    } else {
                        let scene = effects.image_as_scene(&image)?;
                        state.record_scene(&node, Some(scene.clone()));
                        scenes.push(Some(scene));
                        active_scenes.remove(&node.id);
                    }
                }
                Frame::ForwardFinalScene(node) => {
                    let scene = scenes.pop().ok_or(PointwiseError::InvalidOperation)?;
                    final_render
                        .as_deref_mut()
                        .expect("final scene frame")
                        .record_scene(&node, scene.clone());
                    scenes.push(scene);
                    active_scenes.remove(&node.id);
                }
                Frame::LeaveFinalScene(node, count) => {
                    let children = scenes.split_off(scenes.len() - count);
                    let scene = if node.is_scene_generator() {
                        effects.generate_scene(&node, time)?
                    } else {
                        effects.modify_prepared_scene(&node, children.into_iter().map(Ok), time)?
                    };
                    super::check_render_cancelled(effects.cancel_check())?;
                    final_render
                        .as_deref_mut()
                        .expect("final scene frame")
                        .record_scene(&node, scene.clone());
                    scenes.push(scene);
                    active_scenes.remove(&node.id);
                }
                Frame::PrepareUvTexture(node, textured) => {
                    let model = node.slot(0)?.node();
                    let has_texture = node.uv_has_texture_input();
                    let texture = has_texture.then(|| node.slot(1)).transpose()?.map(|slot| {
                        slot.node()
                            .map_or(Frame::LeaveEmpty(width, height), Frame::Enter)
                    });
                    let geometry_only = node.uv_uses_geometry_only();
                    stack.push(Frame::FinishUvTexture(node, textured));
                    if let Some(texture) = texture {
                        stack.push(texture);
                    }
                    if !geometry_only {
                        stack.push(model.map_or(Frame::EmptyFinalScene, Frame::EnterFinalScene));
                    } else {
                        let state = final_render.as_deref().expect("UV geometry final frame");
                        let scene = model
                            .map(|model| {
                                effects.geometry_scene(model, time, |node| {
                                    state.reference_target(node, time, true)
                                })
                            })
                            .transpose()?
                            .flatten();
                        scenes.push(scene);
                    }
                }
                Frame::FinishUvTexture(node, textured) => {
                    let model = scenes.pop().ok_or(PointwiseError::InvalidOperation)?;
                    let texture = node
                        .uv_has_texture_input()
                        .then(|| {
                            images
                                .pop()
                                .flatten()
                                .ok_or(PointwiseError::InvalidOperation)
                        })
                        .transpose()?;
                    let state = final_render.as_deref_mut().expect("UV texture final frame");
                    if textured {
                        let scene = effects.texture_uv_scene(
                            &node,
                            time,
                            model,
                            texture.as_ref(),
                            geometry,
                            options,
                        )?;
                        state.record_scene(&node, scene.clone());
                        scenes.push(scene);
                        active_scenes.remove(&node.id);
                    } else {
                        let image = effects.bake_uv_texture(
                            &node,
                            time,
                            model.as_ref(),
                            texture.as_ref(),
                            geometry,
                        )?;
                        image.finish_graph_render()?;
                        state.record(&node, &image);
                        images.push(Some(image));
                        active.remove(&node.id);
                    }
                }
                Frame::FinishFinalSceneImage(node) => {
                    let scene = scenes.pop().ok_or(PointwiseError::InvalidOperation)?;
                    // aqr.m5 requests scenes and textures for Model inputs.
                    // An explicit image demand is what calls z8.k8.
                    let image =
                        effects.rasterize_node_scene(&node, time, scene.as_ref(), geometry)?;
                    super::check_render_cancelled(effects.cancel_check())?;
                    image.finish_graph_render()?;
                    final_render
                        .as_deref_mut()
                        .expect("final scene frame")
                        .record(&node, &image);
                    images.push(Some(image));
                }
            }
        }
        super::check_render_cancelled(effects.cancel_check())?;
        Ok(PointwiseRenderOutcome {
            image: images
                .pop()
                .ok_or(PointwiseError::InvalidOperation)?
                .map(RenderedImage::Still),
            caught,
        })
    }
    fn validate_supplied_graph_frame(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        match &self.core().payload {
            Payload::Filter { kind, .. }
                if !matches!(kind, PointwiseKind::Reference | PointwiseKind::OklabColor) =>
            {
                self.filter()
                    .ok_or(PointwiseError::NullReference)?
                    .ensure_property_evaluation_supported()
            }
            Payload::Invert { .. } => self.ensure_invert_evaluation_supported(true),
            Payload::ImageGenerator { .. } => self
                .image_generator_handle()
                .ok_or(PointwiseError::NullReference)?
                .ensure_property_evaluation_supported(),
            _ => Ok(()),
        }
    }

    fn render_supplied_graph_frame(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
        context: Option<(f64, crate::ProceduralExecution)>,
        options: PointwiseRenderOptions,
        effects: &mut super::PointwiseRenderEffects<'_, E>,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>>
    where
        E: 'static,
    {
        match &self.core().payload {
            Payload::ScenePrimitive { .. }
            | Payload::SceneImport { .. }
            | Payload::SceneClearLights
            | Payload::SceneCombine
            | Payload::SceneTransform { .. }
            | Payload::UvTexture { .. } => {
                let time = context.map_or(0.0, |(time, _)| time);
                let execution = context.map_or(
                    crate::ProceduralExecution::CallingThread,
                    |(_, execution)| execution,
                );
                effects
                    .render_scene_node(self, width, height, time, execution, options)
                    .map(Some)
            }
            Payload::Input { image, .. } => {
                let image = guard(image).clone();
                match image {
                    None => Ok(Some(SharedImageMap::new(ImageGeometry::new(width, height)))),
                    Some(image) => {
                        let execution = context
                            .map(|(_, e)| e)
                            .unwrap_or(crate::ProceduralExecution::CallingThread);
                        super::image_resize::resize(&image, width, height, execution).map(Some)
                    }
                }
            }
            Payload::Filter { .. } => {
                let filter = self.filter().ok_or(PointwiseError::NullReference)?;
                if filter.kind() == PointwiseKind::WizardToneCurve
                    && guarantees::wizard_passthrough(&filter)? == 0
                {
                    Ok(Some(inputs.first().and_then(Clone::clone).unwrap_or_else(
                        || SharedImageMap::new(ImageGeometry::new(width, height)),
                    )))
                } else if filter.kind() == PointwiseKind::Reference {
                    if self.reference_target()?.is_none() {
                        Ok(Some(SharedImageMap::new(ImageGeometry::new(width, height))))
                    } else {
                        Err(PointwiseError::UnsupportedCapability(
                            "Reference group scheduling",
                        ))
                    }
                } else {
                    let (time, execution) =
                        context.unwrap_or((0.0, crate::ProceduralExecution::CallingThread));
                    filter.render_supplied_at_time_with_effects(
                        inputs, width, height, time, execution, options, effects,
                    )
                }
            }
            Payload::Invert { .. } => self
                .render_invert_graph_frame(
                    inputs.first().and_then(Option::as_ref),
                    width,
                    height,
                    context.map_or(0.0, |(time, _)| time),
                )
                .map(Some),
            Payload::ImageGenerator { .. } => {
                let generator = self
                    .image_generator_handle()
                    .ok_or(PointwiseError::NullReference)?;
                if generator.kind() == super::ImageGeneratorKind::Gradient {
                    let (time, execution) =
                        context.unwrap_or((0.0, crate::ProceduralExecution::CallingThread));
                    generator.render_gradient(
                        width,
                        height,
                        time,
                        execution,
                        effects.cancel_check(),
                    )
                } else if generator.kind() == super::ImageGeneratorKind::ImportedImage {
                    let (time, execution) =
                        context.unwrap_or((0.0, crate::ProceduralExecution::CallingThread));
                    generator.render_imported(width, height, time, options, execution)
                } else {
                    match context {
                        Some((time, execution)) => generator.render_with_preflight(
                            width,
                            height,
                            time,
                            execution,
                            &mut |w, q| effects.preflight(w, q),
                        ),
                        None => generator.render_unbound(width, height, 0.0),
                    }
                }
                .map(Some)
            }
            Payload::Solid { model, .. } => model
                .render_with_bindings(
                    width,
                    height,
                    context.map_or(0.0, |(time, _)| time),
                    |kind| {
                        self.animated_solid_binding_value(
                            kind,
                            context.map_or(0.0, |(time, _)| time),
                        )
                    },
                )
                .map(|image| Some(SharedImageMap::from_image(image)))
                .map_err(|error| match error {
                    crate::LiveSolidColorError::Property(error)
                        if !error.is_unsupported_capability() =>
                    {
                        error
                    }
                    crate::LiveSolidColorError::Image(error) => PointwiseError::Image(error),
                    _ => PointwiseError::UnsupportedCapability(
                        "Solid Color evaluated property dependency",
                    ),
                }),
        }
    }

    pub fn source_clone(&self) -> Result<Self, PointwiseError<E>>
    where
        E: 'static,
    {
        // No memoization: qi(qi,zl) invokes child.ws() for EACH occurrence.
        fn copy<E: 'static>(
            source: &PointwiseNode<E>,
            active: &mut BTreeSet<NodeId>,
        ) -> Result<PointwiseNode<E>, PointwiseError<E>> {
            if !active.insert(source.id) {
                return Err(PointwiseError::UnsupportedCapability(
                    "cyclic source graph cloning",
                ));
            }
            let mut children = Vec::new();
            for (index, slot) in source.slots().into_iter().enumerate() {
                if !source.is_clone_mode() && source.is_filter() && index < 3 {
                    children.push(None);
                    continue;
                }
                children.push(match slot.node() {
                    Some(child) => Some(copy(&child, active)?),
                    None => None,
                });
            }
            let mut contents = Vec::new();
            for occurrence in source.contents() {
                let child = occurrence
                    .node()
                    .as_ref()
                    .map(|child| copy(child, active))
                    .transpose()?;
                contents.push((occurrence.name(), child));
            }
            let target = match &source.core().payload {
                Payload::Input { image, .. } => {
                    let target = PointwiseNode::input();
                    target.set_input_image(guard(image).clone())?;
                    target
                }
                Payload::SceneClearLights => PointwiseNode::publish_scene_clear_lights(),
                Payload::SceneCombine => PointwiseNode::publish_scene_combine(),
                Payload::UvTexture { kind, properties } => PointwiseNode::publish_uv_texture(
                    *kind,
                    properties
                        .iter()
                        .map(|property| property.copied(true, None))
                        .collect::<Result<_, _>>()?,
                ),
                Payload::SceneTransform { .. } => source
                    .scene_transform_3d()
                    .ok_or(PointwiseError::InvalidCast)?
                    .source_clone()?
                    .node(),
                Payload::ScenePrimitive { .. } => source
                    .scene_primitive()
                    .ok_or(PointwiseError::InvalidCast)?
                    .source_clone()?
                    .node(),
                Payload::SceneImport { state } => {
                    state.copy(guard(&state.mesh).clone(), true, None)?
                }
                Payload::Filter { .. } => source
                    .filter()
                    .ok_or(PointwiseError::NullReference)?
                    .copy_local()?
                    .node(),
                Payload::Invert { model, wire, .. } => PointwiseNode::publish_invert_with_wire(
                    model.source_clone(),
                    wire.source_copy()?,
                ),
                Payload::ImageGenerator { .. } => source
                    .image_generator_handle()
                    .ok_or(PointwiseError::NullReference)?
                    .copy_local()?
                    .node(),
                Payload::Solid { model, .. } => PointwiseNode::publish_solid(
                    model
                        .clone_unadjusted_for_graph()
                        .map_err(|_| {
                            PointwiseError::UnsupportedCapability("Solid Color copy dependency")
                        })?
                        .ok_or(PointwiseError::UnsupportedCapability(
                            "adjusted Solid Color copy",
                        ))?,
                ),
            };
            if let Some(program) = source.shader_program() {
                target.load_shader_program(program);
            }
            // z7.ws constructs the concrete z7 copy; zl copy does not copy bl.
            guard(&target.core().links).default_input = source.is_default_input();
            // zl copy retains ag only when source.bl is false (the ark
            // exclusion is outside current payload families). No handlers copy.
            if !source.is_filter() {
                if let (Some(source), Some(target)) = (
                    source.gradient_generator_handle(),
                    target.gradient_generator_handle(),
                ) {
                    target.copy_image_caches_from(&source);
                } else {
                    target.set_rendered_output(source.cached_rendered_output())?;
                    let preview = guard(&source.core().mode_events).timed_images[0].clone();
                    guard(&target.core().mode_events).timed_images[0] = preview;
                }
            }
            for (child, dst) in children.into_iter().zip(target.slots()) {
                if let Some(child) = child {
                    dst.set_node_quiet(Some(&child))?;
                }
            }
            for (name, child) in contents {
                let destination = target.append_content(name);
                destination.set_node_quiet(child.as_ref())?;
            }
            active.remove(&source.id);
            Ok(target)
        }
        copy(self, &mut BTreeSet::new())
    }
}
impl<E> PointwiseSlot<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.core, &other.core)
    }
    pub fn parent(&self) -> PointwiseNode<E> {
        self.owner.clone()
    }
    pub fn name(&self) -> Vec<u16> {
        self.core.name.clone()
    }
    pub fn node(&self) -> Option<PointwiseNode<E>> {
        guard(&self.core.child).map(|id| self.owner.at(id))
    }
    pub fn set_node(&self, node: Option<&PointwiseNode<E>>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.assign(node, true)
    }
    pub fn set_node_quiet(&self, node: Option<&PointwiseNode<E>>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.assign(node, false)
    }
    fn assign(&self, node: Option<&PointwiseNode<E>>, notify: bool) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        if self.node().as_ref().map(|n| n.id) == node.map(|n| n.id) {
            return Ok(());
        }
        let history_slot = self.owner.child_history_slot(self)?;
        if history_slot.is_none()
            && node.is_some_and(PointwiseNode::has_live_property_history_binding)
        {
            return Err(PointwiseError::UnsupportedCapability(
                "property history child/group ownership",
            ));
        }
        let source = history_slot
            .as_ref()
            .map(|slot| slot.prepare_input(&self.owner, node))
            .transpose()?
            .flatten();
        let previous_sibling = match (&history_slot, &source) {
            (Some(dest), Some(source)) if !dest.same_reference(source) => source
                .graph_occurrence()?
                .filter(|input| input.parent().same_reference(&self.owner)),
            _ => None,
        };
        if let Some(node) = node {
            self.owner.sponsor.merge(&node.sponsor)
        }
        // qi.Node publishes b before setting new Parent. Do not undo partial
        // publication if the old-parent detach/callback path later fails.
        *guard(&self.core.child) = node.map(|n| n.id);
        if let Some(slot) = history_slot {
            slot.publish_input(source.as_ref(), node);
        }
        if let Some(previous_sibling) = previous_sibling {
            // qi changes within the same parent do not invoke zl.Parent's
            // old-parent detach. Moving the editor wrapper owns that detach.
            previous_sibling.set_node(None)?;
        }
        if let Some(node) = node {
            node.assign_parent(Some(&self.owner))?;
        }
        if notify {
            self.owner.notify_slot_change(self)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "slot_occurrence_test.rs"]
mod slot_occurrence_test;

#[derive(Clone)]
struct ShaderLayout {
    program: Option<Arc<super::ShaderProgram>>,
    slots: Vec<Arc<SlotCore>>,
}

impl<E: 'static> PointwiseNode<E> {
    pub fn shader_program(&self) -> Option<Arc<super::ShaderProgram>> {
        if self.kind() != Some(PointwiseKind::Shader) {
            return None;
        }
        static DEFAULT: std::sync::OnceLock<Arc<super::ShaderProgram>> = std::sync::OnceLock::new();
        let current = guard(&self.core().links).shader.clone();
        Some(current.unwrap_or_else(|| {
            DEFAULT
                .get_or_init(|| {
                    Arc::new(
                        super::ShaderProgram::compile(super::DEFAULT_SHADER_SOURCE)
                            .expect("the built-in Shader source is valid"),
                    )
                })
                .clone()
        }))
    }

    fn shader_layout(
        &self,
        program: Arc<super::ShaderProgram>,
        slots: &[Arc<SlotCore>],
    ) -> ShaderLayout {
        ShaderLayout {
            slots: program
                .inputs()
                .iter()
                .map(|input| {
                    slots
                        .iter()
                        .find(|slot| slot.name.iter().copied().eq(input.name().encode_utf16()))
                        .cloned()
                        .unwrap_or_else(|| {
                            SlotCore::new(self.id, input.name().encode_utf16().collect(), None)
                        })
                })
                .collect(),
            program: Some(program),
        }
    }

    pub(super) fn load_shader_program(&self, program: Arc<super::ShaderProgram>) {
        let core = self.core();
        let mut links = guard(&core.links);
        let layout = self.shader_layout(program, &links.slots);
        links.shader = layout.program;
        links.slots = layout.slots;
    }

    fn exchange_shader_layout(&self, expected: &ShaderLayout, restored: &ShaderLayout) -> bool {
        let core = self.core();
        let mut links = guard(&core.links);
        let unchanged = match (&links.shader, &expected.program) {
            (Some(current), Some(expected)) => Arc::ptr_eq(current, expected),
            (None, None) => true,
            _ => false,
        };
        if !unchanged {
            return false;
        }
        links.shader = restored.program.clone();
        links.slots.clone_from(&restored.slots);
        true
    }

    pub fn edit_shader_program(
        &self,
        program: Arc<super::ShaderProgram>,
    ) -> Result<bool, PointwiseError<E>> {
        let previous = self.shader_program().ok_or(PointwiseError::InvalidCast)?;
        if previous.source() == program.source() {
            return Ok(false);
        }
        let before = {
            let core = self.core();
            let links = guard(&core.links);
            ShaderLayout {
                program: links.shader.clone(),
                slots: links.slots.clone(),
            }
        };
        let after = self.shader_layout(program, &before.slots);
        let history_inputs = self.shader_history_inputs(&before.slots, &after.slots);
        if !self.exchange_shader_layout(&before, &after) {
            return Err(PointwiseError::InvalidOperation);
        }
        if let Some((_, updated)) = history_inputs.as_ref() {
            updated.replace();
        }
        if let Some((slot, history)) = self
            .texture_node_logic()
            .and_then(|logic| logic.slot())
            .and_then(|slot| slot.history().map(|history| (slot, history)))
        {
            let sequence = history.edit_sequence();
            if let Err(error) = history.record_graph_edit(
                slot.history_target(),
                "Edit Shader code".encode_utf16().collect(),
                Box::new(ShaderEdit {
                    owner: Arc::downgrade(&self.core()),
                    before: before.clone(),
                    after: after.clone(),
                    history_inputs: history_inputs.clone(),
                }),
            ) {
                if sequence == history.edit_sequence() {
                    self.exchange_shader_layout(&after, &before);
                    if let Some((original, _)) = history_inputs {
                        original.replace();
                    }
                }
                return Err(PointwiseError::History(Box::new(error)));
            }
        }
        self.notify_property_change_with(crate::ContextChangeArgument::shared_empty())?;
        Ok(true)
    }
}

struct ShaderEdit<E> {
    owner: std::sync::Weak<NodeCore<E>>,
    before: ShaderLayout,
    after: ShaderLayout,
    history_inputs: Option<(
        super::history_binding::ShaderHistoryInputs<E>,
        super::history_binding::ShaderHistoryInputs<E>,
    )>,
}

impl<E: 'static> crate::history::HistoryGraphEdit<E> for ShaderEdit<E> {
    fn apply(&self, direction: crate::HistoryDirection) -> Result<(), crate::HistoryError<E>> {
        let owner = self
            .owner
            .upgrade()
            .ok_or(crate::HistoryError::BranchChanged)?;
        let sponsor =
            super::region::Sponsor::for_node(&owner).ok_or(crate::HistoryError::BranchChanged)?;
        let node = PointwiseNode {
            id: owner.id,
            sponsor,
        };
        let (expected, restored) = match direction {
            crate::HistoryDirection::Undo => (&self.after, &self.before),
            crate::HistoryDirection::Redo => (&self.before, &self.after),
        };
        if !node.exchange_shader_layout(expected, restored) {
            return Err(crate::HistoryError::BranchChanged);
        }
        if let Some((before, after)) = &self.history_inputs {
            match direction {
                crate::HistoryDirection::Undo => before,
                crate::HistoryDirection::Redo => after,
            }
            .replace();
        }
        node.notify_property_change_with(crate::ContextChangeArgument::shared_empty())
            .map_err(|error| crate::HistoryError::Property(Box::new(error)))
    }
}
