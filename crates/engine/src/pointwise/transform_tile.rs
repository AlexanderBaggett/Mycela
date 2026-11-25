//! fw's width-derived grid/resize worker and distinct nearest original Regions.
use super::numeric::{add, div, mul, sub};
use super::transform::*;
use super::{PointwiseError, PointwiseFilter, PointwiseRenderOptions, SharedImageMap};
use crate::{FloatBuffer, ImageError, ImageGeometry, ProceduralExecution, SharedFloatStore};
#[derive(Clone, Copy)]
struct Grid {
    w: i32,
    h: i32,
    n: i32,
    sy: f32,
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    sx: f32,
}
impl Grid {
    fn coordinates(&self, row: i32, col: i32) -> (f32, f32) {
        let v = mul(row as f32, self.sy);
        let a = mul(sub(1., v), self.a);
        let b = mul(v, self.b);
        let c = add(a, self.c);
        let d = add(b, self.d);
        let x = mul(col as f32, self.sx);
        let y = sub(1., x);
        (add(mul(y, a), mul(x, c)), add(mul(y, b), mul(x, d)))
    }
}
pub(super) fn tile<E>(
    source: Option<SharedImageMap>,
    amount: i32,
    execution: ProceduralExecution,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    if amount <= 0 {
        return Ok(source);
    }
    let source = required(source.as_ref())?;
    let w = source.geometry().width();
    let h = source.geometry().height();
    let out = SharedImageMap::new(ImageGeometry::new(w, h));
    let wf = w as f32;
    let hf = h as f32;
    let n = amount.wrapping_add(1);
    let dx = div(wf, n as f32);
    let dy = div(hf, n as f32);
    let a = dx;
    let b = mul(amount as f32, dy);
    let c = mul(amount as f32, dx);
    let d = dy;
    let grid = Grid {
        w,
        h,
        n,
        sy: div(1., h as f32),
        a,
        b,
        c,
        d,
        sx: div(1., wf),
    };
    let x0 = f64::from(a);
    let y0 = 0.;
    let x1 = x0 + f64::from(c);
    let y1 = y0 + f64::from(d);
    let diagonal = libm::sqrt(libm::pow(x1 - x0, 2.) + libm::pow(y1 - y0, 2.)) * f64::from(n);
    let res = cast(libm::ceil(f64::from(w.wrapping_mul(w)) / diagonal));
    let res = cast(super::min64(f64::from(w), f64::from(res) * 2.));
    let resized = super::image_resize::resize(source, res, res, execution)?;
    resized.seal_permanently()?;
    let scale = div(res as f32, w as f32);
    for ch in 0..resized.all_channel_count() {
        let values = resized.channel_with_effects(ch, true)?;
        if values.len() == 1 {
            out.copy_indexed_constant(ch, values.read(0)?, &resized)?;
        } else {
            let dest = out.allocate_indexed(ch, &resized)?;
            grid_worker(&grid, &values, &dest, res, res, scale, false, execution)?;
        }
        out.seal_channel(ch)?;
    }
    if let Some(region) = source.region_store() {
        let values = region.data(true)?;
        let dest = super::adaptive::floats(
            i32::try_from(values.len()).map_err(|_| ImageError::Overflow)?,
        )?;
        grid_worker(&grid, &values, &dest, w, h, 1., true, execution)?;
        out.set_region_store(Some(SharedFloatStore::new(dest)?));
    }
    Ok(Some(out))
}
#[allow(clippy::too_many_arguments)]
fn grid_worker<E>(
    grid: &Grid,
    values: &FloatBuffer,
    dest: &FloatBuffer,
    sw: i32,
    sh: i32,
    scale: f32,
    regions: bool,
    execution: ProceduralExecution,
) -> Result<(), PointwiseError<E>> {
    crate::procedural_schedule::for_each_chunk(
        grid.w.wrapping_mul(grid.h),
        execution,
        |start, count| {
            let mut row = start
                .checked_div(grid.w)
                .ok_or(PointwiseError::DivideByZero)?;
            let mut col = rem(start, grid.w)?;
            for cell in start..start.wrapping_add(count) {
                if col >= grid.w {
                    col = 0;
                    row = row.wrapping_add(1);
                }
                let (x, y) = grid.coordinates(row, col);
                let value = if regions {
                    let x = rem(cast(f64::from(mul(grid.n as f32, x))), grid.w)? as f32;
                    let y = rem(cast(f64::from(mul(grid.n as f32, y))), grid.h)? as f32;
                    read(
                        values,
                        cast(f64::from(x)).wrapping_add(cast(f64::from(y)).wrapping_mul(grid.w)),
                    )?
                } else {
                    sample(
                        mul(mul(grid.n as f32, x), scale),
                        mul(mul(grid.n as f32, y), scale),
                        values,
                        sw,
                        sh,
                    )?
                };
                write(dest, cell, value)?;
                col = col.wrapping_add(1);
            }
            Ok(())
        },
    )
}
pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let mut source = get(inputs, 0)?;
    let amount = cast(f.parameter(1)?.effective_value_at(time)?);
    if !options.is_channel_rendering() && !required(source.as_ref())?.named_channels().is_empty() {
        let copy = required(source.as_ref())?.shallow_copy()?;
        copy.clear_named_channels();
        source = Some(copy);
    }
    let output = tile(source, amount, execution)?;
    let output = required(output.as_ref())?;
    output.seal_permanently()?;
    let size = f.parameter(0)?;
    super::imported_render::affine(
        output,
        [size.value_at(0, time)?, size.value_at(1, time)?],
        [0.; 2],
        options.is_channel_rendering(),
        execution,
    )
    .map(Some)
}
