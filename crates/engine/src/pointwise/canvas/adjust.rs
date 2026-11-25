use super::style::ColorAdjustment;
use crate::{HslColor, RgbColor};

pub(super) struct AdjustRgb {
    tint: [f32; 3],
    hue: f32,
    saturation: f64,
    brightness: f32,
    contrast: f32,
}

impl From<&ColorAdjustment> for AdjustRgb {
    fn from(settings: &ColorAdjustment) -> Self {
        let [_, r, g, b] = settings.argb;
        Self {
            tint: [r as f32, g as f32, b as f32],
            hue: settings.hue as f32 / 360.,
            saturation: settings.saturation,
            brightness: (settings.brightness as f32 / 100.).clamp(-1., 1.),
            contrast: settings.contrast as f32 / 100.,
        }
    }
}

impl AdjustRgb {
    pub(super) fn apply(&self, mut rgb: [f32; 3]) -> [f32; 3] {
        for (channel, tint) in rgb.iter_mut().zip(self.tint) {
            *channel *= tint;
        }
        if self.hue != 0. || self.saturation != 0. {
            let [r, g, b] = rgb.map(f64::from);
            let [hue, saturation, lightness] = RgbColor::new(r, g, b, 1.).to_hsl().components();
            let mut hue = (hue + f64::from(self.hue)) as f32;
            if hue.is_finite() {
                hue %= 1.;
                if hue < 0. {
                    hue += 1.;
                }
            }
            let [_, r, g, b] = HslColor::new(f64::from(hue), saturation, lightness)
                .to_rgb()
                .argb();
            rgb = [r as f32, g as f32, b as f32];
            if self.saturation != 0. {
                let [r, g, b] = rgb.map(f64::from);
                let [hue, _, lightness] = RgbColor::new(r, g, b, 1.).to_hsl().components();
                let target = f64::from(self.saturation > 0.);
                let [_, r, g, b] = HslColor::new(hue, target, lightness).to_rgb().argb();
                let weight = (100. - self.saturation.abs() as f32) / 100.;
                for (value, target) in rgb.iter_mut().zip([r, g, b]) {
                    *value = (f64::from(weight * *value) + f64::from(1. - weight) * target) as f32;
                }
            }
        }
        if self.brightness > 0. {
            rgb = rgb.map(|value| (1. - self.brightness) * value + self.brightness);
        } else if self.brightness != 0. {
            rgb = rgb.map(|value| (1. + self.brightness) * value);
        }
        if self.contrast > 0. {
            let half = self.contrast / 2.;
            let gain = 1. / (1. - 2. * half);
            rgb = rgb.map(|value| ((value - half) * gain).clamp(0., 1.));
        } else if self.contrast != 0. {
            rgb = rgb.map(|value| (value + (value - 0.5) * self.contrast).clamp(0., 1.));
        }
        rgb
    }
}
