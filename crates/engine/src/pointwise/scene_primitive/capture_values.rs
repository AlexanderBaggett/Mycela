//! Side-effect-free raw comparison over the actual modern root backing.
use super::*;
fn text(value: &[u16], out: &mut Vec<u64>) {
    out.push(value.len() as u64);
    out.extend(value.iter().copied().map(u64::from));
}
fn curve(value: &crate::CurveRecord, out: &mut Vec<u64>) {
    out.extend([
        u64::from(value.minimum.to_bits()),
        u64::from(value.maximum.to_bits()),
        u64::from(value.wrap),
        value.layers.len() as u64,
    ]);
    for l in &value.layers {
        layer(l, out);
    }
}
fn layer(l: &crate::CurveLayerRecord, out: &mut Vec<u64>) {
    out.extend([
        l.kind().tag() as u64,
        l.mode as u64,
        u64::from(l.loop_points),
        l.points.len() as u64,
    ]);
    for p in &l.points {
        out.extend([
            u64::from(p.x.to_bits()),
            u64::from(p.y.to_bits()),
            p.left as u64,
            p.right as u64,
        ]);
    }
    out.push(l.controls().len() as u64);
    for c in l.controls() {
        out.extend([u64::from(c.enabled), c.id as u64]);
        text(&c.name, out);
        match &c.value {
            crate::CurveControlValue::Toggle(v) => out.extend([0, u64::from(*v)]),
            crate::CurveControlValue::Enumeration { value, maximum } => {
                out.extend([1, *value as u64, *maximum as u64])
            }
            crate::CurveControlValue::Scalar {
                value,
                minimum,
                maximum,
            } => {
                out.push(2);
                out.extend([value, minimum, maximum].map(|v| u64::from(v.to_bits())));
            }
            crate::CurveControlValue::Modulated {
                mode,
                scalar,
                minimum,
                maximum,
                child,
            } => {
                out.extend([3, *mode as u64]);
                out.extend([scalar, minimum, maximum].map(|v| u64::from(v.to_bits())));
                out.push(u64::from(child.is_some()));
                if let Some(child) = child {
                    layer(child, out);
                }
            }
        }
    }
}
pub(super) fn primitive<E: 'static>(
    node: &ScenePrimitiveNode<E>,
    comparison: &mut super::super::property::capture::capture_values::Comparison<E>,
) -> Result<Vec<u64>, PointwiseError<E>> {
    let mut out = vec![node.kind() as u64];
    out.extend(properties(node.properties(), comparison)?);
    Ok(out)
}
pub(in crate::pointwise) fn properties<E: 'static>(
    properties: Vec<ModernNodeProperty<E>>,
    comparison: &mut super::super::property::capture::capture_values::Comparison<E>,
) -> Result<Vec<u64>, PointwiseError<E>> {
    let mut out = Vec::new();
    for property in properties {
        let p = property.backing_snapshot();
        if let Some(labels) = property.component_labels() {
            out.push(1);
            for l in labels {
                text(&l, &mut out);
            }
        } else {
            out.push(0);
        }
        out.extend([
            property.kind() as u64,
            u64::from(p.adjustable),
            u64::from(p.animatable),
            u64::from(p.name.is_some()),
        ]);
        if let Some(name) = &p.name {
            text(name, &mut out);
        }
        if let Some(options) = property.selected_options() {
            out.extend([u64::MAX, options.len() as u64]);
            for option in options {
                text(&option, &mut out);
            }
        }
        match p.value() {
            ModernPropertyValue::Double { current, previous } => {
                out.extend([current.to_bits(), previous.to_bits()])
            }
            ModernPropertyValue::Int32 { current, previous }
            | ModernPropertyValue::Selected { current, previous } => {
                out.extend([*current as u64, *previous as u64])
            }
            ModernPropertyValue::Boolean(value) => out.push(u64::from(*value)),
            ModernPropertyValue::Vector { current, previous } => {
                out.extend(current.map(f64::to_bits));
                out.extend(previous.map(f64::to_bits));
            }
            _ => unreachable!("closed modern property cohort"),
        }
        comparison.animation(property.retained_animation().map(|a| a.animation), &mut out)?;
        out.push(u64::from(p.adjustment.is_some()));
        if let Some(a) = &p.adjustment {
            out.extend([a.property_type as u64, a.mode as u64]);
            out.extend(a.guid.map(u64::from));
            out.push(a.components().len() as u64);
            for c in a.components() {
                text(&c.name, &mut out);
                out.extend([u64::from(c.active), u64::from(c.curve.is_some())]);
                out.extend(c.default_bounds().map(|v| u64::from(v.to_bits())));
                if let Some(c) = &c.curve {
                    curve(c, &mut out);
                }
            }
        }
        let limits = property.root.limits();
        out.push(limits.len() as u64);
        for l in limits {
            comparison.limits(&l, &mut out);
        }
    }
    Ok(out)
}
