//! Retained node roots in the GTX container. This preserves file front matter
//! without regenerating the source application's JPEG previews.
use super::catalog::GraphCatalog;
use super::*;
use crate::history::HistoryGraphEdit;
use crate::pointwise::GraphConnectionEdit;
use crate::{NodeWireError, PointwiseError, PointwiseNode, SharedSolidColorNode};
use std::borrow::Cow;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard};

#[path = "legacy_xml.rs"]
mod legacy_xml;

fn same_branch<E: 'static>(
    left: Option<&PointwiseNode<E>>,
    right: Option<&PointwiseNode<E>>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_reference(right),
        (None, None) => true,
        _ => false,
    }
}

fn slot_edit_error<E>(error: PointwiseError<E>) -> GtxGraphError<E> {
    match error {
        PointwiseError::History(cause) => GtxGraphError::History(*cause),
        error => GtxGraphError::Node(error.into()),
    }
}

#[path = "curve_adjusters.rs"]
mod curve_adjusters;
pub use curve_adjusters::{CurveAdjuster, CurveAdjusterLink};
#[path = "environment_adjusters.rs"]
mod environment_adjusters;
#[path = "style_synthesis_numbers.rs"]
mod style_synthesis_numbers;
#[path = "synthesis_adjusters.rs"]
mod synthesis_adjusters;
use environment_adjusters::restore_environment_catalog;
pub use environment_adjusters::{
    EnvironmentAdjuster, EnvironmentAdjusterLink, EnvironmentColorLink, EnvironmentMapTarget,
    EnvironmentNumericMapping,
};
pub use style_synthesis_numbers::{
    StyleSynthesisNumberMapping, StyleSynthesisNumberTarget, SynthesisPairTarget,
};
use synthesis_adjusters::restore_synthesis_catalog;
pub use synthesis_adjusters::{
    SynthesisAdjuster, SynthesisAdjusterLink, SynthesisDraftPreviewTarget,
};
#[path = "lab_adjuster_metadata.rs"]
mod lab_adjuster_metadata;
pub use lab_adjuster_metadata::LabAdjusterMetadata;
#[path = "lab_adjuster_visibility.rs"]
mod lab_adjuster_visibility;
#[path = "lab_group_scope.rs"]
mod lab_group_scope;
use lab_group_scope::LabAssignment;
pub(crate) use lab_group_scope::LabLayoutEdit;
#[path = "lab_preset_scope.rs"]
mod lab_preset_scope;
#[path = "lab_presets.rs"]
mod lab_presets;
pub(crate) use lab_presets::AffectedProperties;
pub use lab_presets::{
    ColorAdjusterLink, GradientAdjusterLink, LabColorAdjuster, LabColorValue, LabPreset,
    LabPresetOperation, LabPresetSection,
};
#[path = "numeric_adjusters.rs"]
mod numeric_adjusters;
pub use numeric_adjusters::{
    LabNumericAdjuster, LabNumericRange, LabNumericValue, LabScalarDefinition,
};
#[path = "numeric_links.rs"]
mod numeric_links;
pub use numeric_links::{LabNumericAxis, LabNumericLink, LabNumericMapping};
#[path = "overview_presets.rs"]
mod overview_presets;
pub use overview_presets::OverviewPresetMode;
pub(crate) use overview_presets::PresetInputMove;

pub(crate) type DocumentPreviewSamples = Arc<Mutex<[i32; 3]>>;

/// Test image supplied to the first input of a filter document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterPreviewSample {
    Robot,
    RobotCutout,
}

impl FilterPreviewSample {
    fn index(self) -> i32 {
        match self {
            Self::Robot => 0,
            Self::RobotCutout => 3,
        }
    }
}

pub(crate) struct PreviewSampleChange {
    samples: DocumentPreviewSamples,
    before: i32,
    after: i32,
}

impl PreviewSampleChange {
    pub(crate) fn sample(&self) -> i32 {
        self.after
    }

    pub(crate) fn matches(&self, undo: bool) -> bool {
        preview_samples(&self.samples)[0] == if undo { self.after } else { self.before }
    }

    pub(crate) fn publish(&self, undo: bool) {
        preview_samples(&self.samples)[0] = if undo { self.before } else { self.after };
    }
}

fn preview_samples(samples: &DocumentPreviewSamples) -> MutexGuard<'_, [i32; 3]> {
    samples.lock().unwrap_or_else(|error| error.into_inner())
}

struct BranchOrder {
    slots: Vec<usize>,
    // Positions are keyed by the retained slot, including hidden occurrences.
    positions: Vec<[f32; 2]>,
}

impl BranchOrder {
    fn lock(order: &Mutex<Self>) -> MutexGuard<'_, Self> {
        order.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn active_positions(&self) -> Vec<[f32; 2]> {
        self.slots
            .iter()
            .skip(1)
            .map(|&id| self.positions[id])
            .collect()
    }
}

pub(crate) struct BranchOrderChange {
    order: Arc<Mutex<BranchOrder>>,
    before: Vec<usize>,
    after: Vec<usize>,
    position: Option<BranchPositionEdit>,
}

impl BranchOrderChange {
    fn new(order: &Arc<Mutex<BranchOrder>>, edit: impl FnOnce(&mut Vec<usize>)) -> Self {
        let before = BranchOrder::lock(order).slots.clone();
        let mut after = before.clone();
        edit(&mut after);
        Self {
            order: order.clone(),
            before,
            after,
            position: None,
        }
    }

    pub(crate) fn matches(&self, undo: bool) -> bool {
        let order = BranchOrder::lock(&self.order);
        &order.slots == if undo { &self.after } else { &self.before }
            && self.position.as_ref().is_none_or(|position| {
                order.positions[position.slot].map(f32::to_bits)
                    == (if undo {
                        position.after
                    } else {
                        position.before
                    })
                    .map(f32::to_bits)
            })
    }

    pub(crate) fn publish(&self, undo: bool) {
        let mut order = BranchOrder::lock(&self.order);
        order.slots = if undo {
            self.before.clone()
        } else {
            self.after.clone()
        };
        if let Some(position) = &self.position {
            order.positions[position.slot] = if undo {
                position.before
            } else {
                position.after
            };
        }
    }
}

struct BranchPositionEdit {
    order: Arc<Mutex<BranchOrder>>,
    slot: usize,
    before: [f32; 2],
    after: [f32; 2],
}

impl<E> HistoryGraphEdit<E> for BranchPositionEdit {
    fn apply(&self, direction: crate::HistoryDirection) -> Result<(), crate::HistoryError<E>> {
        let (expected, next) = match direction {
            crate::HistoryDirection::Undo => (self.after, self.before),
            crate::HistoryDirection::Redo => (self.before, self.after),
        };
        let mut order = BranchOrder::lock(&self.order);
        if !order.slots.iter().skip(1).any(|slot| *slot == self.slot)
            || order.positions[self.slot].map(f32::to_bits) != expected.map(f32::to_bits)
        {
            return Err(crate::HistoryError::BranchChanged);
        }
        order.positions[self.slot] = next;
        Ok(())
    }
}

enum PositionsMut<'a> {
    Unbound(&'a mut [[f32; 2]]),
    Bound {
        order: &'a Mutex<BranchOrder>,
        slots: Vec<usize>,
        positions: Vec<[f32; 2]>,
    },
}

impl Deref for PositionsMut<'_> {
    type Target = [[f32; 2]];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Unbound(positions) => positions,
            Self::Bound { positions, .. } => positions,
        }
    }
}

impl DerefMut for PositionsMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Unbound(positions) => positions,
            Self::Bound { positions, .. } => positions,
        }
    }
}

impl Drop for PositionsMut<'_> {
    fn drop(&mut self) {
        if let Self::Bound {
            order,
            slots,
            positions,
        } = self
        {
            let mut order = BranchOrder::lock(order);
            for (&slot, &position) in slots.iter().zip(positions.iter()) {
                order.positions[slot] = position;
            }
        }
    }
}

enum Branches<E> {
    Unbound(Vec<Option<PointwiseNode<E>>>),
    Bound {
        history: crate::History<E>,
        slots: Vec<crate::PointwiseHistorySlot<E>>,
        // History changes which occurrences are visible. The slot handles stay
        // in the document and keep their identities across those changes.
        order: Arc<Mutex<BranchOrder>>,
    },
}
impl<E> Branches<E> {
    fn len(&self) -> usize {
        match self {
            Self::Unbound(nodes) => nodes.len(),
            Self::Bound { order, .. } => BranchOrder::lock(order).slots.len(),
        }
    }
}

/// A document owns its branch list and file metadata. Each nonnull branch is a
/// real retained node graph, including property roots and live slot occurrences.
/// Recursive wire loading creates new nodes for each serialized occurrence;
/// the file contains no invented reference table or cached rendered texture.
pub struct Gtx375GraphDocument<E = ()> {
    header: Gtx375Header,
    presets: GtxPresetCollection,
    catalog: GraphCatalog,
    positions: Vec<[f32; 2]>,
    branches: Branches<E>,
    preview_samples: DocumentPreviewSamples,
    consumed: i32,
}
impl<E> fmt::Debug for Gtx375GraphDocument<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gtx375GraphDocument")
            .field("header", &self.header)
            .field("branch_count", &self.branches.len())
            .field("output_order", &*preview_samples(&self.preview_samples))
            .field("consumed", &self.consumed)
            .finish_non_exhaustive()
    }
}
impl<E: 'static> Gtx375GraphDocument<E> {
    /// Create a Rust document around actual node roots. Empty documents are an
    /// API error: this container's branch count must be positive. New front
    /// matter has no fabricated preview, contributor or document-control data.
    pub fn new(branches: Vec<Option<PointwiseNode<E>>>) -> Result<Self, GtxGraphError<E>> {
        if branches.is_empty() {
            return Err(GtxReadError::Unsupported(UnsupportedGtx::Count {
                section: "branches",
                count: 0,
            })
            .into());
        }
        count_i32(branches.len())?;
        let mut positions = Vec::new();
        reserve(&mut positions, branches.len() - 1)?;
        positions.resize(branches.len() - 1, [0.0; 2]);
        let preview_samples = Arc::new(Mutex::new([0, 1, 2]));
        for node in branches.iter().flatten() {
            node.set_document_preview_samples(&preview_samples);
        }
        Ok(Self {
            header: Gtx375Header {
                source_version: VERSION,
                filter_inputs: 0,
                previews: Vec::new(),
                contributors: crate::DocumentContributors::new(),
            },
            presets: GtxPresetCollection::default(),
            catalog: GraphCatalog::default(),
            positions,
            branches: Branches::Unbound(branches),
            preview_samples,
            consumed: 0,
        })
    }
    pub fn load(input: &[u8]) -> Result<Self, GtxGraphLoadFailure<E>> {
        let mut reader = BinaryCursor::new(input).map_err(|error| GtxGraphLoadFailure {
            position: 0,
            kind: GtxGraphError::FrontMatter(error.into()),
        })?;
        let result = Self::read(&mut reader, input.len());
        let position = reader.position().unwrap_or(0);
        reader.close();
        result.map_err(|kind| GtxGraphLoadFailure { position, kind })
    }
    /// Read from the caller's current cursor. Unknown node payloads have no
    /// length, so factory errors stop at their real consumed prefix.
    pub fn read(
        reader: &mut BinaryCursor<'_>,
        file_length: usize,
    ) -> Result<Self, GtxGraphError<E>> {
        let mut context = crate::pointwise::NodeWireReadContext::for_document(None);
        Self::read_with_context(reader, file_length, &mut context)
    }
    /// Optional caller policy checks at each actual node factory entry after
    /// consuming its identity, before construction. Counts span all branches.
    pub fn load_bounded(
        input: &[u8],
        policy: crate::NodeWireReadPolicy,
    ) -> Result<Self, GtxGraphLoadFailure<E>> {
        let mut reader = BinaryCursor::new(input).map_err(|error| GtxGraphLoadFailure {
            position: 0,
            kind: GtxGraphError::FrontMatter(error.into()),
        })?;
        let mut context = crate::pointwise::NodeWireReadContext::for_document(Some(policy));
        let result = Self::read_with_context(&mut reader, input.len(), &mut context);
        let position = reader.position().unwrap_or(0);
        reader.close();
        result.map_err(|kind| GtxGraphLoadFailure { position, kind })
    }

    pub fn is_mycela_project(input: &[u8]) -> bool {
        input.starts_with(PROJECT_MAGIC)
    }

    /// Opens versioned Mycela storage, including supported Mycela-only nodes.
    pub fn load_project(
        input: &[u8],
        policy: crate::NodeWireReadPolicy,
    ) -> Result<Self, GtxGraphLoadFailure<E>> {
        let mut reader = BinaryCursor::new(input).map_err(|error| GtxGraphLoadFailure {
            position: 0,
            kind: GtxGraphError::FrontMatter(error.into()),
        })?;
        let result = (|| {
            if !Self::is_mycela_project(input) {
                return Err(GtxReadError::ProjectSignature.into());
            }
            reader.read_bytes(PROJECT_MAGIC.len() as i32)?;
            let version = reader.read_i32()?;
            if !(1..=PROJECT_VERSION).contains(&version) {
                return Err(GtxReadError::ProjectVersion(version).into());
            }
            let mut context = crate::pointwise::NodeWireReadContext::for_project(policy, version);
            let header = Gtx375Header::read_body(&mut reader, input.len(), VERSION)?;
            let front = read_front_matter_body(&mut reader, input.len(), header)?;
            let document = Self::read_graph(&mut reader, front, &mut context)?;
            if document.consumed as usize != input.len() {
                return Err(GtxReadError::TrailingProjectData.into());
            }
            Ok(document)
        })();
        result.map_err(|kind| GtxGraphLoadFailure {
            position: reader.position().unwrap_or(0),
            kind,
        })
    }
    /// Load a fresh standalone Solid Color for a workspace that edits the same
    /// retained source through both standalone and graph controls.
    pub fn load_standalone_solid_color(
        input: &[u8],
    ) -> Result<(Self, SharedSolidColorNode<E>), GtxGraphLoadFailure<E>> {
        let document = Self::load_bounded(input, crate::NodeWireReadPolicy::new(1, 1))?;
        let model = match document.branches().as_ref() {
            [Some(node)] => node.solid_model().map_err(NodeWireError::Solid),
            _ => Err(NodeWireError::UnsupportedCapability(
                "Standalone Solid Color branch",
            )),
        }
        .map_err(|error| GtxGraphLoadFailure {
            position: document.consumed,
            kind: GtxGraphError::Node(error),
        })?;
        Ok((document, model))
    }
    /// Resolve variable wildcard options using the host service throughout all
    /// recursively loaded branches. The optional budget shares the same node
    /// and depth counters as `load_bounded`.
    pub fn load_with_variable_service(
        input: &[u8],
        policy: Option<crate::NodeWireReadPolicy>,
        base: &[u16],
        service: &mut dyn crate::VariableFileService<Error = crate::PointwiseError<E>>,
    ) -> Result<Self, GtxGraphLoadFailure<E>> {
        let mut reader = BinaryCursor::new(input).map_err(|error| GtxGraphLoadFailure {
            position: 0,
            kind: GtxGraphError::FrontMatter(error.into()),
        })?;
        let mut context =
            crate::pointwise::NodeWireReadContext::with_variables(policy, base, service);
        context.allow_legacy_mycela_layout();
        let result = Self::read_with_context(&mut reader, input.len(), &mut context);
        let position = reader.position().unwrap_or(0);
        reader.close();
        result.map_err(|kind| GtxGraphLoadFailure { position, kind })
    }
    pub(crate) fn read_with_context(
        reader: &mut BinaryCursor<'_>,
        file_length: usize,
        context: &mut crate::pointwise::NodeWireReadContext<'_, E>,
    ) -> Result<Self, GtxGraphError<E>> {
        if let Some(version) = legacy_xml::read_version(reader)? {
            return legacy_xml::read(reader, file_length, version, context);
        }
        let front = read_front_matter(reader, file_length)?;
        Self::read_graph(reader, front, context)
    }

    fn read_graph(
        reader: &mut BinaryCursor<'_>,
        front: FrontMatter,
        context: &mut crate::pointwise::NodeWireReadContext<'_, E>,
    ) -> Result<Self, GtxGraphError<E>> {
        let FrontMatter {
            header,
            presets,
            catalog,
            positions,
            count,
        } = front;
        let mut branches = Vec::new();
        reserve(&mut branches, count as usize)?;
        for _ in 0..count {
            let identity = reader.read_string()?;
            let branch = if identity.iter().copied().eq("null".encode_utf16()) {
                None
            } else {
                Some(
                    crate::pointwise::load_node_binary_with_context(
                        reader,
                        header.source_version,
                        Some(&identity),
                        Some(context),
                    )
                    .map_err(GtxGraphError::Node)?,
                )
            };
            branches.push(branch);
        }
        let preview_samples = Arc::new(Mutex::new(read_output_order(
            reader,
            header.source_version,
        )?));
        for node in branches.iter().flatten() {
            node.set_document_preview_samples(&preview_samples);
        }
        if header.source_version < 3.52 {
            for node in branches.iter().flatten() {
                migrate_legacy_seeds(node).map_err(GtxGraphError::Node)?;
            }
        }
        prepare_filter_document(
            branches.first().and_then(Option::as_ref),
            header.filter_inputs,
        )?;
        if let Some(catalog) = &catalog {
            curve_adjusters::restore_curve_catalog(&branches, catalog)?;
            restore_environment_catalog(&branches, catalog)
                .map_err(|error| GtxGraphError::Node(error.into()))?;
            restore_synthesis_catalog(&branches, catalog)
                .map_err(|error| GtxGraphError::Node(error.into()))?;
        }
        let catalog = GraphCatalog::new(catalog.unwrap_or_default());
        numeric_adjusters::restore_lab_values(
            &branches,
            &catalog,
            context.preserves_saved_patch_shape_values(),
        )?;
        let consumed = reader.position()?;
        Ok(Self {
            header,
            presets,
            catalog,
            positions,
            branches: Branches::Unbound(branches),
            preview_samples,
            consumed,
        })
    }
    /// Join supported properties in the document's inputs and group contents.
    /// The caller supplies contributor, resource, and travel services.
    pub fn bind_property_history(
        &mut self,
        services: std::sync::Arc<dyn crate::HistoryServices<E>>,
    ) -> Result<crate::History<E>, crate::HistoryError<E>> {
        let branches = match &self.branches {
            Branches::Unbound(branches) => branches,
            Branches::Bound { history, .. } => return Ok(history.clone()),
        };
        let anchor = branches
            .iter()
            .flatten()
            .next()
            .cloned()
            .unwrap_or_else(PointwiseNode::invert);
        crate::PointwiseHistorySlot::validate_trees(branches.iter().flatten())?;
        let history = crate::History::new(services)?;
        let mut slots = Vec::with_capacity(branches.len());
        for branch in branches {
            let slot = crate::PointwiseHistorySlot::bind(&anchor, &history, Vec::new());
            slot.set_node(branch.as_ref());
            slots.push(slot);
        }
        let history = slots[0]
            .history()
            .expect("document owns joined history core");
        self.branches = Branches::Bound {
            history: history.clone(),
            slots,
            order: Arc::new(Mutex::new(BranchOrder {
                slots: (0..branches.len()).collect(),
                positions: std::iter::once([0., 0.])
                    .chain(std::mem::take(&mut self.positions))
                    .collect(),
            })),
        };
        Ok(history)
    }
    pub fn property_history(&self) -> Option<crate::History<E>> {
        match &self.branches {
            Branches::Bound { history, .. } => Some(history.clone()),
            Branches::Unbound(_) => None,
        }
    }
    /// Release the editor bindings while retaining the current graph.
    pub fn unbind_property_history(&mut self) {
        if let Branches::Bound {
            slots,
            history,
            order,
        } = &self.branches
        {
            history.retire_document_binding();
            let nodes: Vec<_> = slots
                .iter()
                .map(crate::PointwiseHistorySlot::release_document_binding)
                .collect();
            let order = BranchOrder::lock(order);
            let branches = order.slots.iter().map(|&id| nodes[id].clone()).collect();
            self.positions = order.active_positions();
            drop(order);
            self.branches = Branches::Unbound(branches);
        }
    }
    /// The asy branch target remains identical when set_branch replaces its rw.
    /// An empty branch still has the same target root after history is bound.
    pub fn history_slot(&self, ix: usize) -> Option<crate::PointwiseHistorySlot<E>> {
        match &self.branches {
            Branches::Bound { slots, .. } => {
                self.branch_occurrence_id(ix).map(|id| slots[id].clone())
            }
            Branches::Unbound(_) => None,
        }
    }

    /// Stable root occurrence identifier within the current document binding.
    pub fn branch_occurrence_id(&self, index: usize) -> Option<usize> {
        match &self.branches {
            Branches::Unbound(nodes) => (index < nodes.len()).then_some(index),
            Branches::Bound { order, .. } => BranchOrder::lock(order).slots.get(index).copied(),
        }
    }
    pub fn header(&self) -> &Gtx375Header {
        &self.header
    }
    pub fn presets(&self) -> &GtxPresetCollection {
        &self.presets
    }
    pub fn presets_mut(&mut self) -> &mut GtxPresetCollection {
        &mut self.presets
    }
    pub fn catalog(&self) -> Option<Vec<&GtxCatalogRecord>> {
        let entries: Vec<_> = self.catalog.entries().map(|(entry, _)| entry).collect();
        (!entries.is_empty()).then_some(entries)
    }
    pub fn catalog_mut(&mut self) -> Option<Vec<&mut GtxCatalogRecord>> {
        let entries = self.catalog.active_mut();
        (!entries.is_empty()).then_some(entries)
    }
    pub fn positions(&self) -> Cow<'_, [[f32; 2]]> {
        match &self.branches {
            Branches::Unbound(_) => Cow::Borrowed(&self.positions),
            Branches::Bound { order, .. } => {
                Cow::Owned(BranchOrder::lock(order).active_positions())
            }
        }
    }
    pub fn positions_mut(&mut self) -> impl DerefMut<Target = [[f32; 2]]> + '_ {
        match &self.branches {
            Branches::Unbound(_) => PositionsMut::Unbound(&mut self.positions),
            Branches::Bound { order, .. } => {
                let current = BranchOrder::lock(order);
                PositionsMut::Bound {
                    order,
                    slots: current.slots[1..].to_vec(),
                    positions: current.active_positions(),
                }
            }
        }
    }
    /// Move a disconnected document branch. The principal branch is the origin.
    pub fn edit_branch_position(
        &mut self,
        index: usize,
        position: [f32; 2],
    ) -> Result<bool, GtxGraphError<E>> {
        if index == 0 || !position.into_iter().all(f32::is_finite) {
            return Err(GtxGraphError::BranchIndex);
        }
        match &mut self.branches {
            Branches::Unbound(_) => {
                let current = self
                    .positions
                    .get_mut(index - 1)
                    .ok_or(GtxGraphError::BranchIndex)?;
                let changed = current.map(f32::to_bits) != position.map(f32::to_bits);
                *current = position;
                Ok(changed)
            }
            Branches::Bound {
                slots,
                order,
                history,
            } => {
                let mut current = BranchOrder::lock(order);
                let slot = *current.slots.get(index).ok_or(GtxGraphError::BranchIndex)?;
                let before = current.positions[slot];
                if before.map(f32::to_bits) == position.map(f32::to_bits) {
                    return Ok(false);
                }
                current.positions[slot] = position;
                drop(current);
                history
                    .record_graph_edit(
                        slots[slot].history_target(),
                        "Move Disconnected Branch".encode_utf16().collect(),
                        Box::new(BranchPositionEdit {
                            order: order.clone(),
                            slot,
                            before,
                            after: position,
                        }),
                    )
                    .map_err(GtxGraphError::History)?;
                Ok(true)
            }
        }
    }
    /// Read the current branch list. Bound branches project node handles from
    /// their retained editor slots, which history can update directly.
    pub fn branches(&self) -> Cow<'_, [Option<PointwiseNode<E>>]> {
        match &self.branches {
            Branches::Unbound(branches) => Cow::Borrowed(branches),
            Branches::Bound { slots, order, .. } => Cow::Owned(
                BranchOrder::lock(order)
                    .slots
                    .iter()
                    .map(|&id| slots[id].texture_node().map(|logic| logic.node()))
                    .collect(),
            ),
        }
    }
    /// The current node or empty slot at a valid branch index.
    pub fn branch(&self, index: usize) -> Option<Option<PointwiseNode<E>>> {
        match &self.branches {
            Branches::Unbound(branches) => branches.get(index).cloned(),
            Branches::Bound { .. } => self
                .history_slot(index)
                .map(|slot| slot.texture_node().map(|logic| logic.node())),
        }
    }
    /// Add a freshly constructed node as a document root. Bound documents
    /// retain the new occurrence so history can undo and redo its contents.
    pub fn append_branch(&mut self, node: PointwiseNode<E>) -> Result<usize, GtxGraphError<E>> {
        self.append_branch_at(node, [0.; 2])
    }

    /// Add a document root with its position relative to the principal branch.
    /// Undo and redo keep the position with the same retained occurrence.
    pub fn append_branch_at(
        &mut self,
        node: PointwiseNode<E>,
        position: [f32; 2],
    ) -> Result<usize, GtxGraphError<E>> {
        self.insert_document_root(node, position, false)
    }

    /// Dock a tree in the principal branch. The outgoing texture stays in
    /// the graph, taking the incoming tree's loose position when there is one.
    pub fn dock_texture_output(&mut self, node: PointwiseNode<E>) -> Result<(), GtxGraphError<E>> {
        if self
            .branch(0)
            .flatten()
            .is_some_and(|root| root.same_reference(&node))
        {
            return Ok(());
        }
        self.insert_document_root(node, [0.; 2], true).map(|_| ())
    }

    /// Move the complete output tree to a loose branch at the dropped position.
    pub fn detach_texture_output_at(
        &mut self,
        position: [f32; 2],
    ) -> Result<usize, GtxGraphError<E>> {
        let root = self.branch(0).flatten().ok_or(GtxGraphError::BranchIndex)?;
        if let Branches::Unbound(branches) = &mut self.branches {
            if !position.into_iter().all(f32::is_finite) {
                return Err(GtxGraphError::BranchIndex);
            }
            let index = branches.len();
            count_i32(index + 1)?;
            reserve(branches, 1)?;
            reserve(&mut self.positions, 1)?;
            branches[0] = None;
            branches.push(Some(root));
            self.positions.push(position);
            return Ok(index);
        }
        self.append_branch_at(root, position)
    }

    fn insert_document_root(
        &mut self,
        node: PointwiseNode<E>,
        position: [f32; 2],
        dock: bool,
    ) -> Result<usize, GtxGraphError<E>> {
        if !position.into_iter().all(f32::is_finite) {
            return Err(GtxGraphError::BranchIndex);
        }
        let index = self.branches.len();
        count_i32(index + 1)?;
        let loose_source = dock
            .then(|| {
                self.branches()
                    .iter()
                    .enumerate()
                    .skip(1)
                    .find_map(|(index, branch)| {
                        branch
                            .as_ref()
                            .is_some_and(|root| root.same_reference(&node))
                            .then_some(index)
                    })
            })
            .flatten();
        let occupied = dock && self.branch(0).ok_or(GtxGraphError::BranchIndex)?.is_some();
        node.set_document_preview_samples(&self.preview_samples);
        match &mut self.branches {
            Branches::Unbound(branches) => {
                if let Some(source) = loose_source {
                    branches.swap(0, source);
                    if !occupied {
                        branches.remove(source);
                        self.positions.remove(source - 1);
                    }
                } else {
                    reserve(branches, 1)?;
                    reserve(&mut self.positions, 1)?;
                    if dock {
                        node.set_parent(None).map_err(slot_edit_error)?;
                        if let Some(previous) = branches[0].replace(node) {
                            branches.push(Some(previous));
                            self.positions.push(position);
                        }
                    } else {
                        branches.push(Some(node));
                        self.positions.push(position);
                    }
                }
            }
            Branches::Bound {
                slots,
                history,
                order,
            } => {
                reserve(slots, 1)?;
                let physical = slots.len();
                let mut change = BranchOrderChange::new(order, |next| {
                    if !dock {
                        next.push(physical);
                        return;
                    }
                    let previous = std::mem::replace(&mut next[0], physical);
                    match loose_source {
                        Some(source) if occupied => next[source] = previous,
                        Some(source) => {
                            next.remove(source);
                        }
                        None if occupied => next.push(previous),
                        None => {}
                    }
                });
                {
                    let mut current = BranchOrder::lock(order);
                    if occupied && let Some(source) = loose_source {
                        let previous = current.slots[0];
                        change.position = Some(BranchPositionEdit {
                            order: order.clone(),
                            slot: previous,
                            before: current.positions[previous],
                            after: current.positions[current.slots[source]],
                        });
                    }
                    reserve(&mut current.positions, 1)?;
                    current.positions.push(position);
                }
                let slot = crate::PointwiseHistorySlot::bind(&node, history, Vec::new());
                slots.push(slot.clone());
                let label = if dock {
                    "Set Texture Output"
                } else {
                    "Add Branch"
                };
                if let Err(error) = slot.insert_document_branch(&node, change, label) {
                    let mut current = BranchOrder::lock(order);
                    if !current.slots.contains(&physical) {
                        slots.pop();
                        current.positions.pop();
                        drop(current);
                        slot.release_document_binding();
                    }
                    return Err(GtxGraphError::History(error));
                }
            }
        }
        Ok(if dock { 0 } else { index })
    }
    /// Replaces one document root without projecting or cloning node values.
    /// Slot Parent relationships remain owned by the actual graph APIs.
    pub fn set_branch(
        &mut self,
        ix: usize,
        branch: Option<PointwiseNode<E>>,
    ) -> Result<(), GtxGraphError<E>> {
        self.replace_branch(ix, branch, false, None)
    }
    /// Replace an editor branch and record the change when history is bound.
    pub fn edit_branch(
        &mut self,
        ix: usize,
        branch: Option<PointwiseNode<E>>,
    ) -> Result<(), GtxGraphError<E>> {
        self.replace_branch(ix, branch, true, None)
    }

    /// Enclose a document branch in a Group with ports for its external References.
    pub fn group_branch(&mut self, index: usize) -> Result<(), GtxGraphError<E>> {
        let source = self.branch(index).ok_or(GtxGraphError::BranchIndex)?;
        let group = PointwiseNode::group_branch_copy(source.as_ref()).map_err(slot_edit_error)?;
        self.replace_branch(index, Some(group), true, Some("Group Branch"))
    }

    /// Enclose an input or contents branch in a Group in one document edit.
    pub fn group_slot(&mut self, slot: &crate::PointwiseSlot<E>) -> Result<(), GtxGraphError<E>> {
        let group =
            PointwiseNode::group_branch_copy(slot.node().as_ref()).map_err(slot_edit_error)?;
        self.edit_slot_with_label(slot, Some(&group), Some("Group Branch"))
    }

    /// Expand a template Lab into editable basic nodes in one document edit.
    pub fn convert_lab_branch(&mut self, index: usize) -> Result<(), GtxGraphError<E>> {
        let source = self
            .branch(index)
            .flatten()
            .ok_or(GtxGraphError::BranchIndex)?;
        let group = source.converted_lab().map_err(slot_edit_error)?;
        self.replace_branch(index, Some(group), true, Some("Convert Lab to Basic Nodes"))
    }

    /// Expand a template Lab occupying an input or contents slot.
    pub fn convert_lab_slot(
        &mut self,
        slot: &crate::PointwiseSlot<E>,
    ) -> Result<(), GtxGraphError<E>> {
        let source = slot.node().ok_or(GtxGraphError::BranchIndex)?;
        let group = source.converted_lab().map_err(slot_edit_error)?;
        self.edit_slot_with_label(slot, Some(&group), Some("Convert Lab to Basic Nodes"))
    }

    /// Replace an input or contents occurrence and reconcile its Lab links.
    pub fn edit_slot(
        &mut self,
        slot: &crate::PointwiseSlot<E>,
        node: Option<&PointwiseNode<E>>,
    ) -> Result<(), GtxGraphError<E>> {
        self.edit_slot_with_label(slot, node, None)
    }

    fn edit_slot_with_label(
        &mut self,
        slot: &crate::PointwiseSlot<E>,
        node: Option<&PointwiseNode<E>>,
        label: Option<&str>,
    ) -> Result<(), GtxGraphError<E>> {
        if same_branch(slot.node().as_ref(), node) {
            return Ok(());
        }
        let lab = self.stage_lab_assignment(LabAssignment::Slot(slot), node)?;
        // An unbound document-root occurrence has no editor slot for the
        // graph's input setter to vacate. A same-parent unbound move likewise
        // needs an explicit detach from its old occurrence.
        let previous_sibling = if matches!(&self.branches, Branches::Unbound(_)) {
            node.and_then(PointwiseNode::connected_occurrence)
                .filter(|source| source.parent().same_reference(&slot.parent()))
        } else {
            None
        };
        if let Some(source) = &previous_sibling {
            source
                .set_node_quiet(None)
                .map_err(|error| GtxGraphError::Node(error.into()))?;
        }
        let result = slot
            .edit_node_with_lab(node, label, lab)
            .map_err(slot_edit_error);
        if same_branch(slot.node().as_ref(), node) {
            if let Branches::Unbound(roots) = &mut self.branches
                && let Some(node) = node
            {
                for root in roots {
                    if root.as_ref().is_some_and(|root| root.same_reference(node)) {
                        *root = None;
                    }
                }
            }
        } else if let Some(source) = &previous_sibling {
            let _ = source.set_node_quiet(node);
        }
        result
    }

    /// Remove a slot's node and promote its first connected editor input.
    pub fn delete_current_slot(
        &mut self,
        slot: &crate::PointwiseSlot<E>,
    ) -> Result<(), GtxGraphError<E>> {
        let removed = slot.node().ok_or(GtxGraphError::BranchIndex)?;
        let retained = removed
            .first_connected_editor_input()
            .map_err(slot_edit_error)?;
        let lab = self.stage_lab_assignment(LabAssignment::Slot(slot), retained.as_ref())?;
        slot.delete_current_with_lab(lab).map_err(slot_edit_error)
    }

    /// Delete an extra Group contents occurrence including any attached controls.
    pub fn remove_group_content(
        &mut self,
        slot: &crate::PointwiseSlot<E>,
    ) -> Result<(), GtxGraphError<E>> {
        let group = slot.parent().filter().ok_or(GtxGraphError::BranchIndex)?;
        let lab = self.stage_lab_assignment(LabAssignment::Slot(slot), None)?;
        group
            .edit_remove_group_content_with_lab(slot, lab)
            .map_err(slot_edit_error)
    }

    /// Insert a downstream node at an occupied root without replacing the
    /// root's branch occurrence or copying the former subtree.
    pub fn insert_below_branch(
        &mut self,
        ix: usize,
        node: &PointwiseNode<E>,
    ) -> Result<(), GtxGraphError<E>> {
        node.set_document_preview_samples(&self.preview_samples);
        match &mut self.branches {
            Branches::Bound { slots, order, .. } => {
                let physical = *BranchOrder::lock(order)
                    .slots
                    .get(ix)
                    .ok_or(GtxGraphError::BranchIndex)?;
                slots[physical]
                    .insert_below(node)
                    .map_err(|error| GtxGraphError::Node(error.into()))
            }
            Branches::Unbound(branches) => {
                let branch = branches.get_mut(ix).ok_or(GtxGraphError::BranchIndex)?;
                let old = branch.as_ref().ok_or(GtxGraphError::BranchIndex)?;
                let input = node
                    .prepare_insert_below(old)
                    .map_err(|error| GtxGraphError::Node(error.into()))?;
                input
                    .set_node(Some(old))
                    .map_err(|error| GtxGraphError::Node(error.into()))?;
                *branch = Some(node.clone());
                Ok(())
            }
        }
    }

    /// Remove a root node and promote its first connected editor input.
    pub fn delete_current_branch(&mut self, ix: usize) -> Result<(), GtxGraphError<E>> {
        let old = self
            .branch(ix)
            .flatten()
            .ok_or(GtxGraphError::BranchIndex)?;
        let retained = old
            .first_connected_editor_input()
            .map_err(|error| GtxGraphError::Node(error.into()))?;
        if let Some(node) = &retained {
            node.set_document_preview_samples(&self.preview_samples);
        }
        let lab = self.stage_lab_assignment(LabAssignment::Branch(ix), retained.as_ref())?;
        match &mut self.branches {
            Branches::Bound { slots, order, .. } => {
                let physical = *BranchOrder::lock(order)
                    .slots
                    .get(ix)
                    .ok_or(GtxGraphError::BranchIndex)?;
                let change = (ix > 0 && retained.is_none()).then(|| {
                    BranchOrderChange::new(order, |next| {
                        next.remove(ix);
                    })
                });
                slots[physical]
                    .delete_current(retained.as_ref(), change, lab)
                    .map_err(GtxGraphError::History)
            }
            Branches::Unbound(branches) => {
                let notify = lab
                    .as_ref()
                    .map(|lab| lab.publish(false))
                    .transpose()
                    .map_err(GtxGraphError::History)?;
                if let Some(node) = &retained {
                    node.set_parent(None)
                        .map_err(|error| GtxGraphError::Node(error.into()))?;
                    branches[ix] = retained;
                } else if ix == 0 {
                    branches[ix] = None;
                } else {
                    branches.remove(ix);
                    self.positions.remove(ix - 1);
                }
                notify.map_or(Ok(()), |affected| {
                    affected.notify().map_err(GtxGraphError::History)
                })
            }
        }
    }

    /// Enclose the current branches in a Group and place a fresh node in
    /// its first exterior input.
    pub fn insert_document_input(
        &mut self,
        input: &PointwiseNode<E>,
    ) -> Result<(), GtxGraphError<E>> {
        let roots = self.branches().into_owned();
        if roots
            .iter()
            .flatten()
            .any(|root| root.same_reference(input))
        {
            return Err(slot_edit_error(PointwiseError::InvalidOperation));
        }
        let edit = input
            .prepare_document_input(roots.first().and_then(Option::as_ref))
            .map_err(slot_edit_error)?;
        let projection =
            lab_preset_scope::LabProjection::read(&roots, None, edit.enclosure_node(), |slot| {
                edit.projected_node(slot)
            });
        let lab = self.stage_lab_layout(&roots, &projection)?;
        self.apply_document_enclosure(edit, &roots, lab)
            .map_err(slot_edit_error)
    }

    /// Feed another input from a node that already feeds a connected occurrence.
    /// The new connection is represented by real numbered Group References.
    pub fn reference_copy_output(
        &mut self,
        source: &PointwiseNode<E>,
        destination: &crate::PointwiseSlot<E>,
    ) -> Result<(), GtxGraphError<E>> {
        use crate::PointwiseError;

        let check_membership = |mut node: PointwiseNode<E>| {
            let mut visited = Vec::new();
            while let Some(parent) = node.parent() {
                if visited.iter().any(|old| node.same_reference(old)) {
                    return Err(PointwiseError::InvalidOperation);
                }
                visited.push(node);
                node = parent;
            }
            if self
                .branches()
                .iter()
                .flatten()
                .any(|root| root.same_reference(&node))
            {
                Ok(())
            } else {
                Err(PointwiseError::InvalidOperation)
            }
        };
        let checked = (|| {
            let from = source
                .connected_occurrence()
                .ok_or(PointwiseError::InvalidOperation)?;
            from.validate_reference_copy(destination)?;
            check_membership(source.clone())?;
            check_membership(destination.parent())?;
            let group = from.reference_scope()?;
            let target_group = destination.reference_scope()?;
            if !match (&group, &target_group) {
                (None, None) => true,
                (Some(a), Some(b)) => a.same_reference(b),
                _ => false,
            } {
                return Err(PointwiseError::UnsupportedCapability(
                    "sharing across Group levels",
                ));
            }
            Ok(from)
        })();
        let from = checked.map_err(|error| GtxGraphError::Node(error.into()))?;
        let edit = from
            .prepare_reference_copy(destination)
            .map_err(|error| GtxGraphError::Node(error.into()))?;
        let roots = self.branches().into_owned();
        let projection =
            lab_preset_scope::LabProjection::read(&roots, None, edit.enclosure_node(), |slot| {
                edit.projected_node(slot)
            });
        let lab = self.stage_lab_layout(&roots, &projection)?;
        let result = if edit.needs_enclosure() {
            edit.validate_reference_enclosure(&roots)
                .and_then(|()| self.apply_document_enclosure(edit, &roots, lab))
        } else {
            edit.apply(destination, lab)
        };
        result.map_err(|error| GtxGraphError::Node(error.into()))
    }

    fn apply_document_enclosure(
        &mut self,
        edit: GraphConnectionEdit<E>,
        roots: &[Option<PointwiseNode<E>>],
        lab: Option<LabLayoutEdit<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let positions = self.positions().into_owned();
        match &mut self.branches {
            Branches::Bound { slots, order, .. } => {
                let branches: Vec<_> = BranchOrder::lock(order)
                    .slots
                    .iter()
                    .map(|&id| slots[id].clone())
                    .collect();
                let change = BranchOrderChange::new(order, |next| next.truncate(1));
                edit.enclose_roots(
                    roots,
                    &positions,
                    &self.preview_samples,
                    Some((&branches, change)),
                    lab,
                    |_| {},
                )
            }
            Branches::Unbound(branches) => edit.enclose_roots(
                roots,
                &positions,
                &self.preview_samples,
                None,
                lab,
                |group| {
                    *branches = vec![Some(group)];
                    self.positions.clear();
                },
            ),
        }
    }

    fn replace_branch(
        &mut self,
        ix: usize,
        branch: Option<PointwiseNode<E>>,
        record: bool,
        label: Option<&str>,
    ) -> Result<(), GtxGraphError<E>> {
        let lab = if record {
            let current = self.branch(ix).ok_or(GtxGraphError::BranchIndex)?;
            if same_branch(current.as_ref(), branch.as_ref()) {
                return Ok(());
            }
            self.stage_lab_assignment(LabAssignment::Branch(ix), branch.as_ref())?
        } else {
            None
        };
        if let Some(node) = &branch {
            node.set_document_preview_samples(&self.preview_samples);
        }
        match &mut self.branches {
            Branches::Bound { slots, order, .. } => {
                let physical = *BranchOrder::lock(order)
                    .slots
                    .get(ix)
                    .ok_or(GtxGraphError::BranchIndex)?;
                slots[physical]
                    .replace_document_branch_with_lab(branch.as_ref(), record, label, lab)
                    .map_err(GtxGraphError::History)
            }
            Branches::Unbound(branches) => {
                if record && let Some(node) = &branch {
                    node.set_parent(None).map_err(slot_edit_error)?;
                    for (index, root) in branches.iter_mut().enumerate() {
                        if index != ix
                            && root.as_ref().is_some_and(|root| root.same_reference(node))
                        {
                            *root = None;
                        }
                    }
                }
                let notify = lab
                    .as_ref()
                    .map(|lab| lab.publish(false))
                    .transpose()
                    .map_err(GtxGraphError::History)?;
                *branches.get_mut(ix).ok_or(GtxGraphError::BranchIndex)? = branch;
                notify.map_or(Ok(()), |affected| {
                    affected.notify().map_err(GtxGraphError::History)
                })
            }
        }
    }
    pub fn output_order(&self) -> [i32; 3] {
        *preview_samples(&self.preview_samples)
    }
    /// Set the raw GTX trailer. Use `set_filter_preview_sample` for a live edit.
    pub fn set_output_order(&mut self, order: [i32; 3]) {
        *preview_samples(&self.preview_samples) = order;
    }

    /// Return the first active filter input's saved sample index.
    pub fn filter_preview_sample_index(&self) -> Option<i32> {
        self.branch(0)
            .flatten()
            .filter(|root| {
                root.is_filter()
                    && principal_input_count(root).is_ok_and(|n| n > 0)
                    && root
                        .slot(0)
                        .ok()
                        .and_then(|slot| slot.node())
                        .is_some_and(|node| node.is_default_input())
            })
            .map(|_| self.output_order()[0])
    }

    /// Change the first filter sample and record the live links and saved
    /// sample together when document history is active.
    pub fn set_filter_preview_sample(
        &mut self,
        sample: FilterPreviewSample,
    ) -> Result<bool, GtxGraphError<E>> {
        let before = self
            .filter_preview_sample_index()
            .ok_or(GtxGraphError::Node(PointwiseError::InvalidOperation.into()))?;
        let after = sample.index();
        if before == after {
            return Ok(false);
        }
        let root = self.branch(0).flatten().ok_or(GtxGraphError::BranchIndex)?;
        root.edit_filter_preview_sample(
            self.history_slot(0).as_ref(),
            PreviewSampleChange {
                samples: self.preview_samples.clone(),
                before,
                after,
            },
        )
        .map_err(|error| GtxGraphError::Node(error.into()))?;
        Ok(true)
    }
    /// Position after the decoded document, not an assertion that input is EOF.
    pub fn consumed(&self) -> i32 {
        self.consumed
    }
    /// Writes retained file metadata and the actual live nodes in wire order.
    /// Node Save may lazily create property children and may fail after partial
    /// writes. No immutable render frame or prior original bytes supply values.
    /// Preview regeneration, host progress and file replacement are separate.
    pub fn save(&mut self, writer: &mut BinaryWriter) -> Result<(), GtxGraphError<E>> {
        self.save_graph(writer, false)
    }

    /// Writes the native Mycela project format.
    pub fn save_project(&mut self, writer: &mut BinaryWriter) -> Result<(), GtxGraphError<E>> {
        self.save_graph(writer, true)
    }

    /// Reports graph content the Genetica reader cannot interpret.
    pub fn gtx_incompatibilities(&self) -> Result<Vec<GtxIncompatibility>, GtxGraphError<E>> {
        let mut issues = Vec::new();
        for (index, root) in self.branches().iter().enumerate() {
            let mut nodes = root.iter().cloned().collect::<Vec<_>>();
            let mut inspected = std::collections::BTreeSet::new();
            while let Some(node) = nodes.pop() {
                if !inspected.insert(node.graph_identity()) {
                    continue;
                }
                for reason in node.gtx_incompatibilities().map_err(GtxGraphError::Node)? {
                    issues.push(GtxIncompatibility {
                        branch: index + 1,
                        node: String::from_utf16_lossy(&node.identity_name()),
                        reason,
                    });
                }
                nodes.extend(node.gtx_record_children());
            }
        }
        issues.sort();
        issues.dedup();
        Ok(issues)
    }

    /// Writes a GTX only when the compatibility preflight succeeds.
    pub fn save_compatible_gtx(
        &mut self,
        writer: &mut BinaryWriter,
    ) -> Result<(), GtxGraphError<E>> {
        let issues = self.gtx_incompatibilities()?;
        if !issues.is_empty() {
            return Err(GtxGraphError::IncompatibleGtx(issues));
        }
        self.save(writer)
    }

    fn save_graph(
        &mut self,
        writer: &mut BinaryWriter,
        project: bool,
    ) -> Result<(), GtxGraphError<E>> {
        let (positions, branches) = match &self.branches {
            Branches::Unbound(branches) => (
                Cow::Borrowed(self.positions.as_slice()),
                Cow::Borrowed(branches.as_slice()),
            ),
            Branches::Bound { slots, order, .. } => {
                let order = BranchOrder::lock(order);
                let branches = order
                    .slots
                    .iter()
                    .map(|&id| slots[id].texture_node().map(|logic| logic.node()))
                    .collect();
                (Cow::Owned(order.active_positions()), Cow::Owned(branches))
            }
        };
        let filter_inputs = branches
            .first()
            .and_then(Option::as_ref)
            .filter(|node| node.is_filter())
            .map(principal_input_count)
            .transpose()?
            .unwrap_or(0);
        // Style records write the selected effect. Controls whose only
        // receivers are in parked effects have no receiver in the saved graph.
        let parked = self.parked_only_lab_names(&branches, None);
        if project {
            writer.write_bytes(PROJECT_MAGIC)?;
            writer.write_i32(PROJECT_VERSION)?;
            self.header.save_body(writer, filter_inputs)?;
        } else {
            self.header.save_with_filter_inputs(writer, filter_inputs)?;
        }
        write_front_matter(
            &mut self.presets,
            self.catalog
                .active_mut()
                .into_iter()
                .filter(|entry| !parked.contains(entry.metadata()))
                .collect(),
            &positions,
            branches.len(),
            writer,
        )?;
        for branch in branches.iter() {
            match branch {
                None => writer.write_utf16(&"null".encode_utf16().collect::<Vec<_>>())?,
                Some(node) => node
                    .save_document_binary(writer, project)
                    .map_err(GtxGraphError::Node)?,
            }
        }
        for channel in self.output_order() {
            writer.write_i32(channel)?;
        }
        Ok(())
    }
}

fn principal_input_count<E: 'static>(node: &PointwiseNode<E>) -> Result<i32, GtxGraphError<E>> {
    if node.contents().is_empty() {
        Ok(count_i32(node.slots().len())?)
    } else {
        let inputs = node
            .reference_inputs()
            .map_err(|error| GtxGraphError::Node(error.into()))?;
        Ok(count_i32(inputs.len())?)
    }
}

fn prepare_filter_document<E: 'static>(
    principal: Option<&PointwiseNode<E>>,
    inputs: i32,
) -> Result<(), GtxGraphError<E>> {
    if inputs == 0 {
        return Ok(());
    }
    let root = principal.ok_or(GtxGraphError::Node(NodeWireError::UnsupportedCapability(
        "filter document without a principal node",
    )))?;
    if principal_input_count(root)? != inputs
        || root.slots().iter().any(|slot| slot.node().is_some())
    {
        return Err(GtxGraphError::Node(NodeWireError::UnsupportedCapability(
            "filter document input layout",
        )));
    }
    root.convert_to_filter(false)
        .map_err(|error| GtxGraphError::Node(error.into()))
}

#[derive(Debug)]
pub enum GtxGraphError<E = ()> {
    FrontMatter(GtxReadError),
    Node(NodeWireError<E>),
    /// Rust document-list API error, not an asserted original exception class.
    BranchIndex,
    CurveAdjuster(&'static str),
    EnvironmentAdjuster(&'static str),
    SynthesisAdjuster(&'static str),
    NumericAdjuster(&'static str),
    LabPreset(&'static str),
    LabColorAdjuster(&'static str),
    IncompatibleGtx(Vec<GtxIncompatibility>),
    History(crate::HistoryError<E>),
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GtxIncompatibility {
    branch: usize,
    node: String,
    reason: &'static str,
}

impl fmt::Display for GtxIncompatibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Branch {} · {} — {}",
            self.branch, self.node, self.reason
        )
    }
}
impl<E> From<GtxReadError> for GtxGraphError<E> {
    fn from(error: GtxReadError) -> Self {
        Self::FrontMatter(error)
    }
}
impl<E> From<BinaryError> for GtxGraphError<E> {
    fn from(error: BinaryError) -> Self {
        Self::FrontMatter(error.into())
    }
}
impl<E: fmt::Debug> fmt::Display for GtxGraphError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::Node(
            error @ (NodeWireError::UnimplementedNode { .. }
            | NodeWireError::Property(crate::LivePropertyWireError::UnknownChoice {
                ..
            })),
        ) = self
        {
            return fmt::Display::fmt(error, f);
        }
        if let Self::FrontMatter(
            error @ (GtxReadError::ProjectSignature
            | GtxReadError::ProjectVersion(_)
            | GtxReadError::TrailingProjectData),
        ) = self
        {
            return fmt::Display::fmt(error, f);
        }
        if let Self::IncompatibleGtx(issues) = self {
            for (index, issue) in issues.iter().enumerate() {
                if index != 0 {
                    f.write_str("\n")?;
                }
                fmt::Display::fmt(issue, f)?;
            }
            return Ok(());
        }
        if let Self::CurveAdjuster(message)
        | Self::EnvironmentAdjuster(message)
        | Self::SynthesisAdjuster(message)
        | Self::NumericAdjuster(message)
        | Self::LabPreset(message)
        | Self::LabColorAdjuster(message) = self
        {
            return f.write_str(message);
        }
        write!(f, "GTX graph: {self:?}")
    }
}
impl<E: fmt::Debug + 'static> Error for GtxGraphError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::FrontMatter(e) => Some(e),
            Self::Node(e) => Some(e),
            Self::BranchIndex
            | Self::CurveAdjuster(_)
            | Self::EnvironmentAdjuster(_)
            | Self::SynthesisAdjuster(_)
            | Self::NumericAdjuster(_)
            | Self::LabPreset(_)
            | Self::LabColorAdjuster(_)
            | Self::IncompatibleGtx(_)
            | Self::History(_) => None,
        }
    }
}
#[derive(Debug)]
pub struct GtxGraphLoadFailure<E = ()> {
    position: i32,
    kind: GtxGraphError<E>,
}
impl<E> GtxGraphLoadFailure<E> {
    pub fn position(&self) -> i32 {
        self.position
    }
    pub fn kind(&self) -> &GtxGraphError<E> {
        &self.kind
    }
}
impl<E: fmt::Debug> fmt::Display for GtxGraphLoadFailure<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GTX graph at {}: {}", self.position, self.kind)
    }
}
impl<E: fmt::Debug + 'static> Error for GtxGraphLoadFailure<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.kind)
    }
}

/// zl.a8/os.bb post-load migration for the retained node families currently
/// recognized by the binary factory. Group contents remain unimplemented and
/// cannot reach this path through a falsely successful load.
fn migrate_legacy_seeds<E: 'static>(node: &PointwiseNode<E>) -> Result<(), NodeWireError<E>> {
    if let Some(filter) = node.filter() {
        for parameter in filter.parameters() {
            if parameter.name().iter().copied().eq("Seed".encode_utf16())
                && matches!(
                    parameter.value(),
                    crate::PointwiseParameterValue::Number { .. }
                )
                && parameter
                    .retained_limits()
                    .is_some_and(|limits| limits.snapshot().minimum.to_framework_f64() < 0.0)
            {
                parameter.set_number(-(parameter.number() + 1.0))?;
            }
        }
    }
    if let Some(generator) = node.image_generator_handle() {
        for parameter in generator.parameters() {
            if parameter.name() == "Seed"
                && parameter.is_scalar()
                && parameter.limits()?.snapshot().minimum.to_framework_f64() < 0.0
            {
                parameter.set_number(-(parameter.number()? + 1.0))?;
            }
        }
    }
    for slot in node.slots() {
        if let Some(child) = slot.node() {
            migrate_legacy_seeds(&child)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod legacy_seed_tests {
    use super::*;
    #[test]
    fn migration_uses_retained_seed_and_traverses_actual_slots() {
        let generator =
            crate::ImageGenerator::<()>::with_kind(crate::ImageGeneratorKind::Noise).unwrap();
        let seed = generator.parameter(crate::ImageGeneratorParameterKind::Seed);
        seed.set_number(-12.0).unwrap();
        let parent = PointwiseNode::<()>::invert();
        parent
            .slot(0)
            .unwrap()
            .set_node(Some(&generator.node()))
            .unwrap();
        migrate_legacy_seeds(&parent).unwrap();
        assert_eq!(seed.number().unwrap(), 11.0);
    }
}

#[cfg(test)]
mod environment_catalog_tests {
    use super::*;
    use crate::{
        EnvironmentField, EnvironmentLayerKind, EnvironmentRecord, ImageGeometry, PointwiseFilter,
        PointwiseKind, PointwiseRenderEffects, PointwiseRenderOptions, ProceduralExecution,
        RenderCaptureSession,
    };

    fn red_map(red: f64) -> EnvironmentRecord {
        let mut map = EnvironmentRecord::default();
        map.add_layer(EnvironmentLayerKind::Color).unwrap();
        map.set_field(EnvironmentField::Red(0), red).unwrap();
        map
    }

    fn environment_paint_node(kind: PointwiseKind, red: f64) -> PointwiseFilter<()> {
        let filter = PointwiseFilter::new(kind).unwrap();
        if kind == PointwiseKind::Canvas {
            filter
                .edit_add_canvas_shape(crate::CanvasPrimitive::Rectangle, None)
                .unwrap();
        }
        let mut edit = environment_style_edit(&filter);
        edit.set_paint(0, crate::StylePaintKind::Environment)
            .unwrap();
        let initial = edit.environment_map(0).unwrap().clone();
        edit.replace_environment_map(0, &initial, red_map(red))
            .unwrap();
        if let Some(layer) = filter.canvas_layer_editors().first() {
            edit.duplicate(0).unwrap();
            layer.edit_style(edit, "Set Environment paints").unwrap();
        } else {
            filter.edit_style(edit, "Set Environment paint").unwrap();
        }
        filter
    }

    fn environment_style_edit(filter: &PointwiseFilter<()>) -> crate::StyleEdit {
        if let Some(layer) = filter.canvas_layer_editors().first() {
            layer.style_edit().unwrap()
        } else {
            filter.style_edit().unwrap()
        }
    }

    fn link_environment_paint(
        doc: &mut Gtx375GraphDocument<()>,
        filter: &PointwiseFilter<()>,
        pass: usize,
        link: EnvironmentAdjusterLink,
    ) {
        let layers = filter.canvas_layer_editors();
        let edit = environment_style_edit(filter);
        assert!(
            doc.set_style_environment_adjuster(
                &filter.node(),
                layers.first(),
                pass,
                edit.environment_map(pass).unwrap(),
                link,
            )
            .unwrap()
        );
    }

    fn paint_adjuster(
        doc: &Gtx375GraphDocument<()>,
        filter: &PointwiseFilter<()>,
        guid: [u8; 16],
        red: f64,
    ) -> EnvironmentAdjuster {
        let edit = environment_style_edit(filter);
        let adjuster = doc.style_environment_adjuster(&edit, 0).unwrap();
        for pass in 0..edit.count() {
            assert_eq!(edit.environment_adjuster_key(pass).unwrap().0, guid);
            assert!(Arc::ptr_eq(
                edit.environment_map(pass).unwrap(),
                &adjuster.value()
            ));
        }
        assert_eq!(adjuster.value().field(EnvironmentField::Red(0)), Some(red));
        adjuster
    }

    #[test]
    fn ordinary_color_adjuster_history_and_reopen_preserve_owned_rgb() {
        let first = PointwiseFilter::<()>::new(PointwiseKind::Selector).unwrap();
        let second = PointwiseFilter::new(PointwiseKind::Selector).unwrap();
        let a = first.parameter(0).unwrap();
        let b = second.parameter(0).unwrap();
        let initial = [0.5, 0.25, 0.125];
        let other = [0.25, 0.75, 0.125];
        let edited = [0.75, 0.125, 0.5];
        a.set_color(crate::ContextColorValue::unlinked(initial))
            .unwrap();
        b.set_color(crate::ContextColorValue::unlinked(other))
            .unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(first.node()), Some(second.node())]).unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);

        assert!(doc.available_color_adjusters(&a).is_empty());
        assert!(
            doc.set_color_adjuster(&a, ColorAdjusterLink::Create("  Selection Hue  ".into()))
                .unwrap()
        );
        let adjuster = doc.color_adjuster(&a).unwrap();
        assert_eq!(adjuster.name(), "Selection Hue");
        assert!(doc.available_color_adjusters(&b)[0].same_reference(&adjuster));
        assert!(
            doc.set_color_adjuster(&b, ColorAdjusterLink::Existing(adjuster.clone()))
                .unwrap()
        );
        assert!(
            !doc.set_color_adjuster(&b, ColorAdjusterLink::Existing(adjuster.clone()))
                .unwrap()
        );
        assert_eq!(b.color().unwrap().components(), initial);
        assert_eq!(history.commands().len(), 2);
        let capture = || RenderCaptureSession::new().capture_filter(&second).unwrap();
        let previous_render = capture();
        assert!(
            doc.edit_color_lab_adjuster(
                Some(&first.node()),
                &adjuster,
                &LabColorValue::Color(initial),
                LabColorValue::Color(edited)
            )
            .unwrap()
        );
        assert_eq!(a.color().unwrap().components(), edited);
        assert_eq!(b.effective_color_at(0.).unwrap().components(), edited);
        let updated_render = capture();
        assert!(!updated_render.same_render_values(&previous_render));
        history.undo().unwrap();
        assert!(capture().same_render_values(&previous_render));
        history.redo().unwrap();
        assert!(capture().same_render_values(&updated_render));

        for native in [true, false] {
            let mut wire = BinaryWriter::default();
            let mut reopened = if native {
                doc.save_project(&mut wire).unwrap();
                Gtx375GraphDocument::<()>::load_project(
                    wire.bytes(),
                    crate::NodeWireReadPolicy::new(32, 128),
                )
                .unwrap()
            } else {
                doc.save_compatible_gtx(&mut wire).unwrap();
                Gtx375GraphDocument::<()>::load(wire.bytes()).unwrap()
            };
            let [a, b] = [0, 1].map(|branch| {
                reopened
                    .branch(branch)
                    .flatten()
                    .unwrap()
                    .filter()
                    .unwrap()
                    .parameter(0)
                    .unwrap()
            });
            assert!(
                reopened
                    .color_adjuster(&a)
                    .unwrap()
                    .same_reference(&reopened.color_adjuster(&b).unwrap())
            );
            assert_eq!(b.color().unwrap().components(), edited);
            reopened
                .set_color_adjuster(&a, ColorAdjusterLink::Unlink)
                .unwrap();
            assert_eq!(a.color().unwrap().components(), initial);
            assert_eq!(b.color().unwrap().components(), edited);
            assert_eq!(reopened.available_color_adjusters(&a).len(), 1);
            assert!(
                reopened
                    .set_color_adjuster(&b, ColorAdjusterLink::Unlink)
                    .unwrap()
            );
            assert_eq!(b.color().unwrap().components(), other);
            assert!(reopened.available_color_adjusters(&b).is_empty());
        }
        doc.set_color_adjuster(&a, ColorAdjusterLink::Unlink)
            .unwrap();
        doc.set_color_adjuster(&b, ColorAdjusterLink::Unlink)
            .unwrap();
        assert!(doc.available_color_adjusters(&a).is_empty());
        history.undo().unwrap();
        assert_eq!(b.color().unwrap().components(), edited);
        assert_eq!(doc.available_color_adjusters(&a).len(), 1);
        history.redo().unwrap();
        assert_eq!(b.color().unwrap().components(), other);
        assert!(doc.available_color_adjusters(&a).is_empty());
    }

    #[test]
    fn ordinary_color_adjuster_keeps_solid_and_environment_receivers() {
        let selector = PointwiseFilter::<()>::new(PointwiseKind::Selector).unwrap();
        let property = selector.parameter(0).unwrap();
        let environment = PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        let map = environment.parameter(0).unwrap();
        map.set_environment(Arc::new(red_map(0.25))).unwrap();
        let solid = PointwiseNode::solid_color().unwrap();
        let solid_adjustment = solid
            .solid_model()
            .unwrap()
            .color_property()
            .adjustment()
            .unwrap()
            .unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![
            Some(selector.node()),
            Some(environment.node()),
            Some(solid),
        ])
        .unwrap();
        doc.set_color_adjuster(&property, ColorAdjusterLink::Create("Fill".into()))
            .unwrap();
        let control = doc.color_adjuster(&property).unwrap();
        let field = crate::EnvironmentColorField::LayerColor(0);
        doc.set_environment_color_adjuster(
            EnvironmentMapTarget::Property(&map),
            &map.effective_environment().unwrap(),
            field,
            EnvironmentColorLink::Existing(control.clone()),
        )
        .unwrap();
        doc.set_color_adjuster(&property, ColorAdjusterLink::Unlink)
            .unwrap();
        assert!(doc.available_color_adjusters(&property)[0].same_reference(&control));
        assert!(
            doc.environment_color_adjuster(&map.effective_environment().unwrap(), field)
                .is_some()
        );

        doc.set_color_adjuster(&property, ColorAdjusterLink::Existing(control.clone()))
            .unwrap();
        doc.set_environment_color_adjuster(
            EnvironmentMapTarget::Property(&map),
            &map.effective_environment().unwrap(),
            field,
            EnvironmentColorLink::Unlink,
        )
        .unwrap();
        solid_adjustment
            .names()
            .set_entry(0, Some("Fill".encode_utf16().collect()))
            .unwrap();
        solid_adjustment.flags().set(0, true).unwrap();
        doc.set_color_adjuster(&property, ColorAdjusterLink::Unlink)
            .unwrap();
        assert!(doc.available_color_adjusters(&property)[0].same_reference(&control));
    }

    #[test]
    fn author_color_and_gradient_links_on_environment_fields() {
        use crate::{EnvironmentColorField as Field, SharedGradient};

        let make = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        let color = make.parameter(0).unwrap();
        let mut map = red_map(0.25);
        map.set_recolor_enabled(0, true).unwrap();
        color.set_environment(Arc::new(map)).unwrap();
        let style = environment_paint_node(PointwiseKind::Style, 0.6);
        let canvas = environment_paint_node(PointwiseKind::Canvas, 0.75);
        let mut doc = Gtx375GraphDocument::new(
            [&make, &style, &canvas]
                .map(|filter| Some(filter.node()))
                .to_vec(),
        )
        .unwrap();

        doc.set_environment_adjuster(&color, EnvironmentAdjusterLink::Create("Sky".into()))
            .unwrap();
        let sky = doc.environment_adjuster(&color).unwrap();
        for pass in 0..2 {
            link_environment_paint(
                &mut doc,
                &canvas,
                pass,
                EnvironmentAdjusterLink::Existing(sky.clone()),
            );
        }
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let stale = sky.value();
        let main = EnvironmentMapTarget::Property(&color);
        assert!(
            doc.set_environment_color_adjuster(
                main,
                &stale,
                Field::LayerColor(0),
                EnvironmentColorLink::Create("Fill".into()),
            )
            .unwrap()
        );
        assert!(
            doc.set_environment_color_adjuster(
                EnvironmentMapTarget::Property(&color),
                &stale,
                Field::Colorize(0),
                EnvironmentColorLink::Unlink,
            )
            .is_err()
        );
        let fill = doc
            .environment_color_adjuster(&sky.value(), Field::LayerColor(0))
            .unwrap();

        let style_node = style.node();
        let style_map = || {
            environment_style_edit(&style)
                .environment_map(0)
                .unwrap()
                .clone()
        };
        let style_target = || EnvironmentMapTarget::Paint {
            owner: &style_node,
            layer: None,
            pass: 0,
        };
        assert!(
            doc.set_environment_color_adjuster(
                style_target(),
                &style_map(),
                Field::Colorize(0),
                EnvironmentColorLink::Existing(fill.clone()),
            )
            .unwrap()
        );
        assert_eq!(style_map().field(EnvironmentField::Tint(0, 0)), Some(0.25));
        let canvas_node = canvas.node();
        let layers = canvas.canvas_layer_editors();
        let canvas_target = || EnvironmentMapTarget::Paint {
            owner: &canvas_node,
            layer: Some(&layers[0]),
            pass: 1,
        };
        assert!(
            doc.set_environment_color_adjuster(
                canvas_target(),
                &sky.value(),
                Field::Recolor(0),
                EnvironmentColorLink::Create("Ramp".into()),
            )
            .unwrap()
        );
        assert!(
            doc.set_environment_color_adjuster(
                canvas_target(),
                &sky.value(),
                Field::Recolor(0),
                EnvironmentColorLink::Existing(fill.clone()),
            )
            .is_err()
        );
        assert_eq!(history.cursor(), 3);
        let ramp = doc
            .environment_color_adjuster(&sky.value(), Field::Recolor(0))
            .unwrap();
        assert_eq!(
            doc.available_environment_color_adjusters(canvas_target(), Field::Recolor(0))
                .len(),
            1
        );
        let gradient = SharedGradient::new();
        for position in [0., 1.] {
            gradient
                .add_normalized_color(position, 0.125, 0.5, 0.25)
                .unwrap();
            gradient.add_normalized_alpha(position, 0.5).unwrap();
        }
        assert!(
            doc.edit_color_lab_adjuster(
                None,
                &ramp,
                &ramp.draft_value().unwrap(),
                LabColorValue::Gradient(gradient.clone()),
            )
            .unwrap()
        );
        let check = |doc: &Gtx375GraphDocument<()>| {
            let color = doc
                .branch(0)
                .flatten()
                .unwrap()
                .filter()
                .unwrap()
                .parameter(0)
                .unwrap();
            let map = color.effective_environment().unwrap();
            assert!(
                map.copy_recolor_gradient(0)
                    .unwrap()
                    .unwrap()
                    .source_equals(Some(&gradient))
                    .unwrap()
            );
            let canvas = doc.branch(2).flatten().unwrap().filter().unwrap();
            let paint = environment_style_edit(&canvas);
            for pass in 0..2 {
                assert!(Arc::ptr_eq(&map, paint.environment_map(pass).unwrap()));
            }
            assert!(
                doc.environment_color_adjuster(&map, Field::Recolor(0))
                    .is_some()
            );
        };
        check(&doc);

        assert!(
            doc.set_environment_color_adjuster(
                EnvironmentMapTarget::Property(&color),
                &sky.value(),
                Field::LayerColor(0),
                EnvironmentColorLink::Unlink,
            )
            .unwrap()
        );
        assert!(
            doc.environment_color_adjuster(&style_map(), Field::Colorize(0))
                .is_some()
        );
        assert!(
            doc.set_environment_color_adjuster(
                style_target(),
                &style_map(),
                Field::Colorize(0),
                EnvironmentColorLink::Unlink,
            )
            .unwrap()
        );
        assert_eq!(style_map().field(EnvironmentField::Tint(0, 0)), Some(0.25));
        assert_eq!(doc.color_lab_adjusters(None).len(), 1);
        history.undo().unwrap();
        assert_eq!(doc.color_lab_adjusters(None).len(), 2);
        assert!(
            doc.environment_color_adjuster(&style_map(), Field::Colorize(0))
                .is_some()
        );
        history.redo().unwrap();
        assert_eq!(doc.color_lab_adjusters(None).len(), 1);
        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        let reopened = Gtx375GraphDocument::load_project(
            native.bytes(),
            crate::NodeWireReadPolicy::new(128, 4096),
        )
        .unwrap();
        check(&reopened);
        let mut gtx = BinaryWriter::default();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        check(&Gtx375GraphDocument::load(gtx.bytes()).unwrap());
    }

    #[test]
    fn environment_colors_and_recolor_gradients_restore_from_the_lab_catalog() {
        use crate::{
            AdjustmentRecord, ReferenceAdjusterKind, ReferenceAdjusterRecord, ReferenceValueHandle,
            SharedGradient, ValueAdjusterPayload,
        };

        let name = |text: &str| Some(text.encode_utf16().collect());
        let link = |text: &str, tag, scope| {
            let mut adjustment = AdjustmentRecord::new(&[[0., 0.]]);
            adjustment.guid = scope;
            adjustment.property_type = tag;
            adjustment.components_mut()[0].name = name(text).unwrap();
            adjustment.components_mut()[0].active = true;
            adjustment
        };
        let ramp = |gradient: &SharedGradient| {
            for position in [0., 1.] {
                gradient
                    .add_normalized_color(position, 0.75, 0.375, 0.1875)
                    .unwrap();
                gradient.add_normalized_alpha(position, 0.75).unwrap();
            }
        };
        let mut before = red_map(0.25);
        before.set_recolor_enabled(0, true).unwrap();
        before.install_test_adjustment(EnvironmentField::Red(0), link("Fill", 6, [0; 16]));
        before.install_test_adjustment(EnvironmentField::Tint(0, 0), link("Tint", 6, [0; 16]));
        before.install_test_recolor_adjustment(0, link("Ramp", 7, [0; 16]));
        let mut wrong_scope = before.clone();
        wrong_scope.install_test_adjustment(EnvironmentField::Red(0), link("Fill", 6, [5; 16]));
        let make = |map| {
            let node = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
            node.parameter(0)
                .unwrap()
                .set_environment(Arc::new(map))
                .unwrap();
            node
        };
        let local = make(before.clone());
        let shared = make(before.clone());
        let unrelated = make(wrong_scope);
        let style = environment_paint_node(PointwiseKind::Style, 0.25);
        let mut style_edit = style.style_edit().unwrap();
        let old = style_edit.environment_map(0).unwrap().clone();
        style_edit.replace_environment_map(0, &old, before).unwrap();
        style
            .edit_style(style_edit, "Set linked Environment fields")
            .unwrap();
        let canvas = environment_paint_node(PointwiseKind::Canvas, 0.25);
        let solid = PointwiseNode::solid_color().unwrap();
        let graph_ramp = PointwiseFilter::new(PointwiseKind::Colorize).unwrap();
        let ramp_parameter = |filter: &PointwiseFilter<()>| {
            filter
                .retained_parameters()
                .into_iter()
                .find(|property| {
                    matches!(
                        property.raw_value(),
                        crate::PointwiseParameterValue::Gradient(_)
                    )
                })
                .unwrap()
        };
        let solid_color = solid.solid_model().unwrap().color_property();
        solid_color.set_color([0.125; 3]).unwrap();
        let gradient_property = ramp_parameter(&graph_ramp);
        for (adjustment, tag, text) in [
            (solid_color.adjustment().unwrap().unwrap(), 6, "Fill"),
            (gradient_property.adjustment().unwrap().unwrap(), 7, "Ramp"),
        ] {
            assert_eq!(adjustment.property_type(), tag);
            adjustment.names().set_entry(0, name(text)).unwrap();
            adjustment.flags().set(0, true).unwrap();
        }
        let mut branches = [&local, &shared, &unrelated, &style, &canvas]
            .map(|filter| Some(filter.node()))
            .to_vec();
        branches.extend([Some(solid), Some(graph_ramp.node())]);
        let mut doc = Gtx375GraphDocument::new(branches).unwrap();
        let property = shared.parameter(0).unwrap();
        doc.set_environment_adjuster(&property, EnvironmentAdjusterLink::Create("Sky".into()))
            .unwrap();
        let control = doc.environment_adjuster(&property).unwrap();
        for pass in 0..2 {
            link_environment_paint(
                &mut doc,
                &canvas,
                pass,
                EnvironmentAdjusterLink::Existing(control.clone()),
            );
        }
        for (text, value) in [("Fill", [0.75, 0.125, 0.25]), ("Tint", [0.5, 1., 0.875])] {
            let mut entry = ValueAdjusterRecord::new(ValueAdjusterKind::Color, name(text), [0; 16]);
            entry.payload = ValueAdjusterPayload::Color {
                value,
                previous: value,
            };
            doc.catalog
                .stage(GtxCatalogRecord::Value(Box::new(entry)))
                .store(true, std::sync::atomic::Ordering::Release);
        }
        let mut gradient =
            ReferenceAdjusterRecord::new(ReferenceAdjusterKind::Gradient, name("Ramp"), [0; 16]);
        let ReferenceValueHandle::Gradient(sample) = gradient.value() else {
            unreachable!()
        };
        ramp(&sample.live());
        doc.catalog
            .stage(GtxCatalogRecord::Reference(Box::new(gradient)))
            .store(true, std::sync::atomic::Ordering::Release);
        let mut original = BinaryWriter::default();
        doc.save_compatible_gtx(&mut original).unwrap();
        let mut doc = Gtx375GraphDocument::<()>::load(original.bytes()).unwrap();

        let check = |doc: &Gtx375GraphDocument<()>, red, tint, ramp: &SharedGradient| {
            let filters =
                [0, 1, 2, 3, 4].map(|index| doc.branch(index).flatten().unwrap().filter().unwrap());
            let properties =
                [&filters[0], &filters[1], &filters[2]].map(|filter| filter.parameter(0).unwrap());
            let style = environment_style_edit(&filters[3]);
            let canvas = environment_style_edit(&filters[4]);
            let shared = doc.environment_adjuster(&properties[1]).unwrap().value();
            let mapped = properties
                .each_ref()
                .map(|property| property.effective_environment().unwrap());
            assert!(Arc::ptr_eq(&shared, &mapped[1]));
            for pass in 0..2 {
                assert!(Arc::ptr_eq(&shared, canvas.environment_map(pass).unwrap()));
            }
            for map in [&mapped[0], &mapped[1], style.environment_map(0).unwrap()] {
                assert_eq!(map.field(EnvironmentField::Red(0)), Some(red));
                assert_eq!(map.field(EnvironmentField::Tint(0, 0)), Some(tint));
                let gradient = map.copy_recolor_gradient(0).unwrap().unwrap();
                assert!(gradient.source_equals(Some(ramp)).unwrap());
            }
            assert_eq!(mapped[2].field(EnvironmentField::Red(0)), Some(0.25));
            let solid = doc.branch(5).flatten().unwrap().solid_model().unwrap();
            assert_eq!(
                solid.color_property().color_value().unwrap().components()[0],
                red
            );
            let graph = doc.branch(6).flatten().unwrap().filter().unwrap();
            assert!(
                ramp_parameter(&graph)
                    .gradient()
                    .unwrap()
                    .source_equals(Some(ramp))
                    .unwrap()
            );
        };
        let render = |doc: &Gtx375GraphDocument<()>| {
            let node = doc.branch(0).flatten().unwrap();
            let preview = node.render_original(32, 32).unwrap();
            let mut capacity = |_| Ok(true);
            let mut preflight = |_, _| Ok(());
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            let export = RenderCaptureSession::new()
                .capture_node(&node, 0.)
                .unwrap()
                .render_export_with_effects(
                    ImageGeometry::new(32, 32),
                    ProceduralExecution::CallingThread,
                    PointwiseRenderOptions::new(false, false),
                    &mut effects,
                )
                .unwrap();
            assert!(preview.caught_errors().is_empty());
            assert!(export.caught_errors().is_empty());
            let channels = |image: &crate::SharedImageMap| {
                [0, 1, 2, 3].map(|channel| image.channel(channel, true).unwrap().values())
            };
            let preview = channels(&preview.image().unwrap());
            let export = channels(&export.image().unwrap());
            assert_eq!(preview, export);
            preview
        };
        check(&doc, 0.75, 0.5, &sample.live());
        let pixels = render(&doc);
        for (channel, expected) in pixels.iter().zip([0.375, 0.375, 0.1640625, 0.25]) {
            assert!((channel[0] - expected).abs() < 1e-6);
        }
        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        let native = Gtx375GraphDocument::load_project(
            native.bytes(),
            crate::NodeWireReadPolicy::new(128, 4096),
        )
        .unwrap();
        let mut gtx = BinaryWriter::default();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        let gtx = Gtx375GraphDocument::load(gtx.bytes()).unwrap();
        for reopened in [&native, &gtx] {
            check(reopened, 0.75, 0.5, &sample.live());
            assert_eq!(render(reopened), pixels);
        }

        doc.edit_lab_preset(None, LabPresetOperation::Save("Morning".into()))
            .unwrap();
        let garden = crate::SharedGradientRecord::from_live(SharedGradient::new());
        for position in [0., 1.] {
            garden
                .live()
                .add_normalized_color(position, 0.25, 0.125, 0.25)
                .unwrap();
            garden.live().add_normalized_alpha(position, 0.5).unwrap();
        }
        let mut incoming = (*doc
            .environment_adjuster(
                &doc.branch(1)
                    .flatten()
                    .unwrap()
                    .filter()
                    .unwrap()
                    .parameter(0)
                    .unwrap(),
            )
            .unwrap()
            .value())
        .clone();
        incoming.set_exposure(1.);
        let preset = GtxPresetGroup::lab_snapshot(
            "Evening".encode_utf16().collect(),
            [0; 16],
            vec![
                GtxPresetRecord::new(GtxPresetKind::Environment, name("Sky"), [0; 16])
                    .with_value(GtxPresetValue::Environment(Box::new(incoming))),
                GtxPresetRecord::new(GtxPresetKind::Color, name("Fill"), [0; 16])
                    .with_value(GtxPresetValue::Color([0.125, 0.25, 0.5])),
                GtxPresetRecord::new(GtxPresetKind::Color, name("Tint"), [0; 16])
                    .with_value(GtxPresetValue::Color([0.875, 1., 0.5])),
                GtxPresetRecord::new(GtxPresetKind::Gradient, name("Ramp"), [0; 16]).with_value(
                    GtxPresetValue::Reference(ReferenceValueHandle::Gradient(garden.clone())),
                ),
            ],
        );
        preset
            .presence()
            .store(true, std::sync::atomic::Ordering::Release);
        doc.presets_mut().stage_lab_snapshot(preset, None);
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let find = |doc: &Gtx375GraphDocument<()>, name| {
            doc.lab_presets(None)
                .presets
                .into_iter()
                .find(|preset| preset.name() == name)
                .unwrap()
        };
        doc.edit_lab_preset(None, LabPresetOperation::Apply(find(&doc, "Evening")))
            .unwrap();
        assert_eq!(history.cursor(), 1);
        check(&doc, 0.125, 0.875, &garden.live());
        let evening = render(&doc);
        assert!(evening != pixels);
        history.undo().unwrap();
        check(&doc, 0.75, 0.5, &sample.live());
        assert_eq!(render(&doc), pixels);
        history.redo().unwrap();
        assert_eq!(render(&doc), evening);
        let mut gtx = BinaryWriter::default();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        let reopened = Gtx375GraphDocument::load(gtx.bytes()).unwrap();
        check(&reopened, 0.125, 0.875, &garden.live());
        assert_eq!(render(&reopened), evening);

        doc.edit_lab_preset(None, LabPresetOperation::Apply(find(&doc, "Morning")))
            .unwrap();
        check(&doc, 0.75, 0.5, &sample.live());
        assert_eq!(render(&doc), pixels);

        let edit_control = |doc: &Gtx375GraphDocument<()>, name| {
            doc.color_lab_adjusters(None)
                .into_iter()
                .find(|control| control.name() == name)
                .unwrap()
        };
        let fill = edit_control(&doc, "Fill");
        let before = fill.draft_value().unwrap();
        let cursor = history.cursor();
        assert!(
            !doc.edit_color_lab_adjuster(None, &fill, &before, before.clone())
                .unwrap()
        );
        assert_eq!(history.cursor(), cursor);
        assert!(
            doc.edit_color_lab_adjuster(None, &fill, &before, LabColorValue::Color([0.375; 3]),)
                .unwrap()
        );
        assert!(
            doc.edit_color_lab_adjuster(None, &fill, &before, LabColorValue::Color([0.25; 3]),)
                .is_err()
        );
        check(&doc, 0.375, 0.5, &sample.live());

        let ramp = edit_control(&doc, "Ramp");
        assert!(
            doc.edit_color_lab_adjuster(
                None,
                &ramp,
                &ramp.draft_value().unwrap(),
                LabColorValue::Gradient(garden.live()),
            )
            .unwrap()
        );
        assert_eq!(history.cursor(), cursor + 2);
        check(&doc, 0.375, 0.5, &garden.live());
        let changed = render(&doc);
        history.undo().unwrap();
        check(&doc, 0.375, 0.5, &sample.live());
        history.undo().unwrap();
        assert_eq!(render(&doc), pixels);
        history.redo().unwrap();
        history.redo().unwrap();
        assert_eq!(render(&doc), changed);
        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        let native = Gtx375GraphDocument::load_project(
            native.bytes(),
            crate::NodeWireReadPolicy::new(128, 4096),
        )
        .unwrap();
        assert_eq!(render(&native), changed);
    }

    #[test]
    fn author_number_and_selection_links_on_shared_environment() {
        use crate::{
            EnvironmentNumericReceiver as Receiver, EnvironmentTransformFilter as Filter,
            LabNumericProperty,
        };

        let make = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        let property = make.parameter(0).unwrap();
        let mut map = red_map(0.5);
        map.set_exposure(1.);
        map.set_transform_filter(None, Filter::Diffuse).unwrap();
        property.set_environment(Arc::new(map)).unwrap();
        let style = environment_paint_node(PointwiseKind::Style, 0.75);
        let solid = PointwiseNode::solid_color().unwrap();
        let transparency = LabNumericProperty::transparency(&solid).unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(make.node()), Some(style.node()), Some(solid)])
                .unwrap();
        doc.set_environment_adjuster(&property, EnvironmentAdjusterLink::Create("Sky".into()))
            .unwrap();
        let sky = doc.environment_adjuster(&property).unwrap();
        link_environment_paint(
            &mut doc,
            &style,
            0,
            EnvironmentAdjusterLink::Existing(sky.clone()),
        );
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let exposure = Receiver::Field(EnvironmentField::Exposure);
        let filter = Receiver::Filter(None);
        let owner = || EnvironmentMapTarget::Property(&property);

        let stale = sky.value();
        assert!(
            doc.set_environment_numeric_adjuster(
                owner(),
                &stale,
                exposure,
                LabNumericLink::Create("Exposure".into())
            )
            .unwrap()
        );
        assert_eq!(sky.value().field(EnvironmentField::Exposure), Some(1.));
        assert!(
            doc.set_environment_numeric_adjuster(
                owner(),
                &stale,
                filter,
                LabNumericLink::Create("Filter".into())
            )
            .is_err()
        );
        let exposure_control = doc
            .environment_numeric_adjuster(&sky.value(), exposure)
            .unwrap();
        assert_eq!(exposure_control.value(), LabNumericValue::Number(75.));
        assert!(
            doc.set_numeric_property_link(
                &transparency,
                LabNumericAxis::Whole,
                LabNumericLink::Existing(exposure_control)
            )
            .unwrap()
        );
        assert!(
            doc.set_numeric_property_link(
                &transparency,
                LabNumericAxis::Whole,
                LabNumericLink::Unlink
            )
            .unwrap()
        );
        let exposure_control = doc
            .environment_numeric_adjuster(&sky.value(), exposure)
            .unwrap();
        assert!(
            doc.edit_numeric_lab_adjuster(None, &exposure_control, LabNumericValue::Number(0.))
                .unwrap()
        );
        assert_eq!(sky.value().field(EnvironmentField::Exposure), Some(-2.));

        assert!(
            doc.set_environment_numeric_adjuster(
                owner(),
                &sky.value(),
                filter,
                LabNumericLink::Create("Filter".into())
            )
            .unwrap()
        );
        assert_eq!(sky.value().transform_filter(None), Some(Filter::Diffuse));
        let selection = doc
            .environment_numeric_adjuster(&sky.value(), filter)
            .unwrap();
        assert_eq!(
            selection.options(),
            &["None", "Diffuse", "Scatter", "Anisotropic"]
        );
        assert!(
            doc.edit_numeric_lab_adjuster(None, &selection, LabNumericValue::Selection(2))
                .unwrap()
        );
        assert_eq!(sky.value().transform_filter(None), Some(Filter::Scatter));
        let style_node = style.node();
        let style_owner = || EnvironmentMapTarget::Paint {
            owner: &style_node,
            layer: None,
            pass: 0,
        };
        let mapping = doc
            .environment_numeric_mapping(style_owner(), &sky.value(), exposure)
            .unwrap();
        assert_eq!(mapping.bounds(), [-2., 2.]);
        assert!(!doc.edit_environment_numeric_mapping(&mapping).unwrap());
        mapping.curve().set_minimum(-3.);
        assert!(doc.edit_environment_numeric_mapping(&mapping).is_err());
        mapping.curve().set_minimum(-1.);
        assert_eq!(sky.value().field(EnvironmentField::Exposure), Some(-2.));
        assert!(doc.edit_environment_numeric_mapping(&mapping).unwrap());
        assert_eq!(sky.value().field(EnvironmentField::Exposure), Some(-1.));
        assert!(doc.edit_environment_numeric_mapping(&mapping).is_err());
        history.undo().unwrap();
        assert_eq!(sky.value().field(EnvironmentField::Exposure), Some(-2.));
        history.redo().unwrap();
        assert_eq!(sky.value().field(EnvironmentField::Exposure), Some(-1.));
        let check = |doc: &Gtx375GraphDocument<()>| {
            let make = doc.branch(0).flatten().unwrap().filter().unwrap();
            let style = doc.branch(1).flatten().unwrap().filter().unwrap();
            let map = make.parameter(0).unwrap().effective_environment().unwrap();
            assert!(Arc::ptr_eq(
                &map,
                style.style_edit().unwrap().environment_map(0).unwrap()
            ));
            assert_eq!(map.field(EnvironmentField::Exposure), Some(-1.));
            assert_eq!(map.transform_filter(None), Some(Filter::Scatter));
            assert_eq!(
                doc.environment_numeric_adjuster(&map, filter)
                    .unwrap()
                    .value(),
                LabNumericValue::Selection(2)
            );
        };
        check(&doc);
        assert!(
            doc.set_environment_numeric_adjuster(
                owner(),
                &sky.value(),
                exposure,
                LabNumericLink::Unlink
            )
            .unwrap()
        );
        assert_eq!(doc.numeric_lab_adjusters(None).len(), 1);
        assert_eq!(sky.value().field(EnvironmentField::Exposure), Some(-1.));
        history.undo().unwrap();
        assert_eq!(doc.numeric_lab_adjusters(None).len(), 2);
        check(&doc);
        let mut gtx = BinaryWriter::default();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        check(&Gtx375GraphDocument::load(gtx.bytes()).unwrap());
        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        check(
            &Gtx375GraphDocument::load_project(
                native.bytes(),
                crate::NodeWireReadPolicy::new(128, 4096),
            )
            .unwrap(),
        );
    }

    #[test]
    fn map_selection_in_one_local_canvas_environment_paint() {
        use crate::{EnvironmentNumericReceiver, EnvironmentTransformFilter as Filter};

        let canvas = environment_paint_node(PointwiseKind::Canvas, 0.5);
        let node = canvas.node();
        let layer = canvas.canvas_layer_editors().remove(0);
        let owner = || EnvironmentMapTarget::Paint {
            owner: &node,
            layer: Some(&layer),
            pass: 1,
        };
        let map = |pass| {
            layer
                .style_edit()
                .unwrap()
                .environment_map(pass)
                .unwrap()
                .clone()
        };
        let receiver = EnvironmentNumericReceiver::Filter(None);
        let mut doc = Gtx375GraphDocument::new(vec![Some(node.clone())]).unwrap();
        doc.set_environment_numeric_adjuster(
            owner(),
            &map(1),
            receiver,
            LabNumericLink::Create("Filter".into()),
        )
        .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let mapping = doc
            .environment_numeric_mapping(owner(), &map(1), receiver)
            .unwrap();
        assert_eq!(mapping.bounds(), [0.5, 4.5]);
        mapping.curve().set_minimum(3.);
        mapping.curve().set_maximum(3.);
        let untouched = map(0);
        assert!(doc.edit_environment_numeric_mapping(&mapping).unwrap());
        assert!(Arc::ptr_eq(&untouched, &map(0)));
        assert_eq!(map(0).transform_filter(None), Some(Filter::None));
        assert_eq!(map(1).transform_filter(None), Some(Filter::Scatter));
        assert_eq!(
            doc.environment_numeric_adjuster(&map(1), receiver)
                .unwrap()
                .value(),
            LabNumericValue::Selection(0)
        );
        history.undo().unwrap();
        assert_eq!(map(1).transform_filter(None), Some(Filter::None));
        history.redo().unwrap();
        assert_eq!(map(1).transform_filter(None), Some(Filter::Scatter));

        let check = |doc: &Gtx375GraphDocument<()>| {
            let canvas = doc.branch(0).flatten().unwrap().filter().unwrap();
            let style = environment_style_edit(&canvas);
            assert_eq!(
                style.environment_map(0).unwrap().transform_filter(None),
                Some(Filter::None)
            );
            assert_eq!(
                style.environment_map(1).unwrap().transform_filter(None),
                Some(Filter::Scatter)
            );
        };
        let mut gtx = BinaryWriter::default();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        check(&Gtx375GraphDocument::load(gtx.bytes()).unwrap());
        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        check(
            &Gtx375GraphDocument::load_project(
                native.bytes(),
                crate::NodeWireReadPolicy::new(128, 4096),
            )
            .unwrap(),
        );
    }

    #[test]
    fn environment_mapping_reopens_with_both_modulation_forms() {
        use crate::{AnalyticCurveKind, CurveLayerHandle, EnvironmentNumericReceiver};

        let make = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        let property = make.parameter(0).unwrap();
        let map = || property.effective_environment().unwrap();
        let owner = || EnvironmentMapTarget::Property(&property);
        let field = EnvironmentNumericReceiver::Field(EnvironmentField::Exposure);
        let mut doc = Gtx375GraphDocument::new(vec![Some(make.node())]).unwrap();
        doc.set_environment_numeric_adjuster(
            owner(),
            &map(),
            field,
            LabNumericLink::Create("Exposure".into()),
        )
        .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let open = |doc: &Gtx375GraphDocument<()>| {
            doc.environment_numeric_mapping(owner(), &map(), field)
                .unwrap()
        };
        let amplitude = |draft: &EnvironmentNumericMapping<()>| {
            draft.curve().layers()[0]
                .as_ref()
                .unwrap()
                .control(7)
                .unwrap()
                .unwrap()
        };
        let draft = open(&doc);
        let control = amplitude(&draft);
        let child = CurveLayerHandle::new(AnalyticCurveKind::Linear);
        let shift = child.control(8).unwrap().unwrap();
        let nested = CurveLayerHandle::new(AnalyticCurveKind::Arc);
        nested
            .control(12)
            .unwrap()
            .unwrap()
            .set_scalar(72.)
            .unwrap();
        shift.set_child(Some(&nested)).unwrap();
        assert!(!shift.is_active());
        control.set_scalar(37.).unwrap();
        control.set_child(Some(&child)).unwrap();
        control.set_active(true);
        assert!(doc.edit_environment_numeric_mapping(&draft).unwrap());

        let assert_nested = |control: &crate::CurveControlHandle<()>| {
            let child = control.retained_child().unwrap();
            let shift = child.control(8).unwrap().unwrap();
            assert!(!shift.is_active());
            let nested = shift.retained_child().unwrap();
            assert_eq!(nested.kind(), crate::CurveLayerKind::Arc);
            assert_eq!(nested.control(12).unwrap().unwrap().scalar().unwrap(), 72.);
        };
        let draft = open(&doc);
        let control = amplitude(&draft);
        assert!(control.is_active());
        assert_eq!(control.scalar().unwrap(), 37.);
        assert_nested(&control);
        assert!(!doc.edit_environment_numeric_mapping(&draft).unwrap());
        control.set_active(false);
        assert!(doc.edit_environment_numeric_mapping(&draft).unwrap());

        let inspect = |doc: &Gtx375GraphDocument<()>, active| {
            let reopened = open(doc);
            let control = amplitude(&reopened);
            assert_eq!(control.is_active(), active);
            assert_eq!(control.scalar().unwrap(), 37.);
            assert_nested(&control);
        };
        inspect(&doc, false);
        history.undo().unwrap();
        inspect(&doc, true);
        history.redo().unwrap();
        inspect(&doc, false);

        let mut gtx = BinaryWriter::default();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(gtx.bytes()).unwrap();
        let make = reopened.branch(0).flatten().unwrap().filter().unwrap();
        let property = make.parameter(0).unwrap();
        let map = property.effective_environment().unwrap();
        let draft = reopened
            .environment_numeric_mapping(EnvironmentMapTarget::Property(&property), &map, field)
            .unwrap();
        let control = amplitude(&draft);
        assert_eq!(control.scalar().unwrap(), 37.);
        assert!(control.retained_child().is_none());
    }

    #[test]
    fn nested_environment_choices_reconnect_in_catalog_order() {
        use crate::environment_record::EnvironmentNumericReceiver as Receiver;
        use crate::{
            AdjustmentRecord, EnvironmentBlendMode, EnvironmentLight, EnvironmentTransformFilter,
            LabNumericValue, SelectionOptionRecord, ValueAdjusterPayload,
        };
        let link = |name: &str, bounds, property_type| {
            let mut record = AdjustmentRecord::new(&[bounds]);
            record.property_type = property_type;
            let component = &mut record.components_mut()[0];
            component.name = name.encode_utf16().collect();
            component.active = true;
            component.curve_mut();
            record
        };
        let mut map = EnvironmentRecord::default();
        map.add_layer(EnvironmentLayerKind::Light(EnvironmentLight::Hard))
            .unwrap();
        map.install_test_adjustment(Receiver::Light(0), link("Light", [1., 4.], 4));
        map.install_test_adjustment(Receiver::Blend(0), link("Blend", [1., 18.], 4));
        for layer in [None, Some(0)] {
            map.install_test_adjustment(Receiver::Filter(layer), link("Filter", [1., 3.], 4));
        }
        map.install_test_adjustment(
            EnvironmentField::FilterRadius(Some(0)),
            link("Radius", [0., 72.], 0),
        );
        let map = Arc::new(map);
        let open = |radius_first| {
            let node = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
            node.parameter(0)
                .unwrap()
                .set_environment(map.clone())
                .unwrap();
            let mut doc = Gtx375GraphDocument::new(vec![Some(node.node())]).unwrap();
            let mut add = |name: &str, value, kind| {
                let mut entry =
                    ValueAdjusterRecord::new(kind, Some(name.encode_utf16().collect()), [0; 16]);
                let ValueAdjusterPayload::Number(number) = &mut entry.payload else {
                    unreachable!()
                };
                number.value = value;
                number.previous = value;
                if kind == ValueAdjusterKind::Selection {
                    number.selected = 0;
                    number.previous_selected = 0;
                    number.options = ["Hard", "Cloudy"]
                        .into_iter()
                        .zip([0., 100.])
                        .map(|(name, value)| SelectionOptionRecord {
                            name: Some(name.encode_utf16().collect()),
                            value,
                        })
                        .collect();
                }
                doc.catalog
                    .stage(GtxCatalogRecord::Value(Box::new(entry)))
                    .store(true, std::sync::atomic::Ordering::Release);
            };
            let number = ValueAdjusterKind::Number;
            if radius_first {
                add("Radius", 100., number);
            }
            add("Filter", 50., number);
            if !radius_first {
                add("Radius", 100., number);
            }
            add("Light", 50., ValueAdjusterKind::Selection);
            add("Blend", 100., number);
            let mut gtx = BinaryWriter::default();
            doc.save_compatible_gtx(&mut gtx).unwrap();
            Gtx375GraphDocument::load(gtx.bytes()).unwrap()
        };
        let current = |doc: &Gtx375GraphDocument<()>| {
            doc.branch(0)
                .unwrap()
                .unwrap()
                .filter()
                .unwrap()
                .parameter(0)
                .unwrap()
                .effective_environment()
                .unwrap()
        };
        let radius = |doc: &Gtx375GraphDocument<()>| {
            current(doc)
                .field(EnvironmentField::FilterRadius(Some(0)))
                .unwrap()
        };
        assert_eq!(radius(&open(true)), 30.);
        let mut doc = open(false);
        let restored = current(&doc);
        assert_eq!(radius(&doc), 72.);
        assert_eq!(
            restored.transform_filter(None),
            Some(EnvironmentTransformFilter::Diffuse)
        );
        assert_eq!(
            restored.transform_filter(Some(0)),
            Some(EnvironmentTransformFilter::Diffuse)
        );
        assert_eq!(
            restored.blend_mode(0),
            Some(EnvironmentBlendMode::Luminosity)
        );
        // The catalog's saved number (50) maps to 2.5 and rounds to choice 2.
        // The stored option index has a different numeric value (0).
        assert_eq!(
            restored.layer_kind(0),
            Some(EnvironmentLayerKind::Light(EnvironmentLight::Soft))
        );

        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.edit_lab_preset(None, LabPresetOperation::Save("Default".into()))
            .unwrap();
        let edit = |doc: &Gtx375GraphDocument<()>, name, value| {
            let control = doc
                .numeric_lab_adjusters(None)
                .into_iter()
                .find(|control| control.name() == name)
                .unwrap();
            assert!(
                doc.edit_numeric_lab_adjuster(None, &control, value)
                    .unwrap()
            );
        };
        let pixels = |doc: &Gtx375GraphDocument<()>| {
            let outcome = doc
                .branch(0)
                .unwrap()
                .unwrap()
                .render_original(32, 32)
                .unwrap();
            assert!(outcome.caught_errors().is_empty());
            outcome.image().unwrap().channel(0, true).unwrap().values()
        };
        let before = pixels(&doc);
        edit(&doc, "Light", LabNumericValue::Selection(1));
        assert_eq!(
            current(&doc).layer_kind(0),
            Some(EnvironmentLayerKind::Light(EnvironmentLight::CloudySky))
        );
        assert!(pixels(&doc) != before);
        history.undo().unwrap();
        assert_eq!(pixels(&doc), before);
        history.redo().unwrap();
        assert!(pixels(&doc) != before);
        edit(&doc, "Filter", LabNumericValue::Number(100.));
        assert_eq!(
            current(&doc).transform_filter(Some(0)),
            Some(EnvironmentTransformFilter::Scatter)
        );
        assert_eq!(radius(&doc), 30.);
        history.undo().unwrap();
        assert_eq!(radius(&doc), 72.);
        history.redo().unwrap();
        let preset = doc.lab_presets(None).presets.remove(0);
        doc.edit_lab_preset(None, LabPresetOperation::Apply(preset))
            .unwrap();
        assert_eq!(
            current(&doc).transform_filter(Some(0)),
            Some(EnvironmentTransformFilter::Diffuse)
        );
        assert_eq!(radius(&doc), 72.);
        // Preset application uses the saved option, whose numeric value is 0.
        assert_eq!(
            current(&doc).layer_kind(0),
            Some(EnvironmentLayerKind::Light(EnvironmentLight::Hard))
        );
        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        let reopened = Gtx375GraphDocument::load_project(
            native.bytes(),
            crate::NodeWireReadPolicy::new(128, 4096),
        )
        .unwrap();
        assert_eq!(radius(&reopened), 72.);
        assert_eq!(
            current(&reopened).blend_mode(0),
            Some(EnvironmentBlendMode::Luminosity)
        );
        assert_eq!(pixels(&reopened), pixels(&doc));
    }

    #[test]
    fn nested_environment_numbers_reconnect_and_follow_lab_history() {
        use crate::{AdjustmentRecord, LabNumericValue, ValueAdjusterPayload};

        let link = |bounds: [f32; 2], guid, active| {
            let mut record = AdjustmentRecord::new(&[bounds]);
            record.guid = guid;
            let component = &mut record.components_mut()[0];
            component.name = "Light level".encode_utf16().collect();
            component.active = active;
            component.curve_mut();
            record
        };
        let make = |name_scope, active| {
            let filter = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
            let mut map = red_map(0.25);
            map.set_exposure(-1.);
            map.install_test_adjustment(
                EnvironmentField::Exposure,
                link([-2., 2.], name_scope, active),
            );
            map.install_test_adjustment(
                EnvironmentField::Heading(Some(0)),
                link([0., 180.], name_scope, active),
            );
            filter
                .parameter(0)
                .unwrap()
                .set_environment(Arc::new(map))
                .unwrap();
            filter
        };
        let local = make([0; 16], true);
        let shared = make([0; 16], true);
        let inactive = make([0; 16], false);
        let different_scope = make([1; 16], true);
        let mut doc = Gtx375GraphDocument::new(
            [&local, &shared, &inactive, &different_scope]
                .iter()
                .map(|filter| Some(filter.node()))
                .collect(),
        )
        .unwrap();
        doc.set_environment_adjuster(
            &shared.parameter(0).unwrap(),
            EnvironmentAdjusterLink::Create("Shared sky".into()),
        )
        .unwrap();
        let mut entry = ValueAdjusterRecord::new(
            ValueAdjusterKind::Number,
            Some("Light level".encode_utf16().collect()),
            [0; 16],
        );
        let ValueAdjusterPayload::Number(number) = &mut entry.payload else {
            unreachable!()
        };
        number.value = 50.;
        number.previous = 50.;
        doc.catalog
            .stage(GtxCatalogRecord::Value(Box::new(entry)))
            .store(true, std::sync::atomic::Ordering::Release);

        // The stored scalar fields differ deliberately from the saved Lab value.
        let mut original = BinaryWriter::default();
        doc.save_compatible_gtx(&mut original).unwrap();
        let mut doc = Gtx375GraphDocument::<()>::load(original.bytes()).unwrap();
        let branch_property = |doc: &Gtx375GraphDocument<()>, index| {
            doc.branch(index)
                .unwrap()
                .unwrap()
                .filter()
                .unwrap()
                .parameter(0)
                .unwrap()
        };
        let check = |doc: &Gtx375GraphDocument<()>, exposure, heading| {
            for (index, expected) in [
                (0, (exposure, heading)),
                (1, (exposure, heading)),
                (2, (-1., 0.)),
                (3, (-1., 0.)),
            ] {
                let property = branch_property(doc, index);
                let map = property.effective_environment().unwrap();
                assert_eq!(
                    (
                        map.exposure(),
                        map.field(EnvironmentField::Heading(Some(0))).unwrap()
                    ),
                    expected
                );
                if index == 1 {
                    assert!(Arc::ptr_eq(
                        &map,
                        &doc.environment_adjuster(&property).unwrap().value()
                    ));
                }
            }
        };
        check(&doc, 0., 90.);
        let render = |doc: &Gtx375GraphDocument<()>| {
            let node = doc.branch(0).unwrap().unwrap();
            let frame = RenderCaptureSession::new().capture_node(&node, 0.).unwrap();
            let geometry = ImageGeometry::new(32, 32);
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
            rendered
                .image()
                .unwrap()
                .channel(0, true)
                .unwrap()
                .read(0)
                .unwrap()
        };
        let baseline = render(&doc);
        assert_eq!(baseline, 0.25);
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.edit_lab_preset(None, LabPresetOperation::Save("Midday".into()))
            .unwrap();
        let control = doc.numeric_lab_adjusters(None).remove(0);
        assert!(
            doc.edit_numeric_lab_adjuster(None, &control, LabNumericValue::Number(75.))
                .unwrap()
        );
        check(&doc, 1., 135.);
        assert_eq!(render(&doc), 0.5);
        history.undo().unwrap();
        check(&doc, 0., 90.);
        assert_eq!(render(&doc), baseline);
        history.redo().unwrap();
        check(&doc, 1., 135.);

        let preset = doc.lab_presets(None).presets.remove(0);
        doc.edit_lab_preset(None, LabPresetOperation::Apply(preset))
            .unwrap();
        check(&doc, 0., 90.);
        history.undo().unwrap();
        check(&doc, 1., 135.);
        history.redo().unwrap();
        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load_project(
            native.bytes(),
            crate::NodeWireReadPolicy::new(128, 4096),
        )
        .unwrap();
        check(&reopened, 0., 90.);
        assert_eq!(render(&reopened), baseline);
    }

    #[test]
    fn environment_paints_follow_saved_lab_control_and_history() {
        use crate::{
            AdjustmentRecord, CanvasPrimitive, LabNumericValue, StylePaintKind, StyleSection,
            ValueAdjusterPayload,
        };

        let style = PointwiseFilter::<()>::new(PointwiseKind::Style).unwrap();
        let canvas = PointwiseFilter::new(PointwiseKind::Canvas).unwrap();
        canvas
            .edit_add_canvas_shape(CanvasPrimitive::Rectangle, None)
            .unwrap();
        let layer = &canvas.canvas_layer_editors()[0];
        for (is_canvas, mut edit) in [
            (false, style.style_edit().unwrap()),
            (true, layer.style_edit().unwrap()),
        ] {
            if !is_canvas {
                let mut area: Vec<_> = edit
                    .controls(0, StyleSection::Area)
                    .iter()
                    .map(|control| control.value())
                    .collect();
                area[0] = 0.;
                edit.set_controls(0, StyleSection::Area, &area).unwrap();
            }
            edit.set_paint(0, StylePaintKind::Environment).unwrap();
            edit.duplicate(0).unwrap();
            for pass in 0..2 {
                let mut map = red_map(0.25);
                map.set_exposure(-1.);
                let mut link = AdjustmentRecord::new(&[[-2., 2.]]);
                if pass == 1 && !is_canvas {
                    link.guid = [1; 16];
                }
                let component = &mut link.components_mut()[0];
                component.name = "Light level".encode_utf16().collect();
                component.active = true;
                component.curve_mut();
                map.install_test_adjustment(EnvironmentField::Exposure, link);
                let expected = edit.environment_map(pass).unwrap().clone();
                edit.replace_environment_map(pass, &expected, map).unwrap();
            }
            if is_canvas {
                layer.edit_style(edit, "Set Environment paints").unwrap();
            } else {
                style.edit_style(edit, "Set Environment paints").unwrap();
            }
        }
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(style.node()), Some(canvas.node())]).unwrap();
        let mut entry = ValueAdjusterRecord::new(
            ValueAdjusterKind::Number,
            Some("Light level".encode_utf16().collect()),
            [0; 16],
        );
        let ValueAdjusterPayload::Number(number) = &mut entry.payload else {
            unreachable!()
        };
        number.value = 50.;
        number.previous = 50.;
        doc.catalog
            .stage(GtxCatalogRecord::Value(Box::new(entry)))
            .store(true, std::sync::atomic::Ordering::Release);
        let mut gtx = BinaryWriter::default();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        let mut doc = Gtx375GraphDocument::<()>::load(gtx.bytes()).unwrap();

        let check = |doc: &Gtx375GraphDocument<()>, value| {
            for (branch, exposures) in [[value, -1.], [value, value]].into_iter().enumerate() {
                let filter = doc.branch(branch).unwrap().unwrap().filter().unwrap();
                let edit = if branch == 0 {
                    filter.style_edit()
                } else {
                    filter.canvas_layer_editors()[0].style_edit()
                }
                .unwrap();
                assert_eq!(edit.count(), 2);
                for (pass, exposure) in exposures.into_iter().enumerate() {
                    assert_eq!(edit.environment_map(pass).unwrap().exposure(), exposure);
                }
            }
        };
        let pixels = |doc: &Gtx375GraphDocument<()>| {
            [0, 1].map(|branch| {
                let node = doc.branch(branch).unwrap().unwrap();
                let preview = node.render_original(32, 32).unwrap();
                let frame = RenderCaptureSession::new().capture_node(&node, 0.).unwrap();
                let mut capacity = |_| Ok(true);
                let mut preflight = |_, _| Ok(());
                let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
                let export = frame
                    .render_export_with_effects(
                        ImageGeometry::new(32, 32),
                        ProceduralExecution::CallingThread,
                        PointwiseRenderOptions::new(false, false),
                        &mut effects,
                    )
                    .unwrap();
                assert!(preview.caught_errors().is_empty());
                assert!(export.caught_errors().is_empty());
                let preview = preview.image().unwrap().channel(0, true).unwrap().values();
                let export = export.image().unwrap().channel(0, true).unwrap().values();
                assert_eq!(preview, export);
                preview
            })
        };

        check(&doc, 0.);
        let before = pixels(&doc);
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.edit_lab_preset(None, LabPresetOperation::Save("Default".into()))
            .unwrap();
        let control = doc.numeric_lab_adjusters(None).remove(0);
        assert!(
            doc.edit_numeric_lab_adjuster(None, &control, LabNumericValue::Number(75.))
                .unwrap()
        );
        check(&doc, 1.);
        let after = pixels(&doc);
        for (branch, (before, after)) in before.iter().zip(&after).enumerate() {
            assert!(before != after, "exposure must affect branch {branch}");
        }
        history.undo().unwrap();
        check(&doc, 0.);
        assert_eq!(pixels(&doc), before);
        history.redo().unwrap();
        assert_eq!(pixels(&doc), after);

        let preset = doc.lab_presets(None).presets.remove(0);
        doc.edit_lab_preset(None, LabPresetOperation::Apply(preset))
            .unwrap();
        check(&doc, 0.);
        assert_eq!(pixels(&doc), before);
        history.undo().unwrap();
        check(&doc, 1.);

        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        let reopened = Gtx375GraphDocument::load_project(
            native.bytes(),
            crate::NodeWireReadPolicy::new(128, 4096),
        )
        .unwrap();
        check(&reopened, 1.);
        assert_eq!(pixels(&reopened), after);
    }

    #[test]
    fn saved_shared_environment_maps_drive_style_canvas_and_graph_properties() {
        use crate::{
            AdjustmentRecord, CanvasPrimitive, RenderFreezeSession, StylePaintKind, StyleSection,
        };

        let property = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        let parameter = property.parameter(0).unwrap();
        parameter.set_environment(Arc::new(red_map(0.25))).unwrap();
        let style = PointwiseFilter::new(PointwiseKind::Style).unwrap();
        let canvas = PointwiseFilter::new(PointwiseKind::Canvas).unwrap();
        canvas
            .edit_add_canvas_shape(CanvasPrimitive::Rectangle, None)
            .unwrap();
        let canvas_layer = &canvas.canvas_layer_editors()[0];
        for (is_canvas, mut edit) in [
            (false, style.style_edit().unwrap()),
            (true, canvas_layer.style_edit().unwrap()),
        ] {
            if !is_canvas {
                let mut area: Vec<_> = edit
                    .controls(0, StyleSection::Area)
                    .iter()
                    .map(|control| control.value())
                    .collect();
                area[0] = 0.;
                edit.set_controls(0, StyleSection::Area, &area).unwrap();
            }
            edit.set_paint(0, StylePaintKind::Environment).unwrap();
            let mut adjustment = AdjustmentRecord::new(&[[0., 0.]]);
            adjustment.property_type = 9;
            adjustment.components_mut()[0].name = "Shared sky".encode_utf16().collect();
            adjustment.components_mut()[0].active = true;
            edit.install_test_environment_adjustment(0, adjustment);
            if is_canvas {
                canvas_layer
                    .edit_style(edit, "Link Environment paint")
                    .unwrap();
            } else {
                style.edit_style(edit, "Link Environment paint").unwrap();
            }
        }
        let mut source = Gtx375GraphDocument::new(
            [&property, &style, &canvas]
                .map(|node| Some(node.node()))
                .to_vec(),
        )
        .unwrap();
        source
            .set_environment_adjuster(
                &parameter,
                EnvironmentAdjusterLink::Create("Shared sky".into()),
            )
            .unwrap();
        let mut gtx = BinaryWriter::default();
        source.save_compatible_gtx(&mut gtx).unwrap();
        let mut doc = Gtx375GraphDocument::<()>::load(gtx.bytes()).unwrap();
        let filter = |doc: &Gtx375GraphDocument<()>, index| {
            doc.branch(index).unwrap().unwrap().filter().unwrap()
        };
        let check = |doc: &Gtx375GraphDocument<()>, expected| {
            let parameter = filter(doc, 0).parameter(0).unwrap();
            let style = filter(doc, 1).style_edit().unwrap();
            let layer = filter(doc, 2).canvas_layer_editors()[0]
                .style_edit()
                .unwrap();
            let adjuster = doc.style_environment_adjuster(&style, 0).unwrap();
            assert!(
                doc.style_environment_adjuster(&layer, 0)
                    .unwrap()
                    .same_reference(&adjuster)
            );
            let shared = adjuster.value();
            for current in [
                &parameter.effective_environment().unwrap(),
                style.environment_map(0).unwrap(),
                layer.environment_map(0).unwrap(),
            ] {
                assert!(Arc::ptr_eq(current, &shared));
            }
            assert_eq!(shared.field(EnvironmentField::Red(0)), Some(expected));
        };
        check(&doc, 0.25);
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.edit_lab_preset(None, LabPresetOperation::Save("Original sky".into()))
            .unwrap();
        let adjuster = doc
            .style_environment_adjuster(&filter(&doc, 1).style_edit().unwrap(), 0)
            .unwrap();
        let before = adjuster.value();
        let working = Arc::new(red_map(0.75));
        let render = |mut session: RenderFreezeSession<()>| {
            let pending = [0, 1, 2].map(|index| {
                session
                    .freeze_node(&filter(&doc, index).node(), 0.)
                    .unwrap()
            });
            let mut batch = session.finish().unwrap().materialize().unwrap();
            pending.map(|frame| {
                let mut capacity = |_| Ok(true);
                let mut preflight = |_, _| Ok(());
                let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
                let rendered = frame
                    .complete(&mut batch)
                    .unwrap()
                    .render_export_with_effects(
                        ImageGeometry::new(32, 32),
                        ProceduralExecution::CallingThread,
                        PointwiseRenderOptions::new(false, false),
                        &mut effects,
                    )
                    .unwrap();
                assert!(rendered.caught_errors().is_empty());
                rendered.image().unwrap().channel(0, true).unwrap().values()
            })
        };
        let original = render(RenderFreezeSession::new());
        let edit = Arc::new(doc.preview_environment_adjuster(&adjuster, working.clone()));
        let preview = render(RenderFreezeSession::with_environment_override(edit));
        for index in 0..3 {
            assert!(
                preview[index] != original[index],
                "draft should affect branch {index}"
            );
        }
        check(&doc, 0.25);
        assert!(
            doc.edit_environment_adjuster_value(&adjuster, &before, (*working).clone())
                .unwrap()
        );
        check(&doc, 0.75);
        assert_eq!(render(RenderFreezeSession::new()), preview);
        assert!(
            !doc.edit_environment_adjuster_value(&adjuster, &before, red_map(0.5))
                .unwrap()
        );
        history.undo().unwrap();
        check(&doc, 0.25);
        history.redo().unwrap();
        check(&doc, 0.75);
        let preset = doc.lab_presets(None).presets.remove(0);
        doc.edit_lab_preset(None, LabPresetOperation::Apply(preset))
            .unwrap();
        check(&doc, 0.25);
        history.undo().unwrap();
        check(&doc, 0.75);
        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        let reopened = Gtx375GraphDocument::load_project(
            native.bytes(),
            crate::NodeWireReadPolicy::new(128, 4096),
        )
        .unwrap();
        check(&reopened, 0.75);
        let metadata = doc.lab_adjuster_metadata(None).remove(0);
        assert!(
            doc.edit_lab_adjuster_metadata(None, &metadata, "Evening sky", 0)
                .unwrap()
        );
        check(&doc, 0.75);
        let name = |doc: &Gtx375GraphDocument<()>| {
            let style = filter(doc, 1).style_edit().unwrap();
            String::from_utf16_lossy(doc.style_environment_adjuster(&style, 0).unwrap().name())
        };
        assert_eq!(name(&doc), "Evening sky");
        history.undo().unwrap();
        check(&doc, 0.75);
        assert_eq!(name(&doc), "Shared sky");
        history.redo().unwrap();
        assert_eq!(name(&doc), "Evening sky");
        let parameter = filter(&doc, 0).parameter(0).unwrap();
        doc.set_environment_adjuster(&parameter, EnvironmentAdjusterLink::Unlink)
            .unwrap();
        assert!(doc.environment_adjuster(&parameter).is_none());
        assert!(
            doc.style_environment_adjuster(&filter(&doc, 1).style_edit().unwrap(), 0)
                .is_some()
        );
    }

    #[test]
    fn embedded_synthesis_number_links_share_freeze_and_restore() {
        use crate::{
            CanvasPrimitive, LabNumericProperty, StylePaintKind, SynthesisLayerKind,
            SynthesisNumberField,
        };
        let style = PointwiseFilter::<()>::new(PointwiseKind::Style).unwrap();
        let canvas = PointwiseFilter::new(PointwiseKind::Canvas).unwrap();
        canvas
            .edit_add_canvas_shape(CanvasPrimitive::Rectangle, None)
            .unwrap();
        let area = canvas.canvas_layer_editors().remove(0);
        let property_owner = PointwiseFilter::new(PointwiseKind::Distort).unwrap();
        let property = LabNumericProperty::filter(&property_owner.parameter(1).unwrap()).unwrap();
        let paint = |mut edit: crate::StyleEdit| {
            edit.set_paint(0, StylePaintKind::Synthesis).unwrap();
            let mut synthesis = edit.synthesis_edit(0).unwrap();
            synthesis.add_layer(SynthesisLayerKind::Patches);
            assert!(edit.set_synthesis(0, synthesis).unwrap());
            edit
        };
        style
            .edit_style(paint(style.style_edit().unwrap()), "Texture Synthesis")
            .unwrap();
        area.edit_style(paint(area.style_edit().unwrap()), "Texture Synthesis")
            .unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![
            Some(style.node()),
            Some(canvas.node()),
            Some(property_owner.node()),
        ])
        .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let draft = || style.style_edit().unwrap().synthesis_edit(0).unwrap();
        let canvas_draft = || area.style_edit().unwrap().synthesis_edit(0).unwrap();
        let angle = SynthesisNumberField::AngleJitter;
        let seed = SynthesisNumberField::Seed;
        let style_target = StyleSynthesisNumberTarget::new(0, Some(0), angle);
        let canvas_target = StyleSynthesisNumberTarget::new(0, None, seed);

        let original = draft();
        let authored = original.number_value(Some(0), angle).unwrap();
        assert!(
            doc.set_style_synthesis_number_adjuster(
                &style.node(),
                None,
                &original,
                style_target,
                LabNumericLink::Create("Placement".into()),
            )
            .unwrap()
        );
        assert_eq!(draft().number_value(Some(0), angle), Some(authored));
        assert!(
            doc.set_style_synthesis_number_adjuster(
                &style.node(),
                None,
                &original,
                style_target,
                LabNumericLink::Unlink,
            )
            .is_err()
        );
        let control = doc
            .available_style_synthesis_number_adjusters(&canvas.node(), &canvas_draft(), None, seed)
            .remove(0);
        assert_eq!(control.name(), "Placement");
        assert!(
            doc.set_style_synthesis_number_adjuster(
                &canvas.node(),
                Some(&area),
                &canvas_draft(),
                canvas_target,
                LabNumericLink::Existing(control.clone()),
            )
            .unwrap()
        );
        assert!(
            doc.set_numeric_property_link(
                &property,
                LabNumericAxis::Whole,
                LabNumericLink::Existing(control.clone())
            )
            .unwrap()
        );
        assert!(
            doc.set_numeric_property_link(&property, LabNumericAxis::Whole, LabNumericLink::Unlink)
                .unwrap()
        );
        assert_eq!(doc.numeric_lab_adjusters(None).len(), 1);
        doc.edit_numeric_lab_adjuster(None, &control, LabNumericValue::Number(75.))
            .unwrap();
        assert_eq!(draft().number_value(Some(0), angle), Some(75.));
        assert_eq!(canvas_draft().number_value(None, seed), Some(750.));

        let mapping = doc
            .style_synthesis_number_mapping(&style.node(), None, &draft(), style_target)
            .unwrap();
        assert_eq!(mapping.bounds(), [0., 100.]);
        mapping.curve().set_maximum(80.);
        assert!(doc.edit_style_synthesis_number_mapping(&mapping).unwrap());
        assert_eq!(draft().number_value(Some(0), angle), Some(60.));
        assert_eq!(canvas_draft().number_value(None, seed), Some(750.));
        history.undo().unwrap();
        assert_eq!(draft().number_value(Some(0), angle), Some(75.));

        assert!(
            doc.set_style_synthesis_automatic_placement(&style.node(), None, &draft(), 0, 0, false)
                .unwrap()
        );
        assert_eq!(draft().automatic_placement(0), Some(false));
        assert_eq!(draft().number_value(Some(0), angle), Some(75.));
        assert!(draft().number_adjustment(Some(0), angle).is_none());
        assert_eq!(doc.numeric_lab_adjusters(None).len(), 1);
        assert!(
            doc.set_style_synthesis_number_adjuster(
                &style.node(),
                None,
                &draft(),
                style_target,
                LabNumericLink::Existing(control),
            )
            .is_err()
        );
        history.undo().unwrap();
        assert_eq!(draft().automatic_placement(0), Some(true));
        assert!(draft().number_adjustment(Some(0), angle).is_some());

        let check_saved = |doc: Gtx375GraphDocument<()>| {
            let style = doc.branch(0).flatten().unwrap().filter().unwrap();
            let canvas = doc.branch(1).flatten().unwrap().filter().unwrap();
            let style = style.style_edit().unwrap().synthesis_edit(0).unwrap();
            let canvas = canvas.canvas_layer_editors()[0]
                .style_edit()
                .unwrap()
                .synthesis_edit(0)
                .unwrap();
            assert_eq!(style.number_value(Some(0), angle), Some(75.));
            assert_eq!(canvas.number_value(None, seed), Some(750.));
            assert_eq!(doc.numeric_lab_adjusters(None)[0].name(), "Placement");
        };
        let mut native = crate::BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        check_saved(
            Gtx375GraphDocument::load_project(
                native.bytes(),
                crate::NodeWireReadPolicy::new(128, 4096),
            )
            .unwrap(),
        );
        let mut gtx = crate::BinaryWriter::default();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        check_saved(Gtx375GraphDocument::load(gtx.bytes()).unwrap());

        assert!(
            doc.set_style_synthesis_number_adjuster(
                &style.node(),
                None,
                &draft(),
                style_target,
                LabNumericLink::Unlink,
            )
            .unwrap()
        );
        assert_eq!(draft().number_value(Some(0), angle), Some(75.));
        assert!(draft().number_adjustment(Some(0), angle).is_none());
        assert_eq!(doc.numeric_lab_adjusters(None).len(), 1);
        assert!(
            doc.set_style_synthesis_number_adjuster(
                &canvas.node(),
                Some(&area),
                &canvas_draft(),
                canvas_target,
                LabNumericLink::Unlink,
            )
            .unwrap()
        );
        assert_eq!(canvas_draft().number_value(None, seed), Some(750.));
        assert!(canvas_draft().number_adjustment(None, seed).is_none());
        assert!(doc.numeric_lab_adjusters(None).is_empty());
        history.undo().unwrap();
        assert_eq!(
            doc.numeric_lab_adjusters(None)[0].value(),
            LabNumericValue::Number(75.)
        );
        assert!(canvas_draft().number_adjustment(None, seed).is_some());
        history.redo().unwrap();
        assert!(doc.numeric_lab_adjusters(None).is_empty());
        assert_eq!(canvas_draft().number_value(None, seed), Some(750.));
    }

    #[test]
    fn numeric_links_initialize_share_and_switch_between_pair_and_axis_history() {
        use crate::{LabNumericAxis as Axis, LabNumericLink as Link, LabNumericProperty};
        let first = PointwiseFilter::<()>::new(PointwiseKind::Distort).unwrap();
        let second = PointwiseFilter::new(PointwiseKind::Distort).unwrap();
        first.parameter(1).unwrap().set_number(8.).unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(first.node()), Some(second.node())]).unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let steps = LabNumericProperty::filter(&first.parameter(1).unwrap()).unwrap();
        let other = LabNumericProperty::filter(&second.parameter(1).unwrap()).unwrap();
        let pair = LabNumericProperty::filter(&first.parameter(0).unwrap()).unwrap();
        let filter_choice = LabNumericProperty::filter(&second.parameter(2).unwrap()).unwrap();
        doc.set_numeric_property_link(&steps, Axis::Whole, Link::Create("Steps".into()))
            .unwrap();
        assert_eq!(first.parameter(1).unwrap().number(), 8.);
        let shared = doc.numeric_lab_adjusters(None).remove(0);
        doc.set_numeric_property_link(&other, Axis::Whole, Link::Existing(shared.clone()))
            .unwrap();
        assert_eq!(second.parameter(1).unwrap().number(), 8.);
        doc.set_numeric_property_link(&pair, Axis::Whole, Link::Create("Size".into()))
            .unwrap();
        assert_eq!(first.parameter(0).unwrap().pair().unwrap(), [1., 1.]);
        assert_eq!(doc.numeric_lab_adjusters(None).len(), 2);

        doc.set_numeric_property_link(&pair, Axis::X, Link::Existing(shared.clone()))
            .unwrap();
        let pair_now = first.parameter(0).unwrap().pair().unwrap();
        assert_ne!(pair_now[0], 1.);
        assert_eq!(pair_now[1], 1.);
        assert_eq!(
            doc.numeric_property_link_name(&pair, Axis::X).as_deref(),
            Some("Steps")
        );
        assert_eq!(doc.numeric_lab_adjusters(None).len(), 1);
        history.undo().unwrap();
        assert_eq!(first.parameter(0).unwrap().pair().unwrap(), [1., 1.]);
        assert_eq!(
            doc.numeric_property_link_name(&pair, Axis::Whole)
                .as_deref(),
            Some("Size")
        );
        history.redo().unwrap();
        assert_eq!(first.parameter(0).unwrap().pair().unwrap(), pair_now);

        second.parameter(2).unwrap().set_index(1).unwrap();
        doc.set_numeric_property_link(&filter_choice, Axis::Whole, Link::Create("Sampling".into()))
            .unwrap();
        assert_eq!(second.parameter(2).unwrap().index(), 1);
        for (receiver, axis) in [
            (&steps, Axis::Whole),
            (&other, Axis::Whole),
            (&pair, Axis::X),
        ] {
            assert!(
                doc.set_numeric_property_link(receiver, axis, Link::Unlink)
                    .unwrap()
            );
        }
        assert_eq!(
            doc.numeric_lab_adjusters(None)
                .iter()
                .map(|control| control.name())
                .collect::<Vec<_>>(),
            ["Sampling"]
        );
        assert_eq!(first.parameter(0).unwrap().pair().unwrap(), pair_now);
        history.undo().unwrap();
        assert_eq!(
            doc.numeric_property_link_name(&pair, Axis::X).as_deref(),
            Some("Steps")
        );

        let mut saved = BinaryWriter::default();
        doc.save(&mut saved).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(saved.bytes()).unwrap();
        assert_eq!(reopened.numeric_lab_adjusters(None).len(), 2);
        let first = reopened.branch(0).unwrap().unwrap().filter().unwrap();
        assert_eq!(first.parameter(0).unwrap().pair().unwrap(), pair_now);
    }

    #[test]
    fn numeric_links_use_group_contents_and_document_scopes_independently() {
        use crate::{LabNumericAxis::Whole, LabNumericLink as Link, LabNumericProperty};
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        group.parameter(4).unwrap().set_index(0).unwrap();
        let inside = PointwiseFilter::new(PointwiseKind::Distort).unwrap();
        let outside = PointwiseFilter::new(PointwiseKind::Distort).unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&inside.node()))
            .unwrap();
        group
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&outside.node()))
            .unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(group.node())]).unwrap();
        let inside = LabNumericProperty::filter(&inside.parameter(1).unwrap()).unwrap();
        let outside = LabNumericProperty::filter(&outside.parameter(1).unwrap()).unwrap();
        doc.set_numeric_property_link(&inside, Whole, Link::Create("Steps".into()))
            .unwrap();
        let inner = doc.numeric_property_adjusters(&inside, Whole).remove(0);
        assert!(doc.numeric_property_adjusters(&outside, Whole).is_empty());
        assert!(matches!(
            doc.set_numeric_property_link(&outside, Whole, Link::Existing(inner)),
            Err(GtxGraphError::NumericAdjuster(_))
        ));
        doc.set_numeric_property_link(&outside, Whole, Link::Create("Steps".into()))
            .unwrap();
        assert_eq!(doc.numeric_lab_adjusters(Some(&group.node())).len(), 1);
        assert_eq!(doc.numeric_lab_adjusters(None).len(), 1);
        doc.set_numeric_property_link(&inside, Whole, Link::Unlink)
            .unwrap();
        assert!(doc.numeric_lab_adjusters(Some(&group.node())).is_empty());
        assert_eq!(doc.numeric_lab_adjusters(None).len(), 1);
    }

    #[test]
    fn selection_definition_preserves_shared_links_and_retires_stale_handles() {
        use crate::{LabNumericAxis::Whole, LabNumericLink as Link, LabNumericProperty};
        let first = PointwiseFilter::<()>::new(PointwiseKind::Distort).unwrap();
        let second = PointwiseFilter::new(PointwiseKind::Distort).unwrap();
        first.parameter(1).unwrap().set_number(8.).unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(first.node()), Some(second.node())]).unwrap();
        let property = |filter: &PointwiseFilter| {
            LabNumericProperty::filter(&filter.parameter(1).unwrap()).unwrap()
        };
        doc.set_numeric_property_link(&property(&first), Whole, Link::Create("Steps".into()))
            .unwrap();
        let number = doc.numeric_lab_adjusters(None).remove(0);
        doc.set_numeric_property_link(&property(&second), Whole, Link::Existing(number.clone()))
            .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let steps = || {
            [
                first.parameter(1).unwrap().number(),
                second.parameter(1).unwrap().number(),
            ]
        };
        assert_eq!(steps(), [8., 8.]);
        doc.edit_lab_scalar_definition(None, &number, LabScalarDefinition::default_selection())
            .unwrap();
        assert_eq!(steps(), [1., 1.]);
        assert_eq!(
            doc.numeric_lab_adjusters(None)[0].value(),
            LabNumericValue::Selection(0)
        );
        assert!(matches!(
            doc.edit_numeric_lab_adjuster(None, &number, LabNumericValue::Number(25.)),
            Err(GtxGraphError::NumericAdjuster(_))
        ));
        let selection = doc.numeric_lab_adjusters(None).remove(0);
        let mut options = selection.selection_options().to_vec();
        options[0].value = 100.;
        doc.edit_lab_scalar_definition(
            None,
            &selection,
            LabScalarDefinition::Selection {
                options,
                selected: Some(0),
            },
        )
        .unwrap();
        assert_eq!(steps(), [30., 30.]);
        assert_eq!(history.commands().len(), 2);
        history.undo().unwrap();
        assert_eq!(steps(), [1., 1.]);
        history.undo().unwrap();
        assert_eq!(steps(), [8., 8.]);
        history.redo().unwrap();
        history.redo().unwrap();
        assert_eq!(steps(), [30., 30.]);
        assert_eq!(doc.numeric_lab_adjusters(None).len(), 1);
    }

    #[test]
    fn lab_metadata_renames_enabled_numeric_axes_and_restores_both_orders() {
        use crate::{LabNumericAxis as Axis, LabNumericLink as Link, LabNumericProperty};
        let filter = PointwiseFilter::<()>::new(PointwiseKind::Distort).unwrap();
        let pair = LabNumericProperty::filter(&filter.parameter(0).unwrap()).unwrap();
        let steps = LabNumericProperty::filter(&filter.parameter(1).unwrap()).unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(filter.node())]).unwrap();
        doc.set_numeric_property_link(&pair, Axis::X, Link::Create("Drive".into()))
            .unwrap();
        doc.set_numeric_property_link(&pair, Axis::Y, Link::Create("Hold".into()))
            .unwrap();
        let drive = doc.numeric_lab_adjusters(None).remove(0);
        doc.set_numeric_property_link(&steps, Axis::Whole, Link::Existing(drive))
            .unwrap();
        let meta = doc.lab_adjuster_metadata(None).remove(0);
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let original = filter.parameter(0).unwrap().pair().unwrap();
        assert!(
            doc.edit_lab_adjuster_metadata(None, &meta, "  X  drive  ", 8)
                .unwrap()
        );
        assert!(
            doc.edit_lab_adjuster_metadata(None, &meta, "Late", 8)
                .is_err()
        );
        let names = || {
            doc.numeric_lab_adjusters(None)
                .into_iter()
                .map(|control| control.name().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(), ["Hold", "X drive"]);
        assert_eq!(
            doc.numeric_property_link_name(&pair, Axis::X).as_deref(),
            Some("X drive")
        );
        assert_eq!(
            doc.numeric_property_link_name(&pair, Axis::Y).as_deref(),
            Some("Hold")
        );
        assert_eq!(
            doc.numeric_property_link_name(&steps, Axis::Whole)
                .as_deref(),
            Some("X drive")
        );
        assert_eq!(filter.parameter(0).unwrap().pair().unwrap(), original);
        history.undo().unwrap();
        assert_eq!(names(), ["Drive", "Hold"]);
        assert_eq!(
            doc.numeric_property_link_name(&pair, Axis::X).as_deref(),
            Some("Drive")
        );
        history.redo().unwrap();
        assert_eq!(names(), ["Hold", "X drive"]);
        let control = doc.numeric_lab_adjusters(None).remove(1);
        doc.edit_numeric_lab_adjuster(None, &control, LabNumericValue::Number(75.))
            .unwrap();
        let mapped = filter.parameter(0).unwrap().pair().unwrap();
        assert_ne!(mapped[0], original[0]);
        assert_eq!(mapped[1], original[1]);
        let mut wire = BinaryWriter::default();
        doc.save(&mut wire).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(wire.bytes()).unwrap();
        let meta = reopened.lab_adjuster_metadata(None);
        assert_eq!((meta[1].name(), meta[1].order()), ("X drive", 8));
        let filter = reopened.branch(0).unwrap().unwrap().filter().unwrap();
        assert_eq!(filter.parameter(0).unwrap().pair().unwrap(), mapped);
    }

    #[test]
    fn lab_metadata_renames_and_merges_nested_environment_fields() {
        use crate::{
            EnvironmentColorField as Color, EnvironmentNumericReceiver as Numeric,
            EnvironmentTransformFilter as Filter, SharedGradient,
        };
        let primary = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        let donor = PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        let style = environment_paint_node(PointwiseKind::Style, 0.25);
        let canvas = environment_paint_node(PointwiseKind::Canvas, 0.25);
        let solid = PointwiseNode::solid_color().unwrap();
        solid.set_solid_color([0.25, 0., 0.]).unwrap();
        let wire = solid
            .solid_model()
            .unwrap()
            .color_property()
            .adjustment()
            .unwrap()
            .unwrap();
        wire.names()
            .set_entry(0, Some("Fill".encode_utf16().collect()))
            .unwrap();
        wire.flags().set(0, true).unwrap();
        let colorize = PointwiseFilter::new(PointwiseKind::Colorize).unwrap();
        let ramp_property = |node: &PointwiseFilter<()>| {
            node.retained_parameters()
                .into_iter()
                .find(|p| matches!(p.raw_value(), crate::PointwiseParameterValue::Gradient(_)))
                .unwrap()
        };
        let ramp = ramp_property(&colorize);
        let wire = ramp.adjustment().unwrap().unwrap();
        wire.names()
            .set_entry(0, Some("Ramp".encode_utf16().collect()))
            .unwrap();
        wire.flags().set(0, true).unwrap();

        let target = primary.parameter(0).unwrap();
        let donor_target = donor.parameter(0).unwrap();
        let mut map = red_map(0.25);
        map.set_exposure(-2.);
        map.set_recolor_enabled(0, true).unwrap();
        let initial_ramp = map.copy_recolor_gradient(0).unwrap().unwrap();
        ramp.set_gradient(Some(initial_ramp.clone())).unwrap();
        target.set_environment(Arc::new(map)).unwrap();
        let mut map = red_map(0.75);
        map.set_exposure(2.);
        map.set_transform_filter(None, Filter::Scatter).unwrap();
        map.set_recolor_enabled(0, true).unwrap();
        let donor_ramp = SharedGradient::new();
        for position in [0., 1.] {
            donor_ramp
                .add_normalized_color(position, 0.125, 0.5, 0.75)
                .unwrap();
            donor_ramp.add_normalized_alpha(position, 0.5).unwrap();
        }
        map.set_recolor_gradient(0, &donor_ramp).unwrap();
        donor_target.set_environment(Arc::new(map)).unwrap();

        let mut doc = Gtx375GraphDocument::new(vec![
            Some(primary.node()),
            Some(donor.node()),
            Some(style.node()),
            Some(canvas.node()),
            Some(solid),
            Some(colorize.node()),
        ])
        .unwrap();
        doc.set_environment_adjuster(&target, EnvironmentAdjusterLink::Create("Sky".into()))
            .unwrap();
        let sky = doc.environment_adjuster(&target).unwrap();
        for pass in 0..2 {
            link_environment_paint(
                &mut doc,
                &canvas,
                pass,
                EnvironmentAdjusterLink::Existing(sky.clone()),
            );
        }
        let red = Color::LayerColor(0);
        let recolor = Color::Recolor(0);
        let exposure = Numeric::Field(EnvironmentField::Exposure);
        let filter = Numeric::Filter(None);
        let initial_names = ["Fill", "Ramp", "Exposure", "Filter"];
        let renamed = ["Main fill", "Main ramp", "Main exposure", "Main filter"];
        let destination_names = [
            "Palette fill",
            "Palette ramp",
            "Palette exposure",
            "Palette filter",
        ];
        for (property, names) in [(&target, initial_names), (&donor_target, destination_names)] {
            for (field, name) in [(red, names[0]), (recolor, names[1])] {
                doc.set_environment_color_adjuster(
                    EnvironmentMapTarget::Property(property),
                    &property.effective_environment().unwrap(),
                    field,
                    EnvironmentColorLink::Create(name.into()),
                )
                .unwrap();
            }
            for (field, name) in [(exposure, names[2]), (filter, names[3])] {
                doc.set_environment_numeric_adjuster(
                    EnvironmentMapTarget::Property(property),
                    &property.effective_environment().unwrap(),
                    field,
                    LabNumericLink::Create(name.into()),
                )
                .unwrap();
            }
        }
        let style_node = style.node();
        let style_map = || {
            environment_style_edit(&style)
                .environment_map(0)
                .unwrap()
                .clone()
        };
        let style_owner = || EnvironmentMapTarget::Paint {
            owner: &style_node,
            layer: None,
            pass: 0,
        };
        let exposure_control = doc
            .environment_numeric_adjuster(&sky.value(), exposure)
            .unwrap();
        doc.set_environment_numeric_adjuster(
            style_owner(),
            &style_map(),
            exposure,
            LabNumericLink::Existing(exposure_control),
        )
        .unwrap();

        let check = |doc: &Gtx375GraphDocument<()>, names: [&str; 4], merged: bool| {
            let node = |index| doc.branch(index).flatten().unwrap();
            let map = node(0)
                .filter()
                .unwrap()
                .parameter(0)
                .unwrap()
                .effective_environment()
                .unwrap();
            let local = environment_style_edit(&node(2).filter().unwrap())
                .environment_map(0)
                .unwrap()
                .clone();
            let canvas = environment_style_edit(&node(3).filter().unwrap());
            for pass in 0..2 {
                assert!(Arc::ptr_eq(&map, canvas.environment_map(pass).unwrap()));
            }
            let name = |key: Option<([u8; 16], &[u16])>| String::from_utf16_lossy(key.unwrap().1);
            assert_eq!(name(map.color_adjuster_key(red)), names[0]);
            assert_eq!(name(map.color_adjuster_key(recolor)), names[1]);
            assert_eq!(name(map.numeric_adjuster_key(exposure)), names[2]);
            assert_eq!(name(map.numeric_adjuster_key(filter)), names[3]);
            assert_eq!(name(local.numeric_adjuster_key(exposure)), names[2]);
            let expected_red = if merged { 0.75 } else { 0.25 };
            assert_eq!(map.field(EnvironmentField::Red(0)), Some(expected_red));
            assert_eq!(node(4).solid_color_values().unwrap()[0], expected_red);
            let expected_exposure = if merged { 2. } else { -2. };
            assert_eq!(map.exposure(), expected_exposure);
            assert_eq!(local.exposure(), expected_exposure);
            assert_eq!(
                map.transform_filter(None),
                Some(if merged {
                    Filter::Scatter
                } else {
                    Filter::None
                })
            );
            let expected_ramp = if merged { &donor_ramp } else { &initial_ramp };
            assert!(
                map.copy_recolor_gradient(0)
                    .unwrap()
                    .unwrap()
                    .source_equals(Some(expected_ramp))
                    .unwrap()
            );
            assert!(
                ramp_property(&node(5).filter().unwrap())
                    .gradient()
                    .unwrap()
                    .source_equals(Some(expected_ramp))
                    .unwrap()
            );
        };

        check(&doc, initial_names, false);
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let edit = |doc: &mut Gtx375GraphDocument<()>, source: &str, destination: &str| {
            let metadata = doc
                .lab_adjuster_metadata(None)
                .into_iter()
                .find(|entry| entry.name() == source)
                .unwrap();
            assert!(
                doc.edit_lab_adjuster_metadata(None, &metadata, destination, 20)
                    .unwrap()
            );
        };
        for (source, destination) in initial_names.into_iter().zip(renamed) {
            edit(&mut doc, source, destination);
        }
        check(&doc, renamed, false);
        let before_merge = sky.value();
        for (source, destination) in renamed.into_iter().zip(destination_names) {
            edit(&mut doc, source, destination);
        }
        assert_eq!(history.cursor(), 8);
        assert_eq!(doc.catalog().unwrap().len(), 5);
        check(&doc, destination_names, true);
        for _ in 0..4 {
            history.undo().unwrap();
        }
        assert!(Arc::ptr_eq(&before_merge, &sky.value()));
        assert_eq!(doc.catalog().unwrap().len(), 9);
        check(&doc, renamed, false);
        for _ in 0..4 {
            history.redo().unwrap();
        }
        check(&doc, destination_names, true);
        let mut gtx = BinaryWriter::default();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        check(
            &Gtx375GraphDocument::load(gtx.bytes()).unwrap(),
            destination_names,
            true,
        );
        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        check(
            &Gtx375GraphDocument::load_project(
                native.bytes(),
                crate::NodeWireReadPolicy::new(128, 4096),
            )
            .unwrap(),
            destination_names,
            true,
        );
    }

    #[test]
    fn lab_metadata_merges_selections_and_resamples_both_linked_axes() {
        use crate::{
            LabNumericAxis as Axis, LabNumericLink as Link, LabNumericProperty,
            LabScalarDefinition, SelectionOptionRecord,
        };
        let filter = PointwiseFilter::<()>::new(PointwiseKind::Distort).unwrap();
        let pair = filter.parameter(0).unwrap();
        let property = LabNumericProperty::filter(&pair).unwrap();
        let secondary = PointwiseFilter::new(PointwiseKind::Distort).unwrap();
        let secondary_pair = secondary.parameter(0).unwrap();
        let secondary_property = LabNumericProperty::filter(&secondary_pair).unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(filter.node()), Some(secondary.node())]).unwrap();
        let option = |name: &str, value| SelectionOptionRecord {
            name: Some(name.encode_utf16().collect()),
            value,
        };
        let common = option("Middle", 50.);
        let added = option("Middle", 25.);
        let source_options = vec![common.clone(), added.clone(), option("Full", 100.)];
        let destination_options = vec![option("Minimum", 0.), common];
        for (axis, name, options, selected) in [
            (Axis::X, "Source", source_options.clone(), 2),
            (Axis::Y, "Destination", destination_options.clone(), 0),
        ] {
            doc.set_numeric_property_link(&property, axis, Link::Create(name.into()))
                .unwrap();
            let control = doc
                .numeric_lab_adjusters(None)
                .into_iter()
                .find(|control| control.name() == name)
                .unwrap();
            doc.edit_lab_scalar_definition(
                None,
                &control,
                LabScalarDefinition::Selection {
                    options,
                    selected: Some(selected),
                },
            )
            .unwrap();
        }
        let source = doc.lab_adjuster_metadata(None).remove(0);
        let destination = doc.numeric_lab_adjusters(None).remove(1);
        assert_eq!(source.name(), "Source");
        assert_eq!(destination.name(), "Destination");
        for (axis, adjuster) in [
            (Axis::X, doc.numeric_lab_adjusters(None).remove(0)),
            (Axis::Y, destination.clone()),
        ] {
            doc.set_numeric_property_link(&secondary_property, axis, Link::Existing(adjuster))
                .unwrap();
        }
        secondary_pair
            .retained_adjustment()
            .unwrap()
            .flags()
            .set(0, false)
            .unwrap();
        let inactive_link = || {
            let adjustment = secondary_pair.retained_adjustment().unwrap();
            assert!(!adjustment.flags().value(0).unwrap());
            String::from_utf16_lossy(adjustment.names().entries()[0].as_deref().unwrap())
        };
        let inactive_x = secondary_pair.pair().unwrap()[0];
        let original = pair.pair().unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.edit_lab_adjuster_metadata(None, &source, "Destination", -20)
            .unwrap();
        let merged_value = pair.pair().unwrap();
        let merged_options = [destination_options.clone(), source_options[1..].to_vec()].concat();
        assert_ne!(merged_value[0], original[0]);
        assert_eq!(merged_value[1], original[1]);
        assert_eq!(inactive_link(), "Destination");
        assert_eq!(secondary_pair.pair().unwrap()[0], inactive_x);
        let controls = doc.numeric_lab_adjusters(None);
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].selection_options(), merged_options);
        assert_eq!(controls[0].value(), LabNumericValue::Selection(0));
        assert_eq!(doc.lab_adjuster_metadata(None)[0].order(), 1);
        assert!(
            doc.edit_lab_scalar_definition(None, &destination, LabScalarDefinition::Number)
                .is_err()
        );
        assert_eq!(history.commands().len(), 1);
        assert_eq!(
            doc.numeric_property_link_name(&property, Axis::X)
                .as_deref(),
            Some("Destination")
        );
        history.undo().unwrap();
        assert_eq!(pair.pair().unwrap(), original);
        assert_eq!(inactive_link(), "Source");
        assert_eq!(
            doc.numeric_lab_adjusters(None)[0].selection_options(),
            source_options
        );
        assert_eq!(
            doc.numeric_lab_adjusters(None)[1].selection_options(),
            destination_options
        );
        assert_eq!(
            doc.numeric_property_link_name(&property, Axis::X)
                .as_deref(),
            Some("Source")
        );
        history.redo().unwrap();
        doc.edit_numeric_lab_adjuster(None, &controls[0], LabNumericValue::Selection(2))
            .unwrap();
        let updated = pair.pair().unwrap();
        assert_ne!(updated[0], merged_value[0]);
        assert_ne!(updated[1], merged_value[1]);
        let mut wire = BinaryWriter::default();
        doc.save(&mut wire).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(wire.bytes()).unwrap();
        assert_eq!(
            reopened.numeric_lab_adjusters(None)[0].selection_options(),
            merged_options
        );
        let filter = reopened.branch(0).unwrap().unwrap().filter().unwrap();
        assert_eq!(filter.parameter(0).unwrap().pair().unwrap(), updated);
    }

    #[test]
    fn linked_curves_share_edits_and_reconnect_the_catalog_on_open() {
        let first = PointwiseFilter::<()>::new(PointwiseKind::ToneCurve).unwrap();
        let second = PointwiseFilter::new(PointwiseKind::ToneCurve).unwrap();
        let a = first.parameter(0).unwrap();
        let b = second.parameter(0).unwrap();
        let original = a.curve().unwrap();
        let standalone = b.curve().unwrap();
        standalone.set_wrapped(true);
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(first.node()), Some(second.node())]).unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.set_curve_adjuster(&a, CurveAdjusterLink::Create("Shared Curve".into()))
            .unwrap();
        let adjuster = doc.curve_adjuster(&a).unwrap();
        doc.set_curve_adjuster(&b, CurveAdjusterLink::Existing(adjuster.clone()))
            .unwrap();
        assert!(b.curve().unwrap().same_reference(&original));
        assert_eq!(history.cursor(), 2);

        let next = original.source_clone().unwrap();
        next.set_minimum(0.25);
        next.set_maximum(0.75);
        doc.edit_composite_curve(&a, &original, next.clone(), "Edit Curve")
            .unwrap();
        assert!(a.curve().unwrap().same_reference(&next));
        assert!(b.curve().unwrap().same_reference(&next));
        assert_eq!(history.cursor(), 3);
        assert!(matches!(
            doc.edit_composite_curve(&b, &original, next.clone(), "Stale edit"),
            Err(GtxGraphError::History(crate::HistoryError::InputChanged))
        ));
        history.undo().unwrap();
        assert!(a.curve().unwrap().same_reference(&original));
        assert!(b.curve().unwrap().same_reference(&original));
        history.undo().unwrap();
        assert!(b.curve().unwrap().same_reference(&standalone));
        assert!(doc.curve_adjuster(&b).is_none());
        history.redo().unwrap();
        history.redo().unwrap();
        assert!(b.curve().unwrap().same_reference(&next));

        // Catalog value wins when a historical file stores a stale receiver copy.
        b.set_curve(Some(standalone)).unwrap();
        let mut wire = BinaryWriter::default();
        doc.save(&mut wire).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(wire.bytes()).unwrap();
        let read = |index| {
            reopened
                .branch(index)
                .flatten()
                .unwrap()
                .filter()
                .unwrap()
                .parameter(0)
                .unwrap()
        };
        let (a2, b2) = (read(0), read(1));
        let reopened_curve = a2.curve().unwrap();
        assert_eq!(
            (reopened_curve.minimum(), reopened_curve.maximum()),
            (0.25, 0.75)
        );
        assert!(reopened_curve.same_reference(&b2.curve().unwrap()));
        assert!(
            reopened
                .curve_adjuster(&a2)
                .unwrap()
                .same_reference(&reopened.curve_adjuster(&b2).unwrap())
        );

        b.set_curve(Some(next.clone())).unwrap();
        doc.set_curve_adjuster(&a, CurveAdjusterLink::Unlink)
            .unwrap();
        assert!(a.curve().unwrap().same_reference(&next));
        assert_eq!(doc.available_curve_adjusters(&a).len(), 1);
        doc.set_curve_adjuster(&b, CurveAdjusterLink::Unlink)
            .unwrap();
        assert!(doc.available_curve_adjusters(&a).is_empty());
        history.undo().unwrap();
        assert!(doc.curve_adjuster(&b).unwrap().same_reference(&adjuster));
    }

    #[test]
    fn lab_preset_freezes_shared_curves_and_applies_with_numeric_values() {
        use crate::{CurveRecord, LabNumericProperty};
        let first = PointwiseFilter::<()>::new(PointwiseKind::ToneCurve).unwrap();
        let second = PointwiseFilter::new(PointwiseKind::ToneCurve).unwrap();
        let a = first.parameter(0).unwrap();
        let b = second.parameter(0).unwrap();
        let red = first.parameter(1).unwrap();
        let original = a.curve().unwrap();
        original.set_minimum(0.25);
        original.set_maximum(0.25);
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(first.node()), Some(second.node())]).unwrap();
        doc.set_curve_adjuster(&a, CurveAdjusterLink::Create("Response".into()))
            .unwrap();
        let adjuster = doc.curve_adjuster(&a).unwrap();
        doc.set_curve_adjuster(&b, CurveAdjusterLink::Existing(adjuster.clone()))
            .unwrap();
        doc.set_numeric_property_link(
            &LabNumericProperty::filter(&red).unwrap(),
            LabNumericAxis::Whole,
            LabNumericLink::Create("Red".into()),
        )
        .unwrap();
        doc.edit_lab_preset(None, LabPresetOperation::Save("Low".into()))
            .unwrap();
        let preset = doc.lab_presets(None).presets.remove(0);

        let changed = original.source_clone().unwrap();
        changed.set_minimum(0.75);
        changed.set_maximum(0.75);
        doc.edit_composite_curve(&a, &original, changed.clone(), "Edit Curve")
            .unwrap();
        let number = doc.numeric_lab_adjusters(None).remove(0);
        doc.edit_numeric_lab_adjuster(None, &number, LabNumericValue::Number(100.))
            .unwrap();
        assert_eq!(red.index(), 1);

        let batch = super::curve_adjusters::CurveAdjusterBatch::prepare(
            &doc.branches(),
            vec![(adjuster, Arc::new(CurveRecord::new(0.4, 0.4, false)))],
        )
        .unwrap();
        assert!(matches!(
            batch.publish(crate::HistoryDirection::Redo, || Err::<(), _>(
                crate::HistoryError::InputChanged
            )),
            Err(crate::HistoryError::InputChanged)
        ));
        assert!(a.curve().unwrap().same_reference(&changed));
        assert!(b.curve().unwrap().same_reference(&changed));
        assert_eq!(red.index(), 1);

        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.edit_lab_preset(None, LabPresetOperation::Apply(preset))
            .unwrap();
        let applied = a.curve().unwrap();
        assert_eq!((applied.minimum(), applied.maximum()), (0.25, 0.25));
        assert!(applied.same_reference(&b.curve().unwrap()));
        assert_eq!(red.index(), 0);
        assert_eq!(history.cursor(), 1);
        history.undo().unwrap();
        assert!(a.curve().unwrap().same_reference(&changed));
        assert!(b.curve().unwrap().same_reference(&changed));
        assert_eq!(red.index(), 1);
        history.redo().unwrap();
        assert!(a.curve().unwrap().same_reference(&applied));
        assert!(b.curve().unwrap().same_reference(&applied));
        assert_eq!(red.index(), 0);

        history.undo().unwrap();
        for native in [true, false] {
            let mut wire = BinaryWriter::default();
            let mut reopened = if native {
                doc.save_project(&mut wire).unwrap();
                Gtx375GraphDocument::<()>::load_project(
                    wire.bytes(),
                    crate::NodeWireReadPolicy::new(32, 128),
                )
                .unwrap()
            } else {
                doc.save(&mut wire).unwrap();
                Gtx375GraphDocument::<()>::load(wire.bytes()).unwrap()
            };
            let filter = |index| reopened.branch(index).flatten().unwrap().filter().unwrap();
            let first = filter(0);
            let a = first.parameter(0).unwrap();
            let b = filter(1).parameter(0).unwrap();
            assert_eq!(a.curve().unwrap().minimum(), 0.75);
            let saved = reopened.lab_presets(None).presets.remove(0);
            reopened
                .edit_lab_preset(None, LabPresetOperation::Apply(saved))
                .unwrap();
            assert_eq!(a.curve().unwrap().minimum(), 0.25);
            assert!(a.curve().unwrap().same_reference(&b.curve().unwrap()));
            assert_eq!(first.parameter(1).unwrap().index(), 0);
        }
    }

    #[test]
    fn lab_metadata_renames_and_merges_linked_curves() {
        let first = PointwiseFilter::<()>::new(PointwiseKind::ToneCurve).unwrap();
        let second = PointwiseFilter::new(PointwiseKind::ToneCurve).unwrap();
        let destination = PointwiseFilter::new(PointwiseKind::ToneCurve).unwrap();
        let a = first.parameter(0).unwrap();
        let b = second.parameter(0).unwrap();
        let c = destination.parameter(0).unwrap();
        let source_curve = a.curve().unwrap();
        let destination_curve = c.curve().unwrap();
        destination_curve.set_wrapped(true);
        let mut doc = Gtx375GraphDocument::new(vec![
            Some(first.node()),
            Some(second.node()),
            Some(destination.node()),
        ])
        .unwrap();
        doc.set_curve_adjuster(&a, CurveAdjusterLink::Create("Near".into()))
            .unwrap();
        let link = doc.curve_adjuster(&a).unwrap();
        doc.set_curve_adjuster(&b, CurveAdjusterLink::Existing(link))
            .unwrap();
        doc.set_curve_adjuster(&c, CurveAdjusterLink::Create("Far".into()))
            .unwrap();
        let metadata = doc.lab_adjuster_metadata(None)[0].clone();
        assert_eq!(metadata.type_label(), "Curve");
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.edit_lab_adjuster_metadata(None, &metadata, "Middle", 25)
            .unwrap();
        let choices = doc
            .available_curve_adjusters(&a)
            .iter()
            .map(|choice| String::from_utf16_lossy(choice.name()))
            .collect::<Vec<_>>();
        assert_eq!(choices, ["Far", "Middle"]);
        assert_eq!(
            String::from_utf16_lossy(doc.curve_adjuster(&b).unwrap().name()),
            "Middle"
        );
        assert!(b.curve().unwrap().same_reference(&source_curve));
        let middle = doc.lab_adjuster_metadata(None)[1].clone();
        assert_eq!(middle.order(), 25);

        doc.edit_lab_adjuster_metadata(None, &middle, "Far", -20)
            .unwrap();
        let remaining = doc.lab_adjuster_metadata(None);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].order(), 1);
        assert!(a.curve().unwrap().same_reference(&destination_curve));
        assert!(b.curve().unwrap().same_reference(&destination_curve));
        assert_eq!(history.cursor(), 2);
        history.undo().unwrap();
        assert!(a.curve().unwrap().same_reference(&source_curve));
        assert_eq!(
            String::from_utf16_lossy(doc.curve_adjuster(&b).unwrap().name()),
            "Middle"
        );
        history.redo().unwrap();
        assert!(b.curve().unwrap().same_reference(&destination_curve));
        let mut wire = BinaryWriter::default();
        doc.save(&mut wire).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(wire.bytes()).unwrap();
        let read = |index| {
            reopened
                .branch(index)
                .flatten()
                .unwrap()
                .filter()
                .unwrap()
                .parameter(0)
                .unwrap()
        };
        let curve = read(0).curve().unwrap();
        assert!(curve.is_wrapped());
        assert!(curve.same_reference(&read(1).curve().unwrap()));
        assert!(curve.same_reference(&read(2).curve().unwrap()));
        let adjusters = reopened.lab_adjuster_metadata(None);
        assert_eq!(adjusters.len(), 1);
        assert_eq!(adjusters[0].name(), "Far");
    }

    #[test]
    fn lab_metadata_merges_environment_adjusters_into_the_destination_map() {
        let first = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        let second = PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        let a = first.parameter(0).unwrap();
        let b = second.parameter(0).unwrap();
        let map = |red| {
            let mut map = crate::EnvironmentRecord::default();
            map.add_layer(crate::EnvironmentLayerKind::Color).unwrap();
            map.set_field(crate::EnvironmentField::Red(0), red).unwrap();
            Arc::new(map)
        };
        let first_map = map(0.25);
        let second_map = map(0.75);
        a.set_environment(first_map.clone()).unwrap();
        b.set_environment(second_map.clone()).unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(first.node()), Some(second.node())]).unwrap();
        doc.set_environment_adjuster(&a, EnvironmentAdjusterLink::Create("Dawn".into()))
            .unwrap();
        doc.set_environment_adjuster(&b, EnvironmentAdjusterLink::Create("Dusk".into()))
            .unwrap();
        let saved_destination = doc.environment_adjuster(&b).unwrap();
        let before = doc.environment_adjuster(&a).unwrap().value();
        let after = saved_destination.value();
        let source = doc.lab_adjuster_metadata(None).remove(0);
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let property_map = || match a.raw_value() {
            crate::PointwiseParameterValue::Environment(map) => map,
            _ => panic!("expected Environment"),
        };
        doc.edit_lab_adjuster_metadata(None, &source, "Dusk", -20)
            .unwrap();
        assert!(Arc::ptr_eq(&property_map(), &after));
        assert!(
            doc.environment_adjuster(&a)
                .unwrap()
                .same_reference(&saved_destination)
        );
        assert_eq!(doc.lab_adjuster_metadata(None).len(), 1);
        history.undo().unwrap();
        assert!(Arc::ptr_eq(&property_map(), &before));
        assert_eq!(doc.lab_adjuster_metadata(None).len(), 2);
        history.redo().unwrap();
        assert!(Arc::ptr_eq(&property_map(), &after));
        let mut wire = BinaryWriter::default();
        doc.save(&mut wire).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(wire.bytes()).unwrap();
        let link = |index| {
            let filter = reopened.branch(index).unwrap().unwrap().filter().unwrap();
            reopened
                .environment_adjuster(&filter.parameter(0).unwrap())
                .unwrap()
        };
        assert!(link(0).same_reference(&link(1)));
    }

    #[test]
    fn environment_name_change_preserves_map_and_resolves_stale_link_choice() {
        let first = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        let second = PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        let a = first.parameter(0).unwrap();
        let b = second.parameter(0).unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(first.node()), Some(second.node())]).unwrap();
        doc.set_environment_adjuster(&a, EnvironmentAdjusterLink::Create("Sky".into()))
            .unwrap();
        let old = doc.environment_adjuster(&a).unwrap();
        let meta = doc.lab_adjuster_metadata(None).remove(0);
        assert_eq!(meta.type_label(), "Environment");
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.edit_lab_adjuster_metadata(None, &meta, "Evening sky", -10)
            .unwrap();
        let current = doc.environment_adjuster(&a).unwrap();
        assert!(old.same_reference(&current));
        assert_eq!(String::from_utf16_lossy(current.name()), "Evening sky");
        doc.set_environment_adjuster(&b, EnvironmentAdjusterLink::Existing(old))
            .unwrap();
        assert!(
            doc.environment_adjuster(&b)
                .unwrap()
                .same_reference(&current)
        );
        history.undo().unwrap();
        history.undo().unwrap();
        assert!(doc.environment_adjuster(&b).is_none());
        assert_eq!(
            String::from_utf16_lossy(doc.environment_adjuster(&a).unwrap().name()),
            "Sky"
        );
        history.redo().unwrap();
        history.redo().unwrap();
        let mut wire = BinaryWriter::default();
        doc.save(&mut wire).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(wire.bytes()).unwrap();
        let entry = &reopened.lab_adjuster_metadata(None)[0];
        assert_eq!((entry.name(), entry.order()), ("Evening sky", -10));
        for index in 0..2 {
            let filter = reopened.branch(index).unwrap().unwrap().filter().unwrap();
            let linked = reopened
                .environment_adjuster(&filter.parameter(0).unwrap())
                .unwrap();
            assert_eq!(String::from_utf16_lossy(linked.name()), "Evening sky");
        }
    }

    #[test]
    fn dual_lab_mapping_changes_one_curve_and_preserves_the_link() {
        use crate::{LabNumericAxis as Axis, LabNumericLink as Link, LabNumericProperty};
        let filter = PointwiseFilter::<()>::new(PointwiseKind::Distort).unwrap();
        let pair = filter.parameter(0).unwrap();
        let property = LabNumericProperty::filter(&pair).unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(filter.node())]).unwrap();
        doc.set_numeric_property_link(&property, Axis::Whole, Link::Create("Size".into()))
            .unwrap();
        let control = doc.numeric_lab_adjusters(None).remove(0);
        doc.edit_numeric_lab_adjuster(None, &control, LabNumericValue::Dual([25., 75.]))
            .unwrap();
        let initial = pair.pair().unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let x = doc.numeric_property_mapping(&property, Axis::X).unwrap();
        let y = doc.numeric_property_mapping(&property, Axis::Y).unwrap();
        assert!(!doc.edit_numeric_property_mapping(&x).unwrap());
        let minimum = x.curve().minimum();
        x.curve().set_maximum(minimum);
        assert_eq!(pair.pair().unwrap(), initial);
        assert!(doc.edit_numeric_property_mapping(&x).unwrap());
        let edited = [f64::from(minimum), initial[1]];
        assert_eq!(pair.pair().unwrap(), edited);
        assert!(doc.edit_numeric_property_mapping(&y).is_err());
        assert_eq!(
            doc.numeric_lab_adjusters(None)[0].value(),
            LabNumericValue::Dual([25., 75.])
        );
        assert_eq!(history.commands().len(), 1);
        history.undo().unwrap();
        assert_eq!(pair.pair().unwrap(), initial);
        history.redo().unwrap();
        assert_eq!(pair.pair().unwrap(), edited);
        let mut saved = BinaryWriter::default();
        doc.save(&mut saved).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(saved.bytes()).unwrap();
        let pair = reopened
            .branch(0)
            .unwrap()
            .unwrap()
            .filter()
            .unwrap()
            .parameter(0)
            .unwrap();
        assert_eq!(pair.pair().unwrap(), edited);
    }

    #[test]
    fn dual_lab_range_rebases_both_axes_and_round_trips() {
        use crate::{LabNumericAxis::Whole, LabNumericLink as Link, LabNumericProperty};
        let filter = PointwiseFilter::<()>::new(PointwiseKind::Distort).unwrap();
        let pair = filter.parameter(0).unwrap();
        let property = LabNumericProperty::filter(&pair).unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(filter.node())]).unwrap();
        doc.set_numeric_property_link(&property, Whole, Link::Create("Size".into()))
            .unwrap();
        let control = || doc.numeric_lab_adjusters(None).remove(0);
        doc.edit_numeric_lab_adjuster(None, &control(), LabNumericValue::Dual([25.5, 75.5]))
            .unwrap();
        let original = control();
        let initial_pair = pair.pair().unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let invalid = LabNumericRange::new([25.1, 25.4], true);
        assert!(matches!(
            doc.edit_lab_numeric_range(None, &original, invalid),
            Err(GtxGraphError::NumericAdjuster(_))
        ));
        let edited = LabNumericRange::new([-50.4, 70.4], true);
        assert!(doc.edit_lab_numeric_range(None, &original, edited).unwrap());
        let current = doc.numeric_lab_adjusters(None).remove(0);
        let range = LabNumericRange::new([-50., 70.], true);
        assert_eq!(current.range_settings(), Some(range));
        assert_eq!(current.value(), LabNumericValue::Dual([26., 70.]));
        let rebased_pair = pair.pair().unwrap();
        assert_ne!(initial_pair[0], rebased_pair[0]);
        assert_ne!(initial_pair[1], rebased_pair[1]);
        assert_eq!(history.commands().len(), 1);
        assert!(!doc.edit_lab_numeric_range(None, &current, range).unwrap());
        history.undo().unwrap();
        assert_eq!(pair.pair().unwrap(), initial_pair);
        assert_eq!(doc.numeric_lab_adjusters(None)[0].range(), Some([0., 100.]));
        history.redo().unwrap();
        assert_eq!(pair.pair().unwrap(), rebased_pair);
        let mut saved = BinaryWriter::default();
        doc.save(&mut saved).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(saved.bytes()).unwrap();
        let current = reopened.numeric_lab_adjusters(None).remove(0);
        assert_eq!(current.range_settings(), Some(range));
        assert_eq!(current.value(), LabNumericValue::Dual([26., 70.]));
        let pair = reopened
            .branch(0)
            .unwrap()
            .unwrap()
            .filter()
            .unwrap()
            .parameter(0)
            .unwrap();
        assert_eq!(pair.pair().unwrap(), rebased_pair);
    }

    #[test]
    fn mixed_lab_preset_applies_once_and_survives_gtx_round_trip() {
        use crate::{
            ImageGenerator, ImageGeneratorKind, ImageGeneratorParameterKind as Kind,
            ValueAdjusterPayload,
        };

        let first = PointwiseFilter::<()>::new(PointwiseKind::Distort).unwrap();
        let second = PointwiseFilter::<()>::new(PointwiseKind::Distort).unwrap();
        let map = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        let solid = PointwiseNode::solid_color().unwrap();
        solid.set_solid_color([0.25, 0., 0.]).unwrap();
        let invert = PointwiseNode::invert();
        invert.slot(0).unwrap().set_node(Some(&solid)).unwrap();
        let generator = ImageGenerator::with_kind(ImageGeneratorKind::Noise).unwrap();
        let material = GtxCatalogRecord::read(
            &mut BinaryCursor::new(MATERIAL_CATALOG_BYTES).unwrap(),
            3.83,
        )
        .unwrap();
        let synthesis = material_receiver(&material);
        let link =
            |adjustment: crate::SharedPropertyAdjustment, axis, name: &str, range: [f32; 2]| {
                adjustment
                    .names()
                    .set_entry(axis, Some(name.encode_utf16().collect()))
                    .unwrap();
                adjustment.flags().set(axis, true).unwrap();
                let curve = adjustment.curve(axis).unwrap();
                curve.set_minimum(range[0]);
                curve.set_maximum(range[1]);
            };
        let adjustment = |filter: &PointwiseFilter, index| {
            filter
                .parameter(index)
                .unwrap()
                .adjustment()
                .unwrap()
                .unwrap()
        };
        link(adjustment(&first, 0), 0, "Size", [0., 4.]);
        link(adjustment(&first, 0), 1, "Size", [0., 8.]);
        link(adjustment(&first, 1), 0, "Steps", [1., 9.]);
        link(adjustment(&first, 2), 0, "Sampling", [1., 2.]);
        link(adjustment(&second, 1), 0, "Steps", [1., 17.]);
        let size = second.parameter(0).unwrap();
        link(adjustment(&second, 0), 0, "Steps", [0., 8.]);
        let current = size.retained_adjustment().unwrap();
        let mut parts = current.capture_parts();
        parts.property_type = 0; // Genetica also links a Number to one axis of a pair.
        assert!(size.wire_metadata().compare_and_replace_adjustment(
            Some(&current),
            Some(crate::SharedPropertyAdjustment::from_capture_parts(parts)),
        ));
        link(
            solid
                .solid_model()
                .unwrap()
                .transparency_property()
                .adjustment()
                .unwrap()
                .unwrap(),
            0,
            "Steps",
            [0., 100.],
        );
        link(
            invert
                .invert_property(0)
                .unwrap()
                .adjustment()
                .unwrap()
                .unwrap(),
            0,
            "Sampling",
            [1., 2.],
        );
        let generator_adjustment = |kind| generator.parameter(kind).adjustment().unwrap().unwrap();
        link(generator_adjustment(Kind::Seed), 0, "Steps", [0., 16.]);
        link(generator_adjustment(Kind::Mode), 0, "Sampling", [1., 4.]);
        link(generator_adjustment(Kind::Size), 0, "Size", [0., 4.]);
        link(generator_adjustment(Kind::Size), 1, "Size", [0., 8.]);
        let property = map.parameter(0).unwrap();
        property.set_environment(Arc::new(red_map(0.25))).unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![
            Some(first.node()),
            Some(second.node()),
            Some(map.node()),
            Some(invert),
            Some(generator.node()),
            Some(synthesis),
        ])
        .unwrap();
        doc.set_environment_adjuster(&property, EnvironmentAdjusterLink::Create("Sky".into()))
            .unwrap();
        let mut entry = |kind, name: &str, value| {
            let mut record =
                ValueAdjusterRecord::new(kind, Some(name.encode_utf16().collect()), [0; 16]);
            match &mut record.payload {
                ValueAdjusterPayload::Number(number) => {
                    number.value = value;
                    number.previous = value;
                    if kind == ValueAdjusterKind::Selection {
                        number.selected = 0;
                        number.previous_selected = 0;
                        number.options =
                            Vec::from([("Final", value), ("Draft", 0.0)].map(|(label, value)| {
                                crate::SelectionOptionRecord {
                                    name: Some(label.encode_utf16().collect()),
                                    value,
                                }
                            }));
                    }
                }
                ValueAdjusterPayload::Dual(dual) => {
                    dual.value = [value, 50.];
                    dual.previous = dual.value;
                }
                _ => unreachable!(),
            }
            doc.catalog
                .stage(GtxCatalogRecord::Value(Box::new(record)))
                .store(true, std::sync::atomic::Ordering::Release);
        };
        entry(ValueAdjusterKind::Number, "Steps", 50.);
        entry(ValueAdjusterKind::Selection, "Sampling", 100.);
        entry(ValueAdjusterKind::Dual, "Size", 75.);
        doc.catalog
            .stage(material)
            .store(true, std::sync::atomic::Ordering::Release);
        doc.edit_environment_lab_preset(None, LabPresetOperation::Save("Final".into()))
            .unwrap();
        for (catalog, _) in doc.catalog.entries() {
            let GtxCatalogRecord::Numeric { metadata, value } = catalog else {
                continue;
            };
            match &mut *value.lock() {
                ValueAdjusterPayload::Number(number) => {
                    if metadata.type_tag == 4 {
                        number.selected = 1;
                        number.previous_selected = 1;
                        // Reconnection uses the numeric value read from the
                        // catalog, even when it differs from the option value.
                        number.value = 25.;
                    } else {
                        number.value = 12.5;
                    }
                    number.previous = number.value;
                }
                ValueAdjusterPayload::Dual(dual) => {
                    dual.value = [25., 12.5];
                    dual.previous = dual.value;
                }
                _ => unreachable!(),
            }
        }
        let mut initial = BinaryWriter::default();
        doc.save(&mut initial).unwrap();
        let mut doc = Gtx375GraphDocument::<()>::load(initial.bytes()).unwrap();
        let first = doc.branch(0).unwrap().unwrap().filter().unwrap();
        let second = doc.branch(1).unwrap().unwrap().filter().unwrap();
        let map = doc
            .branch(2)
            .unwrap()
            .unwrap()
            .filter()
            .unwrap()
            .parameter(0)
            .unwrap();
        let adjuster = doc.environment_adjuster(&map).unwrap();
        doc.edit_environment_adjuster(&map, &adjuster, &adjuster.value(), red_map(0.5))
            .unwrap();
        let synthesis = doc.branch(5).flatten().unwrap().filter().unwrap();
        let mut edit = synthesis.synthesis_edit().unwrap();
        edit.add_layer(crate::SynthesisLayerKind::Solid);
        assert!(synthesis.edit_synthesis(edit, "Change Material").unwrap());
        let layers = || synthesis.synthesis_edit().unwrap().layers();
        assert_eq!(layers(), 2);

        let preset = doc.environment_lab_presets(None).presets.remove(0);
        let invert = doc.branch(3).unwrap().unwrap();
        let solid = invert.slot(0).unwrap().node().unwrap();
        let generator = doc
            .branch(4)
            .unwrap()
            .unwrap()
            .image_generator_handle()
            .unwrap();
        let pixels = |invert: &PointwiseNode| {
            invert
                .render_original(4, 4)
                .unwrap()
                .image()
                .unwrap()
                .channel(0, true)
                .unwrap()
                .read(0)
                .unwrap()
        };
        let generator_values = |generator: &ImageGenerator| {
            (
                generator.parameter(Kind::Seed).number().unwrap(),
                generator.parameter(Kind::Mode).index().unwrap(),
                generator.parameter(Kind::Size).pair().unwrap(),
            )
        };
        assert_eq!(generator_values(&generator), (2., 1, [1., 1.]));
        assert_eq!(solid.solid_transparency().unwrap(), 12.5);
        assert_eq!(pixels(&invert), 0.75);

        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let sampling = doc
            .catalog
            .entries()
            .find_map(|(entry, _)| match entry {
                GtxCatalogRecord::Numeric { metadata, value } if metadata.type_tag == 4 => {
                    Some(value.clone())
                }
                _ => None,
            })
            .unwrap();
        let selected = || match sampling.preset_value(GtxPresetKind::Selection).unwrap() {
            GtxPresetValue::Number(index) => index,
            _ => unreachable!(),
        };
        assert_eq!(selected(), 1.);
        let stale = doc
            .numeric_lab_adjusters(None)
            .into_iter()
            .find(|control| control.name() == "Steps")
            .unwrap();
        let values = || {
            (
                first.parameter(0).unwrap().pair().unwrap(),
                first.parameter(1).unwrap().number(),
                first.parameter(2).unwrap().index(),
                second.parameter(1).unwrap().number(),
                second.parameter(0).unwrap().pair().unwrap(),
                adjuster.value().field(EnvironmentField::Red(0)),
            )
        };
        assert_eq!(values(), ([1., 1.], 2., 0, 3., [1., 1.], Some(0.5)));
        doc.edit_environment_lab_preset(None, LabPresetOperation::Apply(preset))
            .unwrap();
        assert_eq!(values(), ([3., 4.], 5., 1, 9., [4., 1.], Some(0.25)));
        assert_eq!(generator_values(&generator), (8., 3, [3., 4.]));
        assert_eq!(solid.solid_transparency().unwrap(), 50.);
        assert_eq!(pixels(&invert), 0.25);
        assert_eq!(selected(), 0.);
        assert_eq!(layers(), 1);
        assert_eq!(history.commands().len(), 1);
        assert!(matches!(
            doc.edit_numeric_lab_adjuster(None, &stale, LabNumericValue::Number(75.)),
            Err(GtxGraphError::NumericAdjuster(_))
        ));
        assert_eq!(history.commands().len(), 1);
        history.undo().unwrap();
        assert_eq!(values(), ([1., 1.], 2., 0, 3., [1., 1.], Some(0.5)));
        assert_eq!(generator_values(&generator), (2., 1, [1., 1.]));
        assert_eq!(solid.solid_transparency().unwrap(), 12.5);
        assert_eq!(pixels(&invert), 0.75);
        assert_eq!(selected(), 1.);
        assert_eq!(layers(), 2);
        history.redo().unwrap();
        assert_eq!(values(), ([3., 4.], 5., 1, 9., [4., 1.], Some(0.25)));
        assert_eq!(generator_values(&generator), (8., 3, [3., 4.]));
        assert_eq!(solid.solid_transparency().unwrap(), 50.);
        assert_eq!(pixels(&invert), 0.25);
        assert_eq!(selected(), 0.);
        assert_eq!(layers(), 1);

        let mut saved = BinaryWriter::default();
        doc.save(&mut saved).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(saved.bytes()).unwrap();
        let first = reopened.branch(0).unwrap().unwrap().filter().unwrap();
        assert_eq!(first.parameter(0).unwrap().pair().unwrap(), [3., 4.]);
        assert_eq!(first.parameter(2).unwrap().index(), 1);
        assert_eq!(pixels(&reopened.branch(3).unwrap().unwrap()), 0.25);
        assert_eq!(
            reopened
                .branch(5)
                .flatten()
                .unwrap()
                .filter()
                .unwrap()
                .synthesis_edit()
                .unwrap()
                .layers(),
            1,
        );
        assert_eq!(
            generator_values(
                &reopened
                    .branch(4)
                    .unwrap()
                    .unwrap()
                    .image_generator_handle()
                    .unwrap()
            ),
            (8., 3, [3., 4.])
        );
        assert!(reopened.catalog().unwrap().iter().any(|entry| {
            matches!(entry, GtxCatalogRecord::Numeric { value, .. }
                if matches!(value.preset_value(GtxPresetKind::Selection), Some(GtxPresetValue::Number(0.))))
        }));
    }

    pub(super) struct LabTestHistory;

    impl crate::HistoryServices<()> for LabTestHistory {
        fn route_property_history(
            &self,
            _: &crate::History<()>,
            _: &crate::HistoryTarget<()>,
            _: Option<&crate::ContextUndoInfo<()>>,
        ) -> Result<Option<crate::History<()>>, crate::HistoryError<()>> {
            Ok(None)
        }

        fn resolve_command_context(
            &self,
            _: &crate::History<()>,
            _: &crate::HistoryCommand<()>,
        ) -> Result<Option<crate::HistoryTarget<()>>, crate::HistoryError<()>> {
            Ok(None)
        }

        fn effect(
            &self,
            _: &crate::History<()>,
            _: Option<&crate::HistoryCommand<()>>,
            _: crate::HistoryEffect,
        ) -> Result<(), crate::HistoryError<()>> {
            Ok(())
        }

        fn pending_commands(
            &self,
            _: &crate::History<()>,
            _: &crate::HistoryCommand<()>,
        ) -> Result<Vec<crate::HistoryCommand<()>>, crate::HistoryError<()>> {
            Ok(Vec::new())
        }
    }

    const MATERIAL_CATALOG_BYTES: &[u8] =
        include_bytes!("../../../../../port/evidence/synthesis-material-catalog-original.bin");

    #[test]
    fn synthesis_adjuster_choices_are_scoped_to_the_receivers_lab_section() {
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        group.set_group_guid([7; 16]);
        group.parameter(4).unwrap().set_index(0).unwrap();
        let inside = PointwiseFilter::new(PointwiseKind::Synthesis).unwrap();
        let outside = PointwiseFilter::new(PointwiseKind::Synthesis).unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&inside.node()))
            .unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(group.node()), Some(outside.node())]).unwrap();
        doc.set_synthesis_adjuster(&outside, SynthesisAdjusterLink::Create("Material".into()))
            .unwrap();
        let outer_material = doc.synthesis_adjuster(&outside).unwrap();
        assert!(doc.available_synthesis_adjusters(&inside).is_empty());
        assert!(matches!(
            doc.set_synthesis_adjuster(
                &inside,
                SynthesisAdjusterLink::Existing(outer_material.clone())
            ),
            Err(GtxGraphError::SynthesisAdjuster(_))
        ));
        doc.set_synthesis_adjuster(&inside, SynthesisAdjusterLink::Create("Material".into()))
            .unwrap();
        let inner_material = doc.synthesis_adjuster(&inside).unwrap();
        assert!(!inner_material.same_reference(&outer_material));
        let choices = doc.available_synthesis_adjusters(&inside);
        assert_eq!(choices.len(), 1);
        assert!(choices[0].same_reference(&inner_material));
        assert_eq!(
            inside
                .synthesis_wire()
                .unwrap()
                .retained_adjustment()
                .unwrap()
                .guid(),
            [7; 16]
        );
    }

    fn material_receiver(entry: &GtxCatalogRecord) -> PointwiseNode<()> {
        let filter = PointwiseFilter::<()>::new(PointwiseKind::Synthesis).unwrap();
        let adjuster = filter
            .synthesis_wire()
            .unwrap()
            .adjustment(10, 1, None)
            .unwrap()
            .unwrap();
        adjuster
            .names()
            .set_entry(0, entry.metadata().name.clone())
            .unwrap();
        adjuster.flags().set(0, true).unwrap();
        filter.node()
    }

    #[test]
    fn deleting_shared_synthesis_texture_layer_retires_number_and_restores_preset() {
        use crate::{
            AdjustmentRecord, SynthesisLayerKind, SynthesisNumberField, ValueAdjusterPayload,
        };
        let first = PointwiseFilter::<()>::new(PointwiseKind::Synthesis).unwrap();
        let second = PointwiseFilter::<()>::new(PointwiseKind::Synthesis).unwrap();
        let mut texture = first.synthesis_edit().unwrap();
        let initial_layers = texture.layers();
        let patch = 0;
        texture.add_layer(SynthesisLayerKind::Patches);
        let field = SynthesisNumberField::PatchFrequency(0);
        let mut record = AdjustmentRecord::new(&[[0., 100.]]);
        let component = &mut record.components_mut()[0];
        let name: Vec<_> = "Placement".encode_utf16().collect();
        component.name.clone_from(&name);
        component.active = true;
        component.curve_mut();
        assert_eq!(
            texture.set_number_adjustment(Some(patch), field, Some((record, 40.))),
            Some(true)
        );
        first.edit_synthesis(texture, "Patches fixture").unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(first.node()), Some(second.node())]).unwrap();
        let mut number = ValueAdjusterRecord::new(ValueAdjusterKind::Number, Some(name), [0; 16]);
        let ValueAdjusterPayload::Number(value) = &mut number.payload else {
            unreachable!()
        };
        value.value = 40.;
        let presence = doc.catalog.stage(GtxCatalogRecord::Value(Box::new(number)));
        presence.store(true, std::sync::atomic::Ordering::Release);
        // Imported Material can already carry its own nested Number receiver.
        let value = crate::SynthesisCatalogValue::new(first.synthesis_link_state().unwrap().local);
        doc.catalog
            .stage(GtxCatalogRecord::Synthesis {
                metadata: AdjusterMetadataRecord {
                    name: Some("Material".encode_utf16().collect()),
                    group_id: [0; 16],
                    type_tag: 10,
                    order: 1,
                    loaded: true,
                },
                value,
            })
            .store(true, std::sync::atomic::Ordering::Release);
        let material = doc.available_synthesis_adjusters(&first).remove(0);
        doc.set_synthesis_adjuster(&first, SynthesisAdjusterLink::Existing(material.clone()))
            .unwrap();
        doc.set_synthesis_adjuster(&second, SynthesisAdjusterLink::Existing(material))
            .unwrap();
        doc.edit_lab_preset(None, LabPresetOperation::Save("Setup".into()))
            .unwrap();

        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let counts = || {
            [
                first.synthesis_edit().unwrap().layers(),
                second.synthesis_edit().unwrap().layers(),
            ]
        };
        let visible_numbers = |doc: &Gtx375GraphDocument<()>| {
            doc.presets
                .groups()
                .iter()
                .filter(|group| group.presence().load(std::sync::atomic::Ordering::Acquire))
                .flat_map(|group| group.presets())
                .filter(|preset| !preset.is_hidden() && preset.kind() == GtxPresetKind::Number)
                .count()
        };
        assert_eq!(visible_numbers(&doc), 1);
        let mut removal = first.synthesis_edit().unwrap();
        assert!(removal.remove_layer(patch));
        assert!(
            doc.edit_synthesis_removing_unused_adjusters(
                &first.node(),
                &removal,
                "Remove linked Patches"
            )
            .unwrap()
        );
        assert_eq!(counts(), [initial_layers; 2]);
        assert!(doc.numeric_lab_adjusters(None).is_empty());
        assert_eq!(visible_numbers(&doc), 0);
        assert_eq!(history.commands().len(), 1);
        history.undo().unwrap();
        assert_eq!(counts(), [initial_layers + 1; 2]);
        assert_eq!(
            second
                .synthesis_edit()
                .unwrap()
                .number_value(Some(patch), field),
            Some(40.)
        );
        assert_eq!(doc.numeric_lab_adjusters(None)[0].name(), "Placement");
        assert_eq!(visible_numbers(&doc), 1);
        history.redo().unwrap();
        assert_eq!(counts(), [initial_layers; 2]);
        assert!(doc.numeric_lab_adjusters(None).is_empty());

        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        let restored = Gtx375GraphDocument::<()>::load_project(
            native.bytes(),
            crate::NodeWireReadPolicy::new(128, 4096),
        )
        .unwrap();
        assert!(restored.numeric_lab_adjusters(None).is_empty());
        assert_eq!(
            restored
                .branch(1)
                .flatten()
                .unwrap()
                .filter()
                .unwrap()
                .synthesis_edit()
                .unwrap()
                .layers(),
            initial_layers
        );
    }

    #[test]
    fn material_catalog_value_reopens_and_shares_synthesis_edits_and_history() {
        let original = MATERIAL_CATALOG_BYTES;
        let mut reader = BinaryCursor::new(original).unwrap();
        let mut entry = GtxCatalogRecord::read(&mut reader, 3.83).unwrap();
        assert_eq!(reader.position().unwrap() as usize, original.len());
        assert_eq!(entry.metadata().order, -10);
        let record = entry.as_synthesis().unwrap();
        assert_eq!(record.layer_count(), 1);
        assert!(record.uses_input());

        let mut wire = BinaryWriter::default();
        entry.save(&mut wire).unwrap();
        assert_eq!(wire.bytes(), original);
        let preset = GtxPresetRecord::new(
            GtxPresetKind::Synthesis,
            entry.metadata().name.clone(),
            entry.metadata().group_id,
        )
        .with_value(GtxPresetValue::Synthesis(record));
        let mut wire = BinaryWriter::default();
        preset.save(&mut wire).unwrap();
        let preset =
            GtxPresetRecord::read(&mut BinaryCursor::new(wire.bytes()).unwrap(), 3.75).unwrap();
        assert!(matches!(preset.value(), GtxPresetValue::Synthesis(record) if record.uses_input()));

        let mut document = Gtx375GraphDocument::new(vec![
            Some(material_receiver(&entry)),
            Some(material_receiver(&entry)),
        ])
        .unwrap();
        document.catalog = GraphCatalog::new(vec![entry]);
        let mut file = BinaryWriter::default();
        document.save_compatible_gtx(&mut file).unwrap();
        let mut restored = Gtx375GraphDocument::<()>::load(file.bytes()).unwrap();
        let first = restored.branch(0).flatten().unwrap().filter().unwrap();
        let second = restored.branch(1).flatten().unwrap().filter().unwrap();
        assert_eq!(first.synthesis_edit().unwrap().layers(), 1);
        assert_eq!(second.synthesis_edit().unwrap().layers(), 1);
        assert!(restored.lab_presets(None).can_capture);
        restored
            .edit_lab_preset(None, LabPresetOperation::Save("Material".into()))
            .unwrap();
        assert_eq!(restored.lab_presets(None).presets.len(), 1);

        let history = restored
            .bind_property_history(Arc::new(LabTestHistory))
            .unwrap();
        history.set_recording(true);
        let mut edit = first.synthesis_edit().unwrap();
        edit.add_layer(crate::SynthesisLayerKind::Solid);
        assert!(first.edit_synthesis(edit, "Edit Material").unwrap());
        assert_eq!(second.synthesis_edit().unwrap().layers(), 2);
        assert_eq!(
            restored.catalog().unwrap()[0]
                .as_synthesis()
                .unwrap()
                .layer_count(),
            2
        );
        history.undo().unwrap();
        assert_eq!(second.synthesis_edit().unwrap().layers(), 1);
        history.redo().unwrap();
        assert_eq!(second.synthesis_edit().unwrap().layers(), 2);

        let preset = restored.lab_presets(None).presets.remove(0);
        restored
            .edit_lab_preset(None, LabPresetOperation::Apply(preset))
            .unwrap();
        assert_eq!(first.synthesis_edit().unwrap().layers(), 1);
        assert_eq!(second.synthesis_edit().unwrap().layers(), 1);
        assert_eq!(history.commands().len(), 2);
        history.undo().unwrap();
        assert_eq!(first.synthesis_edit().unwrap().layers(), 2);
        assert_eq!(second.synthesis_edit().unwrap().layers(), 2);
        history.redo().unwrap();
        assert_eq!(second.synthesis_edit().unwrap().layers(), 1);
        history.undo().unwrap();

        let preset = restored.lab_presets(None).presets.remove(0);
        restored
            .edit_lab_preset(None, LabPresetOperation::Replace(preset))
            .unwrap();

        let mut native = BinaryWriter::default();
        restored.save_project(&mut native).unwrap();
        let mut reopened = Gtx375GraphDocument::<()>::load_project(
            native.bytes(),
            crate::NodeWireReadPolicy::new(64, 256),
        )
        .unwrap();
        assert_eq!(
            reopened.catalog().unwrap()[0]
                .as_synthesis()
                .unwrap()
                .layer_count(),
            2
        );
        let layers = reopened
            .branch(1)
            .flatten()
            .unwrap()
            .filter()
            .unwrap()
            .synthesis_edit()
            .unwrap()
            .layers();
        assert_eq!(layers, 2);
        let first = reopened.branch(0).flatten().unwrap().filter().unwrap();
        let mut edit = first.synthesis_edit().unwrap();
        edit.add_layer(crate::SynthesisLayerKind::Solid);
        assert!(first.edit_synthesis(edit, "Change Material").unwrap());
        let preset = reopened.lab_presets(None).presets.remove(0);
        reopened
            .edit_lab_preset(None, LabPresetOperation::Apply(preset))
            .unwrap();
        assert_eq!(first.synthesis_edit().unwrap().layers(), 2);
    }

    #[test]
    fn lab_snapshots_capture_nested_sections_and_reject_matched_unsupported_entries() {
        let outer = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let inner = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        for group in [&outer, &inner] {
            group.parameter(4).unwrap().set_index(0).unwrap();
        }
        let inside = PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        inner
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&inside.node()))
            .unwrap();
        outer
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&inner.node()))
            .unwrap();
        let outside = PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        outer
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&outside.node()))
            .unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(outer.node())]).unwrap();
        let inside = inside.parameter(0).unwrap();
        let outside = outside.parameter(0).unwrap();
        for (property, red) in [(&inside, 0.25), (&outside, 0.75)] {
            property.set_environment(Arc::new(red_map(red))).unwrap();
            doc.set_environment_adjuster(property, EnvironmentAdjusterLink::Create("Sky".into()))
                .unwrap();
        }

        doc.edit_environment_lab_preset(
            Some(&outer.node()),
            LabPresetOperation::Save("Dawn".into()),
        )
        .unwrap();
        let dawn = doc
            .environment_lab_presets(Some(&outer.node()))
            .presets
            .remove(0);
        assert!(doc.environment_lab_presets(None).presets.is_empty());
        let collection = outer.group_presets().unwrap();
        {
            let collection = collection.lock().unwrap();
            let entries = collection.groups()[0].presets();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].group_id(), inner.group_guid().unwrap());
        }
        let adjuster = doc.environment_adjuster(&inside).unwrap();
        doc.edit_environment_adjuster(&inside, &adjuster, &adjuster.value(), red_map(0.5))
            .unwrap();
        doc.edit_environment_lab_preset(
            Some(&outer.node()),
            LabPresetOperation::Apply(dawn.clone()),
        )
        .unwrap();
        assert_eq!(adjuster.value().field(EnvironmentField::Red(0)), Some(0.25));
        assert_eq!(
            outside
                .effective_environment()
                .unwrap()
                .field(EnvironmentField::Red(0)),
            Some(0.75)
        );

        let mut wire = BinaryWriter::default();
        doc.save(&mut wire).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(wire.bytes()).unwrap();
        let reopened_group = reopened.branch(0).unwrap().unwrap();
        assert_eq!(
            reopened
                .environment_lab_presets(Some(&reopened_group))
                .presets[0]
                .name(),
            "Dawn"
        );

        let mut mixed_environment = GtxPresetRecord::new(
            GtxPresetKind::Environment,
            Some("Sky".encode_utf16().collect()),
            inner.group_guid().unwrap(),
        );
        *mixed_environment.as_environment_mut().unwrap() = red_map(0.125);
        let name = Some("Vector".encode_utf16().collect());
        let vector = GtxPresetRecord::new(GtxPresetKind::Vector, name.clone(), [0; 16]);
        doc.catalog
            .stage(GtxCatalogRecord::Value(Box::new(ValueAdjusterRecord::new(
                ValueAdjusterKind::Vector,
                name,
                [0; 16],
            ))))
            .store(true, std::sync::atomic::Ordering::Release);
        // Use the ordinary wire loader to create a mixed authored fixture.
        let mut writer = BinaryWriter::default();
        writer.write_i32(1).unwrap();
        writer
            .write_utf16(&"Mixed".encode_utf16().collect::<Vec<_>>())
            .unwrap();
        writer.write_bytes(&[0; 16]).unwrap();
        writer.write_i32(2).unwrap();
        mixed_environment.save(&mut writer).unwrap();
        vector.save(&mut writer).unwrap();
        doc.presets_mut()
            .load(&mut BinaryCursor::new(writer.bytes()).unwrap(), VERSION)
            .unwrap();
        let mixed = doc.environment_lab_presets(None).presets.remove(0);
        assert!(matches!(
            doc.edit_environment_lab_preset(None, LabPresetOperation::Apply(mixed)),
            Err(GtxGraphError::LabPreset(_))
        ));
        assert_eq!(adjuster.value().field(EnvironmentField::Red(0)), Some(0.25));
    }

    #[test]
    fn catch_adjusters_scopes_group_contents_but_leaves_external_inputs_in_the_outer_scope() {
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        group.parameter(4).unwrap().set_index(0).unwrap();
        let contents = PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        let external = PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&contents.node()))
            .unwrap();
        group
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&external.node()))
            .unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(group.node())]).unwrap();
        let contents = contents.parameter(0).unwrap();
        let external = external.parameter(0).unwrap();
        for property in [&contents, &external] {
            assert!(
                doc.set_environment_adjuster(
                    property,
                    EnvironmentAdjusterLink::Create("Sky".into())
                )
                .unwrap()
            );
        }
        let inner = doc.environment_adjuster(&contents).unwrap();
        let outer = doc.environment_adjuster(&external).unwrap();
        assert_eq!(
            contents.retained_adjustment().unwrap().guid(),
            group.group_guid().unwrap()
        );
        assert_eq!(external.retained_adjustment().unwrap().guid(), [0; 16]);
        assert!(!inner.same_reference(&outer));
        assert_eq!(doc.available_environment_adjusters(&contents).len(), 1);
        assert!(matches!(
            doc.set_environment_adjuster(&external, EnvironmentAdjusterLink::Existing(inner)),
            Err(GtxGraphError::EnvironmentAdjuster(_))
        ));
        assert!(matches!(
            doc.set_environment_adjuster(
                &external,
                EnvironmentAdjusterLink::Create(" Sky ".into())
            ),
            Err(GtxGraphError::EnvironmentAdjuster(_))
        ));
        assert_eq!(doc.catalog().unwrap().len(), 2);
    }

    #[test]
    fn environment_paint_authoring_uses_the_scope_of_its_graph_owner() {
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        group.parameter(4).unwrap().set_index(0).unwrap();
        let make = || {
            let style = PointwiseFilter::new(PointwiseKind::Style).unwrap();
            let mut edit = style.style_edit().unwrap();
            edit.set_paint(0, crate::StylePaintKind::Environment)
                .unwrap();
            style.edit_style(edit, "Set Environment paint").unwrap();
            style
        };
        let inside = make();
        let outside = make();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&inside.node()))
            .unwrap();
        group
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&outside.node()))
            .unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(group.node())]).unwrap();
        let current = |node: &PointwiseFilter<()>| {
            node.style_edit()
                .unwrap()
                .environment_map(0)
                .unwrap()
                .clone()
        };
        for owner in [&inside, &outside] {
            assert!(
                doc.set_style_environment_adjuster(
                    &owner.node(),
                    None,
                    0,
                    &current(owner),
                    EnvironmentAdjusterLink::Create("Sky".into()),
                )
                .unwrap()
            );
        }
        let inner = inside.style_edit().unwrap();
        let outer = outside.style_edit().unwrap();
        assert_eq!(
            inner.environment_adjuster_key(0).unwrap().0,
            group.group_guid().unwrap()
        );
        assert_eq!(outer.environment_adjuster_key(0).unwrap().0, [0; 16]);
        let choices = doc.available_style_environment_adjusters(&inside.node());
        assert_eq!(choices.len(), 1);
        assert!(matches!(
            doc.set_style_environment_adjuster(
                &outside.node(),
                None,
                0,
                &current(&outside),
                EnvironmentAdjusterLink::Existing(choices[0].clone()),
            ),
            Err(GtxGraphError::EnvironmentAdjuster(_))
        ));
        assert!(matches!(
            doc.set_style_environment_adjuster(
                &inside.node(),
                None,
                0,
                &current(&outside),
                EnvironmentAdjusterLink::Unlink,
            ),
            Err(GtxGraphError::EnvironmentAdjuster(_))
        ));
        assert_eq!(doc.catalog().unwrap().len(), 2);
    }

    #[test]
    fn group_scope_splits_and_joins_connected_controls_with_history() {
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let nested = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        nested.parameter(4).unwrap().set_index(0).unwrap();
        let make = || PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        let (inside, outside, deep) = (make(), make(), make());
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&inside.node()))
            .unwrap();
        group
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&outside.node()))
            .unwrap();
        group
            .append_group_content([100., 0.])
            .unwrap()
            .set_node(Some(&nested.node()))
            .unwrap();
        nested
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&deep.node()))
            .unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(group.node())]).unwrap();
        let (inside, outside, deep) = (
            inside.parameter(0).unwrap(),
            outside.parameter(0).unwrap(),
            deep.parameter(0).unwrap(),
        );
        doc.set_environment_adjuster(&outside, EnvironmentAdjusterLink::Create("Sky".into()))
            .unwrap();
        let root = doc.environment_adjuster(&outside).unwrap();
        doc.set_environment_adjuster(&inside, EnvironmentAdjusterLink::Existing(root.clone()))
            .unwrap();
        doc.set_environment_adjuster(&deep, EnvironmentAdjusterLink::Create("Sky".into()))
            .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        assert!(
            doc.set_group_catch_adjusters(&group, true, Some(" "))
                .is_err()
        );
        doc.set_group_catch_adjusters(&group, true, Some("  Sky  Lab  "))
            .unwrap();
        assert_eq!(history.commands().len(), 1);
        let section = doc.lab_presets(Some(&group.node()));
        assert_eq!(section.name, "Sky Lab");
        let inner_guid = group.group_guid().unwrap();
        let scopes = || [&inside, &outside, &deep].map(|p| p.retained_adjustment().unwrap().guid());
        assert_eq!(
            scopes(),
            [inner_guid, [0; 16], nested.group_guid().unwrap()]
        );
        let inner = doc.environment_adjuster(&inside).unwrap();
        assert!(!inner.same_reference(&root));
        history.undo().unwrap();
        assert_eq!(scopes()[0], [0; 16]);
        assert!(
            group
                .parameter(2)
                .unwrap()
                .text()
                .unwrap()
                .unwrap()
                .is_empty()
        );
        history.redo().unwrap();
        assert_eq!(scopes()[0], inner_guid);
        doc.edit_environment_adjuster(&inside, &inner, &inner.value(), red_map(0.75))
            .unwrap();
        doc.edit_lab_preset(
            Some(&group.node()),
            LabPresetOperation::Save("Evening".into()),
        )
        .unwrap();
        doc.set_group_catch_adjusters(&group, false, None).unwrap();
        assert_eq!(scopes()[0], [0; 16]);
        assert!(
            doc.environment_adjuster(&inside)
                .unwrap()
                .same_reference(&root)
        );
        assert_eq!(
            inside
                .effective_environment()
                .unwrap()
                .field(crate::EnvironmentField::Red(0)),
            root.value().field(crate::EnvironmentField::Red(0))
        );
        assert_eq!(doc.catalog().unwrap().len(), 2);
        history.undo().unwrap();
        assert_eq!(
            inside
                .effective_environment()
                .unwrap()
                .field(crate::EnvironmentField::Red(0)),
            Some(0.75)
        );
        assert_eq!(doc.lab_presets(Some(&group.node())).presets.len(), 1);
        history.redo().unwrap();
        let mut writer = BinaryWriter::default();
        doc.save(&mut writer).unwrap();
        let opened = Gtx375GraphDocument::<()>::load(writer.bytes()).unwrap();
        assert_eq!(opened.catalog().unwrap().len(), 2);
        let group = opened.branch(0).unwrap().unwrap();
        let contents = group.content(0).unwrap().node().unwrap().filter().unwrap();
        let out = group.slot(0).unwrap().node().unwrap().filter().unwrap();
        let control = |filter: PointwiseFilter<()>| {
            opened
                .environment_adjuster(&filter.parameter(0).unwrap())
                .unwrap()
        };
        assert!(control(contents).same_reference(&control(out)));
    }

    #[test]
    fn catch_adjusters_splits_and_rejoins_whole_environment_paint_links() {
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let root = group.node();
        let outside = PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        let style = environment_paint_node(PointwiseKind::Style, 0.5);
        let canvas = environment_paint_node(PointwiseKind::Canvas, 0.75);
        let parameter = outside.parameter(0).unwrap();
        parameter.set_environment(Arc::new(red_map(0.25))).unwrap();
        root.slot(0)
            .unwrap()
            .set_node(Some(&outside.node()))
            .unwrap();
        root.content(0)
            .unwrap()
            .set_node(Some(&style.node()))
            .unwrap();
        group
            .append_group_content([120., 0.])
            .unwrap()
            .set_node(Some(&canvas.node()))
            .unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(root.clone())]).unwrap();
        doc.set_environment_adjuster(&parameter, EnvironmentAdjusterLink::Create("Sky".into()))
            .unwrap();
        let outer = doc.environment_adjuster(&parameter).unwrap();
        for (owner, pass) in [(&style, 0), (&canvas, 0), (&canvas, 1)] {
            link_environment_paint(
                &mut doc,
                owner,
                pass,
                EnvironmentAdjusterLink::Existing(outer.clone()),
            );
        }
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);

        doc.set_group_catch_adjusters(&group, true, Some("Sky Lab"))
            .unwrap();
        let scope = group.group_guid().unwrap();
        let inner = paint_adjuster(&doc, &style, scope, 0.25);
        assert!(!inner.same_reference(&outer));
        assert!(paint_adjuster(&doc, &canvas, scope, 0.25).same_reference(&inner));
        assert_eq!(history.cursor(), 1);
        doc.edit_environment_adjuster_value(&inner, &inner.value(), red_map(0.75))
            .unwrap();
        paint_adjuster(&doc, &canvas, scope, 0.75);
        assert_eq!(
            parameter
                .effective_environment()
                .unwrap()
                .field(EnvironmentField::Red(0)),
            Some(0.25)
        );
        doc.edit_lab_preset(Some(&root), LabPresetOperation::Save("Evening".into()))
            .unwrap();

        doc.set_group_catch_adjusters(&group, false, None).unwrap();
        for owner in [&style, &canvas] {
            assert!(paint_adjuster(&doc, owner, [0; 16], 0.25).same_reference(&outer));
        }
        assert_eq!(doc.catalog().unwrap().len(), 1);
        assert!(doc.lab_presets(Some(&root)).presets.is_empty());
        history.undo().unwrap();
        assert!(paint_adjuster(&doc, &canvas, scope, 0.75).same_reference(&inner));
        assert_eq!(doc.lab_presets(Some(&root)).presets.len(), 1);
        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        let reopened = Gtx375GraphDocument::load_project(
            native.bytes(),
            crate::NodeWireReadPolicy::new(128, 4096),
        )
        .unwrap();
        let reopened_group = reopened.branch(0).flatten().unwrap();
        let reopened_canvas = reopened_group
            .content(1)
            .unwrap()
            .node()
            .unwrap()
            .filter()
            .unwrap();
        paint_adjuster(&reopened, &reopened_canvas, scope, 0.75);
        history.redo().unwrap();
        paint_adjuster(&doc, &canvas, [0; 16], 0.25);
    }

    #[test]
    fn nested_environment_and_graph_references_follow_lab_scope_and_history() {
        use crate::{
            EnvironmentColorField as Color, EnvironmentNumericReceiver as Numeric,
            EnvironmentTransformFilter as Filter, SharedGradient,
        };
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let root = group.node();
        let outside = PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        let style = environment_paint_node(PointwiseKind::Style, 0.5);
        let canvas = environment_paint_node(PointwiseKind::Canvas, 0.75);
        let local = PointwiseFilter::new(PointwiseKind::MakeEnvironment).unwrap();
        let solid = PointwiseNode::solid_color().unwrap();
        let colorize = PointwiseFilter::new(PointwiseKind::Colorize).unwrap();
        let graph_ramp = |filter: &PointwiseFilter<()>| {
            filter
                .parameters()
                .into_iter()
                .find(|property| {
                    matches!(
                        property.raw_value(),
                        crate::PointwiseParameterValue::Gradient(_)
                    )
                })
                .unwrap()
        };
        let graph_gradient = graph_ramp(&colorize);
        let adjustment = graph_gradient.adjustment().unwrap().unwrap();
        adjustment
            .names()
            .set_entry(0, Some("Ramp".encode_utf16().collect()))
            .unwrap();
        adjustment.flags().set(0, true).unwrap();
        let solid_color = solid.solid_model().unwrap().color_property();
        let wire = solid_color.adjustment().unwrap().unwrap();
        wire.names()
            .set_entry(0, Some("Fill".encode_utf16().collect()))
            .unwrap();
        wire.flags().set(0, true).unwrap();
        solid.set_solid_color([0.25, 0., 0.]).unwrap();

        let outer_property = outside.parameter(0).unwrap();
        let local_property = local.parameter(0).unwrap();
        let mut map = red_map(0.25);
        map.set_recolor_enabled(0, true).unwrap();
        map.set_transform_filter(None, Filter::Diffuse).unwrap();
        map.set_exposure(1.);
        outer_property.set_environment(Arc::new(map)).unwrap();
        local_property
            .set_environment(Arc::new(red_map(0.5)))
            .unwrap();
        root.slot(0)
            .unwrap()
            .set_node(Some(&outside.node()))
            .unwrap();
        root.content(0)
            .unwrap()
            .set_node(Some(&style.node()))
            .unwrap();
        group
            .append_group_content([120., 0.])
            .unwrap()
            .set_node(Some(&local.node()))
            .unwrap();
        group
            .append_group_content([240., 0.])
            .unwrap()
            .set_node(Some(&solid))
            .unwrap();
        let canvas_slot = group.append_group_content([360., 0.]).unwrap();
        group
            .append_group_content([480., 0.])
            .unwrap()
            .set_node(Some(&colorize.node()))
            .unwrap();

        let mut doc =
            Gtx375GraphDocument::new(vec![Some(root.clone()), Some(canvas.node())]).unwrap();
        doc.set_environment_adjuster(
            &outer_property,
            EnvironmentAdjusterLink::Create("Sky".into()),
        )
        .unwrap();
        let sky = doc.environment_adjuster(&outer_property).unwrap();
        link_environment_paint(
            &mut doc,
            &style,
            0,
            EnvironmentAdjusterLink::Existing(sky.clone()),
        );
        let sky_owner = || EnvironmentMapTarget::Property(&outer_property);
        for (field, name) in [(Color::LayerColor(0), "Fill"), (Color::Recolor(0), "Ramp")] {
            doc.set_environment_color_adjuster(
                sky_owner(),
                &sky.value(),
                field,
                EnvironmentColorLink::Create(name.into()),
            )
            .unwrap();
        }
        let exposure = Numeric::Field(EnvironmentField::Exposure);
        let filter = Numeric::Filter(None);
        for (field, name) in [(exposure, "Exposure"), (filter, "Filter")] {
            doc.set_environment_numeric_adjuster(
                sky_owner(),
                &sky.value(),
                field,
                LabNumericLink::Create(name.into()),
            )
            .unwrap();
        }
        let exposure_control = doc
            .environment_numeric_adjuster(&sky.value(), exposure)
            .unwrap();
        doc.set_environment_numeric_adjuster(
            EnvironmentMapTarget::Property(&local_property),
            &local_property.effective_environment().unwrap(),
            exposure,
            LabNumericLink::Existing(exposure_control),
        )
        .unwrap();
        let canvas_node = canvas.node();
        let layers = canvas.canvas_layer_editors();
        let canvas_map = || {
            environment_style_edit(&canvas)
                .environment_map(0)
                .unwrap()
                .clone()
        };
        let fill = doc
            .environment_color_adjuster(&sky.value(), Color::LayerColor(0))
            .unwrap();
        doc.set_environment_color_adjuster(
            EnvironmentMapTarget::Paint {
                owner: &canvas_node,
                layer: layers.first(),
                pass: 0,
            },
            &canvas_map(),
            Color::Colorize(0),
            EnvironmentColorLink::Existing(fill),
        )
        .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);

        doc.set_group_catch_adjusters(&group, true, Some("Inner Lab"))
            .unwrap();
        let scope = group.group_guid().unwrap();
        let check_sky_scope = |map: &EnvironmentRecord, scope| {
            for field in [Color::LayerColor(0), Color::Recolor(0)] {
                assert_eq!(map.color_adjuster_key(field).unwrap().0, scope);
            }
            for field in [exposure, filter] {
                assert_eq!(map.numeric_adjuster_key(field).unwrap().0, scope);
            }
        };
        check_sky_scope(&sky.value(), [0; 16]);
        let inner = paint_adjuster(&doc, &style, scope, 0.25);
        check_sky_scope(&inner.value(), scope);
        assert_eq!(
            local_property
                .effective_environment()
                .unwrap()
                .numeric_adjuster_key(exposure)
                .unwrap()
                .0,
            scope
        );
        assert_eq!(solid_color.adjustment().unwrap().unwrap().guid(), scope);
        assert_eq!(history.cursor(), 1);
        let inner_fill = doc
            .environment_color_adjuster(&inner.value(), Color::LayerColor(0))
            .unwrap();
        doc.edit_color_lab_adjuster(
            Some(&root),
            &inner_fill,
            &inner_fill.draft_value().unwrap(),
            LabColorValue::Color([0.75, 0., 0.]),
        )
        .unwrap();
        let inner_exposure = doc
            .environment_numeric_adjuster(&inner.value(), exposure)
            .unwrap();
        doc.edit_numeric_lab_adjuster(Some(&root), &inner_exposure, LabNumericValue::Number(100.))
            .unwrap();
        let ramp = doc
            .environment_color_adjuster(&inner.value(), Color::Recolor(0))
            .unwrap();
        let gradient = SharedGradient::new();
        for position in [0., 1.] {
            gradient
                .add_normalized_color(position, 0.25, 0.5, 0.75)
                .unwrap();
            gradient.add_normalized_alpha(position, 0.5).unwrap();
        }
        doc.edit_color_lab_adjuster(
            Some(&root),
            &ramp,
            &ramp.draft_value().unwrap(),
            LabColorValue::Gradient(gradient.clone()),
        )
        .unwrap();
        assert_eq!(solid.solid_color_values().unwrap()[0], 0.75);
        assert_eq!(graph_gradient.retained_adjustment().unwrap().guid(), scope);
        assert!(
            graph_gradient
                .gradient()
                .unwrap()
                .source_equals(Some(&gradient))
                .unwrap()
        );
        assert_eq!(
            local_property.effective_environment().unwrap().exposure(),
            2.
        );
        assert_eq!(sky.value().exposure(), 1.);
        assert_eq!(canvas_map().field(EnvironmentField::Tint(0, 0)), Some(0.25));
        doc.edit_lab_preset(Some(&root), LabPresetOperation::Save("Evening".into()))
            .unwrap();

        doc.edit_slot(&canvas_slot, Some(&canvas_node)).unwrap();
        assert_eq!(
            canvas_map()
                .color_adjuster_key(Color::Colorize(0))
                .unwrap()
                .0,
            scope
        );
        assert_eq!(canvas_map().field(EnvironmentField::Tint(0, 0)), Some(0.75));
        history.undo().unwrap();
        assert_eq!(
            canvas_map()
                .color_adjuster_key(Color::Colorize(0))
                .unwrap()
                .0,
            [0; 16]
        );
        history.redo().unwrap();
        assert_eq!(canvas_map().field(EnvironmentField::Tint(0, 0)), Some(0.75));

        doc.set_group_catch_adjusters(&group, false, None).unwrap();
        assert!(paint_adjuster(&doc, &style, [0; 16], 0.25).same_reference(&sky));
        assert_eq!(solid.solid_color_values().unwrap()[0], 0.25);
        assert_eq!(canvas_map().field(EnvironmentField::Tint(0, 0)), Some(0.25));
        assert_eq!(
            local_property.effective_environment().unwrap().exposure(),
            1.
        );
        assert_eq!(doc.catalog().unwrap().len(), 5);
        history.undo().unwrap();
        check_sky_scope(&inner.value(), scope);
        assert_eq!(
            local_property.effective_environment().unwrap().exposure(),
            2.
        );
        assert_eq!(doc.lab_presets(Some(&root)).presets.len(), 1);

        let check = |doc: &Gtx375GraphDocument<()>| {
            let root = doc.branch(0).flatten().unwrap();
            let style = root.content(0).unwrap().node().unwrap().filter().unwrap();
            let inner = paint_adjuster(doc, &style, scope, 0.75);
            check_sky_scope(&inner.value(), scope);
            assert!(
                inner
                    .value()
                    .copy_recolor_gradient(0)
                    .unwrap()
                    .unwrap()
                    .source_equals(Some(&gradient))
                    .unwrap()
            );
            let reopened_solid = root.content(2).unwrap().node().unwrap();
            assert_eq!(reopened_solid.solid_color_values().unwrap()[0], 0.75);
            let reopened_colorize = root.content(4).unwrap().node().unwrap().filter().unwrap();
            assert!(
                graph_ramp(&reopened_colorize)
                    .gradient()
                    .unwrap()
                    .source_equals(Some(&gradient))
                    .unwrap()
            );
        };
        let mut gtx = BinaryWriter::default();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        check(&Gtx375GraphDocument::load(gtx.bytes()).unwrap());
        let mut native = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        check(
            &Gtx375GraphDocument::load_project(
                native.bytes(),
                crate::NodeWireReadPolicy::new(128, 4096),
            )
            .unwrap(),
        );
        history.redo().unwrap();
        assert_eq!(doc.catalog().unwrap().len(), 5);
    }

    #[test]
    fn moving_and_removing_environment_paints_reconciles_the_lab_catalog() {
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let root = group.node();
        let inner = environment_paint_node(PointwiseKind::Style, 0.75);
        let canvas = environment_paint_node(PointwiseKind::Canvas, 0.25);
        root.content(0)
            .unwrap()
            .set_node(Some(&inner.node()))
            .unwrap();
        let destination = group.append_group_content([120., 0.]).unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(root.clone()), Some(canvas.node())]).unwrap();
        doc.set_group_catch_adjusters(&group, true, Some("Sky Lab"))
            .unwrap();
        let scope = group.group_guid().unwrap();
        link_environment_paint(
            &mut doc,
            &inner,
            0,
            EnvironmentAdjusterLink::Create("Sky".into()),
        );
        link_environment_paint(
            &mut doc,
            &canvas,
            0,
            EnvironmentAdjusterLink::Create("Sky".into()),
        );
        let outer = doc
            .style_environment_adjuster(&environment_style_edit(&canvas), 0)
            .unwrap();
        link_environment_paint(
            &mut doc,
            &canvas,
            1,
            EnvironmentAdjusterLink::Existing(outer.clone()),
        );
        let group_map = paint_adjuster(&doc, &inner, scope, 0.75);
        doc.edit_lab_preset(None, LabPresetOperation::Save("Before moving".into()))
            .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);

        let preview = || {
            let rendered = canvas.node().render_original(32, 32).unwrap();
            assert!(rendered.caught_errors().is_empty());
            rendered.image().unwrap().channel(0, true).unwrap().values()
        };
        let outside_pixels = preview();
        doc.edit_slot(&destination, Some(&canvas.node())).unwrap();
        assert!(doc.branch(1).unwrap().is_none());
        assert!(destination.node().unwrap().same_reference(&canvas.node()));
        assert!(paint_adjuster(&doc, &canvas, scope, 0.75).same_reference(&group_map));
        let inside_pixels = preview();
        assert!(inside_pixels != outside_pixels);
        assert_eq!(doc.catalog().unwrap().len(), 1);
        assert_eq!(history.cursor(), 1);
        history.undo().unwrap();
        assert!(destination.node().is_none());
        assert!(paint_adjuster(&doc, &canvas, [0; 16], 0.25).same_reference(&outer));
        assert_eq!(preview(), outside_pixels);
        history.redo().unwrap();
        assert_eq!(preview(), inside_pixels);
        doc.edit_branch(1, Some(canvas.node())).unwrap();
        assert!(destination.node().is_none());
        let detached = paint_adjuster(&doc, &canvas, [0; 16], 0.75);
        assert!(!detached.same_reference(&group_map));
        assert_eq!(doc.catalog().unwrap().len(), 2);
        doc.edit_branch(1, None).unwrap();
        assert_eq!(doc.catalog().unwrap().len(), 1);
        assert!(paint_adjuster(&doc, &inner, scope, 0.75).same_reference(&group_map));
        history.undo().unwrap();
        assert!(paint_adjuster(&doc, &canvas, [0; 16], 0.75).same_reference(&detached));
        let mut gtx = BinaryWriter::default();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(gtx.bytes()).unwrap();
        let reopened_canvas = reopened.branch(1).flatten().unwrap().filter().unwrap();
        paint_adjuster(&reopened, &reopened_canvas, [0; 16], 0.75);
        history.redo().unwrap();
        assert_eq!(doc.catalog().unwrap().len(), 1);
    }

    #[test]
    fn group_scope_refresh_preserves_saved_values_and_captures_new_keys() {
        use crate::{LabNumericAxis, LabNumericLink, LabNumericProperty};
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let solid = PointwiseNode::solid_color().unwrap();
        let property = LabNumericProperty::transparency(&solid).unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&solid))
            .unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(group.node())]).unwrap();
        doc.set_numeric_property_link(
            &property,
            LabNumericAxis::Whole,
            LabNumericLink::Create("Opacity".into()),
        )
        .unwrap();
        let edit = |doc: &Gtx375GraphDocument<()>, selected, number| {
            let adjuster = doc.numeric_lab_adjusters(selected).remove(0);
            doc.edit_numeric_lab_adjuster(selected, &adjuster, LabNumericValue::Number(number))
                .unwrap();
        };
        edit(&doc, None, 20.);
        doc.edit_lab_preset(None, LabPresetOperation::Save("Saved".into()))
            .unwrap();
        edit(&doc, None, 65.);
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let records = |doc: &Gtx375GraphDocument<()>| {
            doc.presets()
                .groups()
                .iter()
                .find(|group| !group.is_hidden())
                .unwrap()
                .presets()
                .iter()
                .map(|record| {
                    let GtxPresetValue::Number(value) = record.value() else {
                        panic!("expected Number")
                    };
                    (record.group_id(), *value, record.is_hidden())
                })
                .collect::<Vec<_>>()
        };
        let current = |doc: &Gtx375GraphDocument<()>, selected| {
            doc.numeric_lab_adjusters(selected)[0].value()
        };
        let group_node = group.node();
        let group_id = group.group_guid().unwrap();
        doc.set_group_catch_adjusters(&group, true, Some("Surfaces"))
            .unwrap();
        assert_eq!(
            records(&doc),
            [([0; 16], 20., true), (group_id, 65., false)]
        );
        history.undo().unwrap();
        assert_eq!(records(&doc), [([0; 16], 20., false)]);
        history.redo().unwrap();
        assert_eq!(
            records(&doc),
            [([0; 16], 20., true), (group_id, 65., false)]
        );
        let mut writer = BinaryWriter::default();
        doc.save(&mut writer).unwrap();
        let loaded = Gtx375GraphDocument::<()>::load(writer.bytes()).unwrap();
        assert_eq!(records(&loaded), [(group_id, 65., false)]);
        edit(&doc, Some(&group_node), 85.);
        let apply = |doc: &mut Gtx375GraphDocument<()>| {
            let preset = doc.lab_presets(None).presets.remove(0);
            doc.edit_lab_preset(None, LabPresetOperation::Apply(preset))
                .unwrap();
        };
        apply(&mut doc);
        assert_eq!(
            current(&doc, Some(&group_node)),
            LabNumericValue::Number(65.)
        );
        doc.set_group_catch_adjusters(&group, false, None).unwrap();
        assert_eq!(
            records(&doc),
            [([0; 16], 20., false), (group_id, 65., true)]
        );
        apply(&mut doc);
        assert_eq!(current(&doc, None), LabNumericValue::Number(20.));
    }

    #[test]
    fn group_scope_preset_refresh_keeps_earlier_sibling_values_dormant() {
        use crate::{LabNumericAxis, LabNumericLink, LabNumericProperty};
        let first = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let second = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(first.node()), Some(second.node())]).unwrap();
        doc.set_group_catch_adjusters(&second, true, Some("Second"))
            .unwrap();
        for (group, name, value) in [(&first, "First", 25.), (&second, "Second", 70.)] {
            let solid = PointwiseNode::solid_color().unwrap();
            group
                .node()
                .content(0)
                .unwrap()
                .set_node(Some(&solid))
                .unwrap();
            let property = LabNumericProperty::transparency(&solid).unwrap();
            doc.set_numeric_property_link(
                &property,
                LabNumericAxis::Whole,
                LabNumericLink::Create(name.into()),
            )
            .unwrap();
            let selected = Some(group.node());
            let control = doc.numeric_lab_adjusters(selected.as_ref()).remove(0);
            doc.edit_numeric_lab_adjuster(
                selected.as_ref(),
                &control,
                LabNumericValue::Number(value),
            )
            .unwrap();
        }
        doc.edit_lab_preset(None, LabPresetOperation::Save("Saved".into()))
            .unwrap();
        let first_guid = first.group_guid().unwrap();
        let second_guid = second.group_guid().unwrap();
        let record = |doc: &Gtx375GraphDocument<()>, id| {
            let record = doc
                .presets()
                .groups()
                .iter()
                .find(|group| !group.is_hidden())
                .unwrap()
                .presets()
                .iter()
                .find(|record| record.group_id() == id)
                .unwrap();
            let GtxPresetValue::Number(value) = record.value() else {
                panic!("expected Number")
            };
            (*value, record.is_hidden())
        };
        doc.set_group_catch_adjusters(&first, true, Some("First"))
            .unwrap();
        assert_eq!(record(&doc, first_guid), (25., true));
        assert_eq!(record(&doc, second_guid), (70., false));
        let first_node = first.node();
        let control = doc.numeric_lab_adjusters(Some(&first_node)).remove(0);
        doc.edit_numeric_lab_adjuster(Some(&first_node), &control, LabNumericValue::Number(40.))
            .unwrap();
        doc.set_group_catch_adjusters(&second, false, None).unwrap();
        assert_eq!(record(&doc, first_guid), (25., false));
        assert_eq!(record(&doc, second_guid), (70., true));
    }

    #[test]
    fn reference_copy_reconnects_group_lab_links_and_travels_with_the_graph() {
        use crate::{LabNumericAxis, LabNumericLink, LabNumericProperty};
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let add = PointwiseFilter::new(PointwiseKind::Add).unwrap().node();
        let producer = PointwiseNode::solid_color().unwrap();
        let outer = PointwiseNode::solid_color().unwrap();
        let destination = add.slot(1).unwrap();
        add.slot(0).unwrap().set_node(Some(&producer)).unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&add))
            .unwrap();
        let group_node = group.node();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(group_node.clone()), Some(outer.clone())]).unwrap();
        doc.set_group_catch_adjusters(&group, true, Some("Surfaces"))
            .unwrap();
        for (node, section, value) in [(&producer, Some(&group_node), 30.), (&outer, None, 75.)] {
            let property = LabNumericProperty::transparency(node).unwrap();
            doc.set_numeric_property_link(
                &property,
                LabNumericAxis::Whole,
                LabNumericLink::Create("Opacity".into()),
            )
            .unwrap();
            let adjuster = doc.numeric_lab_adjusters(section).remove(0);
            doc.edit_numeric_lab_adjuster(section, &adjuster, LabNumericValue::Number(value))
                .unwrap();
        }
        doc.edit_lab_preset(Some(&group_node), LabPresetOperation::Save("Inner".into()))
            .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.reference_copy_output(&producer, &destination).unwrap();
        let actual = group_node.slot(0).unwrap().node().unwrap();
        let adjustment = |node: &PointwiseNode<()>| {
            node.solid_model()
                .unwrap()
                .transparency_property()
                .adjustment()
                .unwrap()
                .unwrap()
        };
        assert_eq!(adjustment(&actual).guid(), [0; 16]);
        assert_eq!(
            actual.solid_transparency().unwrap(),
            outer.solid_transparency().unwrap()
        );
        assert!(doc.numeric_lab_adjusters(Some(&group_node)).is_empty());
        assert!(doc.lab_presets(Some(&group_node)).presets.is_empty());
        assert_eq!(
            destination.node().unwrap().kind(),
            Some(PointwiseKind::Reference)
        );
        assert_eq!(history.commands().len(), 1);
        history.undo().unwrap();
        assert!(group_node.slot(0).unwrap().node().is_none());
        assert!(
            add.slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&producer)
        );
        assert_eq!(adjustment(&producer).guid(), group.group_guid().unwrap());
        assert_eq!(
            doc.numeric_lab_adjusters(Some(&group_node))[0].value(),
            LabNumericValue::Number(30.)
        );
        assert_eq!(doc.lab_presets(Some(&group_node)).presets.len(), 1);
        history.redo().unwrap();
        assert!(
            group_node
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&actual)
        );
        assert_eq!(
            actual.solid_transparency().unwrap(),
            outer.solid_transparency().unwrap()
        );
        let mut file = BinaryWriter::default();
        doc.save(&mut file).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(file.bytes()).unwrap();
        let reopened_group = reopened.branch(0).unwrap().unwrap();
        assert!(
            reopened
                .numeric_lab_adjusters(Some(&reopened_group))
                .is_empty()
        );
        let reopened_actual = reopened_group.slot(0).unwrap().node().unwrap();
        assert_eq!(adjustment(&reopened_actual).guid(), [0; 16]);
        assert_eq!(
            reopened_actual.solid_transparency().unwrap(),
            outer.solid_transparency().unwrap()
        );
    }

    #[test]
    fn reference_copy_of_an_existing_reference_retires_the_replaced_link() {
        use crate::{LabNumericAxis, LabNumericLink, LabNumericProperty};
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let add = PointwiseFilter::new(PointwiseKind::Add).unwrap().node();
        let reference = PointwiseFilter::new(PointwiseKind::Reference).unwrap();
        reference.parameter(0).unwrap().set_number(1.).unwrap();
        let reference = reference.node();
        let replaced = PointwiseNode::solid_color().unwrap();
        let destination = add.slot(1).unwrap();
        add.slot(0).unwrap().set_node(Some(&reference)).unwrap();
        destination.set_node(Some(&replaced)).unwrap();
        let group_node = group.node();
        group_node.content(0).unwrap().set_node(Some(&add)).unwrap();
        group_node
            .slot(0)
            .unwrap()
            .set_node(Some(&PointwiseNode::solid_color().unwrap()))
            .unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(group_node.clone())]).unwrap();
        doc.set_group_catch_adjusters(&group, true, Some("Surfaces"))
            .unwrap();
        let property = LabNumericProperty::transparency(&replaced).unwrap();
        doc.set_numeric_property_link(
            &property,
            LabNumericAxis::Whole,
            LabNumericLink::Create("Opacity".into()),
        )
        .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.reference_copy_output(&reference, &destination).unwrap();
        assert_eq!(
            destination.node().unwrap().kind(),
            Some(PointwiseKind::Reference)
        );
        assert!(doc.numeric_lab_adjusters(Some(&group_node)).is_empty());
        assert_eq!(history.commands().len(), 1);
        history.undo().unwrap();
        assert!(destination.node().unwrap().same_reference(&replaced));
        assert_eq!(
            doc.numeric_lab_adjusters(Some(&group_node))[0].name(),
            "Opacity"
        );
        history.redo().unwrap();
        assert!(doc.numeric_lab_adjusters(Some(&group_node)).is_empty());
    }

    #[test]
    fn editor_move_reconciles_lab_between_root_and_group_and_keeps_one_history_edit() {
        use crate::{LabNumericAxis, LabNumericLink, LabNumericProperty};
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let add = PointwiseFilter::new(PointwiseKind::Add).unwrap().node();
        let moved = PointwiseNode::solid_color().unwrap();
        let remaining = PointwiseNode::solid_color().unwrap();
        let destination = add.slot(0).unwrap();
        add.slot(1).unwrap().set_node(Some(&remaining)).unwrap();
        let group_node = group.node();
        group_node.content(0).unwrap().set_node(Some(&add)).unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(group_node.clone()), Some(moved.clone())]).unwrap();
        doc.set_group_catch_adjusters(&group, true, Some("Group Lab"))
            .unwrap();
        let adjust = |doc: &mut Gtx375GraphDocument<()>, node, section, value, name| {
            let property = LabNumericProperty::transparency(node).unwrap();
            doc.set_numeric_property_link(
                &property,
                LabNumericAxis::Whole,
                LabNumericLink::Create(name),
            )
            .unwrap();
            let control = doc.numeric_lab_adjusters(section).remove(0);
            doc.edit_numeric_lab_adjuster(section, &control, LabNumericValue::Number(value))
                .unwrap();
        };
        adjust(
            &mut doc,
            &remaining,
            Some(&group_node),
            85.,
            "Opacity".into(),
        );
        adjust(&mut doc, &moved, None, 25., "Opacity".into());
        doc.edit_lab_preset(None, LabPresetOperation::Save("Before move".into()))
            .unwrap();
        let original_value = moved.solid_transparency().unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let group_guid = group.group_guid().unwrap();
        let guid = || {
            moved
                .solid_model()
                .unwrap()
                .transparency_property()
                .adjustment()
                .unwrap()
                .unwrap()
                .guid()
        };
        doc.edit_slot(&destination, Some(&moved)).unwrap();
        assert!(doc.branch(1).unwrap().is_none());
        assert!(destination.node().unwrap().same_reference(&moved));
        assert_eq!(guid(), group_guid);
        assert_eq!(
            moved.solid_transparency().unwrap(),
            remaining.solid_transparency().unwrap()
        );
        assert!(doc.numeric_lab_adjusters(None).is_empty());
        assert_eq!(history.commands().len(), 1);
        history.undo().unwrap();
        assert_eq!(guid(), [0; 16]);
        assert!(destination.node().is_none());
        assert!(doc.branch(1).unwrap().unwrap().same_reference(&moved));
        assert_eq!(moved.solid_transparency().unwrap(), original_value);
        history.redo().unwrap();
        assert_eq!(guid(), group_guid);
        doc.edit_branch(1, Some(moved.clone())).unwrap();
        assert!(destination.node().is_none());
        assert_eq!(guid(), [0; 16]);
        assert_eq!(
            doc.numeric_lab_adjusters(None)[0].value(),
            LabNumericValue::Number(85.)
        );
        assert_eq!(
            doc.numeric_lab_adjusters(Some(&group_node))[0].value(),
            LabNumericValue::Number(85.)
        );
        let mut writer = BinaryWriter::default();
        doc.save(&mut writer).unwrap();
        let opened = Gtx375GraphDocument::<()>::load(writer.bytes()).unwrap();
        let root = opened.branch(1).unwrap().unwrap();
        assert_eq!(
            root.solid_transparency().unwrap(),
            remaining.solid_transparency().unwrap()
        );
        history.undo().unwrap();
        assert_eq!(guid(), group_guid);
        history.redo().unwrap();
        assert_eq!(guid(), [0; 16]);
    }

    #[test]
    fn editor_move_reconnects_synthesis_to_the_destination_material_and_restores_history() {
        use crate::{LabNumericAxis, LabNumericLink, LabNumericProperty};

        let synthesis = |layers| {
            let filter = PointwiseFilter::<()>::new(PointwiseKind::Synthesis).unwrap();
            let mut draft = filter.synthesis_edit().unwrap();
            for _ in 0..layers {
                draft.add_layer(crate::SynthesisLayerKind::Solid);
            }
            filter.edit_synthesis(draft, "Set test layers").unwrap();
            filter
        };
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        let group_node = group.node();
        let add = PointwiseFilter::new(PointwiseKind::Add).unwrap().node();
        let destination = add.slot(0).unwrap();
        let moved = synthesis(1);
        let inside = synthesis(2);
        let moving_input = PointwiseNode::solid_color().unwrap();
        let inner_input = PointwiseNode::solid_color().unwrap();
        moved
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&moving_input))
            .unwrap();
        inside
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&inner_input))
            .unwrap();
        moved.parameter(0).unwrap().set_number(0.75).unwrap();
        moved.parameter(1).unwrap().set_number(7.).unwrap();
        add.slot(1).unwrap().set_node(Some(&inside.node())).unwrap();
        group_node.content(0).unwrap().set_node(Some(&add)).unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(group_node.clone()), Some(moved.node())]).unwrap();
        doc.set_group_catch_adjusters(&group, true, Some("Surfaces"))
            .unwrap();
        for filter in [&moved, &inside] {
            doc.set_synthesis_adjuster(filter, SynthesisAdjusterLink::Create("Material".into()))
                .unwrap();
        }
        for (node, section, amount) in [
            (&moving_input, None, 25.),
            (&inner_input, Some(&group_node), 75.),
        ] {
            let property = LabNumericProperty::transparency(node).unwrap();
            doc.set_numeric_property_link(
                &property,
                LabNumericAxis::Whole,
                LabNumericLink::Create("Opacity".into()),
            )
            .unwrap();
            let adjuster = doc.numeric_lab_adjusters(section).remove(0);
            doc.edit_numeric_lab_adjuster(section, &adjuster, LabNumericValue::Number(amount))
                .unwrap();
        }
        let outside_material = doc.synthesis_adjuster(&moved).unwrap();
        let inside_material = doc.synthesis_adjuster(&inside).unwrap();
        let outside_opacity = moving_input.solid_transparency().unwrap();
        let inside_opacity = inner_input.solid_transparency().unwrap();
        assert_ne!(outside_opacity, inside_opacity);
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let layers = || moved.synthesis_edit().unwrap().layers();
        let scope = || {
            moved
                .synthesis_wire()
                .unwrap()
                .retained_adjustment()
                .unwrap()
                .guid()
        };

        doc.edit_slot(&destination, Some(&moved.node())).unwrap();
        assert!(doc.branch(1).unwrap().is_none());
        assert!(destination.node().unwrap().same_reference(&moved.node()));
        assert!(
            doc.synthesis_adjuster(&moved)
                .unwrap()
                .same_reference(&inside_material)
        );
        assert_eq!(layers(), 2);
        assert_eq!(moving_input.solid_transparency().unwrap(), inside_opacity);
        assert_eq!(scope(), group.group_guid().unwrap());
        assert!(doc.lab_adjuster_metadata(None).is_empty());
        assert_eq!(history.commands().len(), 1);
        history.undo().unwrap();
        assert!(destination.node().is_none());
        assert!(
            doc.branch(1)
                .flatten()
                .unwrap()
                .same_reference(&moved.node())
        );
        assert!(
            doc.synthesis_adjuster(&moved)
                .unwrap()
                .same_reference(&outside_material)
        );
        assert_eq!(layers(), 1);
        assert_eq!(moving_input.solid_transparency().unwrap(), outside_opacity);
        assert_eq!(scope(), [0; 16]);
        history.redo().unwrap();

        doc.edit_branch(1, Some(moved.node())).unwrap();
        assert!(destination.node().is_none());
        assert_eq!(scope(), [0; 16]);
        assert_eq!(layers(), 2);
        let copied = doc.synthesis_adjuster(&moved).unwrap();
        assert!(!copied.same_reference(&inside_material));
        assert!(!copied.same_reference(&outside_material));
        assert_eq!(moved.parameter(0).unwrap().number(), 0.75);
        assert_eq!(moved.parameter(1).unwrap().number(), 7.);
        let mut native = BinaryWriter::default();
        let mut gtx = BinaryWriter::default();
        doc.save_project(&mut native).unwrap();
        doc.save_compatible_gtx(&mut gtx).unwrap();
        let native = Gtx375GraphDocument::<()>::load_project(
            native.bytes(),
            crate::NodeWireReadPolicy::new(64, 256),
        )
        .unwrap();
        let gtx = Gtx375GraphDocument::<()>::load(gtx.bytes()).unwrap();
        for opened in [native, gtx] {
            let root = opened.branch(1).flatten().unwrap().filter().unwrap();
            assert_eq!(root.synthesis_edit().unwrap().layers(), 2);
            assert_eq!(root.parameter(1).unwrap().number(), 7.);
            assert_eq!(
                opened.synthesis_adjuster(&root).unwrap().name(),
                copied.name()
            );
            assert_eq!(opened.catalog().unwrap().len(), 4);
        }
        history.undo().unwrap();
        assert!(
            doc.synthesis_adjuster(&moved)
                .unwrap()
                .same_reference(&inside_material)
        );
        history.redo().unwrap();
        assert!(
            doc.synthesis_adjuster(&moved)
                .unwrap()
                .same_reference(&copied)
        );
    }

    #[test]
    fn catching_group_splits_curve_links_and_release_restores_their_preset_on_undo() {
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let root = group.node();
        let inner = PointwiseFilter::new(PointwiseKind::ToneCurve).unwrap();
        let sibling = PointwiseFilter::new(PointwiseKind::ToneCurve).unwrap();
        let external = PointwiseFilter::new(PointwiseKind::ToneCurve).unwrap();
        root.content(0)
            .unwrap()
            .set_node(Some(&inner.node()))
            .unwrap();
        group
            .append_group_content([10., 20.])
            .unwrap()
            .set_node(Some(&sibling.node()))
            .unwrap();
        root.slot(0)
            .unwrap()
            .set_node(Some(&external.node()))
            .unwrap();
        let a = inner.parameter(0).unwrap();
        let b = sibling.parameter(0).unwrap();
        let c = external.parameter(0).unwrap();
        let original = c.curve().unwrap();
        original.set_minimum(0.25);
        original.set_maximum(0.25);
        let mut doc = Gtx375GraphDocument::new(vec![Some(root.clone())]).unwrap();
        doc.set_curve_adjuster(&c, CurveAdjusterLink::Create("Response".into()))
            .unwrap();
        let outside = doc.curve_adjuster(&c).unwrap();
        for property in [&a, &b] {
            doc.set_curve_adjuster(property, CurveAdjusterLink::Existing(outside.clone()))
                .unwrap();
        }
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.set_group_catch_adjusters(&group, true, Some("Surface"))
            .unwrap();
        let inside = doc.curve_adjuster(&a).unwrap();
        let split = a.curve().unwrap();
        assert!(inside.same_reference(&doc.curve_adjuster(&b).unwrap()));
        assert!(!inside.same_reference(&outside));
        assert!(split.same_reference(&b.curve().unwrap()));
        assert!(!split.same_reference(&original));
        assert_eq!(split.minimum(), 0.25);
        assert!(c.curve().unwrap().same_reference(&original));
        history.undo().unwrap();
        assert!(a.curve().unwrap().same_reference(&original));
        assert!(b.curve().unwrap().same_reference(&original));
        history.redo().unwrap();
        assert!(b.curve().unwrap().same_reference(&split));
        let raised = split.source_clone().unwrap();
        raised.set_minimum(0.75);
        raised.set_maximum(0.75);
        doc.edit_composite_curve(&a, &split, raised.clone(), "Edit Curve")
            .unwrap();
        assert_eq!(c.curve().unwrap().minimum(), 0.25);
        doc.edit_lab_preset(Some(&root), LabPresetOperation::Save("Raised".into()))
            .unwrap();
        doc.set_group_catch_adjusters(&group, false, None).unwrap();
        assert!(a.curve().unwrap().same_reference(&original));
        assert!(b.curve().unwrap().same_reference(&original));
        assert_eq!(doc.catalog().unwrap().len(), 1);
        assert!(
            group
                .group_presets()
                .unwrap()
                .lock()
                .unwrap()
                .groups()
                .iter()
                .all(|preset| preset.is_hidden())
        );
        history.undo().unwrap();
        assert!(a.curve().unwrap().same_reference(&raised));
        assert!(b.curve().unwrap().same_reference(&raised));
        assert_eq!(doc.catalog().unwrap().len(), 2);
        assert_eq!(doc.lab_presets(Some(&root)).presets[0].name(), "Raised");
        for native in [true, false] {
            let mut wire = BinaryWriter::default();
            let reopened = if native {
                doc.save_project(&mut wire).unwrap();
                Gtx375GraphDocument::<()>::load_project(
                    wire.bytes(),
                    crate::NodeWireReadPolicy::new(32, 128),
                )
                .unwrap()
            } else {
                doc.save_compatible_gtx(&mut wire).unwrap();
                Gtx375GraphDocument::<()>::load(wire.bytes()).unwrap()
            };
            let root = reopened.branch(0).flatten().unwrap();
            let property = |slot: crate::PointwiseSlot<()>| {
                slot.node().unwrap().filter().unwrap().parameter(0).unwrap()
            };
            let a = property(root.content(0).unwrap());
            let b = property(root.content(1).unwrap());
            let c = property(root.slot(0).unwrap());
            assert_eq!(a.curve().unwrap().minimum(), 0.75);
            assert!(a.curve().unwrap().same_reference(&b.curve().unwrap()));
            assert_eq!(c.curve().unwrap().minimum(), 0.25);
            assert_eq!(
                reopened.lab_presets(Some(&root)).presets[0].name(),
                "Raised"
            );
        }
    }

    #[test]
    fn branch_move_joins_a_curve_in_the_destination_lab_and_undo_restores_the_source() {
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let root = group.node();
        let resident = PointwiseFilter::new(PointwiseKind::ToneCurve).unwrap();
        let moving = PointwiseFilter::new(PointwiseKind::ToneCurve).unwrap();
        root.content(0)
            .unwrap()
            .set_node(Some(&resident.node()))
            .unwrap();
        let target = group.append_group_content([10., 20.]).unwrap();
        let a = resident.parameter(0).unwrap();
        let b = moving.parameter(0).unwrap();
        let inner = a.curve().unwrap();
        inner.set_minimum(0.75);
        inner.set_maximum(0.75);
        let outer = b.curve().unwrap();
        outer.set_minimum(0.25);
        outer.set_maximum(0.25);
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(root.clone()), Some(moving.node())]).unwrap();
        doc.set_group_catch_adjusters(&group, true, Some("Surface"))
            .unwrap();
        for property in [&a, &b] {
            doc.set_curve_adjuster(property, CurveAdjusterLink::Create("Response".into()))
                .unwrap();
        }
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.edit_slot(&target, Some(&moving.node())).unwrap();
        assert!(doc.branch(1).unwrap().is_none());
        assert!(target.node().unwrap().same_reference(&moving.node()));
        assert!(b.curve().unwrap().same_reference(&inner));
        assert!(doc.lab_adjuster_metadata(None).is_empty());
        assert_eq!(history.cursor(), 1);
        history.undo().unwrap();
        assert!(target.node().is_none());
        assert!(
            doc.branch(1)
                .flatten()
                .unwrap()
                .same_reference(&moving.node())
        );
        assert!(b.curve().unwrap().same_reference(&outer));
        assert_eq!(doc.lab_adjuster_metadata(None).len(), 1);
        history.redo().unwrap();
        assert!(b.curve().unwrap().same_reference(&inner));
        doc.edit_branch(1, Some(moving.node())).unwrap();
        let detached = b.curve().unwrap();
        assert_eq!(detached.minimum(), 0.75);
        assert!(!detached.same_reference(&inner));
        assert_eq!(b.retained_adjustment().unwrap().guid(), [0; 16]);
        history.undo().unwrap();
        assert!(b.curve().unwrap().same_reference(&inner));
        history.redo().unwrap();
        assert!(b.curve().unwrap().same_reference(&detached));
    }

    #[test]
    fn catch_adjusters_splits_synthesis_materials_and_release_restores_the_outer_texture() {
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let group_node = group.node();
        let inner = PointwiseFilter::new(PointwiseKind::Synthesis).unwrap();
        let external = PointwiseFilter::new(PointwiseKind::Synthesis).unwrap();
        group_node
            .content(0)
            .unwrap()
            .set_node(Some(&inner.node()))
            .unwrap();
        group_node
            .slot(0)
            .unwrap()
            .set_node(Some(&external.node()))
            .unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(group_node.clone())]).unwrap();
        doc.set_synthesis_adjuster(&external, SynthesisAdjusterLink::Create("Material".into()))
            .unwrap();
        let outer = doc.synthesis_adjuster(&external).unwrap();
        doc.set_synthesis_adjuster(&inner, SynthesisAdjusterLink::Existing(outer.clone()))
            .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.set_group_catch_adjusters(&group, true, Some("Surfaces"))
            .unwrap();
        let caught = doc.synthesis_adjuster(&inner).unwrap();
        assert!(!caught.same_reference(&outer));
        assert!(
            doc.synthesis_adjuster(&external)
                .unwrap()
                .same_reference(&outer)
        );
        history.undo().unwrap();
        assert!(
            doc.synthesis_adjuster(&inner)
                .unwrap()
                .same_reference(&outer)
        );
        history.redo().unwrap();
        assert!(
            doc.synthesis_adjuster(&inner)
                .unwrap()
                .same_reference(&caught)
        );

        let mut draft = inner.synthesis_edit().unwrap();
        draft.add_layer(crate::SynthesisLayerKind::Solid);
        assert!(
            inner
                .edit_synthesis(draft, "Add inner material layer")
                .unwrap()
        );
        let levels = || {
            [
                inner.synthesis_edit().unwrap().layers(),
                external.synthesis_edit().unwrap().layers(),
            ]
        };
        assert_eq!(levels(), [1, 0]);
        doc.edit_lab_preset(
            Some(&group_node),
            LabPresetOperation::Save("Surfaces".into()),
        )
        .unwrap();
        let section = || Some(&group_node);
        doc.set_group_catch_adjusters(&group, false, None).unwrap();
        assert!(
            doc.synthesis_adjuster(&inner)
                .unwrap()
                .same_reference(&outer)
        );
        assert_eq!(levels(), [0, 0]);
        assert_eq!(doc.catalog().unwrap().len(), 1);
        history.undo().unwrap();
        assert_eq!(doc.lab_presets(section()).presets[0].name(), "Surfaces");
        assert_eq!(doc.catalog().unwrap().len(), 2);
        assert!(
            doc.synthesis_adjuster(&inner)
                .unwrap()
                .same_reference(&caught)
        );
        assert_eq!(levels(), [1, 0]);
        history.redo().unwrap();
        assert_eq!(levels(), [0, 0]);
        assert_eq!(doc.catalog().unwrap().len(), 1);
    }

    #[test]
    fn group_contents_removal_retires_last_link_and_restores_its_preset() {
        use crate::{LabNumericAxis, LabNumericLink, LabNumericProperty};
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let node = PointwiseNode::solid_color().unwrap();
        let extra = group.append_group_content([10., 20.]).unwrap();
        extra.set_node(Some(&node)).unwrap();
        let root = group.node();
        let mut doc = Gtx375GraphDocument::new(vec![Some(root.clone())]).unwrap();
        doc.set_group_catch_adjusters(&group, true, Some("Surfaces"))
            .unwrap();
        let property = LabNumericProperty::transparency(&node).unwrap();
        doc.set_numeric_property_link(
            &property,
            LabNumericAxis::Whole,
            LabNumericLink::Create("Opacity".into()),
        )
        .unwrap();
        doc.edit_lab_preset(Some(&root), LabPresetOperation::Save("Saved".into()))
            .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.remove_group_content(&extra).unwrap();
        assert_eq!(root.contents().len(), 1);
        assert!(doc.lab_adjuster_metadata(Some(&root)).is_empty());
        assert!(doc.lab_presets(Some(&root)).presets.is_empty());
        assert_eq!(history.commands().len(), 1);
        history.undo().unwrap();
        assert!(
            root.content(1)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&node)
        );
        assert_eq!(doc.lab_adjuster_metadata(Some(&root))[0].name(), "Opacity");
        assert_eq!(doc.lab_presets(Some(&root)).presets[0].name(), "Saved");
        history.redo().unwrap();
        assert!(doc.lab_adjuster_metadata(Some(&root)).is_empty());
    }

    #[test]
    fn group_branch_compacts_outer_references_and_preserves_nested_contents() {
        let node = |kind| PointwiseFilter::<()>::new(kind).unwrap().node();
        let reference = |number| {
            let reference = node(PointwiseKind::Reference);
            reference
                .filter()
                .unwrap()
                .parameter(0)
                .unwrap()
                .set_number(number)
                .unwrap();
            reference
        };
        let slot_number =
            |node: &PointwiseNode<()>| node.filter().unwrap().parameter(0).unwrap().number();
        let outer = node(PointwiseKind::Group);
        outer
            .filter()
            .unwrap()
            .parameter(0)
            .unwrap()
            .set_number(7.)
            .unwrap();
        for (index, red) in [(1, 0.125), (6, 0.25)] {
            let solid = PointwiseNode::solid_color().unwrap();
            solid.set_solid_color([red, 0., 0.]).unwrap();
            outer.slot(index).unwrap().set_node(Some(&solid)).unwrap();
        }
        let nested = node(PointwiseKind::Group);
        nested
            .filter()
            .unwrap()
            .parameter(0)
            .unwrap()
            .set_number(2.)
            .unwrap();
        nested
            .slot(1)
            .unwrap()
            .set_node(Some(&reference(7.)))
            .unwrap();
        nested
            .content(0)
            .unwrap()
            .set_node(Some(&reference(2.)))
            .unwrap();
        let add_inner = node(PointwiseKind::Add);
        add_inner
            .slot(0)
            .unwrap()
            .set_node(Some(&reference(7.)))
            .unwrap();
        add_inner.slot(1).unwrap().set_node(Some(&nested)).unwrap();
        let branch = node(PointwiseKind::Add);
        branch
            .slot(0)
            .unwrap()
            .set_node(Some(&reference(2.)))
            .unwrap();
        branch.slot(1).unwrap().set_node(Some(&add_inner)).unwrap();
        let target = outer.content(0).unwrap();
        target.set_node(Some(&branch)).unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(outer.clone())]).unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        let red = |root: &PointwiseNode<()>| {
            root.render_original(8, 8)
                .unwrap()
                .image()
                .unwrap()
                .channel(0, true)
                .unwrap()
                .read(0)
                .unwrap()
        };
        let before = red(&outer);
        doc.group_slot(&target).unwrap();
        let wrapped = target.node().unwrap();
        let check_ports = |wrapped: &PointwiseNode<()>| {
            let ports = wrapped.reference_inputs().unwrap();
            assert_eq!(ports.len(), 2);
            assert_eq!(slot_number(&ports[0].node().unwrap()), 2.);
            assert_eq!(slot_number(&ports[1].node().unwrap()), 7.);
            let copy = wrapped.content(0).unwrap().node().unwrap();
            assert!(!copy.same_reference(&branch));
            let first = copy.slot(0).unwrap().node().unwrap();
            assert_eq!(slot_number(&first), 1.);
            assert!(
                first
                    .reference_target()
                    .unwrap()
                    .unwrap()
                    .same_reference(&ports[0].node().unwrap())
            );
            let inner = copy.slot(1).unwrap().node().unwrap();
            assert_eq!(slot_number(&inner.slot(0).unwrap().node().unwrap()), 2.);
            let nested_copy = inner.slot(1).unwrap().node().unwrap();
            assert_eq!(
                slot_number(&nested_copy.slot(1).unwrap().node().unwrap()),
                2.
            );
            assert_eq!(
                slot_number(&nested_copy.content(0).unwrap().node().unwrap()),
                2.
            );
        };
        check_ports(&wrapped);
        assert_eq!(red(&outer), before);
        assert_eq!(history.commands().len(), 1);
        history.undo().unwrap();
        assert!(target.node().unwrap().same_reference(&branch));
        assert_eq!(red(&outer), before);
        history.redo().unwrap();
        assert!(target.node().unwrap().same_reference(&wrapped));
        let mut wire = BinaryWriter::default();
        doc.save(&mut wire).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(wire.bytes())
            .unwrap()
            .branch(0)
            .unwrap()
            .unwrap();
        check_ports(&reopened.content(0).unwrap().node().unwrap());
        assert_eq!(red(&reopened), before);
    }

    #[test]
    fn disconnected_branch_placement_travels_and_round_trips() {
        let group = PointwiseFilter::<()>::new(PointwiseKind::Group).unwrap();
        let inner = group.append_group_content([8., 12.]).unwrap();
        let source = PointwiseNode::solid_color().unwrap();
        inner.set_node_quiet(Some(&source)).unwrap();
        let extra = PointwiseNode::invert();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(group.node()), Some(extra.clone())]).unwrap();
        doc.positions_mut()[0] = [40., -20.];
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        assert!(doc.edit_branch_position(0, [10., 2.]).is_err());
        assert!(doc.edit_branch_position(1, [f32::NAN, 2.]).is_err());
        assert!(!doc.edit_branch_position(1, [40., -20.]).unwrap());
        assert!(doc.edit_branch_position(1, [-135., 76.]).unwrap());
        assert!(
            group
                .edit_group_content_position(&inner, [110., -77.])
                .unwrap()
        );
        assert_eq!(history.commands().len(), 2);
        history.undo().unwrap();
        assert_eq!(group.group_content_positions(), [[8., 12.]]);
        history.undo().unwrap();
        assert_eq!(doc.positions().as_ref(), &[[40., -20.]]);
        history.redo().unwrap();
        history.redo().unwrap();
        assert_eq!(doc.positions().as_ref(), &[[-135., 76.]]);
        assert_eq!(group.group_content_positions(), [[110., -77.]]);
        assert!(inner.node().unwrap().same_reference(&source));
        assert!(doc.branch(1).unwrap().unwrap().same_reference(&extra));
        let mut bytes = BinaryWriter::default();
        doc.save(&mut bytes).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(bytes.bytes()).unwrap();
        assert_eq!(reopened.positions().as_ref(), &[[-135., 76.]]);
        let loaded_group = reopened.branch(0).unwrap().unwrap().filter().unwrap();
        assert_eq!(loaded_group.group_content_positions(), [[110., -77.]]);
    }

    #[test]
    fn docking_a_nested_tree_preserves_the_displaced_output_and_its_history() {
        for recording in [false, true] {
            let output = PointwiseNode::<()>::invert();
            let source = PointwiseNode::solid_color().unwrap();
            let input = output.slot(0).unwrap();
            input.set_node(Some(&source)).unwrap();
            let mut document = Gtx375GraphDocument::new(vec![Some(output.clone())]).unwrap();
            let history = recording.then(|| {
                let history = document
                    .bind_property_history(Arc::new(LabTestHistory))
                    .unwrap();
                history.set_recording(true);
                history
            });
            document.dock_texture_output(source.clone()).unwrap();
            assert!(
                document
                    .branch(0)
                    .flatten()
                    .unwrap()
                    .same_reference(&source)
            );
            assert!(
                document
                    .branch(1)
                    .flatten()
                    .unwrap()
                    .same_reference(&output)
            );
            assert!(input.node().is_none_or(|node| node.is_default_input()));
            if let Some(history) = &history {
                assert_eq!(history.commands().len(), 1);
                history.undo().unwrap();
                assert!(
                    document
                        .branch(0)
                        .flatten()
                        .unwrap()
                        .same_reference(&output)
                );
                assert!(
                    output
                        .slot(0)
                        .unwrap()
                        .node()
                        .unwrap()
                        .same_reference(&source)
                );
                history.redo().unwrap();
            }
            let loose = document.detach_texture_output_at([40.0, 30.0]).unwrap();
            assert!(document.branch(0).unwrap().is_none());
            assert!(
                document
                    .branch(loose)
                    .flatten()
                    .unwrap()
                    .same_reference(&source)
            );
            assert_eq!(document.positions()[loose - 1], [40.0, 30.0]);
            if let Some(history) = history {
                history.undo().unwrap();
                assert!(
                    document
                        .branch(0)
                        .flatten()
                        .unwrap()
                        .same_reference(&source)
                );
            }
        }
    }

    #[test]
    fn document_input_encloses_real_branches_and_retains_lab_links() {
        use crate::{LabNumericAxis, LabNumericLink, LabNumericProperty};

        let root = PointwiseNode::<()>::invert();
        let reference = PointwiseFilter::new(PointwiseKind::Reference)
            .unwrap()
            .node();
        root.slot(0)
            .unwrap()
            .set_node_quiet(Some(&reference))
            .unwrap();
        let extra = PointwiseNode::solid_color().unwrap();
        let input = PointwiseNode::solid_color().unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(root.clone()), None, Some(extra.clone())]).unwrap();
        doc.positions_mut()
            .copy_from_slice(&[[11., 22.], [33., 44.]]);
        let opacity = LabNumericProperty::transparency(&extra).unwrap();
        doc.set_numeric_property_link(
            &opacity,
            LabNumericAxis::Whole,
            LabNumericLink::Create("Opacity".into()),
        )
        .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        assert!(doc.insert_document_input(&root).is_err());
        doc.insert_document_input(&input).unwrap();
        assert_eq!(history.commands().len(), 1);
        assert_eq!(doc.branches().len(), 1);
        let group = doc.branch(0).unwrap().unwrap();
        assert_eq!(group.reference_inputs().unwrap().len(), 1);
        assert!(
            group
                .content(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&root)
        );
        assert!(group.content(1).unwrap().node().is_none());
        assert!(
            group
                .content(2)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&extra)
        );
        assert_eq!(
            group.filter().unwrap().group_content_positions(),
            [[11., 22.], [33., 44.]]
        );
        assert!(
            reference
                .reference_target()
                .unwrap()
                .unwrap()
                .same_reference(&input)
        );
        assert_eq!(doc.numeric_lab_adjusters(None)[0].name(), "Opacity");
        history.undo().unwrap();
        assert_eq!(doc.branches().len(), 3);
        assert!(doc.branch(0).unwrap().unwrap().same_reference(&root));
        assert!(doc.branch(1).unwrap().is_none());
        assert!(doc.branch(2).unwrap().unwrap().same_reference(&extra));
        assert_eq!(doc.positions().as_ref(), &[[11., 22.], [33., 44.]]);
        assert!(reference.reference_target().unwrap().is_none());
        history.redo().unwrap();
        assert!(doc.branch(0).unwrap().unwrap().same_reference(&group));

        let mut writer = BinaryWriter::default();
        doc.save(&mut writer).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(writer.bytes()).unwrap();
        let group = reopened.branch(0).unwrap().unwrap();
        let input = group.slot(0).unwrap().node().unwrap();
        let inner = group.content(0).unwrap().node().unwrap();
        assert!(
            inner
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .reference_target()
                .unwrap()
                .unwrap()
                .same_reference(&input)
        );
        assert_eq!(
            group.filter().unwrap().group_content_positions(),
            [[33., 44.]]
        );
        assert_eq!(reopened.numeric_lab_adjusters(None)[0].name(), "Opacity");
    }

    #[test]
    fn document_input_travels_from_an_empty_document_and_restores_filter_samples() {
        let mut empty = Gtx375GraphDocument::<()>::new(vec![None]).unwrap();
        let history = empty
            .bind_property_history(Arc::new(LabTestHistory))
            .unwrap();
        history.set_recording(true);
        empty
            .insert_document_input(&PointwiseNode::invert())
            .unwrap();
        assert!(
            empty
                .branch(0)
                .unwrap()
                .unwrap()
                .content(0)
                .unwrap()
                .node()
                .is_none()
        );
        history.undo().unwrap();
        assert!(empty.branch(0).unwrap().is_none());
        history.redo().unwrap();
        assert!(
            empty
                .branch(0)
                .unwrap()
                .unwrap()
                .slot(0)
                .unwrap()
                .node()
                .is_some()
        );

        let filter = PointwiseFilter::new(PointwiseKind::Group).unwrap().node();
        let mut doc = Gtx375GraphDocument::new(vec![Some(filter.clone())]).unwrap();
        doc.set_output_order([3, 1, 2]);
        filter.convert_to_filter(false).unwrap();
        let sample = filter.slot(0).unwrap().node().unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.insert_document_input(&PointwiseNode::invert()).unwrap();
        assert!(!filter.is_filter());
        assert!(filter.slot(0).unwrap().node().is_none());
        assert!(doc.filter_preview_sample_index().is_none());
        history.undo().unwrap();
        assert!(filter.is_filter());
        assert!(
            filter
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&sample)
        );
        assert_eq!(doc.filter_preview_sample_index(), Some(3));
        history.redo().unwrap();
        assert!(!filter.is_filter());
        assert_eq!(doc.branches().len(), 1);
    }

    #[test]
    fn group_branch_retains_a_root_lab_control_on_the_copied_receiver() {
        use crate::{LabNumericAxis, LabNumericLink, LabNumericProperty};
        let solid = PointwiseNode::solid_color().unwrap();
        let mut doc = Gtx375GraphDocument::new(vec![Some(solid.clone()), None]).unwrap();
        let opacity = LabNumericProperty::transparency(&solid).unwrap();
        doc.set_numeric_property_link(
            &opacity,
            LabNumericAxis::Whole,
            LabNumericLink::Create("Opacity".into()),
        )
        .unwrap();
        let history = doc.bind_property_history(Arc::new(LabTestHistory)).unwrap();
        history.set_recording(true);
        doc.group_branch(0).unwrap();
        let group = doc.branch(0).unwrap().unwrap();
        assert!(group.reference_inputs().unwrap().is_empty());
        let copy = group.content(0).unwrap().node().unwrap();
        assert!(!copy.same_reference(&solid));
        let control = doc.numeric_lab_adjusters(None).remove(0);
        doc.edit_numeric_lab_adjuster(None, &control, LabNumericValue::Number(25.))
            .unwrap();
        assert_ne!(
            copy.solid_transparency().unwrap(),
            solid.solid_transparency().unwrap()
        );
        history.undo().unwrap();
        history.undo().unwrap();
        assert!(doc.branch(0).unwrap().unwrap().same_reference(&solid));
        doc.group_branch(1).unwrap();
        let empty = doc.branch(1).unwrap().unwrap();
        assert!(empty.reference_inputs().unwrap().is_empty());
        assert!(empty.content(0).unwrap().node().is_none());
    }

    #[test]
    fn load_reconnects_an_environment_adjuster_and_retains_its_gtx_preset() {
        let name: Vec<u16> = "Studio Light".encode_utf16().collect();
        let adjusted = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        let independent = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        let second_target = PointwiseFilter::<()>::new(PointwiseKind::MakeEnvironment).unwrap();
        for (filter, name) in [
            (&adjusted, name.as_slice()),
            (&second_target, name.as_slice()),
            (&independent, &[65][..]),
        ] {
            let value = filter.parameter(0).unwrap();
            value.set_environment(Arc::new(red_map(0.125))).unwrap();
            let adjustment = value.adjustment().unwrap().unwrap();
            adjustment
                .names()
                .set_entry(0, Some(name.to_vec()))
                .unwrap();
            adjustment.flags().set(0, true).unwrap();
        }
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&adjusted.node()))
            .unwrap();
        group
            .append_group_content([100., 0.])
            .unwrap()
            .set_node(Some(&independent.node()))
            .unwrap();
        let mut doc =
            Gtx375GraphDocument::new(vec![Some(group.node()), Some(second_target.node())]).unwrap();
        let entry = |group_id, red| GtxCatalogRecord::Environment {
            metadata: AdjusterMetadataRecord {
                name: Some(name.clone()),
                group_id,
                type_tag: 9,
                order: 0,
                loaded: false,
            },
            value: EnvironmentCatalogValue::new(red_map(red)),
        };
        doc.catalog = GraphCatalog::new(vec![entry([7; 16], 0.875), entry([0; 16], 0.625)]);
        let mut preset = GtxPresetRecord::new(GtxPresetKind::Environment, Some(name), [0; 16]);
        *preset.as_environment_mut().unwrap() = red_map(0.375);
        let mut wire = BinaryWriter::default();
        wire.write_i32(1).unwrap();
        wire.write_utf16(&[80]).unwrap();
        wire.write_bytes(&[0; 16]).unwrap();
        wire.write_i32(1).unwrap();
        preset.save(&mut wire).unwrap();
        doc.presets
            .load(&mut BinaryCursor::new(wire.bytes()).unwrap(), VERSION)
            .unwrap();

        let mut writer = BinaryWriter::default();
        doc.save(&mut writer).unwrap();
        let mut loaded = Gtx375GraphDocument::<()>::load(writer.bytes()).unwrap();
        let root = loaded.branch(0).unwrap().unwrap();
        let node = |index| root.content(index).unwrap().node().unwrap();
        let red = |image: crate::SharedImageMap| image.channel(0, true).unwrap().read(0).unwrap();
        assert_eq!(
            red(root.render_original(16, 16).unwrap().image().unwrap()),
            0.625
        );
        assert_eq!(
            red(node(1).render_original(16, 16).unwrap().image().unwrap()),
            0.125
        );

        let snapshot = RenderCaptureSession::new().capture_node(&root, 0.).unwrap();
        let export_red = |snapshot: &crate::PointwiseNodeRenderSnapshot| {
            let mut capacity = |_| Ok(true);
            let mut preflight = |_, _| Ok(());
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            let rendered = snapshot
                .render_export_with_effects(
                    ImageGeometry::new(16, 16),
                    ProceduralExecution::CallingThread,
                    PointwiseRenderOptions::new(false, false),
                    &mut effects,
                )
                .unwrap();
            assert!(rendered.caught_errors().is_empty());
            red(rendered.image().unwrap())
        };

        let property = node(0).filter().unwrap().parameter(0).unwrap();
        let adjuster = loaded.environment_adjuster(&property).unwrap();
        let before = adjuster.value();
        assert!(
            loaded
                .edit_environment_adjuster(&property, &adjuster, &before, red_map(0.5))
                .unwrap()
        );
        assert!(
            !loaded
                .edit_environment_adjuster(&property, &adjuster, &before, red_map(1.0))
                .unwrap()
        );
        let second_target = loaded.branch(1).unwrap().unwrap();
        assert_eq!(
            red(root.render_original(16, 16).unwrap().image().unwrap()),
            0.5
        );
        assert_eq!(
            red(second_target
                .render_original(16, 16)
                .unwrap()
                .image()
                .unwrap()),
            0.5
        );
        assert_eq!(
            red(node(1).render_original(16, 16).unwrap().image().unwrap()),
            0.125
        );
        let updated = RenderCaptureSession::new().capture_node(&root, 0.).unwrap();
        assert_eq!(export_red(&snapshot), 0.625);
        assert_eq!(export_red(&updated), 0.5);

        let mut saved = BinaryWriter::default();
        loaded.save(&mut saved).unwrap();
        let reopened = Gtx375GraphDocument::<()>::load(saved.bytes()).unwrap();
        let root = reopened.branch(0).unwrap().unwrap();
        assert_eq!(
            red(root.render_original(16, 16).unwrap().image().unwrap()),
            0.5
        );
        let preset = &reopened.presets().groups()[0].presets()[0];
        assert_eq!(preset.kind(), GtxPresetKind::Environment);
        let GtxPresetValue::Environment(map) = preset.value() else {
            panic!("expected the retained Environment preset map");
        };
        assert_eq!(map.field(EnvironmentField::Red(0)), Some(0.375));
    }
}
