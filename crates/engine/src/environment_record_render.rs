//! Native latitude–longitude raster for the Environment value (rh).
use super::blend::{Contrast, Mixer, light_axis, number, soft_light};
use super::{
    EnvironmentBlendMode, EnvironmentImage, EnvironmentImageProjection, EnvironmentLayer,
    EnvironmentRecord, EnvironmentResourceLibrary, ImageAsset, LayerSource, Transform,
};
use crate::pointwise::{
    PointwiseError, convert_environment, convert_environment_cross,
    environment_blur::{EnvironmentBlur, blur_map},
    image_resize,
    rotate_environment::rotate_map,
};
use crate::{
    FloatBuffer, GradientCache, GradientError, HslColor, ImageError, ImageGeometry, ImageMap,
    ModernPropertyRecord, ModernPropertyValue, ProceduralExecution, RgbColor, SharedImageMap,
};
use std::f64::consts::{FRAC_PI_2, PI};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentRenderSettings {
    pub width: i32,
    pub projection: EnvironmentImageProjection,
    pub source_width_limit: Option<i32>,
    /// Heading, pitch, and roll in degrees, applied before projection.
    pub rotation: [f64; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentDisplayTone {
    pub exposure: f64,
    pub gamma: f64,
}

/// A display-only lookup from output pixels to a latitude/longitude map.
pub struct EnvironmentPreviewConverter {
    geometry: ImageGeometry,
    coordinates: Vec<[u8; 2]>,
}

impl EnvironmentPreviewConverter {
    /// Read normalized horizontal and vertical coordinates from RGB8 pixels.
    pub fn from_rgb8(width: i32, height: i32, rgb: &[u8]) -> Result<Self, ImageError> {
        let area = width.checked_mul(height).ok_or(ImageError::Overflow)?;
        if width <= 0 || height <= 0 || (area as usize).checked_mul(3) != Some(rgb.len()) {
            return Err(ImageError::Argument);
        }
        let mut coordinates = Vec::new();
        coordinates
            .try_reserve_exact(area as usize)
            .map_err(|_| ImageError::Allocation)?;
        coordinates.extend(rgb.chunks_exact(3).map(|pixel| [pixel[0], pixel[1]]));
        Ok(Self {
            geometry: ImageGeometry::new(width, height),
            coordinates,
        })
    }

    pub fn project<E>(
        &self,
        source: &SharedImageMap,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let width = source.geometry().width();
        let height = source.geometry().height();
        if width <= 0 || height <= 0 {
            return Err(PointwiseError::Argument);
        }
        let area = width.checked_mul(height).ok_or(ImageError::Overflow)? as usize;
        let source = Raster::read(source)?;
        if source
            .0
            .iter()
            .any(|plane| plane.len() != 1 && plane.len() != area)
        {
            return Err(PointwiseError::Argument);
        }
        let nearest = |value, last: i32| {
            ((f32::from(value) / 255. * last as f32 + 0.5) as usize).min(last as usize)
        };
        rasterize(self.geometry, true, false, cancelled, |_, index| {
            let [u, v] = self.coordinates[index];
            let index = nearest(v, height - 1) * width as usize + nearest(u, width - 1);
            Ok(source.sample(index))
        })
    }
}

/// An immutable layer snapshot for the map editor's isolated preview.
#[derive(Clone)]
pub struct EnvironmentLayerPreview(Arc<EnvironmentLayer>);

impl EnvironmentLayerPreview {
    pub fn same_layer(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Render the selected layer on a 168 × 168 mirrored sphere.
    pub fn render_sphere<E>(
        &self,
        execution: ProceduralExecution,
        library: &EnvironmentResourceLibrary,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        const SIDE: i32 = 168;
        let geometry = ImageGeometry::new(SIDE * 2, SIDE);
        let layer = Layer::prepare(&self.0, geometry, execution, Some(library), cancelled)?;
        let needs_direction = layer.source.needs_direction();
        let dense = needs_direction || layer.source.needs_grid();
        let native = rasterize(
            geometry,
            dense,
            needs_direction,
            cancelled,
            |direction, index| layer.color_sample(direction, index),
        )?;
        // The layer editor shows a Normal blend over its checker when the
        // selected layer has transparency, uses the mixer, or has low strength.
        let native = if layer.mixer.is_some()
            || layer.strength < 1.
            || native.channel(3, true)?.uniform_value() != Some(0.)
        {
            let raster = Raster::read(&native)?;
            rasterize(
                geometry,
                true,
                layer.mixer.is_some(),
                cancelled,
                |direction, index| {
                    let [r, g, b, transparency] = raster.sample(index);
                    let transparency = layer.mixer.as_ref().map_or(transparency, |mixer| {
                        mixer.transparency(transparency, direction)
                    });
                    let x = index % geometry.width() as usize;
                    let y = index / geometry.width() as usize;
                    let gray = if (x / 8 + y / 8) & 1 == 0 { 192. } else { 240. } / 255.;
                    let mut pixel = [gray, gray, gray, 0.];
                    EnvironmentBlendMode::Normal.composite(
                        [r, g, b],
                        &mut pixel,
                        (1. - transparency) * layer.strength,
                    );
                    Ok(pixel)
                },
            )?
        } else {
            native
        };
        EnvironmentRecord::project_latlong(
            &native,
            SIDE,
            EnvironmentImageProjection::MirroredBall,
            cancelled,
        )
    }
}

impl EnvironmentDisplayTone {
    /// Tone the displayed raster without changing the projected export pixels.
    pub fn apply<E>(
        self,
        projected: &SharedImageMap,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        if !self.exposure.is_finite() || !self.gamma.is_finite() || self.gamma <= 0. {
            return Err(PointwiseError::Argument);
        }
        if self.exposure == 0. && self.gamma == 1. {
            return Ok(projected.clone());
        }
        let scale = 2.0_f64.powf(self.exposure) as f32;
        let gamma = f64::from(1.0_f32 / self.gamma as f32);
        let output = SharedImageMap::new(projected.geometry());
        for channel in 0..3 {
            if cancelled() {
                return Err(PointwiseError::Cancelled);
            }
            let mut colors = projected.channel(channel, true)?.values();
            for color in &mut colors {
                *color = signed_gamma(*color * scale, gamma);
            }
            output.set_buffer(channel, Some(&FloatBuffer::new(colors)))?;
        }
        output.set_buffer(3, Some(&projected.channel(3, true)?))?;
        Ok(output)
    }
}

impl EnvironmentRecord {
    pub fn layer_preview(&self, index: usize) -> Option<EnvironmentLayerPreview> {
        self.layers.get(index).cloned().map(EnvironmentLayerPreview)
    }

    /// Render a standalone environment map in the selected projection.
    /// The output width determines the source latitude–longitude sampling density.
    pub fn render_projected<E>(
        &self,
        width: i32,
        projection: EnvironmentImageProjection,
        execution: ProceduralExecution,
        library: Option<&EnvironmentResourceLibrary>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        self.render_projected_with(
            EnvironmentRenderSettings {
                width,
                projection,
                source_width_limit: None,
                rotation: [0.; 3],
            },
            execution,
            library,
            cancelled,
        )
    }

    pub fn render_projected_with<E>(
        &self,
        settings: EnvironmentRenderSettings,
        execution: ProceduralExecution,
        library: Option<&EnvironmentResourceLibrary>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let EnvironmentRenderSettings {
            width,
            projection,
            source_width_limit,
            rotation,
        } = settings;
        let natural = projection
            .source_geometry(width)
            .ok_or(PointwiseError::Argument)?;
        if rotation.iter().any(|angle| !angle.is_finite())
            || source_width_limit.is_some_and(|limit| limit < 2)
        {
            return Err(PointwiseError::Argument);
        }
        if cancelled() {
            return Err(PointwiseError::Cancelled);
        }
        let native_width = natural.width().min(source_width_limit.unwrap_or(i32::MAX));
        let geometry = ImageGeometry::new(native_width, native_width / 2);
        let native = self.render_latlong(geometry, execution, library, cancelled)?;
        let native = if rotation == [0.; 3] {
            native
        } else {
            rotate_map(&native, geometry, rotation, execution, cancelled)?
        };
        if cancelled() {
            return Err(PointwiseError::Cancelled);
        }
        if projection == EnvironmentImageProjection::LatitudeLongitude && native_width != width {
            return image_resize::resize_clamped(&native, width, width / 2, execution);
        }
        Self::project_latlong(&native, width, projection, cancelled)
    }

    pub fn render_preset_thumbnail<E>(
        &self,
        execution: ProceduralExecution,
        library: Option<&EnvironmentResourceLibrary>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        const COMPOSITE_WIDTH: i32 = 168;
        let native = self.render_latlong(
            ImageGeometry::new(COMPOSITE_WIDTH * 2, COMPOSITE_WIDTH),
            execution,
            library,
            cancelled,
        )?;
        let pixels = Raster::read(&native)?;
        let geometry = ImageGeometry::new(COMPOSITE_WIDTH, COMPOSITE_WIDTH);
        // Genetica's standard blend consumes the initial flat range of the
        // 336 × 168 cached map into the 168 × 168 checkerboard.
        let image = rasterize(geometry, true, false, cancelled, |_, index| {
            let x = index % COMPOSITE_WIDTH as usize;
            let y = index / COMPOSITE_WIDTH as usize;
            let shade = if (x / 8 + y / 8) & 1 == 0 { 192. } else { 240. } / 255.;
            let [red, green, blue, transparency] = pixels.sample(index);
            let composite = |color| color * (1. - transparency) + shade * transparency;
            Ok([composite(red), composite(green), composite(blue), 0.])
        })?;
        Self::project_latlong(
            &image,
            110,
            EnvironmentImageProjection::MirroredBall,
            cancelled,
        )
    }

    fn project_latlong<E>(
        native: &SharedImageMap,
        width: i32,
        projection: EnvironmentImageProjection,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        match projection {
            EnvironmentImageProjection::LatitudeLongitude => Ok(native.clone()),
            EnvironmentImageProjection::LightProbe | EnvironmentImageProjection::MirroredBall => {
                convert_environment::from_latlong(native, projection as i32, width, cancelled)
            }
            EnvironmentImageProjection::HorizontalCross
            | EnvironmentImageProjection::VerticalCross => {
                convert_environment_cross::from_latlong(native, projection as i32, width, cancelled)
            }
        }
    }

    pub(crate) fn render_latlong<E>(
        &self,
        geometry: ImageGeometry,
        execution: ProceduralExecution,
        library: Option<&EnvironmentResourceLibrary>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let layers = self
            .layers
            .iter()
            .rev()
            .map(|layer| Layer::prepare(layer, geometry, execution, library, cancelled))
            .collect::<Result<Vec<_>, _>>()?;
        let needs_direction = layers
            .iter()
            .any(|layer| layer.mixer.is_some() || layer.source.needs_direction());
        let dense = needs_direction || layers.iter().any(|layer| layer.source.needs_grid());
        let map = rasterize(
            geometry,
            dense,
            needs_direction,
            cancelled,
            |direction, index| sample(&layers, direction, index),
        )?;
        // rh compacts the composite with ImageMap.i() before its transform.
        // Frequency treats a dense zero plane differently from a scalar zero.
        for channel in 0..4 {
            if cancelled() {
                return Err(PointwiseError::Cancelled);
            }
            let samples = map.channel(channel, true)?;
            if samples.len() > 1
                && let Some(value) = samples.uniform_value()
            {
                map.set_constant(channel, value)?;
            }
        }
        let map = self.transform.apply(map, execution, cancelled)?;
        let exposure = 2.0f64.powf(self.exposure()) as f32;
        let gamma = 1. / number(&self.master[1]);
        if exposure != 1. || gamma != 1. {
            for channel in 0..3 {
                let mut colors = map.channel(channel, true)?.values();
                for color in &mut colors {
                    *color = signed_gamma(*color * exposure, gamma);
                }
                if colors.len() == 1 {
                    map.set_constant(channel, colors[0])?;
                } else {
                    map.set_buffer(channel, Some(&FloatBuffer::new(colors)))?;
                }
            }
        }
        Ok(map)
    }
}

fn rasterize<E>(
    geometry: ImageGeometry,
    dense: bool,
    needs_direction: bool,
    cancelled: &dyn Fn() -> bool,
    mut sample: impl FnMut([f64; 3], usize) -> Result<[f32; 4], GradientError>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let mut image = ImageMap::new(geometry);
    if !dense {
        let pixel = sample([0.; 3], 0)?;
        for (channel, value) in pixel.into_iter().enumerate() {
            image.set_constant(channel as i32, value)?;
        }
        return Ok(SharedImageMap::from_image(image));
    }

    let width = geometry.width() as usize;
    let height = geometry.height() as usize;
    let mut meridians = Vec::new();
    if needs_direction {
        meridians
            .try_reserve_exact(width)
            .map_err(|_| ImageError::Allocation)?;
        let dx = 2.0 / width as f64;
        meridians.extend((0..width).map(|x| ((x as f64 * dx - 1.0) * PI).sin_cos()));
    }
    let area = width.checked_mul(height).ok_or(ImageError::Overflow)?;
    let mut planes: [Vec<f32>; 4] = std::array::from_fn(|_| Vec::new());
    for plane in &mut planes {
        plane
            .try_reserve_exact(area)
            .map_err(|_| ImageError::Allocation)?;
    }
    let dy = 2.0 / height as f64;
    for y in 0..height {
        if cancelled() {
            return Err(PointwiseError::Cancelled);
        }
        let (sin_latitude, cos_latitude) = ((y as f64 * dy - 1.0) * FRAC_PI_2).sin_cos();
        for x in 0..width {
            // abf.f maps the first row to the positive Y pole.
            let direction = meridians
                .get(x)
                .map_or([0.; 3], |&(sin_longitude, cos_longitude)| {
                    [
                        cos_latitude * sin_longitude,
                        -sin_latitude,
                        cos_latitude * cos_longitude,
                    ]
                });
            for (plane, value) in planes.iter_mut().zip(sample(direction, y * width + x)?) {
                plane.push(value);
            }
        }
    }
    for (channel, plane) in planes.into_iter().enumerate() {
        image.set_array(channel as i32, plane)?;
    }
    Ok(SharedImageMap::from_image(image))
}

impl Transform {
    fn angles(&self) -> [f64; 3] {
        std::array::from_fn(|index| number(&self.properties[index]))
    }

    fn active(&self) -> bool {
        selection(&self.properties[3]) != 0 || self.angles() != [0.; 3]
    }

    fn apply<E>(
        &self,
        mut map: SharedImageMap,
        execution: ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let radius = number(&self.properties[4]).max(0.);
        let secondary = number(&self.properties[5]).max(0.);
        let blur = match selection(&self.properties[3]) {
            0 => None,
            1 if radius != 0. => Some(EnvironmentBlur::Axes {
                horizontal: radius,
                vertical: radius,
            }),
            2 if radius != 0. || secondary != 0. => Some(EnvironmentBlur::Scatter {
                radius,
                strength: secondary,
            }),
            3 if radius != 0. || secondary != 0. => Some(EnvironmentBlur::Axes {
                horizontal: radius,
                vertical: secondary,
            }),
            4 if radius != 0. || secondary != 0. => Some(EnvironmentBlur::LegacyAxes {
                horizontal: radius,
                vertical: secondary,
            }),
            1..=4 => None,
            _ => return Err(PointwiseError::Index),
        };
        if let Some(blur) = blur {
            map = blur_map(&map, blur, cancelled)?;
            if cancelled() {
                return Err(PointwiseError::Cancelled);
            }
        }
        let angles = self.angles();
        if angles != [0.; 3] {
            map = rotate_map(&map, map.geometry(), angles, execution, cancelled)?;
            if cancelled() {
                return Err(PointwiseError::Cancelled);
            }
        }
        Ok(map)
    }
}

fn selection(property: &ModernPropertyRecord) -> i32 {
    let ModernPropertyValue::Selected { current, .. } = property.value() else {
        unreachable!("Environment selection");
    };
    *current
}

fn rgb(property: &ModernPropertyRecord) -> [f32; 3] {
    let ModernPropertyValue::Color { argb, .. } = property.value() else {
        unreachable!("Environment color");
    };
    [argb[1] as f32, argb[2] as f32, argb[3] as f32]
}

struct Layer {
    source: Source,
    color: Option<ColorCorrection>,
    blend: EnvironmentBlendMode,
    mixer: Option<Mixer>,
    strength: f32,
    gamma: f64,
}

impl Layer {
    fn prepare<E>(
        layer: &EnvironmentLayer,
        geometry: ImageGeometry,
        execution: ProceduralExecution,
        library: Option<&EnvironmentResourceLibrary>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, PointwiseError<E>> {
        if cancelled() {
            return Err(PointwiseError::Cancelled);
        }
        let blend = EnvironmentBlendMode::read(&layer.blend[0]).ok_or(PointwiseError::Index)?;
        let ModernPropertyValue::Int32 {
            current: strength, ..
        } = layer.blend[1].value()
        else {
            unreachable!("Environment blend Strength");
        };
        let source = match &layer.source {
            LayerSource::Color(color) => Source::Color(rgb(color)),
            LayerSource::Light(properties) => Light::prepare(properties)?.into_source(
                geometry,
                number(&properties[3]),
                number(&properties[4]),
                cancelled,
            )?,
            LayerSource::Image {
                asset,
                projection,
                tone,
            } => match asset.as_deref() {
                None => Source::Empty,
                Some(ImageAsset::Embedded(image)) => {
                    let map = image.render(
                        geometry,
                        *projection,
                        [number(&tone[0]), number(&tone[1])],
                        execution,
                        cancelled,
                    )?;
                    Source::Raster(Box::new(Raster::read(&map)?))
                }
                Some(ImageAsset::Library {
                    path,
                    extension,
                    size,
                    ..
                }) => {
                    let resolved = match library {
                        Some(library) => {
                            library.resolve(path, extension, *size, geometry.width(), execution)?
                        }
                        None => None,
                    };
                    if let Some(source) = resolved {
                        let map = EnvironmentImage::project_source(
                            source,
                            geometry,
                            *projection,
                            [number(&tone[0]), number(&tone[1])],
                            execution,
                            cancelled,
                        )?;
                        Source::Raster(Box::new(Raster::read(&map)?))
                    } else {
                        Source::Empty
                    }
                }
            },
        };
        let source = if layer.mask.active() {
            let needs_direction = source.needs_direction();
            let dense = needs_direction || source.needs_grid();
            let map = rasterize(
                geometry,
                dense,
                needs_direction,
                cancelled,
                |direction, index| Ok(source.sample(direction, index)),
            )?;
            let alpha = match &source {
                Source::Empty => Some(1.),
                Source::Color(_) | Source::Light(_) => Some(0.),
                Source::Raster(raster) => match raster.0[3].as_slice() {
                    [value] => Some(*value),
                    _ => None,
                },
            };
            if let Some(alpha) = alpha {
                map.set_constant(3, alpha)?;
            }
            let map = layer.mask.apply(map, cancelled)?;
            Source::Raster(Box::new(Raster::read(&map)?))
        } else {
            source
        };
        let mut prepared = Self {
            source,
            color: ColorCorrection::prepare(layer)?,
            blend,
            mixer: Mixer::prepare(layer),
            // gw's opacity retains its default 100% when its setter rejects
            // an out-of-range authored strength.
            strength: if (0..=100_000).contains(strength) {
                *strength as f32 / 100.
            } else {
                1.
            },
            gamma: 1.0 / number(&layer.blend[2]),
        };
        if layer.transform.active() {
            let needs_direction = prepared.source.needs_direction();
            let dense = needs_direction || prepared.source.needs_grid();
            let map = rasterize(
                geometry,
                dense,
                needs_direction,
                cancelled,
                |direction, index| prepared.color_sample(direction, index),
            )?;
            let map = layer.transform.apply(map, execution, cancelled)?;
            prepared.source = Source::Raster(Box::new(Raster::read(&map)?));
            prepared.color = None;
        }
        Ok(prepared)
    }

    fn color_sample(&self, direction: [f64; 3], index: usize) -> Result<[f32; 4], GradientError> {
        let sample = self.source.sample(direction, index);
        match &self.color {
            Some(color) => color.apply(sample),
            None => Ok(sample),
        }
    }
}

/// kh applies the layer's color controls before filtering and rotating.
struct ColorCorrection {
    recolor: Option<(GradientCache, bool)>,
    tint: [f32; 3],
    contrast: Contrast,
    hue: f64,
    saturation: f64,
}

impl ColorCorrection {
    fn prepare(layer: &EnvironmentLayer) -> Result<Option<Self>, GradientError> {
        let ModernPropertyValue::Color { argb, grayscale } = layer.color[4].value() else {
            unreachable!("Environment colorize property");
        };
        let tint = RgbColor::from_argb(*argb).grayscale(*grayscale);
        let contrast = number(&layer.color[0]);
        let hue = number(&layer.color[1]);
        let saturation = number(&layer.color[2]);
        if !layer.recolor_enabled()
            && contrast == 0.
            && hue == 0.
            && saturation == 0.
            && tint.rounded_opaque_argb() == -1
        {
            return Ok(None);
        }
        let recolor = if layer.recolor_enabled() {
            let gradient = layer
                .recolor_value()
                .ok_or(GradientError::NullReference)?
                .live()
                .source_clone()?;
            gradient.rebuild()?;
            let cache = gradient.cache().ok_or(GradientError::NullReference)?;
            Some((cache, gradient.has_transparency()?))
        } else {
            None
        };
        Ok(Some(Self {
            recolor,
            tint: rgb(&layer.color[4]),
            contrast: Contrast::new(contrast),
            hue: hue / 360.,
            saturation: saturation / 100.,
        }))
    }

    fn apply(&self, [r, g, b, mut transparency]: [f32; 4]) -> Result<[f32; 4], GradientError> {
        let mut rgb = [r, g, b];
        if let Some((cache, drives_opacity)) = &self.recolor {
            let luma = 0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b);
            let [alpha, red, green, blue] = cache.sample(luma)?.argb();
            rgb = [red as f32, green as f32, blue as f32];
            if *drives_opacity {
                transparency = (1. - alpha) as f32;
            }
        }
        for (value, tint) in rgb.iter_mut().zip(self.tint) {
            *value *= tint;
            if !matches!(self.contrast, Contrast::Identity) {
                *value = self.contrast.apply(*value);
                if *value < 0. {
                    *value = 0.;
                }
            }
        }
        if self.hue != 0. || self.saturation != 0. {
            let color = RgbColor::new(f64::from(rgb[0]), f64::from(rgb[1]), f64::from(rgb[2]), 1.);
            let [mut hue, mut saturation, lightness] = color.to_hsl().components();
            hue += self.hue;
            if hue > 1. {
                hue -= 1.;
            } else if hue < 0. {
                hue += 1.;
            }
            if self.saturation > 0. {
                saturation = self.saturation + (1. - self.saturation) * saturation;
            } else if self.saturation < 0. {
                saturation *= 1. + self.saturation;
            }
            let [_, red, green, blue] = HslColor::new(hue, saturation, lightness).to_rgb().argb();
            rgb = [red as f32, green as f32, blue as f32];
        }
        Ok([rgb[0], rgb[1], rgb[2], transparency])
    }
}

enum Source {
    Empty,
    Color([f32; 3]),
    Light(Light),
    Raster(Box<Raster>),
}

impl Source {
    fn needs_direction(&self) -> bool {
        matches!(self, Self::Light(_))
    }

    fn needs_grid(&self) -> bool {
        matches!(self, Self::Raster(map) if map.0.iter().any(|plane| plane.len() > 1))
    }

    fn sample(&self, direction: [f64; 3], index: usize) -> [f32; 4] {
        match self {
            Self::Empty => [0., 0., 0., 1.],
            Self::Color([r, g, b]) => [*r, *g, *b, 0.],
            Self::Light(light) => {
                let value = light.sample(direction);
                [value, value, value, 0.]
            }
            Self::Raster(image) => image.sample(index),
        }
    }
}

struct Raster([Vec<f32>; 4]);

impl Raster {
    fn read(map: &SharedImageMap) -> Result<Self, ImageError> {
        Ok(Self([
            map.channel(0, true)?.values(),
            map.channel(1, true)?.values(),
            map.channel(2, true)?.values(),
            map.channel(3, true)?.values(),
        ]))
    }

    fn sample(&self, index: usize) -> [f32; 4] {
        self.0
            .each_ref()
            .map(|plane| plane[if plane.len() == 1 { 0 } else { index }])
    }
}

struct Light {
    axis: [f64; 3],
    shape: LightShape,
}

enum LightShape {
    Hard,
    Soft,
    Sky {
        base: f64,
        intensity: f64,
        falloff: f64,
        quadratic: f64,
        extinction: f64,
        normalization: f64,
        scale: f32,
    },
}

impl Light {
    fn prepare<E>(properties: &[ModernPropertyRecord; 5]) -> Result<Self, PointwiseError<E>> {
        let axis = light_axis(number(&properties[1]), number(&properties[2]));
        let shape = match selection(&properties[0]) {
            0 => LightShape::Hard,
            1 => LightShape::Soft,
            kind @ (2 | 3) => {
                let (base, intensity, falloff, quadratic, extinction, minimum) = if kind == 2 {
                    (0.91, 10.0, -3.0, 0.45, -0.32_f64, 0.243_f32)
                } else {
                    (0.526, 5.0, -1.5, 0.0, -0.8_f64, 0.5_f32)
                };
                let angle = (axis[0] * axis[0] + axis[2] * axis[2])
                    .sqrt()
                    .atan2(axis[1]);
                let cosine = angle.cos();
                let atmosphere = 1.0 - extinction.exp();
                let normalization =
                    (base + intensity * (falloff * angle).exp() + quadratic * cosine * cosine)
                        * atmosphere;
                let peak = if kind == 2 { 11.36 } else { 5.526 };
                let scale = (peak * atmosphere / normalization) as f32 * 1.4;
                let scale = (1. / scale) * ((axis[1] as f32).max(0.) * (1.4 - minimum) + minimum);
                LightShape::Sky {
                    base,
                    intensity,
                    falloff,
                    quadratic,
                    extinction,
                    normalization,
                    scale,
                }
            }
            _ => return Err(PointwiseError::Index),
        };
        Ok(Self { axis, shape })
    }

    fn into_source<E>(
        self,
        geometry: ImageGeometry,
        haze: f64,
        depth: f64,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Source, PointwiseError<E>> {
        if haze == 0. || !matches!(self.shape, LightShape::Sky { .. }) {
            return Ok(Source::Light(self));
        }

        let base = rasterize(geometry, true, true, cancelled, |direction, _| {
            let light = self.sample(direction);
            Ok([light, light, light, 0.])
        })?;
        let radius = haze * 40. / 100.;
        let softened = blur_map(
            &base,
            EnvironmentBlur::Axes {
                horizontal: radius,
                vertical: radius,
            },
            cancelled,
        )?;
        let base = Raster::read(&base)?;
        let softened = Raster::read(&softened)?;
        let width = geometry.width() as usize;
        let half = geometry.height() / 2;
        let start =
            crate::framework_cast::integer(f64::from(half + 1) - f64::from(half) * depth / 100.)
                .max(0);
        let map = rasterize(geometry, true, false, cancelled, |_, index| {
            let row = (index / width) as i32;
            let opacity = if row < start {
                0.
            } else if row <= half {
                let transparency = (half + 1 - row) as f32 / (half + 1 - start) as f32;
                1. - transparency
            } else {
                1.
            };
            let [r, g, b, _] = softened.sample(index);
            let mut pixel = base.sample(index);
            EnvironmentBlendMode::Normal.composite([r, g, b], &mut pixel, opacity);
            pixel[3] = 0.;
            Ok(pixel)
        })?;
        map.set_constant(3, 0.)?;
        Ok(Source::Raster(Box::new(Raster::read(&map)?)))
    }

    fn sample(&self, [x, y, z]: [f64; 3]) -> f32 {
        let dot = x * self.axis[0] + y * self.axis[1] + z * self.axis[2];
        match self.shape {
            LightShape::Hard => (dot as f32).max(0.0),
            LightShape::Soft => soft_light([x, y, z], self.axis),
            LightShape::Sky {
                base,
                intensity,
                falloff,
                quadratic,
                extinction,
                normalization,
                scale,
            } => {
                let theta = (x * x + z * z).sqrt().atan2(y);
                let sky =
                    ((base + intensity * (falloff * dot.acos()).exp() + quadratic * dot * dot)
                        * (1.0 - (extinction / theta.cos()).exp())
                        / normalization) as f32;
                let sky = sky * scale;
                if sky < 0.0 { 0.0 } else { sky }
            }
        }
    }
}

pub(super) fn signed_gamma(value: f32, gamma: f64) -> f32 {
    if value < 0. {
        -f64::from(-value).powf(gamma) as f32
    } else {
        f64::from(value).powf(gamma) as f32
    }
}

fn sample(layers: &[Layer], direction: [f64; 3], index: usize) -> Result<[f32; 4], GradientError> {
    let mut bottom = [0., 0., 0., 1.];
    for layer in layers {
        if layer.strength == 0. {
            continue;
        }
        let [r, g, b, mut transparency] = layer.color_sample(direction, index)?;
        if let Some(mixer) = &layer.mixer {
            transparency = mixer.transparency(transparency, direction);
        }
        let coverage = (1. - transparency) * layer.strength;
        let mut top = [r, g, b];
        if layer.gamma != 1. {
            for color in &mut top {
                *color = signed_gamma(*color, layer.gamma);
            }
        }
        layer.blend.composite(top, &mut bottom, coverage);
    }
    Ok(bottom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EnvironmentTransformFilter as Filter;
    use crate::{EnvironmentField as Field, EnvironmentLayerKind, EnvironmentLight};

    #[test]
    fn preview_converter_samples_nearest_source_color_and_transparency() {
        let source = SharedImageMap::new(ImageGeometry::new(2, 2));
        source
            .set_buffer(0, Some(&FloatBuffer::new(vec![0., 0.25, 0.5, 0.75])))
            .unwrap();
        source.set_constant(1, 0.125).unwrap();
        source
            .set_buffer(3, Some(&FloatBuffer::new(vec![0., 0.125, 0.25, 0.5])))
            .unwrap();
        let lookup =
            EnvironmentPreviewConverter::from_rgb8(3, 1, &[127, 0, 249, 128, 127, 0, 255, 255, 7])
                .unwrap();
        let view = lookup.project::<()>(&source, &|| false).unwrap();
        assert_eq!(view.geometry(), ImageGeometry::new(3, 1));
        assert_eq!(view.channel(0, true).unwrap().values(), [0., 0.25, 0.75]);
        assert_eq!(view.channel(1, true).unwrap().read(0).unwrap(), 0.125);
        assert_eq!(view.channel(3, true).unwrap().values(), [0., 0.125, 0.5]);
        assert!(matches!(
            lookup.project::<()>(&source, &|| true),
            Err(PointwiseError::Cancelled)
        ));
    }

    #[test]
    fn layer_sphere_isolates_layer_pixels_and_applies_preview_strength() {
        let mut map = EnvironmentRecord::default();
        map.add_layer(EnvironmentLayerKind::Color).unwrap();
        map.set_field(Field::Blue(0), 0.75).unwrap();
        map.add_layer(EnvironmentLayerKind::Color).unwrap();
        map.set_field(Field::Red(0), 0.75).unwrap();
        let sample = |preview: &EnvironmentLayerPreview| {
            let sphere = preview
                .render_sphere::<()>(
                    ProceduralExecution::CallingThread,
                    &EnvironmentResourceLibrary::default(),
                    &|| false,
                )
                .unwrap();
            assert_eq!(sphere.geometry(), ImageGeometry::new(168, 168));
            [0, 1, 2, 3].map(|channel| {
                let values = sphere.channel(channel, true).unwrap().values();
                match values.as_slice() {
                    [constant] => *constant,
                    dense => dense[84 * 168 + 84],
                }
            })
        };
        let red = map.layer_preview(0).unwrap();
        let blue = map.layer_preview(1).unwrap();
        assert_eq!(sample(&red), [0.75, 0., 0., 0.]);
        assert_eq!(sample(&blue), [0., 0., 0.75, 0.]);
        map.set_exposure(3.);
        assert!(red.same_layer(&map.layer_preview(0).unwrap()));
        assert_eq!(sample(&red), [0.75, 0., 0., 0.]);

        map.set_field(Field::Strength(0), 0.).unwrap();
        let faint = map.layer_preview(0).unwrap();
        assert!(!faint.same_layer(&red));
        assert!(blue.same_layer(&map.layer_preview(1).unwrap()));
        let [r, g, b, t] = sample(&faint);
        assert_eq!([g, b, t], [r, r, 0.]);
        assert!((0.75..=0.95).contains(&r));
        assert_eq!(sample(&red), [0.75, 0., 0., 0.]);
    }

    #[test]
    fn mixer_samples_direction_even_for_a_solid_layer() {
        let geometry = ImageGeometry::new(32, 16);
        let mut map = EnvironmentRecord::default();
        map.add_layer(EnvironmentLayerKind::Color).unwrap();
        map.set_field(Field::Red(0), 1.).unwrap();
        map.set_mixer_enabled(0, true).unwrap();
        for (field, value) in [
            (Field::MixerRotation(0), 0.),
            (Field::MixerElevation(0), 0.),
            (Field::MixerLevel(0), 25.),
            (Field::MixerAmount(0), -25.),
        ] {
            map.set_field(field, value).unwrap();
        }
        let out = map
            .render_latlong::<()>(geometry, ProceduralExecution::CallingThread, None, &|| {
                false
            })
            .unwrap();
        let alpha = out.channel(3, true).unwrap();
        assert_eq!(alpha.read(8 * 32 + 8).unwrap(), 1.);
        assert_eq!(alpha.read(8 * 32 + 24).unwrap(), 0.5);
    }

    #[test]
    fn sky_haze_preserves_upper_rows_and_publishes_an_opaque_map() {
        let geometry = ImageGeometry::new(32, 16);
        let mut map = EnvironmentRecord::default();
        map.add_layer(EnvironmentLayerKind::Light(EnvironmentLight::ClearSky))
            .unwrap();
        let render = |map: &EnvironmentRecord| {
            map.render_latlong::<()>(geometry, ProceduralExecution::CallingThread, None, &|| {
                false
            })
            .unwrap()
        };
        let original = render(&map);
        let base = original.channel(0, true).unwrap().values();
        assert_eq!(
            original.channel(3, true).unwrap().values(),
            [0.],
            "a directionally varying sky still has a scalar opaque plane"
        );
        map.set_field(Field::SkyHaze(0), 80.).unwrap();
        let low_haze = render(&map).channel(0, true).unwrap().values();
        assert_eq!(low_haze[..32 * 9], base[..32 * 9]);
        assert_ne!(low_haze[32 * 9..], base[32 * 9..]);
        map.set_field(Field::SkyDepth(0), 100.).unwrap();
        let full_haze = render(&map);
        let color = full_haze.channel(0, true).unwrap().values();
        assert_eq!(color[..32], base[..32]);
        assert_ne!(color[32 * 2..32 * 8], low_haze[32 * 2..32 * 8]);
        assert!(
            full_haze
                .channel(3, true)
                .unwrap()
                .values()
                .into_iter()
                .all(|value| value == 0.)
        );
    }

    #[test]
    fn recolor_uses_luma_then_tint_contrast_and_hsl_with_gradient_opacity() {
        let geometry = ImageGeometry::new(8, 4);
        let mut map = EnvironmentRecord::default();
        map.add_layer(EnvironmentLayerKind::Color).unwrap();
        map.set_field(Field::Red(0), 1.).unwrap();
        map.set_recolor_enabled(0, true).unwrap();
        let gradient = crate::SharedGradient::new();
        gradient
            .add_color(0, RgbColor::new(0., 0., 0., 1.))
            .unwrap();
        gradient
            .add_color(999, RgbColor::new(1., 0., 0., 1.))
            .unwrap();
        gradient.add_alpha(0, 0.25).unwrap();
        map.set_recolor_gradient(0, &gradient).unwrap();
        // The stored gradient is detached from the caller's owner.
        gradient.alpha_point(0).unwrap().unwrap().set_value(0.9);
        for (field, value) in [
            (Field::Tint(0, 0), 0.5),
            (Field::Contrast(0), -50.),
            (Field::Hue(0), 120.),
            (Field::Saturation(0), -50.),
        ] {
            map.set_field(field, value).unwrap();
        }
        let out = map
            .render_latlong::<()>(geometry, ProceduralExecution::CallingThread, None, &|| {
                false
            })
            .unwrap();
        for (channel, expected) in [0.2686875, 0.3060625, 0.2686875, 0.75]
            .into_iter()
            .enumerate()
        {
            let actual = out.channel(channel as i32, true).unwrap().read(0).unwrap();
            assert!(
                (actual - expected).abs() < 1e-6,
                "channel {channel}: {actual} vs {expected}"
            );
        }
    }

    #[test]
    fn disabled_recolor_omits_gradient_from_wire_and_reenabling_recovers_the_draft() {
        let mut map = EnvironmentRecord::default();
        map.add_layer(EnvironmentLayerKind::Color).unwrap();
        map.set_recolor_enabled(0, true).unwrap();
        let gradient = map.copy_recolor_gradient(0).unwrap().unwrap();
        gradient.add_alpha(0, 0.42).unwrap();
        map.set_recolor_gradient(0, &gradient).unwrap();
        map.set_recolor_enabled(0, false).unwrap();
        assert!(map.copy_recolor_gradient(0).unwrap().is_none());
        let mut writer = crate::BinaryWriter::default();
        map.save(&mut writer).unwrap();
        let mut reader = crate::BinaryCursor::new(writer.bytes()).unwrap();
        let loaded = EnvironmentRecord::read(&mut reader, 3.75).unwrap();
        assert_eq!(reader.position().unwrap() as usize, writer.bytes().len());
        assert!(!loaded.recolor_enabled(0));
        map.set_recolor_enabled(0, true).unwrap();
        let gradient = map.copy_recolor_gradient(0).unwrap().unwrap();
        assert_eq!(gradient.alpha_point(0).unwrap().unwrap().value(), 0.42);
    }

    #[test]
    fn layer_heading_and_master_heading_act_on_the_native_grid() {
        let geometry = ImageGeometry::new(32, 16);
        let mut map = EnvironmentRecord::default();
        map.add_layer(EnvironmentLayerKind::Light(EnvironmentLight::Hard))
            .unwrap();
        let red = |map: &EnvironmentRecord| {
            map.render_latlong::<()>(geometry, ProceduralExecution::CallingThread, None, &|| {
                false
            })
            .unwrap()
            .channel(0, true)
            .unwrap()
            .values()
        };
        let original = red(&map);
        map.set_field(Field::Heading(Some(0)), 90.).unwrap();
        let shifted = red(&map);
        assert_ne!(shifted, original);
        for (row, expected) in shifted.chunks_exact(32).zip(original.chunks_exact(32)) {
            for (x, value) in row.iter().enumerate() {
                assert_eq!(*value, expected[(x + 8) % 32]);
            }
        }
        map.set_field(Field::Heading(None), -90.).unwrap();
        assert_eq!(red(&map), original);
    }

    #[test]
    fn transform_filter_modes_render_and_keep_authored_values_while_switching() {
        let geometry = ImageGeometry::new(32, 16);
        let mut map = EnvironmentRecord::default();
        map.add_layer(EnvironmentLayerKind::Light(EnvironmentLight::Hard))
            .unwrap();
        let red = |map: &EnvironmentRecord| {
            map.render_latlong::<()>(geometry, ProceduralExecution::CallingThread, None, &|| {
                false
            })
            .unwrap()
            .channel(0, true)
            .unwrap()
            .values()
        };
        let original = red(&map);
        map.set_transform_filter(None, Filter::Diffuse).unwrap();
        map.set_field(Field::FilterRadius(None), 18.5).unwrap();
        let diffuse = red(&map);
        assert_ne!(diffuse, original);
        map.set_transform_filter(None, Filter::Scatter).unwrap();
        let scatter = red(&map);
        assert_ne!(scatter, diffuse);
        map.set_transform_filter(None, Filter::Anisotropic).unwrap();
        let anisotropic = red(&map);
        assert_ne!(anisotropic, scatter);
        map.set_transform_filter(None, Filter::LegacyAnisotropic)
            .unwrap();
        assert_ne!(red(&map), anisotropic);
        map.set_transform_filter(None, Filter::Diffuse).unwrap();
        assert_eq!(map.field(Field::FilterRadius(None)), Some(18.5));
        assert_eq!(red(&map), diffuse);
        map.set_field(Field::Exposure, 2.).unwrap();
        map.set_field(Field::Gamma, 2.).unwrap();
        let expected: Vec<_> = diffuse
            .into_iter()
            .map(|v| signed_gamma(v * 4., 0.5))
            .collect();
        assert_eq!(red(&map), expected);
    }
}
