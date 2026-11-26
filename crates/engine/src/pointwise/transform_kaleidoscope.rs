//! aaj: square folds, seven triangles and deliberately width-based source bounds.
use super::numeric::{add, div, mul, sub};
use super::transform::*;
use super::{PointwiseError, PointwiseFilter, PointwiseRenderOptions, SharedImageMap};
use crate::{
    FloatBuffer, ImageError, ImageGeometry, ProceduralExecution, SharedFloatStore,
    image_map::{read_pixel, write_pixel},
};
#[derive(Clone, Copy)]
struct Fold {
    w: i32,
    h: i32,
    mode: i32,
    x: f32,
    y: f32,
    size: f32,
    pixel_height: f32,
    pixel_width: f32,
    horizontal: f32,
}
fn mirror<E>(source: &FloatBuffer, s: Fold) -> Result<FloatBuffer, PointwiseError<E>> {
    let out =
        super::adaptive::floats(i32::try_from(source.len()).map_err(|_| ImageError::Overflow)?)?;
    let source = source.shared_values();
    out.with_mut_slice(|pixels| {
        pixels.copy_from_slice(source.as_slice());
        let mut top = cast(f64::from(s.y));
        if top > 0 {
            top = top.wrapping_sub(1);
        }
        let mut bottom = cast(f64::from(add(add(s.y, s.pixel_height), 0.5)));
        if bottom < s.h.wrapping_sub(1) {
            bottom = bottom.wrapping_add(1);
        }
        let left = cast(f64::from(add(s.x, 0.5)));
        for row in top..bottom {
            let mut end = left.wrapping_add(row).wrapping_sub(top);
            if end > s.w {
                end = s.w;
            }
            let target_col = left.wrapping_add(row).wrapping_sub(top);
            let at = row.wrapping_mul(s.w);
            for col in left..end {
                let target_row = top.wrapping_add(col).wrapping_sub(left);
                let sample = read_pixel(pixels, col.wrapping_add(at))?;
                write_pixel(
                    pixels,
                    target_col.wrapping_add(target_row.wrapping_mul(s.w)),
                    sample,
                )?;
            }
        }
        Ok::<(), PointwiseError<E>>(())
    })?;
    Ok(out)
}
fn square<E>(
    values: &FloatBuffer,
    dest: &FloatBuffer,
    s: Fold,
    regions: bool,
    execution: ProceduralExecution,
) -> Result<(), PointwiseError<E>> {
    let values = if s.mode == 1 {
        mirror(values, s)?
    } else {
        values.clone()
    };
    let values = values.shared_values();
    dest.with_mut_slice(|pixels| {
        let half = cast(f64::from(s.w) / 2. + 0.5);
        crate::procedural_schedule::for_each_chunk(
            half.wrapping_mul(half),
            execution,
            |start, count| {
                let mut row = start
                    .checked_div(half)
                    .ok_or(PointwiseError::DivideByZero)?;
                let mut col = rem(start, half)?;
                for _ in start..start.wrapping_add(count) {
                    if col >= half {
                        col = 0;
                        row = row.wrapping_add(1);
                    }
                    let y = add(mul(mul(row as f32, 2.), s.size), s.y);
                    let x = add(mul(mul(col as f32, 2.), s.horizontal), s.x);
                    let value = if regions {
                        read_pixel(
                            &values,
                            rem(cast(f64::from(x)), s.w)?
                                .wrapping_add(rem(cast(f64::from(y)), s.h)?.wrapping_mul(s.w)),
                        )?
                    } else {
                        super::motion_sampling::sample_pixels(x, y, &values, s.w, s.h, true)?
                    };
                    let a = row.wrapping_mul(s.w);
                    let b = s.h.wrapping_sub(1).wrapping_sub(row).wrapping_mul(s.w);
                    let right = s.w.wrapping_sub(1).wrapping_sub(col);
                    write_pixel(pixels, col.wrapping_add(a), value)?;
                    write_pixel(pixels, a.wrapping_add(right), value)?;
                    write_pixel(pixels, col.wrapping_add(b), value)?;
                    write_pixel(pixels, b.wrapping_add(right), value)?;
                    col = col.wrapping_add(1);
                }
                Ok(())
            },
        )
    })
}
struct Triangle {
    a: [f32; 2],
    b: [f32; 2],
    midpoint: [f32; 2],
    scale: [f32; 2],
}

impl Triangle {
    fn new([ax, ay]: [f32; 2], [bx, by]: [f32; 2], [cx, cy]: [f32; 2]) -> Self {
        let mx = add(cx, mul(sub(ax, bx), 0.5));
        let my = add(cy, mul(sub(ay, by), 0.5));
        Self {
            a: [ax, ay],
            b: [bx, by],
            midpoint: [mx, my],
            scale: [
                div(1., distance(bx, by, ax, ay, mx, my)),
                div(1., distance(cx, cy, ax, ay, bx, by)),
            ],
        }
    }
}

fn triangles() -> [Triangle; 7] {
    let zero = [0., 0.];
    let a = [div(-1., 6.), 0.25];
    let b = [div(1., 6.), 0.25];
    let c = [div(1., 3.), 0.];
    let d = [0.5, 0.25];
    let e = [div(2., 3.), 0.];
    let f = [div(5., 6.), 0.25];
    let g = [1., 0.];
    let h = [1.166_666_6, 0.25];
    [
        Triangle::new(b, zero, a),
        Triangle::new(b, zero, c),
        Triangle::new(b, d, c),
        Triangle::new(e, d, c),
        Triangle::new(e, d, f),
        Triangle::new(e, g, f),
        Triangle::new(h, g, f),
    ]
}
#[allow(clippy::neg_cmp_op_on_partial_ord)] // Source unordered comparisons select the corresponding right-hand triangle.
fn triangle_at(x: f32, y4: f32) -> usize {
    let a = div(1., 6.);
    let b = div(1., 3.);
    let c = div(2., 3.);
    let d = div(5., 6.);
    if x < 0.5 {
        if x < a {
            if mul(x, 6.) > y4 { 1 } else { 0 }
        } else if !(x < b) {
            if mul(sub(x, b), 6.) > y4 { 3 } else { 2 }
        } else if add(mul(sub(x, a), 6.), y4) > 1. {
            2
        } else {
            1
        }
    } else if x < c {
        if add(mul(sub(x, 0.5), 6.), y4) > 1. {
            4
        } else {
            3
        }
    } else if !(x < d) {
        if add(mul(sub(x, d), 6.), y4) > 1. {
            6
        } else {
            5
        }
    } else if mul(sub(x, c), 6.) > y4 {
        5
    } else {
        4
    }
}
fn copies<E>(dest: &mut [f32], s: Fold) -> Result<(), PointwiseError<E>> {
    let mut row = 0i32;
    let mut other = cast(libm::ceil(f64::from(s.h) / 2.));
    while row < other {
        for col in 0..s.w {
            let value = read_pixel(dest, col.wrapping_add(row.wrapping_mul(s.w)))?;
            write_pixel(dest, col.wrapping_add(other.wrapping_mul(s.w)), value)?;
        }
        row = row.wrapping_add(1);
        other = other.wrapping_sub(1);
    }
    let half = s.h / 2;
    let mut row = 0i32;
    loop {
        let other = row.wrapping_add(half);
        if other >= s.h {
            break;
        }
        for col in 0..s.w {
            let value = read_pixel(dest, col.wrapping_add(row.wrapping_mul(s.w)))?;
            write_pixel(dest, col.wrapping_add(other.wrapping_mul(s.w)), value)?;
        }
        row = row.wrapping_add(1);
    }
    Ok(())
}
fn triangular<E>(
    values: &FloatBuffer,
    dest: &FloatBuffer,
    s: Fold,
    triangles: &[Triangle; 7],
    regions: bool,
    execution: ProceduralExecution,
) -> Result<(), PointwiseError<E>> {
    let values = values.shared_values();
    dest.with_mut_slice(|pixels| {
        let rows = cast(libm::ceil(f64::from(s.h) / 4.));
        let step = div(1., s.w as f32);
        let extent = if s.mode == 3 {
            mul(s.pixel_width, 2.)
        } else {
            s.pixel_width
        };
        let edge = add(s.pixel_width, s.x);
        crate::procedural_schedule::for_each_chunk::<E>(
            rows.wrapping_add(1).wrapping_mul(s.w),
            execution,
            |start, count| {
                let mut row = start.checked_div(s.w).ok_or(PointwiseError::DivideByZero)?;
                let mut col = rem(start, s.w)?;
                for cell in start..start.wrapping_add(count) {
                    if col >= s.w {
                        col = 0;
                        row = row.wrapping_add(1);
                    }
                    let y = mul(row as f32, step);
                    let y4 = mul(y, 4.);
                    let x = mul(col as f32, step);
                    let t = &triangles[triangle_at(x, y4)];
                    let mut sx = add(
                        mul(
                            mul(
                                distance(x, y, t.a[0], t.a[1], t.midpoint[0], t.midpoint[1]),
                                t.scale[0],
                            ),
                            extent,
                        ),
                        s.x,
                    );
                    let sy = add(
                        mul(
                            sub(
                                1.,
                                mul(distance(x, y, t.a[0], t.a[1], t.b[0], t.b[1]), t.scale[1]),
                            ),
                            s.pixel_height,
                        ),
                        s.y,
                    );
                    if s.mode == 3 && sx > edge {
                        sx = sub(sx, mul(sub(sx, edge), 2.));
                    }
                    let value = if regions {
                        read_pixel(
                            &values,
                            rem(cast(f64::from(sx)), s.w)?
                                .wrapping_add(rem(cast(f64::from(sy)), s.h)?.wrapping_mul(s.w)),
                        )?
                    } else {
                        super::motion_sampling::sample_pixels(sx, sy, &values, s.w, s.h, true)?
                    };
                    write_pixel(pixels, cell, value)?;
                    col = col.wrapping_add(1);
                }
                Ok(())
            },
        )?;
        copies(pixels, s)
    })
}
pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let original = get(inputs, 0)?;
    let output = SharedImageMap::new(geometry);
    let source = required(original.as_ref())?;
    let channels = count(source, options.is_channel_rendering(), false)?;
    let region = source.region_store().map(|r| r.data(true)).transpose()?;
    let region_out = region
        .as_ref()
        .map(|v| {
            i32::try_from(v.len())
                .map_err(|_| ImageError::Overflow)
                .and_then(super::adaptive::floats)
        })
        .transpose()?;
    let (x, y, size, aspect, mode) = if f.kind() == super::PointwiseKind::WizardKaleidoscope {
        let mode = cast(f.parameter(2)?.number());
        let triangle_aspect = (1.0_f64 / libm::sin(std::f64::consts::PI / 3.0)) as f32;
        let aspect = if mode < 2 {
            1.
        } else if mode == 2 {
            triangle_aspect
        } else if mode == 3 {
            div(triangle_aspect, 2.)
        } else {
            0.1
        };
        (mul(sub(1., mul(0.8, aspect)), 0.5), 0.1, 0.8, aspect, mode)
    } else {
        let r = f.parameter(1)?.region()?;
        (r.x(), r.y(), r.size(), r.aspect(), r.mode())
    };
    let w = geometry.width();
    let h = geometry.height();
    let mut x = mul(x, w as f32);
    let mut y = mul(y, h as f32);
    let pixel_height = mul(size, h as f32);
    let pixel_width = mul(pixel_height, aspect);
    let horizontal = mul(size, aspect);
    if add(x, pixel_width) > sub(w as f32, 0.001) {
        x = sub(x, sub(add(x, pixel_width), sub(w as f32, 0.001)));
    }
    if add(y, pixel_height) >= sub(h as f32, 0.001) {
        y = sub(y, sub(add(y, pixel_height), sub(h as f32, 0.001)));
    }
    let fold = Fold {
        w,
        h,
        mode,
        x,
        y,
        size,
        pixel_height,
        pixel_width,
        horizontal,
    };
    if mode <= 1 {
        for ch in 0..channels {
            let values = source.channel_with_effects(ch, false)?;
            if values.len() == 1 {
                output.copy_indexed_constant(ch, values.read(0)?, source)?;
                continue;
            }
            let dest = output.allocate_indexed(ch, source)?;
            square(&values, &dest, fold, false, execution)?;
            source.seal_channel(ch)?;
        }
        if let (Some(values), Some(dest)) = (&region, &region_out) {
            square(values, dest, fold, true, execution)?;
        }
    } else {
        let triangles = triangles();
        for ch in 0..channels {
            let values = source.channel_with_effects(ch, false)?;
            if values.len() == 1 {
                output.copy_indexed_constant(ch, values.read(0)?, source)?;
                continue;
            }
            let dest = output.allocate_indexed(ch, source)?;
            triangular(&values, &dest, fold, &triangles, false, execution)?;
            output.seal_channel(ch)?;
        }
        if let (Some(values), Some(dest)) = (&region, &region_out) {
            triangular(values, dest, fold, &triangles, true, execution)?;
        }
    }
    if let Some(dest) = region_out {
        output.set_region_store(Some(SharedFloatStore::new(dest)?));
    }
    let size = f.parameter(0)?;
    super::imported_render::affine(
        &output,
        [size.value_at(0, time)?, size.value_at(1, time)?],
        [0.; 2],
        options.is_channel_rendering(),
        execution,
    )
    .map(Some)
}
