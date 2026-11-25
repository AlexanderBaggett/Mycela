//! Contour projection and the rotated image sampler from sj.k8 and sj.k.bx.
use super::{Fill, ImageError, Region, absolute_index, contour, reserved};
use std::f64::consts::{FRAC_PI_2, PI};

const ANGLES: usize = 40;
const ANGLE_STEP: f64 = PI / ANGLES as f64;

type Point = [i32; 2];

pub(super) struct Orientation {
    angle: f64,
    extremes: [Point; 4],
}

impl Orientation {
    pub(super) fn color(&self) -> f32 {
        (self.angle / PI * 0.99 + 0.01) as f32
    }

    pub(super) fn fit(&self, fill: Fill, size: [i32; 2], random: [f32; 2]) -> Rotation {
        let center = size.map(|dimension| dimension as f32 * 0.5);
        let samples = self.extremes.map(|point| rotate(point, self.angle, center));
        let scaled = fill == Fill::Scaled;
        let centered = matches!(fill, Fill::Centered | Fill::Scrambled);
        let lower: [f32; 2] = std::array::from_fn(|axis| {
            samples
                .iter()
                .map(|p| p[axis])
                .fold(f32::INFINITY, f32::min)
        });
        let upper: [f32; 2] = std::array::from_fn(|axis| {
            samples
                .iter()
                .map(|p| p[axis])
                .fold(f32::NEG_INFINITY, f32::max)
        });
        let max_span = (upper[0] - lower[0]).max(upper[1] - lower[1]);
        let mut origin = lower;
        let mut scale = [0.; 2];
        for (axis, dimension) in size.into_iter().enumerate() {
            let mut end = upper[axis];
            let span = end - origin[axis];
            let fit = if scaled {
                max_span
            } else if centered {
                dimension as f32
            } else {
                span
            };
            if span < fit {
                let padding = (fit - span) * 0.5;
                origin[axis] -= padding;
                end += padding;
            }
            if centered {
                let fraction = origin[axis] - origin[axis].trunc();
                let shift = if fraction > 0.5 {
                    fraction - 1.
                } else if fraction < -0.5 {
                    fraction + 1.
                } else {
                    fraction
                };
                origin[axis] -= shift;
                end -= shift;
            }
            let span = end - origin[axis];
            let span = if span == 0. { 1. } else { span };
            let divisor = if centered { dimension } else { dimension - 1 };
            scale[axis] = 1. / (span / divisor as f32);
        }
        let offsets = (fill == Fill::Scrambled)
            .then(|| std::array::from_fn(|axis| (random[axis] * size[axis] as f32).trunc()));
        Rotation {
            angle: self.angle,
            center,
            origin,
            scale,
            size: size.map(|dimension| dimension as f32),
            offsets,
        }
    }
}

pub(super) fn measure(
    regions: &[Region],
    labels: &[usize],
    size: [i32; 2],
) -> Result<Vec<Orientation>, ImageError> {
    let [width, height] = size;
    let edges = contour::boundary(labels, size)?;
    let mut orientations = reserved(regions.len())?;
    let mut anchors = Vec::new();
    for (index, bounds) in regions.iter().enumerate() {
        anchors.clear();
        for y in bounds.top..=bounds.bottom {
            for x in bounds.left..=bounds.right {
                if edges[absolute_index(x, y, width, height)] == index + 1 {
                    let [px, py] = [f64::from(x), f64::from(y)];
                    let radius = (py * py + px * px).sqrt();
                    // sj reads the integer contour anchors. A zero radius
                    // gives a NaN projection and never wins either comparison.
                    if radius > 0. {
                        anchors.try_reserve(1).map_err(|_| ImageError::Allocation)?;
                        anchors.push(([x, y], radius, (py / radius).asin()));
                    }
                }
            }
        }
        orientations.push(project(&anchors));
    }
    Ok(orientations)
}

fn project(anchors: &[(Point, f64, f64)]) -> Orientation {
    let mut chosen = 0.;
    let mut narrowest = 1_000_000.;
    let mut points = [[0; 2]; 2];
    let mut projections = [[[0; 2]; 2]; ANGLES];
    let mut extremes = [[0; 2]; 2];
    for (index, endpoints) in projections.iter_mut().enumerate() {
        let angle = ANGLE_STEP * index as f64;
        let [mut low, mut high] = [1_000_000., -1_000_000.];
        for &(point, radius, polar) in anchors {
            let coordinate = (polar + angle).sin() * radius;
            if coordinate < low {
                low = coordinate;
                points[0] = point;
            }
            if coordinate > high {
                high = coordinate;
                points[1] = point;
            }
        }
        let span = high - low;
        if span < narrowest {
            chosen = angle;
            narrowest = span;
            extremes = points;
        }
        *endpoints = points;
    }
    let cross = (((chosen + FRAC_PI_2) % PI) / PI * ANGLES as f64) as usize;
    Orientation {
        angle: chosen,
        extremes: [
            extremes[0],
            extremes[1],
            projections[cross][0],
            projections[cross][1],
        ],
    }
}

pub(super) struct Rotation {
    angle: f64,
    center: [f32; 2],
    origin: [f32; 2],
    scale: [f32; 2],
    size: [f32; 2],
    offsets: Option<[f32; 2]>,
}

impl Rotation {
    pub(super) fn coordinates(&self, point: Point) -> [f32; 2] {
        let rotated = rotate(point, self.angle, self.center);
        std::array::from_fn(|axis| {
            let mut sample =
                (rotated[axis] - self.origin[axis]) * self.scale[axis] % self.size[axis];
            if sample < 0. {
                sample += self.size[axis];
            }
            if let Some(offsets) = self.offsets {
                sample = (sample + offsets[axis]) % self.size[axis];
            }
            sample
        })
    }
}

fn rotate(point: Point, angle: f64, center: [f32; 2]) -> [f32; 2] {
    let point = point.map(|v| v as f32);
    if angle == 0. {
        return point;
    }
    let x = f64::from(point[0] - center[0]);
    let y = f64::from(point[1] - center[1]);
    let radius = (x * x + y * y).sqrt();
    if radius == 0. {
        return point;
    }
    let polar = if x < 0. {
        (-y / radius).asin() - PI
    } else {
        (y / radius).asin()
    };
    let angle = angle + polar;
    [
        (angle.cos() * radius) as f32 + center[0],
        (angle.sin() * radius) as f32 + center[1],
    ]
}
