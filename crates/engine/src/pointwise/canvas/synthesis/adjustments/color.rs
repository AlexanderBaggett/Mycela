use super::{LayerColor, Master, NumberLinks, effective_float};

/// A scalar color control in Synthesis master controls or a texture layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SynthesisColorField {
    Brightness,
    Contrast,
    Hue,
    Saturation,
    Sharpen,
    SharpenRadius,
    BrightnessJitter,
    HueJitter,
}

impl SynthesisColorField {
    pub(in crate::pointwise::canvas::synthesis) const MASTER: [Self; 6] = [
        Self::Brightness,
        Self::Contrast,
        Self::Hue,
        Self::Saturation,
        Self::Sharpen,
        Self::SharpenRadius,
    ];
    pub(in crate::pointwise::canvas::synthesis) const LAYER: [Self; 6] = [
        Self::Brightness,
        Self::Contrast,
        Self::Hue,
        Self::Saturation,
        Self::BrightnessJitter,
        Self::HueJitter,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Brightness => "brightness",
            Self::Contrast => "contrast",
            Self::Hue => "hue",
            Self::Saturation => "saturation",
            Self::Sharpen => "sharpen",
            Self::SharpenRadius => "sharpen-radius",
            Self::BrightnessJitter => "brightness-jitter",
            Self::HueJitter => "hue-jitter",
        }
    }

    pub(super) const fn range(self) -> [f64; 2] {
        match self {
            Self::Brightness | Self::Contrast => [-100., 100.],
            Self::Hue => [-360., 360.],
            Self::Saturation => [-100., 1000.],
            _ => [0., 100.],
        }
    }
}

impl NumberLinks {
    fn resolve(&self, raw: [f32; 6]) -> [f32; 6] {
        std::array::from_fn(|index| effective_float(raw[index], self.get(index)))
    }
}

impl Master {
    pub(super) fn color_values(&self) -> [f32; 6] {
        [
            self.brightness,
            self.contrast,
            self.hue,
            self.saturation,
            self.sharpen,
            self.sharpen_radius,
        ]
    }

    pub(super) fn color_value_mut(&mut self, index: usize) -> &mut f32 {
        [
            &mut self.brightness,
            &mut self.contrast,
            &mut self.hue,
            &mut self.saturation,
            &mut self.sharpen,
            &mut self.sharpen_radius,
        ]
        .into_iter()
        .nth(index)
        .expect("master color field index")
    }

    pub(in crate::pointwise::canvas::synthesis) fn effective_color_values(&self) -> [f32; 6] {
        self.color_links.resolve(self.color_values())
    }
}

impl LayerColor {
    pub(in crate::pointwise::canvas::synthesis) fn effective_color_values(&self) -> [f32; 6] {
        self.number_links.resolve(self.adjustments)
    }
}
