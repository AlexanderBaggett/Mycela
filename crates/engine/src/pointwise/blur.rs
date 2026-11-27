//! qp/i6 retained bodies and ki's adaptive box pipeline (F204/F205).
//! Resident storage only; no synthetic allocator/GC/thread success profile.
use super::numeric::{add, mul, sub};
use super::{
    PointwiseError, PointwiseFilter, PointwiseKind, PointwiseRenderEffects, SharedImageMap,
};
use crate::procedural_schedule::{for_each_chunk, for_each_dense_band, for_each_row_chunk};
use crate::source_float::{first_nan, ordered_add};
use crate::{
    FloatBuffer, ImageError, ImageGeometry, ProceduralExecution,
    image_map::{read_pixel, write_pixel},
};

// ki row/column Double products use the same first-NaN selection as the
// characterized ordered Double accumulator. This is a named worker profile,
// not a claim about every CLR/JIT operation. Finite arithmetic stays a * b.
fn box_mul64(a: f64, b: f64) -> f64 {
    match first_nan(a, b) {
        Some(value) => value,
        None => a * b,
    }
}

fn index(value: i32) -> Result<usize, ImageError> {
    usize::try_from(value).map_err(|_| ImageError::Index)
}
fn read(a: &FloatBuffer, i: i32) -> Result<f32, ImageError> {
    a.read(index(i)?)
}
fn write(a: &FloatBuffer, i: i32, v: f32) -> Result<(), ImageError> {
    a.write(index(i)?, v)
}
fn floats(n: i32) -> Result<FloatBuffer, ImageError> {
    let n = usize::try_from(n).map_err(|_| ImageError::Overflow)?;
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| ImageError::Allocation)?;
    v.resize(n, 0.);
    Ok(FloatBuffer::new(v))
}
fn doubles(n: i32) -> Result<Vec<f64>, ImageError> {
    let n = usize::try_from(n).map_err(|_| ImageError::Overflow)?;
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| ImageError::Allocation)?;
    v.resize(n, 0.);
    Ok(v)
}
fn line_read(v: &[f64], i: i32) -> Result<f64, ImageError> {
    v.get(index(i)?).copied().ok_or(ImageError::Index)
}
fn line_write(v: &mut [f64], i: i32, x: f64) -> Result<(), ImageError> {
    *v.get_mut(index(i)?).ok_or(ImageError::Index)? = x;
    Ok(())
}
fn copy(source: &FloatBuffer, dest: &FloatBuffer, count: usize) -> Result<(), ImageError> {
    if source.len() < count || dest.len() < count {
        return Err(ImageError::Argument);
    }
    // Snapshot the copied cells so overlapping handles remain valid.
    let values = source.shared_values();
    dest.write_block(0, 1, &values[..count])
}
fn plan(dimension: f64, radius: f64, maximum: i32) -> (i32, i32) {
    let mut passes = maximum;
    let mut size = 0;
    while passes > 0 {
        size = crate::framework_cast::integer(dimension * radius / f64::from(passes));
        if size > 0 {
            break;
        }
        passes = passes.wrapping_sub(1);
    }
    if size % 2 == 0 {
        size = size.wrapping_add(1);
    }
    if size >= crate::framework_cast::integer(dimension) {
        size = crate::framework_cast::integer(dimension).wrapping_sub(1);
    }
    (passes, size)
}
fn lower(d: f64, r: f64, p: (i32, i32)) -> f64 {
    if r == 0. {
        return r;
    }
    let mut v = r;
    loop {
        v -= 0.0001;
        let q = plan(d, v, 3);
        if q != p || v < 0. {
            break;
        }
    }
    v + 0.0001
}
fn upper(d: f64, r: f64, p: (i32, i32)) -> f64 {
    let mut v = r;
    loop {
        v += 0.001;
        if plan(d, v, 3) != p {
            return v;
        }
    }
}
fn scaled_radius(radius: f64, factor: f64) -> f64 {
    let value = radius / factor;
    if value > 1. { 1. } else { value }
}

/// ki.b plans the extent of a centered periodic feather without rendering it.
pub(super) fn periodic_extent(
    source: ImageGeometry,
    limit: ImageGeometry,
    radius: f64,
    force_expansion: bool,
) -> ImageGeometry {
    if radius <= 0.
        || (!force_expansion
            && source.width() >= limit.width()
            && source.height() >= limit.height())
    {
        return source;
    }
    let radius = scaled_radius(radius, 1.);
    let dimension = f64::from(limit.width().min(limit.height()));
    let initial = plan(dimension, radius, 3);
    let high = plan(dimension, upper(dimension, radius, initial), 3);
    let padding = high.1.saturating_mul(6);
    let width = source.width().saturating_add(padding);
    let height = source.height().saturating_add(padding);
    if force_expansion {
        ImageGeometry::new(width, height)
    } else {
        ImageGeometry::new(width.min(limit.width()), height.min(limit.height()))
    }
}
fn expansion<E>(
    input: &SharedImageMap,
    w: i32,
    h: i32,
    centered: bool,
    execution: ProceduralExecution,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let old = input.geometry();
    let ow = old.width();
    let oh = old.height();
    let w = w.max(ow);
    let h = h.max(oh);
    if w == ow && h == oh {
        return Ok(input.clone());
    }
    let ox = w.wrapping_sub(ow) / 2;
    let oy = h.wrapping_sub(oh) / 2;
    let geometry = if centered {
        ImageGeometry::new(w, h).with_origin(ox, oy)
    } else {
        ImageGeometry::new(w, h)
    };
    let out = SharedImageMap::new(geometry);
    let mut ch = 0;
    while ch < input.channel_count()? as i32 {
        let source = input.channel(ch, false)?;
        let dest = out.materialize(ch)?;
        let source = source.shared_values();
        let dense = source.len() > 1;
        dest.with_mut_slice(|pixels| {
            for_each_row_chunk::<E>(oh, execution, |start, count| {
                let end = start.wrapping_add(count);
                let mut y = start;
                while y < end {
                    let old_row = y.wrapping_mul(ow);
                    let new_row = y.wrapping_add(oy).wrapping_mul(w);
                    let mut x = 0;
                    while x < ow {
                        let target = x.wrapping_add(ox).wrapping_add(new_row);
                        let value =
                            read_pixel(&source, if dense { x.wrapping_add(old_row) } else { 0 })?;
                        write_pixel(pixels, target, value)?;
                        x = x.wrapping_add(1);
                    }
                    y = y.wrapping_add(1);
                }
                Ok(())
            })
        })?;
        out.seal_channel(ch)?;
        ch = ch.wrapping_add(1);
    }
    Ok(out)
}
#[derive(Clone, Copy)]
enum Axis {
    Row,
    Column,
}
struct LineJob<'a> {
    source: &'a [f32],
    width: i32,
    height: i32,
    radius: i32,
    reciprocal: f64,
    periodic: bool,
    axis: Axis,
}
impl LineJob<'_> {
    fn at(&self, line: i32, position: i32) -> i32 {
        match self.axis {
            Axis::Row => position.wrapping_add(line.wrapping_mul(self.width)),
            Axis::Column => line.wrapping_add(position.wrapping_mul(self.width)),
        }
    }
    fn apply<E>(
        &self,
        pixels: &mut [f32],
        execution: ProceduralExecution,
    ) -> Result<(), PointwiseError<E>> {
        let (lines, extent) = match self.axis {
            Axis::Row => (self.height, self.width),
            Axis::Column => (self.width, self.height),
        };
        let area = self
            .width
            .checked_mul(self.height)
            .and_then(|n| usize::try_from(n).ok());
        if lines > 0
            && extent > 0
            && (0..extent).contains(&self.radius)
            && area == Some(pixels.len())
            && area == Some(self.source.len())
            && !matches!(
                execution,
                ProceduralExecution::WorkerPool {
                    processor_count: ..=0
                }
            )
        {
            let width = self.width as usize;
            let radius = self.radius as usize;
            match self.axis {
                Axis::Row => for_each_dense_band(pixels, width, execution, |start, band| {
                    let source_start = start * width;
                    for (out, source) in band
                        .chunks_exact_mut(width)
                        .zip(self.source[source_start..].chunks_exact(width))
                    {
                        self.blur_row(source, out, radius);
                    }
                    Ok(())
                })?,
                Axis::Column => self.blur_columns(pixels, width, radius)?,
            }
            Ok(())
        } else {
            for_each_row_chunk(lines, execution, |start, count| {
                self.run(start, count, pixels)
            })
        }
    }

    fn blur_row(&self, source: &[f32], pixels: &mut [f32], radius: usize) {
        let extent = source.len();
        let mut sum = if self.periodic {
            source[extent - radius..]
                .iter()
                .copied()
                .fold(0., |sum, value| ordered_add(sum, f64::from(value)))
        } else {
            f64::from(mul(source[0], radius as f32))
        };
        for &value in &source[..=radius] {
            sum = ordered_add(sum, f64::from(value));
        }
        sum = box_mul64(sum, self.reciprocal);
        pixels[0] = sum as f32;
        for (position, target) in pixels.iter_mut().enumerate().skip(1) {
            let (outgoing, incoming) = window_edges(position, extent, radius, self.periodic);
            let delta = add(-source[outgoing], source[incoming]);
            sum = ordered_add(sum, box_mul64(f64::from(delta), self.reciprocal));
            *target = sum as f32;
        }
    }

    fn blur_columns(
        &self,
        pixels: &mut [f32],
        width: usize,
        radius: usize,
    ) -> Result<(), ImageError> {
        // A row-major sweep carries each column's previous Double sum.
        // Independent columns retain the original recurrence and tap order.
        let height = self.height as usize;
        let mut sums = doubles(self.width)?;
        let source_row = |row| &self.source[row * width..(row + 1) * width];
        if self.periodic {
            for row in height - radius..height {
                for (sum, &value) in sums.iter_mut().zip(source_row(row)) {
                    *sum = ordered_add(*sum, f64::from(value));
                }
            }
        } else {
            for (sum, &value) in sums.iter_mut().zip(source_row(0)) {
                *sum = f64::from(mul(value, radius as f32));
            }
        }
        for row in 0..=radius {
            for (sum, &value) in sums.iter_mut().zip(source_row(row)) {
                *sum = ordered_add(*sum, f64::from(value));
            }
        }
        for (target, sum) in pixels[..width].iter_mut().zip(&mut sums) {
            *sum = box_mul64(*sum, self.reciprocal);
            *target = *sum as f32;
        }
        for (index, row) in pixels.chunks_exact_mut(width).enumerate().skip(1) {
            let (outgoing, incoming) = window_edges(index, height, radius, self.periodic);
            for (((target, sum), &previous), &next) in row
                .iter_mut()
                .zip(&mut sums)
                .zip(source_row(outgoing))
                .zip(source_row(incoming))
            {
                let delta = add(-previous, next);
                *sum = ordered_add(*sum, box_mul64(f64::from(delta), self.reciprocal));
                *target = *sum as f32;
            }
        }
        Ok(())
    }

    fn run<E>(&self, start: i32, count: i32, pixels: &mut [f32]) -> Result<(), PointwiseError<E>> {
        let extent = match self.axis {
            Axis::Row => self.width,
            Axis::Column => self.height,
        };
        let mut sums = doubles(extent)?;
        let end = start.wrapping_add(count);
        let mut line = start;
        while line < end {
            line_write(&mut sums, 0, 0.)?;
            if self.periodic {
                let mut p = extent.wrapping_sub(self.radius);
                while p < extent {
                    let sum = ordered_add(
                        line_read(&sums, 0)?,
                        f64::from(read_pixel(self.source, self.at(line, p))?),
                    );
                    line_write(&mut sums, 0, sum)?;
                    p = p.wrapping_add(1);
                }
            } else {
                let seed = mul(
                    read_pixel(self.source, self.at(line, 0))?,
                    self.radius as f32,
                );
                line_write(&mut sums, 0, f64::from(seed))?;
            }
            let mut p = 0;
            while p <= self.radius {
                let sum = ordered_add(
                    line_read(&sums, 0)?,
                    f64::from(read_pixel(self.source, self.at(line, p))?),
                );
                line_write(&mut sums, 0, sum)?;
                p = p.wrapping_add(1);
            }
            let first = box_mul64(line_read(&sums, 0)?, self.reciprocal);
            line_write(&mut sums, 0, first)?;
            let mut p = 1;
            while p < extent {
                let mut outgoing = p.wrapping_sub(self.radius.wrapping_add(1));
                if outgoing < 0 {
                    outgoing = if self.periodic {
                        outgoing.wrapping_add(extent)
                    } else {
                        0
                    };
                }
                let mut incoming = p.wrapping_add(self.radius);
                if incoming >= extent {
                    incoming = if self.periodic {
                        incoming.wrapping_sub(extent)
                    } else {
                        extent.wrapping_sub(1)
                    };
                }
                // CIL NEG then Single ADD, then conv.r8; never 0f - sample.
                let negative = -read_pixel(self.source, self.at(line, outgoing))?;
                let delta = add(negative, read_pixel(self.source, self.at(line, incoming))?);
                let sum = ordered_add(
                    line_read(&sums, p.wrapping_sub(1))?,
                    box_mul64(f64::from(delta), self.reciprocal),
                );
                line_write(&mut sums, p, sum)?;
                p = p.wrapping_add(1);
            }
            let mut p = 0;
            while p < extent {
                write_pixel(pixels, self.at(line, p), line_read(&sums, p)? as f32)?;
                p = p.wrapping_add(1);
            }
            line = line.wrapping_add(1);
        }
        Ok(())
    }
}
fn window_edges(position: usize, extent: usize, radius: usize, periodic: bool) -> (usize, usize) {
    let outgoing = position.checked_sub(radius + 1).unwrap_or_else(|| {
        if periodic {
            position + extent - radius - 1
        } else {
            0
        }
    });
    let incoming = position + radius;
    let incoming = if incoming < extent {
        incoming
    } else if periodic {
        incoming - extent
    } else {
        extent - 1
    };
    (outgoing, incoming)
}

#[derive(Clone, Copy)]
struct BoxSettings {
    horizontal: bool,
    vertical: bool,
    periodic: bool,
    passes: i32,
    radius: i32,
    all_channels: bool,
}
fn box_pass<E>(
    input: &SharedImageMap,
    s: BoxSettings,
    execution: ProceduralExecution,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let out = input.geometry_copy();
    let mut scratch = None;
    let geometry = input.geometry();
    let w = geometry.width();
    let h = geometry.height();
    let count = if s.all_channels {
        input.all_channel_count()
    } else {
        input.channel_count()? as i32
    };
    for ch in 0..count {
        if s.passes < 1 {
            return Ok(input.clone());
        }
        let divisor = s.radius.wrapping_mul(2).wrapping_add(1) as f32;
        let alias = if ch < 4 {
            input.earlier_alias(ch)?
        } else {
            None
        };
        if let Some(alias) = alias {
            let a = out.channel(alias as i32, true)?;
            out.set_buffer(ch, Some(&a))?;
            continue;
        }
        let mut source = input.channel_with_effects(ch, true)?;
        if source.len() == 1 {
            out.copy_indexed_constant(ch, read(&source, 0)?, input)?;
            continue;
        }
        let dest = out.allocate_indexed(ch, input)?;
        let reciprocal = 1.0 / f64::from(divisor);
        for pass in 0..s.passes {
            if pass > 0 {
                if scratch.is_none() {
                    scratch = Some(floats(geometry.area())?);
                }
                let a = scratch.as_ref().ok_or(ImageError::NullReference)?;
                copy(&dest, a, dest.len())?;
                source = a.clone();
            }
            if s.horizontal {
                let values = source.shared_values();
                let job = LineJob {
                    source: &values,
                    width: w,
                    height: h,
                    radius: s.radius,
                    reciprocal,
                    periodic: s.periodic,
                    axis: Axis::Row,
                };
                dest.with_mut_slice(|pixels| job.apply::<E>(pixels, execution))?;
                drop(values);
                if s.vertical {
                    if scratch.is_none() {
                        scratch = Some(floats(geometry.area())?);
                    }
                    let a = scratch.as_ref().ok_or(ImageError::NullReference)?;
                    copy(&dest, a, dest.len())?;
                    source = a.clone();
                }
            }
            if s.vertical {
                let values = source.shared_values();
                let job = LineJob {
                    source: &values,
                    width: w,
                    height: h,
                    radius: s.radius,
                    reciprocal,
                    periodic: s.periodic,
                    axis: Axis::Column,
                };
                dest.with_mut_slice(|pixels| job.apply::<E>(pixels, execution))?;
                drop(values);
            }
        }
        input.seal_channel(ch)?;
        out.seal_channel(ch)?;
    }
    Ok(out)
}
/// Actual ki1257 one-pass anisotropic antialiasing reached by r7, sharing
/// planner/box owners with the existing qualified blur implementation.
pub(super) fn affine_preblur<E>(
    input: &SharedImageMap,
    rx: f64,
    ry: f64,
    execution: ProceduralExecution,
) -> Result<SharedImageMap, PointwiseError<E>> {
    if input.uses_no_maps()? {
        return Ok(input.clone());
    }
    let dimension = input.geometry().width().min(input.geometry().height());
    if dimension <= 0 {
        return Err(PointwiseError::Exception);
    }
    let (passes, radius) = plan(f64::from(dimension), rx, 1);
    let x = box_pass(
        input,
        BoxSettings {
            horizontal: true,
            vertical: false,
            periodic: true,
            passes,
            radius,
            all_channels: true,
        },
        execution,
    )?;
    let (passes, radius) = plan(f64::from(dimension), ry, 1);
    box_pass(
        &x,
        BoxSettings {
            horizontal: false,
            vertical: true,
            periodic: true,
            passes,
            radius,
            all_channels: true,
        },
        execution,
    )
}
fn interpolate<E>(
    low: &SharedImageMap,
    high: &SharedImageMap,
    original: &SharedImageMap,
    weight: f32,
    execution: ProceduralExecution,
) -> Result<SharedImageMap, PointwiseError<E>> {
    low.seal_stores_temporarily()?;
    let out = if high.same_reference(original) {
        original.geometry_copy()
    } else {
        high.clone()
    };
    let other = sub(1.0f32, weight);
    let mut ch = 0;
    while ch < low.channel_count()? as i32 {
        if let Some(alias) = low.earlier_alias(ch)? {
            out.copy_channel_within(ch, alias as i32)?;
        } else {
            let a = low.channel(ch, false)?;
            let b = high.channel(ch, false)?;
            if a.len() == 1 {
                let first = mul(read(&a, 0)?, weight);
                let second = mul(read(&b, 0)?, other);
                out.set_constant(ch, add(first, second))?;
            } else {
                let dest = out.materialize(ch)?;
                let count = a.len() as i32;
                let a = (!a.shares_storage(&dest)).then(|| a.shared_values());
                let b = (!b.shares_storage(&dest)).then(|| b.shared_values());
                dest.with_mut_slice(|pixels| {
                    for_each_chunk::<E>(count, execution, |start, count| {
                        for i in start..start.wrapping_add(count) {
                            let first = mul(
                                read_pixel(a.as_deref().map_or(&*pixels, Vec::as_slice), i)?,
                                weight,
                            );
                            let second = mul(
                                read_pixel(b.as_deref().map_or(&*pixels, Vec::as_slice), i)?,
                                other,
                            );
                            write_pixel(pixels, i, add(first, second))?;
                        }
                        Ok(())
                    })
                })?;
            }
        }
        out.seal_channel(ch)?;
        low.seal_channel(ch)?;
        high.seal_channel(ch)?;
        ch = ch.wrapping_add(1);
    }
    Ok(out)
}
#[derive(Clone, Copy)]
pub(super) struct AdaptiveSettings {
    pub(super) horizontal: bool,
    pub(super) vertical: bool,
    pub(super) radius: f64,
    pub(super) factor: f64,
    pub(super) periodic: bool,
    pub(super) all_channels: bool,
    pub(super) force_expansion: bool,
}
pub(super) fn adaptive<E>(
    input: &SharedImageMap,
    geometry: ImageGeometry,
    s: AdaptiveSettings,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    if input.uses_no_maps()? {
        return Ok(input.clone());
    }
    let radius = scaled_radius(s.radius, s.factor);
    let dimension = geometry.width().min(geometry.height());
    if dimension <= 0 {
        return Err(PointwiseError::Exception);
    }
    let initial = plan(f64::from(dimension), radius, 3);
    let lo = lower(f64::from(dimension), radius, initial);
    let low_plan = plan(f64::from(dimension), lo, 3);
    let hi = upper(f64::from(dimension), radius, initial);
    let high_plan = plan(f64::from(dimension), hi, 3);
    let old = input.geometry();
    let mut original = input.clone();
    if radius > 0.
        && (old.width() < geometry.width() || old.height() < geometry.height() || s.force_expansion)
    {
        let mut w = old.width().wrapping_add(high_plan.1.wrapping_mul(6));
        let mut h = old.height().wrapping_add(high_plan.1.wrapping_mul(6));
        if !s.force_expansion {
            if w > geometry.width() {
                w = geometry.width();
            }
            if h > geometry.height() {
                h = geometry.height();
            }
        }
        let x = old.origin()[0].wrapping_sub(w.wrapping_sub(old.width()) / 2);
        let y = old.origin()[1].wrapping_sub(h.wrapping_sub(old.height()) / 2);
        if !s.periodic {
            let mut x = x;
            let mut y = y;
            if x < 0 {
                w = w.wrapping_add(x);
                x = 0;
            }
            if y < 0 {
                h = h.wrapping_add(y);
                y = 0;
            }
            if x.wrapping_add(w) > geometry.width() {
                w = geometry.width().wrapping_sub(x);
            }
            if y.wrapping_add(h) > geometry.height() {
                h = geometry.height().wrapping_sub(y);
            }
        }
        original = expansion(&original, w, h, s.periodic, execution)?;
    }
    let settings = |p: (i32, i32)| BoxSettings {
        horizontal: s.horizontal,
        vertical: s.vertical,
        periodic: s.periodic,
        passes: p.0,
        radius: p.1,
        all_channels: s.all_channels,
    };
    let low = box_pass(&original, settings(low_plan), execution)?;
    effects.preflight(geometry.width(), 2)?;
    if lo == radius {
        return Ok(low);
    }
    let high = box_pass(&original, settings(high_plan), execution)?;
    effects.preflight(geometry.width(), 2)?;
    let weight = ((hi - radius) / (hi - lo)) as f32;
    interpolate(&low, &high, &original, weight, execution)
}
fn is_text<E: 'static>(
    p: &super::PointwiseParameter<E>,
    time: f64,
    expected: &str,
) -> Result<bool, PointwiseError<E>> {
    let ix = p.effective_index_at(time)?;
    let options = p.options()?.entries();
    let text = options.get(index(ix)?).ok_or(PointwiseError::Index)?;
    Ok(text
        .as_ref()
        .is_some_and(|s| s.iter().copied().eq(expected.encode_utf16())))
}
pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs.first().ok_or(PointwiseError::Index)?;
    if filter.kind() == PointwiseKind::GaussianBlur {
        let direction = filter.parameter(0)?;
        let horizontal =
            is_text(&direction, time, "All")? || is_text(&direction, time, "Horizontal")?;
        let vertical = is_text(&direction, time, "All")? || is_text(&direction, time, "Vertical")?;
        let periodic = is_text(&filter.parameter(2)?, time, "Yes")?;
        let radius = filter.parameter(1)?.effective_value_at(time)?;
        let factor = filter.factor();
        let source = input.as_ref().ok_or(PointwiseError::NullReference)?;
        let out = adaptive(
            source,
            geometry,
            AdaptiveSettings {
                horizontal,
                vertical,
                radius,
                factor,
                periodic,
                all_channels: false,
                force_expansion: false,
            },
            execution,
            effects,
        )?;
        if !out.same_reference(source) {
            out.copy_auxiliary(source)?;
        }
        return Ok(Some(out));
    }
    let p = filter.parameter(1)?;
    if p.effective_value_at(time)? == 0. {
        return Ok(input.clone());
    }
    let radius = p.effective_value_at(time)?;
    let factor = filter.factor();
    let source = input.as_ref().ok_or(PointwiseError::NullReference)?;
    let out = adaptive(
        source,
        geometry,
        AdaptiveSettings {
            horizontal: true,
            vertical: true,
            radius,
            factor,
            periodic: true,
            all_channels: false,
            force_expansion: false,
        },
        execution,
        effects,
    )?;
    let strength = mul(filter.parameter(0)?.effective_value_at(time)? as f32, 30.0);
    let mut ch = 0;
    while ch < source.channel_count()? as i32 {
        let blurred = out.channel(ch, false)?;
        if !blurred.is_empty() {
            let original = source.channel(ch, false)?;
            if !original.shares_storage(&blurred) {
                for_each_chunk::<E>(blurred.len() as i32, execution, |start, count| {
                    let end = start.wrapping_add(count);
                    let mut i = start;
                    while i < end {
                        let first = read(&original, i)?;
                        let difference = sub(read(&blurred, i)?, read(&original, i)?);
                        let scaled = mul(strength, difference);
                        write(&blurred, i, sub(first, scaled))?;
                        i = i.wrapping_add(1);
                    }
                    Ok(())
                })?;
            }
        }
        out.seal_channel(ch)?;
        ch = ch.wrapping_add(1);
    }
    out.copy_auxiliary(source)?;
    Ok(Some(out))
}
impl<E: 'static> PointwiseFilter<E> {
    /// Actual qp/i6/si gb radius operation only; no attached zl.s9 propagation or
    /// Factor commit is implied. Bounds callbacks/normalization remain fallible.
    pub fn scale_blur_radius(&self, scale: f64) -> Result<(), PointwiseError<E>> {
        if !matches!(
            self.kind(),
            PointwiseKind::Equalize
                | PointwiseKind::GaussianBlur
                | PointwiseKind::Sharpen
                | PointwiseKind::ShapeBlur
        ) {
            return Err(PointwiseError::UnsupportedCapability(
                "blur radius scale on another kind",
            ));
        }
        let p = self.parameter(
            if matches!(
                self.kind(),
                PointwiseKind::ShapeBlur | PointwiseKind::Equalize
            ) {
                0
            } else {
                1
            },
        )?;
        if super::parameter_ops::is_animated(|| p.retained_animation())? {
            let curve = p.animation_curve(0)?;
            let min = p.limits()?.snapshot().minimum.to_framework_f32();
            let max = p.limits()?.snapshot().maximum.to_framework_f32();
            let step = p.limits()?.snapshot().increment.to_framework_f32();
            curve
                .scale_bounds((1.0 / scale) as f32, min, max, step)
                .map_err(crate::AnimationError::Bounds)?;
            Ok(())
        } else {
            p.set_number(scaled_radius(p.number(), scale))
        }
    }
}
#[cfg(test)]
#[path = "blur_test.rs"]
mod tests;
