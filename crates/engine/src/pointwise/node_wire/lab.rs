//! alz template acquisition and the nested aqa control records.
use super::{NodeWireError, NodeWireReadContext, Property};
use crate::{
    BinaryCursor, BinaryWriter, Gtx375GraphDocument, PointwiseError, PointwiseFilter,
    PointwiseKind, PointwiseNode, PointwiseParameter, TemplateLabKind,
};
use std::collections::BTreeSet;
use std::sync::Arc;

#[path = "lab_noise.rs"]
mod noise;

enum WizardTemplate {
    Native(&'static str),
    Bundled(&'static str, &'static [u8]),
}

impl WizardTemplate {
    fn resolve(path: &str) -> Option<Self> {
        const NOISE_CHOICES: &[&str] = &["None", "Use Input", "3D Noise", "Matrix Noise", "Cells"];
        if let Some(key) = NOISE_CHOICES.iter().copied().find(|key| *key == path) {
            return Some(Self::Native(key));
        }
        super::lab_assets::read(path).map(|(key, bytes)| Self::Bundled(key, bytes))
    }

    fn key(&self) -> &'static str {
        match self {
            Self::Native(key) | Self::Bundled(key, _) => key,
        }
    }
}

pub(in crate::pointwise) fn read_style_noise_lab<E: 'static>(
    reader: &mut BinaryCursor<'_>,
    version: f64,
    context: Option<&mut NodeWireReadContext<'_, E>>,
) -> Result<PointwiseNode<E>, NodeWireError<E>> {
    let identity = reader.read_string()?;
    if !identity.iter().copied().eq("Noise Lab".encode_utf16()) {
        return Err(NodeWireError::UnsupportedCapability(
            "Style Noise Lab identity",
        ));
    }
    let mut local_context = NodeWireReadContext::for_raw_node();
    let context = context.unwrap_or(&mut local_context);
    context.enter()?;
    let result = (|| {
        let node = super::factory::create_versioned_node(Some(&identity), version, Some(context))?;
        node.load_binary_profile(reader, version, Some(context), true)?;
        Ok(node)
    })();
    context.leave();
    result
}

pub(super) fn disable_noise_animation<E: 'static>(parameter: &PointwiseParameter<E>) {
    let wire = parameter.wire_metadata();
    wire.set_capabilities(false, wire.adjustable());
}

pub(in crate::pointwise) fn default_style_noise_lab() -> Result<Arc<[u8]>, NodeWireError<()>> {
    let identity = "Noise Lab".encode_utf16().collect::<Vec<_>>();
    let node = super::factory::create_node_by_identity::<()>(Some(&identity))?;
    write_style_noise_lab(&node)
}

pub(in crate::pointwise) fn write_style_noise_lab<E: 'static>(
    node: &PointwiseNode<E>,
) -> Result<Arc<[u8]>, NodeWireError<E>> {
    if node.kind() != Some(PointwiseKind::TemplateLab(TemplateLabKind::Noise)) {
        return Err(NodeWireError::UnsupportedCapability(
            "Style Noise Lab identity",
        ));
    }
    let filter = node.filter().ok_or(PointwiseError::InvalidCast)?;
    for parameter in filter.parameters() {
        disable_noise_animation(&parameter);
    }
    let mut owners = child_owners(node)?;
    while let Some(owner) = owners.pop() {
        for parameter in owner_properties(&owner)? {
            disable_noise_animation(&parameter);
        }
        owners.extend(child_owners(&owner)?);
    }
    let mut writer = BinaryWriter::default();
    node.save_binary(&mut writer)?;
    Ok(writer.bytes().into())
}

fn owns_lab_controls(kind: PointwiseKind) -> bool {
    matches!(
        kind,
        PointwiseKind::TemplateLab(_)
            | PointwiseKind::WizardGroup
            | PointwiseKind::MapVariable
            | PointwiseKind::WizardReference
    )
}

pub(super) fn attach_template<E: 'static>(
    node: &PointwiseNode<E>,
    lab: TemplateLabKind,
    context: &mut NodeWireReadContext<'_, E>,
) -> Result<(), NodeWireError<E>> {
    if matches!(lab, TemplateLabKind::Noise | TemplateLabKind::SelectNoise) {
        let principal = noise::template(lab, context)?;
        node.content(0)?.set_node_quiet(principal.as_ref())?;
    } else if let Some(path) = lab.template().contents {
        let (path, bytes) = super::lab_assets::read(path)
            .ok_or_else(|| NodeWireError::MissingLabTemplate(path.into()))?;
        let principal = read_principal(path, bytes, context)?;
        node.content(0)?.set_node_quiet(principal.as_ref())?;
    }
    Ok(())
}

fn read_principal<E: 'static>(
    path: &'static str,
    bytes: &[u8],
    context: &mut NodeWireReadContext<'_, E>,
) -> Result<Option<PointwiseNode<E>>, NodeWireError<E>> {
    if context.active_templates.contains(&path) {
        return Err(NodeWireError::UnsupportedCapability(
            "recursive Lab template",
        ));
    }
    let mut reader = BinaryCursor::new(bytes)?;
    context.active_templates.push(path);
    let principal = Gtx375GraphDocument::read_with_context(&mut reader, bytes.len(), context)
        .map(|document| document.branch(0).flatten());
    context.active_templates.pop();
    let principal = principal.map_err(|error| NodeWireError::LabTemplate {
        path,
        position: reader.position().unwrap_or(0),
        error: Box::new(error),
    })?;
    if let Some(root) = &principal
        && root.kind().is_some_and(owns_lab_controls)
    {
        // x0.s supplies numbered References for the template's exterior inputs.
        for (index, slot) in root.slots().into_iter().enumerate() {
            let reference = PointwiseFilter::new(PointwiseKind::Reference)?;
            reference.parameter(0)?.set_number((index + 1) as f64)?;
            slot.set_node_quiet(Some(&reference.node()))?;
        }
    }
    Ok(principal)
}

pub(in crate::pointwise) fn switch_wizard_template<E: 'static>(
    reference: &PointwiseFilter<E>,
    index: i32,
) -> Result<(), PointwiseError<E>> {
    // Only templates provided by Mycela can be selected here.
    let mut context = NodeWireReadContext::for_raw_node();
    select_template(reference, index, &mut context).map_err(|error| match error {
        NodeWireError::Node(error) => error,
        NodeWireError::MissingLabTemplate(path) => PointwiseError::TemplateResource(path),
        NodeWireError::LabTemplate { path, position, .. } => {
            PointwiseError::TemplateResource(format!("{path} at byte {position}"))
        }
        _ => PointwiseError::UnsupportedCapability("Wizard Reference template selection"),
    })
}

fn select_template<E: 'static>(
    reference: &PointwiseFilter<E>,
    index: i32,
    context: &mut NodeWireReadContext<'_, E>,
) -> Result<(), NodeWireError<E>> {
    let crate::PointwiseParameterValue::Variable(options) = reference.parameter(3)?.value() else {
        return Err(PointwiseError::InvalidCast.into());
    };
    let path = options
        .resolved_options()
        .and_then(|options| {
            usize::try_from(index)
                .ok()
                .and_then(|index| options.get(index))
        })
        .ok_or(PointwiseError::Index)?
        .1
        .as_ref()
        .map(|path| {
            let path = String::from_utf16(path).map_err(|_| PointwiseError::Argument)?;
            WizardTemplate::resolve(&path).ok_or(NodeWireError::MissingLabTemplate(path))
        })
        .transpose()?;
    let path_key = path.as_ref().map(WizardTemplate::key);
    if reference.wizard_template() == path_key {
        return Ok(());
    }
    let node = reference.node();
    let slot = node.content(0)?;
    let previous = slot.node();
    let principal = if let Some(path) = path {
        if let Some(id) = reference.cached_wizard_template(path.key()) {
            Some(PointwiseNode {
                id,
                sponsor: node.sponsor.clone(),
            })
        } else {
            match path {
                WizardTemplate::Native(key) => noise::choice(key, context)?,
                WizardTemplate::Bundled(key, bytes) => read_principal(key, bytes, context)?,
            }
        }
    } else {
        None
    };
    // Apply inherited control values before publishing the selected occurrence.
    if let Some(principal) = &principal {
        let (_, seed) = reference.lab_inheritance();
        principal.set_graph_scale(reference.factor())?;
        principal.set_graph_seed_modifier(seed)?;
    }
    if let Some(previous) = &previous {
        previous.clear_mode_subtree(&mut BTreeSet::new())?;
    }
    slot.set_node_quiet(principal.as_ref())?;
    reference.remember_wizard_template(path_key, previous.as_ref().map(|node| node.id));
    Ok(())
}

fn owner_properties<E>(
    node: &PointwiseNode<E>,
) -> Result<Vec<PointwiseParameter<E>>, NodeWireError<E>> {
    let filter = node.filter().ok_or(PointwiseError::InvalidCast)?;
    Ok(match filter.kind() {
        PointwiseKind::TemplateLab(_) => filter.parameters().into_iter().skip(3).collect(),
        PointwiseKind::WizardGroup | PointwiseKind::WizardReference => {
            filter.wizard_dynamic_properties()?
        }
        PointwiseKind::MapVariable => Vec::new(),
        _ => return Err(PointwiseError::InvalidCast.into()),
    })
}

impl<E: 'static> PointwiseNode<E> {
    /// Direct Lab owners in the order used by Genetica's nested control stream.
    /// Only contents are searched. Exterior slots belong to the enclosing Lab.
    pub fn lab_control_children(&self) -> Result<Vec<Self>, PointwiseError<E>> {
        if !self.kind().is_some_and(owns_lab_controls) {
            return Ok(Vec::new());
        }
        child_owners(self)
    }
}

fn child_owners<E: 'static>(
    owner: &PointwiseNode<E>,
) -> Result<Vec<PointwiseNode<E>>, PointwiseError<E>> {
    fn collect<E: 'static>(
        node: PointwiseNode<E>,
        active: &mut BTreeSet<crate::pointwise::region::NodeId>,
        owners: &mut Vec<PointwiseNode<E>>,
    ) -> Result<(), PointwiseError<E>> {
        if !active.insert(node.id) {
            return Err(PointwiseError::UnsupportedCapability("cyclic Lab contents"));
        }
        for child in node.slots().into_iter().filter_map(|slot| slot.node()) {
            collect(child, active, owners)?;
        }
        // x0.v8 traverses Slots into the enclosing owner, adds itself, then
        // starts a new owner scope for Contents. zl.v8 keeps the same owner.
        if node.kind().is_some_and(owns_lab_controls) {
            active.remove(&node.id);
            owners.push(node);
        } else {
            for child in node.contents().into_iter().filter_map(|slot| slot.node()) {
                collect(child, active, owners)?;
            }
            active.remove(&node.id);
        }
        Ok(())
    }

    let mut owners = Vec::new();
    let mut active = BTreeSet::from([owner.id]);
    for child in owner.contents().into_iter().filter_map(|slot| slot.node()) {
        collect(child, &mut active, &mut owners)?;
    }
    Ok(owners)
}

pub(super) fn save_controls<E: 'static>(
    owner: &PointwiseNode<E>,
    writer: &mut BinaryWriter,
) -> Result<(), NodeWireError<E>> {
    let properties = owner_properties(owner)?;
    writer.write_u8(u8::try_from(properties.len()).map_err(|_| PointwiseError::Arithmetic)?)?;
    for property in properties {
        property.save_binary(writer)?;
    }
    let owners = child_owners(owner)?;
    writer.write_u8(u8::try_from(owners.len()).map_err(|_| PointwiseError::Arithmetic)?)?;
    for child in owners {
        save_controls(&child, writer)?;
    }
    Ok(())
}

pub(super) fn load_controls<E: 'static>(
    owner: &PointwiseNode<E>,
    reader: &mut BinaryCursor<'_>,
    version: f64,
    context: &mut Option<&mut NodeWireReadContext<'_, E>>,
    embedded_noise: bool,
) -> Result<(), NodeWireError<E>> {
    let properties = owner_properties(owner)?;
    if usize::from(reader.read_u8()?) != properties.len() {
        return Err(NodeWireError::UnsupportedCapability(
            "Lab template control layout",
        ));
    }
    let reference = owner
        .filter()
        .filter(|filter| filter.kind() == PointwiseKind::WizardReference);
    let selection = properties.first().filter(|_| reference.is_some()).cloned();
    for property in properties {
        if embedded_noise {
            disable_noise_animation(&property);
        }
        Property::Filter(property).load(reader, version, context.as_deref_mut())?;
    }
    if let (Some(reference), Some(selection)) = (reference, selection) {
        match context.as_deref_mut() {
            Some(context) => select_template(&reference, selection.index(), context)?,
            None => select_template(
                &reference,
                selection.index(),
                &mut NodeWireReadContext::for_raw_node(),
            )?,
        }
    }
    let owners = child_owners(owner)?;
    if usize::from(reader.read_u8()?) != owners.len() {
        return Err(NodeWireError::UnsupportedCapability(
            "Lab template child control layout",
        ));
    }
    for child in owners {
        load_controls(&child, reader, version, context, embedded_noise)?;
    }
    Ok(())
}
