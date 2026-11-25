//! Original ImageMap preview conversion over retained portable bitmap roots.
//!
//! RGB24/ARGB32 logical rows are the pixel boundary, independent of native GDI
//! stride/padding, GPU transport and composition. Imported target rows are
//! caller inputs; importing them does not implement native construction/Clear.
//! PArgb32 logical pixels use the separately characterized Wine11.19 portable
//! profile; this does not prove a native DLL load or other GDI implementations.
//! Extra channel layouts, malformed planes, native fallback/OOM retries,
//! disposal/locking and concurrent mutation remain explicitly uncharacterized.
use crate::{FloatBuffer, ImageError, ImageMap};
use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewBitmapFormat {
    Rgb24,
    Argb32,
    PArgb32,
}
impl PreviewBitmapFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb24 => 3,
            Self::Argb32 | Self::PArgb32 => 4,
        }
    }
    pub const fn source_pixel_format(self) -> i32 {
        match self {
            Self::Rgb24 => 137224,
            Self::Argb32 => 2498570,
            Self::PArgb32 => 925707,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewBitmapOptions {
    half_size: bool,
    dither: bool,
}
impl Default for PreviewBitmapOptions {
    fn default() -> Self {
        Self {
            half_size: false,
            dither: true,
        }
    }
}
impl PreviewBitmapOptions {
    pub const fn with_half_size(mut self, value: bool) -> Self {
        self.half_size = value;
        self
    }
    pub const fn with_dither(mut self, value: bool) -> Self {
        self.dither = value;
        self
    }
    pub const fn is_half_size(self) -> bool {
        self.half_size
    }
    pub const fn is_dither_enabled(self) -> bool {
        self.dither
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewBitmapError {
    NullImage,
    Image(ImageError),
    InvalidGeometry,
    Allocation,
    InvalidTargetRows,
    PixelIndex,
    PArgbPixelUnavailable,
    UncharacterizedChannelLayout {
        channels: usize,
    },
    UncharacterizedPlaneLayout {
        channel: usize,
        length: usize,
        required: usize,
    },
}
impl PreviewBitmapError {
    /// Only characterized source error classes are mapped. Capability gaps are
    /// explicit failures, not invented original exceptions or blank success.
    pub const fn source_class(self) -> Option<&'static str> {
        match self {
            Self::NullImage => Some("NullReferenceException"),
            Self::Image(error) => Some(error.source_class()),
            Self::InvalidGeometry => Some("ArgumentException"),
            _ => None,
        }
    }
}
impl From<ImageError> for PreviewBitmapError {
    fn from(value: ImageError) -> Self {
        Self::Image(value)
    }
}
impl fmt::Display for PreviewBitmapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for PreviewBitmapError {}
struct Storage {
    width: i32,
    height: i32,
    format: PreviewBitmapFormat,
    bytes: Mutex<Vec<u8>>,
}
/// A retained bitmap root. Cloning shares in-place writes and never copies on
/// write. Width/height/format are fixed; caller snapshots own copied byte rows.
#[derive(Clone)]
pub struct PreviewBitmap(Arc<Storage>);
impl fmt::Debug for PreviewBitmap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreviewBitmap")
            .field("width", &self.width())
            .field("height", &self.height())
            .field("format", &self.format())
            .field("root", &Arc::as_ptr(&self.0))
            .finish()
    }
}
impl PreviewBitmap {
    fn blank(
        width: i32,
        height: i32,
        format: PreviewBitmapFormat,
    ) -> Result<Self, PreviewBitmapError> {
        let length = storage_length(width, height, format)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| PreviewBitmapError::Allocation)?;
        bytes.resize(length, 0);
        Ok(Self(Arc::new(Storage {
            width,
            height,
            format,
            bytes: Mutex::new(bytes),
        })))
    }
    /// Import actual caller target rows. Storage construction does not emulate
    /// native stride, padding or Clear; logical PArgb reads use the qualified
    /// shared portable pixel primitive.
    pub fn from_rows(
        width: i32,
        height: i32,
        format: PreviewBitmapFormat,
        rows: &[Vec<u8>],
    ) -> Result<Self, PreviewBitmapError> {
        let length = storage_length(width, height, format)?;
        let row_length = length / height as usize;
        if rows.len() != height as usize || rows.iter().any(|row| row.len() != row_length) {
            return Err(PreviewBitmapError::InvalidTargetRows);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| PreviewBitmapError::Allocation)?;
        for row in rows {
            bytes.extend_from_slice(row);
        }
        Ok(Self(Arc::new(Storage {
            width,
            height,
            format,
            bytes: Mutex::new(bytes),
        })))
    }
    pub fn width(&self) -> i32 {
        self.0.width
    }
    pub fn height(&self) -> i32 {
        self.0.height
    }
    pub fn format(&self) -> PreviewBitmapFormat {
        self.0.format
    }
    pub fn shares_storage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn rows(&self) -> Result<Vec<Vec<u8>>, PreviewBitmapError> {
        let row_length = self.width() as usize * self.format().bytes_per_pixel();
        let mut rows = Vec::new();
        rows.try_reserve_exact(self.height() as usize)
            .map_err(|_| PreviewBitmapError::Allocation)?;
        let bytes = self.0.bytes.lock().unwrap_or_else(|e| e.into_inner());
        for row in bytes.chunks_exact(row_length) {
            let mut copy = Vec::new();
            copy.try_reserve_exact(row_length)
                .map_err(|_| PreviewBitmapError::Allocation)?;
            copy.extend_from_slice(row);
            rows.push(copy);
        }
        Ok(rows)
    }
    /// Read ARGB from actual target bytes. PArgb uses the same qualified
    /// Wine11.19 15-bit producer as portable Bitmap, rather than an idealized
    /// floating unpremultiply. Other native profiles remain unproved.
    pub fn argb_pixel(&self, x: i32, y: i32) -> Result<u32, PreviewBitmapError> {
        if x < 0 || y < 0 || x >= self.width() || y >= self.height() {
            return Err(PreviewBitmapError::PixelIndex);
        }
        let bpp = self.format().bytes_per_pixel();
        let at = (y as usize * self.width() as usize + x as usize) * bpp;
        let bytes = self.0.bytes.lock().unwrap_or_else(|e| e.into_inner());
        let alpha = if bpp == 4 { bytes[at + 3] } else { 255 };
        if self.format() == PreviewBitmapFormat::PArgb32 {
            return Ok(crate::bitmap::pargb_argb([
                bytes[at],
                bytes[at + 1],
                bytes[at + 2],
                alpha,
            ]));
        }
        Ok(u32::from_le_bytes([
            bytes[at],
            bytes[at + 1],
            bytes[at + 2],
            alpha,
        ]))
    }
    fn scalar_fill(&self, mut rgba: [u8; 4]) {
        // Native original Clear loses hidden RGB at scalar fill alpha zero.
        // Dense writes later overwrite their own channels, including under
        // zero alpha; there is no final premultiplication/zero-RGB pass.
        if rgba[3] == 0 {
            rgba[..3].fill(0);
        }
        let bgra = [rgba[2], rgba[1], rgba[0], rgba[3]];
        let bpp = self.format().bytes_per_pixel();
        let mut bytes = self.0.bytes.lock().unwrap_or_else(|e| e.into_inner());
        for pixel in bytes.chunks_exact_mut(bpp) {
            pixel.copy_from_slice(&bgra[..bpp]);
        }
    }
    fn set_byte(&self, pixel: usize, channel: usize, value: u8) {
        let at = pixel * self.format().bytes_per_pixel() + channel;
        self.0.bytes.lock().unwrap_or_else(|e| e.into_inner())[at] = value;
    }
}
fn storage_length(
    width: i32,
    height: i32,
    format: PreviewBitmapFormat,
) -> Result<usize, PreviewBitmapError> {
    if width <= 0 || height <= 0 {
        return Err(PreviewBitmapError::InvalidGeometry);
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|v| v.checked_mul(format.bytes_per_pixel()))
        .ok_or(PreviewBitmapError::Allocation)
}
/// Original scalar helper: directed comparisons and separate f32 multiply/add.
pub fn preview_scalar_byte(value: f32) -> u8 {
    if value > 1.0 {
        255
    } else if value > 0.0 {
        let scaled = value * 255.0;
        let rounded = scaled + 0.5;
        rounded as u8
    } else {
        0
    }
}
enum Plane {
    Source(FloatBuffer),
    Alpha(Vec<f32>),
}
impl Plane {
    fn len(&self) -> usize {
        match self {
            Self::Source(v) => v.len(),
            Self::Alpha(v) => v.len(),
        }
    }
    fn read(&self, at: usize) -> Result<f32, PreviewBitmapError> {
        match self {
            Self::Source(v) => Ok(v.read(at)?),
            Self::Alpha(v) => v
                .get(at)
                .copied()
                .ok_or(PreviewBitmapError::Image(ImageError::Index)),
        }
    }
}
/// Convert the existing live ImageMap. Source preparation commits/read order
/// and retained target identities are preserved. Uncharacterized extra-channel
/// or malformed-plane contexts fail explicitly; no native-error fallback or
/// allocation retry is fabricated. Concurrent source/target mutation is unproved.
pub fn preview_bitmap(
    image: Option<&mut ImageMap>,
    options: PreviewBitmapOptions,
    target: Option<&PreviewBitmap>,
) -> Result<PreviewBitmap, PreviewBitmapError> {
    let image = image.ok_or(PreviewBitmapError::NullImage)?;
    let geometry = image.geometry();
    let mut width = geometry.width();
    let mut height = geometry.height();
    if options.half_size {
        width /= 2;
        height /= 2;
    }
    let mut channels = image.channel_count()?.max(3);
    let matching_geometry = target.is_some_and(|v| v.width() == width && v.height() == height);
    if channels == 3
        && matching_geometry
        && target.is_some_and(|v| v.format() == PreviewBitmapFormat::Argb32)
    {
        channels = 4;
    }
    if channels > 4 {
        return Err(PreviewBitmapError::UncharacterizedChannelLayout { channels });
    }
    let mut planes = Vec::new();
    planes
        .try_reserve_exact(channels)
        .map_err(|_| PreviewBitmapError::Allocation)?;
    if channels == 3 {
        for role in 0..3 {
            planes.push(Plane::Source(image.channel(role, true)?));
        }
    } else {
        let source_alpha = image.channel(3, true)?;
        let mut alpha = Vec::new();
        alpha
            .try_reserve_exact(source_alpha.len())
            .map_err(|_| PreviewBitmapError::Allocation)?;
        for at in 0..source_alpha.len() {
            alpha.push(source_alpha.read(at)?);
        }
        for role in 0..3 {
            planes.push(Plane::Source(image.channel(role, true)?));
        }
        for value in &mut alpha {
            *value = 1.0 - *value;
        }
        planes.insert(0, Plane::Alpha(alpha));
    }
    let format = if channels == 4 {
        PreviewBitmapFormat::Argb32
    } else {
        PreviewBitmapFormat::Rgb24
    };
    let bitmap = if matching_geometry && target.is_some_and(|v| v.format() == format) {
        target.expect("matching target").clone()
    } else {
        PreviewBitmap::blank(width, height, format)?
    };
    if planes.iter().any(|v| v.len() == 1) {
        let mut rgba = [0; 4];
        for role in 0..3 {
            if !image.uses_plane(role)? {
                rgba[role as usize] = preview_scalar_byte(image.channel(role, true)?.read(0)?);
            }
        }
        rgba[3] = if image.uses_plane(3)? {
            255
        } else {
            preview_scalar_byte(1.0 - image.channel(3, true)?.read(0)?)
        };
        bitmap.scalar_fill(rgba);
    }
    if planes.iter().all(|v| v.len() == 1) {
        return Ok(bitmap);
    }
    let input_width = geometry.width() as usize;
    let required = (geometry.width() as usize)
        .checked_mul(geometry.height() as usize)
        .ok_or(PreviewBitmapError::Allocation)?;
    for (conversion_channel, plane) in planes.iter().enumerate() {
        if plane.len() == 1 {
            continue;
        }
        if plane.len() != required {
            return Err(PreviewBitmapError::UncharacterizedPlaneLayout {
                channel: conversion_channel,
                length: plane.len(),
                required,
            });
        }
        let rng = DitherRng::new(conversion_channel as u8);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let mut value = if options.half_size {
                    let at = y * 2 * input_width + x * 2;
                    let first = plane.read(at)? + plane.read(at + 1)?;
                    let second = first + plane.read(at + input_width)?;
                    let third = second + plane.read(at + input_width + 1)?;
                    third * 0.25
                } else {
                    plane.read(y * input_width + x)?
                };
                if options.dither {
                    let base = dotnet_i32(value * 255.0) as f32 * DITHER_STEP;
                    let residual = value - base;
                    let coordinate = if options.half_size {
                        x
                    } else {
                        y * input_width + x
                    };
                    let threshold = rng.sample(coordinate as i32, y as i32) * DITHER_STEP;
                    if residual > threshold {
                        value += DITHER_STEP;
                    }
                }
                let byte = if options.half_size {
                    half_byte(value)
                } else if value > 1.0 {
                    255
                } else if value > 0.0 {
                    (value * 255.0) as u8
                } else {
                    0
                };
                bitmap.set_byte(
                    y * width as usize + x,
                    channels - 1 - conversion_channel,
                    byte,
                );
            }
        }
    }
    Ok(bitmap)
}
// Original j.bx keeps these ordered comparisons, including the unordered
// branch, before its byte conversion. Limit the lint exception to this helper.
#[allow(clippy::manual_clamp)]
fn half_byte(mut value: f32) -> u8 {
    if value > 1.0 {
        value = 1.0;
    } else if value < 0.0 {
        value = 0.0;
    }
    (value * 255.0) as u8
}
const DITHER_STEP: f32 = 0.003_921_569;
fn dotnet_i32(value: f32) -> i32 {
    // Private conv.i4 compatibility choice for this preview leaf. Captures
    // prove final converter pixels, not the nonfinite/out-of-range i32
    // intermediate. MIN is not a proved generic Framework cast result.
    if !value.is_finite() || !(-2_147_483_648.0..2_147_483_648.0).contains(&value) {
        i32::MIN
    } else {
        value as i32
    }
}
// Both preview and live Noise use the same concrete seeded state authority.
use crate::seeded_random::SeededRandom;
struct DitherRng {
    samples: [f32; 256],
}
impl DitherRng {
    fn new(seed: u8) -> Self {
        let mut source = SeededRandom::for_preview_channel(seed);
        let mut samples = [0.0; 256];
        for value in &mut samples {
            *value = source.sample() as f32;
        }
        Self { samples }
    }
    fn sample(&self, x: i32, y: i32) -> f32 {
        let row = PERMUTATION[(y & 255) as usize];
        self.samples[usize::from(PERMUTATION[(x.wrapping_add(i32::from(row)) & 255) as usize])]
    }
}
// Same original ahr FieldRVA authority used by all procedural lattices.
use crate::procedural_lattice::PERMUTATION;

#[cfg(test)]
mod rng_parity {
    use super::{DitherRng, PERMUTATION, SeededRandom};
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    const CAPTURE: &str =
        include_str!("../../../../port/evidence/preview-dither-rng-rng-r1-original.jsonl");
    fn state(actual: &SeededRandom, probe: &Value) {
        let expected = &probe["value"];
        assert_eq!(
            actual.indices().0 as u64,
            expected["index_a"].as_u64().expect("source index")
        );
        assert_eq!(
            actual.indices().1 as u64,
            expected["index_b"].as_u64().expect("source index")
        );
        let words: Vec<_> = actual
            .state_words()
            .iter()
            .map(|v| format!("{:08x}", *v as u32))
            .collect();
        assert_eq!(
            words,
            expected["words"]
                .as_array()
                .expect("source state")
                .iter()
                .map(|v| v.as_str().expect("word").to_owned())
                .collect::<Vec<_>>()
        );
    }
    fn sample(actual: f64, expected: &Value) {
        assert_eq!(
            format!("{:016x}", actual.to_bits()),
            expected["bits"].as_str().expect("actual original sample")
        );
    }
    fn ahr(actual: &DitherRng, probe: &Value) {
        let words: Vec<_> = actual
            .samples
            .iter()
            .map(|v| format!("{:08x}", v.to_bits()))
            .collect();
        assert_eq!(
            words,
            probe["instance"]["value"]["words"]
                .as_array()
                .expect("actual samples")
                .iter()
                .map(|v| v.as_str().expect("word").to_owned())
                .collect::<Vec<_>>()
        );
        let table: Vec<_> = probe["table"]["value"]["values"]
            .as_array()
            .expect("original table")
            .iter()
            .map(|v| v.as_u64().expect("integer") as u8)
            .collect();
        assert_eq!(PERMUTATION.as_slice(), table);
    }
    #[test]
    fn actual_original_12_rng_cases_4528_public_samples_and_retained_state() {
        assert_eq!(
            format!("{:x}", Sha256::digest(CAPTURE.as_bytes())),
            "eca3cccac1b8bc74b54916e352d6ce4ec5e4da0c55ad33e34f269a25b3436af9"
        );
        let mut cases = 0;
        let mut e_calls = 0;
        let mut lookups = 0;
        for line in CAPTURE.lines() {
            let row: Value = serde_json::from_str(line).expect("source JSON");
            if row["kind"] != "rng" {
                continue;
            }
            let recipe = &row["recipe"];
            let seed = recipe["seed"].as_u64().expect("input seed");
            assert!(seed < 4);
            let seed = seed as u8;
            let outputs = row["outputs"]
                .as_array()
                .expect("actual original output samples");
            match recipe["kind"].as_str().expect("input kind") {
                "awm-public-e-stream" => {
                    let mut rng = SeededRandom::for_preview_channel(seed);
                    state(&rng, &row["initial"]);
                    let snapshots = row["snapshots"].as_array().expect("source checkpoints");
                    for checkpoint in snapshots.iter().filter(|v| v["after_calls"] == 0) {
                        state(&rng, &checkpoint["state"]);
                    }
                    let count = recipe["count"].as_u64().expect("input count") as usize;
                    assert_eq!(outputs.len(), count);
                    for (ix, output) in outputs.iter().enumerate() {
                        assert_eq!(output["call"].as_u64(), Some((ix + 1) as u64));
                        sample(rng.sample(), output);
                        e_calls += 1;
                        for checkpoint in snapshots
                            .iter()
                            .filter(|v| v["after_calls"].as_u64() == Some((ix + 1) as u64))
                        {
                            state(&rng, &checkpoint["state"]);
                        }
                    }
                    state(&rng, &row["final"]);
                    assert_eq!(
                        row["initial"]["value"]["state_root"],
                        row["final"]["value"]["state_root"]
                    );
                }
                "awm-same-seed-independent-pair" => {
                    let mut left = SeededRandom::for_preview_channel(seed);
                    let mut right = SeededRandom::for_preview_channel(seed);
                    assert!(!std::ptr::eq(
                        left.state_words().as_ptr(),
                        right.state_words().as_ptr()
                    ));
                    state(&left, &row["initial"]["left"]);
                    state(&right, &row["initial"]["right"]);
                    assert_ne!(
                        row["initial"]["left"]["value"]["state_root"],
                        row["initial"]["right"]["value"]["state_root"]
                    );
                    let mut at = 0;
                    for step in recipe["sequence"]
                        .as_array()
                        .expect("original input sequence")
                    {
                        let receiver = step["receiver"].as_str().expect("input receiver");
                        let count = step["count"].as_u64().expect("input calls") as usize;
                        for call in 1..=count {
                            let output = &outputs[at];
                            assert_eq!(output["receiver"], receiver);
                            assert_eq!(output["call"].as_u64(), Some(call as u64));
                            sample(
                                if receiver == "left" {
                                    left.sample()
                                } else {
                                    assert_eq!(receiver, "right");
                                    right.sample()
                                },
                                output,
                            );
                            at += 1;
                            e_calls += 1;
                        }
                        for snapshot in row["snapshots"]
                            .as_array()
                            .expect("snapshots")
                            .iter()
                            .filter(|s| s["after"] == format!("{receiver}{count}"))
                        {
                            state(&left, &snapshot["state"]["left"]);
                            state(&right, &snapshot["state"]["right"]);
                        }
                    }
                    assert_eq!(outputs.len(), at);
                    state(&left, &row["final"]["left"]);
                    state(&right, &row["final"]["right"]);
                }
                "ahr-public-2d" => {
                    let rng = DitherRng::new(seed);
                    ahr(&rng, &row["initial"]);
                    // Query coordinates are generated from input recipes,
                    // never from output lookup values or captured sample arrays.
                    let mut coordinates = Vec::new();
                    for query in recipe["query_recipe"].as_array().expect("input queries") {
                        match query["kind"].as_str().expect("input query kind") {
                            "full-row" => {
                                for x in query["x_start"].as_i64().expect("x")
                                    ..=query["x_end_inclusive"].as_i64().expect("end")
                                {
                                    coordinates
                                        .push((x as i32, query["y"].as_i64().expect("y") as i32));
                                }
                            }
                            "ordered-cartesian" => {
                                for x in query["x_values"].as_array().expect("x inputs") {
                                    for y in query["y_values"].as_array().expect("y inputs") {
                                        coordinates.push((
                                            x.as_i64().expect("x") as i32,
                                            y.as_i64().expect("y") as i32,
                                        ));
                                    }
                                }
                            }
                            _ => panic!("unhandled input query"),
                        }
                    }
                    assert_eq!(
                        coordinates.len() as u64,
                        recipe["queries_per_epoch"].as_u64().expect("query count")
                    );
                    let epochs = recipe["epochs"].as_u64().expect("epochs") as usize;
                    assert_eq!(outputs.len(), epochs * coordinates.len());
                    for epoch in 0..epochs {
                        for (ix, (x, y)) in coordinates.iter().enumerate() {
                            let output = &outputs[epoch * coordinates.len() + ix];
                            assert_eq!(output["x"].as_i64(), Some(i64::from(*x)));
                            assert_eq!(output["y"].as_i64(), Some(i64::from(*y)));
                            assert_eq!(
                                format!("{:08x}", rng.sample(*x, *y).to_bits()),
                                output["bits"].as_str().expect("actual original lookup")
                            );
                            lookups += 1;
                        }
                    }
                    for snapshot in row["snapshots"]
                        .as_array()
                        .expect("actual source snapshots")
                    {
                        ahr(&rng, &snapshot["state"]);
                    }
                    ahr(&rng, &row["final"]);
                    assert_eq!(
                        row["initial"]["instance"]["value"]["samples_root"],
                        row["final"]["instance"]["value"]["samples_root"]
                    );
                }
                _ => panic!("unhandled RNG input kind"),
            }
            cases += 1;
        }
        assert_eq!((cases, e_calls, lookups), (12, 1680, 2848));
    }
}
