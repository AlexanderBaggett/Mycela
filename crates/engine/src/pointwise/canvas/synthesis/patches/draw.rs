use super::super::stamp::TileColor;
use super::{
    LayerColor, Photo,
    mask::{PatchMask, PreparedPatch},
    path::Point,
};
use crate::PointwiseError;

pub(super) struct Target<'a> {
    pub(super) size: [usize; 2],
    pub(super) pixels: &'a mut [[f32; 4]],
    pub(super) photo: &'a Photo,
    pub(super) color: &'a LayerColor,
    pub(super) cancelled: &'a dyn Fn() -> bool,
}

impl Target<'_> {
    pub(super) fn place<E>(
        &mut self,
        patch: &PreparedPatch,
        at: [f32; 2],
        mut scale: [f32; 2],
        angle: f32,
        random: f64,
    ) -> Result<(), PointwiseError<E>> {
        let source_axis = usize::from(self.photo.size[0] <= self.photo.size[1]);
        let ratio = self.size[source_axis] as f32 / self.photo.size[source_axis] as f32;
        scale = scale.map(|value| value * ratio);
        if scale.contains(&0.) {
            return Ok(());
        }
        if scale
            .into_iter()
            .chain(at)
            .chain([angle])
            .any(|value| !value.is_finite())
        {
            return Err(PointwiseError::Argument);
        }
        let [at_x, at_y] = at.map(f64::from);
        let [scale_x, scale_y] = scale.map(f64::from);
        let angle = f64::from(0. - angle) * (std::f64::consts::PI / 180.);
        let sin = angle.sin();
        let cos = angle.cos();
        let [center_x, center_y] = patch.bounds.center();
        let forward_offset = [
            (-center_x * scale_x * cos - -center_y * scale_y * sin) + at_x,
            (-center_x * scale_x * sin + -center_y * scale_y * cos) + at_y,
        ];
        let forward = |[x, y]: Point| {
            [
                x * (scale_x * cos) + y * (-scale_y * sin) + forward_offset[0],
                x * (scale_x * sin) + y * (scale_y * cos) + forward_offset[1],
            ]
        };
        let [inverse_x, inverse_y] = scale.map(|value| f64::from(1. / value));
        let inverse_offset = [
            (-at_x * cos - -at_y * -sin) * inverse_x + center_x,
            (-at_x * -sin + -at_y * cos) * inverse_y + center_y,
        ];
        let inverse = |[x, y]: Point| {
            [
                x * (cos * inverse_x) + y * (sin * inverse_x) + inverse_offset[0],
                x * (-sin * inverse_y) + y * (cos * inverse_y) + inverse_offset[1],
            ]
        };
        let path = patch.outline.transform(forward)?;
        let color = TileColor::new(self.color, Some(random))?;
        let top = path.bounds.min[1];
        let bottom = top + path.bounds.size[1];
        let rows = (bottom.ceil() - top.floor()) as usize;
        let first_coverage = 1. - (top - top.floor());
        let first_row = wrapped(top, self.size[1] as f64) as usize;
        let mut crossings = Vec::new();
        // The original adds one row at ceil(bottom) with zero coverage.
        for row in 0..rows {
            crate::pointwise::check_render_cancelled(self.cancelled)?;
            let y = if row == 0 {
                top
            } else {
                top.floor() + row as f64
            };
            let coverage = if row == 0 { first_coverage } else { 1. };
            path.crossings(y, &mut crossings);
            let output_row = (first_row + row) % self.size[1];
            for span in crossings.chunks_exact(2) {
                let left = span[0].floor();
                let right = span[1].ceil();
                let width = (right - left) as usize;
                if width == 0 {
                    continue;
                }
                let start = inverse([left, y]);
                let end = inverse([right, y]);
                let step: Point = std::array::from_fn(|i| (end[i] - start[i]) / width as f64);
                let minifying = step[0] > 1.25 || step[1] > 1.25;
                let mut source = start;
                let mut mask = MaskSample::new(&patch.color, start, step);
                let mut shadow = patch
                    .shadow
                    .as_ref()
                    .map(|value| MaskSample::new(value, start, step));
                let source_rgb_dense = self.photo.pixels[..3].iter().all(|plane| plane.len() > 1);
                for column in 0..width {
                    let opacity = if column == 0 {
                        1. - (span[0] - left) as f32
                    } else if column == width - 1 {
                        let fraction = (span[1] - span[1].floor()) as f32;
                        if fraction < 0.0001 { 1. } else { fraction }
                    } else {
                        1.
                    };
                    let opacity = (f64::from(opacity) * coverage) as f32;
                    let mut transparency = 1.;
                    let mut rgb = [0.; 3];
                    if contains(source, self.photo.size) {
                        let alpha = sample(source, self.photo.size, &self.photo.pixels[3], true);
                        transparency =
                            1. - (1. - alpha) * (1. - mask.read(minifying, false)) * opacity;
                        if transparency < 0.9999 {
                            let rgb_end = std::array::from_fn(|i| source[i] + step[i]);
                            rgb = std::array::from_fn(|i| {
                                let values = &self.photo.pixels[i];
                                if minifying && values.len() > 1 {
                                    footprint(
                                        source,
                                        rgb_end,
                                        self.photo.size,
                                        values,
                                        source_rgb_dense,
                                    )
                                } else {
                                    sample(source, self.photo.size, values, true)
                                }
                            });
                        }
                        rgb = color.apply(rgb);
                    }
                    if let Some(shadow) = shadow.as_mut() {
                        if contains(shadow.at, shadow.mask.size) {
                            let value = shadow.read(minifying, true);
                            let shadow_alpha = 1. - (1. - value) * opacity;
                            if shadow_alpha < 0.9999 && transparency > 0.0001 {
                                if transparency > 0.9999 {
                                    rgb = [0.; 3];
                                    transparency = shadow_alpha;
                                } else {
                                    let above = 1. - transparency;
                                    let below = 1. - shadow_alpha;
                                    let total = above + below * (1. - above);
                                    transparency = 1. - total;
                                    if total > 0. {
                                        rgb = rgb.map(|value| value * (above / total));
                                    }
                                }
                            }
                        }
                        shadow.advance();
                    }
                    let output_column =
                        (left as i64 + column as i64).rem_euclid(self.size[0] as i64) as usize;
                    let index = output_row * self.size[0] + output_column;
                    composite(&mut self.pixels[index], rgb, transparency);
                    mask.advance();
                    for axis in 0..2 {
                        source[axis] += step[axis];
                    }
                }
            }
        }
        Ok(())
    }
}

fn contains(point: Point, size: [usize; 2]) -> bool {
    (0..2).all(|axis| point[axis] > -1. && point[axis] < size[axis] as f64)
}

struct MaskSample<'a> {
    mask: &'a PatchMask,
    at: Point,
    step: Point,
    footprint: Point,
}

impl<'a> MaskSample<'a> {
    fn new(mask: &'a PatchMask, start: Point, step: Point) -> Self {
        Self {
            mask,
            at: std::array::from_fn(|i| start[i] - mask.origin[i]),
            step: std::array::from_fn(|i| step[i] * mask.scale[i]),
            footprint: step,
        }
    }
    fn advance(&mut self) {
        for axis in 0..2 {
            self.at[axis] += self.step[axis];
        }
    }
    fn read(&self, minifying: bool, shadow: bool) -> f32 {
        if minifying {
            let step = if shadow { self.footprint } else { self.step };
            let end = std::array::from_fn(|i| self.at[i] + step[i]);
            footprint(self.at, end, self.mask.size, &self.mask.pixels, false)
        } else {
            sample(self.at, self.mask.size, &self.mask.pixels, shadow)
        }
    }
}

fn composite(destination: &mut [f32; 4], above: [f32; 3], mut transparency: f32) {
    if transparency >= 0.9999 {
        return;
    }
    if destination[3] > 0.9999 || transparency < 0.0001 {
        destination[..3].copy_from_slice(&above);
    } else {
        let opacity = 1. - transparency;
        let below = 1. - destination[3];
        let total = opacity + below * (1. - opacity);
        if total > 0. {
            let reciprocal = 1. / total;
            for channel in 0..3 {
                destination[channel] = (above[channel] * opacity
                    + below * destination[channel] * (1. - opacity))
                    * reciprocal;
            }
        }
        transparency = 1. - total;
    }
    destination[3] = if transparency < 0.0001 {
        0.
    } else {
        transparency
    };
}

pub(super) fn sample(point: Point, size: [usize; 2], values: &[f32], periodic: bool) -> f32 {
    if let [constant] = values {
        return *constant;
    }
    let axis = |i| {
        let extent = size[i];
        if periodic {
            let value = wrapped(point[i], extent as f64);
            let first = value as usize;
            (first % extent, (first + 1) % extent, value - first as f64)
        } else {
            lower_sample(point[i], extent)
        }
    };
    let (x0, x1, x) = axis(0);
    let (y0, y1, y) = axis(1);
    interpolate(x0, x1, y0, y1, [x, y], size[0], values) as f32
}

fn wrapped(value: f64, span: f64) -> f64 {
    if value < 0. {
        value % span + span
    } else if value >= span {
        value % span
    } else {
        value
    }
}

fn lower_sample(at: f64, length: usize) -> (usize, usize, f64) {
    if at < 0. {
        (0, 0, 0.)
    } else if at >= (length - 1) as f64 {
        (length - 1, length - 1, 0.)
    } else {
        let first = at as usize;
        (first, first + 1, at - first as f64)
    }
}

fn interpolate(
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    weight: Point,
    width: usize,
    values: &[f32],
) -> f64 {
    let a = f64::from(values[y0 * width + x0]);
    let b = f64::from(values[y0 * width + x1]);
    let c = f64::from(values[y1 * width + x0]);
    let d = f64::from(values[y1 * width + x1]);
    let top = a + weight[0] * (b - a);
    let bottom = c + weight[0] * (d - c);
    top + weight[1] * (bottom - top)
}

fn footprint(start: Point, end: Point, size: [usize; 2], values: &[f32], multiply: bool) -> f32 {
    let (x0, x1, x) = lower_sample(start[0], size[0]);
    let (y0, y1, y) = lower_sample(start[1], size[1]);
    let last_x = (end[0] as usize).clamp(x1, size[0] - 1);
    let last_y = (end[1] as usize).clamp(y1, size[1] - 1);
    let mut total = 0.;
    for row in y1..=last_y {
        let wy = if row == last_y && row != y1 {
            end[1] - end[1].trunc()
        } else {
            y
        };
        for column in x1..=last_x {
            let wx = if column == last_x && column != x1 {
                end[0] - end[0].trunc()
            } else {
                x
            };
            total += interpolate(
                x0 + column - x1,
                column,
                y0 + row - y1,
                row,
                [wx, wy],
                size[0],
                values,
            );
        }
    }
    let count = ((last_x - x1 + 1) * (last_y - y1 + 1)) as f64;
    (if multiply {
        total * (1. / count)
    } else {
        total / count
    }) as f32
}
