use std::{
    error::Error,
    fmt,
    io::{self, Read, Write},
};

/// Floating color data owned by value. Named comparisons retain Genetica's
/// nontransitive tolerances; this type deliberately does not implement Eq/Hash.
#[derive(Clone, Copy, Debug, Default)]
pub struct RgbColor {
    alpha: f64,
    red: f64,
    green: f64,
    blue: f64,
    grayscale: bool,
}

impl RgbColor {
    pub const fn new(red: f64, green: f64, blue: f64, alpha: f64) -> Self {
        Self {
            alpha,
            red,
            green,
            blue,
            grayscale: false,
        }
    }

    pub const fn from_argb(components: [f64; 4]) -> Self {
        Self::new(components[1], components[2], components[3], components[0])
    }

    pub fn from_argb_bytes(components: [u8; 4]) -> Self {
        // Source construction explicitly rounds each quotient to f32 first.
        Self::from_argb(components.map(|component| f64::from(f32::from(component) / 255.0)))
    }

    pub const fn argb(self) -> [f64; 4] {
        [self.alpha, self.red, self.green, self.blue]
    }

    pub const fn grayscale(mut self, value: bool) -> Self {
        self.grayscale = value;
        self
    }

    pub const fn is_grayscale(self) -> bool {
        self.grayscale
    }

    pub fn channel(self, index: i32) -> Result<f64, RgbChannelError> {
        [self.red, self.green, self.blue, self.alpha]
            .get(index as usize)
            .copied()
            .ok_or(RgbChannelError(index))
    }

    pub fn grayscale_value(self) -> f64 {
        ((self.red * self.red + self.green * self.green + self.blue * self.blue) / 3.0).sqrt()
    }

    pub fn equals(self, other: Self) -> bool {
        self.within(other, 1e-5)
    }
    pub fn is_near(self, other: Self) -> bool {
        self.within(other, 1e-4)
    }

    /// Source white detection uses a strict threshold and ignores alpha and
    /// the grayscale display flag.
    pub fn is_white(self) -> bool {
        (self.red - 1.0).abs() < 1e-5
            && (self.green - 1.0).abs() < 1e-5
            && (self.blue - 1.0).abs() < 1e-5
    }

    fn within(self, other: Self, tolerance: f64) -> bool {
        (other.red - self.red).abs() <= tolerance
            && (other.green - self.green).abs() <= tolerance
            && (other.blue - self.blue).abs() <= tolerance
            && (other.alpha - self.alpha).abs() <= tolerance
    }

    /// Returns newly constructed clipped channels; the source resets grayscale.
    pub fn clipped(self) -> Self {
        Self::from_argb(self.argb().map(clip_unit))
    }

    pub fn to_hsl(self) -> HslColor {
        let red = clip_unit(self.red);
        let green = clip_unit(self.green);
        let blue = clip_unit(self.blue);
        let maximum = maximum(maximum(red, green), blue);
        let minimum = minimum(minimum(red, green), blue);
        let lightness = (minimum + maximum) / 2.0;
        let mut hue = 0.0;
        let mut saturation = 0.0;
        if lightness <= 0.0 {
            return HslColor::new(hue, saturation, lightness);
        }
        let span = maximum - minimum;
        saturation = span;
        if saturation > 0.0 {
            saturation /= if lightness <= 0.5 {
                maximum + minimum
            } else {
                2.0 - maximum - minimum
            };
            let red_distance = (maximum - red) / span;
            let green_distance = (maximum - green) / span;
            let blue_distance = (maximum - blue) / span;
            hue = if red == maximum {
                if green == minimum {
                    5.0 + blue_distance
                } else {
                    1.0 - green_distance
                }
            } else if green != maximum {
                if red == minimum {
                    3.0 + green_distance
                } else {
                    5.0 - red_distance
                }
            } else if blue == minimum {
                1.0 + red_distance
            } else {
                3.0 - blue_distance
            };
            hue /= 6.0;
            // System.Double.Epsilon is the smallest positive subnormal, not
            // Rust's f64::EPSILON (the relative machine precision).
            if (hue - 1.0).abs() < f64::from_bits(1) {
                hue = 0.0;
            }
        }
        HslColor::new(hue, saturation, lightness)
    }

    pub fn to_argb_bytes(self) -> [u8; 4] {
        let alpha = round_byte(self.alpha);
        if self.grayscale {
            let lightness = round_byte(self.to_hsl().lightness);
            [alpha, lightness, lightness, lightness]
        } else {
            [
                alpha,
                round_byte(self.red),
                round_byte(self.green),
                round_byte(self.blue),
            ]
        }
    }

    /// Source rounded opaque export. Non-grayscale components are intentionally
    /// neither clamped nor masked before signed bit packing.
    pub fn rounded_opaque_argb(self) -> i32 {
        let [red, green, blue] = if self.grayscale {
            let value = i32::from(round_byte(self.grayscale_value()));
            [value; 3]
        } else {
            [self.red, self.green, self.blue].map(|value| (value * 255.0).round_ties_even() as i32)
        };
        (red << 16) | (green << 8) | blue | (0xff00_0000u32 as i32)
    }

    pub fn truncated_opaque_argb(self) -> i32 {
        let [red, green, blue] = if self.grayscale {
            [truncate_byte(self.to_hsl().lightness); 3]
        } else {
            [self.red, self.green, self.blue].map(truncate_byte)
        };
        (i32::from(red) << 16) | (i32::from(green) << 8) | i32::from(blue) | (0xff00_0000u32 as i32)
    }

    /// Borrows the caller's writer; encodes A/R/G/B doubles without the flag.
    pub fn write_to(self, writer: &mut impl Write) -> io::Result<()> {
        for component in self.argb() {
            writer.write_all(&component.to_le_bytes())?;
        }
        Ok(())
    }

    /// Borrows the caller's reader. A short read retains already-read fields
    /// and the existing grayscale flag, matching the source's sequential reads.
    pub fn read_from(&mut self, reader: &mut impl Read) -> io::Result<()> {
        for component in [
            &mut self.alpha,
            &mut self.red,
            &mut self.green,
            &mut self.blue,
        ] {
            let mut bytes = [0; 8];
            reader.read_exact(&mut bytes)?;
            *component = f64::from_le_bytes(bytes);
        }
        Ok(())
    }
}

/// Hue, saturation and lightness values; conversion owns no shared state.
#[derive(Clone, Copy, Debug, Default)]
pub struct HslColor {
    hue: f64,
    saturation: f64,
    lightness: f64,
}

impl HslColor {
    pub const fn new(hue: f64, saturation: f64, lightness: f64) -> Self {
        Self {
            hue,
            saturation,
            lightness,
        }
    }
    pub const fn components(self) -> [f64; 3] {
        [self.hue, self.saturation, self.lightness]
    }

    pub fn to_rgb(self) -> RgbColor {
        let mut color = RgbColor::new(0.0, 0.0, 0.0, 1.0);
        self.update_rgb(&mut color);
        color
    }

    /// Updates the channels and alpha while preserving the caller's flag.
    pub fn update_rgb(self, color: &mut RgbColor) {
        let mut hue = self.hue;
        let saturation = self.saturation;
        let lightness = self.lightness;
        if hue >= 1.0 {
            hue %= 1.0;
        }
        let mut channels = [lightness; 3];
        let peak = if lightness <= 0.5 {
            lightness * (1.0 + saturation)
        } else {
            lightness + saturation - lightness * saturation
        };
        if peak > 0.0 {
            let floor = lightness + lightness - peak;
            let span = (peak - floor) / peak;
            hue *= 6.0;
            let sector = hue as i32;
            let fraction = hue - f64::from(sector);
            let delta = peak * span * fraction;
            let rising = floor + delta;
            let falling = peak - delta;
            channels = match sector {
                0 => [peak, rising, floor],
                1 => [falling, peak, floor],
                2 => [floor, peak, rising],
                3 => [floor, falling, peak],
                4 => [rising, floor, peak],
                5 => [peak, floor, falling],
                _ => channels,
            };
        }
        [color.red, color.green, color.blue] = channels;
        color.alpha = 1.0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RgbChannelError(i32);
impl RgbChannelError {
    pub const fn index(self) -> i32 {
        self.0
    }
}
impl fmt::Display for RgbChannelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "color channel {} is outside 0..=3", self.0)
    }
}
impl Error for RgbChannelError {}

fn clip_unit(value: f64) -> f64 {
    // Source Math.Max(0, Math.Min(1, x)) turns negative zero positive.
    if value == 0.0 {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}
fn minimum(left: f64, right: f64) -> f64 {
    if left.is_nan() {
        left
    } else if right.is_nan() {
        right
    } else {
        left.min(right)
    }
}
fn maximum(left: f64, right: f64) -> f64 {
    if left.is_nan() {
        left
    } else if right.is_nan() {
        right
    } else {
        left.max(right)
    }
}
fn round_byte(value: f64) -> u8 {
    ((value * 255.0).round_ties_even() as i32).clamp(0, 255) as u8
}
fn truncate_byte(value: f64) -> u8 {
    ((value * 255.0) as i32).clamp(0, 255) as u8
}
