//! The synthesis mesh samples the original image. Placement samples the warped
//! tile and its feather separately before blending the repeated copies.
use super::LayerColor;
use crate::{HslColor, ImageError, PointwiseError, RgbColor, seeded_random::SeededRandom};

pub(super) struct Cell {
    pub(super) origin: [usize; 2],
    pub(super) size: [usize; 2],
    pub(super) corners: [[f32; 2]; 4],
}

pub(super) fn warp<E>(
    source: &[Vec<f32>; 4],
    source_size: [usize; 2],
    target: &mut [[f32; 4]],
    size: [usize; 2],
    cell: Cell,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    let [mut left, mut right, bottom_left, bottom_right] = cell.corners;
    let slope = |top: [f32; 2], bottom: [f32; 2]| {
        std::array::from_fn::<_, 2, _>(|axis| (bottom[axis] - top[axis]) / cell.size[1] as f32)
    };
    let (left_step, right_step) = (slope(left, bottom_left), slope(right, bottom_right));
    for y in cell.origin[1]..(cell.origin[1] + cell.size[1]).min(size[1]) {
        crate::pointwise::check_render_cancelled(cancelled)?;
        let step: [f32; 2] = std::array::from_fn(|axis| {
            (right[axis] - left[axis]) * source_size[axis] as f32 / cell.size[0] as f32
        });
        let mut point: [f32; 2] = std::array::from_fn(|axis| left[axis] * source_size[axis] as f32);
        for x in cell.origin[0]..(cell.origin[0] + cell.size[0]).min(size[0]) {
            let sample = Bilinear::new(point, source_size, true);
            let alpha = sample.read(|index| source[3][index]);
            if f64::from(alpha) < 0.99999 {
                target[y * size[0] + x] = std::array::from_fn(|channel| {
                    if channel == 3 {
                        alpha
                    } else {
                        sample.read(|index| source[channel][index])
                    }
                });
            }
            point[0] += step[0];
            point[1] += step[1];
        }
        for axis in 0..2 {
            left[axis] += left_step[axis];
            right[axis] += right_step[axis];
        }
    }
    Ok(())
}

pub(super) struct Tile<'a> {
    pub(super) pixels: &'a [[f32; 4]],
    pub(super) mask: Option<&'a [f32]>,
    pub(super) size: [usize; 2],
}

pub(super) struct Target<'a> {
    pub(super) pixels: &'a mut [[f32; 4]],
    pub(super) weights: &'a mut [f32],
    pub(super) size: [usize; 2],
}

impl Tile<'_> {
    pub(super) fn place<E>(
        &self,
        target: &mut Target<'_>,
        origin: [f32; 2],
        span: [f32; 2],
        color: &LayerColor,
        random: f64,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), PointwiseError<E>> {
        let [x, y] = std::array::from_fn(|axis| {
            StampAxis::new(origin[axis], span[axis], self.size[axis], target.size[axis])
        });
        let magnifies = x.step < 1. && y.step < 1.;
        let color = TileColor::new(color, Some(random))?;
        for row in y.samples() {
            crate::pointwise::check_render_cancelled(cancelled)?;
            for column in x.samples() {
                let mut coverage = column.coverage * row.coverage;
                let sample = if magnifies {
                    Sample::Point(Bilinear::new(
                        [
                            (column.lower + column.upper) / 2.,
                            (row.lower + row.upper) / 2.,
                        ],
                        self.size,
                        false,
                    ))
                } else {
                    Sample::Region(Region {
                        x: Range::new(column.lower, column.upper),
                        y: Range::new(row.lower, row.upper),
                        width: self.size[0],
                    })
                };
                let alpha = sample.read(|index| self.pixels[index][3]);
                if alpha >= 0.9999 {
                    continue;
                }
                if let Some(mask) = self.mask {
                    coverage *= sample.read(|index| mask[index]);
                }
                let rgb = color.apply(std::array::from_fn(|channel| {
                    sample.read(|index| self.pixels[index][channel])
                }));
                let index = row.index * target.size[0] + column.index;
                blend(
                    &mut target.pixels[index],
                    &mut target.weights[index],
                    rgb,
                    alpha,
                    coverage,
                    self.mask.is_some(),
                );
            }
        }
        Ok(())
    }
}

fn blend(
    destination: &mut [f32; 4],
    accumulated: &mut f32,
    rgb: [f32; 3],
    transparency: f32,
    coverage: f32,
    feathered: bool,
) {
    if destination[3] > 0.9999 || (transparency < 0.0001 && coverage > 0.9999) {
        destination[..3].copy_from_slice(&rgb);
        destination[3] = if transparency < 0.0001 {
            0.
        } else {
            transparency
        };
        *accumulated = coverage;
        return;
    }
    let above = 1. - transparency;
    let below = 1. - destination[3];
    let remaining = (*accumulated).min(1. - coverage);
    let total = coverage + remaining;
    if total <= 0. {
        return;
    }
    let reciprocal = 1. / total;
    destination[3] = 1. - (above * coverage + below * remaining) * reciprocal;
    if destination[3] < 0.0001 {
        destination[3] = 0.;
    }
    *accumulated = total;
    if feathered {
        let above = above * coverage;
        let below = below * remaining;
        let total = above + below * (1. - above);
        if total > 0. {
            let reciprocal = 1. / total;
            for (out, top) in destination.iter_mut().zip(rgb) {
                *out = (top * above + below * *out * (1. - above)) * reciprocal;
            }
        }
    } else if coverage != remaining {
        for (out, top) in destination.iter_mut().zip(rgb) {
            *out = (top * coverage + *out * remaining) * reciprocal;
        }
    }
}

struct StampAxis {
    origin: f32,
    span: f32,
    fraction: f32,
    step: f32,
    source_size: f32,
    target_size: usize,
    count: usize,
}

struct AxisSample {
    index: usize,
    lower: f32,
    upper: f32,
    coverage: f32,
}

impl StampAxis {
    fn new(mut origin: f32, span: f32, source: usize, target: usize) -> Self {
        if origin < 0. {
            origin += target as f32;
        }
        let fraction = origin - origin.trunc();
        let count =
            ((f64::from(span + fraction) * 10000.).round_ties_even() / 10000.).ceil() as usize;
        Self {
            origin,
            span,
            fraction,
            step: source as f32 / span,
            source_size: source as f32,
            target_size: target,
            count,
        }
    }

    fn samples(&self) -> impl Iterator<Item = AxisSample> + '_ {
        let limit = |value| {
            if value >= self.source_size {
                self.source_size - 0.001
            } else {
                value
            }
        };
        let mut lower = 0.;
        let first = if self.fraction > 0. {
            (1. - self.fraction) * self.source_size / self.span
        } else {
            self.step
        };
        let mut upper = limit(first);
        (0..self.count).map_while(move |local| {
            if upper < lower {
                return None;
            }
            let coverage = if local == 0 {
                if self.fraction == 0.5 {
                    0.5001
                } else {
                    1. - self.fraction
                }
            } else if local == self.count - 1 {
                let end = self.origin + self.span;
                let weight = end - end.trunc();
                if weight < 0.0001 {
                    1.
                } else if weight == 0.5 {
                    0.4999
                } else {
                    weight
                }
            } else {
                1.
            };
            let index =
                (self.origin as i64 + local as i64).rem_euclid(self.target_size as i64) as usize;
            let sample = AxisSample {
                index,
                lower,
                upper,
                coverage,
            };
            lower = upper;
            upper = limit(upper + self.step);
            Some(sample)
        })
    }
}

enum Sample {
    Point(Bilinear),
    Region(Region),
}

impl Sample {
    fn read(&self, values: impl Fn(usize) -> f32) -> f32 {
        match self {
            Self::Point(point) => point.read(values),
            Self::Region(region) => region.read(values),
        }
    }
}

pub(super) struct Bilinear {
    corners: [usize; 4],
    fraction: [f32; 2],
}

impl Bilinear {
    pub(super) fn new(point: [f32; 2], size: [usize; 2], periodic: bool) -> Self {
        let axis = |mut value: f32, size: usize| {
            let extent = size as f32;
            if periodic {
                if value < 0. {
                    value = value % extent + extent;
                } else if value >= extent {
                    value %= extent;
                }
                let lo = value as usize;
                (lo % size, (lo + 1) % size, value - lo as f32)
            } else {
                let value = value.clamp(0., extent - 1.);
                let lo = value as usize;
                (lo, (lo + 1).min(size - 1), value - lo as f32)
            }
        };
        let (x0, x1, fx) = axis(point[0], size[0]);
        let (y0, y1, fy) = axis(point[1], size[1]);
        Self {
            corners: [
                x0 + size[0] * y0,
                x1 + size[0] * y0,
                x0 + size[0] * y1,
                x1 + size[0] * y1,
            ],
            fraction: [fx, fy],
        }
    }

    pub(super) fn read(&self, values: impl Fn(usize) -> f32) -> f32 {
        let [a, b, c, d] = self.corners.map(values);
        let [x, y] = self.fraction;
        let top = a + x * (b - a);
        let bottom = c + x * (d - c);
        top + y * (bottom - top)
    }
}

struct Range {
    first: usize,
    last: usize,
    enter: f32,
    exit: f32,
    length: f32,
}

impl Range {
    fn new(start: f32, end: f32) -> Self {
        let (first, last) = (start as usize, end as usize);
        let [enter, exit, length] = if first == last {
            [1., 0., 1.]
        } else {
            [1. - (start - first as f32), end - last as f32, end - start]
        };
        Self {
            first,
            last,
            enter,
            exit,
            length,
        }
    }

    fn interior(&self) -> std::ops::Range<usize> {
        self.first + 1..self.last
    }
}

struct Region {
    x: Range,
    y: Range,
    width: usize,
}

impl Region {
    fn read(&self, values: impl Fn(usize) -> f32) -> f32 {
        let Self { x, y, width } = self;
        let at = |column, row| values(column + row * width);
        let area = x.length * y.length;
        let mut total = at(x.first, y.first) * x.enter * y.enter
            + at(x.last, y.first) * x.exit * y.enter
            + at(x.first, y.last) * x.enter * y.exit
            + at(x.last, y.last) * x.exit * y.exit;
        for column in x.interior() {
            total += at(column, y.first) * y.enter + at(column, y.last) * y.exit;
        }
        for row in y.interior() {
            total += at(x.first, row) * x.enter + at(x.last, row) * x.exit;
        }
        for row in y.interior() {
            for column in x.interior() {
                total += at(column, row);
            }
        }
        if area > 0. { total / area } else { 0. }
    }
}

pub(super) struct TileColor {
    tint: [f32; 3],
    brightness: f32,
    hue: f32,
    saturation: f32,
    adjust_hsl: bool,
    contrast: f32,
}

impl TileColor {
    pub(super) fn new(color: &LayerColor, sample: Option<f64>) -> Result<Self, ImageError> {
        let [r, g, b] = color.effective_tint();
        let [
            brightness,
            contrast,
            hue,
            saturation,
            brightness_jitter,
            hue_jitter,
        ] = color.effective_color_values();
        let mut brightness = brightness / 100.;
        let saturation = saturation / 100. + 1.;
        let adjust_hsl = hue != 0. || saturation != 1. || hue_jitter != 0.;
        let mut hue = hue / 360.;
        if let Some(sample) = sample
            && (brightness_jitter != 0. || hue_jitter != 0.)
        {
            let mut random = SeededRandom::from_seed((sample * 1000.) as i32)
                .map_err(|_| ImageError::Overflow)?;
            hue += ((random.sample() * 2. - 1.) * f64::from(hue_jitter) / 100.) as f32;
            brightness +=
                ((random.sample() * 2. - 1.) * f64::from(brightness_jitter) / 100.) as f32;
        }
        Ok(Self {
            tint: [r as f32, g as f32, b as f32],
            brightness: brightness.clamp(-1., 1.),
            hue,
            saturation,
            adjust_hsl,
            contrast: contrast / 100.,
        })
    }

    pub(super) fn apply(&self, mut color: [f32; 3]) -> [f32; 3] {
        for (value, tint) in color.iter_mut().zip(self.tint) {
            *value *= tint;
        }
        if self.adjust_hsl {
            let [r, g, b] = color.map(f64::from);
            let [hue, saturation, lightness] = RgbColor::new(r, g, b, 1.).to_hsl().components();
            let wrapped = ((hue + f64::from(self.hue)) as f32).rem_euclid(1.);
            let hue = if wrapped < 1. { wrapped } else { 0. };
            let [_, r, g, b] = HslColor::new(
                f64::from(hue),
                saturation * f64::from(self.saturation),
                lightness,
            )
            .to_rgb()
            .argb();
            color = [r as f32, g as f32, b as f32];
        }
        if self.brightness > 0. {
            color = color.map(|value| (1. - self.brightness) * value + self.brightness);
        } else if self.brightness != 0. {
            color = color.map(|value| (1. + self.brightness) * value);
        }
        if self.contrast > 0. {
            let half = self.contrast / 2.;
            let gain = 1. / (1. - 2. * half);
            color = color.map(|value| ((value - half) * gain).clamp(0., 1.));
        } else if self.contrast != 0. {
            color = color.map(|value| (value + (value - 0.5) * self.contrast).clamp(0., 1.));
        }
        color
    }
}
