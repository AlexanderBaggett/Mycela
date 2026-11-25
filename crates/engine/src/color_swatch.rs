use crate::ColorBgra;
use std::{error::Error, fmt};

/// Stores the original swatch's color for the characterized opaque RGB value
/// operation. Painting, created handles, input bindings and dialogs belong to
/// their own capabilities; this value does not install an invalidation provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorSwatchValue {
    color: ColorBgra,
}

impl Default for ColorSwatchValue {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorSwatchValue {
    pub const fn new() -> Self {
        Self {
            color: ColorBgra::new(0, 0, 0, 255),
        }
    }

    pub const fn color(&self) -> ColorBgra {
        self.color
    }

    /// Converts opaque RGB, then publishes the complete color. Failure leaves
    /// the stored color unchanged. The original corpus includes NaN in red;
    /// general native casts and exception parameters are not claims of this API.
    pub fn set_opaque_rgb(&mut self, rgb: [f64; 3]) -> Result<(), ColorSwatchError> {
        let [red, green, blue] = rgb.map(component);
        let red = red?;
        let green = green?;
        let blue = blue?;
        let color = ColorBgra::new(blue, green, red, 255);
        self.color = color;
        Ok(())
    }
}

fn component(mut value: f64) -> Result<u8, ColorSwatchError> {
    // Keep the source's ordered comparisons, including NaN and signed zero.
    #[allow(clippy::manual_clamp)]
    {
        if value < 0.0 {
            value = 0.0;
        }
        if value > 1.0 {
            value = 1.0;
        }
    }
    let scaled = value * 255.0;
    if scaled.is_nan() {
        // A Rust saturating NaN cast would silently turn the captured original
        // ArgumentException into a successful color. No native integer or
        // internal failure stage is exposed by this boundary.
        return Err(ColorSwatchError);
    }
    Ok(scaled as u8)
}

/// The observed original color conversion failure class, without invented
/// native cast intermediates, parameter names or exception messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ColorSwatchError;

impl ColorSwatchError {
    pub const fn source_class(self) -> &'static str {
        "ArgumentException"
    }
}

impl fmt::Display for ColorSwatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid swatch color")
    }
}

impl Error for ColorSwatchError {}
