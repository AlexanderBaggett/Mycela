//! Source-native graphs for the supported Noise Lab controls in compact GTX records.
use super::{NodeWireError, NodeWireReadContext};
use crate::{
    AnalyticCurveKind, CurveLayerHandle, GradientGeneratorParameterKind, ImageGeneratorKind,
    MatrixCoefficients, PointwiseError, PointwiseFilter, PointwiseKind, PointwiseNode,
    PointwiseParameter, PointwiseParameterValue, TemplateLabKind, VARIABLE_NUMERIC,
    VARIABLE_SELECTION, VariableSpec,
};

type Build<T, E> = Result<T, NodeWireError<E>>;

const TURBULENCE: &str = "0>10000;1>200;10>100;20>50;30>30;70>6;80>3.5;90>2;100>1";
const BCI_CONTRAST: &str = "0>-100;50>0;70>100;80>200;90>400;100>1000";

// Wizard Reference selections are named on the wire. Include only choices
// for which this module can supply a principal.
const NOISE_TYPES: &str =
    "None|None;Use Input|Use Input;3D Noise|3D Noise;Matrix Noise|Matrix Noise";
const MATRIX_TYPES: &str = "Use Input|Use Input;Cells|Cells";

pub(super) fn template<E: 'static>(
    lab: TemplateLabKind,
    context: &mut NodeWireReadContext<'_, E>,
) -> Build<Option<PointwiseNode<E>>, E> {
    let node = match lab {
        TemplateLabKind::Noise => combine_noise(context)?,
        TemplateLabKind::SelectNoise => {
            selector("Noise", "Noise Type", NOISE_TYPES, 2, context)?.node()
        }
        _ => return Ok(None),
    };
    Ok(Some(node))
}

pub(super) fn choice<E: 'static>(
    key: &str,
    context: &mut NodeWireReadContext<'_, E>,
) -> Build<Option<PointwiseNode<E>>, E> {
    Ok(Some(match key {
        "None" => return Ok(None),
        "Use Input" => use_input(context)?,
        "3D Noise" => lit_noise(context)?,
        "Matrix Noise" => matrix_noise(context)?,
        "Cells" => cells(context)?,
        _ => return Err(NodeWireError::MissingLabTemplate(key.into())),
    }))
}

fn new_filter<E: 'static>(
    context: &mut NodeWireReadContext<'_, E>,
    kind: PointwiseKind,
) -> Build<PointwiseFilter<E>, E> {
    context.enter()?;
    let filter = PointwiseFilter::new(kind);
    context.leave();
    Ok(filter?)
}

fn input<E: 'static>(
    filter: &PointwiseFilter<E>,
    index: i32,
    node: &PointwiseNode<E>,
) -> Build<(), E> {
    Ok(filter.node().slot(index)?.set_node_quiet(Some(node))?)
}

fn content<E: 'static>(filter: &PointwiseFilter<E>, node: &PointwiseNode<E>) -> Build<(), E> {
    Ok(filter.node().content(0)?.set_node_quiet(Some(node))?)
}

fn reference<E: 'static>(index: i32) -> Build<PointwiseNode<E>, E> {
    let filter = PointwiseFilter::new(PointwiseKind::Reference)?;
    filter.parameter(0)?.set_number(index as f64)?;
    Ok(filter.node())
}

fn numbers<E: 'static>(filter: &PointwiseFilter<E>, values: &[(i32, f64)]) -> Build<(), E> {
    for &(index, value) in values {
        filter.parameter(index)?.set_number(value)?;
    }
    Ok(())
}

fn choices<E: 'static>(filter: &PointwiseFilter<E>, values: &[(i32, i32)]) -> Build<(), E> {
    for &(index, value) in values {
        filter.parameter(index)?.set_index(value)?;
    }
    Ok(())
}

fn bind<E>(parameter: PointwiseParameter<E>, index: i32, mapping: Option<&str>) {
    parameter.set_binding_index(index);
    let mapping = mapping.map(|value| value.encode_utf16().collect::<Vec<_>>());
    parameter.set_binding_name(mapping.as_deref());
}

fn variable<E: 'static>(
    parameter: PointwiseParameter<E>,
    name: &str,
    options: Option<&str>,
    signed: bool,
) -> Build<(), E> {
    let mut spec = VariableSpec::new(if options.is_some() {
        VARIABLE_SELECTION
    } else {
        VARIABLE_NUMERIC
    });
    spec.set_name(Some(&name.encode_utf16().collect::<Vec<_>>()));
    spec.set_numeric_flags(signed, false, false);
    if let Some(options) = options {
        spec.set_options(
            Some(&options.encode_utf16().collect::<Vec<_>>()),
            &[],
            &mut super::super::lab_assets::LabFiles::<E>::default(),
        )
        .map_err(|_| PointwiseError::Argument)?;
    }
    Ok(parameter.set_variable(spec)?)
}

enum Control {
    Number(&'static str, f64, bool),
    Choice(&'static str, &'static str, i32),
}

fn wizard<E: 'static>(
    name: &str,
    inputs: i32,
    controls: &[Control],
    context: &mut NodeWireReadContext<'_, E>,
) -> Build<PointwiseFilter<E>, E> {
    let group = new_filter(context, PointwiseKind::WizardGroup)?;
    group.parameter(0)?.set_number(inputs as f64)?;
    group
        .parameter(2)?
        .set_text(Some(&name.encode_utf16().collect::<Vec<_>>()))?;
    for (index, control) in controls.iter().enumerate() {
        let parameter = group.parameter(index as i32 + 3)?;
        match control {
            Control::Number(name, _, signed) => variable(parameter, name, None, *signed)?,
            Control::Choice(name, options, _) => variable(parameter, name, Some(options), false)?,
        }
    }
    for (property, control) in group.wizard_dynamic_properties()?.into_iter().zip(controls) {
        match control {
            Control::Number(_, value, _) => property.set_number(*value)?,
            Control::Choice(_, _, index) => property.set_index(*index)?,
        }
    }
    wire_template_inputs(&group.node())?;
    Ok(group)
}

fn wire_template_inputs<E: 'static>(node: &PointwiseNode<E>) -> Build<(), E> {
    for (index, slot) in node.slots().iter().enumerate() {
        slot.set_node_quiet(Some(&reference(index as i32 + 1)?))?;
    }
    Ok(())
}

fn selector<E: 'static>(
    name: &str,
    label: &str,
    options: &str,
    initial: i32,
    context: &mut NodeWireReadContext<'_, E>,
) -> Build<PointwiseFilter<E>, E> {
    let reference_node = new_filter(context, PointwiseKind::WizardReference)?;
    reference_node.parameter(0)?.set_number(1.)?;
    reference_node
        .parameter(2)?
        .set_text(Some(&name.encode_utf16().collect::<Vec<_>>()))?;
    variable(reference_node.parameter(3)?, label, Some(options), false)?;
    wire_template_inputs(&reference_node.node())?;
    super::select_template(&reference_node, initial, context)?;
    reference_node.wizard_dynamic_properties()?[0].set_index_from_wire(initial)?;
    Ok(reference_node)
}

fn combine_noise<E: 'static>(
    context: &mut NodeWireReadContext<'_, E>,
) -> Build<PointwiseNode<E>, E> {
    use Control::*;
    let root = wizard(
        "Combine Noise",
        1,
        &[
            Choice(
                "Combine Mode",
                "Add;Subtract;Multiply;Divide;Difference;Greatest;Least",
                0,
            ),
            Number("Mix Bias", 0., true),
            Number("Brightness", 0., true),
            Number("Contrast", 0., true),
            Number("Level", 0., false),
        ],
        context,
    )?;

    let combine = new_filter(context, PointwiseKind::Combine)?;
    choices(&combine, &[(0, 2), (1, 0), (2, 1), (4, 1)])?;
    numbers(&combine, &[(3, 1.)])?;
    bind(combine.parameter(0)?, 0, None);
    bind(combine.parameter(1)?, 1, Some("0>1;49.9>1;50>0;100>0"));
    bind(combine.parameter(3)?, 1, Some("0>0;50>1;100>0"));
    for index in 0..2 {
        let select = new_filter(
            context,
            PointwiseKind::TemplateLab(TemplateLabKind::SelectNoise),
        )?;
        super::attach_template(&select.node(), TemplateLabKind::SelectNoise, context)?;
        input(&select, 0, &reference(1)?)?;
        if index == 0 {
            let selector = select
                .node()
                .content(0)?
                .node()
                .ok_or(PointwiseError::NullReference)?;
            let filter = selector.filter().ok_or(PointwiseError::InvalidCast)?;
            super::select_template(&filter, 0, context)?;
            filter.wizard_dynamic_properties()?[0].set_index_from_wire(0)?;
        }
        let center = new_filter(context, PointwiseKind::ChangeBci)?;
        choices(&center, &[(0, 1)])?;
        numbers(&center, &[(3, -50.)])?;
        input(&center, 0, &select.node())?;
        input(&combine, index, &center.node())?;
    }
    let adjust = new_filter(context, PointwiseKind::ChangeBci)?;
    choices(&adjust, &[(0, 0)])?;
    bind(adjust.parameter(1)?, 2, Some("0>-100;100>100"));
    bind(adjust.parameter(2)?, 3, Some(BCI_CONTRAST));
    input(&adjust, 0, &combine.node())?;
    let level = red_frequency(&adjust.node(), 4, context)?;
    let store = new_filter(context, PointwiseKind::StoreMap)?;
    store
        .parameter(0)?
        .set_text(Some(&"Noise".encode_utf16().collect::<Vec<_>>()))?;
    choices(&store, &[(1, 1), (2, 1)])?;
    input(&store, 0, &level)?;
    content(&root, &store.node())?;
    Ok(root.node())
}

fn red_frequency<E: 'static>(
    original: &PointwiseNode<E>,
    variable: i32,
    context: &mut NodeWireReadContext<'_, E>,
) -> Build<PointwiseNode<E>, E> {
    let red = new_filter(context, PointwiseKind::GetChannel)?;
    input(&red, 0, original)?;
    let frequency = new_filter(context, PointwiseKind::Frequency)?;
    numbers(&frequency, &[(0, 0.5)])?;
    choices(&frequency, &[(2, 1)])?;
    bind(frequency.parameter(0)?, variable, Some("0>0;100>.5"));
    input(&frequency, 0, &red.node())?;
    Ok(frequency.node())
}

fn use_input<E: 'static>(context: &mut NodeWireReadContext<'_, E>) -> Build<PointwiseNode<E>, E> {
    use Control::*;
    let root = wizard(
        "Use Input",
        1,
        &[
            Number("Brightness", 0., true),
            Number("Contrast", 0., true),
            Number("Level", 0., false),
            Number("Blur", 0., false),
        ],
        context,
    )?;
    let adjust = new_filter(context, PointwiseKind::ChangeBci)?;
    choices(&adjust, &[(0, 1)])?;
    bind(adjust.parameter(1)?, 0, Some("0>-100;100>100"));
    bind(adjust.parameter(2)?, 1, Some(BCI_CONTRAST));
    input(&adjust, 0, &reference(1)?)?;
    let level = red_frequency(&adjust.node(), 2, context)?;
    let blur = new_filter(context, PointwiseKind::GaussianBlur)?;
    numbers(&blur, &[(1, 0.)])?;
    bind(blur.parameter(1)?, 3, Some("0>0;50>.05;75>.1;100>.5"));
    input(&blur, 0, &level)?;
    content(&root, &blur.node())?;
    Ok(root.node())
}

fn group<E: 'static>(
    name: &str,
    context: &mut NodeWireReadContext<'_, E>,
) -> Build<PointwiseFilter<E>, E> {
    let group = new_filter(context, PointwiseKind::Group)?;
    group.parameter(0)?.set_number(1.)?;
    group
        .parameter(2)?
        .set_text(Some(&name.encode_utf16().collect::<Vec<_>>()))?;
    Ok(group)
}

fn generator<E: 'static>(
    kind: ImageGeneratorKind,
    size: f64,
    mode: i32,
    seed: f64,
    context: &mut NodeWireReadContext<'_, E>,
) -> Build<PointwiseNode<E>, E> {
    use GradientGeneratorParameterKind::*;
    context.enter()?;
    let node = PointwiseNode::image_generator(kind);
    context.leave();
    let node = node?;
    let generator = node
        .image_generator_handle()
        .ok_or(PointwiseError::InvalidCast)?;
    generator.parameter(Size).set_pair([size; 2])?;
    generator.parameter(Mode).set_index(mode)?;
    generator.parameter(Seed).set_number(seed)?;
    if kind == ImageGeneratorKind::Noise {
        generator.parameter(Iterations).set_number(1.)?;
    }
    Ok(node)
}

fn fractal<E: 'static>(
    base: &PointwiseNode<E>,
    iterations: f64,
    context: &mut NodeWireReadContext<'_, E>,
) -> Build<PointwiseFilter<E>, E> {
    let fractal = new_filter(context, PointwiseKind::Fractalize)?;
    numbers(&fractal, &[(0, iterations)])?;
    input(&fractal, 0, base)?;
    Ok(fractal)
}

fn distort<E: 'static>(
    variable: i32,
    size: f64,
    context: &mut NodeWireReadContext<'_, E>,
) -> Build<PointwiseFilter<E>, E> {
    let distort = new_filter(context, PointwiseKind::Distort)?;
    // Distort's x and y are authored through the Size binding.
    let diameter = distort.parameter(0)?;
    diameter.set_axis(0, size)?;
    diameter.set_axis(1, size)?;
    bind(diameter, variable, Some(TURBULENCE));
    numbers(&distort, &[(1, 1.)])?;
    choices(&distort, &[(2, 0)])?;
    Ok(distort)
}

fn lit_noise<E: 'static>(context: &mut NodeWireReadContext<'_, E>) -> Build<PointwiseNode<E>, E> {
    use Control::*;
    let root = wizard(
        "3D Noise",
        0,
        &[
            Choice("Detail", "Low;Medium;High;Very High", 3),
            Number("Turbulence", 20., false),
        ],
        context,
    )?;
    let group = group("", context)?;
    input(
        &group,
        0,
        &generator(ImageGeneratorKind::Noise, 2., 3, 3., context)?,
    )?;
    let relief = new_filter(context, PointwiseKind::PointLight)?;
    numbers(&relief, &[(1, 0.182), (3, 0.)])?;
    choices(&relief, &[(6, 1)])?;
    input(&relief, 0, &reference(1)?)?;
    let detail = fractal(&relief.node(), 5., context)?;
    bind(detail.parameter(0)?, 0, Some("0>0;2>2;3>5"));
    let warp = distort(1, 50., context)?;
    input(&warp, 0, &detail.node())?;
    for index in [1, 2] {
        input(&warp, index, &fractal(&reference(1)?, 6., context)?.node())?;
    }
    content(&group, &warp.node())?;
    content(&root, &group.node())?;
    Ok(root.node())
}

fn matrix_noise<E: 'static>(
    context: &mut NodeWireReadContext<'_, E>,
) -> Build<PointwiseNode<E>, E> {
    use Control::*;
    let root = wizard(
        "Matrix Noise",
        1,
        &[
            Number("Element Size", 40., false),
            Number("Element Sharpness", 80., false),
            Number("Radius", 30., false),
            Number("Angle", 0., false),
            Number("Turbulence", 0., false),
            Choice("Iterations", "One;Two;Three;Four;Five;Six;Seven;Eight", 3),
        ],
        context,
    )?;
    let group = group("Distort", context)?;
    let select = selector("", "Matrix Noise Type", MATRIX_TYPES, 1, context)?;
    let prepare = new_filter(context, PointwiseKind::ChangeBci)?;
    choices(&prepare, &[(0, 1)])?;
    numbers(&prepare, &[(1, -13.), (2, 1100.)])?;
    bind(prepare.parameter(1)?, 0, Some("0>-55;100>50"));
    bind(
        prepare.parameter(2)?,
        1,
        Some("0>0;20>100;40>257;60>557;80>1100;100>2000"),
    );
    input(&prepare, 0, &select.node())?;
    let matrix = new_filter(context, PointwiseKind::WizardMatrix)?;
    if let PointwiseParameterValue::Matrix {
        current: Some(object),
        ..
    } = matrix.parameter(0)?.value()
    {
        object
            .matrix()?
            .payload()
            .set_coefficients(Some(MatrixCoefficients::new(vec![
                0.33, 0., 0.17, 0., 0.17, 0., 0.17, 0., 0.17,
            ])));
    } else {
        return Err(PointwiseError::InvalidCast.into());
    }
    numbers(&matrix, &[(1, 4.), (2, 0.111)])?;
    bind(matrix.parameter(1)?, 5, Some("0>1;7>8"));
    bind(matrix.parameter(2)?, 2, Some("0>.03;100>.3"));
    bind(matrix.parameter(3)?, 3, Some("0>0;100>90"));
    input(&matrix, 0, &prepare.node())?;
    let normalize = new_filter(context, PointwiseKind::ChangeBci)?;
    choices(&normalize, &[(0, 0)])?;
    input(&normalize, 0, &matrix.node())?;
    input(&group, 0, &normalize.node())?;
    let warp = distort(4, 10_000., context)?;
    input(&warp, 0, &reference(1)?)?;
    for index in [1, 2] {
        let noise = generator(
            ImageGeneratorKind::Noise,
            1.,
            3,
            (index - 1) as f64,
            context,
        )?;
        input(&warp, index, &fractal(&noise, 5., context)?.node())?;
    }
    content(&group, &warp.node())?;
    content(&root, &group.node())?;
    Ok(root.node())
}

fn cells<E: 'static>(context: &mut NodeWireReadContext<'_, E>) -> Build<PointwiseNode<E>, E> {
    use Control::*;
    use GradientGeneratorParameterKind::{Density, Distribution, Irregularity, Mode};
    let root = wizard(
        "Cells",
        0,
        &[
            Choice("Mode", "Cells;Jagged Cells", 0),
            Number("Irregularity", 0., false),
            Number("Distribution", 100., false),
        ],
        context,
    )?;
    let group = group("Soften", context)?;
    let cells = generator(ImageGeneratorKind::Cells, 4., 0, 0., context)?;
    let generator = cells
        .image_generator_handle()
        .ok_or(PointwiseError::InvalidCast)?;
    generator.parameter(Density).set_number(1.)?;
    generator.parameter(Mode).set_binding_index(0);
    let irregularity = generator.parameter(Irregularity);
    irregularity.set_number(0.)?;
    irregularity.set_binding_index(1);
    irregularity.set_binding_name(Some(
        &"0>0;25>.2;50>.6;75>1.4;100>3"
            .encode_utf16()
            .collect::<Vec<_>>(),
    ));
    let distribution = generator.parameter(Distribution);
    distribution.set_number(1.)?;
    distribution.set_binding_index(2);
    distribution.set_binding_name(Some(&"0>0;100>1".encode_utf16().collect::<Vec<_>>()));
    input(&group, 0, &cells)?;
    let combine = new_filter(context, PointwiseKind::Combine)?;
    choices(&combine, &[(0, 0), (1, 0), (2, 1), (4, 1)])?;
    numbers(&combine, &[(3, 1.)])?;
    input(&combine, 0, &reference(1)?)?;
    let blur = new_filter(context, PointwiseKind::GaussianBlur)?;
    numbers(&blur, &[(1, 0.01)])?;
    input(&blur, 0, &reference(1)?)?;
    input(&combine, 1, &blur.node())?;
    let curve = new_filter(context, PointwiseKind::ToneCurve)?;
    let arc = CurveLayerHandle::new(AnalyticCurveKind::Arc);
    arc.control(13)
        .map_err(PointwiseError::from)?
        .ok_or(PointwiseError::NullReference)?
        .set_enumeration(2)
        .map_err(PointwiseError::from)?;
    arc.shape_control()
        .ok_or(PointwiseError::NullReference)?
        .set_scalar(87.5)
        .map_err(PointwiseError::from)?;
    let layers = curve.parameter(0)?.curve()?;
    layers.clear();
    layers.append(&[Some(arc)]);
    input(&curve, 0, &reference(1)?)?;
    input(&combine, 2, &curve.node())?;
    let normalize = new_filter(context, PointwiseKind::ChangeBci)?;
    choices(&normalize, &[(0, 0)])?;
    input(&normalize, 0, &combine.node())?;
    content(&group, &normalize.node())?;
    content(&root, &group.node())?;
    Ok(root.node())
}
