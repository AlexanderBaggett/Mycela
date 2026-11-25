use crate::{ImageError, ImageGeometry, ImageMap};
use std::{error::Error, fmt};

/// Solid Color's detached, unanimated and unadjusted stored-value behavior.
///
/// RGB stores raw doubles. Transparency has the source's three-decimal hard
/// limits. Property events, attachments, animation and graph evaluation remain
/// separate capabilities; this value never silently evaluates those inputs.
#[derive(Debug, Default)]
pub struct StoredSolidColor {
    color: [f64; 3],
    transparency: f64,
}

impl StoredSolidColor {
    pub const fn color(&self) -> [f64; 3] {
        self.color
    }
    pub const fn transparency(&self) -> f64 {
        self.transparency
    }

    /// Exact numeric equality suppresses replacement, including zero signs.
    pub fn set_color(&mut self, color: [f64; 3]) {
        if self.color != color {
            self.color = color;
        }
    }

    pub fn set_transparency(&mut self, value: f64) {
        let value = normalize_transparency(value);
        if self.transparency != value {
            self.transparency = value;
        }
    }

    /// The source node's direct channel-edit operation accepts a float and
    /// widens it before assigning the corresponding double-valued property.
    pub fn set_channel(&mut self, channel: i32, value: f32) -> Result<(), SolidColorChannelError> {
        match channel {
            0..=2 => {
                let mut color = self.color;
                color[channel as usize] = f64::from(value);
                self.set_color(color);
            }
            3 => self.set_transparency(f64::from(value)),
            _ => return Err(SolidColorChannelError { channel }),
        }
        Ok(())
    }

    /// Reads above channel three return zero; negative reads are errors.
    pub fn constant_channel(&self, channel: i32) -> Result<f32, SolidColorChannelError> {
        match channel {
            0..=2 => Ok(self.color[channel as usize] as f32),
            3 => Ok(self.transparency as f32),
            4.. => Ok(0.0),
            _ => Err(SolidColorChannelError { channel }),
        }
    }

    /// All channels of this stored-value node satisfy the source constant
    /// query, including indices outside its four rendered channels.
    pub const fn is_channel_constant(&self, _channel: i32) -> bool {
        true
    }

    /// Render scalar RGB and transparency at origin zero. Stored values are
    /// independent of time; signed dimensions keep ImageMap's wrapping rules.
    pub fn render(&self, width: i32, height: i32, _time: f64) -> Result<ImageMap, ImageError> {
        render_stored_values(self.color, self.transparency, width, height)
    }
}

/// Shared scalar rendering seam. Live nodes supply a current-value snapshot;
/// this helper owns no second property cache or evaluation state.
pub(crate) fn render_stored_values(
    color: [f64; 3],
    transparency: f64,
    width: i32,
    height: i32,
) -> Result<ImageMap, ImageError> {
    let mut image = ImageMap::new(ImageGeometry::new(width, height));
    for (channel, value) in color.iter().enumerate() {
        image.set_constant(channel as i32, *value as f32)?;
    }
    image.set_constant(3, transparency as f32)?;
    Ok(image)
}

pub(crate) fn normalize_transparency(value: f64) -> f64 {
    let rounded = crate::double_rounding::round_three_decimal_places(value);
    rounded.clamp(-1_000_000.0, 1_000_000.0)
}

impl Clone for StoredSolidColor {
    fn clone(&self) -> Self {
        // os's color clone uses its normal setter against default black. This
        // intentionally turns an all-zero color's negative zeros positive.
        let mut result = Self {
            color: [0.0; 3],
            transparency: self.transparency,
        };
        result.set_color(self.color);
        result
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SolidColorChannelError {
    pub channel: i32,
}
impl SolidColorChannelError {
    pub const fn source_class(self) -> &'static str {
        "ArgumentException"
    }
}
impl fmt::Display for SolidColorChannelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid Solid Color RGB channel {}",
            self.channel
        )
    }
}
impl Error for SolidColorChannelError {}
