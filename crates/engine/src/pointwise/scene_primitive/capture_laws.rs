use super::*;
#[test]
fn primitive_capture_memo_keeps_canonical_node_and_detaches_actual_modern_limits() {
    let primitive = crate::ScenePrimitiveNode::<()>::new(crate::ScenePrimitiveKind::Cube).unwrap();
    let mut session = RenderCaptureSession::new();
    let a = session.capture_node(&primitive.node(), 0.).unwrap();
    let b = session.capture_node(&primitive.node(), 0.).unwrap();
    assert!(a.node.same_reference(&b.node));
    let copy = a.node.scene_primitive().unwrap();
    assert!(!copy.same_reference(&primitive));
    let old = primitive.property(0).unwrap();
    let new = copy.property(0).unwrap();
    assert!(!old.same_reference(&new));
    assert!(
        !old.component_limits(0)
            .unwrap()
            .same_reference(&new.component_limits(0).unwrap())
    );
    assert!(
        new.component_limits(0)
            .unwrap()
            .same_reference(&new.component_limits(1).unwrap())
    );
    old.set_vector3d([2.; 3]).unwrap();
    assert_eq!(new.value(), crate::ModernNodeValue::Vector3D([1.; 3]));
    let mut admission = |_: crate::SceneAllocationRequest| Ok(());
    let scene = a
        .render_scene_with_effects(&mut crate::SceneAllocationEffects::new(&mut admission))
        .unwrap();
    assert!(
        scene.models()[0]
            .try_positions()
            .unwrap()
            .iter()
            .any(|v| v.values() == [0.5; 3])
    );
}
#[test]
fn capture_compares_raw_modern_values_without_evaluating_or_creating_animation() {
    let primitive =
        crate::ScenePrimitiveNode::<()>::new(crate::ScenePrimitiveKind::Sphere).unwrap();
    assert!(
        primitive
            .property(3)
            .unwrap()
            .retained_animation()
            .is_none()
    );
    let first = RenderCaptureSession::new()
        .capture_node(&primitive.node(), 0.25)
        .unwrap();
    assert!(
        primitive
            .property(3)
            .unwrap()
            .retained_animation()
            .is_none()
    );
    primitive.property(3).unwrap().set_int32(4).unwrap();
    let second = RenderCaptureSession::new()
        .capture_node(&primitive.node(), 0.25)
        .unwrap();
    assert!(!first.same_render_values(&second));
    assert_eq!(
        first
            .node
            .scene_primitive()
            .unwrap()
            .property(3)
            .unwrap()
            .value(),
        crate::ModernNodeValue::Int32(10)
    );
}
#[test]
fn capture_uses_raw_vector_previous_while_source_clone_has_constructor_copy_default() {
    let primitive = crate::ScenePrimitiveNode::<()>::new(crate::ScenePrimitiveKind::Cube).unwrap();
    let frame = RenderCaptureSession::new()
        .capture_node(&primitive.node(), 0.)
        .unwrap();
    let captured = frame
        .node
        .scene_primitive()
        .unwrap()
        .property(0)
        .unwrap()
        .backing_snapshot();
    let copied = primitive
        .source_clone()
        .unwrap()
        .property(0)
        .unwrap()
        .backing_snapshot();
    assert!(
        matches!(captured.value(),crate::ModernPropertyValue::Vector{previous,..}if *previous==[1.;3])
    );
    assert!(
        matches!(copied.value(),crate::ModernPropertyValue::Vector{previous,..}if *previous==[0.;3])
    );
}
#[test]
fn actual_animation_source_copy_and_capture_keep_distinct_alias_cache_and_limits_rules() {
    let node = crate::ScenePrimitiveNode::<()>::new(crate::ScenePrimitiveKind::Cube).unwrap();
    let property = node.property(0).unwrap();
    let animation = property.animation().unwrap().unwrap();
    let curve = crate::SharedCurve::new(0., 1., false);
    let noise = crate::CurveLayerHandle::new_noise().unwrap();
    curve.append(&[Some(noise.clone())]);
    animation.set_curves(Some(crate::AnimationCurves::from_values(vec![
        Some(curve.clone()),
        Some(curve.clone()),
        None,
    ])));
    animation.set_flags(Some(crate::AnimationFlags::from_values(vec![
        true, true, false,
    ])));
    property.evaluated_value(0.25).unwrap();
    let source_caches = noise.noise_caches().unwrap();
    assert!(source_caches.iter().any(Option::is_some));
    let cloned = node.source_clone().unwrap();
    let cloned_property = cloned.property(0).unwrap();
    let cloned_animation = cloned_property.retained_animation().unwrap();
    let source_x = cloned_animation
        .curves()
        .unwrap()
        .value(0)
        .unwrap()
        .unwrap();
    let source_y = cloned_animation
        .curves()
        .unwrap()
        .value(1)
        .unwrap()
        .unwrap();
    assert!(!source_x.same_reference(&source_y));
    assert!(!source_x.same_reference(&curve));
    let rebuilt = source_x.layers()[0]
        .as_ref()
        .unwrap()
        .noise_caches()
        .unwrap();
    let rebuilt_y = source_y.layers()[0]
        .as_ref()
        .unwrap()
        .noise_caches()
        .unwrap();
    assert!(rebuilt.iter().any(Option::is_some));
    for ((original, copy), other) in source_caches
        .iter()
        .zip(rebuilt.iter())
        .zip(rebuilt_y.iter())
    {
        if let (Some(original), Some(copy), Some(other)) = (original, copy, other) {
            assert!(!original.same_reference(copy));
            assert!(!copy.same_reference(other));
        }
    }
    assert!(
        cloned_animation
            .limits()
            .unwrap()
            .same_reference(&animation.limits().unwrap())
    );
    assert!(
        !cloned_property
            .component_limits(0)
            .unwrap()
            .same_reference(&animation.limits().unwrap().value(0).unwrap().unwrap())
    );
    let frame = RenderCaptureSession::new()
        .capture_node(&node.node(), 0.25)
        .unwrap();
    let copied = frame.node.scene_primitive().unwrap().property(0).unwrap();
    let captured = copied.retained_animation().unwrap();
    assert!(!captured.same_reference(&animation));
    let x = captured.curves().unwrap().value(0).unwrap().unwrap();
    let y = captured.curves().unwrap().value(1).unwrap().unwrap();
    assert!(x.same_reference(&y));
    assert!(!x.same_reference(&curve));
    assert!(
        captured
            .limits()
            .unwrap()
            .value(0)
            .unwrap()
            .unwrap()
            .same_reference(&copied.component_limits(0).unwrap())
    );
    let caches = x.layers()[0].as_ref().unwrap().noise_caches().unwrap();
    for (a, b) in source_caches.iter().zip(caches.iter()) {
        match (a, b) {
            (Some(a), Some(b)) => {
                assert_eq!(
                    a.points()
                        .iter()
                        .map(|p| [p.x().to_bits(), p.y().to_bits()])
                        .collect::<Vec<_>>(),
                    b.points()
                        .iter()
                        .map(|p| [p.x().to_bits(), p.y().to_bits()])
                        .collect::<Vec<_>>()
                );
                assert!(!a.same_reference(b));
            }
            (None, None) => {}
            _ => panic!("actual warm caches must survive raw capture"),
        }
    }
}

#[test]
fn actual_animation_curve_and_control_aliases_survive_repeated_getvalue_and_edits() {
    let node = crate::ScenePrimitiveNode::<()>::new(crate::ScenePrimitiveKind::Torus).unwrap();
    let p = node.property(5).unwrap();
    let animation = p.animation().unwrap().unwrap();
    let alias = p.animation().unwrap().unwrap();
    assert!(alias.same_reference(&animation));
    assert!(alias.parent().same_reference(&p));
    animation.ensure_curve(0).unwrap();
    let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
    let layer = crate::CurveLayerHandle::new(crate::AnalyticCurveKind::Linear);
    curve.append(&[Some(layer.clone())]);
    let control = layer
        .controls()
        .into_iter()
        .flatten()
        .find(|c| c.id() == 2)
        .unwrap();
    animation.flags().unwrap().set(0, true).unwrap();
    let expected = f64::from(curve.sample(0.25).unwrap());
    assert_eq!(
        p.evaluated_value(0.25).unwrap(),
        crate::ModernNodeValue::Double(expected)
    );
    assert_eq!(
        p.evaluated_value(0.25).unwrap(),
        crate::ModernNodeValue::Double(expected)
    );
    assert!(
        animation
            .curves()
            .unwrap()
            .value(0)
            .unwrap()
            .unwrap()
            .same_reference(&curve)
    );
    assert!(curve.layers()[0].as_ref().unwrap().same_reference(&layer));
    assert!(
        layer
            .controls()
            .into_iter()
            .flatten()
            .find(|c| c.id() == 2)
            .unwrap()
            .same_reference(&control)
    );
    control.set_scalar(2.).unwrap();
    curve.set_minimum(20.);
    curve.set_maximum(40.);
    let expected = f64::from(curve.sample(0.25).unwrap());
    assert_eq!(
        p.evaluated_value(0.25).unwrap(),
        crate::ModernNodeValue::Double(expected)
    );
    let captured = RenderCaptureSession::new()
        .capture_node(&node.node(), 0.25)
        .unwrap();
    let copy = captured
        .node
        .scene_primitive()
        .unwrap()
        .property(5)
        .unwrap();
    let retained = copy.retained_animation().unwrap();
    let copied = retained.curves().unwrap().value(0).unwrap().unwrap();
    assert!(!copied.same_reference(&curve));
    assert_eq!(
        copy.evaluated_value(0.25).unwrap(),
        crate::ModernNodeValue::Double(expected)
    );
    control.set_scalar(3.).unwrap();
    assert_eq!(
        copy.evaluated_value(0.25).unwrap(),
        crate::ModernNodeValue::Double(expected)
    );
}
