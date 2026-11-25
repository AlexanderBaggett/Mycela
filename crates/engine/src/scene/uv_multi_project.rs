//! Face bases and normal-weighted sampling for UV Multi Project.
use std::f64::consts::{PI, TAU};

use super::uv_project::{dot, normalize};
use crate::ImageError;

struct Face {
    u: [f32; 3],
    v: [f32; 3],
    normal: [f32; 3],
}

impl Face {
    fn new(u: [f32; 3], v: [f32; 3], normal: [f32; 3]) -> Self {
        Self {
            u: normalize(u),
            v: normalize(v),
            normal,
        }
    }

    fn tilted(pitch: f64, heading: f64, reverse_uv: bool) -> Self {
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let (sin_heading, cos_heading) = heading.sin_cos();
        let sign = if reverse_uv { -1. } else { 1. };
        let single = |axis: [f64; 3]| axis.map(|value| value as f32);
        Self::new(
            single([sign * cos_heading, 0., -sign * sin_heading]),
            single([
                sign * sin_heading * cos_pitch,
                -sign * sin_pitch,
                sign * cos_heading * cos_pitch,
            ]),
            single([
                -sin_heading * sin_pitch,
                -cos_pitch,
                -cos_heading * sin_pitch,
            ]),
        )
    }
}

pub(crate) struct MultiProjection {
    faces: Vec<Face>,
    feather: f32,
}

impl MultiProjection {
    pub(crate) fn new(sides: i32, feather: f32) -> Option<Self> {
        let half_root_three = 3_f32.sqrt() * 0.5;
        let diagonal = 0.5_f32.sqrt();
        let third = (1_f32 / 3.).sqrt();
        let faces = match sides {
            0 => {
                let a = half_root_three;
                let b = a * 0.5;
                vec![
                    Face::new([1., 0., 0.], [0., 0., 1.], [0., -1., 0.]),
                    Face::new([1., 0., 0.], [0., -a, -0.5], [0., 0.5, -a]),
                    Face::new([-0.5, 0., a], [b, -a, 0.25], [0.75, 0.5, b]),
                    Face::new([-0.5, 0., -a], [-b, -a, 0.25], [-0.75, 0.5, b]),
                ]
            }
            1 => vec![
                Face::new([-1., 0., 0.], [0., -1., 0.], [0., 0., 1.]),
                Face::new([1., 0., 0.], [0., -1., 0.], [0., 0., -1.]),
                Face::new([1., 0., 0.], [0., 0., -1.], [0., 1., 0.]),
                Face::new([1., 0., 0.], [0., 0., 1.], [0., -1., 0.]),
                Face::new([0., 0., 1.], [0., -1., 0.], [1., 0., 0.]),
                Face::new([0., 0., -1.], [0., -1., 0.], [-1., 0., 0.]),
            ],
            2 => {
                let a = diagonal;
                let b = third;
                vec![
                    Face::new([-a, 0., a], [0.5, -a, 0.5], [b, b, b]),
                    Face::new([-a, 0., -a], [-0.5, -a, 0.5], [-b, b, b]),
                    Face::new([a, 0., -a], [-0.5, -a, -0.5], [-b, b, -b]),
                    Face::new([a, 0., a], [0.5, -a, -0.5], [b, b, -b]),
                    Face::new([-a, 0., a], [-0.5, -a, -0.5], [b, -b, b]),
                    Face::new([-a, 0., -a], [0.5, -a, -0.5], [-b, -b, b]),
                    Face::new([a, 0., -a], [0.5, -a, 0.5], [-b, -b, -b]),
                    Face::new([a, 0., a], [-0.5, -a, 0.5], [b, -b, -b]),
                ]
            }
            3 => {
                let angle = PI * 0.3;
                let tangent = angle.tan();
                let pitch = PI * tangent / (2. + tangent + angle.sin());
                let mut faces = Vec::with_capacity(12);
                faces.push(Face::new([1., 0., 0.], [0., 0., 1.], [0., -1., 0.]));
                faces.extend(ring(pitch, 0., false));
                faces.push(Face::new([1., 0., 0.], [0., 0., -1.], [0., 1., 0.]));
                faces.extend(ring(pitch - PI, 0., true));
                faces
            }
            4 => {
                let root_five = 5_f64.sqrt();
                let z_outer = ((5. + 2. * root_five) / 20.).sqrt();
                let z_inner = ((5. + root_five) / 10.).sqrt() - z_outer;
                let scale = 2. / 3_f64.sqrt();
                let outer = (z_outer * scale).acos();
                let inner = (z_inner * scale).acos();
                ring(outer, 0., false)
                    .chain(ring(inner, 0., false))
                    .chain(ring(PI - inner, PI / 5., false))
                    .chain(ring(PI - outer, PI / 5., false))
                    .collect()
            }
            _ => return None,
        };
        Some(Self { faces, feather })
    }

    pub(super) fn sample(
        &self,
        position: [f32; 3],
        normal: [f32; 3],
        tile: bool,
        background: [f32; 4],
        sample: impl Fn([f64; 2]) -> Result<[f32; 4], ImageError>,
    ) -> Result<[f32; 4], ImageError> {
        let mut factors = [0.; 20];
        let factors = &mut factors[..self.faces.len()];
        let mut largest = 0.;
        let mut strongest = None;
        for (i, (factor, face)) in factors.iter_mut().zip(&self.faces).enumerate() {
            let angle = dot(face.normal, normal).acos();
            *factor = 1. - angle * std::f32::consts::FRAC_1_PI;
            if *factor > largest {
                largest = *factor;
                strongest = Some(i);
            }
        }
        let threshold = (1. - self.feather) * largest;
        let mut total = 0.;
        for (i, factor) in factors.iter_mut().enumerate() {
            if Some(i) != strongest {
                *factor = if *factor > 0. && *factor > threshold {
                    (*factor - threshold) / self.feather
                } else {
                    0.
                };
            }
            *factor *= *factor;
            total += *factor;
        }
        if total <= 0. {
            return Ok(background);
        }
        let mut foreground = [0.; 4];
        for (&factor, face) in factors.iter().zip(&self.faces) {
            if factor <= 0. {
                continue;
            }
            let uv = [dot(face.u, position) + 0.5, dot(face.v, position) + 0.5];
            if uv
                .iter()
                .any(|v| !v.is_finite() || (!tile && !(0. ..=1.).contains(v)))
            {
                continue;
            }
            let uv = if tile {
                uv.map(|value| value - value.floor())
            } else {
                uv
            };
            let weight = factor / total;
            for (target, value) in foreground.iter_mut().zip(sample(uv.map(f64::from))?) {
                *target += value * weight;
            }
        }
        let front_alpha = foreground[3];
        let background_weight = background[3] * (1. - front_alpha);
        let alpha = front_alpha + background_weight;
        if alpha > 0. {
            // The original shader divides RGB by the reciprocal of alpha.
            let inverse = 1. / alpha;
            for (front, back) in foreground.iter_mut().zip(background).take(3) {
                *front = (*front * front_alpha + back * background_weight) / inverse;
            }
            foreground[3] = alpha;
        }
        Ok(foreground)
    }
}

fn ring(pitch: f64, start: f64, reverse_uv: bool) -> impl Iterator<Item = Face> {
    (0..5).map(move |index| Face::tilted(pitch, start + f64::from(index) * TAU / 5., reverse_uv))
}
