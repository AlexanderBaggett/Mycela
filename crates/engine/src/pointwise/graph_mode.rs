//! Retained zl.bl / qi / Contents topology. Public mutations preserve source
//! publication and notification prefixes; callbacks never run under guards.
use super::*;
use crate::ordinary_events::{DelegateCore, HandlerCore};
#[path = "graph_defaults.rs"]
mod defaults;
pub use defaults::SharedDefaultInputMapping;
type NodeEvents<E> = DelegateCore<PointwiseNode<E>, Arc<crate::ContextChangeArgument<E>>, E>;
type RenderedImageEvent<E> = (NodeEvents<E>, Arc<crate::ContextChangeArgument<E>>);
type SlotEvents<E> = DelegateCore<PointwiseNode<E>, PointwiseSlotChange, E>;
type WizardContents<E> = (Vec<PointwiseSlot<E>>, Vec<[f32; 2]>);
pub struct PointwiseNodeChangeHandler<E = ()>(
    Arc<HandlerCore<PointwiseNode<E>, Arc<crate::ContextChangeArgument<E>>, E>>,
);
#[derive(Clone, Debug)]
pub struct PointwiseSlotChange(Arc<i32>);
impl PointwiseSlotChange {
    pub fn index(&self) -> i32 {
        *self.0
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
pub struct PointwiseSlotChangeHandler<E = ()>(
    Arc<HandlerCore<PointwiseNode<E>, PointwiseSlotChange, E>>,
);
impl<E> Clone for PointwiseNodeChangeHandler<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<E> Clone for PointwiseSlotChangeHandler<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<E> PointwiseNodeChangeHandler<E> {
    pub fn new(
        callback: impl Fn(&PointwiseNode<E>, Arc<crate::ContextChangeArgument<E>>) -> Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(HandlerCore {
            callback: Box::new(callback),
        }))
    }
}
impl<E> PointwiseSlotChangeHandler<E> {
    pub fn new(
        callback: impl Fn(&PointwiseNode<E>, PointwiseSlotChange) -> Result<(), E>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(HandlerCore {
            callback: Box::new(callback),
        }))
    }
}
type PreparationSnapshot = ([bool; 7], i32, [bool; 4], [f32; 4], [bool; 4]);
struct DefaultPreparation {
    state: PreparationSnapshot,
}
impl DefaultPreparation {
    fn initialize() -> Self {
        Self {
            state: (
                [true, true, false, false, false, false, false],
                -1,
                [false; 4],
                [0.0; 4],
                [false; 4],
            ),
        }
    }
}
pub(super) struct ModeEvents<E> {
    changed: NodeEvents<E>,
    rendered_image: Option<RenderedImageEvent<E>>,
    topology: SlotEvents<E>,
    prepared_default: Option<DefaultPreparation>,
    document_preview_samples: Option<crate::document_container::DocumentPreviewSamples>,
    pub(super) timed_images: [Option<(PointwiseRenderOptions, RenderedImage)>; 2],
    pub(super) scene_rendered_image: Option<RenderedImage>,
    pub(super) final_cache: super::super::generator::FinalImageCache,
    pub(super) final_graph_clone: bool,
    pub(super) preparation: super::guarantees::PreparationCore<E>,
    defer_wizard_change: bool,
    wizard_change: Option<DeferredWizardChange<E>>,
}
impl<E> ModeEvents<E> {
    pub(super) fn new() -> Self {
        Self {
            changed: DelegateCore::Absent,
            rendered_image: None,
            topology: DelegateCore::Absent,
            prepared_default: None,
            document_preview_samples: None,
            timed_images: [None, None],
            scene_rendered_image: None,
            final_cache: super::super::generator::FinalImageCache::default(),
            final_graph_clone: false,
            preparation: super::guarantees::PreparationCore::new(),
            defer_wizard_change: false,
            wizard_change: None,
        }
    }
}
enum Sender<E> {
    Node(PointwiseNode<E>),
    Slot(PointwiseSlot<E>),
    Property,
    GroupInputCount { old: f64, new: f64 },
}
struct DeferredWizardChange<E> {
    sender: Sender<E>,
    args: Arc<crate::ContextChangeArgument<E>>,
}
impl<E> PointwiseNode<E> {
    pub fn is_filter(&self) -> bool {
        guard(&self.core().links).is_filter
    }
    pub fn is_base_node(&self) -> bool {
        self.parent().is_none()
    }
    pub fn is_default_input(&self) -> bool {
        guard(&self.core().links).default_input
    }
    /// Actual inherited constructor identity, not a UI label or factory alias.
    pub fn identity_name(&self) -> Vec<u16> {
        let name = match &self.core().payload {
            Payload::Filter { kind, .. } => match kind {
                PointwiseKind::Add => "A + B",
                PointwiseKind::Multiply => "A * B",
                _ => kind.name(),
            },
            Payload::ImageGenerator { .. } => self
                .image_generator_handle()
                .expect("actual image-generator payload")
                .kind()
                .name(),
            Payload::Input { .. } => "Input",
            Payload::Invert { .. } => "Invert",
            Payload::Solid { .. } => "Solid Color",
            Payload::ScenePrimitive { state } => state.kind.name(),
            Payload::SceneImport { .. } => "Import 3D",
            Payload::SceneClearLights => "Clear Lights",
            Payload::SceneCombine => "Combine 3D",
            Payload::SceneTransform { .. } => "Transform 3D",
            Payload::UvTexture { kind, .. } => kind.name(),
        };
        name.encode_utf16().collect()
    }
    pub fn contents(&self) -> Vec<PointwiseSlot<E>> {
        guard(&self.core().links)
            .contents
            .clone()
            .into_iter()
            .map(|core| PointwiseSlot {
                owner: self.at(core.owner),
                core,
            })
            .collect()
    }
    pub fn content(&self, index: i32) -> Result<PointwiseSlot<E>, PointwiseError<E>> {
        self.contents()
            .get(usize::try_from(index).map_err(|_| PointwiseError::Index)?)
            .cloned()
            .ok_or(PointwiseError::Index)
    }
    /// Resolve ark.j through the nearest ancestor group whose contents contain
    /// the path to this node. A group containing the path in an input slot does
    /// not establish the reference context.
    pub fn reference_target(&self) -> Result<Option<Self>, PointwiseError<E>>
    where
        E: 'static,
    {
        self.reference_target_impl(None)
    }
    /// Resolve ark.h(time), evaluating the reference's animated slot number.
    pub fn reference_target_at(&self, time: f64) -> Result<Option<Self>, PointwiseError<E>>
    where
        E: 'static,
    {
        self.reference_target_impl(Some(time))
    }
    fn reference_target_impl(&self, time: Option<f64>) -> Result<Option<Self>, PointwiseError<E>>
    where
        E: 'static,
    {
        let Some(index) = self.reference_slot_at(time)? else {
            return Ok(None);
        };
        let Some(parent) = self.reference_group()? else {
            return Ok(None);
        };
        let group = parent.filter().ok_or(PointwiseError::InvalidCast)?;
        if index as i32 >= group.parameter(0)?.number() as i32 {
            return Ok(None);
        }
        Ok(parent.slot(index as i32)?.node())
    }
    pub(in crate::pointwise) fn reference_slot_at(
        &self,
        time: Option<f64>,
    ) -> Result<Option<usize>, PointwiseError<E>>
    where
        E: 'static,
    {
        if self.kind() != Some(PointwiseKind::Reference) {
            return Err(PointwiseError::InvalidCast);
        }
        let reference = self.filter().ok_or(PointwiseError::InvalidCast)?;
        let property = reference.parameter(0)?;
        let number = match time {
            Some(time) => property.effective_value_at(time)?,
            None => property.number(),
        } as i32;
        Ok(usize::try_from(number.wrapping_sub(1)).ok())
    }
    pub(in crate::pointwise) fn reference_group(&self) -> Result<Option<Self>, PointwiseError<E>> {
        if self.kind() != Some(PointwiseKind::Reference) {
            return Err(PointwiseError::InvalidCast);
        }
        self.enclosing_reference_group()
    }
    fn enclosing_reference_group(&self) -> Result<Option<Self>, PointwiseError<E>> {
        let mut current = self.clone();
        let mut visited = std::collections::BTreeSet::new();
        while let Some(parent) = current.parent() {
            if !visited.insert(parent.id) {
                return Err(PointwiseError::UnsupportedCapability(
                    "cyclic Reference ancestry",
                ));
            }
            if parent.kind().is_some_and(PointwiseKind::is_group)
                && parent.contents().iter().any(|slot| {
                    slot.node()
                        .is_some_and(|child| child.same_reference(&current))
                })
            {
                return Ok(Some(parent));
            }
            current = parent;
        }
        Ok(None)
    }

    /// Exterior inputs that can be selected by a Reference inside this group.
    pub fn reference_inputs(&self) -> Result<Vec<PointwiseSlot<E>>, PointwiseError<E>>
    where
        E: 'static,
    {
        if !self.kind().is_some_and(PointwiseKind::has_compact_inputs) {
            return Ok(Vec::new());
        }
        let group = self.filter().ok_or(PointwiseError::InvalidCast)?;
        let count = group.parameter(0)?.number() as usize;
        let mut slots = self.slots();
        slots.truncate(count);
        Ok(slots)
    }
    /// Contents remain independent of Slots.
    pub(in crate::pointwise) fn append_content(&self, name: Vec<u16>) -> PointwiseSlot<E> {
        let slot = SlotCore::new(self.id, name, None);
        guard(&self.core().links).contents.push(slot.clone());
        let occurrence = PointwiseSlot {
            owner: self.clone(),
            core: slot,
        };
        self.register_history_content(&occurrence);
        occurrence
    }
    pub(in crate::pointwise) fn remove_empty_group_content(
        &self,
        target: &PointwiseSlot<E>,
        positions_count: usize,
    ) -> Result<usize, PointwiseError<E>> {
        let core = self.core();
        let mut links = guard(&core.links);
        if links.contents.len() != positions_count + 1 {
            return Err(PointwiseError::UnsupportedCapability(
                "Group contents position layout",
            ));
        }
        let index = links
            .contents
            .iter()
            .position(|slot| Arc::ptr_eq(slot, &target.core))
            .ok_or(PointwiseError::Index)?;
        if index == 0 {
            return Err(PointwiseError::InvalidOperation);
        }
        if guard(&links.contents[index].child).is_some() {
            return Err(PointwiseError::UnsupportedCapability(
                "Clear contents before removing the branch",
            ));
        }
        links.contents.remove(index);
        drop(links);
        self.unregister_history_content(target);
        Ok(index - 1)
    }
    pub(in crate::pointwise) fn compact_wizard_contents(
        &self,
    ) -> Result<WizardContents<E>, PointwiseError<E>>
    where
        E: 'static,
    {
        let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
        let contents = self.contents();
        let positions = filter.wizard_content_positions();
        if contents.is_empty() || positions.len() != contents.len() - 1 {
            return Err(PointwiseError::UnsupportedCapability(
                "Wizard Group contents position layout",
            ));
        }
        let mut retained = vec![contents[0].clone()];
        let mut retained_positions = Vec::new();
        for (slot, position) in contents.into_iter().skip(1).zip(positions) {
            if slot.node().is_some() {
                retained.push(slot);
                retained_positions.push(position);
            }
        }
        // A save can compact the wire layout without removing live editor
        // slots that earlier commands still reference.
        if self.texture_node_logic().is_none() {
            guard(&self.core().links).contents =
                retained.iter().map(|slot| slot.core.clone()).collect();
            filter.replace_wizard_content_positions(retained_positions.clone());
        }
        Ok((retained, retained_positions))
    }
    pub(in crate::pointwise) fn capture_mode_links(
        &self,
    ) -> (bool, bool, Vec<Option<Self>>, Vec<Vec<u16>>) {
        let (flag, nominal, contents) = {
            let core = self.core();
            let links = guard(&core.links);
            (links.is_filter, links.default_input, links.contents.clone())
        };
        (
            flag,
            nominal,
            contents
                .iter()
                .map(|slot| guard(&slot.child).map(|id| self.at(id)))
                .collect(),
            contents.iter().map(|slot| slot.name.clone()).collect(),
        )
    }
    pub(in crate::pointwise) fn install_captured_mode_links(
        &self,
        is_filter: bool,
        default_input: bool,
        contents: Vec<Option<Self>>,
        names: Vec<Vec<u16>>,
    ) {
        assert_eq!(contents.len(), names.len(), "captured Contents/name layout");
        for node in contents.iter().flatten() {
            self.merge_region(node);
        }
        let contents = contents
            .into_iter()
            .zip(names)
            .map(|(child, name)| SlotCore::new(self.id, name, child.map(|node| node.id)))
            .collect();
        let core = self.core();
        let mut links = guard(&core.links);
        links.is_filter = is_filter;
        links.default_input = default_input;
        links.contents = contents;
    }
    /// Actual cold slotless z7 b2 metadata, where that initializer has run.
    /// Order: bt/bu/bx/by/bv/bw/b3, ao, b0/b1/b2. Source clone does not run b2.
    pub fn default_input_preparation(&self) -> Option<PreparationSnapshot> {
        guard(&self.core().mode_events)
            .prepared_default
            .as_ref()
            .map(|state| state.state)
    }
    pub(in crate::pointwise) fn install_captured_default_preparation(
        &self,
        state: Option<PreparationSnapshot>,
    ) {
        guard(&self.core().mode_events).prepared_default =
            state.map(|state| DefaultPreparation { state });
    }
    pub fn add_change_handler(&self, handler: PointwiseNodeChangeHandler<E>) {
        let core = self.core();
        let mut events = guard(&core.mode_events);
        events.changed = events.changed.combine(&DelegateCore::Single(handler.0));
    }
    pub fn remove_change_handler(&self, handler: &PointwiseNodeChangeHandler<E>) {
        let old = {
            let core = self.core();
            let mut events = guard(&core.mode_events);
            let next = events
                .changed
                .remove(&DelegateCore::Single(handler.0.clone()));
            std::mem::replace(&mut events.changed, next)
        };
        drop(old);
    }
    /// Observe publication to Rendered Image. Cache invalidation is silent.
    pub fn add_rendered_image_change_handler(&self, handler: PointwiseNodeChangeHandler<E>)
    where
        E: 'static,
    {
        let empty = crate::ContextChangeArgument::shared_empty();
        let core = self.core();
        let mut events = guard(&core.mode_events);
        let (handlers, _) = events
            .rendered_image
            .get_or_insert_with(|| (DelegateCore::Absent, empty));
        *handlers = handlers.combine(&DelegateCore::Single(handler.0));
    }
    pub fn remove_rendered_image_change_handler(&self, handler: &PointwiseNodeChangeHandler<E>) {
        let old = {
            let core = self.core();
            let mut events = guard(&core.mode_events);
            let Some((handlers, _)) = &mut events.rendered_image else {
                return;
            };
            let next = handlers.remove(&DelegateCore::Single(handler.0.clone()));
            let empty = next.is_absent();
            let old = std::mem::replace(handlers, next);
            if empty {
                events.rendered_image = None;
            }
            old
        };
        drop(old);
    }
    pub(in crate::pointwise) fn notify_rendered_image_changed(
        &self,
    ) -> Result<(), PointwiseError<E>> {
        let event = guard(&self.core().mode_events).rendered_image.clone();
        if let Some((handlers, args)) = event {
            handlers
                .invoke(self, args)
                .map_err(PointwiseError::Observer)?;
        }
        // zl.h rereads TextureNodeLogic after the rendered-image handlers.
        if let Some(logic) = self.texture_node_logic() {
            logic.notify_preparation_change()?;
        }
        Ok(())
    }
    pub fn add_slot_change_handler(&self, handler: PointwiseSlotChangeHandler<E>) {
        let core = self.core();
        let mut events = guard(&core.mode_events);
        events.topology = events.topology.combine(&DelegateCore::Single(handler.0));
    }
    pub fn remove_slot_change_handler(&self, handler: &PointwiseSlotChangeHandler<E>) {
        let old = {
            let core = self.core();
            let mut events = guard(&core.mode_events);
            let next = events
                .topology
                .remove(&DelegateCore::Single(handler.0.clone()));
            std::mem::replace(&mut events.topology, next)
        };
        drop(old);
    }
    pub(super) fn clear_local_mode_cache(&self) {
        let old = {
            let core = self.core();
            let mut events = guard(&core.mode_events);
            (
                std::mem::take(&mut events.timed_images),
                events.scene_rendered_image.take(),
                events.final_cache.invalidate(),
            )
        };
        drop(old);
        match &self.core().payload {
            Payload::ScenePrimitive { .. }
            | Payload::SceneImport { .. }
            | Payload::SceneClearLights
            | Payload::SceneCombine
            | Payload::SceneTransform { .. }
            | Payload::UvTexture { .. } => {}
            Payload::Filter { .. } => {
                if let Some(filter) = self.filter() {
                    filter.clear_cache_without_notification();
                }
            }
            Payload::ImageGenerator { model } => {
                let old = guard(model).take_image_caches();
                drop(old);
            }
            Payload::Input { cache, .. }
            | Payload::Solid { cache, .. }
            | Payload::Invert { cache, .. } => {
                let old = guard(cache).take();
                drop(old);
            }
        }
    }
}

impl<E> PointwiseSlot<E> {
    /// The group a Reference placed in this occurrence would resolve against.
    pub fn reference_scope(&self) -> Result<Option<PointwiseNode<E>>, PointwiseError<E>> {
        let parent = self.parent();
        if parent.kind().is_some_and(PointwiseKind::is_group)
            && parent
                .contents()
                .iter()
                .any(|slot| slot.same_reference(self))
        {
            return Ok(Some(parent));
        }
        parent.enclosing_reference_group()
    }
}
impl<E: 'static> PointwiseNode<E> {
    // Actual payload clone authority. Filters, Invert and Solid retain their
    // concrete modes, and Gradient distinguishes a final-render clone.
    pub(in crate::pointwise) fn is_clone_mode(&self) -> bool {
        if self.is_final_graph_clone() {
            return true;
        }
        match &self.core().payload {
            Payload::Filter { .. } => {
                self.filter().expect("actual filter payload").clone_mode()
                    != super::super::PointwiseCloneMode::Original
            }
            Payload::Invert { model, .. } => model.mode() != crate::SelectionCloneMode::Original,
            Payload::Solid { model, .. } => {
                model.snapshot().mode != crate::SolidColorCloneMode::Original
            }
            Payload::ImageGenerator { model } => guard(model).final_clone,
            Payload::Input { .. }
            | Payload::ScenePrimitive { .. }
            | Payload::SceneImport { .. }
            | Payload::SceneClearLights
            | Payload::SceneCombine
            | Payload::SceneTransform { .. }
            | Payload::UvTexture { .. } => false,
        }
    }

    pub(in crate::pointwise) fn is_final_graph_clone(&self) -> bool {
        guard(&self.core().mode_events).final_graph_clone
    }

    pub(in crate::pointwise) fn mark_final_graph_clone(&self) {
        guard(&self.core().mode_events).final_graph_clone = true;
    }
    /// Actual z7(gz.Original): y2 constructor plus concrete nominal subtype.
    /// No resource, default image or static apq lookup is implied by this ctor.
    pub fn default_input() -> Result<Self, PointwiseError<E>> {
        let node = Self::image_generator(super::super::ImageGeneratorKind::ImportedImage)?;
        guard(&node.core().links).default_input = true;
        Ok(node)
    }
    pub fn default_input_prototype(index: i32) -> Result<Self, PointwiseError<E>> {
        defaults::prototype(index)
    }
    pub fn default_input_preview(index: i32) -> Result<crate::Bitmap, PointwiseError<E>> {
        defaults::preview(index)
    }
    pub fn default_input_mapping() -> Result<Option<SharedDefaultInputMapping>, PointwiseError<E>> {
        defaults::mapping::<E>()
    }
    pub fn set_default_input_mapping(
        mapping: Option<&SharedDefaultInputMapping>,
    ) -> Result<(), PointwiseError<E>> {
        defaults::set_mapping::<E>(mapping.cloned())
    }
    pub(crate) fn set_document_preview_samples(
        &self,
        samples: &crate::document_container::DocumentPreviewSamples,
    ) {
        guard(&self.core().mode_events).document_preview_samples = Some(samples.clone());
    }
    pub(in crate::pointwise) fn inherit_document_preview_samples(&self, root: &Self) {
        if let Some(samples) = guard(&root.core().mode_events)
            .document_preview_samples
            .clone()
        {
            self.set_document_preview_samples(&samples);
        }
    }
    pub(in crate::pointwise) fn new_preview_sample(
        resource: i32,
    ) -> Result<Self, PointwiseError<E>> {
        defaults::prototype_by_resource(resource)?.source_clone()
    }
    /// Actual zl.t: old list survives; a fresh list/qi is published before
    /// prototype acquisition. bl is written only after all attachments succeed.
    pub fn convert_to_filter(&self, force: bool) -> Result<(), PointwiseError<E>> {
        if !self.is_filter() || force {
            let samples = guard(&self.core().mode_events)
                .document_preview_samples
                .as_ref()
                .map(|samples| *guard(samples));
            let old = {
                let core = self.core();
                let mut links = guard(&core.links);
                std::mem::take(&mut links.slots)
            };
            let mut index = 0;
            while index < old.len() {
                let source = old[index].clone();
                let fresh = SlotCore::new(self.id, source.name.clone(), None);
                guard(&self.core().links).slots.push(fresh);
                let child = guard(&source.child).map(|id| self.at(id));
                let use_default = index < 3
                    && (child.is_none() || child.as_ref().is_some_and(Self::is_default_input));
                let child = if use_default {
                    Some(if let Some(samples) = samples {
                        Self::new_preview_sample(samples[index])?
                    } else {
                        Self::default_input_prototype(index as i32)?.source_clone()?
                    })
                } else {
                    guard(&source.child).map(|id| self.at(id))
                };
                // Source refetches this.t[index] after acquisition/reentry.
                self.slot(index as i32)?.set_node_quiet(child.as_ref())?;
                index += 1;
            }
            guard(&self.core().links).is_filter = true;
        }
        self.clear_mode_subtree(&mut BTreeSet::new())?;
        self.notify_graph(
            Sender::Node(self.clone()),
            crate::ContextChangeArgument::shared_empty(),
            &mut BTreeSet::new(),
        )
    }
    /// Actual inverse i(qi): retained bl clears before any qi or child read.
    pub fn leave_filter_mode(&self, selected: &PointwiseSlot<E>) -> Result<(), PointwiseError<E>> {
        self.leave_filter_mode_quiet(Some(selected))?;
        self.clear_mode_subtree(&mut BTreeSet::new())?;
        self.notify_graph(
            Sender::Node(self.clone()),
            crate::ContextChangeArgument::shared_empty(),
            &mut BTreeSet::new(),
        )
    }

    pub(in crate::pointwise) fn leave_filter_mode_quiet(
        &self,
        selected: Option<&PointwiseSlot<E>>,
    ) -> Result<(), PointwiseError<E>> {
        guard(&self.core().links).is_filter = false;
        let count = self.slots().len().min(3);
        for index in 0..count {
            let slot = self.slot(index as i32)?;
            if selected.is_none_or(|selected| !slot.same_reference(selected))
                && slot.node().as_ref().is_some_and(Self::is_default_input)
            {
                self.slot(index as i32)?.set_node_quiet(None)?;
            }
        }
        Ok(())
    }
    pub(in crate::pointwise) fn clear_mode_subtree(
        &self,
        active: &mut BTreeSet<NodeId>,
    ) -> Result<(), PointwiseError<E>> {
        if !active.insert(self.id) {
            return Err(PointwiseError::UnsupportedCapability(
                "cyclic source bm recursion",
            ));
        }
        self.clear_local_mode_cache();
        let slots = self.slots();
        for slot in slots {
            if slot.node().is_some() {
                slot.node()
                    .ok_or(PointwiseError::NullReference)?
                    .clear_mode_subtree(active)?;
            }
        }
        let contents = self.contents();
        for slot in contents {
            if slot.node().is_some() {
                slot.node()
                    .ok_or(PointwiseError::NullReference)?
                    .clear_mode_subtree(active)?;
            }
        }
        active.remove(&self.id);
        Ok(())
    }
    fn slot_mode_changed(&self, index: i32) -> Result<(), PointwiseError<E>> {
        // Attached TextureNodeLogic remains unqualified; sh has already applied
        // the actual Original clone-mode gate before calling aa5.
        if self.is_base_node() || self.is_filter() {
            if self.is_filter() && self.slot(index)?.node().is_some() {
                self.leave_filter_mode(&self.slot(index)?)?;
            } else if !self.is_filter()
                // Oklab Color has authored fallbacks for its optional maps.
                && self.kind() != Some(PointwiseKind::OklabColor)
                && self.slot(index)?.node().is_none()
                && self.slots().iter().all(|slot| slot.node().is_none())
            {
                self.convert_to_filter(false)?;
            }
        }
        let handler = guard(&self.core().mode_events).topology.clone();
        handler
            .invoke(self, PointwiseSlotChange(Arc::new(index)))
            .map_err(PointwiseError::Observer)
    }
    fn notify_graph(
        &self,
        sender: Sender<E>,
        args: Arc<crate::ContextChangeArgument<E>>,
        active: &mut BTreeSet<NodeId>,
    ) -> Result<(), PointwiseError<E>> {
        self.notify_graph_until(sender, args, active, None)
    }
    fn notify_graph_until(
        &self,
        sender: Sender<E>,
        args: Arc<crate::ContextChangeArgument<E>>,
        active: &mut BTreeSet<NodeId>,
        stop_at: Option<NodeId>,
    ) -> Result<(), PointwiseError<E>> {
        // zl.sh returns before cache, aa5, callbacks and Parent propagation for
        // non-Original. Slot publication/Parent assignment have already occurred.
        if self.is_clone_mode() {
            return Ok(());
        }
        if matches!(args.as_ref(),crate::ContextChangeArgument::Property(value) if value.update_controls_only())
            || matches!(args.as_ref(),crate::ContextChangeArgument::Modern(value) if value.update_controls_only())
        {
            return Ok(());
        }
        if stop_at == Some(self.id) {
            return Ok(());
        }
        if !active.insert(self.id) {
            return Err(PointwiseError::UnsupportedCapability(
                "cyclic source sh parent recursion",
            ));
        }
        if self.kind().is_some_and(PointwiseKind::is_group) {
            if let Sender::GroupInputCount { old, new } = &sender {
                let start = crate::framework_cast::integer(old.min(*new)).wrapping_add(1);
                let end = crate::framework_cast::integer(old.max(*new));
                for number in start..=end {
                    self.invalidate_group_input_references(number, true, active)?;
                }
            } else if let Some(index) = self.slots().iter().position(|slot| match &sender {
                Sender::Slot(actual) => slot.same_reference(actual),
                Sender::Node(actual) => slot
                    .node()
                    .is_some_and(|child| child.same_reference(actual)),
                Sender::Property | Sender::GroupInputCount { .. } => false,
            }) {
                self.invalidate_group_input_references(index as i32 + 1, false, active)?;
            }
        }
        self.clear_local_mode_cache();
        let core = self.core();
        let mut events = guard(&core.mode_events);
        if events.defer_wizard_change {
            let previous = events
                .wizard_change
                .replace(DeferredWizardChange { sender, args });
            drop(events);
            drop(previous);
            active.remove(&self.id);
            return Ok(());
        }
        drop(events);
        let mut index = 0;
        loop {
            if index >= self.slots().len() {
                break;
            }
            let slot = self.slot(index as i32)?;
            let matches = match &sender {
                Sender::Slot(actual) => slot.same_reference(actual),
                Sender::Node(actual) => slot
                    .node()
                    .is_some_and(|child| child.same_reference(actual)),
                Sender::Property | Sender::GroupInputCount { .. } => false,
            };
            if matches {
                self.slot_mode_changed(index as i32)?;
                break;
            }
            index += 1;
        }
        let handlers = guard(&self.core().mode_events).changed.clone();
        handlers
            .invoke(self, args.clone())
            .map_err(PointwiseError::Observer)?;
        if let Some(parent) = self.parent() {
            parent.notify_graph_until(
                Sender::Node(self.clone()),
                crate::ContextChangeArgument::shared_empty(),
                active,
                stop_at,
            )?;
        }
        self.record_property_history(&args)?;
        active.remove(&self.id);
        Ok(())
    }
    fn invalidate_group_input_references(
        &self,
        number: i32,
        first_content_only: bool,
        active: &mut BTreeSet<NodeId>,
    ) -> Result<(), PointwiseError<E>> {
        // hj.sh sends abi through its Contents, and zl.abi walks ordinary
        // input slots. ark.abi clears the dependent path back to this group.
        let mut contents = self.contents();
        if first_content_only {
            contents.truncate(1);
        }
        let mut pending: Vec<_> = contents
            .into_iter()
            .rev()
            .filter_map(|s| s.node())
            .collect();
        let mut visited = BTreeSet::new();
        while let Some(node) = pending.pop() {
            if !visited.insert(node.id) {
                continue;
            }
            if node.kind() == Some(PointwiseKind::Reference)
                && node
                    .reference_group()?
                    .is_some_and(|group| group.same_reference(self))
            {
                let selector = node
                    .filter()
                    .ok_or(PointwiseError::NullReference)?
                    .parameter(0)?;
                if selector.number() == f64::from(number) || selector.has_animated_binding()? {
                    node.notify_graph_until(
                        Sender::Node(node.clone()),
                        crate::ContextChangeArgument::shared_empty(),
                        active,
                        Some(self.id),
                    )?;
                }
            }
            pending.extend(
                node.slots()
                    .into_iter()
                    .rev()
                    .filter_map(|slot| slot.node()),
            );
        }
        Ok(())
    }
    pub(in crate::pointwise) fn notify_property_change_with(
        &self,
        args: Arc<crate::ContextChangeArgument<E>>,
    ) -> Result<(), PointwiseError<E>> {
        self.notify_graph(Sender::Property, args, &mut BTreeSet::new())
    }

    pub(in crate::pointwise) fn notify_wizard_control_change(
        &self,
        filter: &super::PointwiseFilter<E>,
        variable: i32,
        args: Arc<crate::ContextChangeArgument<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let undo = match args.as_ref() {
            crate::ContextChangeArgument::Property(change) => change.typed_undo_info(),
            _ => None,
        };
        let force = matches!(undo, Some(crate::ContextUndoInfo::Animation(_)));
        // aff.sh holds changes reaching this group during variable propagation.
        // Child notifications run immediately. The last group change is then
        // sent before recording the separate Wizard control command.
        let core = self.core();
        let previous = {
            let mut events = guard(&core.mode_events);
            events.defer_wizard_change = true;
            events.wizard_change.take()
        };
        drop(previous);
        let propagated = filter.propagate_variable_change(variable, force);
        let pending = {
            let mut events = guard(&core.mode_events);
            events.defer_wizard_change = false;
            events.wizard_change.take()
        };
        propagated?;
        if let Some(change) = pending {
            self.notify_graph(change.sender, change.args, &mut BTreeSet::new())?;
        }
        if let Some(undo) = undo {
            undo.mark_lab_undo();
        }
        self.record_property_history(&args)
    }
    pub(in crate::pointwise) fn notify_group_input_count_change(
        &self,
        old: f64,
        new: f64,
        args: Arc<crate::ContextChangeArgument<E>>,
    ) -> Result<(), PointwiseError<E>> {
        self.notify_graph(
            Sender::GroupInputCount { old, new },
            args,
            &mut BTreeSet::new(),
        )
    }
    pub(in crate::pointwise) fn notify_slot_change(
        &self,
        slot: &PointwiseSlot<E>,
    ) -> Result<(), PointwiseError<E>> {
        self.notify_graph(
            Sender::Slot(slot.clone()),
            crate::ContextChangeArgument::shared_empty(),
            &mut BTreeSet::new(),
        )
    }

    pub(in crate::pointwise) fn notify_restored_inputs(
        &self,
        indices: impl IntoIterator<Item = usize>,
    ) -> Result<(), PointwiseError<E>> {
        self.clear_mode_subtree(&mut BTreeSet::new())?;
        for index in indices {
            let handler = guard(&self.core().mode_events).topology.clone();
            handler
                .invoke(self, PointwiseSlotChange(Arc::new(index as i32)))
                .map_err(PointwiseError::Observer)?;
        }
        self.notify_property_change_with(crate::ContextChangeArgument::shared_empty())
    }
}
#[cfg(test)]
#[path = "graph_mode_test.rs"]
mod tests;
