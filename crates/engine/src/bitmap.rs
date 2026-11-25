//! Portable caller-owned Bitmap storage and the original ImageMap conversion.
//!
//! This models characterized pixel storage, not universal native Bitmap
//! construction, lock failures, codec decoding or GDI error recovery.
use crate::{ByteBuffer, FloatBuffer, ImageError, ImageGeometry, ImageMap};
use std::{
    error::Error,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitmapFormat {
    Rgb24,
    Argb32,
    Rgb32,
    PArgb32,
    Indexed8,
}
impl BitmapFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb24 => 3,
            Self::Argb32 | Self::Rgb32 | Self::PArgb32 => 4,
            Self::Indexed8 => 1,
        }
    }
    pub const fn source_pixel_format(self) -> i32 {
        match self {
            Self::Rgb24 => 137224,
            Self::Argb32 => 2498570,
            Self::Rgb32 => 139273,
            Self::PArgb32 => 925707,
            Self::Indexed8 => 198659,
        }
    }
    const fn is_raw_conversion(self) -> bool {
        matches!(self, Self::Rgb24 | Self::Argb32 | Self::Rgb32)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitmapError {
    NullBitmap,
    Image(ImageError),
    InvalidLayout,
    InvalidPalette,
    MissingPalette,
    PixelIndex,
    DisposedBitmap,
    Allocation,
    UnsupportedCapability(&'static str),
}
impl BitmapError {
    /// Layout, palette and post-disposal native failures were not captured.
    /// Keep those Rust capability boundaries separate from actual CLR classes.
    pub const fn source_class(self) -> Option<&'static str> {
        match self {
            Self::NullBitmap => Some("NullReferenceException"),
            Self::Image(error) => Some(error.source_class()),
            _ => None,
        }
    }
}
impl From<ImageError> for BitmapError {
    fn from(value: ImageError) -> Self {
        Self::Image(value)
    }
}
impl fmt::Display for BitmapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for BitmapError {}

/// Caller layout configuration. Bitmap/view construction validates the complete
/// signed row span against its actual storage; fields cannot bypass that check.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BitmapLayout {
    width: i32,
    height: i32,
    stride: i32,
    scan0_offset: usize,
    format: BitmapFormat,
}
impl BitmapLayout {
    pub fn new(width: i32, height: i32, format: BitmapFormat) -> Result<Self, BitmapError> {
        if width <= 0 || height <= 0 {
            return Err(BitmapError::InvalidLayout);
        }
        let row = i64::from(width) * format.bytes_per_pixel() as i64;
        let aligned = (row + 3) & !3;
        let stride = i32::try_from(aligned).map_err(|_| BitmapError::InvalidLayout)?;
        Ok(Self {
            width,
            height,
            stride,
            scan0_offset: 0,
            format,
        })
    }
    pub const fn with_stride(mut self, stride: i32) -> Self {
        self.stride = stride;
        self
    }
    pub const fn with_scan0_offset(mut self, offset: usize) -> Self {
        self.scan0_offset = offset;
        self
    }
    pub const fn width(self) -> i32 {
        self.width
    }
    pub const fn height(self) -> i32 {
        self.height
    }
    pub const fn stride(self) -> i32 {
        self.stride
    }
    pub const fn scan0_offset(self) -> usize {
        self.scan0_offset
    }
    pub const fn format(self) -> BitmapFormat {
        self.format
    }

    fn validate(self, length: usize) -> Result<(), BitmapError> {
        let row = i128::from(self.width) * self.format.bytes_per_pixel() as i128;
        let pitch = i128::from(self.stride).abs();
        if self.width <= 0 || self.height <= 0 || pitch < row || pitch % 4 != 0 {
            return Err(BitmapError::InvalidLayout);
        }
        let last_row = i128::from(self.height - 1) * i128::from(self.stride);
        if last_row < i128::from(i32::MIN) || last_row > i128::from(i32::MAX) {
            return Err(BitmapError::UnsupportedCapability(
                "native signed row product overflow",
            ));
        }
        let first = self.scan0_offset as i128;
        let last = first + last_row;
        if first.min(last) < 0 || first.max(last) + row > length as i128 {
            return Err(BitmapError::InvalidLayout);
        }
        Ok(())
    }
    fn pixel_offset(self, x: i32, y: i32) -> Result<usize, BitmapError> {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return Err(BitmapError::PixelIndex);
        }
        let at = self.scan0_offset as i128
            + i128::from(y) * i128::from(self.stride)
            + i128::from(x) * self.format.bytes_per_pixel() as i128;
        usize::try_from(at).map_err(|_| BitmapError::InvalidLayout)
    }
}

struct Storage {
    layout: BitmapLayout,
    bytes: ByteBuffer,
    palette: Mutex<Option<[u32; 256]>>,
    disposed: AtomicBool,
}
/// One portable Bitmap root retaining its actual fixed-length caller buffer.
/// Clones share disposal and palette state; disposing never frees caller bytes.
#[derive(Clone)]
pub struct Bitmap(Arc<Storage>);
impl fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bitmap")
            .field("layout", &self.layout())
            .field("disposed", &self.is_disposed())
            .field("root", &Arc::as_ptr(&self.0))
            .finish()
    }
}
impl Bitmap {
    pub fn from_buffer(layout: BitmapLayout, bytes: ByteBuffer) -> Result<Self, BitmapError> {
        layout.validate(bytes.len())?;
        Ok(Self(Arc::new(Storage {
            layout,
            bytes,
            palette: Mutex::new(None),
            disposed: AtomicBool::new(false),
        })))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn layout(&self) -> BitmapLayout {
        self.0.layout
    }
    pub fn bytes(&self) -> ByteBuffer {
        self.0.bytes.clone()
    }
    pub fn is_disposed(&self) -> bool {
        self.0.disposed.load(Ordering::Acquire)
    }
    pub fn dispose(&self) {
        self.0.disposed.store(true, Ordering::Release);
    }
    pub fn set_palette(&self, argb: &[u32]) -> Result<(), BitmapError> {
        self.ensure_live()?;
        validate_palette(self.layout().format(), argb)?;
        let mut values = [0; 256];
        values.copy_from_slice(argb);
        *self
            .0
            .palette
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(values);
        Ok(())
    }
    pub fn argb_pixel(&self, x: i32, y: i32) -> Result<u32, BitmapError> {
        logical_pixel(self, x, y)
    }
    fn ensure_live(&self) -> Result<(), BitmapError> {
        if self.is_disposed() {
            Err(BitmapError::DisposedBitmap)
        } else {
            Ok(())
        }
    }
}

/// A safe borrowed view into caller pixel and optional palette storage. Signed
/// stride is preserved; no native pointer or source output is accepted here.
#[derive(Clone, Copy, Debug)]
pub struct BitmapView<'a> {
    layout: BitmapLayout,
    bytes: &'a [u8],
    palette: Option<&'a [u32]>,
}
impl<'a> BitmapView<'a> {
    pub fn new(layout: BitmapLayout, bytes: &'a [u8]) -> Result<Self, BitmapError> {
        layout.validate(bytes.len())?;
        Ok(Self {
            layout,
            bytes,
            palette: None,
        })
    }
    pub fn with_palette(mut self, argb: &'a [u32]) -> Result<Self, BitmapError> {
        validate_palette(self.layout.format(), argb)?;
        self.palette = Some(argb);
        Ok(self)
    }
    pub const fn layout(&self) -> BitmapLayout {
        self.layout
    }
    pub fn argb_pixel(&self, x: i32, y: i32) -> Result<u32, BitmapError> {
        logical_pixel(self, x, y)
    }
}
fn validate_palette(format: BitmapFormat, values: &[u32]) -> Result<(), BitmapError> {
    if format != BitmapFormat::Indexed8 || values.len() != 256 {
        return Err(BitmapError::InvalidPalette);
    }
    Ok(())
}
trait PixelSource {
    fn layout(&self) -> BitmapLayout;
    fn ensure_live(&self) -> Result<(), BitmapError>;
    fn byte(&self, at: usize) -> Result<u8, BitmapError>;
    fn with_bytes<T>(&self, read: impl FnOnce(&[u8]) -> T) -> T;
    fn palette_color(&self, ix: usize) -> Result<u32, BitmapError>;
}
impl PixelSource for Bitmap {
    fn layout(&self) -> BitmapLayout {
        self.layout()
    }
    fn ensure_live(&self) -> Result<(), BitmapError> {
        self.ensure_live()
    }
    fn byte(&self, at: usize) -> Result<u8, BitmapError> {
        self.0
            .bytes
            .read(at)
            .map_err(|_| BitmapError::InvalidLayout)
    }
    fn with_bytes<T>(&self, read: impl FnOnce(&[u8]) -> T) -> T {
        // Read all planes from one buffer generation, including signed rows.
        read(&self.0.bytes.generation())
    }
    fn palette_color(&self, ix: usize) -> Result<u32, BitmapError> {
        self.0
            .palette
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|values| values[ix])
            .ok_or(BitmapError::MissingPalette)
    }
}
impl PixelSource for BitmapView<'_> {
    fn layout(&self) -> BitmapLayout {
        self.layout
    }
    fn ensure_live(&self) -> Result<(), BitmapError> {
        Ok(())
    }
    fn byte(&self, at: usize) -> Result<u8, BitmapError> {
        self.bytes
            .get(at)
            .copied()
            .ok_or(BitmapError::InvalidLayout)
    }
    fn with_bytes<T>(&self, read: impl FnOnce(&[u8]) -> T) -> T {
        read(self.bytes)
    }
    fn palette_color(&self, ix: usize) -> Result<u32, BitmapError> {
        self.palette
            .map(|values| values[ix])
            .ok_or(BitmapError::MissingPalette)
    }
}
fn logical_pixel(source: &impl PixelSource, x: i32, y: i32) -> Result<u32, BitmapError> {
    source.ensure_live()?;
    let layout = source.layout();
    let at = layout.pixel_offset(x, y)?;
    if layout.format() == BitmapFormat::Indexed8 {
        return source.palette_color(usize::from(source.byte(at)?));
    }
    let blue = source.byte(at)?;
    let green = source.byte(at + 1)?;
    let red = source.byte(at + 2)?;
    let alpha = match layout.format() {
        BitmapFormat::Rgb24 | BitmapFormat::Rgb32 => 255,
        BitmapFormat::Argb32 => source.byte(at + 3)?,
        BitmapFormat::PArgb32 => return Ok(pargb_argb([blue, green, red, source.byte(at + 3)?])),
        BitmapFormat::Indexed8 => unreachable!("indexed pixel handled before components"),
    };
    Ok(u32::from_le_bytes([blue, green, red, alpha]))
}

/// Wine11.19 commit455e3509, image.c264-281 and inspected local GetPixel
/// 0xbca5/0xbcaf/0xbcf5: exact 15-bit quotient, alpha0 hidden RGB, c>a saturation.
/// The earlier capture's actual loaded native DLL identity was not recorded.
/// This primitive describes that qualified portable profile, not all GDI builds.
pub(crate) fn pargb_argb(mut bgra: [u8; 4]) -> u32 {
    let alpha = bgra[3];
    if alpha != 0 {
        let quotient = (255_u32 << 15) / u32::from(alpha);
        for component in &mut bgra[..3] {
            *component = if *component > alpha {
                255
            } else {
                ((u32::from(*component) * quotient) >> 15) as u8
            };
        }
    }
    u32::from_le_bytes(bgra)
}

impl ImageMap {
    /// Convert the actual portable Bitmap producer through the characterized
    /// raw/fallback source paths. Native lock/constructor failures are separate.
    pub fn from_bitmap(bitmap: Option<&Bitmap>, red_only: bool) -> Result<Self, BitmapError> {
        convert(bitmap.ok_or(BitmapError::NullBitmap)?, red_only)
    }
    pub fn from_bitmap_view(
        bitmap: Option<&BitmapView<'_>>,
        red_only: bool,
    ) -> Result<Self, BitmapError> {
        convert(bitmap.ok_or(BitmapError::NullBitmap)?, red_only)
    }
}
fn convert(source: &impl PixelSource, red_only: bool) -> Result<ImageMap, BitmapError> {
    source.ensure_live()?;
    let layout = source.layout();
    if layout.format().is_raw_conversion() {
        return source.with_bytes(|bytes| convert_raw(layout, bytes, red_only));
    }
    let mut output = ImageMap::new(ImageGeometry::new(layout.width(), layout.height()));
    let materialized = if red_only { 1 } else { 3 };
    let mut planes: [Option<FloatBuffer>; 3] = std::array::from_fn(|_| None);
    for (ix, plane) in planes.iter_mut().enumerate().take(materialized) {
        *plane = Some(output.materialize(ix as i32)?);
    }
    for y in 0..layout.height() {
        for x in 0..layout.width() {
            let pixel = (i64::from(y) * i64::from(layout.width()) + i64::from(x)) as usize;
            let [blue, green, red, _alpha] = logical_pixel(source, x, y)?.to_le_bytes();
            planes[0]
                .as_ref()
                .expect("materialized red")
                .write(pixel, f32::from(red) / 255.0_f32)?;
            if !red_only {
                planes[1]
                    .as_ref()
                    .expect("materialized green")
                    .write(pixel, f32::from(green) / 255.0_f32)?;
                planes[2]
                    .as_ref()
                    .expect("materialized blue")
                    .write(pixel, f32::from(blue) / 255.0_f32)?;
            }
        }
    }
    for (ix, plane) in planes.iter().enumerate().take(materialized) {
        let plane = plane.as_ref().expect("materialized conversion channel");
        let first = plane.read(0)?;
        let mut all_equal = true;
        for at in 1..plane.len() {
            if plane.read(at)? != first {
                all_equal = false;
                break;
            }
        }
        if all_equal {
            output.set_constant(ix as i32, first)?;
        }
    }
    Ok(output)
}

fn convert_raw(
    layout: BitmapLayout,
    bytes: &[u8],
    red_only: bool,
) -> Result<ImageMap, BitmapError> {
    const FACTOR: f32 = f32::from_bits(0x3b808081);
    let width = layout.width() as usize;
    let components = layout.format().bytes_per_pixel();
    let count = if red_only { 1 } else { components };
    let mut output = ImageMap::new(ImageGeometry::new(layout.width(), layout.height()));
    for (channel, offset) in [2, 1, 0, 3].into_iter().enumerate().take(count) {
        let destination = output.materialize(channel as i32)?;
        let constant = destination.with_mut_slice(|samples| -> Result<_, BitmapError> {
            for (y, row) in samples.chunks_exact_mut(width).enumerate() {
                let start = layout.pixel_offset(0, y as i32)?;
                let pixels = &bytes[start..start + width * components];
                for (sample, pixel) in row.iter_mut().zip(pixels.chunks_exact(components)) {
                    let value = f32::from(pixel[offset]) * FACTOR;
                    *sample = if offset == 3 { 1.0 - value } else { value };
                }
            }
            let first = samples.first().copied().ok_or(ImageError::Index)?;
            Ok(samples.iter().all(|&value| value == first).then_some(first))
        })?;
        if let Some(value) = constant {
            output.set_constant(channel as i32, value)?;
        }
    }
    Ok(output)
}

#[cfg(test)]
#[path = "bitmap_test.rs"]
mod test;
