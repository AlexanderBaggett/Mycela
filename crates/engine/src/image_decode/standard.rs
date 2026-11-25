//! Standard image decoders that produce native-profile bitmap layouts.
use super::{ImageDecodeError, zeroed};
use crate::{Bitmap, BitmapFormat, BitmapLayout, ByteBuffer};
use ::image::ImageDecoder;
use std::io::Cursor;

fn unsupported() -> ImageDecodeError {
    ImageDecodeError::UnsupportedCapability("unqualified native encoded image or decoder failure")
}
pub(crate) fn decode_bitmap(bytes: &[u8]) -> Result<Bitmap, ImageDecodeError> {
    // Decode from the stream origin after the caller's TGA probe.
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(bytes)
    } else if bytes.starts_with(b"BM") {
        bmp(bytes)
    } else if bytes.starts_with(b"\xff\xd8") {
        decode_jpeg(bytes)
    } else {
        Err(unsupported())
    }
}
pub(crate) fn decode_jpeg(bytes: &[u8]) -> Result<Bitmap, ImageDecodeError> {
    use mycela_jpeg::{DecodeError, DecodedJpeg};

    match DecodedJpeg::decode(bytes) {
        Ok(decoded) => {
            let [width, height] = decoded.dimensions();
            return bitmap(
                width,
                height,
                BitmapFormat::Rgb24,
                decoded.rgb(),
                3,
                None,
                None,
            );
        }
        Err(DecodeError::Allocation) => return Err(ImageDecodeError::Allocation),
        Err(DecodeError::Format | DecodeError::ColorModel) => {}
    }
    let pixels = ::image::load_from_memory_with_format(bytes, ::image::ImageFormat::Jpeg)
        .map_err(|_| ImageDecodeError::Format)?
        .into_rgb8();
    bitmap(
        pixels.width(),
        pixels.height(),
        BitmapFormat::Rgb24,
        pixels.as_raw(),
        3,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)] // One decoded byte layout plus palette/transparency metadata.
fn bitmap(
    width: u32,
    height: u32,
    format: BitmapFormat,
    pixels: &[u8],
    components: usize,
    palette: Option<&[[u8; 3]]>,
    transparency: Option<&[u8]>,
) -> Result<Bitmap, ImageDecodeError> {
    let width = i32::try_from(width).map_err(|_| unsupported())?;
    let height = i32::try_from(height).map_err(|_| unsupported())?;
    let layout = BitmapLayout::new(width, height, format)?;
    let length = (layout.stride() as usize)
        .checked_mul(height as usize)
        .ok_or(ImageDecodeError::Allocation)?;
    let mut storage: Vec<u8> = zeroed(length)?;
    let input_row = (width as usize)
        .checked_mul(components)
        .ok_or(ImageDecodeError::Allocation)?;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let at = y * input_row + x * components;
            let target = y * layout.stride() as usize + x * format.bytes_per_pixel();
            if format == BitmapFormat::Indexed8 {
                storage[target] = *pixels.get(at).ok_or_else(unsupported)?;
            } else {
                let color = if let Some(palette) = palette {
                    let index = usize::from(*pixels.get(at).ok_or_else(unsupported)?);
                    let rgb = palette.get(index).ok_or_else(unsupported)?;
                    [
                        rgb[0],
                        rgb[1],
                        rgb[2],
                        transparency
                            .and_then(|v| v.get(index))
                            .copied()
                            .unwrap_or(255),
                    ]
                } else {
                    let rgb = pixels.get(at..at + 3).ok_or_else(unsupported)?;
                    let alpha = if components == 4 {
                        *pixels.get(at + 3).ok_or_else(unsupported)?
                    } else {
                        // PNG truecolor tRNS compares the full16bit key values;
                        // with8bit input only keys<=255 can match.
                        if transparency.is_some_and(|key| {
                            key.len() == 6
                                && key[0] == 0
                                && key[2] == 0
                                && key[4] == 0
                                && rgb == [key[1], key[3], key[5]]
                        }) {
                            0
                        } else {
                            255
                        }
                    };
                    [rgb[0], rgb[1], rgb[2], alpha]
                };
                storage[target..target + 3].copy_from_slice(&[color[2], color[1], color[0]]);
                if format.bytes_per_pixel() == 4 {
                    storage[target + 3] = color[3];
                }
            }
        }
    }
    let bitmap = Bitmap::from_buffer(layout, ByteBuffer::new(storage))?;
    if format == BitmapFormat::Indexed8 {
        let mut argb = [0xff000000u32; 256];
        for (target, color) in argb.iter_mut().zip(palette.ok_or_else(unsupported)?) {
            *target = 0xff000000
                | (u32::from(color[0]) << 16)
                | (u32::from(color[1]) << 8)
                | u32::from(color[2]);
        }
        bitmap.set_palette(&argb)?;
    }
    Ok(bitmap)
}
fn png(bytes: &[u8]) -> Result<Bitmap, ImageDecodeError> {
    let mut offset = 8usize;
    let mut has_transparency = false;
    let mut raw_transparency = None;
    let mut before_data = true;
    while offset < bytes.len() {
        let prefix = bytes.get(offset..offset + 8).ok_or_else(unsupported)?;
        let length =
            u32::from_be_bytes(prefix[..4].try_into().map_err(|_| unsupported())?) as usize;
        let kind = &prefix[4..8];
        // The source resource loader calls Image.FromStream(Stream), whose
        // default disables embedded color management. Keep sample values for
        // sRGB/gAMA/cHRM metadata; IDENTITY below performs no gamma conversion.
        // ICC, significant-bit and animation behavior remain separate profiles.
        if matches!(kind, b"iCCP" | b"sBIT" | b"acTL") {
            return Err(ImageDecodeError::UnsupportedCapability(
                "PNG color management/animation profile",
            ));
        }
        if before_data && kind == b"tRNS" {
            has_transparency = true;
            let start = offset.checked_add(8).ok_or_else(unsupported)?;
            let end = start.checked_add(length).ok_or_else(unsupported)?;
            raw_transparency = Some(bytes.get(start..end).ok_or_else(unsupported)?);
        }
        if kind == b"IDAT" || kind == b"IEND" {
            before_data = false;
        }
        offset = offset
            .checked_add(length)
            .and_then(|n| n.checked_add(12))
            .ok_or_else(unsupported)?;
        if offset > bytes.len() {
            return Err(unsupported());
        }
        if kind == b"IEND" {
            break;
        }
    }
    let mut decoder = ::png::Decoder::new(Cursor::new(bytes));
    // The library's64MiB default is not an application production limit.
    decoder.set_limits(::png::Limits {
        bytes: isize::MAX as usize,
    });
    decoder.set_transformations(::png::Transformations::IDENTITY);
    let mut reader = decoder.read_info().map_err(|_| unsupported())?;
    let info = reader.info();
    if info.bit_depth != ::png::BitDepth::Eight || info.interlaced {
        return Err(ImageDecodeError::UnsupportedCapability(
            "PNG depth/interlace native producer profile",
        ));
    }
    let width = info.width;
    let height = info.height;
    // png0.18.1 normalizes RGB8 Info.trns from6 to3 bytes. Retain the actual
    // encoded key instead, including its high bytes, for the RGB producer.
    let transparency = if info.color_type == ::png::ColorType::Rgb {
        raw_transparency.map(<[u8]>::to_vec)
    } else {
        info.trns.as_deref().map(<[u8]>::to_vec)
    };
    let palette = info.palette.as_deref().map(|bytes| {
        bytes
            .chunks_exact(3)
            .map(|rgb| [rgb[0], rgb[1], rgb[2]])
            .collect::<Vec<_>>()
    });
    let (format, components) = match info.color_type {
        ::png::ColorType::Rgb => (
            if has_transparency {
                BitmapFormat::Argb32
            } else {
                BitmapFormat::Rgb24
            },
            3,
        ),
        ::png::ColorType::Rgba => (BitmapFormat::Argb32, 4),
        ::png::ColorType::Indexed => (
            if has_transparency {
                BitmapFormat::Argb32
            } else {
                BitmapFormat::Indexed8
            },
            1,
        ),
        _ => {
            return Err(ImageDecodeError::UnsupportedCapability(
                "PNG grayscale native producer profile",
            ));
        }
    };
    let mut pixels = zeroed(reader.output_buffer_size().ok_or_else(unsupported)?)?;
    let output = reader.next_frame(&mut pixels).map_err(|_| unsupported())?;
    pixels.truncate(output.buffer_size());
    bitmap(
        width,
        height,
        format,
        &pixels,
        components,
        palette.as_deref(),
        transparency.as_deref(),
    )
}
fn le32(bytes: &[u8], offset: usize) -> Result<u32, ImageDecodeError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(unsupported)?
            .try_into()
            .map_err(|_| unsupported())?,
    ))
}
fn bmp(bytes: &[u8]) -> Result<Bitmap, ImageDecodeError> {
    let dib = le32(bytes, 14)?;
    let bits = u16::from_le_bytes(
        bytes
            .get(28..30)
            .ok_or_else(unsupported)?
            .try_into()
            .map_err(|_| unsupported())?,
    );
    let compression = le32(bytes, 30)?;
    let format = match (dib, bits, compression) {
        (40, 24, 0) => BitmapFormat::Rgb24,
        (40, 8, 0) => BitmapFormat::Indexed8,
        (108, 32, 3)
            if le32(bytes, 54)? == 0x00ff0000
                && le32(bytes, 58)? == 0x0000ff00
                && le32(bytes, 62)? == 0x000000ff
                && le32(bytes, 66)? == 0xff000000
                && le32(bytes, 70)? == 0x73524742 =>
        {
            BitmapFormat::Rgb32
        }
        _ => {
            return Err(ImageDecodeError::UnsupportedCapability(
                "BMP header/mask/compression native producer profile",
            ));
        }
    };
    let mut decoder =
        ::image::codecs::bmp::BmpDecoder::new(Cursor::new(bytes)).map_err(|_| unsupported())?;
    if format == BitmapFormat::Indexed8 {
        decoder.set_indexed_color(true);
    }
    let palette = decoder.get_palette().map(<[[u8; 3]]>::to_vec);
    let (width, height) = decoder.dimensions();
    let components = usize::from(decoder.color_type().channel_count());
    if (format == BitmapFormat::Indexed8 && components != 1)
        || (format == BitmapFormat::Rgb24 && components != 3)
        || (format == BitmapFormat::Rgb32 && components != 4)
    {
        return Err(unsupported());
    }
    let mut pixels =
        zeroed(usize::try_from(decoder.total_bytes()).map_err(|_| ImageDecodeError::Allocation)?)?;
    decoder.read_image(&mut pixels).map_err(|_| unsupported())?;
    // Wine decode_image_bmp demotes successful ARGB metadata to RGB32 without
    // changing WIC's fourth bytes. Existing raw converter reads those bytes.
    bitmap(
        width,
        height,
        format,
        &pixels,
        components,
        palette.as_deref(),
        None,
    )
}

#[cfg(test)]
mod laws {
    use super::{bitmap, decode_bitmap};
    use crate::{BitmapFormat, ImageMap};

    #[test]
    fn subsampled_jpeg_matches_original_bitmap_for_both_scan_layouts() {
        use sha2::{Digest, Sha256};

        // System.Drawing.Bitmap on the Genetica reference profile, in RGB row order.
        let expected = "c77c11e4b015a2f13697b4fd7fdffa51f30fbf5532548a0da4a9697eaca334cd";
        let samples: [&[u8]; 2] = [
            include_bytes!("../../tests/fixtures/imported_image/chroma-baseline.jpg"),
            include_bytes!("../../tests/fixtures/imported_image/chroma-progressive.jpg"),
        ];
        for sample in samples {
            let image = decode_bitmap(sample).unwrap();
            assert_eq!(image.layout().format(), BitmapFormat::Rgb24);
            assert_eq!(image.layout().width(), 19);
            assert_eq!(image.layout().height(), 13);
            let mut digest = Sha256::new();
            for row in 0..13 {
                for column in 0..19 {
                    let argb = image.argb_pixel(column, row).unwrap().to_be_bytes();
                    digest.update(&argb[1..]);
                }
            }
            assert_eq!(format!("{:x}", digest.finalize()), expected);
        }
    }

    #[test]
    fn rgb_png_transparency_uses_the_raw_key_and_retains_nonmatching_alpha() {
        let pixels = [17, 43, 91, 7, 53, 199];
        let key = [0, 17, 0, 43, 0, 91];
        let mut encoded = Vec::new();
        {
            let mut encoder = ::png::Encoder::new(&mut encoded, 2, 1);
            encoder.set_color(::png::ColorType::Rgb);
            encoder.set_depth(::png::BitDepth::Eight);
            encoder.set_trns(key.as_slice());
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&pixels).unwrap();
        }
        let decoded = decode_bitmap(&encoded).unwrap();
        assert_eq!(decoded.layout().format(), BitmapFormat::Argb32);
        assert_eq!(decoded.argb_pixel(0, 0).unwrap() >> 24, 0);
        assert_eq!(decoded.argb_pixel(1, 0).unwrap() >> 24, 255);
        let mut map = ImageMap::from_bitmap(Some(&decoded), false).unwrap();
        assert_eq!(map.channel(3, true).unwrap().values(), vec![1.0, 0.0]);
        // A16bit key outside the8bit sample range must not match merely by
        // dropping the high byte, regardless of library Info normalization.
        let high = [1, 17, 0, 43, 0, 91];
        let nonmatching =
            bitmap(2, 1, BitmapFormat::Argb32, &pixels, 3, None, Some(&high)).unwrap();
        assert_eq!(nonmatching.argb_pixel(0, 0).unwrap() >> 24, 255);
    }
}
