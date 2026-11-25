//! Convert a detached copy of an active Lab into editable graph nodes.
use super::{
    PointwiseError, PointwiseFilter, PointwiseKind, PointwiseNode, PointwiseParameterValue,
    PointwiseSlot, TemplateLabKind, graph::LabSimplifications, parameter_ops, template_lab,
};
use crate::{GradientGeneratorParameterKind, SolidColorProperty};
use std::collections::BTreeSet;

impl<E: 'static> PointwiseNode<E> {
    /// Convert the active template and its editable inputs on a detached copy.
    /// The caller can install the result in a single document history edit.
    pub(crate) fn converted_lab(&self) -> Result<Self, PointwiseError<E>> {
        if !matches!(self.kind(), Some(PointwiseKind::TemplateLab(_))) {
            return Err(PointwiseError::InvalidCast);
        }
        convert_template(self.source_clone()?)
    }
}

fn convert_template<E: 'static>(
    lab: PointwiseNode<E>,
) -> Result<PointwiseNode<E>, PointwiseError<E>> {
    let filter = lab.filter().ok_or(PointwiseError::InvalidCast)?;
    let PointwiseKind::TemplateLab(kind) = filter.kind() else {
        return Err(PointwiseError::InvalidCast);
    };
    fold_lab_modifiers(&filter)?;
    if let Some(principal) = lab.content(0)?.node() {
        commit_subtree(&principal)?;
    }
    let name = kind
        .template()
        .name
        .replace(" Lab", "")
        .replace("Select ", "");
    let group = build_group(&lab, &name.encode_utf16().collect::<Vec<_>>())?;
    simplify_contents(&group)?;
    group.clear_mode_subtree(&mut BTreeSet::new())?;
    Ok(group)
}

fn fold_lab_modifiers<E: 'static>(lab: &PointwiseFilter<E>) -> Result<(), PointwiseError<E>> {
    let (scale, seed) = lab.lab_inheritance();
    if scale != 1.0 {
        let control = lab.parameter(template_lab::SCALE)?;
        lab.set_lab_inherited_scale(1.0);
        control.set_number(template_lab::scale_control_value(
            scale * template_lab::scale_multiplier(control.number()),
        ))?;
    }
    if seed != 0.0 {
        let control = lab.parameter(template_lab::SEED)?;
        lab.set_lab_inherited_seed(0.0);
        control.set_number(parameter_ops::seed_modifier(control.number(), seed, true))?;
    }
    Ok(())
}

fn commit_subtree<E: 'static>(root: &PointwiseNode<E>) -> Result<(), PointwiseError<E>> {
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        let is_lab = matches!(node.kind(), Some(PointwiseKind::TemplateLab(_)));
        if is_lab {
            fold_lab_modifiers(&node.filter().ok_or(PointwiseError::InvalidCast)?)?;
        } else {
            commit_local(&node)?;
            pending.extend(
                node.contents()
                    .into_iter()
                    .rev()
                    .filter_map(|slot| slot.node()),
            );
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

fn commit_local<E: 'static>(node: &PointwiseNode<E>) -> Result<(), PointwiseError<E>> {
    if let Some(generator) = node.image_generator_handle() {
        for property in generator.parameters() {
            property.set_binding_index(-1);
            property.set_binding_name(None);
        }
        let scale = generator.factor();
        if scale != 1.0 {
            let [x, y] = generator.normalized_size();
            let size = generator.parameter(GradientGeneratorParameterKind::Size);
            generator.set_local_node_scale(1.0)?;
            size.set_axis(0, x)?;
            size.set_axis(1, y)?;
            if generator.kind() == crate::ImageGeneratorKind::BricksBoards {
                generator.commit_bricks_scale(scale)?;
            }
        }
        if let Some(seed) = generator
            .parameters()
            .into_iter()
            .find(|p| p.kind() == GradientGeneratorParameterKind::Seed)
            && seed.modifier()? != 0.0
        {
            let value = seed.derived_seed()?;
            seed.set_modifier(0.0)?;
            seed.set_number(value)?;
        }
    } else if let Some(filter) = node.filter() {
        for property in filter.parameters() {
            property.set_binding_index(-1);
            property.set_binding_name(None);
            if property.name().into_iter().eq("Seed".encode_utf16())
                && matches!(property.value(), PointwiseParameterValue::Number { .. })
                && property.seed_modifier() != 0.0
            {
                let value = parameter_ops::seed_modifier(
                    property.number(),
                    property.seed_modifier(),
                    property.limits()?.snapshot().minimum.to_framework_f64() < 0.0,
                );
                property.set_seed_modifier(0.0)?;
                property.set_number(value)?;
            }
        }
        let scale = filter.factor();
        if scale != 1.0 {
            if let Ok(size) = filter.parameter(0)
                && size.size_scale().is_ok_and(|value| value != 1.0)
            {
                let [x, y] = size.normalized_pair()?;
                size.set_size_scale(1.0, Some(&[false; 20]))?;
                size.set_axis(0, x)?;
                size.set_axis(1, y)?;
            }
            filter.commit_spatial_scale(scale)?;
            filter.set_local_factor(1.0);
        }
    } else if matches!(&node.core().payload, super::graph::Payload::Solid { .. }) {
        for property in [SolidColorProperty::Color, SolidColorProperty::Transparency] {
            node.set_solid_binding_index(property, -1)?;
            node.set_solid_binding_mapping(property, None)?;
        }
    }
    Ok(())
}

fn build_group<E: 'static>(
    source: &PointwiseNode<E>,
    name: &[u16],
) -> Result<PointwiseNode<E>, PointwiseError<E>> {
    let inputs = source.reference_inputs()?;
    let filter = PointwiseFilter::new(PointwiseKind::Group)?;
    filter.parameter(0)?.set_number(inputs.len() as f64)?;
    filter.parameter(2)?.set_text(Some(name))?;
    let group = filter.node();
    group
        .content(0)?
        .set_node_quiet(take_node(&source.content(0)?)?.as_ref())?;
    for (from, to) in inputs.into_iter().zip(group.slots()) {
        if let Some(child) = take_node(&from)?.filter(|node| !node.is_default_input()) {
            to.set_node_quiet(Some(&child))?;
        }
    }
    Ok(group)
}

fn take_node<E: 'static>(
    slot: &PointwiseSlot<E>,
) -> Result<Option<PointwiseNode<E>>, PointwiseError<E>> {
    let node = slot.node();
    slot.set_node_quiet(None)?;
    Ok(node)
}

fn simplify_contents<E: 'static>(group: &PointwiseNode<E>) -> Result<(), PointwiseError<E>> {
    let principal = group.content(0)?;
    if let Some(node) = principal.node() {
        let prepared = LabSimplifications::prepare(group)?;
        let node = simplify(node, &prepared)?;
        principal.set_node_quiet(node.as_ref())?;
    }
    collapse_principal_groups(group)
}

fn is_reference_chain<E: 'static>(mut node: PointwiseNode<E>) -> bool {
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(node.id) || node.kind() == Some(PointwiseKind::Reference) {
            return true;
        }
        let slots = node.slots();
        let [slot] = slots.as_slice() else {
            return false;
        };
        let Some(child) = slot.node() else {
            return false;
        };
        node = child;
    }
}

fn simplify<E: 'static>(
    mut node: PointwiseNode<E>,
    prepared: &LabSimplifications,
) -> Result<Option<PointwiseNode<E>>, PointwiseError<E>> {
    if let Some(values) = prepared.uniform(node.id)
        && !is_reference_chain(node.clone())
    {
        // zl.a4 folds the original branch before converting the wizard properties.
        if values == [0.; 4] {
            return Ok(None);
        }
        let replacement = PointwiseNode::solid_color()?;
        for (index, value) in values.into_iter().enumerate() {
            replacement
                .set_solid_channel(index as i32, value)
                .map_err(|_| PointwiseError::InvalidOperation)?;
        }
        return Ok(Some(replacement));
    }
    let parent = node.parent();
    while node.kind() == Some(PointwiseKind::ChangeBci)
        && prepared
            .channels(node.id)
            .and_then(|channels| channels.passthrough_slot())
            == Some(0)
    {
        let Some(input) = take_node(&node.slot(0)?)? else {
            return Ok(None);
        };
        node = input;
    }
    if redundant_intensity(&node, parent.as_ref(), prepared)? {
        let Some(input) = take_node(&node.slot(0)?)? else {
            return Ok(None);
        };
        node = input;
    }
    match node.kind() {
        Some(PointwiseKind::TemplateLab(
            TemplateLabKind::Rings | TemplateLabKind::SelectRings | TemplateLabKind::SelectArea,
        )) => return convert_template(node).map(Some),
        Some(
            PointwiseKind::WizardGroup
            | PointwiseKind::WizardReference
            | PointwiseKind::MapVariable,
        ) => {
            let filter = node.filter().ok_or(PointwiseError::InvalidCast)?;
            let name = filter.parameter(2)?.text()?.unwrap_or_default();
            node = build_group(&node, &name)?;
        }
        Some(
            PointwiseKind::WizardColor
            | PointwiseKind::WizardFiveColor
            | PointwiseKind::WizardSlideColor,
        ) => {
            node = convert_palette(&node)?;
        }
        Some(
            PointwiseKind::WizardToneCurve
            | PointwiseKind::WizardMatrix
            | PointwiseKind::WizardKaleidoscope,
        ) => {
            node = convert_wizard_filter(&node)?;
        }
        _ => {}
    }
    for slot in node.slots() {
        if let Some(child) = slot.node() {
            slot.set_node_quiet(simplify(child, prepared)?.as_ref())?;
        }
    }
    if !matches!(node.kind(), Some(PointwiseKind::TemplateLab(_))) {
        for slot in node.contents() {
            if let Some(child) = slot.node() {
                slot.set_node_quiet(simplify(child, prepared)?.as_ref())?;
            }
        }
    }
    if node.kind() == Some(PointwiseKind::Group) {
        collapse_principal_groups(&node)?;
    }
    Ok(Some(node))
}

// ab.h: An Auto Fix parent can rescale its original in-range image directly
// when an intervening static Change BCI only reduces intensity.
fn redundant_intensity<E: 'static>(
    node: &PointwiseNode<E>,
    parent: Option<&PointwiseNode<E>>,
    prepared: &LabSimplifications,
) -> Result<bool, PointwiseError<E>> {
    let bci = |node: &PointwiseNode<E>| {
        node.filter()
            .filter(|filter| filter.kind() == PointwiseKind::ChangeBci)
    };
    let (Some(filter), Some(parent)) = (bci(node), parent.and_then(bci)) else {
        return Ok(false);
    };
    let parent_auto = parent.parameter(0)?;
    if parent_auto.index() != 0 || parameter_ops::is_animated(|| parent_auto.retained_animation())?
    {
        return Ok(false);
    }
    let [auto, bright, contrast, intensity] = [
        filter.parameter(0)?,
        filter.parameter(1)?,
        filter.parameter(2)?,
        filter.parameter(3)?,
    ];
    let reduction = intensity.number();
    if auto.index() != 1
        || bright.number() != 0.
        || contrast.number() != 0.
        || !(reduction > -100. && reduction < 0.)
    {
        return Ok(false);
    }
    for property in [&auto, &bright, &contrast, &intensity] {
        if parameter_ops::is_animated(|| property.retained_animation())? {
            return Ok(false);
        }
    }
    let Some(input) = node.slot(0)?.node() else {
        return Ok(true);
    };
    Ok(prepared
        .channels(input.id)
        .is_some_and(|channels| (0..3).all(|channel| channels.effective_range(channel))))
}

fn collapse_principal_groups<E: 'static>(
    outer: &PointwiseNode<E>,
) -> Result<(), PointwiseError<E>> {
    let outer_filter = outer.filter().ok_or(PointwiseError::InvalidCast)?;
    let principal = outer.content(0)?;
    while let Some(inner) = principal
        .node()
        .filter(|node| node.kind() == Some(PointwiseKind::Group))
    {
        let inner_filter = inner.filter().ok_or(PointwiseError::InvalidCast)?;
        let inputs = inner.reference_inputs()?;
        let outer_inputs = outer.reference_inputs()?;
        if inputs.len() > outer_inputs.len()
            || inner.contents().len() != 1
            || inner_filter.parameter(1)?.number() != 1.0
            || inner_filter.parameter(3)?.index() != 1
            || inner_filter.parameter(4)?.index() != 1
        {
            break;
        }
        let mut passthrough = true;
        for (index, slot) in inputs.iter().enumerate() {
            let Some(reference) = slot
                .node()
                .filter(|node| node.kind() == Some(PointwiseKind::Reference))
            else {
                passthrough = false;
                break;
            };
            let property = reference
                .filter()
                .ok_or(PointwiseError::InvalidCast)?
                .parameter(0)?;
            if property.axis_is_animated(0)? || reference.reference_slot_at(None)? != Some(index) {
                passthrough = false;
                break;
            }
        }
        if !passthrough {
            break;
        }
        for slot in outer_inputs.iter().skip(inputs.len()) {
            slot.set_node_quiet(None)?;
        }
        outer_filter.parameter(0)?.set_number(inputs.len() as f64)?;
        let name = outer_filter.parameter(2)?;
        if name.text()?.as_deref().unwrap_or_default().is_empty() {
            name.set_text(inner_filter.parameter(2)?.text()?.as_deref())?;
        }
        principal.set_node_quiet(take_node(&inner.content(0)?)?.as_ref())?;
    }
    Ok(())
}

fn convert_palette<E: 'static>(
    source: &PointwiseNode<E>,
) -> Result<PointwiseNode<E>, PointwiseError<E>> {
    let filter = source.filter().ok_or(PointwiseError::InvalidCast)?;
    let stops: Vec<(f64, i32)> = match filter.kind() {
        PointwiseKind::WizardColor => vec![(0.0, 0), (1.0, 1)],
        PointwiseKind::WizardFiveColor => (0..5)
            .map(|index| (f64::from(index) / 4.0, index))
            .collect(),
        PointwiseKind::WizardSlideColor => match filter.parameter(3)?.number() {
            midpoint if midpoint <= 0.0 => vec![(0.0, 2), (1.0, 1)],
            midpoint if midpoint >= 1.0 => vec![(0.0, 0), (1.0, 2)],
            midpoint => vec![(0.0, 0), (midpoint, 2), (1.0, 1)],
        },
        _ => return Err(PointwiseError::InvalidCast),
    };
    let gradient = crate::SharedGradient::new();
    for (position, index) in stops {
        let [r, g, b] = filter.parameter(index)?.color()?.components();
        gradient.add_normalized_color(position, r, g, b)?;
    }
    let colorize = PointwiseFilter::new(PointwiseKind::Colorize)?;
    colorize.parameter(0)?.set_gradient(Some(gradient))?;
    let node = colorize.node();
    node.slot(0)?
        .set_node_quiet(take_node(&source.slot(0)?)?.as_ref())?;
    Ok(node)
}

fn convert_wizard_filter<E: 'static>(
    source: &PointwiseNode<E>,
) -> Result<PointwiseNode<E>, PointwiseError<E>> {
    let filter = source.filter().ok_or(PointwiseError::InvalidCast)?;
    let target = match filter.kind() {
        PointwiseKind::WizardToneCurve => {
            let target = PointwiseFilter::new(PointwiseKind::ToneCurve)?;
            super::tone_curve::install_arc(
                &target,
                filter.parameter(0)?.number(),
                (filter.parameter(1)?.number() * 100.0) as f32,
                (filter.parameter(2)?.number() * 100.0) as f32,
            )?;
            for index in 0..4 {
                target
                    .parameter(index + 1)?
                    .set_index(filter.parameter(index + 3)?.index())?;
            }
            target
        }
        PointwiseKind::WizardMatrix => {
            let target = PointwiseFilter::new(PointwiseKind::Matrix)?;
            let old = filter
                .parameter(0)?
                .matrix()?
                .ok_or(PointwiseError::InvalidCast)?;
            let new = target
                .parameter(0)?
                .matrix()?
                .ok_or(PointwiseError::InvalidCast)?;
            new.payload()
                .copy_from(Some(&old.payload()))
                .map_err(|_| PointwiseError::InvalidOperation)?;
            new.payload().set_radius(filter.parameter(2)?.number());
            new.payload().set_angle(filter.parameter(3)?.number());
            new.notify()?;
            target
                .parameter(1)?
                .set_number(filter.parameter(1)?.number())?;
            target
        }
        PointwiseKind::WizardKaleidoscope => {
            let target = PointwiseFilter::new(PointwiseKind::Kaleidoscope)?;
            let region = target.parameter(1)?.region()?;
            region.set_mode(crate::framework_cast::integer(
                filter.parameter(2)?.number(),
            ));
            region.set_x((1.0 - 0.8 * region.aspect()) * 0.5);
            region.set_y(0.1);
            region.set_size(0.8);
            region.notify()?;
            target
        }
        _ => return Err(PointwiseError::InvalidCast),
    };
    let node = target.node();
    node.slot(0)?
        .set_node_quiet(take_node(&source.slot(0)?)?.as_ref())?;
    Ok(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GradientGenerator;

    #[test]
    fn lab_simplification_removes_only_redundant_change_bci() {
        for (intensity, auto_fix, use_hdr, animated, kept_inner) in [
            (-40., true, false, false, false),
            (0., true, false, false, false),
            (-100., true, false, false, true),
            (-40., false, false, false, true),
            (-40., true, true, false, true),
            (-40., true, false, true, true),
        ] {
            let group = PointwiseFilter::<()>::new(PointwiseKind::Group)
                .unwrap()
                .node();
            let outer = PointwiseFilter::new(PointwiseKind::ChangeBci).unwrap();
            let inner = PointwiseFilter::new(PointwiseKind::ChangeBci).unwrap();
            let source = GradientGenerator::new().unwrap().node();
            let original = if use_hdr {
                let colorize = PointwiseFilter::new(PointwiseKind::Colorize).unwrap();
                let gradient = colorize.parameter(0).unwrap().gradient().unwrap();
                gradient.clear_points().unwrap();
                gradient.add_normalized_color(0., 0.2, 0.2, 0.3).unwrap();
                gradient.add_normalized_color(1., 1.5, 0.2, 0.3).unwrap();
                let node = colorize.node();
                node.slot(0).unwrap().set_node(Some(&source)).unwrap();
                node
            } else {
                source
            };
            outer
                .parameter(0)
                .unwrap()
                .set_index(i32::from(!auto_fix))
                .unwrap();
            let intensity_parameter = inner.parameter(3).unwrap();
            intensity_parameter.set_number(intensity).unwrap();
            if animated {
                intensity_parameter.set_axis_animated(0, true).unwrap();
            }
            inner
                .node()
                .slot(0)
                .unwrap()
                .set_node(Some(&original))
                .unwrap();
            outer
                .node()
                .slot(0)
                .unwrap()
                .set_node(Some(&inner.node()))
                .unwrap();
            group
                .content(0)
                .unwrap()
                .set_node(Some(&outer.node()))
                .unwrap();

            simplify_contents(&group).unwrap();
            let output = group.content(0).unwrap().node().unwrap();
            assert_eq!(output.kind(), Some(PointwiseKind::ChangeBci));
            if !auto_fix {
                assert!(output.same_reference(&inner.node()));
            } else if !kept_inner {
                assert!(
                    output
                        .slot(0)
                        .unwrap()
                        .node()
                        .unwrap()
                        .same_reference(&original)
                );
            } else {
                assert!(
                    output
                        .slot(0)
                        .unwrap()
                        .node()
                        .unwrap()
                        .same_reference(&inner.node())
                );
            }
        }
    }
}
