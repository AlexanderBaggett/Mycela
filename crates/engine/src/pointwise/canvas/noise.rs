//! The saved Noise Lab inside a Style property renders through the ordinary final graph.
use crate::pointwise::{
    node_wire::read_style_noise_lab,
    template_lab::{SCALE, SEED},
};
use crate::{
    BinaryCursor, ImageGeometry, NodeWireError, PointwiseError, PointwiseFilter, PointwiseKind,
    PointwiseNode, PointwiseRenderEffects, PointwiseRenderOptions, ProceduralExecution,
    SharedImageMap, TemplateLabKind,
};

pub(super) fn render<E: 'static>(
    lab_record: &[u8],
    source: Option<&SharedImageMap>,
    width: i32,
    raw_scale: f32,
    seed: i32,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Vec<f32>, PointwiseError<E>> {
    crate::pointwise::check_render_cancelled(effects.cancel_check())?;
    let mut reader = BinaryCursor::new(lab_record).map_err(|_| PointwiseError::InvalidOperation)?;
    let lab = read_style_noise_lab(&mut reader, 3.75, None).map_err(|error| match error {
        NodeWireError::Node(error) => error,
        _ => PointwiseError::InvalidOperation,
    })?;
    let slot = lab.slot(0)?;
    let input = if let Some(input) = slot.node() {
        input
    } else {
        let input = PointwiseNode::input();
        slot.set_node_quiet(Some(&input))?;
        input
    };
    input.set_input_image(source.cloned())?;

    let wrapper = PointwiseFilter::new(PointwiseKind::TemplateLab(TemplateLabKind::Wrapper))?;
    wrapper.node().content(0)?.set_node_quiet(Some(&lab))?;
    wrapper.parameter(SCALE)?.set_number(f64::from(raw_scale))?;
    wrapper.parameter(SEED)?.set_number(f64::from(seed))?;
    wrapper.node().set_graph_scale(1.)?;
    wrapper.node().set_graph_seed_modifier(0.)?;
    let final_graph = wrapper.node().final_graph_clone()?;
    let image = final_graph
        .render_with_effects(
            ImageGeometry::new(width, width),
            0.,
            execution,
            PointwiseRenderOptions::new(false, false),
            false,
            effects,
        )?
        .image()
        .ok_or(PointwiseError::NullReference)?;
    Ok(image.materialize(0)?.values())
}
