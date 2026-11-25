//! Environment blend (ahg, adi), with the fu.j order and hv's channel formulas.
use super::EnvironmentLayer;
use crate::{HslColor, ModernPropertyRecord, ModernPropertyValue, RgbColor};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentBlendMode {
    Normal,
    Add,
    Subtractive,
    Multiply,
    Darken,
    ColorBurn,
    Lighten,
    Screen,
    ColorDodge,
    Overlay,
    SoftLight,
    HardLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl EnvironmentBlendMode {
    pub const ALL: [Self; 18] = [
        Self::Normal,
        Self::Add,
        Self::Subtractive,
        Self::Multiply,
        Self::Darken,
        Self::ColorBurn,
        Self::Lighten,
        Self::Screen,
        Self::ColorDodge,
        Self::Overlay,
        Self::SoftLight,
        Self::HardLight,
        Self::Difference,
        Self::Exclusion,
        Self::Hue,
        Self::Saturation,
        Self::Color,
        Self::Luminosity,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Add => "Add",
            Self::Subtractive => "Subtractive",
            Self::Multiply => "Multiply",
            Self::Darken => "Darken",
            Self::ColorBurn => "Color Burn",
            Self::Lighten => "Lighten",
            Self::Screen => "Screen",
            Self::ColorDodge => "Color Dodge",
            Self::Overlay => "Overlay",
            Self::SoftLight => "Soft Light",
            Self::HardLight => "Hard Light",
            Self::Difference => "Difference",
            Self::Exclusion => "Exclusion",
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Color => "Color",
            Self::Luminosity => "Luminosity",
        }
    }

    pub(super) fn read(property: &ModernPropertyRecord) -> Option<Self> {
        let ModernPropertyValue::Selected { current, .. } = property.value() else {
            return None;
        };
        Self::ALL.get(*current as usize).copied()
    }

    pub(crate) fn composite(self, mut top: [f32; 3], bottom: &mut [f32; 4], opacity: f32) {
        let bottom_transparency = bottom[3];
        let bottom_opacity = 1. - bottom_transparency;
        let top_weight = if opacity == 0. && bottom_opacity == 0. {
            1.
        } else {
            opacity
        };
        let bottom_weight = bottom_opacity * (1. - top_weight);
        let total = top_weight + bottom_weight;
        let normalization = if total == 0. { 0. } else { 1. / total };
        let [r, g, b, _] = *bottom;
        let mut below = [r, g, b];

        if self != Self::Normal {
            let blend = match self {
                Self::Hue | Self::Saturation | Self::Color | Self::Luminosity => {
                    self.hsl_blend(top, below)
                }
                _ => std::array::from_fn(|i| self.channel(&mut top[i], &mut below[i])),
            };
            top = std::array::from_fn(|i| top[i] + bottom_opacity * (blend[i] - top[i]));
        }

        for ((out, above), below) in bottom[..3].iter_mut().zip(top).zip(below) {
            *out = (above * top_weight + below * bottom_weight) * normalization;
        }
        bottom[3] = ((1. - opacity) * bottom_transparency).clamp(0., 1.);
    }

    fn channel(self, top: &mut f32, bottom: &mut f32) -> f32 {
        let (t, b) = (*top, *bottom);
        let multiply = || 2. * (t * b);
        let screen = || 1. - 2. * (1. - t) * (1. - b);
        match self {
            Self::Add => t + b,
            Self::Subtractive => t + b - 1.,
            Self::Multiply => t * b,
            Self::Darken => {
                if t < b {
                    t
                } else {
                    b
                }
            }
            Self::ColorBurn => {
                if b < 0.001 {
                    *bottom = 0.001;
                }
                let value = 1. - (1. - t) / *bottom;
                if value < 0. { 0. } else { value }
            }
            Self::Lighten => {
                if t > b {
                    t
                } else {
                    b
                }
            }
            Self::Screen => 1. - (1. - b) * (1. - t),
            Self::ColorDodge => {
                if t > 0.99999 {
                    *top = 0.99999;
                }
                (b / (1. - *top)).clamp(0., 1.)
            }
            Self::Overlay => {
                if t < 0.5 {
                    multiply()
                } else {
                    screen()
                }
            }
            Self::SoftLight => {
                let softened = 0.25 + t * 0.5;
                if b < 0.5 {
                    2. * softened * b
                } else {
                    1. - 2. * (1. - softened) * (1. - b)
                }
            }
            Self::HardLight => {
                if b > 0.5 {
                    multiply()
                } else {
                    screen()
                }
            }
            Self::Difference => {
                let value = t - b;
                if value < 0. { -value } else { value }
            }
            Self::Exclusion => t + b - 2. * t * b,
            Self::Normal | Self::Hue | Self::Saturation | Self::Color | Self::Luminosity => {
                unreachable!("non-channel blend")
            }
        }
    }

    fn hsl_blend(self, top: [f32; 3], bottom: [f32; 3]) -> [f32; 3] {
        let to_hsl = |rgb: [f32; 3]| {
            RgbColor::new(f64::from(rgb[0]), f64::from(rgb[1]), f64::from(rgb[2]), 1.)
                .to_hsl()
                .components()
        };
        let [th, ts, tl] = to_hsl(top);
        let [bh, bs, bl] = to_hsl(bottom);
        let hsl = match self {
            Self::Hue => HslColor::new(th, bs, bl),
            Self::Saturation => HslColor::new(bh, ts, bl),
            Self::Color => HslColor::new(th, ts, bl),
            Self::Luminosity => HslColor::new(bh, bs, tl),
            _ => unreachable!("HSL blend"),
        };
        let [_, r, g, b] = hsl.to_rgb().argb();
        [r as f32, g as f32, b as f32]
    }
}

pub(super) fn number(property: &ModernPropertyRecord) -> f64 {
    property.double_pair().expect("Environment double").0
}

pub(super) fn light_axis(rotation: f64, elevation: f64) -> [f64; 3] {
    let rotation = rotation / 180. * PI;
    let elevation = elevation / 180. * PI;
    [
        rotation.cos() * elevation.cos(),
        rotation.sin() * elevation.cos(),
        elevation.sin(),
    ]
}

pub(super) fn soft_light([x, y, z]: [f64; 3], axis: [f64; 3]) -> f32 {
    0.5 * (x * axis[0] + y * axis[1] + z * axis[2]) as f32 + 0.5
}

pub(super) enum Contrast {
    Identity,
    Increase { offset: f32, gain: f32 },
    Decrease(f32),
}

impl Contrast {
    pub(super) fn new(value: f64) -> Self {
        let amount = value as f32 / 100.;
        if value > 0. {
            let offset = amount / 2.;
            let gain = 1. / (1. - 2. * offset).max(0.0001);
            Self::Increase { offset, gain }
        } else if value < 0. {
            Self::Decrease(amount)
        } else {
            Self::Identity
        }
    }

    pub(super) fn apply(&self, value: f32) -> f32 {
        match *self {
            Self::Identity => value,
            Self::Increase { offset, gain } => (value - offset) * gain,
            Self::Decrease(amount) => value + (value - 0.5) * amount,
        }
    }
}

pub(super) struct Mixer {
    axis: [f64; 3],
    level: f32,
    amount: f32,
    contrast: Contrast,
}

impl Mixer {
    pub(super) fn prepare(layer: &EnvironmentLayer) -> Option<Self> {
        layer.mixer_enabled().then(|| {
            let blend = &layer.blend;
            Self {
                axis: light_axis(number(&blend[7]), number(&blend[8])),
                level: number(&blend[6]) as f32 / 100.,
                amount: number(&blend[4]) as f32 / 100.,
                contrast: Contrast::new(number(&blend[5])),
            }
        })
    }

    pub(super) fn transparency(&self, input: f32, direction: [f64; 3]) -> f32 {
        let mut level = soft_light(direction, self.axis) + self.level;
        if level > 1. {
            level = 2. - level;
        }
        if self.amount != 0. || !matches!(self.contrast, Contrast::Identity) {
            level = self.contrast.apply(level + self.amount).clamp(0., 1.);
        }
        1. - (1. - input) * level
    }
}

#[cfg(test)]
mod tests {
    use super::EnvironmentBlendMode as Mode;

    #[test]
    fn all_modes_use_the_registered_color_formula_in_wire_order() {
        let expected = [
            0.75, 1., 0., 0.1875, 0.25, 0., 0.75, 0.8125, 1., 0.625, 0.3125, 0.625, 0.5, 0.625,
            0.25, 0.25, 0.25, 0.75,
        ];
        for (mode, expected) in Mode::ALL.into_iter().zip(expected) {
            let mut bottom = [0.25, 0.25, 0.25, 0.];
            mode.composite([0.75; 3], &mut bottom, 1.);
            assert_eq!(bottom, [expected, expected, expected, 0.], "{mode:?}");
        }
        let mut bottom = [0.25, 0.25, 0.25, 0.];
        Mode::HardLight.composite([0.25; 3], &mut bottom, 1.);
        assert_eq!(bottom, [-0.125, -0.125, -0.125, 0.]);
        let mut bottom = [1., 0.5, 0.5, 0.];
        Mode::Hue.composite([0., 0., 1.], &mut bottom, 1.);
        assert_eq!(bottom, [0.5, 0.5, 1., 0.]);
    }

    #[test]
    fn source_over_uses_the_unblended_top_where_the_bottom_is_transparent() {
        let mut bottom = [0.25, 0.25, 0.25, 0.5];
        Mode::Multiply.composite([0.75; 3], &mut bottom, 0.5);
        assert_eq!(bottom, [0.39583334, 0.39583334, 0.39583334, 0.25]);
        let mut empty = [0.; 4];
        empty[3] = 1.;
        Mode::Multiply.composite([0.75; 3], &mut empty, 1.);
        assert_eq!(empty, [0.75, 0.75, 0.75, 0.]);
    }
}
