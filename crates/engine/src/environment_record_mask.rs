//! Genetica's hemisphere mask (apj), applied to the generated map before kh.
use super::Mask;
use crate::pointwise::{
    PointwiseError,
    environment_blur::{EnvironmentBlur, blur_map},
};
use crate::{FloatBuffer, GradientError, ImageError, ImageMap, SharedGradient, SharedImageMap};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EnvironmentMaskHemisphere {
    #[default]
    Off,
    Upper,
    Lower,
}

impl EnvironmentMaskHemisphere {
    pub const ALL: [Self; 3] = [Self::Off, Self::Upper, Self::Lower];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Upper => "Upper hemisphere",
            Self::Lower => "Lower hemisphere",
        }
    }

    pub(super) fn from_wire(value: i32) -> Self {
        Self::ALL.get(value as usize).copied().unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EnvironmentMaskFill {
    #[default]
    KeepSource,
    Mirror,
    SolidColor,
    Gradient,
    Transparent,
}

impl EnvironmentMaskFill {
    pub const ALL: [Self; 5] = [
        Self::KeepSource,
        Self::Mirror,
        Self::SolidColor,
        Self::Gradient,
        Self::Transparent,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::KeepSource => "Keep source",
            Self::Mirror => "Mirror",
            Self::SolidColor => "Solid color",
            Self::Gradient => "Gradient",
            Self::Transparent => "Transparent",
        }
    }

    pub(super) fn from_wire(value: i32) -> Self {
        Self::ALL.get(value as usize).copied().unwrap_or_default()
    }
}

// apj walks flat arrays from this boundary. On an odd-height map, a span
// can begin inside a row. Clip the terminal span to the actual array.
struct Boundary {
    width: usize,
    area: usize,
    start: usize,
    upper: bool,
}

impl Boundary {
    fn pixels(&self, skips_final_sample: bool) -> Range<usize> {
        if self.start >= self.area || (skips_final_sample && self.start == self.area - 1) {
            return 0..0;
        }
        if self.upper {
            0..self.start + 1
        } else {
            self.start..self.area - usize::from(skips_final_sample)
        }
    }

    fn spans(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let step = self.width as isize * if self.upper { -1 } else { 1 };
        (0..).map_while(move |offset| {
            let row = self.start.checked_add_signed(step * offset)?;
            (row < self.area).then_some((row, offset as usize))
        })
    }

    fn mirror(&self, plane: &mut [f32]) {
        let step = self.width as isize * if self.upper { 1 } else { -1 };
        for (destination, offset) in self.spans() {
            let Some(source) = self.start.checked_add_signed(step * offset as isize) else {
                break;
            };
            if source >= self.area {
                break;
            }
            let count = self
                .width
                .min(self.area - source)
                .min(self.area - destination);
            plane.copy_within(source..source + count, destination);
        }
    }
}

impl Mask {
    pub(super) fn active(&self) -> bool {
        self.hemisphere != EnvironmentMaskHemisphere::Off
            && (self.fill != EnvironmentMaskFill::KeepSource
                || self.amount != 0.
                || self.colors[0][1..] != [1.; 3])
    }

    pub(super) fn apply<E>(
        &self,
        image: SharedImageMap,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let geometry = image.geometry();
        let width = geometry.width() as usize;
        let height = geometry.height() as usize;
        let area = usize::try_from(geometry.area()).map_err(|_| ImageError::Overflow)?;
        let upper = self.hemisphere == EnvironmentMaskHemisphere::Upper;
        let boundary = Boundary {
            width,
            area,
            start: area / 2 + if height & 1 != 0 || upper { width } else { 0 },
            upper,
        };
        let transparency = image.has_transparency()?;
        let gradient = if self.fill == EnvironmentMaskFill::Gradient {
            let gradient = SharedGradient::from_wire_record(self.gradient.clone());
            gradient.rebuild()?;
            Some((
                gradient.cache().ok_or(GradientError::NullReference)?,
                gradient.has_transparency()?,
            ))
        } else {
            None
        };
        let modifies_alpha = transparency
            || self.fill == EnvironmentMaskFill::Transparent
            || gradient
                .as_ref()
                .is_some_and(|(_, changes_alpha)| *changes_alpha);
        let mut composed = ImageMap::new(geometry);
        for channel in 0..4 {
            if cancelled() {
                return Err(PointwiseError::Cancelled);
            }
            let original = image.channel(channel, true)?;
            if channel == 3 && !modifies_alpha {
                composed.set_buffer(channel, &original)?;
                continue;
            }
            let mut values = original.values();
            if values.len() == 1 {
                values
                    .try_reserve_exact(area - 1)
                    .map_err(|_| ImageError::Allocation)?;
                values.resize(area, values[0]);
            }
            match self.fill {
                EnvironmentMaskFill::Mirror => boundary.mirror(&mut values),
                EnvironmentMaskFill::SolidColor if channel < 3 => {
                    let color = self.colors[1][channel as usize + 1] as f32;
                    values[boundary.pixels(true)].fill(color);
                }
                EnvironmentMaskFill::Gradient => {
                    let (cache, _) = gradient.as_ref().expect("prepared mask gradient");
                    let denominator = (height as f64 / 2.).ceil() + 1.;
                    for (start, row) in boundary.spans() {
                        let [rgb_alpha, r, g, b] = cache.sample(row as f64 / denominator)?.argb();
                        let value =
                            [r as f32, g as f32, b as f32, 1. - rgb_alpha as f32][channel as usize];
                        values[start..(start + width).min(area)].fill(value);
                    }
                }
                EnvironmentMaskFill::Transparent if channel == 3 => {
                    values[boundary.pixels(true)].fill(1.)
                }
                _ => {}
            }
            if channel < 3 {
                let tint = self.colors[0][channel as usize + 1] as f32;
                if tint != 1. {
                    for value in &mut values[boundary.pixels(false)] {
                        *value *= tint;
                    }
                }
            }
            composed.set_array(channel, values)?;
        }
        let result = SharedImageMap::from_image(composed);
        if self.amount != 0. {
            let blurred = blur_map(
                &result,
                EnvironmentBlur::Axes {
                    horizontal: self.amount,
                    vertical: self.amount,
                },
                cancelled,
            )?;
            for channel in 0..if modifies_alpha { 4 } else { 3 } {
                if cancelled() {
                    return Err(PointwiseError::Cancelled);
                }
                let pixels = boundary.pixels(false);
                let blurred = blurred.channel(channel, true)?.values();
                let mut values = result.channel(channel, true)?.values();
                values[pixels.clone()].copy_from_slice(&blurred[pixels]);
                result.set_buffer(channel, Some(&FloatBuffer::new(values)))?;
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GradientColorPoint, GradientRecord, GradientTransparencyPoint, ImageGeometry};

    fn lower_mask(fill: EnvironmentMaskFill) -> Mask {
        Mask {
            fill,
            hemisphere: EnvironmentMaskHemisphere::Lower,
            amount: 0.,
            colors: [[1., 0.5, 1., 1.], [1., 0.75, 0.25, 0.]],
            gradient: GradientRecord::default(),
        }
    }

    #[test]
    fn mirror_solid_and_transparency_keep_the_original_flat_boundaries() {
        let mut image = ImageMap::new(ImageGeometry::new(8, 8));
        image
            .set_array(0, (0..64).map(|value| value as f32).collect())
            .unwrap();
        image.set_constant(3, 0.25).unwrap();
        let image = SharedImageMap::from_image(image);
        for (fill, red, transparency) in [
            (EnvironmentMaskFill::Mirror, [31., 16., 12., 7.5], [0.25; 4]),
            (
                EnvironmentMaskFill::SolidColor,
                [31., 0.375, 0.375, 31.5],
                [0.25; 4],
            ),
            (
                EnvironmentMaskFill::Transparent,
                [31., 16., 20., 31.5],
                [0.25, 1., 1., 0.25],
            ),
        ] {
            let result = lower_mask(fill)
                .apply::<()>(image.clone(), &|| false)
                .unwrap();
            for (channel, expected) in [(0, red), (3, transparency)] {
                let values = result.channel(channel, true).unwrap().values();
                assert_eq!([31, 32, 40, 63].map(|index| values[index]), expected);
            }
        }
    }

    #[test]
    fn odd_height_gradient_clips_the_final_span_and_writes_opacity_before_tint() {
        let mut mask = lower_mask(EnvironmentMaskFill::Gradient);
        mask.gradient.colors.push(GradientColorPoint {
            coordinate: 0,
            argb: [1., 0.75, 0.25, 0.],
        });
        mask.gradient.transparency.push(GradientTransparencyPoint {
            coordinate: 0,
            value: 0.25,
        });
        let image = SharedImageMap::new(ImageGeometry::new(10, 5));
        let result = mask.apply::<()>(image, &|| false).unwrap();
        let red = result.channel(0, true).unwrap().values();
        let transparency = result.channel(3, true).unwrap().values();
        assert_eq!(&red[..35], &[0.; 35]);
        assert_eq!(&red[35..], &[0.375; 15]);
        assert_eq!(&transparency[..35], &[0.; 35]);
        assert_eq!(&transparency[35..], &[0.75; 15]);
    }
}
