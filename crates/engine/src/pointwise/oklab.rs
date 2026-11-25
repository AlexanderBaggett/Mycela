//! Mycela's Oklab color source. Connected red channels replace scalar controls.
use super::{PointwiseError, PointwiseFilter, SharedImageMap, check_render_cancelled};
use crate::{FloatBuffer, ImageError, ImageGeometry};

const CHROMA_LIMIT: f64 = 0.4;
const BLOCK: usize = 256;

fn map_value(value: f32, chromatic: bool) -> f64 {
    let value = f64::from(value);
    if chromatic {
        (value - 0.5) * (2.0 * CHROMA_LIMIT)
    } else {
        value
    }
}

enum Component {
    Constant(f64),
    Map(FloatBuffer),
}

impl Component {
    fn sample(&self, index: usize, chromatic: bool) -> Result<f64, ImageError> {
        let (minimum, maximum) = if chromatic {
            (-CHROMA_LIMIT, CHROMA_LIMIT)
        } else {
            (0.0, 1.0)
        };
        let value = match self {
            Self::Constant(value) => *value,
            Self::Map(values) => map_value(values.read(index)?, chromatic),
        };
        Ok(if value.is_nan() {
            0.0
        } else {
            value.clamp(minimum, maximum)
        })
    }

    fn is_constant(&self) -> bool {
        matches!(self, Self::Constant(_))
    }
}

fn srgb(lightness: f64, a: f64, b: f64) -> [f32; 3] {
    // Björn Ottosson's D65 Oklab inverse, https://bottosson.github.io/posts/oklab/.
    let l = (lightness + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m = (lightness - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s = (lightness - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ]
    // ImageMap stores the encoded RGB samples consumed by the bitmap paths.
    .map(|linear| {
        let linear = linear.clamp(0.0, 1.0);
        if linear <= 0.0031308 {
            (12.92 * linear) as f32
        } else {
            (1.055 * linear.powf(1.0 / 2.4) - 0.055) as f32
        }
    })
}

pub(super) fn render<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    check_render_cancelled(cancelled)?;
    if geometry.width() <= 0 || geometry.height() <= 0 || inputs.len() > 4 {
        return Err(ImageError::Argument.into());
    }
    let pixels = geometry
        .width()
        .checked_mul(geometry.height())
        .ok_or(ImageError::Overflow)? as usize;
    let component = |index: usize| -> Result<Component, PointwiseError<E>> {
        let Some(map) = inputs.get(index).and_then(Option::as_ref) else {
            return Ok(Component::Constant(
                filter.parameter(index as i32)?.effective_value_at(time)?,
            ));
        };
        let red = map.channel(0, true)?;
        if red.len() == 1 {
            return Ok(Component::Constant(map_value(
                red.read(0)?,
                matches!(index, 1 | 2),
            )));
        }
        if map.geometry() != geometry || red.len() != pixels {
            return Err(ImageError::Argument.into());
        }
        Ok(Component::Map(red))
    };
    let [lightness, a, b, opacity] = [component(0)?, component(1)?, component(2)?, component(3)?];
    let color = |index| {
        Ok::<_, ImageError>(srgb(
            lightness.sample(index, false)?,
            a.sample(index, true)?,
            b.sample(index, true)?,
        ))
    };
    let transparency = |index| Ok::<_, ImageError>((1.0 - opacity.sample(index, false)?) as f32);
    let output = SharedImageMap::new(geometry);

    if lightness.is_constant() && a.is_constant() && b.is_constant() {
        for (channel, value) in color(0)?.into_iter().enumerate() {
            output.set_constant(channel as i32, value)?;
        }
    } else {
        let planes = [
            output.materialize(0)?,
            output.materialize(1)?,
            output.materialize(2)?,
        ];
        let mut samples = [[0.0; BLOCK]; 3];
        for start in (0..pixels).step_by(BLOCK) {
            check_render_cancelled(cancelled)?;
            let count = (pixels - start).min(BLOCK);
            for offset in 0..count {
                for (block, value) in samples.iter_mut().zip(color(start + offset)?) {
                    block[offset] = value;
                }
            }
            for (plane, block) in planes.iter().zip(samples.iter()) {
                plane.write_block(start, 1, &block[..count])?;
            }
        }
    }
    if opacity.is_constant() {
        output.set_constant(3, transparency(0)?)?;
    } else {
        let plane = output.materialize(3)?;
        let mut samples = [0.0; BLOCK];
        for start in (0..pixels).step_by(BLOCK) {
            check_render_cancelled(cancelled)?;
            let count = (pixels - start).min(BLOCK);
            for (offset, sample) in samples[..count].iter_mut().enumerate() {
                *sample = transparency(start + offset)?;
            }
            plane.write_block(start, 1, &samples[..count])?;
        }
    }
    Ok(output)
}

#[cfg(test)]
#[path = "oklab_test.rs"]
mod test;
