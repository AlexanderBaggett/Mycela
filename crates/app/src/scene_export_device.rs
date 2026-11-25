//! Real-device workflow through the application-owned capture/export pipeline.
use super::{ExportRenderJob, ExportSource, PendingExportRenderJob};
use crate::{ForegroundError, solid_color::map_preview::PreviewSelection};
use mycela_engine::{
    AnalyticCurveKind, CameraOrbitComponent, CurveLayerHandle, GradientGenerator,
    GradientGeneratorParameterKind as GradientParameter, PointwiseFilter, PointwiseKind,
    PointwiseNode, SceneCamera, SceneClearLightsNode, ScenePrimitiveKind, ScenePrimitiveNode,
    SelectionCloneMode, SharedImageMap, SharedInvertNode,
};

fn freeze(
    node: &PointwiseNode<ForegroundError>,
    invert: Option<&SharedInvertNode<ForegroundError>>,
    camera: &SceneCamera<ForegroundError>,
) -> PendingExportRenderJob {
    ExportRenderJob::freeze_at_time(
        ExportSource::Graph {
            node,
            camera: Some(camera),
            time: 0.375,
        },
        invert,
        &[],
        PreviewSelection::BaseColor,
        48,
        32,
        0.375,
    )
    .unwrap()
}

#[test]
#[ignore = "requires a real compatible wgpu device; run explicitly on the host"]
fn scene_export_finalizes_frozen_gradient_through_color_filters() {
    let gradient = GradientGenerator::<ForegroundError>::new().unwrap();
    let location = gradient.parameter(GradientParameter::Location);
    location.set_axis(0, 0.25).unwrap();
    let levels = PointwiseFilter::new(PointwiseKind::Levels).unwrap();
    levels
        .node()
        .slot(0)
        .unwrap()
        .set_node(Some(&gradient.node()))
        .unwrap();
    let channel = PointwiseFilter::new(PointwiseKind::GetChannel).unwrap();
    let channel_choice = channel.parameter(0).unwrap();
    channel_choice.set_index(6).unwrap();
    channel
        .node()
        .slot(0)
        .unwrap()
        .set_node(Some(&levels.node()))
        .unwrap();
    let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
    group.parameter(0).unwrap().set_number(2.).unwrap();
    let group = group.node();
    group
        .slot(0)
        .unwrap()
        .set_node(Some(&channel.node()))
        .unwrap();
    let unused = GradientGenerator::new().unwrap();
    unused
        .parameter(GradientParameter::Mode)
        .set_index(99)
        .unwrap();
    group
        .slot(1)
        .unwrap()
        .set_node(Some(&unused.node()))
        .unwrap();
    let reference = PointwiseFilter::new(PointwiseKind::Reference).unwrap();
    group
        .content(0)
        .unwrap()
        .set_node(Some(&reference.node()))
        .unwrap();
    let bci = PointwiseFilter::new(PointwiseKind::ChangeBci).unwrap();
    let brightness = bci.parameter(1).unwrap();
    brightness.set_number(12.).unwrap();
    bci.node().slot(0).unwrap().set_node(Some(&group)).unwrap();
    let colorize = PointwiseFilter::new(PointwiseKind::Colorize).unwrap();
    let ramp = colorize.parameter(0).unwrap().gradient().unwrap();
    ramp.clear_points().unwrap();
    ramp.add_normalized_color(0., 0.25, 0., 0.).unwrap();
    ramp.add_normalized_color(1., 1., 0., 0.).unwrap();
    colorize
        .node()
        .slot(0)
        .unwrap()
        .set_node(Some(&bci.node()))
        .unwrap();
    let hsb = PointwiseFilter::new(PointwiseKind::ChangeHsb).unwrap();
    hsb.node()
        .slot(0)
        .unwrap()
        .set_node(Some(&colorize.node()))
        .unwrap();
    let animation = hsb.parameter(0).unwrap().animation().unwrap();
    animation.ensure_curve(0).unwrap();
    let hue = animation.curves().unwrap().value(0).unwrap().unwrap();
    hue.set_minimum(0.);
    hue.set_maximum(320.);
    hue.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
    animation.flags().unwrap().set(0, true).unwrap();
    let scene = SceneClearLightsNode::new().node();
    scene.slot(0).unwrap().set_node(Some(&hsb.node())).unwrap();
    let camera = SceneCamera::source_preview();
    let expected = freeze(&scene, None, &camera);
    channel_choice.set_index(1).unwrap();
    channel_choice.set_axis_animated(0, true).unwrap();
    let channel_curve = channel_choice.animation_curve(0).unwrap();
    channel_curve.set_minimum(7.);
    channel_curve.set_maximum(7.);
    channel_curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
    let selector = reference.parameter(0).unwrap();
    selector.set_axis_animated(0, true).unwrap();
    let selector_curve = selector.animation_curve(0).unwrap();
    selector_curve.set_minimum(2.);
    selector_curve.set_maximum(2.);
    selector_curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
    brightness.set_number(0.).unwrap();
    brightness.set_axis_animated(0, true).unwrap();
    let brightness_curve = brightness.animation_curve(0).unwrap();
    brightness_curve.set_minimum(0.);
    brightness_curve.set_maximum(32.);
    brightness_curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
    gradient
        .parameter(GradientParameter::Mode)
        .options()
        .unwrap()
        .set_entry(0, None)
        .unwrap();
    let frozen = freeze(&scene, None, &camera);
    ramp.clear_points().unwrap();
    ramp.add_normalized_color(0., 0., 0., 0.25).unwrap();
    ramp.add_normalized_color(1., 0., 0., 1.).unwrap();
    let changed_ramp = freeze(&scene, None, &camera);
    brightness_curve.set_minimum(-32.);
    brightness_curve.set_maximum(0.);
    let edited = freeze(&scene, None, &camera);
    let result = render(frozen);
    assert!(result[3].iter().any(|&t| t < 1.));
    assert!(
        result[1]
            .iter()
            .zip(&result[0])
            .any(|(&green, &red)| green > red)
    );
    assert_eq!(result, render(expected));
    let changed_ramp = render(changed_ramp);
    assert_ne!(result, changed_ramp);
    assert_ne!(changed_ramp, render(edited));
}

fn pixels(image: SharedImageMap) -> [Vec<f32>; 4] {
    let channels =
        std::array::from_fn(|channel| image.channel(channel as i32, true).unwrap().values());
    image.dispose();
    channels
}

fn render(job: PendingExportRenderJob) -> [Vec<f32>; 4] {
    let rendered = job
        .materialize()
        .unwrap()
        .render_scene(|| false, false)
        .unwrap();
    assert_eq!(
        (rendered.bitmap.width(), rendered.bitmap.height()),
        (48, 32)
    );
    pixels(rendered.image)
}

#[test]
#[ignore = "requires a real compatible wgpu device; run explicitly on the host"]
fn scene_export_freezes_committed_source_and_invert_and_renders_real_pixels() {
    let primitive = ScenePrimitiveNode::<ForegroundError>::new(ScenePrimitiveKind::Cube).unwrap();
    let node = primitive.node();
    let camera = SceneCamera::source_preview();
    let invert = SharedInvertNode::<ForegroundError>::new(SelectionCloneMode::Original);
    // The retained actual Invert owner affects Red only; transparency remains untouched.
    invert.property(1).unwrap().set_index(1).unwrap();
    invert.property(2).unwrap().set_index(1).unwrap();

    let reference = freeze(&node, None, &camera);
    let old = freeze(&node, None, &camera);
    let inverted = freeze(&node, Some(&invert), &camera);
    let before = old.frozen_values();
    camera
        .set_orbit_component(CameraOrbitComponent::Azimuth, 90.)
        .unwrap();
    let camera_changed = freeze(&node, None, &camera);
    primitive
        .property(0)
        .unwrap()
        .set_vector3d([1.8, 0.6, 1.])
        .unwrap();
    primitive
        .property(1)
        .unwrap()
        .set_vector3d([0.6, 0., 0.])
        .unwrap();
    invert.property(0).unwrap().set_index(1).unwrap();
    let current = freeze(&node, None, &camera);
    assert!(!before.same_values(&current.frozen_values()));

    let expected = render(reference);
    let old = render(old);
    let inverted = render(inverted);
    let camera_changed = render(camera_changed);
    let current = render(current);
    assert_eq!(
        old, expected,
        "both old detached jobs survive later live edits"
    );
    assert_ne!(
        camera_changed, old,
        "camera edits change only subsequent captures"
    );
    assert_ne!(
        current, old,
        "the current primitive uses the new source values"
    );
    for channel in &old {
        assert_eq!(channel.len(), 48 * 32);
        assert!(
            channel
                .iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        );
    }
    assert!(
        old[3].iter().any(|value| *value < 1.0),
        "primitive has visible coverage"
    );
    assert!(
        old[3].contains(&1.0),
        "scene retains a transparent background"
    );
    assert!(
        old[..3].iter().flatten().any(|value| *value > 0.0),
        "surface is lit"
    );
    for channel in 0..4 {
        for (&source, &result) in old[channel].iter().zip(&inverted[channel]) {
            let expected = if channel == 0 { 1.0 - source } else { source };
            assert_eq!(
                result.to_bits(),
                expected.to_bits(),
                "Invert must use the frozen selection"
            );
        }
    }
}
