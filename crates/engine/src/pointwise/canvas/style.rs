use super::synthesis::{
    NumberLinks, NumericLink, PropertyAnimations, SynthesisNumberField, SynthesisTexture,
    wire::{read_numeric_bindings, read_unadjusted_animation, write_animated_property},
};
use super::*;
use crate::pointwise::node_wire::{NodeWireReadContext, read_style_noise_lab};
use crate::{
    AdjustmentRecord, AnimationRecord, CurveRecord, EnvironmentRecord, GradientRecord,
    PropertyRecordError, SynthesisCatalogValue, SynthesisRecord,
};
#[cfg(test)]
#[path = "style/blend_animation_tests.rs"]
mod blend_animation_tests;
mod edit;
pub use edit::{
    StyleControl, StyleControlKind, StyleCurve, StyleEdit, StyleNoiseScope, StylePaintKind,
    StyleSection,
};

#[derive(Clone, Debug)]
pub(in crate::pointwise) struct CanvasStyle {
    pub(super) passes: Vec<StylePass>,
    selected: i32,
}

impl CanvasStyle {
    pub(in crate::pointwise) fn preview_environment(
        &self,
        index: usize,
        map: &Arc<EnvironmentRecord>,
    ) -> Option<Arc<Self>> {
        let mut style = self.clone();
        *style.environment_map_mut(index)? = map.clone();
        Some(Arc::new(style))
    }

    fn environment_map_mut(&mut self, index: usize) -> Option<&mut Arc<EnvironmentRecord>> {
        let Paint::Environment(paint) = &mut self.passes.get_mut(index)?.paint else {
            return None;
        };
        Some(&mut paint.map)
    }
}

#[derive(Clone, Debug)]
pub(super) struct StylePass {
    identity: Arc<()>,
    pub(super) area: Option<ShapeArea>,
    pub(super) gradient_mask: Option<GradientField>,
    pub(super) noise_mask: Option<StyleNoise>,
    pub(super) blend: Blend,
    pub(super) paint: Paint,
    inactive: InactiveControls,
}

impl StylePass {
    pub(super) fn parked_paints(&self) -> &[Paint] {
        &self.inactive.paints
    }

    pub(super) fn parked_paints_mut(&mut self) -> &mut [Paint] {
        &mut self.inactive.paints
    }
}

// Source pass objects keep their inactive controls while the editor is open.
// GTX writes the selected effect and enabled masks only.
#[derive(Clone, Debug, Default)]
struct InactiveControls {
    paints: Vec<Paint>,
    area: Option<ShapeArea>,
    shadow: Option<SecondaryArea>,
    gradient_mask: Option<GradientField>,
    effect_field: Option<GradientField>,
    noise_mask: Option<StyleNoise>,
}

#[derive(Clone, Debug)]
pub(super) struct ShapeArea {
    pub(super) offset: [f64; 2],
    pub(super) thickness: f64,
    pub(super) hard_feather: f64,
    pub(super) soft_feather: f64,
    pub(super) feather_curve: CurveRecord,
    pub(super) outline: bool,
    pub(super) outline_thickness: f64,
    pub(super) secondary: Option<SecondaryArea>,
}

#[derive(Clone, Debug)]
pub(super) struct SecondaryArea {
    pub(super) mode: i32,
    pub(super) offset: [f64; 2],
    pub(super) hard_feather: f64,
    pub(super) soft_feather: f64,
    pub(super) curve: CurveRecord,
}

#[derive(Clone, Debug)]
pub(super) struct Blend {
    pub(super) mode: i32,
    pub(super) erosion_mode: i32,
    erosion_edges_only: bool,
    pub(super) erosion: f32,
    pub(super) erosion_sharpness: f32,
    pub(super) opacity: f64,
    pub(super) relief: [BlendRelief; 2],
    pub(super) skip_for_color: bool,
    animations: PropertyAnimations<6>,
    pub(super) synthesis_links: NumberLinks,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct BlendRelief {
    pub(super) mode: i32,
    pub(super) normal_blur: f64,
    pub(super) depth: f64,
    pub(super) offset_blur: f64,
    pub(super) direction: [f64; 2],
    pub(super) distance: f64,
}

#[derive(Clone, Debug)]
pub(super) enum Paint {
    Adjust(ColorAdjustment),
    Environment(Box<EnvironmentPaint>),
    Refract(Box<RefractionPaint>),
    Blur { radius: f64 },
    Gradient(Box<GradientPaint>),
    Noise(Box<NoisePaint>),
    Solid { argb: [f64; 4], transparency: f64 },
    Input { offset: [f64; 2], rotation: i32 },
    Copy { offset: [f64; 2], rotation: i32 },
    Synthesis(SynthesisPaint),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SynthesisPaint {
    pub(crate) local: Arc<SynthesisTexture>,
    pub(crate) adjustment: Option<Arc<AdjustmentRecord>>,
    pub(crate) resolved: Option<(SynthesisCatalogValue, SynthesisRecord)>,
}

impl SynthesisPaint {
    pub(crate) fn key(&self) -> Option<([u8; 16], &[u16])> {
        let record = self.adjustment.as_ref()?;
        let [component] = record.components() else {
            return None;
        };
        (record.property_type == 10 && component.active && !component.name.is_empty())
            .then_some((record.guid, component.name.as_slice()))
    }

    pub(crate) fn catalog(&self) -> Option<&SynthesisCatalogValue> {
        self.resolved.as_ref().map(|(value, _)| value)
    }

    pub(super) fn texture(&self) -> &Arc<SynthesisTexture> {
        self.resolved
            .as_ref()
            .map_or(&self.local, |(_, record)| record.texture())
    }

    pub(crate) fn record(&self) -> SynthesisRecord {
        SynthesisRecord::from_texture(self.texture().clone())
    }

    pub(crate) fn with_material_record(
        &self,
        catalog: &SynthesisCatalogValue,
        record: &SynthesisRecord,
    ) -> Option<Self> {
        let (current, before) = self.resolved.as_ref()?;
        (current.same_reference(catalog) && !Arc::ptr_eq(before.texture(), record.texture())).then(
            || Self {
                resolved: Some((current.clone(), record.clone())),
                ..self.clone()
            },
        )
    }

    pub(super) fn disconnect_for_effect_change(&mut self) {
        self.adjustment = None;
        self.resolved = None;
    }
}

#[derive(Clone, Debug)]
pub(super) struct Bevel {
    pub(super) feather: [f64; 2],
    pub(super) curve: CurveRecord,
    pub(super) noise: Option<StyleNoise>,
    inactive_noise: Option<StyleNoise>,
}

#[derive(Clone, Debug)]
pub(super) struct ColorAdjustment {
    pub(super) argb: [f64; 4],
    pub(super) brightness: f64,
    pub(super) contrast: f64,
    pub(super) hue: f64,
    pub(super) saturation: f64,
}

#[derive(Clone, Debug)]
pub(super) struct RefractionPaint {
    pub(super) bevel: Bevel,
    pub(super) index: f64,
    pub(super) depth: f64,
    pub(super) affects_height: bool,
}

#[derive(Clone, Debug)]
pub(super) struct EnvironmentPaint {
    pub(super) bevel: Bevel,
    pub(super) map: Arc<EnvironmentRecord>,
    pub(super) adjustment: Option<Arc<AdjustmentRecord>>,
    pub(super) resolution: i32,
    pub(super) depth: f64,
    pub(super) affects_height: bool,
    pub(super) flatten_color: bool,
}

#[derive(Clone, Debug)]
pub(super) struct GradientField {
    pub(super) wrap: bool,
    pub(super) curve: CurveRecord,
    pub(super) blur: f64,
    pub(super) angle: f64,
    pub(super) offset: [f64; 2],
    pub(super) kind: i32,
}

#[derive(Clone, Debug)]
pub(super) struct StyleNoise {
    pub(super) strength: f64,
    // Validated with the enclosing file's read policy before retaining the
    // current-format record. Each render receives a private Lab instance.
    pub(super) lab: Arc<[u8]>,
    pub(super) affects_opacity: bool,
}

#[derive(Clone, Debug)]
pub(super) struct GradientPaint {
    pub(super) noise: Option<StyleNoise>,
    inactive_noise: Option<StyleNoise>,
    pub(super) field: Option<GradientField>,
    pub(super) gradient: GradientRecord,
}

#[derive(Clone, Debug)]
pub(super) struct NoisePaint {
    pub(super) lab: Arc<[u8]>,
    pub(super) gradient: GradientRecord,
}

struct StyleReader<'a, 'b, 'c, E> {
    reader: &'a mut BinaryCursor<'b>,
    context: Option<&'a mut NodeWireReadContext<'c, E>>,
    version: f64,
}

impl Default for CanvasStyle {
    fn default() -> Self {
        Self {
            passes: vec![StylePass {
                identity: Arc::new(()),
                inactive: InactiveControls::default(),
                area: Some(ShapeArea {
                    offset: [0.; 2],
                    thickness: 0.,
                    hard_feather: 0.,
                    soft_feather: 0.,
                    feather_curve: CurveRecord::new(0., 1., false),
                    outline: false,
                    outline_thickness: 5.,
                    secondary: None,
                }),
                gradient_mask: None,
                noise_mask: None,
                blend: Blend {
                    mode: 1,
                    erosion_mode: 0,
                    erosion_edges_only: true,
                    erosion: 0.,
                    erosion_sharpness: 70.,
                    opacity: 100.,
                    relief: [BlendRelief::default(); 2],
                    skip_for_color: false,
                    animations: PropertyAnimations::default(),
                    synthesis_links: NumberLinks::default(),
                },
                paint: Paint::Solid {
                    argb: [
                        1.,
                        f64::from(151_f32 / 255.),
                        f64::from(166_f32 / 255.),
                        f64::from(236_f32 / 255.),
                    ],
                    transparency: 0.,
                },
            }],
            selected: 0,
        }
    }
}

impl CanvasStyle {
    pub(super) fn from_input() -> Self {
        let mut style = Self::default();
        style.passes[0].paint = Paint::Input {
            offset: [0.; 2],
            rotation: 0,
        };
        style
    }

    pub(super) fn needs_input(&self) -> bool {
        self.passes.iter().any(|pass| {
            pass.noise_mask.is_some()
                || match &pass.paint {
                    Paint::Input { .. } => true,
                    Paint::Environment(paint) => paint.bevel.noise.is_some(),
                    Paint::Refract(paint) => paint.bevel.noise.is_some(),
                    Paint::Noise(_) => true,
                    Paint::Synthesis(paint) => paint.texture().uses_input(),
                    Paint::Gradient(paint) => paint.noise.is_some(),
                    _ => false,
                }
        })
    }

    pub(in crate::pointwise) fn uses_environment_library(&self) -> bool {
        self.passes.iter().any(|pass| match &pass.paint {
            Paint::Environment(paint) => paint.map.uses_library_images(),
            Paint::Synthesis(paint) => paint.texture().uses_library(),
            _ => false,
        })
    }

    pub(in crate::pointwise) fn read<E: 'static>(
        reader: &mut BinaryCursor<'_>,
        version: f64,
        mut context: Option<&mut NodeWireReadContext<'_, E>>,
    ) -> Result<Self, NodeWireError<E>> {
        let passes = read_list(reader, 4, |reader| {
            StyleReader {
                reader,
                version,
                context: context.as_deref_mut(),
            }
            .pass()
        })?;
        Ok(Self {
            passes,
            selected: reader.read_i32()?,
        })
    }

    pub(in crate::pointwise) fn write<E>(
        &self,
        writer: &mut BinaryWriter,
    ) -> Result<(), NodeWireError<E>> {
        write_list(writer, &self.passes, StylePass::write)?;
        Ok(writer.write_i32(self.selected)?)
    }
}

impl<E: 'static> StyleReader<'_, '_, '_, E> {
    fn pass(&mut self) -> Result<StylePass, NodeWireError<E>> {
        let mode = self.reader.read_i32()?;
        let area = if self.boolean()? {
            Some(self.area()?)
        } else {
            None
        };
        let gradient_mask = self.gradient_field()?;
        let noise_mask = self.noise()?;
        let blend = self.blend()?;
        let paint = match mode {
            0 => Paint::Adjust(ColorAdjustment {
                argb: self.color()?,
                brightness: self.double()?,
                contrast: self.double()?,
                hue: self.double()?,
                saturation: self.double()?,
            }),
            1 => Paint::Environment(Box::new(self.environment()?)),
            2 => Paint::Refract(Box::new(RefractionPaint {
                bevel: self.bevel()?,
                index: self.double()?,
                depth: self.double()?,
                affects_height: self.boolean()?,
            })),
            3 => Paint::Blur {
                radius: self.double()?,
            },
            4 => {
                let argb = self.color()?;
                Paint::Solid {
                    argb,
                    transparency: if self.version > 3.16 {
                        self.double()?
                    } else {
                        0.
                    },
                }
            }
            6 | 8 => {
                let offset = [self.double()?, self.double()?];
                let rotation = self.integer()?;
                if mode == 6 {
                    Paint::Input { offset, rotation }
                } else {
                    Paint::Copy { offset, rotation }
                }
            }
            5 => Paint::Gradient(Box::new(self.gradient_paint()?)),
            7 => Paint::Noise(Box::new(NoisePaint {
                lab: self.noise_lab()?,
                gradient: self.gradient()?,
            })),
            9 => Paint::Synthesis(self.synthesis()?),
            _ => {
                return Err(NodeWireError::UnsupportedCapability(
                    "Style paint operation",
                ));
            }
        };
        Ok(StylePass {
            identity: Arc::new(()),
            inactive: InactiveControls::default(),
            area,
            gradient_mask,
            noise_mask,
            blend,
            paint,
        })
    }

    fn environment(&mut self) -> Result<EnvironmentPaint, NodeWireError<E>> {
        let bevel = self.bevel()?;
        let mut adjustment = None;
        let map = if self.version >= 3.7 || self.reader.read_bool()? {
            if self.version >= 3.72 || self.version == 3.71 || self.version == 3.7 {
                if self.reader.read_bool()? {
                    return Err(NodeWireError::UnsupportedCapability(
                        "Animated Style Environment",
                    ));
                }
                if self.reader.read_bool()? {
                    let mut record = AdjustmentRecord::new(&[[0., 0.]]);
                    record
                        .load(self.reader, self.version)
                        .map_err(crate::pointwise::LivePropertyWireError::Payload)?;
                    adjustment = Some(Arc::new(record));
                }
            }
            EnvironmentRecord::read(self.reader, self.version)
                .map_err(crate::pointwise::LivePropertyWireError::Environment)?
        } else {
            let mut map = EnvironmentRecord::default();
            map.add_layer(crate::EnvironmentLayerKind::Light(
                crate::EnvironmentLight::Soft,
            ))
            .map_err(|_| BinaryError::Allocation)?;
            map
        };
        Ok(EnvironmentPaint {
            bevel,
            map: Arc::new(map),
            adjustment,
            resolution: self.reader.read_i32()?,
            depth: self.double()?,
            affects_height: self.boolean()?,
            flatten_color: self.boolean()?,
        })
    }

    fn synthesis(&mut self) -> Result<SynthesisPaint, NodeWireError<E>> {
        let mut adjustment = None;
        if self.version >= 3.7 {
            if self.reader.read_bool()? {
                return Err(NodeWireError::UnsupportedCapability(
                    "Animated Style Synthesis Texture",
                ));
            }
            if self.reader.read_bool()? {
                let mut record = AdjustmentRecord::new(&[[0., 0.]]);
                record
                    .load(self.reader, self.version)
                    .map_err(crate::pointwise::LivePropertyWireError::Payload)?;
                if record.property_type != 10 {
                    return Err(NodeWireError::UnsupportedCapability(
                        "Style Synthesis Texture adjuster type",
                    ));
                }
                adjustment = Some(Arc::new(record));
            }
        }
        Ok(SynthesisPaint {
            local: Arc::new(SynthesisTexture::read(self.reader, self.version)?),
            adjustment,
            resolved: None,
        })
    }

    fn color(&mut self) -> Result<[f64; 4], NodeWireError<E>> {
        self.property_prefix()?;
        let mut argb = [0.; 4];
        for value in &mut argb {
            *value = self.reader.read_f64()?;
        }
        Ok(argb)
    }

    fn bevel(&mut self) -> Result<Bevel, NodeWireError<E>> {
        Ok(Bevel {
            feather: [self.double()?, self.double()?],
            curve: self.curve()?,
            noise: self.noise()?,
            inactive_noise: None,
        })
    }

    fn noise(&mut self) -> Result<Option<StyleNoise>, NodeWireError<E>> {
        if !self.boolean()? {
            return Ok(None);
        }
        Ok(Some(StyleNoise {
            strength: self.double()?,
            lab: self.noise_lab()?,
            affects_opacity: self.version <= 3.15 || self.boolean()?,
        }))
    }

    fn noise_lab(&mut self) -> Result<Arc<[u8]>, NodeWireError<E>> {
        if self.version < 3.71 {
            self.reader.read_f64()?;
        }
        let node = read_style_noise_lab(self.reader, self.version, self.context.as_deref_mut())?;
        let mut writer = BinaryWriter::default();
        node.save_binary(&mut writer)?;
        Ok(writer.bytes().into())
    }

    fn gradient_field(&mut self) -> Result<Option<GradientField>, NodeWireError<E>> {
        if !self.boolean()? {
            return Ok(None);
        }
        Ok(Some(GradientField {
            wrap: self.boolean()?,
            curve: self.curve()?,
            blur: self.double()?,
            angle: self.double()?,
            offset: self.pair()?,
            kind: self.integer()?,
        }))
    }

    fn gradient_paint(&mut self) -> Result<GradientPaint, NodeWireError<E>> {
        Ok(GradientPaint {
            inactive_noise: None,
            noise: self.noise()?,
            field: self.gradient_field()?,
            gradient: self.gradient()?,
        })
    }

    fn gradient(&mut self) -> Result<GradientRecord, NodeWireError<E>> {
        self.property_prefix()?;
        let mut gradient = GradientRecord::default();
        gradient.load(self.reader).map_err(gradient_error)?;
        Ok(gradient)
    }

    fn area(&mut self) -> Result<ShapeArea, NodeWireError<E>> {
        let offset = self.pair()?;
        let thickness = self.double()?;
        let hard_feather = self.double()?;
        let soft_feather = self.double()?;
        let feather_curve = self.curve()?;
        let outline = self.boolean()?;
        let outline_thickness = self.double()?;
        let mode = self.reader.read_i32()?;
        let secondary = if mode == 0 {
            None
        } else {
            Some(SecondaryArea {
                mode,
                offset: self.pair()?,
                hard_feather: self.double()?,
                soft_feather: self.double()?,
                curve: self.curve()?,
            })
        };
        Ok(ShapeArea {
            offset,
            thickness,
            hard_feather,
            soft_feather,
            feather_curve,
            outline,
            outline_thickness,
            secondary,
        })
    }

    fn rgb_blend(&mut self, retain_animations: bool) -> Result<Blend, NodeWireError<E>> {
        let mut animations = std::array::from_fn(|_| None);
        let mut links = std::array::from_fn(|_| None);
        let mut binding = |reader: &mut Self, slot| -> Result<(), NodeWireError<E>> {
            (animations[slot], links[slot]) = reader.rgb_binding(retain_animations, slot)?;
            Ok(())
        };
        binding(self, 0)?;
        let mode = self.reader.read_i32()?;
        binding(self, 1)?;
        let erosion_mode = self.reader.read_i32()?;
        binding(self, 2)?;
        let erosion_edges_only = self.reader.read_bool()?;
        binding(self, 3)?;
        let erosion = self.reader.read_f32()?;
        binding(self, 4)?;
        let erosion_sharpness = self.reader.read_f32()?;
        binding(self, 5)?;
        let opacity = self.reader.read_f64()?;
        let relief = [self.blend_relief()?, self.blend_relief()?];
        Ok(Blend {
            mode,
            erosion_mode,
            erosion_edges_only,
            erosion,
            erosion_sharpness,
            opacity,
            relief,
            skip_for_color: false,
            animations: PropertyAnimations::from_records(animations),
            synthesis_links: NumberLinks::from_records(links),
        })
    }

    fn rgb_binding(
        &mut self,
        retain_animations: bool,
        slot: usize,
    ) -> Result<(Option<AnimationRecord>, Option<NumericLink>), NodeWireError<E>> {
        if !retain_animations {
            self.property_prefix()?;
            return Ok((None, None));
        }
        if !(self.version >= 3.72 || self.version == 3.71 || self.version == 3.7) {
            return Ok((None, None));
        }
        if let Some(field) = SynthesisNumberField::BLEND_WIRE_FIELDS[slot] {
            let bounds = field.mapping_range().map(|value| value as f32);
            read_numeric_bindings(self.reader, self.version, &[bounds])
        } else {
            Ok((
                read_unadjusted_animation(self.reader, self.version, 1)?,
                None,
            ))
        }
    }

    fn blend(&mut self) -> Result<Blend, NodeWireError<E>> {
        let mut blend = self.rgb_blend(false)?;
        blend.skip_for_color = self.reader.read_bool()?;
        if self.version < 3.7 && !self.reader.read_bool()? {
            blend.mode += 1;
        }
        Ok(blend)
    }

    fn blend_relief(&mut self) -> Result<BlendRelief, NodeWireError<E>> {
        Ok(BlendRelief {
            mode: self.reader.read_i32()?,
            normal_blur: self.reader.read_f64()?,
            depth: self.reader.read_f64()?,
            offset_blur: self.reader.read_f64()?,
            direction: [self.reader.read_f64()?, self.reader.read_f64()?],
            distance: self.reader.read_f64()?,
        })
    }

    fn property_prefix(&mut self) -> Result<(), NodeWireError<E>> {
        if (self.version >= 3.72 || self.version == 3.71 || self.version == 3.7)
            && (self.reader.read_bool()? || self.reader.read_bool()?)
        {
            return Err(NodeWireError::UnsupportedCapability(
                "Animated or adjusted Style pass property",
            ));
        }
        Ok(())
    }

    fn boolean(&mut self) -> Result<bool, NodeWireError<E>> {
        self.property_prefix()?;
        Ok(self.reader.read_bool()?)
    }

    fn integer(&mut self) -> Result<i32, NodeWireError<E>> {
        self.property_prefix()?;
        Ok(self.reader.read_i32()?)
    }

    fn double(&mut self) -> Result<f64, NodeWireError<E>> {
        self.property_prefix()?;
        Ok(self.reader.read_f64()?)
    }

    fn pair(&mut self) -> Result<[f64; 2], NodeWireError<E>> {
        self.property_prefix()?;
        Ok([self.reader.read_f64()?, self.reader.read_f64()?])
    }

    fn curve(&mut self) -> Result<CurveRecord, NodeWireError<E>> {
        self.property_prefix()?;
        let mut curve = CurveRecord::new(0., 1., false);
        curve.load(self.reader, self.version).map_err(curve_error)?;
        Ok(curve)
    }
}

impl StylePass {
    pub(super) fn needs_prepared_area(&self) -> bool {
        self.gradient_mask
            .as_ref()
            .is_some_and(|field| field.kind == 0)
            || match &self.paint {
                Paint::Environment(_) | Paint::Refract(_) => true,
                Paint::Gradient(paint) => paint.field.as_ref().is_none_or(|field| field.kind == 0),
                _ => false,
            }
    }

    fn write<E>(&self, writer: &mut BinaryWriter) -> Result<(), NodeWireError<E>> {
        writer.write_i32(match self.paint {
            Paint::Adjust(_) => 0,
            Paint::Environment(_) => 1,
            Paint::Refract(_) => 2,
            Paint::Blur { .. } => 3,
            Paint::Solid { .. } => 4,
            Paint::Gradient(_) => 5,
            Paint::Input { .. } => 6,
            Paint::Noise(_) => 7,
            Paint::Copy { .. } => 8,
            Paint::Synthesis(_) => 9,
        })?;
        write_property(writer, |writer| writer.write_bool(self.area.is_some()))?;
        if let Some(area) = &self.area {
            area.write(writer)?;
        }
        write_gradient_field(writer, self.gradient_mask.as_ref())?;
        write_noise(writer, self.noise_mask.as_ref())?;
        self.blend.write(writer)?;
        match &self.paint {
            Paint::Adjust(adjust) => {
                write_color(writer, adjust.argb)?;
                for value in [
                    adjust.brightness,
                    adjust.contrast,
                    adjust.hue,
                    adjust.saturation,
                ] {
                    write_property(writer, |writer| writer.write_f64(value))?;
                }
            }
            Paint::Environment(paint) => paint.write(writer)?,
            Paint::Refract(paint) => {
                paint.bevel.write(writer)?;
                write_property(writer, |writer| writer.write_f64(paint.index))?;
                write_property(writer, |writer| writer.write_f64(paint.depth))?;
                write_property(writer, |writer| writer.write_bool(paint.affects_height))?;
            }
            Paint::Blur { radius } => write_property(writer, |writer| writer.write_f64(*radius))?,
            Paint::Gradient(paint) => {
                write_noise(writer, paint.noise.as_ref())?;
                write_gradient_field(writer, paint.field.as_ref())?;
                write_gradient(writer, &paint.gradient)?;
            }
            Paint::Noise(paint) => {
                writer.write_bytes(&paint.lab)?;
                write_gradient(writer, &paint.gradient)?;
            }
            Paint::Solid { argb, transparency } => {
                write_color(writer, *argb)?;
                write_property(writer, |writer| writer.write_f64(*transparency))?;
            }
            Paint::Synthesis(paint) => {
                writer.write_bool(false)?;
                writer.write_bool(paint.adjustment.is_some())?;
                if let Some(adjustment) = &paint.adjustment {
                    (**adjustment)
                        .clone()
                        .save(writer)
                        .map_err(crate::pointwise::LivePropertyWireError::Payload)?;
                }
                paint.texture().write(writer)?;
            }
            Paint::Input { offset, rotation } | Paint::Copy { offset, rotation } => {
                for value in offset {
                    write_property(writer, |writer| writer.write_f64(*value))?;
                }
                write_property(writer, |writer| writer.write_i32(*rotation))?;
            }
        }
        Ok(())
    }
}

impl EnvironmentPaint {
    pub(super) fn adjuster_key(&self) -> Option<([u8; 16], &[u16])> {
        let record = self.adjustment.as_ref()?;
        let component = record.components().first()?;
        (record.property_type == 9 && component.active && !component.name.is_empty())
            .then_some((record.guid, component.name.as_slice()))
    }

    fn write<E>(&self, writer: &mut BinaryWriter) -> Result<(), NodeWireError<E>> {
        self.bevel.write(writer)?;
        writer.write_bool(false)?;
        writer.write_bool(self.adjustment.is_some())?;
        if let Some(adjustment) = &self.adjustment {
            (**adjustment)
                .clone()
                .save(writer)
                .map_err(crate::pointwise::LivePropertyWireError::Payload)?;
        }
        self.map
            .save(writer)
            .map_err(crate::pointwise::LivePropertyWireError::Environment)?;
        writer.write_i32(self.resolution)?;
        write_property(writer, |writer| writer.write_f64(self.depth))?;
        write_property(writer, |writer| writer.write_bool(self.affects_height))?;
        Ok(write_property(writer, |writer| {
            writer.write_bool(self.flatten_color)
        })?)
    }
}

impl Bevel {
    fn write<E>(&self, writer: &mut BinaryWriter) -> Result<(), NodeWireError<E>> {
        for value in self.feather {
            write_property(writer, |writer| writer.write_f64(value))?;
        }
        write_curve(writer, &self.curve)?;
        Ok(write_noise(writer, self.noise.as_ref())?)
    }
}

impl ShapeArea {
    fn write<E>(&self, writer: &mut BinaryWriter) -> Result<(), NodeWireError<E>> {
        write_pair(writer, self.offset)?;
        for value in [self.thickness, self.hard_feather, self.soft_feather] {
            write_property(writer, |writer| writer.write_f64(value))?;
        }
        write_curve(writer, &self.feather_curve)?;
        write_property(writer, |writer| writer.write_bool(self.outline))?;
        write_property(writer, |writer| writer.write_f64(self.outline_thickness))?;
        writer.write_i32(self.secondary.as_ref().map_or(0, |area| area.mode))?;
        if let Some(area) = &self.secondary {
            write_pair(writer, area.offset)?;
            for value in [area.hard_feather, area.soft_feather] {
                write_property(writer, |writer| writer.write_f64(value))?;
            }
            write_curve(writer, &area.curve)?;
        }
        Ok(())
    }

    pub(super) fn is_plain(&self) -> bool {
        self.offset == [0.; 2]
            && self.thickness == 0.
            && self.hard_feather == 0.
            && self.soft_feather == 0.
            && self.feather_curve.layers.is_empty()
            && !self.outline
            && self.secondary.is_none()
    }
}

impl Blend {
    pub(super) fn read_synthesis<E: 'static>(
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<Self, NodeWireError<E>> {
        let mut blend = StyleReader {
            reader,
            context: None,
            version,
        }
        .rgb_blend(true)?;
        blend.mode = blend.mode.checked_add(1).ok_or(BinaryError::Count)?;
        Ok(blend)
    }

    pub(super) fn write_synthesis<E>(
        &self,
        writer: &mut BinaryWriter,
    ) -> Result<(), NodeWireError<E>> {
        self.write_rgb(writer, self.mode - 1)
    }

    pub(super) fn has_spatial_shading(&self) -> bool {
        self.mode != 0
            && self
                .relief
                .iter()
                .any(|effect| matches!(effect.mode, 1 | 2))
    }

    pub(super) fn for_named_map(&self, existing: bool) -> Self {
        let mut blend = self.clone();
        if existing {
            blend.mode = self.mode.max(1);
        } else {
            blend.mode = 1;
            blend.erosion = 0.;
            blend.relief = [BlendRelief::default(); 2];
        }
        blend
    }

    fn write_rgb<E>(&self, writer: &mut BinaryWriter, mode: i32) -> Result<(), NodeWireError<E>> {
        for (index, value) in [mode, self.erosion_mode].into_iter().enumerate() {
            write_animated_property(
                writer,
                self.animations.get(index),
                self.synthesis_links.get(index),
                |writer| writer.write_i32(value),
            )?;
        }
        write_animated_property(writer, self.animations.get(2), None, |writer| {
            writer.write_bool(self.erosion_edges_only)
        })?;
        for (index, value) in [self.erosion, self.erosion_sharpness]
            .into_iter()
            .enumerate()
        {
            let slot = 3 + index;
            write_animated_property(
                writer,
                self.animations.get(slot),
                self.synthesis_links.get(slot),
                |writer| writer.write_f32(value),
            )?;
        }
        write_animated_property(
            writer,
            self.animations.get(5),
            self.synthesis_links.get(5),
            |writer| writer.write_f64(self.opacity),
        )?;
        for effect in self.relief {
            writer.write_i32(effect.mode)?;
            for value in [
                effect.normal_blur,
                effect.depth,
                effect.offset_blur,
                effect.direction[0],
                effect.direction[1],
                effect.distance,
            ] {
                writer.write_f64(value)?;
            }
        }
        Ok(())
    }

    fn write<E>(&self, writer: &mut BinaryWriter) -> Result<(), NodeWireError<E>> {
        self.write_rgb(writer, self.mode)?;
        Ok(writer.write_bool(self.skip_for_color)?)
    }
}

fn write_color(writer: &mut BinaryWriter, argb: [f64; 4]) -> Result<(), BinaryError> {
    write_property(writer, |writer| {
        for value in argb {
            writer.write_f64(value)?;
        }
        Ok(())
    })
}

fn write_gradient<E>(
    writer: &mut BinaryWriter,
    gradient: &GradientRecord,
) -> Result<(), NodeWireError<E>> {
    write_property(writer, |_| Ok(()))?;
    gradient.save(writer).map_err(gradient_error)
}

fn write_noise(writer: &mut BinaryWriter, noise: Option<&StyleNoise>) -> Result<(), BinaryError> {
    write_property(writer, |writer| writer.write_bool(noise.is_some()))?;
    if let Some(noise) = noise {
        write_property(writer, |writer| writer.write_f64(noise.strength))?;
        writer.write_bytes(&noise.lab)?;
        write_property(writer, |writer| writer.write_bool(noise.affects_opacity))?;
    }
    Ok(())
}

fn write_gradient_field<E>(
    writer: &mut BinaryWriter,
    field: Option<&GradientField>,
) -> Result<(), NodeWireError<E>> {
    write_property(writer, |writer| writer.write_bool(field.is_some()))?;
    if let Some(field) = field {
        write_property(writer, |writer| writer.write_bool(field.wrap))?;
        write_curve(writer, &field.curve)?;
        write_property(writer, |writer| writer.write_f64(field.blur))?;
        write_property(writer, |writer| writer.write_f64(field.angle))?;
        write_pair(writer, field.offset)?;
        write_property(writer, |writer| writer.write_i32(field.kind))?;
    }
    Ok(())
}

fn gradient_error<E>(error: PropertyRecordError) -> NodeWireError<E> {
    match error {
        PropertyRecordError::Binary(error) => error.into(),
        PropertyRecordError::Allocation => BinaryError::Allocation.into(),
        _ => NodeWireError::UnsupportedCapability("Style gradient record"),
    }
}

pub(super) fn write_property(
    writer: &mut BinaryWriter,
    write_value: impl FnOnce(&mut BinaryWriter) -> Result<(), BinaryError>,
) -> Result<(), BinaryError> {
    writer.write_bool(false)?;
    writer.write_bool(false)?;
    write_value(writer)
}

fn write_pair(writer: &mut BinaryWriter, [x, y]: [f64; 2]) -> Result<(), BinaryError> {
    write_property(writer, |writer| {
        writer.write_f64(x)?;
        writer.write_f64(y)
    })
}

fn write_curve<E>(writer: &mut BinaryWriter, curve: &CurveRecord) -> Result<(), NodeWireError<E>> {
    writer.write_bool(false)?;
    writer.write_bool(false)?;
    curve.save(writer).map_err(curve_error)
}

fn curve_error<E>(error: crate::CurveRecordError) -> NodeWireError<E> {
    match error {
        crate::CurveRecordError::Binary(error) => error.into(),
        _ => NodeWireError::UnsupportedCapability("Canvas mask curve record"),
    }
}
