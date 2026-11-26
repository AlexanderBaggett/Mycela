//! Image encoders shared by disk exports.
use crate::solid_color::{
    map_preview::{PreviewSelection, project_bitmap},
    preview_export::PreviewPixels,
};
use image::{
    DynamicImage, ImageFormat, Rgb, RgbaImage,
    codecs::{hdr::HdrEncoder, jpeg::JpegEncoder},
};
use mycela_engine::SharedImageMap;
use std::io::{Seek, Write};

fn failure(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Format {
    Png,
    Jpeg,
    Tga,
    Tiff,
    Bmp,
    Pfm,
    Hdr,
}

impl Format {
    pub(crate) const ALL: [Self; 7] = [
        Self::Png,
        Self::Jpeg,
        Self::Tga,
        Self::Tiff,
        Self::Bmp,
        Self::Pfm,
        Self::Hdr,
    ];

    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Tga => "tga",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Pfm => "pfm",
            Self::Hdr => "hdr",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Tga => "Targa",
            Self::Tiff => "TIFF",
            Self::Bmp => "BMP",
            Self::Pfm => "PFM",
            Self::Hdr => "Radiance HDR",
        }
    }
}

pub(crate) fn write_standard(
    map: &SharedImageMap,
    format: Format,
    writer: &mut (impl Write + Seek),
    jpeg_quality: u8,
) -> Result<(), String> {
    let bitmap = project_bitmap(map, &PreviewSelection::BaseColor).map_err(failure)?;
    let pixels = PreviewPixels::capture_rendered(&bitmap)?.into_rgba();
    let image = DynamicImage::ImageRgba8(
        RgbaImage::from_raw(bitmap.width() as u32, bitmap.height() as u32, pixels)
            .ok_or("The image dimensions are invalid.")?,
    );
    let codec = match format {
        Format::Png => ImageFormat::Png,
        Format::Jpeg => {
            return JpegEncoder::new_with_quality(writer, jpeg_quality)
                .encode_image(&image.into_rgb8())
                .map_err(failure);
        }
        Format::Tga => ImageFormat::Tga,
        Format::Tiff => ImageFormat::Tiff,
        Format::Bmp => ImageFormat::Bmp,
        Format::Pfm | Format::Hdr => unreachable!("floating-point writer"),
    };
    image.write_to(writer, codec).map_err(failure)
}

pub(crate) fn write_hdr(
    map: &SharedImageMap,
    format: Format,
    writer: &mut impl Write,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let geometry = map.geometry();
    let (width, height) = (geometry.width() as usize, geometry.height() as usize);
    let channels =
        [0, 1, 2].map(|channel| map.channel(channel, true).map(|buffer| buffer.values()));
    let [red, green, blue] = channels;
    let planes = [
        red.map_err(failure)?,
        green.map_err(failure)?,
        blue.map_err(failure)?,
    ];
    let rgb = |index| {
        planes
            .each_ref()
            .map(|plane| plane[if plane.len() == 1 { 0 } else { index }])
    };
    if matches!(format, Format::Pfm) {
        write!(writer, "PF\n{width} {height}\n-1.000000\n").map_err(failure)?;
        for y in (0..height).rev() {
            if cancelled() {
                return Ok(());
            }
            for x in 0..width {
                for channel in rgb(y * width + x) {
                    writer.write_all(&channel.to_le_bytes()).map_err(failure)?;
                }
            }
        }
        return Ok(());
    }
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(width * height).map_err(failure)?;
    for y in 0..height {
        if cancelled() {
            return Ok(());
        }
        pixels.extend((0..width).map(|x| Rgb(rgb(y * width + x).map(|value| value.max(0.)))));
    }
    HdrEncoder::new(writer)
        .encode(&pixels, width, height)
        .map_err(failure)
}
