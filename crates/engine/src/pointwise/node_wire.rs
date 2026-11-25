//! Live zl.kw/i0 orchestration. Retained property handles and qi occurrences
//! remain the authority; no wire DTO, graph snapshot or alias-ID format exists.
mod factory;
mod lab;
mod lab_assets;
use super::graph::Payload;
use super::{
    ImageGeneratorParameter, ImageGeneratorParameterKind, LivePropertyWireError, PointwiseError,
    PointwiseInvertError, PointwiseKind, PointwiseNode, PointwiseParameter,
    PointwiseSolidEditError,
};
use crate::encoded_image::EncodedImageGenerations;
use crate::{BinaryCursor, BinaryError, BinaryWriter};
pub(crate) use factory::load_node_binary_with_context;
mod xml;
pub use factory::{create_node_by_identity, load_node_binary};
pub(super) use lab::{
    default_style_noise_lab, read_style_noise_lab, switch_wizard_template, write_style_noise_lab,
};
use std::fmt;
pub(crate) use xml::load_node_xml;

/// Real leaf failures retain their typed causes and already committed state.
#[derive(Debug)]
pub enum NodeWireError<E = ()> {
    Binary(BinaryError),
    Property(LivePropertyWireError<E>),
    Node(PointwiseError<E>),
    Solid(PointwiseSolidEditError<E>),
    Invert(PointwiseInvertError<E>),
    Preset(crate::GtxPresetError),
    PropertyIndex,
    SlotCountMismatch {
        serialized: u8,
        constructed: usize,
    },
    LabTemplate {
        path: &'static str,
        position: i32,
        error: Box<crate::GtxGraphError<E>>,
    },
    MissingLabTemplate(String),
    LegacyXml(String),
    FactoryArgument {
        class_name: Vec<u16>,
    },
    UnimplementedNode {
        identity: Vec<u16>,
    },
    UnsupportedCapability(&'static str),
    /// Optional caller admission policy, never an original exception class.
    ReadLimit {
        depth: usize,
        nodes: usize,
    },
}
impl<E> From<BinaryError> for NodeWireError<E> {
    fn from(error: BinaryError) -> Self {
        Self::Binary(error)
    }
}
impl<E> From<LivePropertyWireError<E>> for NodeWireError<E> {
    fn from(error: LivePropertyWireError<E>) -> Self {
        Self::Property(error)
    }
}
impl<E> From<PointwiseError<E>> for NodeWireError<E> {
    fn from(error: PointwiseError<E>) -> Self {
        Self::Node(error)
    }
}
impl<E> NodeWireError<E> {
    pub fn parameter_name(&self) -> Option<&'static str> {
        matches!(self, Self::FactoryArgument { .. }).then_some("className")
    }
    /// None denotes a target capability/API boundary, never a source exception.
    pub fn source_class_with(&self, observer: impl Fn(&E) -> &'static str) -> Option<&'static str> {
        match self {
            Self::Binary(error) => Some(error.source_class()),
            Self::Property(LivePropertyWireError::UnsupportedCapability(_)) => None,
            Self::Property(LivePropertyWireError::Property(error)) => {
                pointwise_source_class_with(error, &observer)
            }
            Self::Property(error) => Some(error.source_class()),
            Self::Node(error) => pointwise_source_class_with(error, &observer),
            Self::Solid(error) => error.source_class_with(observer),
            Self::Invert(error) => error.source_class_with(observer),
            Self::Preset(error) => error.source_class(),
            Self::PropertyIndex => Some("ArgumentOutOfRangeException"),
            Self::SlotCountMismatch { .. } => Some("Exception"),
            Self::FactoryArgument { .. } => Some("ArgumentException"),
            Self::UnimplementedNode { .. }
            | Self::UnsupportedCapability(_)
            | Self::LegacyXml(_)
            | Self::ReadLimit { .. }
            | Self::LabTemplate { .. }
            | Self::MissingLabTemplate(_) => None,
        }
    }
}
pub(in crate::pointwise) fn pointwise_source_class_with<E>(
    error: &PointwiseError<E>,
    observer: &impl Fn(&E) -> &'static str,
) -> Option<&'static str> {
    if error.is_unsupported_capability() {
        return None;
    }
    match error {
        PointwiseError::Observer(error)
        | PointwiseError::Selection(crate::SelectionError::Observer(error)) => {
            Some(observer(error))
        }
        PointwiseError::Curve(curve) => match curve.as_ref() {
            crate::CurveError::Observer(cause) => Some(observer(cause)),
            _ => Some(error.source_class()),
        },
        PointwiseError::Animation(error) => match error.as_ref() {
            crate::AnimationError::Curve(crate::CurveError::Observer(error)) => {
                Some(observer(error))
            }
            _ => Some(error.source_class()),
        },
        PointwiseError::CapacityQuery(_) => None,
        _ => Some(error.source_class()),
    }
}
impl<E: fmt::Debug> fmt::Display for NodeWireError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::Property(error @ LivePropertyWireError::UnknownChoice { .. }) = self {
            return fmt::Display::fmt(error, formatter);
        }
        if let Self::UnimplementedNode { identity } = self {
            return write!(
                formatter,
                "Mycela does not support the “{}” node yet.",
                String::from_utf16_lossy(identity)
            );
        }
        write!(formatter, "{self:?}")
    }
}
impl<E: fmt::Debug + 'static> std::error::Error for NodeWireError<E> {}

/// Explicit application load budget. Default source-shaped APIs remain unlimited.
#[derive(Clone, Copy, Debug)]
pub struct NodeWireReadPolicy {
    max_depth: usize,
    max_nodes: usize,
}
impl NodeWireReadPolicy {
    pub fn new(max_depth: usize, max_nodes: usize) -> Self {
        Self {
            max_depth,
            max_nodes,
        }
    }
    pub fn max_depth(self) -> usize {
        self.max_depth
    }
    pub fn max_nodes(self) -> usize {
        self.max_nodes
    }
}
pub(crate) struct NodeWireReadContext<'a, E> {
    policy: Option<NodeWireReadPolicy>,
    depth: usize,
    nodes: usize,
    legacy_mycela_layout: bool,
    mycela_project_version: Option<i32>,
    active_templates: Vec<&'static str>,
    image_generations: EncodedImageGenerations,
    variables: Option<(
        &'a [u16],
        &'a mut dyn crate::VariableFileService<Error = PointwiseError<E>>,
    )>,
}
impl<'a, E> NodeWireReadContext<'a, E> {
    pub(crate) fn for_document(policy: Option<NodeWireReadPolicy>) -> Self {
        Self {
            policy,
            depth: 0,
            nodes: 0,
            legacy_mycela_layout: true,
            mycela_project_version: None,
            active_templates: Vec::new(),
            image_generations: EncodedImageGenerations::default(),
            variables: None,
        }
    }
    pub(crate) fn with_variables(
        policy: Option<NodeWireReadPolicy>,
        base: &'a [u16],
        service: &'a mut dyn crate::VariableFileService<Error = PointwiseError<E>>,
    ) -> Self {
        Self {
            policy,
            depth: 0,
            nodes: 0,
            legacy_mycela_layout: false,
            mycela_project_version: None,
            active_templates: Vec::new(),
            image_generations: EncodedImageGenerations::default(),
            variables: Some((base, service)),
        }
    }
    pub(crate) fn allow_legacy_mycela_layout(&mut self) {
        self.legacy_mycela_layout = true;
    }
    pub(crate) fn for_project(policy: NodeWireReadPolicy, version: i32) -> Self {
        Self {
            mycela_project_version: Some(version),
            ..Self::for_document(Some(policy))
        }
    }
    pub(super) fn is_mycela_project(&self) -> bool {
        self.mycela_project_version.is_some()
    }
    pub(super) fn uses_legacy_canvas_visibility(&self) -> bool {
        self.mycela_project_version == Some(1)
    }
    pub(crate) fn preserves_saved_patch_shape_values(&self) -> bool {
        !matches!(self.mycela_project_version, Some(1 | 2))
    }
    fn for_raw_node() -> Self {
        Self {
            legacy_mycela_layout: false,
            ..Self::for_document(None)
        }
    }
    fn enter(&mut self) -> Result<(), NodeWireError<E>> {
        if self
            .policy
            .is_some_and(|policy| self.depth >= policy.max_depth || self.nodes >= policy.max_nodes)
        {
            return Err(NodeWireError::ReadLimit {
                depth: self.depth,
                nodes: self.nodes,
            });
        }
        self.depth += 1;
        self.nodes += 1;
        Ok(())
    }
    fn leave(&mut self) {
        self.depth -= 1;
    }
}

// A property array contains actual retained handles, not copied values or
// independent wire metadata. Solid/Invert use the actual live leaf adapters.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeWriteLayout {
    Standalone,
    GtxDocument,
    MycelaProject,
}

enum Property<E> {
    Modern(super::scene_primitive::ModernNodeProperty<E>),
    Filter(PointwiseParameter<E>),
    Style(super::PointwiseFilter<E>),
    Synthesis(super::PointwiseFilter<E>),
    Generator(ImageGeneratorParameter<E>),
    Solid(PointwiseNode<E>, usize),
    Invert(PointwiseNode<E>, usize),
    ImportedMesh(PointwiseNode<E>),
}
impl<E: 'static> Property<E> {
    fn save(&self, writer: &mut BinaryWriter, project: bool) -> Result<(), NodeWireError<E>> {
        match self {
            Self::Filter(property) => Ok(property.save_binary_with_layout(writer, project)?),
            Self::Style(filter) => {
                super::property::wire::save_code(
                    writer,
                    &"Style".encode_utf16().collect::<Vec<_>>(),
                )?;
                filter
                    .style()
                    .ok_or(PointwiseError::InvalidCast)?
                    .write(writer)?;
                writer.write_bool(false)?;
                Ok(writer.write_bool(false)?)
            }
            Self::Synthesis(filter) => {
                super::property::wire::save_code(
                    writer,
                    &"Texture".encode_utf16().collect::<Vec<_>>(),
                )?;
                filter
                    .synthesis_texture()
                    .ok_or(PointwiseError::InvalidCast)?
                    .write(writer)?;
                filter
                    .synthesis_wire()
                    .ok_or(PointwiseError::InvalidCast)?
                    .save_tail(writer, || Ok(None))?;
                Ok(())
            }
            Self::Modern(property) => Ok(property.save_binary(writer)?),
            Self::Generator(property) => Ok(property.save_binary(writer)?),
            Self::Solid(node, index) => node.save_solid_wire_property(*index, writer),
            Self::Invert(node, index) => node.save_invert_wire_property(*index, writer),
            Self::ImportedMesh(node) => {
                writer.write_bool(false)?;
                writer.write_bool(false)?;
                let mesh = node.imported_mesh();
                writer.write_bool(mesh.is_some())?;
                if let Some(mesh) = mesh {
                    mesh.write_wire(writer, project)?;
                }
                Ok(())
            }
        }
    }
    fn load(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
        context: Option<&mut NodeWireReadContext<'_, E>>,
    ) -> Result<(), NodeWireError<E>> {
        match self {
            Self::Filter(property) => {
                if let Some((base, service)) =
                    context.and_then(|context| context.variables.as_mut())
                {
                    Ok(property.load_binary_with_variable_service(
                        reader,
                        version,
                        base,
                        &mut **service,
                    )?)
                } else if property.owner().kind() == PointwiseKind::WizardReference {
                    let mut files = lab_assets::LabFiles::default();
                    Ok(property.load_binary_with_variable_service(
                        reader,
                        version,
                        &[],
                        &mut files,
                    )?)
                } else {
                    Ok(property.load_binary(reader, version)?)
                }
            }
            Self::Modern(property) => Ok(property.load_binary(reader, version)?),
            Self::Style(filter) => {
                super::property::wire::check_code(
                    reader,
                    &"Style".encode_utf16().collect::<Vec<_>>(),
                    false,
                )?;
                let style = super::canvas::CanvasStyle::read(reader, version, context)?;
                if reader.read_bool()? || (version >= 3.54 && reader.read_bool()?) {
                    return Err(NodeWireError::UnsupportedCapability(
                        "Style property binding",
                    ));
                }
                filter.set_style(style);
                Ok(())
            }
            Self::Synthesis(filter) => filter.load_synthesis_texture(reader, version),
            Self::Generator(property) => {
                property.load_binary(reader, version)?;
                if property.kind() == ImageGeneratorParameterKind::Image
                    && let Some(context) = context
                    && let Ok(Some(object)) = property.custom_value()
                    && let Ok(image) = object.imported_image()
                    && let Some(store) = image.container().and_then(|container| container.store())
                    && let Some(bytes) = store.resident_bytes()
                {
                    context.image_generations.share(&bytes);
                }
                Ok(())
            }
            Self::Solid(node, index) => node.load_solid_wire_property(*index, reader, version),
            Self::Invert(node, index) => node.load_invert_wire_property(*index, reader, version),
            Self::ImportedMesh(node) => {
                if version >= 3.7 && (reader.read_bool()? || reader.read_bool()?) {
                    return Err(NodeWireError::UnsupportedCapability(
                        "animated or bound ImportBridge",
                    ));
                }
                let mesh = if reader.read_bool()? {
                    let project = context.is_some_and(|context| context.is_mycela_project());
                    Some(crate::ImportedMesh::read_wire(reader, project)?)
                } else {
                    None
                };
                let Payload::SceneImport { state } = &node.core().payload else {
                    return Err(PointwiseError::InvalidCast.into());
                };
                *super::guard(&state.mesh) = mesh;
                Ok(())
            }
        }
    }
}

fn legacy_environment_skip(reader: &BinaryCursor<'_>) -> Result<bool, BinaryError> {
    let mut probe = reader.fork();
    if !probe.read_string()?.into_iter().eq("Ski".encode_utf16()) {
        return Ok(false);
    }
    probe.read_string()?;
    if probe.read_bool()? {
        probe.read_i16()?;
        probe.read_string()?;
    }
    // The original tail is a false adjustment marker followed by two slots.
    // Mycela's former tail begins with the animation active marker. If false,
    // the subsequent Gradient marker is always false too.
    Ok(match probe.read_u8()? {
        0 => probe.read_u8()? == 0,
        1 => true,
        _ => false,
    })
}
impl<E: 'static> PointwiseNode<E> {
    fn wire_property_count(&self) -> Result<usize, NodeWireError<E>> {
        match &self.core().payload {
            Payload::Input { .. } | Payload::SceneClearLights | Payload::SceneCombine => Ok(0),
            Payload::UvTexture { properties, .. } => Ok(properties.len()),
            Payload::SceneImport { state } => Ok(state.properties.len() + 1),
            Payload::SceneTransform { .. } => Ok(self
                .scene_transform_3d()
                .ok_or(PointwiseError::InvalidCast)?
                .properties()
                .len()),
            Payload::ScenePrimitive { .. } => Ok(self
                .scene_primitive()
                .ok_or(PointwiseError::InvalidCast)?
                .properties()
                .len()),
            Payload::Filter { kind, .. } => {
                let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
                Ok(filter.parameters().len()
                    + usize::from(matches!(
                        kind,
                        PointwiseKind::Style | PointwiseKind::Synthesis
                    )))
            }
            Payload::ImageGenerator { .. } => Ok(self
                .image_generator_handle()
                .ok_or(PointwiseError::InvalidCast)?
                .parameters()
                .len()),
            Payload::Solid { .. } => Ok(2),
            Payload::Invert { .. } => Ok(4),
        }
    }
    fn wire_properties(&self) -> Result<Vec<Property<E>>, NodeWireError<E>> {
        match &self.core().payload {
            Payload::Input { .. } | Payload::SceneClearLights | Payload::SceneCombine => {
                Ok(Vec::new())
            }
            Payload::UvTexture { .. } => Ok(self
                .uv_texture_properties()
                .ok_or(PointwiseError::InvalidCast)?
                .into_iter()
                .map(Property::Modern)
                .collect()),
            Payload::SceneImport { .. } => {
                let mut properties: Vec<_> = self
                    .import_3d_properties()
                    .ok_or(PointwiseError::InvalidCast)?
                    .into_iter()
                    .map(Property::Modern)
                    .collect();
                properties.insert(3, Property::ImportedMesh(self.clone()));
                Ok(properties)
            }
            Payload::SceneTransform { .. } => Ok(self
                .scene_transform_3d()
                .ok_or(PointwiseError::InvalidCast)?
                .properties()
                .into_iter()
                .map(Property::Modern)
                .collect()),
            Payload::ScenePrimitive { .. } => Ok(self
                .scene_primitive()
                .ok_or(PointwiseError::InvalidCast)?
                .properties()
                .into_iter()
                .map(Property::Modern)
                .collect()),
            Payload::Filter { kind, .. } => {
                let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
                let mut properties: Vec<_> = filter
                    .parameters()
                    .into_iter()
                    .map(Property::Filter)
                    .collect();
                if *kind == PointwiseKind::Style {
                    properties.insert(0, Property::Style(filter));
                } else if *kind == PointwiseKind::Synthesis {
                    properties.insert(0, Property::Synthesis(filter));
                }
                Ok(properties)
            }
            Payload::ImageGenerator { .. } => Ok(self
                .image_generator_handle()
                .ok_or(PointwiseError::InvalidCast)?
                .parameters()
                .into_iter()
                .map(Property::Generator)
                .collect()),
            Payload::Solid { .. } => Ok((0..2)
                .map(|index| Property::Solid(self.clone(), index))
                .collect()),
            Payload::Invert { .. } => Ok((0..4)
                .map(|index| Property::Invert(self.clone(), index))
                .collect()),
        }
    }
    /// Writes current live state, with identity first and fresh late property
    /// handles. Byte count narrowing never truncates either ensuing full loop.
    pub fn save_binary(&self, writer: &mut BinaryWriter) -> Result<(), NodeWireError<E>> {
        self.save_binary_inner(writer, NodeWriteLayout::Standalone)
    }
    pub(crate) fn save_document_binary(
        &self,
        writer: &mut BinaryWriter,
        project: bool,
    ) -> Result<(), NodeWireError<E>> {
        let layout = if project {
            NodeWriteLayout::MycelaProject
        } else {
            NodeWriteLayout::GtxDocument
        };
        self.save_binary_inner(writer, layout)
    }
    fn save_binary_inner(
        &self,
        writer: &mut BinaryWriter,
        layout: NodeWriteLayout,
    ) -> Result<(), NodeWireError<E>> {
        writer.write_utf16(&self.identity_name())?;
        let count = self.wire_property_count()?;
        writer.write_u8(count as u8)?;
        // The actual Properties array is obtained AFTER writing the count.
        for property in self.wire_properties()? {
            property.save(writer, layout == NodeWriteLayout::MycelaProject)?;
        }
        if self.kind() == Some(PointwiseKind::WizardGroup) {
            let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
            writer.write_utf16(filter.wizard_label().as_deref().unwrap_or_default())?;
            let properties = filter.wizard_dynamic_properties()?;
            writer.write_u8(properties.len() as u8)?;
            for property in properties {
                property.save_binary(writer)?;
            }
            // hj.aah is true, so x0 writes no child aqa owners here.
            writer.write_u8(0)?;
        }
        if self.kind() == Some(PointwiseKind::MapVariable) {
            let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
            writer.write_utf16(filter.wizard_label().as_deref().unwrap_or_default())?;
            // po inherits x0.si (no dynamic controls), and hj.aah is true,
            // so x0.wa writes a zero child-owner count.
            writer.write_u8(0)?;
            writer.write_u8(0)?;
        }
        if matches!(
            self.kind(),
            Some(PointwiseKind::TemplateLab(_) | PointwiseKind::WizardReference)
        ) {
            let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
            writer.write_utf16(filter.wizard_label().as_deref().unwrap_or_default())?;
            lab::save_controls(self, writer)?;
        }
        if let Some(program) = self.shader_program() {
            writer.write_bool(program.is_imported_pixel_script())?;
            writer.write_utf16(&program.source().encode_utf16().collect::<Vec<_>>())?;
        }
        let mut slots = self.wire_slot_count()?;
        if layout != NodeWriteLayout::Standalone
            && self.kind().is_some_and(PointwiseKind::has_compact_inputs)
            && !self.is_filter()
        {
            // Older Mycela saves could retain connected inactive inputs.
            // Preserve their expanded layout when a compact save would lose data.
            let physical = self.slots();
            if physical
                .iter()
                .skip(slots)
                .any(|slot| slot.node().is_some())
            {
                slots = physical.len();
            }
        }
        writer.write_u8(slots as u8)?;
        for index in 0..slots {
            let index = i32::try_from(index).map_err(|_| PointwiseError::Index)?;
            // IsFilter is actual retained mode, never inferred from filter kind.
            // Refetch Slots/Node for the recursive call as the source does.
            if self.is_filter() || self.slot(index)?.node().is_none() {
                writer.write_utf16(&"null".encode_utf16().collect::<Vec<_>>())?;
            } else {
                self.slot(index)?
                    .node()
                    .ok_or(PointwiseError::NullReference)?
                    .save_binary_inner(writer, layout)?;
            }
        }
        if self.serializes_group_contents() {
            let (contents, positions) = self.compact_wizard_contents()?;
            writer.write_i32(
                i32::try_from(contents.len()).map_err(|_| PointwiseError::Arithmetic)?,
            )?;
            for position in positions {
                writer.write_f32(position[0])?;
                writer.write_f32(position[1])?;
            }
            for slot in contents {
                if let Some(child) = slot.node() {
                    child.save_binary_inner(writer, layout)?;
                } else {
                    writer.write_utf16(&"null".encode_utf16().collect::<Vec<_>>())?;
                }
            }
        }
        if self.kind() == Some(PointwiseKind::Group) {
            let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
            writer.write_bytes(&filter.group_guid().ok_or(PointwiseError::NullReference)?)?;
            let presets = filter
                .group_presets()
                .ok_or(PointwiseError::NullReference)?;
            super::guard(&presets)
                .save(writer)
                .map_err(NodeWireError::Preset)?;
        }
        if self.kind() == Some(PointwiseKind::Canvas) {
            self.filter()
                .and_then(|filter| filter.canvas_document())
                .ok_or(PointwiseError::InvalidCast)?
                .write(writer, layout != NodeWriteLayout::MycelaProject)?;
        }
        Ok(())
    }
    fn wire_slot_count(&self) -> Result<usize, NodeWireError<E>> {
        // zl.kw/i0 call the virtual zl.kr. Group overrides it with the active count.
        if self.kind().is_some_and(PointwiseKind::has_compact_inputs) {
            Ok(self.reference_inputs()?.len())
        } else {
            Ok(self.slots().len())
        }
    }

    fn serializes_group_contents(&self) -> bool {
        matches!(
            self.kind(),
            Some(
                PointwiseKind::WizardGroup
                    | PointwiseKind::Group
                    | PointwiseKind::MapVariable
                    | PointwiseKind::Canvas
            )
        )
    }

    pub(crate) fn gtx_record_children(&self) -> impl Iterator<Item = PointwiseNode<E>> {
        // Fixed template contents are reconstructed from control values.
        // Their generated input References are outside the project's node records.
        let contents = if self.serializes_group_contents() {
            self.contents()
        } else {
            Vec::new()
        };
        self.slots()
            .into_iter()
            .chain(contents)
            .filter_map(|slot| slot.node())
    }

    pub(crate) fn gtx_incompatibilities(&self) -> Result<Vec<&'static str>, NodeWireError<E>> {
        let mut issues = Vec::new();
        let kind = self.kind();
        if matches!(
            kind,
            Some(PointwiseKind::OklabColor | PointwiseKind::MedianBlur | PointwiseKind::Shader)
        ) {
            issues.push("This node type is unavailable in Genetica.");
        }
        if let Some(issue) = self
            .imported_mesh()
            .and_then(|mesh| mesh.gtx_attribute_issue())
        {
            issues.push(issue);
        }
        if kind.is_some_and(PointwiseKind::has_compact_inputs)
            && self
                .slots()
                .iter()
                .skip(self.wire_slot_count()?)
                .any(|slot| {
                    // Filter preview samples are regenerated from the document trailer.
                    slot.node()
                        .is_some_and(|child| !self.is_filter() || !child.is_default_input())
                })
        {
            issues.push("A connected input is outside the active slot layout.");
        }
        let skip = match kind {
            Some(PointwiseKind::ReflectEnvironment) => Some(1),
            Some(PointwiseKind::RefractEnvironment) => Some(2),
            _ => None,
        };
        if let Some(index) = skip {
            let property = self
                .filter()
                .ok_or(PointwiseError::InvalidCast)?
                .parameter(index)?;
            if property.is_animatable() || property.is_adjustable() {
                issues
                    .push("Genetica cannot read this Skip control's animation or adjustment data.");
            }
        }
        Ok(issues)
    }
    /// In-place node body load. The caller has already consumed its identity.
    /// Every completed property/slot operation remains committed on later error.
    #[allow(clippy::approx_constant)] // 3.14 is the original file-format revision.
    pub fn load_binary(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), NodeWireError<E>> {
        self.load_binary_with_context(reader, version, None)
    }
    /// Load an in-place node with a real service for wildcard variable options.
    pub fn load_binary_with_variable_service(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
        base: &[u16],
        service: &mut dyn crate::VariableFileService<Error = PointwiseError<E>>,
    ) -> Result<(), NodeWireError<E>> {
        let mut context = NodeWireReadContext::with_variables(None, base, service);
        self.load_binary_with_context(reader, version, Some(&mut context))
    }
    fn load_binary_with_context(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
        context: Option<&mut NodeWireReadContext<'_, E>>,
    ) -> Result<(), NodeWireError<E>> {
        self.load_binary_profile(reader, version, context, false)
    }

    #[allow(clippy::approx_constant)] // Original format revision.
    fn load_binary_profile(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
        mut context: Option<&mut NodeWireReadContext<'_, E>>,
        embedded_noise: bool,
    ) -> Result<(), NodeWireError<E>> {
        let count = reader.read_u8()?;
        if matches!(&self.core().payload, Payload::Solid { .. }) && version < 3.14 {
            self.load_solid_wire_legacy(reader, version, count)?;
        } else if self.kind() == Some(PointwiseKind::Synthesis) && version < 3.58 {
            self.filter()
                .ok_or(PointwiseError::InvalidCast)?
                .load_legacy_synthesis_properties(reader, version)?;
        } else if self.kind() == Some(PointwiseKind::Selector) && version < 3.14 {
            // ek.gx ignores count. Four reads complete before either setter.
            let _ = reader.read_string()?;
            let color = [reader.read_f64()?, reader.read_f64()?, reader.read_f64()?];
            let transparency = reader.read_f64()?;
            let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
            filter
                .parameter(0)?
                .set_color(crate::ContextColorValue::unlinked(color))?;
            filter.parameter(1)?.set_number(transparency)?;
            let _ = reader.read_bool()?;
            let _ = reader.read_bool()?;
            filter.parameter(2)?.load_binary(reader, version)?;
        } else {
            let mut property_index = 0;
            for _ in 0..count {
                if self.kind() == Some(PointwiseKind::ClearMaps)
                    && version < 3.1
                    && property_index == 0
                {
                    property_index += 1;
                }
                // No up-front schema check or rollback: index failure follows
                // every preceding real property load and its callbacks.
                let properties = self.wire_properties()?;
                let property = properties
                    .get(property_index)
                    .ok_or(NodeWireError::PropertyIndex)?;
                if embedded_noise && let Property::Filter(parameter) = property {
                    lab::disable_noise_animation(parameter);
                }
                if version == 3.75
                    && context
                        .as_ref()
                        .is_some_and(|read| read.legacy_mycela_layout)
                    && matches!(
                        (self.kind(), property_index),
                        (Some(PointwiseKind::ReflectEnvironment), 1)
                            | (Some(PointwiseKind::RefractEnvironment), 2)
                    )
                    && let Property::Filter(parameter) = property
                    && legacy_environment_skip(reader).unwrap_or(false)
                {
                    parameter.wire_metadata().set_capabilities(true, true);
                    property.load(reader, version, context.as_deref_mut())?;
                    if !parameter.axis_is_animated(0)? && parameter.retained_adjustment().is_none()
                    {
                        parameter.wire_metadata().set_capabilities(false, false);
                    }
                } else {
                    property.load(reader, version, context.as_deref_mut())?;
                }
                property_index += 1;
            }
        }
        if self.kind() == Some(PointwiseKind::WizardGroup) {
            let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
            filter.load_wizard_label(reader.read_string()?);
            let properties = filter.wizard_dynamic_properties()?;
            let count = reader.read_u8()?;
            if usize::from(count) != properties.len() {
                return Err(PointwiseError::Exception.into());
            }
            for property in properties {
                Property::Filter(property).load(reader, version, context.as_deref_mut())?;
            }
            if reader.read_u8()? != 0 {
                return Err(NodeWireError::UnsupportedCapability(
                    "Wizard Group child dynamic-property owners",
                ));
            }
        }
        if self.kind() == Some(PointwiseKind::MapVariable) {
            let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
            filter.load_wizard_label(reader.read_string()?);
            if reader.read_u8()? != 0 {
                return Err(PointwiseError::Exception.into());
            }
            if reader.read_u8()? != 0 {
                return Err(NodeWireError::UnsupportedCapability(
                    "Map Variable child dynamic-property owners",
                ));
            }
        }
        if matches!(
            self.kind(),
            Some(PointwiseKind::TemplateLab(_) | PointwiseKind::WizardReference)
        ) {
            let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
            filter.load_wizard_label(reader.read_string()?);
            lab::load_controls(self, reader, version, &mut context, embedded_noise)?;
        }
        if self.kind() == Some(PointwiseKind::Shader) {
            if !context
                .as_ref()
                .is_some_and(|read| read.is_mycela_project())
            {
                return Err(NodeWireError::UnsupportedCapability(
                    "Shader requires a Mycela project",
                ));
            }
            let imported = reader.read_bool()?;
            let source = String::from_utf16(&reader.read_string()?)
                .map_err(|_| PointwiseError::Shader("Shader source is invalid Unicode.".into()))?;
            let program = if imported {
                super::ShaderProgram::from_pixel_script(source)
            } else {
                super::ShaderProgram::compile(&source).map_err(PointwiseError::Shader)?
            };
            self.load_shader_program(std::sync::Arc::new(program));
        }
        let slots = reader.read_u8()?;
        let serialized = usize::from(slots);
        let constructed = self.wire_slot_count()?;
        if serialized != constructed
            && !(context
                .as_ref()
                .is_some_and(|read| read.legacy_mycela_layout)
                && self.kind().is_some_and(PointwiseKind::has_compact_inputs)
                && serialized == self.slots().len())
        {
            return Err(NodeWireError::SlotCountMismatch {
                serialized: slots,
                constructed,
            });
        }
        for index in 0..slots {
            let identity = reader.read_string()?;
            if identity.iter().copied().eq("null".encode_utf16()) {
                continue;
            }
            // Evaluate and HOLD this actual qi BEFORE recursive child loading.
            // A callback replacing the current slot list cannot retarget it.
            let slot = self.slot(i32::from(index))?;
            let child = factory::load_node_binary_with_context(
                reader,
                version,
                Some(&identity),
                context.as_deref_mut(),
            )?;
            slot.set_node(Some(&child))?;
        }
        if self.serializes_group_contents() {
            let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
            let count = if version >= 3.53 {
                reader.read_i32()?
            } else {
                1
            };
            let mut positions = filter.wizard_content_positions();
            for _ in 1..count {
                positions.push([reader.read_f32()?, reader.read_f32()?]);
            }
            filter.replace_wizard_content_positions(positions);
            let identity = reader.read_string()?;
            if !identity.iter().copied().eq("null".encode_utf16()) {
                let child = factory::load_node_binary_with_context(
                    reader,
                    version,
                    Some(&identity),
                    context.as_deref_mut(),
                )?;
                self.content(0)?.set_node(Some(&child))?;
            }
            for index in 1..count {
                let identity = reader.read_string()?;
                if identity.iter().copied().eq("null".encode_utf16()) {
                    let mut positions = filter.wizard_content_positions();
                    let index = usize::try_from(index - 1).map_err(|_| PointwiseError::Index)?;
                    if index >= positions.len() {
                        return Err(PointwiseError::Index.into());
                    }
                    positions.remove(index);
                    filter.replace_wizard_content_positions(positions);
                } else {
                    let slot = self.append_content(Vec::new());
                    let child = factory::load_node_binary_with_context(
                        reader,
                        version,
                        Some(&identity),
                        context.as_deref_mut(),
                    )?;
                    slot.set_node(Some(&child))?;
                }
            }
        }
        if self.kind() == Some(PointwiseKind::Group) && version >= 3.72 {
            let filter = self.filter().ok_or(PointwiseError::InvalidCast)?;
            let guid = reader
                .read_bytes(16)?
                .try_into()
                .map_err(|_| PointwiseError::Argument)?;
            filter.set_group_guid(guid);
            let presets = filter
                .group_presets()
                .ok_or(PointwiseError::NullReference)?;
            super::guard(&presets)
                .load(reader, version)
                .map_err(NodeWireError::Preset)?;
        }
        if self.kind() == Some(PointwiseKind::Canvas) {
            let document = super::canvas::CanvasDocument::read(reader, version, context)?;
            self.filter()
                .ok_or(PointwiseError::InvalidCast)?
                .set_canvas_document(document);
        }
        Ok(())
    }
}

#[cfg(test)]
mod ownership_laws {
    use super::*;
    use crate::PointwiseFilter;
    fn empty(kind: PointwiseKind) -> PointwiseNode {
        PointwiseFilter::new(kind).unwrap().node()
    }
    fn write(node: &PointwiseNode) -> BinaryWriter {
        let mut writer = BinaryWriter::default();
        node.save_binary(&mut writer).unwrap();
        writer
    }

    #[test]
    fn original_emboss_optics_properties_end_before_the_two_input_records() {
        let raw = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../genetica/Presets/Filter/Texture Effect/Print & Emboss/Emboss.gtx"
        ));
        let identity = b"\x13Reflect Environment";
        let start = raw
            .windows(identity.len())
            .position(|bytes| bytes == identity)
            .unwrap();
        let tail = b"\x03Ski\x03Yes\0\0\x02";
        let end = start
            + raw[start..]
                .windows(tail.len())
                .position(|bytes| bytes == tail)
                .unwrap()
            + tail.len();
        // Keep Genetica's complete actual property bytes and isolate the node
        // from its dependency graph to verify the raw source reader.
        let mut node = raw[start..end].to_vec();
        node.extend_from_slice(b"\x04null\x04null");
        let mut cursor = BinaryCursor::new(&node).unwrap();
        let identity = cursor.read_string().unwrap();
        let loaded: PointwiseNode = load_node_binary(&mut cursor, 3.75, Some(&identity)).unwrap();
        assert_eq!(cursor.position().unwrap() as usize, node.len());
        let filter = loaded.filter().unwrap();
        assert!(filter.parameter(0).unwrap().retained_adjustment().is_some());
        let skip = filter.parameter(1).unwrap();
        assert_eq!(skip.index(), 0);
        assert!(!skip.is_animatable());
        assert!(!skip.is_adjustable());
    }

    #[test]
    fn environment_optics_upgrade_legacy_skip_fields_and_retain_authored_controls() {
        use crate::Gtx375GraphDocument;

        for (kind, index) in [
            (PointwiseKind::ReflectEnvironment, 1),
            (PointwiseKind::RefractEnvironment, 2),
        ] {
            for (animated, adjusted) in [(false, false), (true, false), (false, true)] {
                let filter = PointwiseFilter::<()>::new(kind).unwrap();
                let skip = filter.parameter(index).unwrap();
                assert!(!skip.is_animatable());
                assert!(!skip.is_adjustable());
                skip.set_index(1).unwrap();
                let mut document = Gtx375GraphDocument::new(vec![Some(filter.node())]).unwrap();
                let mut original_layout = BinaryWriter::default();
                document.save(&mut original_layout).unwrap();
                assert!(
                    original_layout
                        .bytes()
                        .windows(11)
                        .any(|part| part == b"\x03Ski\x02No\0\0\x02\x04")
                );

                skip.wire_metadata().set_capabilities(true, true);
                if animated {
                    skip.set_axis_animated(0, true).unwrap();
                }
                if adjusted {
                    skip.adjustment()
                        .unwrap()
                        .unwrap()
                        .flags()
                        .set(0, true)
                        .unwrap();
                }
                let mut legacy = BinaryWriter::default();
                document.save(&mut legacy).unwrap();
                let mut reopened = Gtx375GraphDocument::<()>::load(legacy.bytes()).unwrap();
                assert_eq!(reopened.consumed() as usize, legacy.bytes().len());
                let skip = reopened
                    .branch(0)
                    .flatten()
                    .unwrap()
                    .filter()
                    .unwrap()
                    .parameter(index)
                    .unwrap();
                assert_eq!(skip.index(), 1);
                assert_eq!(skip.is_animatable(), animated || adjusted);
                assert_eq!(skip.axis_is_animated(0).unwrap(), animated);
                assert_eq!(skip.retained_adjustment().is_some(), adjusted);

                let mut saved = BinaryWriter::default();
                reopened.save(&mut saved).unwrap();
                assert_eq!(
                    saved.bytes(),
                    if animated || adjusted {
                        legacy.bytes()
                    } else {
                        original_layout.bytes()
                    }
                );
            }
        }
    }
    #[test]
    fn input_node_roundtrip_has_no_serialized_host_image() {
        let node = PointwiseNode::input();
        let image = crate::SharedImageMap::new(crate::ImageGeometry::new(2, 2));
        node.set_input_image(Some(image)).unwrap();
        let writer = write(&node);
        let mut reader = BinaryCursor::new(writer.bytes()).unwrap();
        let identity = reader.read_string().unwrap();
        let loaded: PointwiseNode = load_node_binary(&mut reader, 3.75, Some(&identity)).unwrap();
        assert_eq!(
            loaded.identity_name(),
            "Input".encode_utf16().collect::<Vec<_>>()
        );
        assert!(loaded.input_image().unwrap().is_none());
        assert_eq!(write(&loaded).bytes(), writer.bytes());
    }
    #[test]
    fn repeated_live_child_occurrences_load_as_fresh_independently_parented_owners() {
        let source = empty(PointwiseKind::Add);
        let child = empty(PointwiseKind::FixMapContrast);
        source.slot(0).unwrap().set_node(Some(&child)).unwrap();
        source.slot(1).unwrap().set_node(Some(&child)).unwrap();
        let writer = write(&source);
        let mut reader = BinaryCursor::new(writer.bytes()).unwrap();
        let identity = reader.read_string().unwrap();
        let loaded: PointwiseNode = load_node_binary(&mut reader, 3.75, Some(&identity)).unwrap();
        let a = loaded.slot(0).unwrap().node().unwrap();
        let b = loaded.slot(1).unwrap().node().unwrap();
        assert!(!a.same_reference(&b));
        assert!(a.parent().unwrap().same_reference(&loaded));
        assert!(b.parent().unwrap().same_reference(&loaded));
        assert_eq!(write(&loaded).bytes(), writer.bytes());
    }
    #[test]
    fn document_image_payloads_share_bytes_with_independent_edits() {
        use crate::{ByteBuffer, Gtx375GraphDocument, ImportedImageNode, RenderCaptureSession};

        let payload = |node: &PointwiseNode| {
            node.imported_image_handle()
                .unwrap()
                .parameter(ImageGeneratorParameterKind::Image)
                .custom_value()
                .unwrap()
                .unwrap()
                .imported_image()
                .unwrap()
        };
        let input = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        let branches = (0..3)
            .map(|index| {
                let node = ImportedImageNode::new().unwrap().node();
                let mut bytes = input;
                if index == 2 {
                    bytes[10] = 12;
                }
                payload(&node)
                    .set_bytes(Some(ByteBuffer::new(bytes.to_vec())))
                    .unwrap();
                Some(node)
            })
            .collect();
        let mut document = Gtx375GraphDocument::new(branches).unwrap();
        let policy = NodeWireReadPolicy::new(4, 8);
        for native in [false, true] {
            let save = |document: &mut Gtx375GraphDocument, writer: &mut BinaryWriter| {
                if native {
                    document.save_project(writer)
                } else {
                    document.save(writer)
                }
            };
            let mut original = BinaryWriter::default();
            save(&mut document, &mut original).unwrap();
            let mut loaded = if native {
                Gtx375GraphDocument::load_project(original.bytes(), policy)
            } else {
                Gtx375GraphDocument::load_bounded(original.bytes(), policy)
            }
            .unwrap();
            let mut saved = BinaryWriter::default();
            save(&mut loaded, &mut saved).unwrap();
            assert_eq!(original.bytes(), saved.bytes());

            let nodes: [_; 3] =
                std::array::from_fn(|index| loaded.branch(index).flatten().unwrap());
            let [first, second, distinct] = nodes
                .each_ref()
                .map(|node| payload(node).bytes().unwrap().unwrap());
            assert!(!first.same_reference(&second));
            assert!(std::sync::Arc::ptr_eq(
                &first.generation(),
                &second.generation()
            ));
            assert!(!std::sync::Arc::ptr_eq(
                &first.generation(),
                &distinct.generation()
            ));

            let before = RenderCaptureSession::new()
                .capture_node(&nodes[1], 0.0)
                .unwrap();
            second.write(10, 13).unwrap();
            assert_eq!(first.values(), input);
            assert_eq!(second.read(10).unwrap(), 13);
            assert_eq!(distinct.read(10).unwrap(), 12);
            let after = RenderCaptureSession::new()
                .capture_node(&nodes[1], 0.0)
                .unwrap();
            assert!(!before.same_render_values(&after));
        }
    }
    #[test]
    fn null_markers_preserve_in_place_child_and_its_actual_stale_parent_relationship() {
        let source = empty(PointwiseKind::Add);
        let target = empty(PointwiseKind::Add);
        let retained = empty(PointwiseKind::FixMapContrast);
        target.slot(0).unwrap().set_node(Some(&retained)).unwrap();
        let occurrence = target.slot(0).unwrap();
        let writer = write(&source);
        let mut reader = BinaryCursor::new(writer.bytes()).unwrap();
        reader.read_string().unwrap();
        target.load_binary(&mut reader, 3.75).unwrap();
        assert!(target.slot(0).unwrap().same_reference(&occurrence));
        assert!(occurrence.node().unwrap().same_reference(&retained));
        assert!(retained.parent().unwrap().same_reference(&target));
    }
    #[test]
    fn later_missing_factory_child_keeps_earlier_attachment_and_untouched_later_child() {
        let source = empty(PointwiseKind::Add);
        let first = empty(PointwiseKind::FixMapContrast);
        let missing = empty(PointwiseKind::MedianBlur);
        source.slot(0).unwrap().set_node(Some(&first)).unwrap();
        source.slot(1).unwrap().set_node(Some(&missing)).unwrap();
        let target = empty(PointwiseKind::Add);
        let old_first = empty(PointwiseKind::FixMapContrast);
        let old_later = empty(PointwiseKind::FixMapContrast);
        target.slot(0).unwrap().set_node(Some(&old_first)).unwrap();
        target.slot(1).unwrap().set_node(Some(&old_later)).unwrap();
        let writer = write(&source);
        let mut reader = BinaryCursor::new(writer.bytes()).unwrap();
        reader.read_string().unwrap();
        let error = target.load_binary(&mut reader, 3.75).unwrap_err();
        assert_eq!(error.parameter_name(), Some("className"));
        assert!(
            matches!(error, NodeWireError::FactoryArgument { class_name } if class_name == missing.identity_name())
        );
        let committed = target.slot(0).unwrap().node().unwrap();
        assert!(!committed.same_reference(&old_first));
        assert!(committed.parent().unwrap().same_reference(&target));
        assert!(
            target
                .slot(1)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&old_later)
        );
        assert!(old_first.parent().unwrap().same_reference(&target));
    }
}
