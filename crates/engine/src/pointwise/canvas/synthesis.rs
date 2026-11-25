//! Shared texture record used by Synthesis nodes and Style paint passes.
mod adjustments;
mod color_link;
pub use color_link::SynthesisColorTarget;
mod edit;
mod order;
mod patches;
mod photo_blend;
mod repeat;
mod stamp;
pub(super) mod wire;

use super::{
    blend::{BlendGrid, StyleBlend},
    raster::{fill_image, filled},
    style::Blend,
};
use crate::environment_record::ImageAsset;
use crate::pointwise::{
    PointwiseError, PointwiseFilter, PointwiseRenderEffects, SharedImageMap, blur, image_resize,
    parameter_ops::seed_modifier, transform,
};
use crate::{AnimationRecord, HslColor, ImageError, ImageGeometry, ProceduralExecution, RgbColor};
pub(super) use adjustments::{NumberLinks, NumericLink};
pub use adjustments::{SynthesisColorField, SynthesisNumberField, SynthesisPairField};
pub(crate) use edit::TextureEdit;
pub use edit::{
    SynthesisCropHandle, SynthesisEdit, SynthesisField, SynthesisGeometry, SynthesisLayerKind,
    SynthesisPatchResizeHandle, SynthesisPatchShape, SynthesisPatchTransform,
    SynthesisPatchTransformBounds, SynthesisPatchTransformField, SynthesisPlacementClipboard,
    SynthesisPlacementField, SynthesisPlacementGeometry, SynthesisPlacementTransform,
};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone, Debug, Default)]
pub(crate) struct SynthesisTexture {
    master: Master,
    layers: Vec<Arc<Layer>>,
}

/// Saved curves in property order. Runtime Synthesis uses the stored values.
#[derive(Clone, Debug)]
pub(super) struct PropertyAnimations<const N: usize>(Option<Arc<[Option<AnimationRecord>; N]>>);

impl<const N: usize> Default for PropertyAnimations<N> {
    fn default() -> Self {
        Self(None)
    }
}

impl<const N: usize> PropertyAnimations<N> {
    pub(super) fn from_records(records: [Option<AnimationRecord>; N]) -> Self {
        Self(
            records
                .iter()
                .any(Option::is_some)
                .then(|| Arc::new(records)),
        )
    }

    pub(super) fn get(&self, index: usize) -> Option<&AnimationRecord> {
        self.0.as_ref()?.get(index)?.as_ref()
    }
}

/// Retained Texture Synthesis value shared by GTX catalog and preset records.
#[derive(Clone, Debug, Default)]
pub struct SynthesisRecord(Arc<SynthesisTexture>);

impl SynthesisRecord {
    pub(crate) fn from_texture(texture: Arc<SynthesisTexture>) -> Self {
        Self(texture)
    }

    pub(crate) fn texture(&self) -> &Arc<SynthesisTexture> {
        &self.0
    }

    pub(crate) fn read(
        reader: &mut crate::BinaryCursor<'_>,
        version: f64,
    ) -> Result<Self, crate::NodeWireError> {
        Ok(Self(Arc::new(SynthesisTexture::read(reader, version)?)))
    }

    pub(crate) fn write(
        &self,
        writer: &mut crate::BinaryWriter,
    ) -> Result<(), crate::NodeWireError> {
        self.0.write(writer)
    }

    pub fn layer_count(&self) -> usize {
        self.0.layer_count()
    }

    pub fn uses_input(&self) -> bool {
        self.0.uses_input()
    }

    /// Renders a Patches layer's Manual placement authoring tile.
    /// Pass the raw graph Input image if connected. The optional seed supplies
    /// the standalone node's sampled graph Seed instead of the local Texture Seed.
    pub fn render_manual_placement_preview_with_effects<E: 'static>(
        &self,
        layer: usize,
        input: Option<&SharedImageMap>,
        side: i32,
        seed: Option<i32>,
        execution: ProceduralExecution,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        self.render_manual_placement_preview_with_geometry(
            layer, input, side, seed, execution, effects,
        )
        .map(|(image, _)| image)
    }

    /// Returns the Manual tile and the geometry of its actual source raster.
    /// A tile with no copies still reports its available source dimensions.
    pub fn render_manual_placement_preview_with_geometry<E: 'static>(
        &self,
        layer: usize,
        input: Option<&SharedImageMap>,
        side: i32,
        seed: Option<i32>,
        execution: ProceduralExecution,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<(SharedImageMap, Option<ImageGeometry>), PointwiseError<E>> {
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        let layer = self.0.layers.get(layer).ok_or(PointwiseError::Argument)?;
        let Paint::Patches(patches) = &layer.paint else {
            return Err(PointwiseError::InvalidCast);
        };
        let geometry = ImageGeometry::new(side, side);
        if side <= 0 {
            return Err(PointwiseError::Argument);
        }
        let (pixels, source) = patches.render_manual_preview(
            input,
            geometry,
            seed.unwrap_or_else(|| self.0.master.effective_seed()),
            execution,
            effects,
        )?;
        let image = SharedImageMap::new(geometry);
        fill_image(&image, &pixels)?;
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        Ok((image, source))
    }

    /// Renders the texture editor's local texture, optionally through one layer.
    /// A supplied graph input is already frozen at the caller's sample time.
    /// The Whole Texture scope uses the same compositor as final rendering.
    pub fn render_editor_preview_with_effects<E: 'static>(
        &self,
        upstream: Option<&crate::PointwiseNodeRenderSnapshot<E>>,
        layer: Option<usize>,
        geometry: ImageGeometry,
        execution: ProceduralExecution,
        options: crate::PointwiseRenderOptions,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<crate::PointwiseRenderOutcome<E>, PointwiseError<E>> {
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        let uses_input = match layer {
            Some(index) => self
                .0
                .layers
                .get(index)
                .ok_or(PointwiseError::Argument)?
                .uses_input(),
            None => self.0.uses_input(),
        };
        let mut caught = Vec::new();
        let input = if uses_input {
            if let Some(upstream) = upstream {
                let outcome =
                    upstream.render_export_with_effects(geometry, execution, options, effects)?;
                let image = outcome.image().ok_or(PointwiseError::NullReference)?;
                caught = outcome.caught;
                Some(image)
            } else {
                Some(editor_input_sample(geometry)?)
            }
        } else {
            None
        };
        let image = match layer {
            Some(index) => self.0.render_layer(
                index,
                TextureRender {
                    input: input.as_ref(),
                    geometry,
                    scale: self.0.master.effective_scale(),
                    seed: self.0.master.effective_seed(),
                    execution,
                    layer_preview: true,
                },
                effects,
            )?,
            None => self
                .0
                .render_style(input.as_ref(), geometry, 0., 0, execution, effects)?,
        };
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        Ok(crate::PointwiseRenderOutcome {
            image: Some(crate::RenderedImage::Still(image)),
            caught,
        })
    }
}

/// The shared live texture owned by one document catalog entry.
#[derive(Clone, Debug)]
pub struct SynthesisCatalogValue(Arc<Mutex<Arc<SynthesisTexture>>>);

impl SynthesisCatalogValue {
    pub(crate) fn new(record: SynthesisRecord) -> Self {
        Self(Arc::new(Mutex::new(record.0)))
    }

    pub fn record(&self) -> SynthesisRecord {
        SynthesisRecord(self.lock().clone())
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Arc<SynthesisTexture>> {
        super::super::guard(&self.0)
    }

    pub(crate) fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn lock_order(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
}

#[derive(Clone, Debug)]
struct Master {
    brightness: f32,
    contrast: f32,
    hue: f32,
    saturation: f32,
    sharpen: f32,
    sharpen_radius: f32,
    scale: f64,
    scale_adjustment: Option<NumericLink>,
    seed: i32,
    seed_adjustment: Option<NumericLink>,
    color: [f64; 4],
    color_adjustment: Option<color_link::ColorLink>,
    color_links: NumberLinks,
    /// Original property order from Brightness through Colorize.
    animations: PropertyAnimations<9>,
}

impl Default for Master {
    fn default() -> Self {
        Self {
            brightness: 0.,
            contrast: 0.,
            hue: 0.,
            saturation: 0.,
            sharpen: 1.,
            sharpen_radius: 0.005,
            scale: 0.,
            scale_adjustment: None,
            seed: 0,
            seed_adjustment: None,
            color: [1.; 4],
            color_adjustment: None,
            color_links: NumberLinks::default(),
            animations: PropertyAnimations::default(),
        }
    }
}

#[derive(Clone, Debug)]
struct Layer {
    identity: Arc<()>,
    blend: Blend,
    color_controls: LayerColor,
    paint: Paint,
}

#[derive(Clone, Debug)]
struct LayerColor {
    tint: [f64; 4],
    tint_adjustment: Option<color_link::ColorLink>,
    adjustments: [f32; 6],
    // The original layer worker renders with the stored values in `adjustments`.
    animations: PropertyAnimations<7>,
    number_links: NumberLinks,
}

impl Default for LayerColor {
    fn default() -> Self {
        Self {
            tint: [1.; 4],
            tint_adjustment: None,
            adjustments: [0.; 6],
            animations: PropertyAnimations::default(),
            number_links: NumberLinks::default(),
        }
    }
}

#[derive(Clone, Debug)]
struct SolidPaint {
    color: [f64; 4],
    color_adjustment: Option<color_link::ColorLink>,
    animations: PropertyAnimations<1>,
}

#[derive(Clone, Debug)]
enum Paint {
    Solid(SolidPaint),
    Repeat(Box<repeat::Pattern>),
    Blend(Box<photo_blend::PhotoBlend>),
    Patches(Box<patches::Patches>),
}

#[derive(Clone, Debug)]
enum Source {
    Input,
    Asset(Arc<ImageAsset>),
    Missing,
}

struct Photo {
    pixels: [Vec<f32>; 4],
    size: [usize; 2],
}

impl Photo {
    fn read(image: &SharedImageMap) -> Result<Self, ImageError> {
        let geometry = image.geometry();
        let length = area(geometry)?;
        let mut pixels = std::array::from_fn(|_| Vec::new());
        for (channel, plane) in pixels.iter_mut().enumerate() {
            let samples = image.channel(channel as i32, true)?;
            if samples.len() != 1 && samples.len() != length {
                return Err(ImageError::Argument);
            }
            *plane = samples.values();
        }
        Ok(Self {
            pixels,
            size: [geometry.width() as usize, geometry.height() as usize],
        })
    }
}

pub(in crate::pointwise) fn render_synthesis<E: 'static>(
    filter: &PointwiseFilter<E>,
    input: Option<&SharedImageMap>,
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    layer: Option<usize>,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    if let Some(wire) = filter.synthesis_wire() {
        wire.ensure_evaluation_supported()?;
    }
    let texture = filter
        .synthesis_texture()
        .ok_or(PointwiseError::NullReference)?;
    let request = TextureRender {
        input,
        geometry,
        scale: combine_scale(
            filter.parameter(0)?.effective_value_at(time)?,
            filter.factor(),
        ),
        seed: filter.parameter(1)?.effective_seed_at(time)? as i32,
        execution,
        layer_preview: layer.is_some(),
    };
    match layer {
        Some(index) => texture.render_layer(index, request, effects),
        None => texture.render(request, effects),
    }
}

#[derive(Clone, Copy)]
struct TextureRender<'a> {
    input: Option<&'a SharedImageMap>,
    geometry: ImageGeometry,
    scale: f64,
    seed: i32,
    execution: ProceduralExecution,
    layer_preview: bool,
}

impl TextureRender<'_> {
    fn photo_width(self, requested: f32, size: ImageGeometry) -> i32 {
        let source_width = size.width();
        let width = source_width as f32;
        let height = size.height() as f32;
        let width_request = if self.layer_preview && source_width > 0 {
            // auf.h initializes a size quantum for asset previews. The paint
            // workers round to it before applying aspect and source bounds.
            let mut divisor = 32;
            while divisor > 1 && source_width / divisor < 64 {
                divisor /= 2;
            }
            let quantum = (source_width / divisor) as f32;
            (requested / quantum - 0.05).ceil() * quantum
        } else {
            requested
        };
        width_request
            .max((width / height).max(height / width).ceil())
            .min(width) as i32
    }
}

pub(in crate::pointwise) fn combine_scale(raw: f64, factor: f64) -> f64 {
    let scale = linear_scale(raw) * factor;
    if scale < 1. {
        1. - 1. / scale
    } else {
        scale - 1.
    }
}

fn linear_scale(raw: f64) -> f64 {
    if raw < 0. { -1. / (raw - 1.) } else { raw + 1. }
}

impl Layer {
    fn uses_input(&self) -> bool {
        match &self.paint {
            Paint::Repeat(pattern) => matches!(pattern.source, Source::Input),
            Paint::Blend(blend) => matches!(blend.source, Source::Input),
            Paint::Patches(patches) => matches!(patches.source, Source::Input),
            Paint::Solid(_) => false,
        }
    }

    fn render_paint<E: 'static>(
        &self,
        length: usize,
        request: TextureRender<'_>,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<Vec<[f32; 4]>, PointwiseError<E>> {
        match &self.paint {
            Paint::Solid(paint) => {
                let [r, g, b] = paint.effective_color().map(|value| value as f32);
                Ok(filled(length, [r, g, b, 0.])?)
            }
            Paint::Repeat(pattern) => pattern.render(&self.color_controls, request, effects),
            Paint::Blend(blend) => blend.render(&self.color_controls, request, effects),
            Paint::Patches(patches) => patches.render(&self.color_controls, request, effects),
        }
    }
}

impl SynthesisTexture {
    pub(in crate::pointwise) fn layer_count(&self) -> usize {
        self.layers.len()
    }

    pub(in crate::pointwise) fn layer_uses_input(&self, layer: usize) -> bool {
        self.layers
            .get(layer)
            .is_some_and(|layer| layer.uses_input())
    }

    pub(in crate::pointwise) fn uses_input(&self) -> bool {
        self.layers.iter().any(|layer| layer.uses_input())
    }

    pub(in crate::pointwise) fn uses_library(&self) -> bool {
        self.layers.iter().any(|layer| {
            let source = match &layer.paint {
                Paint::Repeat(pattern) => &pattern.source,
                Paint::Blend(blend) => &blend.source,
                Paint::Patches(patches) => &patches.source,
                Paint::Solid(_) => return false,
            };
            matches!(source, Source::Asset(asset) if matches!(asset.as_ref(), ImageAsset::Library { .. }))
        })
    }

    pub(super) fn render_style<E: 'static>(
        &self,
        input: Option<&SharedImageMap>,
        geometry: ImageGeometry,
        scale: f32,
        seed: i32,
        execution: ProceduralExecution,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let local_scale = self.master.effective_scale();
        let scale = if scale == 0. {
            local_scale
        } else {
            combine_scale(local_scale, linear_scale(f64::from(scale)))
        };
        let master_seed = self.master.effective_seed();
        let seed = if seed == 0 {
            master_seed
        } else {
            seed_modifier(f64::from(master_seed), f64::from(seed), false) as i32
        };
        self.render(
            TextureRender {
                input,
                geometry,
                scale,
                seed,
                execution,
                layer_preview: false,
            },
            effects,
        )
    }

    fn render<E: 'static>(
        &self,
        request: TextureRender<'_>,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let TextureRender {
            geometry,
            execution,
            ..
        } = request;
        let output = SharedImageMap::new(geometry);
        let length = usize::try_from(geometry.area()).map_err(|_| PointwiseError::Argument)?;
        if length == 0 {
            return Ok(output);
        }
        if self.layers.is_empty() {
            let [brightness, contrast, hue, saturation, ..] = self.master.effective_color_values();
            let [r, g, b] = Master::adjust(
                [0.; 3],
                [brightness, contrast, hue, saturation],
                self.master.effective_tint(),
            );
            for (channel, value) in [r, g, b, 1.].into_iter().enumerate() {
                output.set_constant(channel as i32, value)?;
            }
            return Ok(output);
        }
        let mut pixels = filled(length, [0., 0., 0., 1.])?;
        let grid = BlendGrid {
            geometry,
            execution,
        };
        for (index, layer) in self.layers.iter().rev().enumerate() {
            crate::pointwise::check_render_cancelled(effects.cancel_check())?;
            let mut paint = layer.render_paint(length, request, effects)?;
            let record = layer.blend.effective_synthesis();
            if index == 0 && record.erosion == 0. && record.opacity == 100. {
                pixels = paint;
                continue;
            }
            let blend = StyleBlend::new(&record)?;
            grid.composite(&blend, &record, &mut pixels, &mut paint, effects)?;
        }
        self.master
            .finish(&output, &mut pixels, execution, effects)?;
        Ok(output)
    }

    fn render_layer<E: 'static>(
        &self,
        index: usize,
        request: TextureRender<'_>,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let layer = self.layers.get(index).ok_or(PointwiseError::Argument)?;
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        let output = SharedImageMap::new(request.geometry);
        let length =
            usize::try_from(request.geometry.area()).map_err(|_| PointwiseError::Argument)?;
        if length > 0 {
            let pixels = layer.render_paint(length, request, effects)?;
            fill_image(&output, &pixels)?;
        }
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        Ok(output)
    }
}

fn editor_input_sample(geometry: ImageGeometry) -> Result<SharedImageMap, ImageError> {
    let image = SharedImageMap::new(geometry);
    for channel in 0..3 {
        image.set_constant(channel, 0.5)?;
    }
    image.set_constant(3, 0.)?;
    Ok(image)
}

fn prepare_photo<E: 'static>(
    source: &SharedImageMap,
    equalize: bool,
    strength: f64,
    execution: ProceduralExecution,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let [width, height] = [source.geometry().width(), source.geometry().height()];
    // Both requested sides are the source width. Fit the photo in that
    // square while keeping its aspect ratio.
    let [width, height] = if width <= height {
        [(i64::from(width).pow(2) / i64::from(height)) as i32, width]
    } else {
        [width, height]
    };
    let photo = image_resize::resize(source, width.max(1), height.max(1), execution)?;
    // The saved Strength is the complement of the radius used by Equalize.
    let radius_percent = 100. - strength;
    if equalize && radius_percent < 100. {
        let alpha = photo.channel(3, true)?;
        let image = PointwiseFilter::equalize_image(
            Some(&photo),
            radius_percent / 100.,
            1.,
            true,
            execution,
            effects,
        )?;
        image.set_buffer(3, Some(&alpha))?;
        Ok(image)
    } else {
        Ok(photo)
    }
}

fn area(size: ImageGeometry) -> Result<usize, ImageError> {
    size.width()
        .checked_mul(size.height())
        .filter(|area| *area > 0)
        .map(|area| area as usize)
        .ok_or(ImageError::Overflow)
}

#[cfg(test)]
#[test]
fn synthesis_equalize_strength_complements_radius_and_preserves_alpha() {
    let photo = SharedImageMap::new(ImageGeometry::new(64, 64));
    let pixels = (0..4096)
        .map(|sample| {
            let tone = (sample % 29) as f32 / 29.;
            [tone, tone, tone, (sample % 7) as f32 / 7.]
        })
        .collect::<Vec<_>>();
    fill_image(&photo, &pixels).unwrap();
    let execution = ProceduralExecution::CallingThread;
    let mut capacity = |_| Ok(true);
    let mut preflight = |_, _| Ok(());
    let mut effects = PointwiseRenderEffects::<()>::new(&mut capacity, &mut preflight);
    let values = |image: &SharedImageMap, channel| image.channel(channel, true).unwrap().values();
    let unfiltered = prepare_photo(&photo, true, 0., execution, &mut effects).unwrap();
    assert_eq!(values(&unfiltered, 0), values(&photo, 0));
    let prepared = prepare_photo(&photo, true, 20., execution, &mut effects).unwrap();
    let expected =
        PointwiseFilter::equalize_image(Some(&photo), 0.8, 1., true, execution, &mut effects)
            .unwrap();
    assert_eq!(values(&prepared, 0), values(&expected, 0));
    assert_ne!(values(&prepared, 0), values(&photo, 0));
    assert_eq!(values(&prepared, 3), values(&photo, 3));
}

impl Master {
    fn adjust(
        mut rgb: [f32; 3],
        [brightness, contrast, hue_shift, saturation_gain]: [f32; 4],
        tint: [f64; 3],
    ) -> [f32; 3] {
        for (value, tint) in rgb.iter_mut().zip(tint) {
            *value *= tint as f32;
        }
        if hue_shift != 0. || saturation_gain != 0. {
            let [r, g, b] = rgb.map(f64::from);
            let [hue, saturation, lightness] = RgbColor::new(r, g, b, 1.).to_hsl().components();
            let hue = (hue + f64::from(hue_shift / 360.)).rem_euclid(1.);
            let saturation = saturation * f64::from(saturation_gain / 100. + 1.);
            let [_, r, g, b] = HslColor::new(hue, saturation, lightness).to_rgb().argb();
            rgb = [r as f32, g as f32, b as f32];
        }
        let brightness = brightness / 100.;
        if brightness > 0. {
            rgb = rgb.map(|value| (1. - brightness) * value + brightness);
        } else if brightness != 0. {
            rgb = rgb.map(|value| (1. + brightness) * value);
        }
        let contrast = contrast / 100.;
        if contrast > 0. {
            let half = contrast / 2.;
            let gain = 1. / (1. - 2. * half).max(0.0001);
            rgb = rgb.map(|value| ((value - half) * gain).clamp(0., 1.));
        } else if contrast != 0. {
            rgb = rgb.map(|value| (value + (value - 0.5) * contrast).clamp(0., 1.));
        }
        rgb
    }

    fn finish<E>(
        &self,
        output: &SharedImageMap,
        pixels: &mut [[f32; 4]],
        execution: ProceduralExecution,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<(), PointwiseError<E>> {
        let geometry = output.geometry();
        let width = geometry.width();
        let height = geometry.height();
        let [
            brightness,
            contrast,
            hue,
            saturation,
            sharpen,
            sharpen_radius,
        ] = self.effective_color_values();
        let color = [brightness, contrast, hue, saturation];
        let tint = self.effective_tint();
        let mut lowpass = None;
        if sharpen > 0. {
            let image = SharedImageMap::new(geometry);
            fill_image(&image, pixels)?;
            let image = if width < 3000 {
                image
            } else {
                image_resize::resize(&image, width / 2, height / 2, execution)?
            };
            let blurred = blur::adaptive(
                &image,
                image.geometry(),
                blur::AdaptiveSettings {
                    horizontal: true,
                    vertical: true,
                    radius: f64::from(sharpen_radius),
                    factor: 1.,
                    periodic: true,
                    all_channels: false,
                    force_expansion: false,
                },
                execution,
                effects,
            )?;
            lowpass = Some([
                blurred.channel(0, true)?,
                blurred.channel(1, true)?,
                blurred.channel(2, true)?,
            ]);
        }
        for (y, row) in pixels.chunks_exact_mut(width as usize).enumerate() {
            crate::pointwise::check_render_cancelled(effects.cancel_check())?;
            for (x, pixel) in row.iter_mut().enumerate() {
                let [r, g, b, _] = *pixel;
                let mut rgb = [r, g, b];
                if let Some(lowpass) = &lowpass {
                    for (value, plane) in rgb.iter_mut().zip(lowpass) {
                        let low = if plane.len() == 1 {
                            plane.read(0)?
                        } else if width < 3000 {
                            plane.read(y * width as usize + x)?
                        } else {
                            transform::sample(
                                x as f32 * 0.5 - 0.25,
                                y as f32 * 0.5 - 0.25,
                                plane,
                                width / 2,
                                height / 2,
                            )?
                        };
                        *value -= sharpen * 0.3 * (low - *value);
                    }
                }
                pixel[..3].copy_from_slice(&Self::adjust(rgb, color, tint));
            }
        }
        Ok(fill_image(output, pixels)?)
    }
}
