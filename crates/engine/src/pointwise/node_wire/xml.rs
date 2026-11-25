//! XML-era property and slot readers. Node identities and post-load migrations
//! share the binary factory, while the old property names and layouts stay here.
use super::{NodeWireError, NodeWireReadContext, factory, lab_assets::LabFiles};
use crate::{
    ContextColorValue, ImageGenerator, ImageGeneratorKind, LegacyColorGradient,
    PointwiseCustomObject, PointwiseError, PointwiseFilter, PointwiseKind, PointwiseNode,
    PointwiseParameter, PointwiseParameterValue, SharedLimits, VariableSpec, VariableSpecError,
};
use roxmltree::Node;
use std::str::FromStr;

type Element<'a, 'input> = Node<'a, 'input>;

fn invalid<E>(message: impl Into<String>) -> NodeWireError<E> {
    NodeWireError::LegacyXml(message.into())
}

fn attr<'a, E>(element: Element<'a, '_>, name: &str) -> Result<&'a str, NodeWireError<E>> {
    element
        .attribute(name)
        .ok_or_else(|| invalid(format!("Missing {name} on {}", element.tag_name().name())))
}

fn number<T: FromStr, E>(element: Element<'_, '_>, name: &str) -> Result<T, NodeWireError<E>> {
    attr(element, name)?
        .trim()
        .parse()
        .map_err(|_| invalid(format!("Invalid {name} on {}", element.tag_name().name())))
}

fn legacy_decimal<T: FromStr, E>(
    element: Element<'_, '_>,
    name: &str,
) -> Result<T, NodeWireError<E>> {
    let text = attr(element, name)?.trim();
    let invalid_value = || invalid(format!("Invalid {name} on {}", element.tag_name().name()));
    if !text.contains(',') {
        return text.parse().map_err(|_| invalid_value());
    }
    // CLR floating-point parsing accepts grouping in the integer part of the
    // default decimal format, including a group whose size is not three.
    let integral_end = text.find(['.', 'e', 'E']).unwrap_or(text.len());
    let (integral, remainder) = text.split_at(integral_end);
    let digits = integral.trim_start_matches(['-', '+']);
    if !digits.as_bytes().first().is_some_and(u8::is_ascii_digit)
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b',')
        || remainder.contains(',')
    {
        return Err(invalid_value());
    }
    let mut normalized = integral.replace(',', "");
    normalized.push_str(remainder);
    normalized.parse().map_err(|_| invalid_value())
}

fn child<'a, 'input>(element: Element<'a, 'input>, name: &str) -> Option<Element<'a, 'input>> {
    element.children().find(|node| node.has_tag_name(name))
}

fn property_elements<'a, 'input>(
    element: Element<'a, 'input>,
) -> impl Iterator<Item = Element<'a, 'input>> {
    element.children().flat_map(|list| {
        list.children().filter(move |property| {
            (list.has_tag_name("Ps") && property.has_tag_name("P"))
                || (list.has_tag_name("Properties") && property.has_tag_name("Property"))
        })
    })
}

fn named_property<E>(
    element: Element<'_, '_>,
    name: &str,
    kind: Option<PointwiseKind>,
    version: f64,
) -> Result<(), NodeWireError<E>> {
    let found = attr(element, "name")?;
    let old_effect = kind == Some(PointwiseKind::ToneCurveG2)
        && (2.0..=2.02).contains(&version)
        && found.starts_with("Effect")
        && name.starts_with("Affect");
    if found == name || old_effect {
        return Ok(());
    }
    Err(invalid(format!("Expected property {name}, found {found}")))
}

fn checked_number<E>(
    element: Element<'_, '_>,
    name: &str,
    limits: &SharedLimits,
) -> Result<f64, NodeWireError<E>> {
    let value = number(element, name)?;
    let range = limits.snapshot();
    if !(range.minimum.to_framework_f64()..=range.maximum.to_framework_f64()).contains(&value) {
        return Err(invalid(format!(
            "Value {value} is outside the allowed range"
        )));
    }
    Ok(value)
}

fn load_variable<E: 'static>(
    element: Element<'_, '_>,
    context: &mut NodeWireReadContext<'_, E>,
) -> Result<VariableSpec, NodeWireError<E>> {
    let data = child(element, "VS").ok_or_else(|| invalid("Missing VS variable specification"))?;
    let mut value = VariableSpec::new(number(data, "vMode")?);
    let name = data.attribute("name").map(utf16);
    value.set_name(name.as_deref());
    let flag = |name| data.attribute(name) == Some("true");
    if flag("neg") && flag("not") {
        return Err(invalid(
            "A variable cannot have both negative and nonzero ranges",
        ));
    }
    value.set_numeric_flags(flag("neg"), flag("who"), flag("not"));
    if let Some(options) = data.attribute("ops") {
        let options = utf16(options);
        let result = if let Some((base, service)) = &mut context.variables {
            value.set_options(Some(&options), base, *service)
        } else {
            value.set_options(Some(&options), &[], &mut LabFiles::<E>::default())
        };
        result.map_err(|error| match error {
            VariableSpecError::Host(error) => error.into(),
            VariableSpecError::Binary(error) => error.into(),
            VariableSpecError::ConflictingBounds => invalid("Conflicting variable ranges"),
        })?;
    }
    Ok(value)
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn selection(label: &str, version: f64) -> &str {
    if version < 2.23 {
        match label {
            "Sharp Cells" => return "Jagged Cells",
            "Cloud Noise" => return "Plasma",
            "Colors and Grooves" if version < 2.12 => return "Colors & Grooves",
            _ => (),
        }
    }
    label
}

fn load_light_position<E: 'static>(
    property: &PointwiseParameter<E>,
    x: f64,
    y: f64,
) -> Result<(), PointwiseError<E>> {
    let object = PointwiseCustomObject::new_light_position();
    object.set_light_position_x(x)?;
    object.set_light_position_y(y)?;
    property.set_custom_object(Some(&object))
}

fn load_property<E: 'static>(
    element: Element<'_, '_>,
    property: &PointwiseParameter<E>,
    kind: PointwiseKind,
    version: f64,
    context: &mut NodeWireReadContext<'_, E>,
) -> Result<(), NodeWireError<E>> {
    let name = String::from_utf16_lossy(&property.name());
    if kind == PointwiseKind::IlluminateG2 && version == 1.0 && name == "Light Position" {
        let [x, y] = [number(element, "value1")?, number(element, "value2")?];
        load_light_position(property, x, y)?;
        return Ok(());
    }
    named_property(element, &name, Some(kind), version)?;
    match property.value() {
        PointwiseParameterValue::Number { limits, .. } => {
            property.set_number(checked_number(element, "value", &limits)?)?;
        }
        PointwiseParameterValue::Pair { limits, .. } => {
            property.set_pair([
                checked_number(element, "value1", &limits)?,
                checked_number(element, "value2", &limits)?,
            ])?;
        }
        PointwiseParameterValue::Choice { options, .. } => {
            let label = selection(attr(element, "selection")?, version);
            let index = options
                .entries()
                .iter()
                .position(|entry| {
                    entry
                        .as_ref()
                        .is_some_and(|units| units.iter().copied().eq(label.encode_utf16()))
                })
                .ok_or_else(|| invalid(format!("Unknown choice {label} for {name}")))?;
            property.set_index(i32::try_from(index).map_err(|_| PointwiseError::Index)?)?;
        }
        PointwiseParameterValue::Text(_) => {
            property.set_text(Some(&utf16(attr(element, "text")?)))?
        }
        PointwiseParameterValue::Color(_) => {
            property.set_color(ContextColorValue::unlinked([
                number(element, "r")?,
                number(element, "g")?,
                number(element, "b")?,
            ]))?;
        }
        PointwiseParameterValue::Variable(_) => {
            property.set_variable(load_variable(element, context)?)?
        }
        PointwiseParameterValue::Matrix {
            current: Some(object),
            ..
        } if object.is_light_position() => {
            let data = child(element, "LightPositionPropertyData")
                .ok_or_else(|| invalid("Missing LightPositionPropertyData"))?;
            load_light_position(property, number(data, "x")?, number(data, "y")?)?;
        }
        PointwiseParameterValue::Matrix { .. } if kind == PointwiseKind::ColorizeG2 => {
            let data = child(element, "GradientPropertyData")
                .ok_or_else(|| invalid("Missing GradientPropertyData"))?;
            let mut stops = Vec::new();
            for stop in data.children().filter(|node| node.has_tag_name("NobData")) {
                stops.push([
                    legacy_decimal(stop, "position")?,
                    legacy_decimal(stop, "val0")?,
                    legacy_decimal(stop, "val1")?,
                    legacy_decimal(stop, "val2")?,
                    legacy_decimal(stop, "val3")?,
                ]);
            }
            let value = LegacyColorGradient::from_xml_values(number(data, "steps")?, stops);
            property.set_custom_object(Some(&PointwiseCustomObject::new_legacy_gradient(value)))?;
        }
        PointwiseParameterValue::Matrix { .. } if kind == PointwiseKind::ToneCurveG2 => {
            let data = child(element, "ToneCurvePropertyData")
                .ok_or_else(|| invalid("Missing ToneCurvePropertyData"))?;
            property.set_custom_object(Some(&PointwiseCustomObject::new_legacy_tone_curve([
                legacy_decimal(data, "curve")?,
                legacy_decimal(data, "start")?,
                legacy_decimal(data, "end")?,
            ])))?;
        }
        _ => {
            return Err(invalid(format!(
                "Unsupported XML property {name} for {}",
                kind.name()
            )));
        }
    }
    if element.attribute("var").is_some() {
        property.set_binding_index(number(element, "var")?);
    }
    if let Some(mapping) = element.attribute("varMap") {
        property.set_binding_name(Some(&utf16(mapping)));
    }
    Ok(())
}

fn filter_properties<E: 'static>(
    element: Element<'_, '_>,
    filter: &PointwiseFilter<E>,
    version: f64,
    context: &mut NodeWireReadContext<'_, E>,
) -> Result<(), NodeWireError<E>> {
    let kind = filter.kind();
    let early_bci = kind == PointwiseKind::ChangeBci && version == 2.0;
    let mut elements = property_elements(element);
    if kind == PointwiseKind::ToneCurveG2 && version < 2.0 {
        let old = elements
            .next()
            .ok_or_else(|| invalid("Missing legacy Tone Curve"))?;
        let shape = legacy_decimal::<f64, _>(old, "value")? as f32;
        filter.parameter(0)?.set_custom_object(Some(
            &PointwiseCustomObject::new_legacy_tone_curve([shape, 0., 1.]),
        ))?;
        return Ok(());
    }
    for property in filter.parameters() {
        let name = String::from_utf16_lossy(&property.name());
        if name == "Skip for Color Only" && (version < 2.24 || (version < 2.25 && kind.is_group()))
            || name == "Catch Adjusters" && kind.is_group()
            || early_bci && name == "Auto Fix"
            || kind == PointwiseKind::IlluminateG2 && name == "Light Range" && version < 3.01
            || kind == PointwiseKind::GaussianBlur
                && (name == "Seamless" || (version < 2.1 && name == "Direction"))
        {
            continue;
        }
        if kind == PointwiseKind::ColorizeG2 && name == "Keep Transparency" && version < 2.0 {
            property.set_index(1)?;
            continue;
        }
        let value = elements
            .next()
            .ok_or_else(|| invalid(format!("Missing {} property {name}", kind.name())))?;
        load_property(value, &property, kind, version, context)?;
    }
    // These original readers visit their own property count and ignore trailing fields.
    if !matches!(kind, PointwiseKind::ColorizeG2 | PointwiseKind::ToneCurveG2)
        && !early_bci
        && elements.next().is_some()
    {
        return Err(invalid(format!(
            "Unexpected XML properties on {}",
            kind.name()
        )));
    }

    if kind == PointwiseKind::WizardGroup {
        if let Some(section) = child(element, "Sec") {
            filter.load_wizard_label(utf16(attr(section, "val")?));
        }
        if let Some(owner) = child(element, "ON") {
            if child(owner, "Cn")
                .is_some_and(|children| children.children().any(|node| node.is_element()))
            {
                return Err(NodeWireError::UnsupportedCapability(
                    "legacy XML nested Wizard controls",
                ));
            }
            let mut elements = property_elements(owner);
            for property in filter.wizard_dynamic_properties()? {
                let element = elements
                    .next()
                    .ok_or_else(|| invalid("Missing Wizard control"))?;
                load_property(element, &property, kind, version, context)?;
            }
            if elements.next().is_some() {
                return Err(invalid("Unexpected XML Wizard controls"));
            }
        }
    }
    Ok(())
}

fn generator_properties<E: 'static>(
    element: Element<'_, '_>,
    generator: &ImageGenerator<E>,
    version: f64,
) -> Result<(), NodeWireError<E>> {
    let mut elements = property_elements(element).peekable();
    for property in generator.parameters() {
        let name = property.name();
        if generator.kind() == ImageGeneratorKind::Noise
            && name == "Iterations"
            && version < 2.02
            && elements.peek().is_none()
        {
            continue;
        }
        let value = elements
            .next()
            .ok_or_else(|| invalid(format!("Missing generator property {name}")))?;
        named_property(value, name, None, version)?;
        if property.is_paired() {
            let limits = property.limits()?;
            property.set_pair([
                checked_number(value, "value1", &limits)?,
                checked_number(value, "value2", &limits)?,
            ])?;
        } else if property.is_choice() {
            let label = selection(attr(value, "selection")?, version);
            property.set_text(Some(&utf16(label)))?;
        } else if property.is_scalar() {
            property.set_number(checked_number(value, "value", &property.limits()?)?)?;
        } else {
            return Err(invalid(format!(
                "Unsupported XML generator property {name}"
            )));
        }
        if value.attribute("var").is_some() {
            property.set_binding_index(number(value, "var")?);
        }
        if let Some(mapping) = value.attribute("varMap") {
            property.set_binding_name(Some(&utf16(mapping)));
        }
    }
    if elements.next().is_some() {
        return Err(invalid("Unexpected XML generator properties"));
    }
    Ok(())
}

pub(crate) fn load_node_xml<E: 'static>(
    element: Element<'_, '_>,
    version: f64,
    context: &mut NodeWireReadContext<'_, E>,
) -> Result<Option<PointwiseNode<E>>, NodeWireError<E>> {
    let name = attr(element, "type")?;
    if name == "null" {
        return Ok(None);
    }
    context.enter()?;
    let result = (|| {
        let node = factory::create_versioned_node(Some(&utf16(name)), version, Some(context))?;
        if let Some(filter) = node.filter() {
            filter_properties(element, &filter, version, context)?;
        } else if let Some(generator) = node.image_generator_handle() {
            generator_properties(element, &generator, version)?;
        } else if name == "Solid Color" {
            let data = property_elements(element)
                .next()
                .and_then(|value| value.first_element_child())
                .ok_or_else(|| invalid("Missing XML Solid Color components"))?;
            node.set_solid_color([
                number(data, "component0")?,
                number(data, "component1")?,
                number(data, "component2")?,
            ])
            .map_err(NodeWireError::Solid)?;
            node.set_solid_transparency(number(data, "component3")?)
                .map_err(NodeWireError::Solid)?;
        } else if name != "Input" || property_elements(element).next().is_some() {
            return Err(invalid(format!("Unsupported XML node {name}")));
        }

        let mut inputs = child(element, "Inputs")
            .into_iter()
            .flat_map(|list| list.children())
            .filter(|node| node.has_tag_name("Node"));
        for index in 0..node.wire_slot_count()? {
            let input = inputs
                .next()
                .ok_or_else(|| invalid(format!("Missing input {index} on {name}")))?;
            let slot = node.slot(index as i32)?;
            if let Some(child) = load_node_xml(input, version, context)? {
                slot.set_node(Some(&child))?;
            }
        }
        if node.kind().is_some_and(PointwiseKind::is_group)
            && let Some(contents) = child(element, "Contents").and_then(|list| child(list, "Node"))
            && let Some(child) = load_node_xml(contents, version, context)?
        {
            node.content(0)?.set_node(Some(&child))?;
        }
        factory::migrate_loaded_node(node).map(Some)
    })();
    context.leave();
    result
}
