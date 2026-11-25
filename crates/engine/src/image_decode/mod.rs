//! Source-derived encoded readers. Decoded state is computed only from bytes.
mod hdr;
mod pfm;
mod standard;
mod tga;

pub use hdr::decode_hdr;
pub use pfm::decode_pfm;
pub(crate) use standard::{decode_bitmap, decode_jpeg};
pub use tga::{decode_tga, probe_tga};

use crate::{BinaryCursor, BinaryError, BitmapError, ImageError, ImageGeometry, ImageMap};
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageDecodeError {
    Binary(BinaryError),
    Image(ImageError),
    Bitmap(BitmapError),
    Exception,
    Format,
    Overflow,
    Index,
    NullReference,
    Allocation,
    UnsupportedCapability(&'static str),
}
impl ImageDecodeError {
    pub const fn source_class(self) -> Option<&'static str> {
        match self {
            Self::Binary(error) => Some(error.source_class()),
            Self::Image(error) => Some(error.source_class()),
            Self::Bitmap(error) => error.source_class(),
            Self::Exception => Some("Exception"),
            Self::Format => Some("FormatException"),
            Self::Overflow => Some("OverflowException"),
            Self::Index => Some("IndexOutOfRangeException"),
            Self::NullReference => Some("NullReferenceException"),
            Self::Allocation => Some("OutOfMemoryException"),
            Self::UnsupportedCapability(_) => None,
        }
    }
}
impl From<BinaryError> for ImageDecodeError {
    fn from(value: BinaryError) -> Self {
        Self::Binary(value)
    }
}
impl From<ImageError> for ImageDecodeError {
    fn from(value: ImageError) -> Self {
        Self::Image(value)
    }
}
impl From<BitmapError> for ImageDecodeError {
    fn from(value: BitmapError) -> Self {
        Self::Bitmap(value)
    }
}
impl fmt::Display for ImageDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for ImageDecodeError {}

pub(super) fn zeroed<T: Default + Clone>(length: usize) -> Result<Vec<T>, ImageDecodeError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| ImageDecodeError::Allocation)?;
    values.resize(length, T::default());
    Ok(values)
}
fn array_length(length: i32) -> Result<usize, ImageDecodeError> {
    usize::try_from(length).map_err(|_| ImageDecodeError::Overflow)
}
fn planes(
    width: i32,
    height: i32,
    count: usize,
) -> Result<(ImageMap, Vec<crate::FloatBuffer>), ImageDecodeError> {
    let mut image = ImageMap::new(ImageGeometry::new(width, height));
    let mut channels = Vec::new();
    channels
        .try_reserve_exact(count)
        .map_err(|_| ImageDecodeError::Allocation)?;
    // Real nn planes are allocated in source order before any codec data reads.
    // Unlike public set_array/store, materialize never collapses area1.
    for channel in 0..count {
        channels.push(image.materialize(channel as i32)?);
    }
    Ok((image, channels))
}
// Framework4.8 Number.ParseNumber / StringToNumber, qualified ASCII/en-US
// Integer and Float|AllowThousands grammar. Validate the ENTIRE syntax before
// converting: a large prefix followed by junk is Format, not Overflow.
fn numeric_ascii(text: &str, floating: bool) -> Result<&str, ImageDecodeError> {
    if !text.is_ascii() {
        return Err(ImageDecodeError::UnsupportedCapability(
            "numeric nonASCII/current-culture profile",
        ));
    }
    let end = text.find('\0').unwrap_or(text.len());
    if text.as_bytes()[end..].iter().any(|&byte| byte != 0) {
        return Err(ImageDecodeError::Format);
    }
    let number = text[..end].trim_matches(ascii_white);
    let bytes = number.as_bytes();
    let mut at = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut digits = false;
    let mut decimal = false;
    while let Some(&byte) = bytes.get(at) {
        if byte.is_ascii_digit() {
            digits = true;
        } else if floating && byte == b'.' && !decimal {
            decimal = true;
        } else if !(floating && byte == b',' && digits && !decimal) {
            break;
        }
        at += 1;
    }
    if !digits {
        return Err(ImageDecodeError::Format);
    }
    if floating && matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let begin = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if begin == at {
            return Err(ImageDecodeError::Format);
        }
    }
    if at != bytes.len() {
        return Err(ImageDecodeError::Format);
    }
    Ok(number)
}
fn ascii_white(value: char) -> bool {
    value == ' ' || ('\t'..='\r').contains(&value)
}
fn parse_i32(text: &str) -> Result<i32, ImageDecodeError> {
    use std::num::IntErrorKind;
    numeric_ascii(text, false)?
        .parse()
        .map_err(|error: std::num::ParseIntError| match error.kind() {
            IntErrorKind::PosOverflow | IntErrorKind::NegOverflow => ImageDecodeError::Overflow,
            _ => ImageDecodeError::Format,
        })
}
fn parse_single(text: &str) -> Result<f32, ImageDecodeError> {
    let number = match numeric_ascii(text, true) {
        Ok(number) => number,
        Err(ImageDecodeError::Format) => {
            // ParseSingle only tries these exact trimmed culture symbols after
            // numeric syntax fails. Symbol+NUL is NOT numeric trailing-NUL input.
            return match text.trim_matches(ascii_white) {
                "Infinity" => Ok(f32::INFINITY),
                "-Infinity" => Ok(f32::NEG_INFINITY),
                "NaN" => Ok(f32::NAN),
                _ => Err(ImageDecodeError::Format),
            };
        }
        Err(error) => return Err(error),
    };
    let normalized = if number.contains(',') {
        std::borrow::Cow::Owned(number.replace(',', ""))
    } else {
        std::borrow::Cow::Borrowed(number)
    };
    // ReferenceSource ParseSingle first converts NumberBuffer to Double, then
    // casts to Single and rejects numeric infinity at either conversion.
    // General Framework native NumberBuffer rounding remains a disclosed gap.
    let double = normalized
        .parse::<f64>()
        .map_err(|_| ImageDecodeError::Overflow)?;
    let single = double as f32;
    if !double.is_finite() || single.is_infinite() {
        return Err(ImageDecodeError::Overflow);
    }
    Ok(single)
}
fn ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&value| if value < 128 { char::from(value) } else { '?' })
        .collect()
}
fn byte_at(text: &str, index: usize) -> Result<u8, ImageDecodeError> {
    text.as_bytes()
        .get(index)
        .copied()
        .ok_or(ImageDecodeError::Index)
}
fn read_u16(reader: &mut BinaryCursor<'_>) -> Result<u16, ImageDecodeError> {
    // vz.c does sequential ReadByte, unlike ReadInt16's partial bulk read.
    let low = reader.read_u8()?;
    let high = reader.read_u8()?;
    Ok(u16::from_le_bytes([low, high]))
}

#[cfg(test)]
mod numeric_laws {
    use super::{ImageDecodeError, parse_i32, parse_single};

    // ReferenceSource ParseNumber/StringToNumber/TrailingZeros. These source
    // laws are separate from original90 and do not claim native rounding proof.
    #[test]
    fn numeric_syntax_precedes_overflow_and_accepts_numeric_nul_suffix() {
        assert_eq!(parse_i32(" \t+2147483647 \0\0"), Ok(i32::MAX));
        assert_eq!(parse_i32("-2147483648\0"), Ok(i32::MIN));
        assert_eq!(parse_i32("2147483648\0"), Err(ImageDecodeError::Overflow));
        assert_eq!(
            parse_i32("999999999999999999999x"),
            Err(ImageDecodeError::Format)
        );
        assert_eq!(parse_i32("1\0 \0"), Err(ImageDecodeError::Format));
        assert_eq!(parse_single("1,,2.5e+1 \0\0"), Ok(125.0));
        assert_eq!(parse_single("1e5000x"), Err(ImageDecodeError::Format));
        assert_eq!(parse_single("1e5000"), Err(ImageDecodeError::Overflow));
        assert_eq!(
            parse_single("3.4028236e38"),
            Err(ImageDecodeError::Overflow)
        );
        assert!(parse_single("3.4028235e38").unwrap().is_finite());
    }

    #[test]
    fn framework_single_symbols_are_exact_fallbacks_not_rust_aliases() {
        assert_eq!(parse_single(" Infinity \t"), Ok(f32::INFINITY));
        assert_eq!(parse_single("-Infinity"), Ok(f32::NEG_INFINITY));
        assert!(parse_single("NaN").unwrap().is_nan());
        for symbol in [
            "inf",
            "infinity",
            "INFINITY",
            "nan",
            "NAN",
            "+Infinity",
            "-NaN",
            "Infinity\0",
            "NaN\0",
        ] {
            assert_eq!(
                parse_single(symbol),
                Err(ImageDecodeError::Format),
                "{symbol:?}"
            );
        }
        assert_eq!(parse_single("-0.25\0"), Ok(-0.25));
        assert_eq!(parse_single(".5e-1"), Ok(0.05));
    }
}
