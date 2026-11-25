use super::{ImageDecodeError, planes, read_u16};
use crate::{BinaryCursor, ImageMap};

struct Header {
    id: u8,
    palette: u8,
    kind: u8,
    palette_length: u16,
    width: u16,
    height: u16,
    depth: u8,
    descriptor: u8,
}
fn header(reader: &mut BinaryCursor<'_>) -> Result<Header, ImageDecodeError> {
    let id = reader.read_u8()?;
    let palette = reader.read_u8()?;
    let kind = reader.read_u8()?;
    read_u16(reader)?; // palette origin, ignored
    let palette_length = read_u16(reader)?;
    reader.read_u8()?; // palette entry depth, ignored
    read_u16(reader)?;
    read_u16(reader)?;
    let width = read_u16(reader)?;
    let height = read_u16(reader)?;
    let depth = reader.read_u8()?;
    let descriptor = reader.read_u8()?;
    Ok(Header {
        id,
        palette,
        kind,
        palette_length,
        width,
        height,
        depth,
        descriptor,
    })
}
impl Header {
    fn valid(&self) -> bool {
        self.width != 0
            && self.height != 0
            && self.palette_length <= 256
            && matches!(self.depth, 8 | 15 | 16 | 24 | 32)
    }
}
/// Actual consuming o0.a(Stream) predicate; it never rewinds the caller stream.
pub fn probe_tga(reader: &mut BinaryCursor<'_>) -> Result<bool, ImageDecodeError> {
    Ok(header(reader)?.valid())
}
/// Actual o0.b(Stream) decoder, including its nonstandard palette/15bit swap.
pub fn decode_tga(reader: &mut BinaryCursor<'_>) -> Result<ImageMap, ImageDecodeError> {
    let header = header(reader)?;
    let rle = match header.kind {
        1..=3 => false,
        9..=11 => true,
        _ => return Err(ImageDecodeError::Format),
    };
    if !header.valid() {
        return Err(ImageDecodeError::Format);
    }
    if header.id > 0 {
        reader.seek(i64::from(reader.position()?) + i64::from(header.id))?;
    }
    let width = usize::from(header.width);
    let height = usize::from(header.height);
    let (image, channels) = planes(i32::from(header.width), i32::from(header.height), 4)?;
    let mut palette = None;
    if header.palette != 0 {
        let mut values = Vec::new();
        values
            .try_reserve_exact(usize::from(header.palette_length))
            .map_err(|_| ImageDecodeError::Allocation)?;
        for _ in 0..header.palette_length {
            let first = reader.read_u8()?;
            let green = reader.read_u8()?;
            let third = reader.read_u8()?;
            values.push([third, green, first, 255]); // ColorBgra.c's B,G,R,A
        }
        palette = Some(values);
    }
    if matches!(header.kind, 3 | 11) {
        palette = Some((0u8..=255).map(|v| [v, v, v, 255]).collect());
    }
    let mut pending = 255u8;
    for row in 0..height {
        let y = if header.descriptor & 32 != 0 {
            row
        } else {
            height - row - 1
        };
        let mut x = 0usize;
        while x < width {
            let (count, run, next, rewind) = if rle {
                let control = if pending != 255 {
                    pending
                } else {
                    reader.read_u8()?
                };
                let run = control & 128 != 0;
                let count = if run {
                    usize::from(control - 127)
                } else {
                    usize::from(control) + 1
                };
                if x + count > width {
                    let remaining = count - (width - x) - 1;
                    (
                        width - x,
                        run,
                        if run {
                            128 + remaining as u8
                        } else {
                            remaining as u8
                        },
                        run,
                    )
                } else {
                    (count, run, 255, false)
                }
            } else {
                (width - x, false, 255, false)
            };
            pending = next;
            let position = reader.position()?;
            if run {
                let color = pixel(reader, header.depth, palette.as_deref())?;
                for _ in 0..count {
                    store(&channels, y * width + x, color)?;
                    x += 1;
                }
                if rewind {
                    reader.seek(i64::from(position))?;
                }
            } else {
                for _ in 0..count {
                    let color = pixel(reader, header.depth, palette.as_deref())?;
                    store(&channels, y * width + x, color)?;
                    x += 1;
                }
            }
        }
    }
    if header.descriptor & 16 != 0 {
        for channel in &channels {
            for row in 0..height {
                for x in 0..width / 2 {
                    let left = row * width + x;
                    let right = row * width + width - x - 1;
                    let previous = channel.read(left)?;
                    let next = channel.read(right)?;
                    channel.write(left, next)?;
                    channel.write(right, previous)?;
                }
            }
        }
    }
    Ok(image)
}
fn pixel(
    reader: &mut BinaryCursor<'_>,
    depth: u8,
    palette: Option<&[[u8; 4]]>,
) -> Result<[u8; 4], ImageDecodeError> {
    Ok(match depth {
        32 => [
            reader.read_u8()?,
            reader.read_u8()?,
            reader.read_u8()?,
            reader.read_u8()?,
        ],
        24 => [reader.read_u8()?, reader.read_u8()?, reader.read_u8()?, 255],
        15 | 16 => {
            let value = read_u16(reader)?;
            [
                ((value >> 7) & 248) as u8,
                ((value >> 2) & 248) as u8,
                ((value & 31) * 8) as u8,
                255,
            ]
        }
        8 => {
            let index = usize::from(reader.read_u8()?);
            *palette
                .ok_or(ImageDecodeError::NullReference)?
                .get(index)
                .ok_or(ImageDecodeError::Format)?
        }
        _ => return Err(ImageDecodeError::Format),
    })
}
fn store(
    channels: &[crate::FloatBuffer],
    index: usize,
    bgra: [u8; 4],
) -> Result<(), ImageDecodeError> {
    channels[2].write(index, f32::from(bgra[0]) / 255.0)?;
    channels[1].write(index, f32::from(bgra[1]) / 255.0)?;
    channels[0].write(index, f32::from(bgra[2]) / 255.0)?;
    channels[3].write(index, (255.0 - f32::from(bgra[3])) / 255.0)?;
    Ok(())
}
