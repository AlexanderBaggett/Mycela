use super::{ImageDecodeError, parse_i32, parse_single, planes, zeroed};
use crate::{BinaryCursor, ImageMap};

/// apk.a(Stream), qualified fresh noBOM ASCII first1024-byte reader profile.
pub fn decode_pfm(reader: &mut BinaryCursor<'_>) -> Result<ImageMap, ImageDecodeError> {
    decode(reader).map_err(|error| match error {
        ImageDecodeError::UnsupportedCapability(_) => error,
        _ => ImageDecodeError::Exception,
    })
}
fn decode(reader: &mut BinaryCursor<'_>) -> Result<ImageMap, ImageDecodeError> {
    // StreamReader's actual initial read advances the shared base stream before
    // parsing any line. Wrong magic/numbers therefore keep this read-ahead.
    let first = reader.read_bytes(1024)?;
    if first.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(ImageDecodeError::UnsupportedCapability(
            "PFM nonASCII/BOM StreamReader profile",
        ));
    }
    let mut start = 0;
    let mut lines = Vec::new();
    for _ in 0..3 {
        if start >= first.len() {
            return Err(ImageDecodeError::Exception);
        }
        let end = first[start..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map(|n| start + n);
        let stop = end.unwrap_or(first.len());
        if first[start..stop].iter().any(|&byte| byte >= 128) {
            return Err(ImageDecodeError::UnsupportedCapability(
                "PFM nonASCII header",
            ));
        }
        if end.is_none() && first.len() == 1024 {
            return Err(ImageDecodeError::UnsupportedCapability(
                "PFM header beyond first reader buffer",
            ));
        }
        if first[start..stop].contains(&b'\r') {
            return Err(ImageDecodeError::UnsupportedCapability(
                "PFM CRLF reader seek profile",
            ));
        }
        lines.push(
            std::str::from_utf8(&first[start..stop]).map_err(|_| ImageDecodeError::Exception)?,
        );
        start = end.map_or(first.len(), |n| n + 1);
        // Parse in actual header order, before reading a later logical line.
        if lines.len() == 1 && !matches!(lines[0], "PF" | "Pf") {
            return Err(ImageDecodeError::Exception);
        }
        if lines.len() == 2 {
            let dimensions: Vec<_> = lines[1].split(' ').collect();
            parse_i32(dimensions[0])?;
            parse_i32(dimensions.get(1).ok_or(ImageDecodeError::Index)?)?;
        }
    }
    if lines.len() != 3 {
        return Err(ImageDecodeError::Exception);
    }
    let dimensions: Vec<_> = lines[1].split(' ').collect();
    let width = parse_i32(dimensions[0])?;
    let height = parse_i32(dimensions.get(1).ok_or(ImageDecodeError::Index)?)?;
    let reverse = parse_single(lines[2])? > 0.0;
    let (image, channels) = planes(width, height, 3)?;
    let header_length = 3 + lines.iter().map(|line| line.len()).sum::<usize>();
    let rewind = 1024_i64 - header_length as i64;
    let mut scratch: Vec<u8> = zeroed(1200)?;
    reader.seek(i64::from(reader.position()?) - rewind)?;
    let mut offset = 0usize;
    while reader.position()? != reader.length()? {
        reader.read_into(&mut scratch, 0, 1200)?;
        for (channel, values) in channels.iter().enumerate() {
            let mut at = channel * 4;
            let mut number = 0;
            while at < scratch.len() && offset + number != values.len() {
                if reverse {
                    scratch[at..at + 4].reverse();
                }
                let value = f32::from_le_bytes(
                    scratch[at..at + 4]
                        .try_into()
                        .map_err(|_| ImageDecodeError::Index)?,
                );
                values.write(offset + number, value)?;
                at += 12;
                number += 1;
            }
        }
        offset = offset.checked_add(100).ok_or(ImageDecodeError::Overflow)?;
    }
    // Source swaps complete rows using a scratch row; no grayscale Pf branch.
    let width = usize::try_from(width).map_err(|_| ImageDecodeError::Exception)?;
    if width == 0 {
        return Err(ImageDecodeError::Exception);
    }
    for values in &channels {
        let mut row: Vec<f32> = zeroed(width)?;
        let mut bottom = values
            .len()
            .checked_sub(width)
            .ok_or(ImageDecodeError::Exception)?;
        let mut top = 0;
        while top < values.len() / 2 {
            for (x, value) in row.iter_mut().enumerate() {
                *value = values.read(bottom + x)?;
            }
            for x in 0..width {
                values.write(bottom + x, values.read(top + x)?)?;
            }
            for (x, value) in row.iter().enumerate() {
                values.write(top + x, *value)?;
            }
            top += width;
            bottom = bottom.wrapping_sub(width);
        }
    }
    Ok(image)
}
