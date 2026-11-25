//! The embedded ij image source and the ic/uw environment projection path.
use crate::pointwise::{PointwiseError, convert_environment, image_resize};
use crate::{
    BinaryCursor, FloatBuffer, ImageDecodeError, ImageError, ImageGeometry, ImageMap,
    ImportedImageDecodeError, ProceduralExecution, SharedImageMap, decode_hdr, decode_pfm,
    decode_tga, image_decode,
};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EnvironmentImageProjection {
    #[default]
    LatitudeLongitude,
    LightProbe,
    MirroredBall,
    HorizontalCross,
    VerticalCross,
}
impl EnvironmentImageProjection {
    pub const ALL: [Self; 5] = [
        Self::LatitudeLongitude,
        Self::LightProbe,
        Self::MirroredBall,
        Self::HorizontalCross,
        Self::VerticalCross,
    ];
    pub const fn label(self) -> &'static str {
        match self {
            Self::LatitudeLongitude => "Latitude/Longitude",
            Self::LightProbe => "Light Probe (Angular)",
            Self::MirroredBall => "Mirrored Ball",
            Self::HorizontalCross => "Horizontal Cross",
            Self::VerticalCross => "Vertical Cross",
        }
    }

    /// Standalone map dimensions, including complete cube faces for cross projections.
    pub fn output_geometry(self, width: i32) -> Option<ImageGeometry> {
        let height = match self {
            Self::LatitudeLongitude => width / 2,
            Self::LightProbe | Self::MirroredBall => width,
            Self::HorizontalCross if width % 4 == 0 => width / 4 * 3,
            Self::VerticalCross if width % 3 == 0 => (width / 3).checked_mul(4)?,
            Self::HorizontalCross | Self::VerticalCross => return None,
        };
        (width > 0 && height > 0 && width.checked_mul(height).is_some())
            .then(|| ImageGeometry::new(width, height))
    }

    pub(super) fn source_geometry(self, width: i32) -> Option<ImageGeometry> {
        self.output_geometry(width)?;
        use EnvironmentImageProjection::*;
        let half = match self {
            LatitudeLongitude => width / 2,
            LightProbe | MirroredBall => (f64::from(width).powi(2) / 2.5464).sqrt().round() as i32,
            HorizontalCross => (f64::from(width / 4) * 3.0_f64.sqrt()).round() as i32,
            VerticalCross => (f64::from(width / 3) * 3.0_f64.sqrt()).round() as i32,
        }
        .max(1);
        let width = if self == LatitudeLongitude {
            width
        } else {
            half.checked_mul(2)?
        };
        width.checked_mul(half)?;
        Some(ImageGeometry::new(width, half))
    }

    /// Choose the target width at roughly the same spherical sampling density.
    pub fn width_for_projection(self, width: i32, target: Self) -> Option<i32> {
        use EnvironmentImageProjection::*;
        let geometry = self.source_geometry(width)?;
        if self == target
            || (matches!(self, LightProbe | MirroredBall)
                && matches!(target, LightProbe | MirroredBall))
        {
            return Some(width);
        }
        let area = f64::from(geometry.area());
        Some(match target {
            LatitudeLongitude => geometry.width(),
            LightProbe | MirroredBall => (area * 1.2732).sqrt().round_ties_even() as i32,
            HorizontalCross => (area / 6.).sqrt().round_ties_even() as i32 * 4,
            VerticalCross => (area / 6.).sqrt().round_ties_even() as i32 * 3,
        })
    }

    pub(super) fn from_wire(projection: i32) -> Option<Self> {
        Self::ALL.get(usize::try_from(projection).ok()?).copied()
    }
}

/// Encoded bytes are saved verbatim. A successful decode is shared across map
/// edits and captured frames. Size-specific resampling stays in the render.
#[derive(Debug)]
pub struct EnvironmentImage {
    format: i32,
    bytes: Vec<u8>,
    decoded: OnceLock<SharedImageMap>,
}

impl EnvironmentImage {
    pub fn from_file_bytes(bytes: Vec<u8>, extension: &str) -> Result<Self, ImageDecodeError> {
        let format = if extension.eq_ignore_ascii_case("tga") {
            1
        } else if extension.eq_ignore_ascii_case("pfm") {
            2
        } else if extension.eq_ignore_ascii_case("hdr") {
            3
        } else {
            0
        };
        let image = Self::from_encoded(format, bytes);
        image.decoded()?;
        Ok(image)
    }

    /// Detect the projection of a newly selected embedded image from its
    /// dimensions. Square images use the supplied preferred disk projection.
    pub fn inferred_projection(
        &self,
        square: EnvironmentImageProjection,
    ) -> Result<EnvironmentImageProjection, ImageDecodeError> {
        use EnvironmentImageProjection::*;
        let geometry = self.decoded()?.geometry();
        let width = i64::from(geometry.width());
        let height = i64::from(geometry.height());
        Ok(if width == height {
            if square == LightProbe {
                LightProbe
            } else {
                MirroredBall
            }
        } else if width * 3 == height * 4 {
            HorizontalCross
        } else if width * 4 == height * 3 {
            VerticalCross
        } else {
            LatitudeLongitude
        })
    }

    pub(super) fn from_encoded(format: i32, bytes: Vec<u8>) -> Self {
        Self {
            format,
            bytes,
            decoded: OnceLock::new(),
        }
    }

    pub(super) fn encoded(&self) -> (i32, &[u8]) {
        (self.format, &self.bytes)
    }

    pub(super) fn decoded(&self) -> Result<&SharedImageMap, ImageDecodeError> {
        if let Some(decoded) = self.decoded.get() {
            return Ok(decoded);
        }
        let image = match self.format {
            0 => {
                let bitmap = if self.bytes.starts_with(&[0xff, 0xd8]) {
                    image_decode::decode_jpeg(&self.bytes)?
                } else {
                    image_decode::decode_bitmap(&self.bytes)?
                };
                let image = ImageMap::from_bitmap(Some(&bitmap), false);
                bitmap.dispose();
                image?
            }
            1 => decode_tga(&mut BinaryCursor::new(&self.bytes)?)?,
            2 => decode_pfm(&mut BinaryCursor::new(&self.bytes)?)?,
            3 => decode_hdr(&mut BinaryCursor::new(&self.bytes)?)?,
            _ => return Err(ImageDecodeError::Format),
        };
        let image = SharedImageMap::from_image(image);
        if image.geometry().width() <= 0 || image.geometry().height() <= 0 {
            return Err(ImageDecodeError::Format);
        }
        image.seal_permanently()?;
        Ok(self.decoded.get_or_init(|| image))
    }

    pub(super) fn render<E>(
        &self,
        geometry: ImageGeometry,
        projection: i32,
        tone: [f64; 2],
        execution: ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let source = self.decoded().map_err(|cause| {
            PointwiseError::ImportedImage(ImportedImageDecodeError::Decode(cause))
        })?;
        let image = self.fit_width(source, geometry.width(), execution, cancelled)?;
        Self::project_source(image, geometry, projection, tone, execution, cancelled)
    }

    pub(super) fn project_source<E>(
        mut image: SharedImageMap,
        geometry: ImageGeometry,
        projection: i32,
        tone: [f64; 2],
        execution: ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let exponent = 1. / tone[0];
        let exposure = 2.0f64.powf(tone[1]) as f32;
        if exponent != 1. || exposure != 1. {
            let toned = SharedImageMap::new(image.geometry());
            for channel in 0..3 {
                if cancelled() {
                    return Err(PointwiseError::Cancelled);
                }
                let mut plane = image.channel(channel, true)?.values();
                for value in &mut plane {
                    *value = super::render::signed_gamma(*value * exposure, exponent);
                }
                toned.set_buffer(channel, Some(&FloatBuffer::new(plane)))?;
            }
            toned.set_buffer(3, Some(&image.channel(3, true)?))?;
            image = toned;
        }
        let format = match projection {
            0 => {
                return image_resize::resize_clamped(
                    &image,
                    geometry.width(),
                    geometry.height(),
                    execution,
                );
            }
            1..=4 => projection,
            // abf selects its mirrored-ball worker for other noncross values.
            _ => 2,
        };
        let width = image
            .geometry()
            .width()
            .saturating_mul(2)
            .min(geometry.width());
        let latlong = convert_environment::project_to_latlong(
            &image,
            format,
            ImageGeometry::new(width, (width / 2).max(1)),
            cancelled,
        )?;
        image_resize::resize_clamped(&latlong, geometry.width(), geometry.height(), execution)
    }

    pub(super) fn fit_width<E>(
        &self,
        original: &SharedImageMap,
        width: i32,
        execution: ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let sw = original.geometry().width();
        let sh = original.geometry().height();
        if sw <= width {
            return Ok(original.clone());
        }
        if self.format != 0 && i64::from(sw) == 2 * i64::from(sh) {
            return image_resize::resize_clamped(original, width, (width / 2).max(1), execution);
        }
        let height = if self.format == 0 {
            (width as f32 * sh as f32 / sw as f32).round_ties_even() as i32
        } else {
            (f64::from(width) * f64::from(sh) / f64::from(sw)).round_ties_even() as i32
        };
        if self.format == 0 {
            return resize_bitmap(original, width, height.max(1), execution, cancelled);
        }
        image_resize::resize(original, width, height.max(1), execution)
    }
}

// ImageMap.r averages the byte components over the source footprint before
// normalizing. The first tap uses the starting fraction, and the last uses the
// fractional scale. Each source scheduler chunk computes its first row
// position, then advances in single precision for the rest of that chunk.
fn resize_bitmap<E>(
    source: &SharedImageMap,
    width: i32,
    height: i32,
    execution: ProceduralExecution,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let sw = source.geometry().width();
    let sh = source.geometry().height();
    let area = usize::try_from(width.checked_mul(height).ok_or(ImageError::Overflow)?)
        .map_err(|_| ImageError::Overflow)?;
    let mut planes = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for plane in &mut planes {
        plane
            .try_reserve_exact(area)
            .map_err(|_| ImageError::Allocation)?;
    }
    let input = [
        source.channel(0, true)?.values(),
        source.channel(1, true)?.values(),
        source.channel(2, true)?.values(),
        source.channel(3, true)?.values(),
    ];
    let step_x = sw as f32 / width as f32;
    let step_y = sh as f32 / height as f32;
    crate::procedural_schedule::for_each_row_chunk(height, execution, |start, count| {
        let mut y = start as f32 * step_y;
        for _ in 0..count {
            if cancelled() {
                return Err(PointwiseError::Cancelled);
            }
            let mut x = 0.;
            for _ in 0..width {
                let mut sum = [0.; 4];
                let mut weight_sum = 0.;
                for dy in 0..step_y.ceil() as i32 {
                    let row = (y as i32 + dy).min(sh - 1);
                    let wy = edge_weight(step_y, y, dy);
                    for dx in 0..step_x.ceil() as i32 {
                        let col = (x as i32 + dx).min(sw - 1);
                        let index = row as usize * sw as usize + col as usize;
                        let weight = wy * edge_weight(step_x, x, dx);
                        for (value, channel) in sum.iter_mut().zip(&input) {
                            let sample = channel[if channel.len() == 1 { 0 } else { index }];
                            *value += (sample * 255.).round() * weight;
                        }
                        weight_sum += weight;
                    }
                }
                let scale = 1. / (weight_sum * 255.);
                for (output, value) in planes.iter_mut().zip(sum) {
                    output.push(value * scale);
                }
                x += step_x;
            }
            y += step_y;
        }
        Ok(())
    })?;
    let mut image = ImageMap::new(ImageGeometry::new(width, height));
    for (index, plane) in planes.into_iter().enumerate() {
        image.set_array(index as i32, plane)?;
    }
    Ok(SharedImageMap::from_image(image))
}

fn edge_weight(step: f32, position: f32, tap: i32) -> f32 {
    if tap == 0 {
        1. - position.fract()
    } else if tap == step as i32 && step.fract() >= 0.0001 {
        step.fract()
    } else {
        1.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn bitmap_resize_preserves_the_source_start_of_each_row_range() {
        let file = include_bytes!("../tests/fixtures/imported_image/chroma-baseline.jpg");
        let image = EnvironmentImage::from_file_bytes(file.to_vec(), "jpg").unwrap();
        let original = image.decoded().unwrap();
        let cases = [
            (
                ProceduralExecution::CallingThread,
                "6d1a6de57f53a9e4956f64cfe07480c965977883ac9f933e5cdadbd0a574a7dc",
            ),
            (
                ProceduralExecution::WorkerPool {
                    processor_count: 16,
                },
                "9bb63f923e9185136226ff2cd4b47ab1762936bf69ad4a137c9e871438d1c34f",
            ),
        ];
        for (execution, expected) in cases {
            let actual = image
                .fit_width::<()>(original, 13, execution, &|| false)
                .unwrap();
            assert_eq!(actual.geometry(), ImageGeometry::new(13, 9));
            let mut digest = Sha256::new();
            for channel in 0..4 {
                let values = actual.materialize(channel).unwrap().values();
                digest.update((values.len() as u32).to_le_bytes());
                for value in values {
                    digest.update(value.to_le_bytes());
                }
            }
            assert_eq!(format!("{:x}", digest.finalize()), expected);
        }
    }
}
