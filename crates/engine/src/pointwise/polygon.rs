//! Polygon (ah8) generator with Genetica's decimal scanline geometry.
use super::polygon_decimal::D;
use super::{PointwiseError, PointwiseFilter, SharedImageMap};
use crate::procedural_schedule::for_each_row_chunk;
use crate::{ImageError, ImageGeometry, ProceduralExecution};
use std::cmp::Ordering;

// ah8.iz checks local animation and the variable owner's animation before
// consulting the stored option text. It never samples Repeat at frame time.
pub(super) fn rounds_scaled_size<E: 'static>(
    f: &PointwiseFilter<E>,
) -> Result<bool, PointwiseError<E>> {
    let repeat = f.parameter(6)?;
    if super::parameter_ops::is_animated(|| repeat.retained_animation())?
        || repeat.has_animated_binding()?
    {
        return Ok(true);
    }
    Ok(repeat.text()?.as_deref() != Some(&[78, 111]))
}

// ok.g obtains both signs before rounding either component.
pub(super) fn round_scaled_size<E>(
    values: [f64; 2],
    scale: f64,
) -> Result<[f64; 2], PointwiseError<E>> {
    if scale == 1.0 {
        return Ok(values);
    }
    let sign = |v: f64| -> Result<f64, PointwiseError<E>> {
        if v.is_nan() {
            Err(PointwiseError::Arithmetic)
        } else if v < 0.0 {
            Ok(-1.0)
        } else if v > 0.0 {
            Ok(1.0)
        } else {
            Ok(0.0)
        }
    };
    let signs = [sign(values[0])?, sign(values[1])?];
    let mut result = values;
    for axis in 0..2 {
        result[axis] = crate::round_decimal_places(result[axis], 0)
            .map_err(|_| ImageError::ArgumentOutOfRange)?;
        if result[axis] == 0.0 {
            result[axis] = signs[axis];
        }
    }
    Ok(result)
}

impl<E: 'static> PointwiseFilter<E> {
    /// Local ok.s6 for Polygon; recursive graph scale propagation is separate.
    pub fn set_local_polygon_scale(
        &self,
        scale: f64,
        flags: Option<&[bool]>,
    ) -> Result<(), PointwiseError<E>> {
        if self.kind() != super::PointwiseKind::Polygon {
            return Err(PointwiseError::InvalidCast);
        }
        self.parameter(0)?.set_size_scale(scale, flags)?;
        self.set_local_factor(scale);
        Ok(())
    }
    /// Local ok.py/ah8.iz followed by the actual ordered Size setters.
    pub fn apply_polygon_size_scale(&self, scale: f64) -> Result<(), PointwiseError<E>> {
        if self.kind() != super::PointwiseKind::Polygon {
            return Err(PointwiseError::InvalidCast);
        }
        let round = rounds_scaled_size(self)?;
        let size = self.parameter(0)?;
        let values = [size.extent(0)? * scale, size.extent(1)? * scale];
        let values = if round {
            round_scaled_size(values, scale)?
        } else {
            values
        };
        size.set_extent(0, values[0])?;
        size.set_extent(1, values[1])?;
        self.commit_spatial_scale(scale)
    }
}

pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    super::check_render_cancelled(cancelled)?;
    let out = SharedImageMap::new(geometry);
    let pixels = out.materialize(0)?;
    let edges = crate::framework_cast::integer(f.parameter(2)?.effective_value_at(time)?);
    let mut sx = f.parameter(0)?.value_at(0, time)?;
    let mut sy = f.parameter(0)?.value_at(1, time)?;
    let mut lx = f.parameter(1)?.raw_value_at(0, time)?;
    let mut ly = f.parameter(1)?.raw_value_at(1, time)?;
    let flip_x = sx < 0.0;
    let flip_y = sy < 0.0;
    if flip_x {
        sx = -sx;
        lx -= sx;
    }
    if flip_y {
        sy = -sy;
        ly -= sy;
    }
    if f.parameter(6)?.effective_index_at(time)? == 1 {
        lx %= 1.0;
        if lx < 0.0 {
            lx += 1.0;
        }
        ly %= 1.0;
        if ly < 0.0 {
            ly += 1.0;
        }
    }
    let vertex_count = edges
        .checked_add(1)
        .and_then(|count| usize::try_from(count).ok())
        .ok_or(ImageError::Overflow)?;
    let n = vertex_count.saturating_sub(1);
    let mut vertices = Vec::new();
    vertices
        .try_reserve_exact(vertex_count)
        .map_err(|_| PointwiseError::Image(ImageError::Allocation))?;
    let angle =
        (-(f.parameter(3)?.effective_value_at(time)? + 90.0)) / 180.0 * std::f64::consts::PI;
    let inner = D::from_f64::<E>(f.parameter(4)?.effective_value_at(time)?)?;
    let zero = D::integer(0);
    let one = D::integer(1);
    let two = D::integer(2);
    let half = one.div::<E>(&two)?;
    let mut outer = true;
    let mut counter = 0;
    for i in 0..n {
        let a = i as f64 * 2.0 * std::f64::consts::PI / edges as f64 + angle;
        let mut x = D::from_f64::<E>(libm::cos(a))?;
        let mut y = D::from_f64::<E>(libm::sin(a))?;
        if !outer {
            x = x.mul::<E>(&inner)?;
            y = y.mul::<E>(&inner)?;
        }
        vertices.push((
            x.div::<E>(&two)?.add::<E>(&half)?,
            y.div::<E>(&two)?.add::<E>(&half)?,
        ));
        counter += 1;
        if f.parameter(7)?.effective_index_at(time)? == 1 {
            if counter > 1 {
                counter = 0;
            }
            outer = counter == 0;
        } else if f.parameter(7)?.effective_index_at(time)? == 2 {
            if counter > 3 {
                counter = 0;
            }
            outer = counter < 2;
        }
    }
    let fit = f.parameter(5)?.effective_index_at(time)? == 1
        || f.parameter(5)?.effective_index_at(time)? == 2;
    if fit {
        let (mut minx, mut maxx, mut miny, mut maxy) =
            (one.clone(), zero.clone(), one.clone(), zero.clone());
        for (x, y) in &vertices {
            if x.cmp(&minx) == Ordering::Less {
                minx = x.clone();
            }
            if x.cmp(&maxx) == Ordering::Greater {
                maxx = x.clone();
            }
            if y.cmp(&miny) == Ordering::Less {
                miny = y.clone();
            }
            if y.cmp(&maxy) == Ordering::Greater {
                maxy = y.clone();
            }
        }
        let dx = minx.add::<E>(&one.sub::<E>(&maxx)?)?;
        let dy = miny.add::<E>(&one.sub::<E>(&maxy)?)?;
        let mut wx = one.sub::<E>(&dx)?;
        let mut wy = one.sub::<E>(&dy)?;
        if f.parameter(5)?.effective_index_at(time)? == 1 {
            if wx.cmp(&wy) == Ordering::Less {
                wx = wy.clone();
            } else {
                wy = wx.clone();
            }
        }
        let ox = one
            .sub::<E>(&maxx.sub::<E>(&minx)?.div::<E>(&wx)?)?
            .div::<E>(&two)?;
        let oy = one
            .sub::<E>(&maxy.sub::<E>(&miny)?.div::<E>(&wy)?)?
            .div::<E>(&two)?;
        for (x, y) in &mut vertices {
            *x = x.sub::<E>(&minx)?.div::<E>(&wx)?.add::<E>(&ox)?;
            *y = y.sub::<E>(&miny)?.div::<E>(&wy)?.add::<E>(&oy)?;
        }
    }
    // Closing the polygon reads vertex zero. Edges = -1 reaches that read
    // with an empty array. Edges = 0 leaves one zero-initialized vertex.
    if vertex_count == 0 {
        return Err(PointwiseError::Index);
    }
    let first = vertices
        .first()
        .cloned()
        .unwrap_or_else(|| (zero.clone(), zero.clone()));
    vertices.push(first);
    for (x, y) in &mut vertices {
        if flip_x {
            *x = one.sub::<E>(x)?;
        }
        if flip_y {
            *y = one.sub::<E>(y)?;
        }
    }
    let dsy = D::from_f64::<E>(sy)?;
    let dy = D::from_f64::<E>(ly)?.add::<E>(&dsy)?.sub::<E>(&one)?;
    let repeat = f.parameter(6)?.effective_index_at(time)? == 1;
    let no_repeat = f.parameter(6)?.effective_index_at(time)? == 0;
    let width = geometry.width();
    let height = geometry.height();
    for_each_row_chunk(height, execution, |start, count| {
        let half_x = 0.5 / f64::from(width);
        let half_y = half.div::<E>(&D::integer(height))?;
        let mut intersections = Vec::new();
        for row in start..start + count {
            super::check_render_cancelled(cancelled)?;
            let mut y = D::integer(row)
                .div::<E>(&D::integer(height))?
                .mul::<E>(&dsy)?
                .sub::<E>(&dy)?
                .add::<E>(&half_y)?;
            if repeat {
                y = y.rem_one::<E>()?;
                if y.cmp(&zero) == Ordering::Less {
                    y = y.add::<E>(&one)?;
                }
            }
            intersections.clear();
            for edge in vertices.windows(2) {
                let (a, b) = if edge[0].1.cmp(&edge[1].1) == Ordering::Less {
                    (&edge[0], &edge[1])
                } else {
                    (&edge[1], &edge[0])
                };
                if y.cmp(&a.1) == Ordering::Greater && y.cmp(&b.1) != Ordering::Greater {
                    let t = y.sub::<E>(&a.1)?.div::<E>(&b.1.sub::<E>(&a.1)?)?;
                    intersections.push(a.0.add::<E>(&t.mul::<E>(&b.0.sub::<E>(&a.0)?)?)?.to_f64());
                }
            }
            intersections.sort_by(f64::total_cmp);
            let mut index = 0;
            let mut inside = false;
            if no_repeat {
                while index < intersections.len() && lx > intersections[index] {
                    inside = !inside;
                    index += 1;
                }
            }
            pixels.with_mut_slice(|pixels| -> Result<(), ImageError> {
                let mut previous = 0.0;
                for col in 0..width {
                    let mut x = f64::from(col) / f64::from(width) * sx + lx;
                    x += half_x;
                    if repeat {
                        let reset = crate::framework_cast::integer(previous)
                            < crate::framework_cast::integer(x)
                            || col == 0;
                        previous = x;
                        x %= 1.0;
                        if x < 0.0 {
                            x += 1.0;
                        }
                        if reset {
                            index = 0;
                            inside = false;
                            while index < intersections.len() && x > intersections[index] {
                                inside = !inside;
                                index += 1;
                            }
                        }
                    }
                    while index < intersections.len() && x > intersections[index] {
                        inside = !inside;
                        index += 1;
                    }
                    if index == intersections.len() {
                        inside = false;
                    }
                    if inside {
                        let index = (col + row * width) as usize;
                        *pixels.get_mut(index).ok_or(ImageError::Index)? = 1.0;
                    }
                }
                Ok(())
            })?;
        }
        Ok(())
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PointwiseKind, PointwiseRenderEffects, PointwiseRenderOptions, RenderCaptureSession,
    };
    fn filter() -> PointwiseFilter<()> {
        PointwiseFilter::new(PointwiseKind::Polygon).unwrap()
    }
    fn render(f: &PointwiseFilter<()>) -> SharedImageMap {
        f.render_supplied_at_time(
            &[],
            32,
            32,
            0.0,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(false, false),
        )
        .unwrap()
        .unwrap()
    }

    #[test]
    fn polygon_cancels_after_starting_the_scanlines() {
        use std::cell::Cell;

        let polygon = filter();
        let checks = Cell::new(0);
        let cancelled = || {
            checks.set(checks.get() + 1);
            checks.get() >= 4
        };
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_cancel_check(&cancelled);
        let result = polygon.render_supplied_at_time_with_effects(
            &[],
            128,
            128,
            0.0,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(false, false),
            &mut effects,
        );
        assert!(matches!(result, Err(PointwiseError::Cancelled)));
        assert_eq!(checks.get(), 4);
    }

    #[test]
    fn default_triangle_and_normal_graph_produce_mask() {
        let f = filter();
        let image = render(&f);
        let pixels = image.channel(0, false).unwrap();
        assert_eq!(pixels.read(0).unwrap(), 0.0);
        assert_eq!(pixels.read(16 + 16 * 32).unwrap(), 1.0);
        let graph = f.node().render_original(32, 32).unwrap();
        assert!(graph.caught_errors().is_empty());
        assert_eq!(
            graph.image().unwrap().channel(0, false).unwrap().values(),
            pixels.values()
        );
    }
    #[test]
    fn factory_wire_clone_capture_and_edited_shape() {
        let node = crate::create_node_by_identity::<()>(Some(
            &"Polygon".encode_utf16().collect::<Vec<_>>(),
        ))
        .unwrap();
        let f = node.filter().unwrap();
        assert_eq!(
            f.parameters()
                .iter()
                .map(|p| String::from_utf16(&p.name()).unwrap())
                .collect::<Vec<_>>(),
            [
                "Size",
                "Location",
                "Edges",
                "Angle",
                "Inner Radius",
                "Fit Mode",
                "Repeat",
                "Radius Changer"
            ]
        );
        let before = RenderCaptureSession::new().capture_filter(&f).unwrap();
        let initial = render(&f).channel(0, false).unwrap().values();
        f.parameter(2).unwrap().set_number(6.0).unwrap();
        f.parameter(4).unwrap().set_number(0.25).unwrap();
        f.parameter(7).unwrap().set_index(1).unwrap();
        assert!(
            !before.same_render_values(&RenderCaptureSession::new().capture_filter(&f).unwrap())
        );
        assert_ne!(initial, render(&f).channel(0, false).unwrap().values());
        assert_eq!(
            f.source_clone().unwrap().parameter(2).unwrap().number(),
            6.0
        );
        let mut writer = crate::BinaryWriter::default();
        node.save_binary(&mut writer).unwrap();
        let mut reader = crate::BinaryCursor::new(writer.bytes()).unwrap();
        let identity = reader.read_string().unwrap();
        let loaded = crate::load_node_binary::<()>(&mut reader, 3.75, Some(&identity)).unwrap();
        assert_eq!(loaded.filter().unwrap().parameter(2).unwrap().number(), 6.0);
    }
    #[test]
    fn decimal_operations_keep_fractional_geometry() {
        let one = D::integer(1);
        let two = D::integer(2);
        let three = D::integer(3);
        assert_eq!(one.div::<()>(&two).unwrap().to_f64(), 0.5);
        assert_eq!(
            D::from_f64::<()>(-1.25)
                .unwrap()
                .rem_one::<()>()
                .unwrap()
                .to_f64(),
            -0.25
        );
        assert_eq!(
            one.div::<()>(&three)
                .unwrap()
                .mul::<()>(&three)
                .unwrap()
                .to_f64(),
            1.0
        );
        assert!(matches!(
            one.div::<()>(&D::integer(0)),
            Err(PointwiseError::DivideByZero)
        ));
    }
}

#[cfg(test)]
mod scale_laws {
    use super::*;
    use crate::PointwiseKind;

    #[test]
    fn repeat_uses_source_rounded_size_and_preserves_small_signs() {
        let node = PointwiseFilter::<()>::new(PointwiseKind::Polygon).unwrap();
        let size = node.parameter(0).unwrap();
        size.set_pair([1.0, -1.0]).unwrap();
        node.set_local_polygon_scale(0.5, Some(&[])).unwrap();
        assert_eq!(size.normalized_pair().unwrap(), [0.5, -0.5]);
        node.parameter(6).unwrap().set_index(1).unwrap();
        node.set_local_polygon_scale(0.25, Some(&[])).unwrap();
        assert_eq!(size.normalized_pair().unwrap(), [1.0, -1.0]);
        assert_eq!(node.factor(), 0.25);
        node.apply_polygon_size_scale(0.5).unwrap();
        assert_eq!(size.pair().unwrap(), [1.0, -1.0]);
    }

    #[test]
    fn repeat_rounding_follows_the_current_option_text() {
        let node = PointwiseFilter::<()>::new(PointwiseKind::Polygon).unwrap();
        let repeat = node.parameter(6).unwrap();
        let options = crate::SharedSelectionOptions::new(vec![
            Some("Yes".encode_utf16().collect()),
            Some("No".encode_utf16().collect()),
        ]);
        repeat.set_options(Some(options)).unwrap();
        repeat.set_index(0).unwrap();
        node.set_local_polygon_scale(0.5, Some(&[])).unwrap();
        assert_eq!(
            node.parameter(0).unwrap().normalized_pair().unwrap(),
            [1.0, 1.0]
        );
    }

    #[test]
    fn orphan_repeat_binding_uses_the_stored_option_when_scaling() {
        let node = PointwiseFilter::<()>::new(PointwiseKind::Polygon).unwrap();
        let repeat = node.parameter(6).unwrap();
        repeat.set_binding_index(0);
        let size = node.parameter(0).unwrap();
        node.set_local_polygon_scale(0.5, Some(&[])).unwrap();
        assert_eq!(size.normalized_pair().unwrap(), [0.5, 0.5]);
        assert_eq!(size.size_scale().unwrap(), 0.5);
        assert_eq!(node.factor(), 0.5);
    }

    #[test]
    fn graph_scale_rounds_polygon_when_repeat_inherits_ancestor_animation() {
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        let polygon = PointwiseFilter::new(PointwiseKind::Polygon).unwrap();
        let root = map.node();
        root.content(0)
            .unwrap()
            .set_node(Some(&group.node()))
            .unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&polygon.node()))
            .unwrap();
        let size = polygon.parameter(0).unwrap();
        size.set_pair([2.5, -1.25]).unwrap();
        let repeat = polygon.parameter(6).unwrap();
        repeat.set_binding_index_notifying(0).unwrap();
        assert_eq!(repeat.index(), 0);

        root.set_graph_scale(0.5).unwrap();
        assert_eq!(size.normalized_pair().unwrap(), [1.25, -0.625]);
        assert_eq!(size.pair().unwrap(), [2.5, -1.25]);

        let source = map.parameter(3).unwrap();
        source.set_axis_animated(0, true).unwrap();
        let curve = source.animation_curve(0).unwrap();
        curve.set_minimum(0.0);
        curve.set_maximum(2.0);
        curve.append(&[Some(crate::CurveLayerHandle::new(
            crate::AnalyticCurveKind::Linear,
        ))]);
        assert_eq!(repeat.effective_index_at(0.125).unwrap(), 0);
        assert_eq!(repeat.effective_index_at(0.875).unwrap(), 1);
        root.set_graph_scale(0.25).unwrap();
        assert_eq!(size.normalized_pair().unwrap(), [1.0, -1.0]);
        assert_eq!(size.pair().unwrap(), [2.5, -1.25]);

        let expected = PointwiseFilter::<()>::new(PointwiseKind::Polygon).unwrap();
        expected
            .parameter(0)
            .unwrap()
            .set_pair([1.0, -1.0])
            .unwrap();
        for (time, index) in [(0.125, 0), (0.875, 1)] {
            expected.parameter(6).unwrap().set_index(index).unwrap();
            let geometry = ImageGeometry::new(64, 32);
            let snapshot = crate::RenderCaptureSession::new()
                .capture_node(&root, time)
                .unwrap();
            let mut capacity = |_| Ok(true);
            let mut preflight = |_, _| Ok(());
            let mut effects = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            let frame = snapshot
                .render_export_with_effects(
                    geometry,
                    ProceduralExecution::CallingThread,
                    crate::PointwiseRenderOptions::new(false, true),
                    &mut effects,
                )
                .unwrap();
            assert!(frame.caught_errors().is_empty());
            let expected_frame = expected.render_supplied(&[], 64, 32).unwrap().unwrap();
            assert_eq!(
                frame.image().unwrap().materialize(0).unwrap().values(),
                expected_frame.materialize(0).unwrap().values()
            );
        }

        root.set_graph_scale(1.0).unwrap();
        root.apply_graph_size_scale(0.5).unwrap();
        assert_eq!(size.pair().unwrap(), [1.0, -1.0]);
        assert_eq!(size.normalized_pair().unwrap(), [1.0, -1.0]);
    }
}
