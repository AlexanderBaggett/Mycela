use super::{ImageDecodeError, array_length, ascii, byte_at, parse_i32, planes, zeroed};
use crate::{BinaryCursor, BinaryError, ImageMap};

/// tx.a(Stream). The caller retains the stream, including its failure cursor.
pub fn decode_hdr(reader: &mut BinaryCursor<'_>) -> Result<ImageMap, ImageDecodeError> {
    decode(reader).map_err(|error| match error {
        ImageDecodeError::Binary(BinaryError::End) => ImageDecodeError::Exception,
        error => error,
    })
}
fn decode(reader: &mut BinaryCursor<'_>) -> Result<ImageMap, ImageDecodeError> {
    let mut previous = 0;
    let mut header = Vec::new();
    loop {
        let byte = reader.read_u8()?;
        if byte == b'\n' && previous == b'\n' {
            break;
        }
        previous = byte;
        header
            .try_reserve(1)
            .map_err(|_| ImageDecodeError::Allocation)?;
        header.push(byte);
    }
    if ascii(&header).trim_end_matches('\n').split('\n').next() != Some("#?RADIANCE") {
        return Err(ImageDecodeError::Exception);
    }
    let mut resolution = Vec::new();
    loop {
        let byte = reader.read_u8()?;
        if byte == b'\n' {
            break;
        }
        resolution
            .try_reserve(1)
            .map_err(|_| ImageDecodeError::Allocation)?;
        resolution.push(byte);
    }
    let text = ascii(&resolution);
    let fields: Vec<_> = text.split(' ').collect();
    if fields.len() != 4 {
        return Err(ImageDecodeError::Exception);
    }
    if byte_at(fields[0], 1)? == b'X' {
        parse_i32(fields[1])?;
        parse_i32(fields[3])?;
        return Err(ImageDecodeError::Exception);
    }
    let height = parse_i32(fields[1])?;
    let width = parse_i32(fields[3])?;
    let length = array_length(width)?;
    let mut scan = [
        zeroed(length)?,
        zeroed(length)?,
        zeroed(length)?,
        zeroed(length)?,
    ];
    let (image, channels) = planes(width, height, 3)?;
    let forward = (byte_at(fields[0], 0)? == b'-' && byte_at(fields[0], 1)? == b'Y')
        || (byte_at(fields[2], 0)? == b'-' && byte_at(fields[2], 1)? == b'Y');
    let mut last_exponent = i32::MIN;
    let mut factor = 0.0;
    for row in (0..height).rev() {
        scanline(&mut scan, width, reader)?;
        let destination = if forward {
            (height - row - 1).wrapping_mul(width)
        } else {
            row.wrapping_mul(width)
        };
        for (x, &scan_exponent) in scan[3].iter().enumerate() {
            let exponent = i32::from(scan_exponent) - 128;
            if last_exponent != exponent {
                // Exact integral powers of2 followed by the source f32 scale.
                factor = (2.0_f64.powi(exponent) as f32) * 0.00390625_f32;
                last_exponent = exponent;
            }
            let at = usize::try_from(destination).map_err(|_| ImageDecodeError::Index)? + x;
            for (channel, plane) in channels.iter().enumerate() {
                plane.write(at, f32::from(scan[channel][x]) * factor)?;
            }
        }
    }
    Ok(image)
}
fn flat(
    scan: &mut [Vec<u8>; 4],
    mut remaining: i32,
    reader: &mut BinaryCursor<'_>,
) -> Result<(), ImageDecodeError> {
    let mut shift: u32 = 0;
    let mut at = 0usize;
    while remaining > 0 {
        for plane in scan.iter_mut() {
            // Array receiver/index validation follows ReadByte in the source.
            let value = reader.read_u8()?;
            *plane.get_mut(at).ok_or(ImageDecodeError::Index)? = value;
        }
        if scan[0][at] == 1 && scan[1][at] == 1 && scan[2][at] == 1 {
            let count = i32::from(scan[3][at]).wrapping_shl(shift);
            for _ in 0..count {
                let previous = at.checked_sub(1).ok_or(ImageDecodeError::Index)?;
                for plane in scan.iter_mut() {
                    let value = *plane.get(previous).ok_or(ImageDecodeError::Index)?;
                    *plane.get_mut(at).ok_or(ImageDecodeError::Index)? = value;
                }
                at += 1;
                remaining -= 1;
            }
            shift = shift.wrapping_add(8);
        } else {
            at += 1;
            remaining -= 1;
            shift = 0;
        }
    }
    Ok(())
}
fn scanline(
    scan: &mut [Vec<u8>; 4],
    width: i32,
    reader: &mut BinaryCursor<'_>,
) -> Result<(), ImageDecodeError> {
    if !(8..=32767).contains(&width) {
        return flat(scan, width, reader);
    }
    let first = reader.read_u8()?;
    if first != 2 {
        reader.seek(i64::from(reader.position()?) - 1)?;
        return flat(scan, width, reader);
    }
    scan[1][0] = reader.read_u8()?;
    scan[2][0] = reader.read_u8()?;
    let fourth = reader.read_u8()?;
    if scan[1][0] != 2 || scan[2][0] & 128 != 0 {
        scan[0][0] = 2;
        scan[3][0] = fourth;
        reader.seek(i64::from(reader.position()?) - 4)?;
        return flat(scan, width, reader);
    }
    // Encoded scanline width is intentionally ignored by tx.
    for plane in scan.iter_mut() {
        let mut at = 0usize;
        while at < width as usize {
            let code = reader.read_u8()?;
            if code > 128 {
                let value = reader.read_u8()?;
                for _ in 0..(code & 127) {
                    *plane.get_mut(at).ok_or(ImageDecodeError::Index)? = value;
                    at += 1;
                }
            } else {
                reader.read_into(plane, at, usize::from(code))?;
                at += usize::from(code);
            }
        }
    }
    Ok(())
}
