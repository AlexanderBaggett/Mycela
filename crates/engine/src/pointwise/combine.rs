//! amm's retained three-input Combine. Source expression/array access order is
//! intentional. This CPU seam does not invent the ambient za/oi memory policy.
use super::combine_arrays::{Array, Pair, allocate, pair, required, reusable, screen};
use super::numeric::{add, approximately_one, div, mul, sub};
use super::{
    HsbOperation, MapArrayOperation, PointwiseChannelGuarantees, PointwiseError, PointwiseFilter,
    PointwiseKind, PointwiseRenderOptions, SharedImageMap, combine_map_arrays, hsb_arrays,
    interpolate_map_arrays,
};
use crate::{FloatBuffer, ImageError, ImageGeometry, SharedFloatStore, SharedNamedChannel};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CombineColorOperation {
    Lighten,
    Darken,
    Tint,
    Highlight,
}
fn equals(value: Option<&[u16]>, text: &str) -> bool {
    value.is_some_and(|v| v.iter().copied().eq(text.encode_utf16()))
}
fn light(top: [f32; 3], bottom: [f32; 3]) -> [f32; 3] {
    let mut lo = if top[0] < top[1] { top[0] } else { top[1] };
    if top[2] < lo {
        lo = top[2];
    }
    let mut below = if bottom[0] < bottom[1] {
        bottom[0]
    } else {
        bottom[1]
    };
    if bottom[2] < below {
        below = bottom[2];
    }
    std::array::from_fn(|i| {
        if top[i] > lo {
            let delta = sub(sub(top[i], lo), sub(bottom[i], below));
            if delta > 0.0 {
                add(bottom[i], delta)
            } else {
                bottom[i]
            }
        } else {
            bottom[i]
        }
    })
}
fn dark(top: [f32; 3], bottom: [f32; 3]) -> [f32; 3] {
    let mut hi = if top[0] > top[1] { top[0] } else { top[1] };
    if top[2] > hi {
        hi = top[2];
    }
    let mut below = if bottom[0] > bottom[1] {
        bottom[0]
    } else {
        bottom[1]
    };
    if bottom[2] > below {
        below = bottom[2];
    }
    let mut out = [0.0; 3];
    for (i, destination) in out.iter_mut().enumerate() {
        if top[i] < hi {
            let delta = sub(sub(hi, top[i]), sub(below, bottom[i]));
            if delta < 0.0 || bottom[i] < 0.0 {
                *destination = bottom[i];
            } else if delta > 0.0 {
                *destination = sub(bottom[i], delta);
                if *destination < 0.0 {
                    *destination = 0.0;
                }
            }
            // Exact zero/NaN delta leaves the fresh destination cell untouched.
        } else {
            *destination = bottom[i];
        }
    }
    out
}
fn color_values(op: CombineColorOperation, t: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    use CombineColorOperation::*;
    match op {
        Lighten => light(t, b),
        Darken => dark(t, b),
        Tint => {
            let v = div(add(add(t[0], t[1]), t[2]), 1.5);
            let out = if v > 1.0 { light(t, b) } else { dark(t, b) };
            std::array::from_fn(|i| {
                if v > 1.0 {
                    add(mul(b[i], sub(2.0, v)), mul(out[i], sub(v, 1.0)))
                } else {
                    add(mul(b[i], v), mul(out[i], sub(1.0, v)))
                }
            })
        }
        Highlight => {
            let v = div(add(add(t[0], t[1]), t[2]), 1.5);
            std::array::from_fn(|i| {
                if v > 1.0 {
                    add(
                        mul(b[i], sub(2.0, v)),
                        mul(add(t[i], mul(b[i], sub(1.0, t[i]))), sub(v, 1.0)),
                    )
                } else {
                    add(mul(b[i], v), mul(mul(t[i], b[i]), sub(1.0, v)))
                }
            })
        }
    }
}
fn collapse_three(outputs: &mut [Array; 3]) -> Result<(), ImageError> {
    let mut constant = [true; 3];
    let mut first = [0.0; 3];
    for (first, output) in first.iter_mut().zip(outputs.iter()) {
        *first = required(output.as_ref())?.read(0)?;
    }
    let n = outputs
        .iter()
        .map(|v| v.as_ref().map_or(0, FloatBuffer::len))
        .max()
        .unwrap_or(0);
    for i in 1..n {
        for (ch, constant) in constant.iter_mut().enumerate() {
            if *constant && required(outputs[ch].as_ref())?.read(i)? != first[ch] {
                *constant = false;
            }
        }
        if !constant[0] && !constant[1] && !constant[2] {
            break;
        }
    }
    for ((output, constant), first) in outputs.iter_mut().zip(constant).zip(first) {
        if constant {
            *output = Some(FloatBuffer::new(vec![first]));
        }
    }
    Ok(())
}
/// Original amm's six-input/out-three helpers. Out roots publish before first
/// input-cell reads, and remain observable on an error; RGB inputs may alias.
pub fn combine_color_arrays(
    op: CombineColorOperation,
    outputs: &mut [Option<FloatBuffer>; 3],
    inputs: [Option<FloatBuffer>; 6],
) -> Result<(), ImageError> {
    let mut lengths = [0; 6];
    for (length, input) in lengths.iter_mut().zip(&inputs) {
        *length = required(input.as_ref())?.len();
    }
    let n = lengths.iter().copied().max().unwrap_or(0);
    for out in outputs.iter_mut() {
        *out = Some(allocate(n, None)?);
    }
    let mut v = [0.0; 6];
    for (value, input) in v.iter_mut().zip(&inputs) {
        *value = required(input.as_ref())?.read(0)?;
    }
    for ix in 0..n {
        for ((value, input), length) in v.iter_mut().zip(&inputs).zip(lengths) {
            if length > 1 {
                *value = required(input.as_ref())?.read(ix)?;
            }
        }
        let out = color_values(op, [v[0], v[1], v[2]], [v[3], v[4], v[5]]);
        for (output, value) in outputs.iter().zip(out) {
            required(output.as_ref())?.write(ix, value)?;
        }
    }
    collapse_three(outputs)
}
fn component(inputs: &[Array; 3], which: usize) -> Result<Array, ImageError> {
    let mut lengths = [0; 3];
    for (length, input) in lengths.iter_mut().zip(inputs) {
        *length = required(input.as_ref())?.len();
    }
    let n = lengths[0].max(lengths[1].max(lengths[2]));
    let out = allocate(n, None)?;
    let mut v = [0.0; 3];
    for (value, input) in v.iter_mut().zip(inputs) {
        *value = required(input.as_ref())?.read(0)?;
    }
    for ix in 0..n {
        for ((value, input), length) in v.iter_mut().zip(inputs).zip(lengths) {
            if length > 1 {
                *value = required(input.as_ref())?.read(ix)?;
            }
        }
        let x = if which == 0 {
            super::math::rgb_values(v[0], v[1], v[2], true)[0]
        } else {
            // Standalone r7.b/c use ordered min/max, NOT full K's sorting or epsilon.
            let mut max = if v[0] > v[1] { v[0] } else { v[1] };
            if v[2] > max {
                max = v[2];
            }
            if which == 2 {
                max
            } else {
                let mut min = if v[0] < v[1] { v[0] } else { v[1] };
                if v[2] < min {
                    min = v[2];
                }
                if max == 0.0 {
                    0.0
                } else {
                    sub(1.0, div(min, max))
                }
            }
        };
        out.write(ix, x)?;
    }
    Ok(Some(out))
}
fn hsb(inputs: [Array; 3], op: HsbOperation) -> Result<[Array; 3], ImageError> {
    let mut outputs = std::array::from_fn(|_| None);
    hsb_arrays(op, &mut outputs, inputs)?;
    Ok(outputs)
}
fn complex(
    mode: Option<&[u16]>,
    top: &[Array],
    bottom: &[Array],
) -> Result<[Array; 3], ImageError> {
    let t: [Array; 3] = std::array::from_fn(|i| top[i].clone());
    let b: [Array; 3] = std::array::from_fn(|i| bottom[i].clone());
    if equals(mode, "Color") {
        let mut a = hsb(t, HsbOperation::RgbToHsb)?;
        a[2] = pair(
            Pair::Multiply,
            component(&b, 2)?.as_ref(),
            a[2].as_ref(),
            None,
        )?;
        return hsb(a, HsbOperation::HsbToRgb);
    }
    if equals(mode, "Hue") || equals(mode, "Saturation") || equals(mode, "Brightness") {
        let hue = component(if equals(mode, "Hue") { &t } else { &b }, 0)?;
        let saturation = component(if equals(mode, "Saturation") { &t } else { &b }, 1)?;
        let brightness = component(if equals(mode, "Brightness") { &t } else { &b }, 2)?;
        return hsb([hue, saturation, brightness], HsbOperation::HsbToRgb);
    }
    let op = if equals(mode, "Color Lighten") {
        Some(CombineColorOperation::Lighten)
    } else if equals(mode, "Color Darken") {
        Some(CombineColorOperation::Darken)
    } else if equals(mode, "Tint") {
        Some(CombineColorOperation::Tint)
    } else if equals(mode, "Highlight") {
        Some(CombineColorOperation::Highlight)
    } else {
        None
    };
    let mut out = std::array::from_fn(|_| None);
    if let Some(op) = op {
        combine_color_arrays(
            op,
            &mut out,
            [
                t[0].clone(),
                t[1].clone(),
                t[2].clone(),
                b[0].clone(),
                b[1].clone(),
                b[2].clone(),
            ],
        )?;
    }
    Ok(out)
}
fn scalar_blend(
    mode: Option<&[u16]>,
    alpha: Option<&FloatBuffer>,
    top: Option<&FloatBuffer>,
    bottom: Option<&FloatBuffer>,
) -> Result<Array, ImageError> {
    let transformed = if equals(mode, "Add") {
        pair(Pair::Add, top, bottom, None)?
    } else if equals(mode, "Subtract") {
        combine_map_arrays(MapArrayOperation::Subtract, bottom, top)?
    } else if equals(mode, "Multiply") {
        pair(Pair::Multiply, top, bottom, None)?
    } else if equals(mode, "Divide") {
        pair(Pair::Divide, top, bottom, None)?
    } else if equals(mode, "Difference") {
        combine_map_arrays(MapArrayOperation::Difference, top, bottom)?
    } else if equals(mode, "Negate") {
        combine_map_arrays(MapArrayOperation::Subtract, top, bottom)?
    } else if equals(mode, "Greatest") {
        combine_map_arrays(MapArrayOperation::Greatest, top, bottom)?
    } else if equals(mode, "Least") {
        combine_map_arrays(MapArrayOperation::Least, top, bottom)?
    } else if equals(mode, "Screen") {
        screen(top, bottom)?
    } else {
        Some(FloatBuffer::new(Vec::new()))
    };
    let preferred = reusable(transformed.as_ref(), top, bottom);
    let a = pair(
        Pair::Inverse,
        alpha,
        transformed.as_ref(),
        preferred.as_ref(),
    )?;
    let b = pair(Pair::Multiply, bottom, alpha, None)?;
    pair(Pair::Add, a.as_ref(), b.as_ref(), preferred.as_ref())
}
fn normal(
    alpha: Option<&FloatBuffer>,
    bottom_alpha: Option<&FloatBuffer>,
    divisor: Option<&FloatBuffer>,
    top: Option<&FloatBuffer>,
    bottom: Option<&FloatBuffer>,
) -> Result<Array, ImageError> {
    let a = pair(Pair::Inverse, alpha, top, None)?;
    let preferred = reusable(a.as_ref(), alpha, top);
    let b = pair(Pair::Multiply, bottom, alpha, None)?;
    let b_preferred = reusable(b.as_ref(), bottom, alpha);
    let c = pair(
        Pair::Inverse,
        bottom_alpha,
        b.as_ref(),
        b_preferred.as_ref(),
    )?;
    let sum = pair(Pair::Add, a.as_ref(), c.as_ref(), preferred.as_ref())?;
    let division_preferred = sum.as_ref().filter(|sum| {
        !c.as_ref().is_some_and(|v| sum.shares_storage(v))
            && !(a.as_ref().is_some_and(|v| sum.shares_storage(v)) && preferred.is_none())
    });
    pair(Pair::Divide, sum.as_ref(), divisor, division_preferred)
}
#[allow(clippy::too_many_arguments)]
fn blend_channel(
    mode_index: i32,
    mode: Option<&[u16]>,
    alpha: Option<&FloatBuffer>,
    bottom_alpha: Option<&FloatBuffer>,
    divisor: Option<&FloatBuffer>,
    top: Option<&FloatBuffer>,
    bottom: Option<&FloatBuffer>,
) -> Result<Array, ImageError> {
    if equals(mode, "Normal") {
        normal(alpha, bottom_alpha, divisor, top, bottom)
    } else if equals(mode, "Backlit") {
        let product = pair(Pair::Multiply, bottom, alpha, None)?;
        pair(Pair::Add, top, product.as_ref(), None)
    } else if mode_index < 10 || equals(mode, "Screen") {
        scalar_blend(mode, alpha, top, bottom)
    } else {
        interpolate_map_arrays(top, bottom, 0.5, None).map(Some)
    }
}
fn effective_alpha(
    top: FloatBuffer,
    selector: Array,
    inverse_opacity: f32,
) -> Result<FloatBuffer, ImageError> {
    let mut alpha = top;
    if let Some(selector) = selector {
        if alpha.len() == 1 && alpha.read(0)? == 0.0 {
            alpha = selector;
        } else {
            let out = allocate(selector.len().max(alpha.len()), None)?;
            if selector.len() == 1 && alpha.len() == 1 {
                let x = alpha.read(0)?;
                out.write(0, add(x, mul(sub(1.0, x), selector.read(0)?)))?;
            } else if selector.len() == 1 && alpha.len() > 1 {
                let s = selector.read(0)?;
                for i in 0..out.len() {
                    let x = alpha.read(i)?;
                    out.write(i, add(x, mul(sub(1.0, x), s)))?;
                }
            } else if selector.len() > 1 && alpha.len() == 1 {
                let x = alpha.read(0)?;
                let k = sub(1.0, x);
                for i in 0..out.len() {
                    out.write(i, add(x, mul(k, selector.read(i)?)))?;
                }
            } else {
                for i in 0..out.len() {
                    let x = alpha.read(i)?;
                    out.write(i, add(x, mul(sub(1.0, x), selector.read(i)?)))?;
                }
            }
            alpha = out;
        }
    }
    if inverse_opacity != 0.0 {
        let out = allocate(alpha.len(), None)?;
        for i in 0..out.len() {
            let x = alpha.read(i)?;
            out.write(i, add(x, mul(sub(1.0, x), inverse_opacity)))?;
        }
        alpha = out;
    }
    Ok(alpha)
}
fn normalize_selector(selector: FloatBuffer) -> Result<Array, ImageError> {
    if selector.len() == 1 && selector.read(0)? == 0.0 {
        Ok(None)
    } else {
        Ok(Some(selector))
    }
}
struct CombineSettings<'a, E> {
    filter: &'a PointwiseFilter<E>,
    time: Option<f64>,
    cancelled: &'a dyn Fn() -> bool,
}

impl<E: 'static> CombineSettings<'_, E> {
    fn choice(&self, slot: i32) -> Result<(i32, Option<Vec<u16>>), PointwiseError<E>> {
        let property = self.filter.parameter(slot)?;
        let Some(time) = self.time else {
            return Ok((property.index(), property.text()?));
        };
        let index = property.effective_index_at(time)?;
        let text = property
            .options()?
            .entries()
            .get(usize::try_from(index).map_err(|_| PointwiseError::Index)?)
            .cloned()
            .ok_or(PointwiseError::Index)?;
        Ok((index, text))
    }

    fn opacity(&self) -> Result<f32, PointwiseError<E>> {
        let property = self.filter.parameter(3)?;
        Ok(match self.time {
            Some(time) => property.effective_value_at(time)?,
            None => property.number(),
        } as f32)
    }
}

// amm.aa and amm.a7/a6 use stored properties and the already prepared slots.
pub(super) fn guarantees<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: [PointwiseChannelGuarantees; 3],
    same_image: impl FnOnce(usize, usize) -> Result<bool, PointwiseError<E>>,
) -> Result<(PointwiseChannelGuarantees, bool), PointwiseError<E>> {
    let is_animated = |index| {
        let property = filter.parameter(index)?;
        Ok::<_, PointwiseError<E>>(
            property.axis_is_animated(0)? || property.has_animated_binding()?,
        )
    };
    let mode_animated = is_animated(0)?;
    let transparency_animated = is_animated(2)?;
    let mode = (!mode_animated)
        .then(|| filter.parameter(0)?.text())
        .transpose()?
        .flatten();
    let alpha = (!transparency_animated)
        .then(|| filter.parameter(2)?.text())
        .transpose()?
        .flatten();
    let mode_is = |text| equals(mode.as_deref(), text);
    let alpha_is = |text| equals(alpha.as_deref(), text);
    let zero = |slot: usize, channel| inputs[slot].constants()[channel] == Some(0.0);
    let all_zero = |slot: usize| (0..4).all(|channel| zero(slot, channel));
    let has_scene = inputs.iter().any(PointwiseChannelGuarantees::has_scene);
    let mut passthrough = None;
    if !mode_animated && !transparency_animated && !is_animated(1)? {
        let top = usize::from(filter.parameter(1)?.index() == 1);
        let bottom = 1 - top;
        if mode_is("Multiply")
            && alpha_is("Bottom")
            && (0..3).all(|channel| {
                zero(bottom, channel)
                    || inputs[top].constants()[channel].is_some_and(approximately_one)
            })
        {
            passthrough = Some(bottom);
        } else if mode_is("Normal")
            && (zero(top, 3) || ["Greatest", "Top", "Bottom"].into_iter().any(alpha_is))
            && same_image(top, bottom)?
        {
            passthrough = Some(top);
        } else {
            let opacity = (!is_animated(3)?)
                .then(|| filter.parameter(3).map(|p| p.number()))
                .transpose()?;
            if opacity == Some(0.0)
                && !mode_is("Backlit")
                && ((zero(top, 3) && zero(bottom, 3))
                    || (zero(bottom, 3) && ["Product", "None"].into_iter().any(alpha_is))
                    || (zero(top, 3) && ["Greatest", "Sum"].into_iter().any(alpha_is))
                    || alpha_is("Bottom"))
            {
                passthrough = Some(bottom);
            } else if opacity == Some(1.0) && zero(top, 3) && zero(bottom, 3) && zero(2, 0) {
                if mode_is("Normal") || (mode_is("Add") && all_zero(bottom)) {
                    passthrough = Some(top);
                }
                if mode_is("Add") && all_zero(top) {
                    passthrough = Some(bottom);
                }
            }
            if passthrough.is_none() && all_zero(top) && all_zero(bottom) {
                passthrough = Some(top);
            }
        }
        passthrough = passthrough.filter(|&selected| {
            inputs
                .iter()
                .enumerate()
                .all(|(index, input)| index == selected || !input.has_scene())
        });
    }
    let constants = std::array::from_fn(|channel| {
        let top = zero(0, channel);
        let bottom = zero(1, channel);
        let forced_zero = if channel < 3 {
            mode_is("Multiply") && (top || bottom)
        } else {
            alpha_is("None") || (alpha_is("Product") && (top || bottom))
        };
        (forced_zero || (top && bottom)).then_some(0.0)
    });
    let uniform = passthrough.is_none() && !has_scene && constants.iter().all(Option::is_some);
    if uniform {
        passthrough = inputs
            .iter()
            .position(|input| input.constants() == constants);
    }
    if let Some(index) = passthrough {
        let input = inputs[index];
        let constants = input.constants();
        let ranges = input.in_range();
        Ok((
            PointwiseChannelGuarantees::new(
                constants,
                std::array::from_fn(|channel| {
                    constants[channel].map_or(ranges[channel], |value| (0.0..=1.0).contains(&value))
                }),
                has_scene,
                Some(index),
            ),
            uniform,
        ))
    } else {
        Ok((
            PointwiseChannelGuarantees::new(constants, [false; 4], has_scene, None),
            uniform,
        ))
    }
}

impl<E> PointwiseFilter<E> {
    /// Raw unadjusted amm.g opacity getter. General animation remains outside
    /// the stored-property render seam, as with the other PointwiseFilter kinds.
    pub fn combine_opacity(&self) -> Result<f64, PointwiseError<E>> {
        check(self)?;
        Ok(self.parameter(3)?.number())
    }
    /// Exact amm private stripe body: explicit count and ordered input roles,
    /// caller-owned output, explicit mode arguments. No k8 region/tail finish,
    /// First Input resolution or ambient memory-policy guess is added here.
    #[allow(clippy::too_many_arguments)]
    pub fn render_combine_stripes_into(
        &self,
        inputs: &[Option<SharedImageMap>],
        iterations: i32,
        output: Option<&SharedImageMap>,
        combine_effects: bool,
        mode_index: i32,
        mode: Option<&[u16]>,
    ) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        check(self)?;
        let top = inputs.first().cloned().ok_or(PointwiseError::Index)?;
        let bottom = inputs.get(1).cloned().ok_or(PointwiseError::Index)?;
        let selector = inputs.get(2).cloned().ok_or(PointwiseError::Index)?;
        stripe(
            &CombineSettings {
                filter: self,
                time: None,
                cancelled: &|| false,
            },
            top.as_ref(),
            bottom.as_ref(),
            selector.as_ref(),
            iterations,
            output,
            combine_effects,
            mode_index,
            mode,
        )
    }
    /// Supplied original za.a(width,oi.e) RESULT, not a fabricated host provider.
    /// Default render leaves ambient-dependent large dense jobs unsupported.
    pub fn render_combine_with_capacity(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
        options: PointwiseRenderOptions,
        capacity_available: bool,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>>
    where
        E: 'static,
    {
        check(self)?;
        render(
            self,
            inputs,
            width,
            height,
            options,
            Some(capacity_available),
            None,
        )
    }
}
fn check<E>(filter: &PointwiseFilter<E>) -> Result<(), PointwiseError<E>> {
    if filter.kind() != PointwiseKind::Combine {
        Err(PointwiseError::UnsupportedCapability(
            "Combine helper on another node kind",
        ))
    } else {
        Ok(())
    }
}
fn map<E>(v: Option<&SharedImageMap>) -> Result<&SharedImageMap, PointwiseError<E>> {
    v.ok_or(PointwiseError::NullReference)
}
fn has_dense_input<E>(
    top: Option<&SharedImageMap>,
    bottom: Option<&SharedImageMap>,
    selector: Option<&SharedImageMap>,
) -> Result<bool, PointwiseError<E>> {
    let mut dense = map(selector)?.uses_plane(0)?;
    for ch in 0..4 {
        if dense {
            break;
        }
        if map(top)?.uses_plane(ch)? || map(bottom)?.uses_plane(ch)? {
            dense = true;
        }
    }
    Ok(dense)
}
fn count_with_query<E>(
    width: i32,
    height: i32,
    top: Option<&SharedImageMap>,
    bottom: Option<&SharedImageMap>,
    selector: Option<&SharedImageMap>,
    query: &mut dyn FnMut(i32) -> Result<bool, PointwiseError<E>>,
) -> Result<i32, PointwiseError<E>> {
    // Source amm probes actual roles first; za's width guard avoids ambient
    // observations below500. Do not move area/base==1 before this lazy query.
    if !has_dense_input(top, bottom, selector)? || width < 500 {
        return Ok(1);
    }
    if query(width)? {
        return Ok(1);
    }
    let area = width.wrapping_mul(height);
    let base = (area / 8_000_000).max(1);
    for candidate in base..base.saturating_mul(2) {
        if area % candidate == 0 {
            return Ok(candidate);
        }
    }
    Ok(base)
}

/// Legacy no-provider/fixed-result chooser. Its bounded base==1 shortcut is
/// retained for existing callers; actual ambient effects use count_with_query
/// before computing area/base, including when the eventual count is one.
pub(super) fn count<E>(
    width: i32,
    height: i32,
    top: Option<&SharedImageMap>,
    bottom: Option<&SharedImageMap>,
    selector: Option<&SharedImageMap>,
    capacity: Option<bool>,
) -> Result<i32, PointwiseError<E>> {
    let dense = has_dense_input(top, bottom, selector)?;
    let area = width.wrapping_mul(height);
    let base = (area / 8_000_000).max(1);
    if !dense || width < 500 || capacity == Some(true) || base == 1 {
        return Ok(1);
    }
    if capacity.is_none() {
        return Err(PointwiseError::UnsupportedCapability(
            "Combine stripe chooser requires actual za/oi ambient capacity result",
        ));
    }
    for candidate in base..base.saturating_mul(2) {
        if area % candidate == 0 {
            return Ok(candidate);
        }
    }
    Ok(base)
}
/// Actual static count helper over three supplied retained roles. Its ambient
/// large-dense branch is explicitly unsupported without the real capacity result.
pub fn combine_iteration_count<E>(
    inputs: &[Option<SharedImageMap>],
    width: i32,
    height: i32,
) -> Result<i32, PointwiseError<E>> {
    let top = inputs.first().cloned().ok_or(PointwiseError::Index)?;
    let bottom = inputs.get(1).cloned().ok_or(PointwiseError::Index)?;
    let selector = inputs.get(2).cloned().ok_or(PointwiseError::Index)?;
    count(
        width,
        height,
        top.as_ref(),
        bottom.as_ref(),
        selector.as_ref(),
        None,
    )
}
/// Actual lazy ambient predicate seam. The callback must evaluate the concrete
/// memory policy from real observations; a fixed source-result fixture belongs
/// to render_combine_with_capacity instead. A dense width>=500 queries even if
/// its wrapped area can only select count1. Other dense probes can fail first.
pub fn combine_iteration_count_with_capacity_query<E>(
    inputs: &[Option<SharedImageMap>],
    width: i32,
    height: i32,
    query: &mut impl FnMut(i32) -> Result<bool, PointwiseError<E>>,
) -> Result<i32, PointwiseError<E>> {
    let top = inputs.first().cloned().ok_or(PointwiseError::Index)?;
    let bottom = inputs.get(1).cloned().ok_or(PointwiseError::Index)?;
    let selector = inputs.get(2).cloned().ok_or(PointwiseError::Index)?;
    count_with_query(
        width,
        height,
        top.as_ref(),
        bottom.as_ref(),
        selector.as_ref(),
        query,
    )
}

enum CapacitySource<'a, E> {
    Fixed(Option<bool>),
    Query(&'a mut dyn FnMut(i32) -> Result<bool, PointwiseError<E>>),
}
pub(super) fn render<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    width: i32,
    height: i32,
    options: PointwiseRenderOptions,
    capacity: Option<bool>,
    time: Option<f64>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    render_with_capacity_source(
        &CombineSettings {
            filter,
            time,
            cancelled: &|| false,
        },
        inputs,
        width,
        height,
        options,
        CapacitySource::Fixed(capacity),
    )
}
pub(super) fn render_with_capacity_query<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    options: PointwiseRenderOptions,
    time: Option<f64>,
    query: &mut dyn FnMut(i32) -> Result<bool, PointwiseError<E>>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    render_with_capacity_source(
        &CombineSettings {
            filter,
            time,
            cancelled,
        },
        inputs,
        geometry.width(),
        geometry.height(),
        options,
        CapacitySource::Query(query),
    )
}
fn render_with_capacity_source<E: 'static>(
    settings: &CombineSettings<'_, E>,
    inputs: &[Option<SharedImageMap>],
    width: i32,
    height: i32,
    options: PointwiseRenderOptions,
    capacity: CapacitySource<'_, E>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    super::check_render_cancelled(settings.cancelled)?;
    let mut top = inputs.first().cloned().ok_or(PointwiseError::Index)?;
    let mut bottom = inputs.get(1).cloned().ok_or(PointwiseError::Index)?;
    let selector = inputs.get(2).cloned().ok_or(PointwiseError::Index)?;
    let (mode_index, mode) = settings.choice(0)?;
    if equals(settings.choice(1)?.1.as_deref(), "Input 2") {
        std::mem::swap(&mut top, &mut bottom);
    }
    let iterations = match capacity {
        CapacitySource::Fixed(value) => count(
            width,
            height,
            top.as_ref(),
            bottom.as_ref(),
            selector.as_ref(),
            value,
        ),
        CapacitySource::Query(query) => count_with_query(
            width,
            height,
            top.as_ref(),
            bottom.as_ref(),
            selector.as_ref(),
            query,
        ),
    }?;
    super::check_render_cancelled(settings.cancelled)?;
    let output = SharedImageMap::new(ImageGeometry::new(width, height));
    let effects = options.is_channel_rendering() && equals(settings.choice(4)?.1.as_deref(), "Yes");
    let mut channels = 4;
    if effects {
        channels += map(top.as_ref())?
            .named_channels()
            .len()
            .min(map(bottom.as_ref())?.named_channels().len());
    }
    if iterations == 1 {
        one_pass(
            settings,
            top.as_ref(),
            bottom.as_ref(),
            selector.as_ref(),
            &output,
            channels,
            mode_index,
            mode.as_deref(),
        )?;
    } else {
        stripe(
            settings,
            top.as_ref(),
            bottom.as_ref(),
            selector.as_ref(),
            iterations,
            Some(&output),
            effects,
            mode_index,
            mode.as_deref(),
        )?;
    }
    let top = map(top.as_ref())?;
    let bottom = map(bottom.as_ref())?;
    let selector = map(selector.as_ref())?;
    let region = top.region_store();
    output.set_region_store(if region.is_none() {
        bottom.region_store()
    } else {
        region
    });
    if options.is_channel_rendering() {
        if !effects {
            output.merge_effects(&[
                Some(top.clone()),
                Some(bottom.clone()),
                Some(selector.clone()),
            ])?;
        } else {
            let a = top.named_channels().len();
            let b = bottom.named_channels().len();
            let longer = if a > b { top } else { bottom };
            for i in a.min(b)..a.max(b) {
                output.append_named(
                    longer
                        .named_channel(i as i32)?
                        .ok_or(ImageError::NullReference)?,
                )?;
            }
        }
    }
    Ok(Some(output))
}
fn arrays<E: 'static>(
    settings: &CombineSettings<'_, E>,
    top: &[Array],
    bottom: &[Array],
    selector: FloatBuffer,
    alpha_output: bool,
    mode_index: i32,
    mode: Option<&[u16]>,
) -> Result<Vec<Array>, PointwiseError<E>> {
    let raw_top = required(top[3].as_ref())?.clone();
    let raw_bottom = required(bottom[3].as_ref())?.clone();
    let selector = normalize_selector(selector)?;
    let n = top.len().min(bottom.len()).max(3);
    let mut out = vec![None; n.max(4)];
    let inverse_opacity = sub(1.0, settings.opacity()?);
    let alpha = effective_alpha(raw_top.clone(), selector.clone(), inverse_opacity)?;
    if equals(mode, "Normal")
        || equals(mode, "Backlit")
        || mode_index < 10
        || equals(mode, "Screen")
    {
        let divisor = if equals(mode, "Normal") {
            let intermediate = pair(Pair::Inverse, Some(&raw_bottom), Some(&alpha), None)?;
            let preferred = reusable(intermediate.as_ref(), Some(&raw_bottom), Some(&alpha));
            pair(
                Pair::ComplementAdd,
                Some(&alpha),
                intermediate.as_ref(),
                preferred.as_ref(),
            )?
        } else {
            None
        };
        for (ch, destination) in out.iter_mut().take(n).enumerate() {
            if ch != 3 {
                super::check_render_cancelled(settings.cancelled)?;
                *destination = blend_channel(
                    mode_index,
                    mode,
                    Some(&alpha),
                    Some(&raw_bottom),
                    divisor.as_ref(),
                    top[ch].as_ref(),
                    bottom[ch].as_ref(),
                )?;
            }
        }
    } else {
        let transformed = complex(mode, top, bottom)?;
        for (ch, destination) in out.iter_mut().take(3).enumerate() {
            super::check_render_cancelled(settings.cancelled)?;
            let a = pair(Pair::Inverse, Some(&alpha), transformed[ch].as_ref(), None)?;
            let b = pair(Pair::Multiply, bottom[ch].as_ref(), Some(&alpha), None)?;
            let preferred = reusable(a.as_ref(), Some(&alpha), transformed[ch].as_ref());
            *destination = pair(Pair::Add, a.as_ref(), b.as_ref(), preferred.as_ref())?;
        }
        for (ch, destination) in out.iter_mut().take(n).enumerate().skip(4) {
            super::check_render_cancelled(settings.cancelled)?;
            *destination = Some(interpolate_map_arrays(
                top[ch].as_ref(),
                bottom[ch].as_ref(),
                0.5,
                None,
            )?);
        }
    }
    if alpha_output {
        super::check_render_cancelled(settings.cancelled)?;
        out[3] = transparency(
            settings,
            Some(&raw_top),
            Some(&raw_bottom),
            selector.as_ref(),
        )?;
    }
    Ok(out)
}
#[allow(clippy::too_many_arguments)]
fn one_pass<E: 'static>(
    settings: &CombineSettings<'_, E>,
    top: Option<&SharedImageMap>,
    bottom: Option<&SharedImageMap>,
    selector: Option<&SharedImageMap>,
    output: &SharedImageMap,
    channels: usize,
    mode_index: i32,
    mode: Option<&[u16]>,
) -> Result<(), PointwiseError<E>> {
    let mut a = vec![None; channels];
    let mut b = vec![None; channels];
    for (ch, (a, b)) in a.iter_mut().zip(&mut b).enumerate() {
        *a = Some(map(top)?.channel_with_effects(ch as i32, false)?);
        *b = Some(map(bottom)?.channel_with_effects(ch as i32, false)?);
    }
    let selector = map(selector)?.channel(0, false)?;
    let result = arrays(settings, &a, &b, selector, true, mode_index, mode)?;
    for (ch, value) in result.iter().take(4).enumerate() {
        output.set_buffer(ch as i32, value.as_ref())?;
        output.seal_channel(ch as i32)?;
    }
    for (ch, value) in result.iter().enumerate().skip(4) {
        let name = map(top)?
            .named_channel((ch - 4) as i32)?
            .ok_or(ImageError::NullReference)?
            .name_utf16()
            .to_vec();
        let store = SharedFloatStore::new(required(value.as_ref())?.clone())?;
        output.append_named(SharedNamedChannel::new(Some(name), Some(store)))?;
        output.seal_channel(ch as i32)?;
    }
    output.compress_uniform()?;
    Ok(())
}
fn stripe_bounds(index: i32, total: i32, area: i32) -> (i32, i32) {
    // Original conv.i4 result at this division/cast boundary is profile-specific;
    // normal positive count/ranges are the actual supported stripe helper seam.
    (
        ((f64::from(index) / f64::from(total)) * f64::from(area)) as i32,
        ((f64::from(index.wrapping_add(1)) / f64::from(total)) * f64::from(area)) as i32,
    )
}
fn part(
    image: &SharedImageMap,
    channel: i32,
    index: i32,
    total: i32,
) -> Result<FloatBuffer, ImageError> {
    if channel < 4 {
        if !image.uses_plane(channel)? || total == 1 {
            return image.channel(channel, false);
        }
        let (start, end) = stripe_bounds(index, total, image.geometry().area());
        image.range(channel, start, end)
    } else {
        let data = image
            .named_channel(channel - 4)?
            .ok_or(ImageError::NullReference)?
            .data()
            .ok_or(ImageError::NullReference)?;
        if data.len() == 1 || total == 1 {
            data.data(true)
        } else {
            let (start, end) = stripe_bounds(index, total, image.geometry().area());
            data.data_range(start, end, true)
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn stripe<E: 'static>(
    settings: &CombineSettings<'_, E>,
    top: Option<&SharedImageMap>,
    bottom: Option<&SharedImageMap>,
    selector: Option<&SharedImageMap>,
    iterations: i32,
    output: Option<&SharedImageMap>,
    effects: bool,
    mode_index: i32,
    mode: Option<&[u16]>,
) -> Result<(), PointwiseError<E>> {
    let mut final_rgb: [Array; 3] = std::array::from_fn(|_| None);
    for ix in 0..iterations {
        super::check_render_cancelled(settings.cancelled)?;
        let mut a = vec![None; 4];
        let mut b = vec![None; 4];
        for (ch, (a, b)) in a.iter_mut().zip(&mut b).enumerate() {
            *a = Some(part(map(top)?, ch as i32, ix, iterations)?);
            *b = Some(part(map(bottom)?, ch as i32, ix, iterations)?);
        }
        let selector = part(map(selector)?, 0, ix, iterations)?;
        let result = arrays(settings, &a, &b, selector, false, mode_index, mode)?;
        if ix == 0 {
            for (ch, destination) in final_rgb.iter_mut().enumerate() {
                let length = if required(result[ch].as_ref())?.len() == 1 {
                    1
                } else {
                    usize::try_from(map(output)?.geometry().area())
                        .map_err(|_| ImageError::Overflow)?
                };
                *destination = Some(allocate(length, None)?);
            }
        }
        let (start, end) = stripe_bounds(ix, iterations, map(output)?.geometry().area());
        let length = end.wrapping_sub(start);
        for (slot, result) in final_rgb.iter_mut().zip(&result) {
            let value = required(result.as_ref())?;
            if value.len() == 1 {
                *slot = Some(value.clone());
            } else {
                let destination = required(slot.as_ref())?;
                if (value.len() as i64) < i64::from(length)
                    || (destination.len() as i64) < i64::from(start.wrapping_add(length))
                {
                    return Err(PointwiseError::Exception);
                }
                let start = usize::try_from(start).map_err(|_| ImageError::Argument)?;
                let length = usize::try_from(length).map_err(|_| ImageError::Argument)?;
                // Array.Copy memmove semantics when aliases happen to overlap.
                let values = value.values();
                if length > values.len()
                    || start
                        .checked_add(length)
                        .is_none_or(|end| end > destination.len())
                {
                    return Err(ImageError::Argument.into());
                }
                for (i, value) in values.into_iter().take(length).enumerate() {
                    destination.write(start + i, value)?;
                }
            }
        }
    }
    for (ch, value) in final_rgb.iter().enumerate() {
        map(output)?.set_buffer(ch as i32, value.as_ref())?;
        map(output)?.seal_channel(ch as i32)?;
    }
    map(output)?.compress_uniform()?;
    named_and_alpha(
        settings, top, bottom, selector, output, effects, mode_index, mode,
    )
}
fn transparency<E: 'static>(
    settings: &CombineSettings<'_, E>,
    top: Option<&FloatBuffer>,
    bottom: Option<&FloatBuffer>,
    selector: Option<&FloatBuffer>,
) -> Result<Array, PointwiseError<E>> {
    let (_, mode) = settings.choice(2)?;
    let mode = mode.as_deref();
    if equals(mode, "Normal") {
        let zero = FloatBuffer::new(vec![0.0]);
        let selector = selector.unwrap_or(&zero);
        let top = required(top)?;
        let bottom = required(bottom)?;
        let n = selector.len().max(top.len().max(bottom.len()));
        let opacity = settings.opacity()?;
        let mut s = selector.read(0)?;
        let mut t = top.read(0)?;
        let mut b = bottom.read(0)?;
        let out = allocate(n, None)?;
        for i in 0..n {
            if selector.len() > 1 {
                s = selector.read(i)?;
            }
            if top.len() > 1 {
                t = top.read(i)?;
            }
            if bottom.len() > 1 {
                b = bottom.read(i)?;
            }
            out.write(
                i,
                mul(sub(1.0, mul(mul(sub(1.0, s), sub(1.0, t)), opacity)), b),
            )?;
        }
        Ok(Some(out))
    } else if equals(mode, "Product") {
        Ok(pair(Pair::Multiply, top, bottom, None)?)
    } else if equals(mode, "Greatest") {
        Ok(combine_map_arrays(
            MapArrayOperation::Greatest,
            top,
            bottom,
        )?)
    } else if equals(mode, "Sum") {
        Ok(pair(Pair::Add, top, bottom, None)?)
    } else if equals(mode, "Top") {
        Ok(top.cloned())
    } else if equals(mode, "Bottom") {
        Ok(bottom.cloned())
    } else {
        Ok(Some(FloatBuffer::new(vec![0.0])))
    }
}
#[allow(clippy::too_many_arguments)]
fn named_and_alpha<E: 'static>(
    settings: &CombineSettings<'_, E>,
    top: Option<&SharedImageMap>,
    bottom: Option<&SharedImageMap>,
    selector: Option<&SharedImageMap>,
    output: Option<&SharedImageMap>,
    effects: bool,
    mode_index: i32,
    mode: Option<&[u16]>,
) -> Result<(), PointwiseError<E>> {
    let raw_top = map(top)?.channel(3, false)?;
    let raw_bottom = map(bottom)?.channel(3, false)?;
    let s = normalize_selector(map(selector)?.channel(0, false)?)?;
    let top = map(top)?;
    let bottom = map(bottom)?;
    let selector = map(selector)?;
    let n = top
        .named_channels()
        .len()
        .min(bottom.named_channels().len());
    if effects && n > 0 {
        let inverse_opacity = sub(1.0, settings.opacity()?);
        let alpha = effective_alpha(raw_top, s, inverse_opacity)?;
        let divisor = if equals(mode, "Normal") {
            let intermediate = pair(Pair::Inverse, Some(&raw_bottom), Some(&alpha), None)?;
            let preferred = reusable(intermediate.as_ref(), Some(&raw_bottom), Some(&alpha));
            pair(
                Pair::ComplementAdd,
                Some(&alpha),
                intermediate.as_ref(),
                preferred.as_ref(),
            )?
        } else {
            None
        };
        for ch in 0..n {
            super::check_render_cancelled(settings.cancelled)?;
            let a = top
                .named_channel(ch as i32)?
                .ok_or(ImageError::NullReference)?
                .data()
                .ok_or(ImageError::NullReference)?
                .data(true)?;
            let b = bottom
                .named_channel(ch as i32)?
                .ok_or(ImageError::NullReference)?
                .data()
                .ok_or(ImageError::NullReference)?
                .data(true)?;
            let result = blend_channel(
                mode_index,
                mode,
                Some(&alpha),
                Some(&raw_bottom),
                divisor.as_ref(),
                Some(&a),
                Some(&b),
            )?;
            let name = top
                .named_channel(ch as i32)?
                .ok_or(ImageError::NullReference)?
                .name_utf16()
                .to_vec();
            map(output)?.add_named_store(
                Some(name),
                Some(SharedFloatStore::new(required(result.as_ref())?.clone())?),
            )?;
            map(output)?.seal_named()?;
        }
    }
    // Original re-reads all three arrays, including the unnormalized selector.
    super::check_render_cancelled(settings.cancelled)?;
    let top = top.channel(3, false)?;
    let bottom = bottom.channel(3, false)?;
    let selector = selector.channel(0, false)?;
    let alpha = transparency(settings, Some(&top), Some(&bottom), Some(&selector))?;
    map(output)?.set_buffer(3, alpha.as_ref())?;
    Ok(())
}
