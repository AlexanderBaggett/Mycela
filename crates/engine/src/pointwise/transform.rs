//! Shared exact leaves for the six retained two-dimensional transforms.
use super::numeric::{add, div, mul, sub};
use super::{
    PointwiseError, PointwiseFilter, PointwiseKind, PointwiseRenderOptions, SharedImageMap,
};
use crate::{
    FloatBuffer, ImageError, ImageGeometry, ProceduralExecution,
    image_map::{read_pixel, write_pixel},
};
pub(super) fn owns(kind: PointwiseKind) -> bool {
    matches!(
        kind,
        PointwiseKind::Distort
            | PointwiseKind::FreeTransform
            | PointwiseKind::Kaleidoscope
            | PointwiseKind::WizardKaleidoscope
            | PointwiseKind::MoveRepeat
            | PointwiseKind::Remap
            | PointwiseKind::RotateTile
    )
}
pub(super) fn get<E>(
    inputs: &[Option<SharedImageMap>],
    ix: usize,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    inputs.get(ix).cloned().ok_or(PointwiseError::Index)
}
pub(super) fn required<E>(
    map: Option<&SharedImageMap>,
) -> Result<&SharedImageMap, PointwiseError<E>> {
    map.ok_or(PointwiseError::NullReference)
}
pub(super) fn cast(v: f64) -> i32 {
    crate::framework_cast::integer(v)
}
pub(super) fn rem<E>(a: i32, b: i32) -> Result<i32, PointwiseError<E>> {
    a.checked_rem(b).ok_or(if b == 0 {
        PointwiseError::DivideByZero
    } else {
        PointwiseError::Arithmetic
    })
}
pub(super) fn read(a: &FloatBuffer, i: i32) -> Result<f32, ImageError> {
    a.read(usize::try_from(i).map_err(|_| ImageError::Index)?)
}
pub(super) fn write(a: &FloatBuffer, i: i32, v: f32) -> Result<(), ImageError> {
    a.write(usize::try_from(i).map_err(|_| ImageError::Index)?, v)
}
pub(super) fn sample<E>(
    x: f32,
    y: f32,
    a: &FloatBuffer,
    w: i32,
    h: i32,
) -> Result<f32, PointwiseError<E>> {
    super::motion_sampling::sample(x, y, a, w, h, true)
}
// Separate r7.b(Double,Double,...), never Single-coordinate sampling.
pub(super) fn sample_double<E>(
    mut x: f64,
    mut y: f64,
    a: &FloatBuffer,
    w: i32,
    h: i32,
) -> Result<f32, PointwiseError<E>> {
    if x < 0. {
        x = x % f64::from(w) + f64::from(w);
    } else if x >= f64::from(w) {
        x %= f64::from(w);
    }
    let mut x0 = cast(x);
    let mut x1 = x0.wrapping_add(1);
    let fx = x - f64::from(x0);
    if x1 >= w {
        x0 = rem(x0, w)?;
        x1 = rem(x1, w)?;
    }
    if y < 0. {
        y = y % f64::from(h) + f64::from(h);
    } else if y >= f64::from(h) {
        y %= f64::from(h);
    }
    let mut y0 = cast(y);
    let mut y1 = y0.wrapping_add(1);
    let fy = y - f64::from(y0);
    if y1 >= h {
        y0 = rem(y0, h)?;
        y1 = rem(y1, h)?;
    }
    let tl = f64::from(read(a, x0.wrapping_add(y0.wrapping_mul(w)))?);
    let tr = f64::from(read(a, x1.wrapping_add(y0.wrapping_mul(w)))?);
    let top = tl + fx * (tr - tl);
    let bl = f64::from(read(a, x0.wrapping_add(y1.wrapping_mul(w)))?);
    let br = f64::from(read(a, x1.wrapping_add(y1.wrapping_mul(w)))?);
    let bottom = bl + fx * (br - bl);
    Ok((top + fy * (bottom - top)) as f32)
}
pub(super) fn distance(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    let a = sub(x, x0);
    let b = sub(y, y0);
    let c = sub(x1, x0);
    let d = sub(y1, y0);
    let square = add(mul(a, a), mul(b, b));
    let dot = add(mul(a, c), mul(b, d));
    let length = add(mul(c, c), mul(d, d));
    let value = f64::from(sub(square, div(mul(dot, dot), length)));
    if value > 0. {
        libm::sqrt(value) as f32
    } else {
        0.
    }
}
pub(super) fn count<E>(
    map: &SharedImageMap,
    effects: bool,
    literal_four: bool,
) -> Result<i32, PointwiseError<E>> {
    if effects {
        Ok(map.all_channel_count())
    } else if literal_four {
        Ok(4)
    } else {
        Ok(map.channel_count()? as i32)
    }
}
pub(super) fn no_scale_move<E: 'static>(f: &PointwiseFilter<E>) -> Result<bool, PointwiseError<E>> {
    let size = f.parameter(0)?;
    let location = f.parameter(1)?;
    let animated = |p: &super::PointwiseParameter<E>| {
        super::parameter_ops::is_animated(|| p.retained_animation())
    };
    let size_plain = !animated(&size)? && size.binding_index() == -1 && size.pair()? == [1.; 2];
    let location_plain =
        !animated(&location)? && location.binding_index() == -1 && location.pair()? == [0.; 2];
    Ok(size_plain && !location_plain)
}

pub(super) fn uses_repeat_scale(kind: PointwiseKind) -> bool {
    matches!(
        kind,
        PointwiseKind::MoveRepeat
            | PointwiseKind::Kaleidoscope
            | PointwiseKind::WizardKaleidoscope
            | PointwiseKind::RotateTile
    )
}

pub(super) fn retains_repeat_size<E: 'static>(
    filter: &PointwiseFilter<E>,
    flags: &[bool],
) -> Result<bool, PointwiseError<E>> {
    if filter.kind() == PointwiseKind::MoveRepeat && no_scale_move(filter)? {
        return Ok(true);
    }
    let node = filter.node();
    Ok(node.slot(0)?.node().is_some() && super::graph::reaches_scaled_reference(&node, flags)?)
}

pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    match f.kind() {
        PointwiseKind::Distort => {
            super::transform_distort::retained(f, inputs, geometry, time, execution, options)
        }
        PointwiseKind::FreeTransform => {
            super::transform_free::retained(f, inputs, geometry, time, execution, options)
        }
        PointwiseKind::Kaleidoscope | PointwiseKind::WizardKaleidoscope => {
            super::transform_kaleidoscope::retained(f, inputs, geometry, time, execution, options)
        }
        PointwiseKind::RotateTile => {
            super::transform_tile::retained(f, inputs, time, execution, options)
        }
        PointwiseKind::Remap => remap(inputs, geometry, execution, options),
        PointwiseKind::MoveRepeat => {
            let original = get(inputs, 0)?;
            let original = required(original.as_ref())?;
            if original.is_empty()? {
                return Ok(Some(original.clone()));
            }
            let location = f.parameter(1)?;
            let mut xy = [
                location.raw_value_at(0, time)?,
                location.raw_value_at(1, time)?,
            ];
            if no_scale_move(f)? {
                xy[0] /= f.factor();
                xy[1] /= f.factor();
            }
            let size = f.parameter(0)?;
            let size = [size.value_at(0, time)?, size.value_at(1, time)?];
            super::imported_render::affine(
                original,
                size,
                xy,
                options.is_channel_rendering(),
                execution,
            )
            .map(Some)
        }
        _ => Err(PointwiseError::InvalidCast),
    }
}
fn remap<E>(
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let original = get(inputs, 0)?;
    let x = get(inputs, 1)?;
    let y = get(inputs, 2)?;
    let output = SharedImageMap::new(geometry);
    let x = required(x.as_ref())?.channel(0, false)?;
    let y = required(y.as_ref())?.channel(0, false)?;
    let original = required(original.as_ref())?;
    let w = geometry.width();
    let h = geometry.height();
    for ch in 0..count(original, options.is_channel_rendering(), true)? {
        let source = original.channel_with_effects(ch, true)?;
        if source.len() == 1 {
            output.copy_indexed_constant(ch, source.read(0)?, original)?;
        } else {
            let dest = output.allocate_indexed(ch, original)?;
            let source = source.shared_values();
            let x = x.shared_values();
            let y = y.shared_values();
            dest.with_mut_slice(|pixels| {
                crate::procedural_schedule::for_each_chunk::<E>(
                    w.wrapping_mul(h),
                    execution,
                    |start, count| {
                        let mut row = start.checked_div(w).ok_or(PointwiseError::DivideByZero)?;
                        let mut col = rem(start, w)?;
                        let mut px = mul(read_pixel(&x, 0)?, w as f32);
                        let mut py = mul(read_pixel(&y, 0)?, h as f32);
                        for _ in start..start.wrapping_add(count) {
                            if col >= w {
                                col = 0;
                                row = row.wrapping_add(1);
                            }
                            let at = col.wrapping_add(row.wrapping_mul(w));
                            if x.len() > 1 {
                                px = mul(read_pixel(&x, at)?, w as f32);
                            }
                            if y.len() > 1 {
                                py = mul(read_pixel(&y, at)?, h as f32);
                            }
                            if px < 0. {
                                px = add(px % (w as f32), w as f32);
                            }
                            if py < 0. {
                                py = add(py % (h as f32), h as f32);
                            }
                            let value =
                                super::motion_sampling::sample_pixels(px, py, &source, w, h, true)?;
                            write_pixel(pixels, at, value)?;
                            col = col.wrapping_add(1);
                        }
                        Ok(())
                    },
                )
            })?;
        }
        output.seal_channel(ch)?;
    }
    Ok(Some(output))
}

impl<E: 'static> PointwiseFilter<E> {
    /// Local ok.s6: Size normalization precedes Factor. Full Slots/Contents traversal is separate.
    pub fn set_local_transform_scale(
        &self,
        scale: f64,
        flags: Option<&[bool]>,
    ) -> Result<(), PointwiseError<E>> {
        if !owns(self.kind()) {
            return Err(PointwiseError::InvalidCast);
        }
        if self.kind() != PointwiseKind::Remap {
            self.parameter(0)?.set_size_scale(scale, flags)?;
        }
        self.set_local_factor(scale);
        Ok(())
    }
    /// Local ok.py / virtual iz, then sequential extent setters and virtual gb.
    pub fn apply_transform_size_scale(&self, scale: f64) -> Result<(), PointwiseError<E>> {
        if !owns(self.kind()) {
            return Err(PointwiseError::InvalidCast);
        }
        if uses_repeat_scale(self.kind())
            && !retains_repeat_size(self, &[])?
            && self.node().slot(0)?.node().is_some()
        {
            return Err(PointwiseError::UnsupportedCapability(
                "attached transform zl.h(Boolean[]) optimizer flags",
            ));
        }
        self.apply_transform_size_scale_in_graph(scale, &[])
    }

    pub(super) fn apply_transform_size_scale_in_graph(
        &self,
        scale: f64,
        flags: &[bool],
    ) -> Result<(), PointwiseError<E>> {
        if self.kind() != PointwiseKind::Remap {
            let size = self.parameter(0)?;
            let repeated = uses_repeat_scale(self.kind());
            let retain = repeated && retains_repeat_size(self, flags)?;
            let effective = if retain { 1. } else { scale };
            let extent = [size.extent(0)? * effective, size.extent(1)? * effective];
            let extent = if repeated && !retain {
                super::polygon::round_scaled_size(extent, effective)?
            } else {
                extent
            };
            size.set_extent(0, extent[0])?;
            size.set_extent(1, extent[1])?;
        }
        self.commit_spatial_scale(scale)
    }
    /// Literal local ad, without inventing upstream optimizer guarantees.
    pub fn transform_accepts_scale(&self, _slot: i32) -> Result<bool, PointwiseError<E>> {
        Ok(match self.kind() {
            PointwiseKind::FreeTransform
            | PointwiseKind::Kaleidoscope
            | PointwiseKind::WizardKaleidoscope
            | PointwiseKind::RotateTile => true,
            PointwiseKind::MoveRepeat => !no_scale_move(self)?,
            PointwiseKind::Distort | PointwiseKind::Remap => false,
            _ => return Err(PointwiseError::InvalidCast),
        })
    }
    /// Actual local cl: Distort's Steps threshold; the other five inherit true.
    pub fn transform_cache_allowed_at(&self, time: f64) -> Result<bool, PointwiseError<E>> {
        if !owns(self.kind()) {
            return Err(PointwiseError::InvalidCast);
        }
        if self.kind() == PointwiseKind::Distort {
            Ok(self.parameter(1)?.effective_value_at(time)? < 10.)
        } else {
            Ok(true)
        }
    }
    /// Local aa result over actual raw/normalized values. Input-dependent Distort guarantee stays typed.
    pub fn transform_identity_input(&self) -> Result<i32, PointwiseError<E>> {
        let animated = |p: &super::PointwiseParameter<E>| {
            Ok::<_, PointwiseError<E>>(
                super::parameter_ops::is_animated(|| p.retained_animation())?
                    || p.has_animated_binding()?,
            )
        };
        let plain_size = |f: &Self| -> Result<bool, PointwiseError<E>> {
            let p = f.parameter(0)?;
            Ok(!animated(&p)? && p.normalized_pair()? == [1.; 2])
        };
        Ok(match self.kind() {
            PointwiseKind::FreeTransform => {
                let l = self.parameter(1)?;
                let k = self.parameter(2)?;
                let r = self.parameter(3)?;
                if !plain_size(self)?
                    || animated(&l)?
                    || animated(&k)?
                    || animated(&r)?
                    || f64::from(cast(l.pair()?[0])) != l.pair()?[0]
                    || f64::from(cast(l.pair()?[1])) != l.pair()?[1]
                    || k.pair()? != [0.; 2]
                    || r.number() != 0.
                {
                    -1
                } else {
                    0
                }
            }
            PointwiseKind::MoveRepeat => {
                let l = self.parameter(1)?;
                if !plain_size(self)?
                    || animated(&l)?
                    || f64::from(cast(l.pair()?[0])) != l.pair()?[0]
                    || f64::from(cast(l.pair()?[1])) != l.pair()?[1]
                {
                    -1
                } else {
                    0
                }
            }
            PointwiseKind::RotateTile => {
                let a = self.parameter(1)?;
                if !plain_size(self)? || animated(&a)? || a.number() != 0. {
                    -1
                } else {
                    0
                }
            }
            PointwiseKind::Distort => {
                let p = self.parameter(0)?;
                let xy = p.normalized_pair()?;
                if !animated(&p)? && xy[0] >= 10000. && xy[1] >= 10000. {
                    0
                } else {
                    return Err(PointwiseError::UnsupportedCapability(
                        "Distort input Distance constant guarantee",
                    ));
                }
            }
            PointwiseKind::Kaleidoscope
            | PointwiseKind::WizardKaleidoscope
            | PointwiseKind::Remap => -1,
            _ => return Err(PointwiseError::InvalidCast),
        })
    }
}
