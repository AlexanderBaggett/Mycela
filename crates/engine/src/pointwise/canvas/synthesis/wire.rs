use super::{
    Layer, LayerColor, Master, NumberLinks, NumericLink, Paint, PropertyAnimations, SolidPaint,
    Source, SynthesisColorField, SynthesisNumberField, SynthesisTexture,
    color_link::ColorLink,
    patches::{Automatic, Knot, Patches, Placement, Selection},
    photo_blend::PhotoBlend,
    repeat::Pattern,
};
use crate::environment_record::ImageAsset;
use crate::pointwise::{
    LivePropertyWireError, NodeWireError, PointwiseError, PointwiseFilter,
    canvas::style::Blend,
    property::wire::{AdjustmentSpec, check_code},
};
use crate::{AdjustmentRecord, AnimationRecord, BinaryCursor, BinaryError, BinaryWriter};
use std::sync::Arc;

type StoredColor = (Option<AnimationRecord>, Option<ColorLink>, [f64; 4]);

#[cfg(test)]
#[path = "wire/legacy_tests.rs"]
mod legacy_tests;

#[cfg(test)]
#[path = "wire/animation_tests.rs"]
mod animation_tests;

#[cfg(test)]
#[path = "wire/corner_tests.rs"]
mod corner_tests;

#[cfg(test)]
#[path = "wire/master_auto_animation_tests.rs"]
mod master_auto_animation_tests;

impl<E: 'static> PointwiseFilter<E> {
    pub(in crate::pointwise) fn load_legacy_synthesis_properties(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), NodeWireError<E>> {
        // The old node loads Scale and Texture in that order. The texture
        // setter supplies both final controls even though Scale is also on wire.
        self.parameter(0)?.load_binary(reader, version)?;
        self.load_synthesis_texture(reader, version)
    }

    pub(in crate::pointwise) fn load_synthesis_texture(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), NodeWireError<E>> {
        check_code(reader, &"Texture".encode_utf16().collect::<Vec<_>>(), false)?;
        let texture = SynthesisTexture::read(reader, version)?;
        self.synthesis_wire()
            .ok_or(PointwiseError::InvalidCast)?
            .load_tail(
                reader,
                version,
                || Ok(None),
                || {
                    Ok(AdjustmentSpec {
                        property_type: 10,
                        count: 1,
                        limits: None,
                    })
                },
            )?;
        let controls = [texture.master.scale, f64::from(texture.master.seed)];
        self.set_synthesis_texture(texture);
        if version < 3.58 {
            self.parameter(0)?.set_number(controls[0])?;
            self.parameter(1)?.set_number(controls[1])?;
        }
        Ok(())
    }
}

impl SynthesisTexture {
    pub(in crate::pointwise) fn read<E: 'static>(
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<Self, NodeWireError<E>> {
        if version < 3.02 {
            return Err(NodeWireError::UnsupportedCapability(
                "Legacy Synthesis texture record",
            ));
        }
        let mut wire = SynthesisReader { reader, version };
        let master = wire.master()?;
        let mut layers = Vec::new();
        let count = wire.count()?;
        for _ in 0..count {
            let tag = wire.reader.read_i32()?;
            let blend = Blend::read_synthesis(wire.reader, version)?;
            let color_controls = wire.layer_color()?;
            let paint = match tag {
                3 => {
                    let (animation, color_adjustment, color) = wire.color_property()?;
                    Paint::Solid(SolidPaint {
                        color,
                        color_adjustment,
                        animations: PropertyAnimations::from_records([animation]),
                    })
                }
                2 => Paint::Repeat(Box::new(wire.repeat_pattern(version)?)),
                1 => Paint::Blend(Box::new(wire.photo_blend(version)?)),
                0 => Paint::Patches(Box::new(wire.patches(version)?)),
                _ => return Err(NodeWireError::UnsupportedCapability("Synthesis layer type")),
            };
            layers.try_reserve(1).map_err(|_| BinaryError::Allocation)?;
            layers.push(Arc::new(Layer {
                identity: Arc::new(()),
                blend,
                color_controls,
                paint,
            }));
        }
        Ok(Self { master, layers })
    }

    pub(in crate::pointwise) fn write<E>(
        &self,
        writer: &mut BinaryWriter,
    ) -> Result<(), NodeWireError<E>> {
        let master = &self.master;
        for (index, value) in [
            master.brightness,
            master.contrast,
            master.hue,
            master.saturation,
            master.sharpen,
            master.sharpen_radius,
        ]
        .into_iter()
        .enumerate()
        {
            write_animated_property(
                writer,
                master.animations.get(index),
                master.color_links.get(index),
                |wire| wire.write_f32(value),
            )?;
        }
        write_animated_property(
            writer,
            master.animations.get(6),
            master.scale_adjustment.as_ref(),
            |wire| wire.write_f64(master.scale),
        )?;
        write_animated_property(
            writer,
            master.animations.get(7),
            master.seed_adjustment.as_ref(),
            |wire| wire.write_i32(master.seed),
        )?;
        write_animated_color(
            writer,
            master.color,
            master.animations.get(8),
            master.color_adjustment.as_ref(),
        )?;
        writer.write_i32(i32::try_from(self.layers.len()).map_err(|_| BinaryError::Count)?)?;
        for layer in &self.layers {
            writer.write_i32(match layer.paint {
                Paint::Solid(_) => 3,
                Paint::Repeat(_) => 2,
                Paint::Blend(_) => 1,
                Paint::Patches(_) => 0,
            })?;
            layer.blend.write_synthesis(writer)?;
            let colors = &layer.color_controls;
            write_animated_color(
                writer,
                colors.tint,
                colors.animations.get(0),
                colors.tint_adjustment.as_ref(),
            )?;
            for (index, value) in layer.color_controls.adjustments.into_iter().enumerate() {
                write_animated_property(
                    writer,
                    colors.animations.get(index + 1),
                    colors.number_links.get(index),
                    |wire| wire.write_f32(value),
                )?;
            }
            match &layer.paint {
                Paint::Solid(paint) => write_animated_color(
                    writer,
                    paint.color,
                    paint.animations.get(0),
                    paint.color_adjustment.as_ref(),
                )?,
                Paint::Repeat(pattern) => {
                    let animation = |slot| pattern.animations.get(slot);
                    write_source(writer, &pattern.source, animation(0))?;
                    writer.write_i32(
                        i32::try_from(pattern.columns).map_err(|_| BinaryError::Count)?,
                    )?;
                    writer
                        .write_i32(i32::try_from(pattern.rows).map_err(|_| BinaryError::Count)?)?;
                    for point in &pattern.points {
                        writer.write_f32(point[0])?;
                        writer.write_f32(point[1])?;
                    }
                    write_animated_property(writer, animation(1), None, |wire| {
                        wire.write_bool(pattern.equalize)
                    })?;
                    write_animated_property(
                        writer,
                        animation(2),
                        pattern.strength_adjustment.as_ref(),
                        |wire| wire.write_f64(pattern.strength),
                    )?;
                    write_animated_property(
                        writer,
                        animation(3),
                        pattern.feather_adjustment.as_ref(),
                        |wire| wire.write_f64(pattern.feather),
                    )?;
                    write_animated_property(
                        writer,
                        animation(4),
                        pattern.frequency_adjustment.as_ref(),
                        |wire| wire.write_i32(pattern.frequency),
                    )?;
                }
                Paint::Blend(blend) => {
                    let animation = |slot| blend.animations.get(slot);
                    write_source(writer, &blend.source, animation(0))?;
                    for value in blend.rectangle {
                        writer.write_f32(value)?;
                    }
                    write_animated_property(writer, animation(1), None, |wire| {
                        wire.write_bool(blend.equalize)
                    })?;
                    write_animated_property(
                        writer,
                        animation(2),
                        blend.strength_adjustment.as_ref(),
                        |wire| wire.write_f64(blend.strength),
                    )?;
                    write_animated_property(writer, animation(3), None, |wire| {
                        wire.write_i32(blend.mode)
                    })?;
                    for (index, value) in blend
                        .stretch
                        .into_iter()
                        .chain(blend.edge_feather)
                        .chain([blend.star_feather])
                        .enumerate()
                    {
                        write_animated_property(
                            writer,
                            animation(index + 4),
                            blend.shaping_links.get(index),
                            |wire| wire.write_f64(value),
                        )?;
                    }
                    write_animated_property(
                        writer,
                        animation(9),
                        blend.repeat_adjustment.as_ref(),
                        |wire| {
                            wire.write_i32(blend.repeat[0])?;
                            wire.write_i32(blend.repeat[1])
                        },
                    )?;
                }
                Paint::Patches(patches) => write_patches(writer, patches)?,
            }
        }
        Ok(())
    }
}

impl<'data> SynthesisReader<'_, 'data> {
    fn master<E>(&mut self) -> Result<Master, NodeWireError<E>> {
        let mut animations = std::array::from_fn(|_| None);
        let mut scalars = [0.; 6];
        let mut links = std::array::from_fn(|_| None);
        for (slot, field) in SynthesisColorField::MASTER.into_iter().enumerate() {
            let bounds = SynthesisNumberField::MasterColor(field)
                .mapping_range()
                .map(|bound| bound as f32);
            (animations[slot], links[slot]) = self.numeric_bindings(&[bounds])?;
            scalars[slot] = self.reader.read_f32()?;
        }
        let [
            brightness,
            contrast,
            hue,
            saturation,
            sharpen,
            sharpen_radius,
        ] = scalars;
        let scale_adjustment;
        (animations[6], scale_adjustment) = self.numeric_bindings(&[[-10., 10.]])?;
        let scale = self.reader.read_f64()?;
        let (seed_animation, seed_adjustment) = self.numeric_bindings(&[[0., 1000.]])?;
        animations[7] = seed_animation;
        let seed = self.reader.read_i32()?;
        let (color_animation, color_adjustment, color) = self.color_property()?;
        animations[8] = color_animation;
        Ok(Master {
            brightness,
            contrast,
            hue,
            saturation,
            sharpen,
            sharpen_radius,
            scale,
            scale_adjustment,
            seed,
            seed_adjustment,
            color,
            color_adjustment,
            color_links: NumberLinks::from_records(links),
            animations: PropertyAnimations::from_records(animations),
        })
    }
}

fn write_patches<E>(writer: &mut BinaryWriter, patches: &Patches) -> Result<(), NodeWireError<E>> {
    let animation = |slot| patches.animations.get(slot);
    write_source(writer, &patches.source, animation(0))?;
    write_animated_property(writer, animation(1), None, |wire| {
        wire.write_bool(patches.equalize)
    })?;
    write_animated_property(
        writer,
        animation(2),
        patches.strength_adjustment.as_ref(),
        |wire| wire.write_f64(patches.strength),
    )?;
    let selections = if patches.selections_parked {
        &[][..]
    } else {
        &patches.selections
    };
    writer.write_i32(i32::try_from(selections.len()).map_err(|_| BinaryError::Count)?)?;
    for selection in selections {
        let animation = |slot| selection.animations.get(slot);
        write_animated_property(
            writer,
            animation(0),
            selection.number_links.get(0),
            |wire| wire.write_i32(selection.frequency),
        )?;
        let shape = if matches!(patches.source, Source::Input) {
            [1, 2].map(|slot| selection.effective_number(slot) as f32)
        } else {
            [selection.feather, selection.jitter]
        };
        for (index, value) in shape.into_iter().enumerate() {
            write_animated_property(
                writer,
                animation(index + 1),
                selection.number_links.get(index + 1),
                |wire| wire.write_f32(value),
            )?;
        }
        write_animated_property(
            writer,
            animation(3),
            selection.number_links.get(3),
            |wire| wire.write_i32(selection.jitter_frequency),
        )?;
        for (index, value) in [selection.shadow_strength, selection.shadow_feather]
            .into_iter()
            .enumerate()
        {
            write_animated_property(
                writer,
                animation(index + 4),
                selection.number_links.get(index + 4),
                |wire| wire.write_f32(value),
            )?;
        }
        writer.write_i32(i32::try_from(selection.knots.len()).map_err(|_| BinaryError::Count)?)?;
        for (index, knot) in selection.knots.iter().enumerate() {
            let handles = Knot::canonical_handles(&selection.knots, index);
            for value in knot.point.into_iter().chain(handles.into_iter().flatten()) {
                writer.write_f32(value)?;
            }
            writer.write_i32(knot.mode)?;
            for flag in knot.flags {
                writer.write_bool(flag)?;
            }
        }
        for flip in selection.flipped {
            writer.write_bool(flip)?;
        }
    }
    let auto = &patches.automatic;
    writer.write_bool(auto.enabled)?;
    write_animated_property(
        writer,
        auto.animations.get(0),
        auto.frequency_adjustment.as_ref(),
        |wire| {
            wire.write_i32(auto.frequency[0])?;
            wire.write_i32(auto.frequency[1])
        },
    )?;
    write_animated_property(
        writer,
        auto.animations.get(1),
        auto.offset_adjustment.as_ref(),
        |wire| {
            wire.write_f32(auto.offset[0])?;
            wire.write_f32(auto.offset[1])
        },
    )?;
    for (index, (value, adjustment)) in [
        (auto.density, &auto.density_adjustment),
        (auto.slide, &auto.slide_adjustment),
        (
            auto.position_jitter[0],
            &auto.position_jitter_adjustments[0],
        ),
        (
            auto.position_jitter[1],
            &auto.position_jitter_adjustments[1],
        ),
        (auto.scale, &auto.scale_adjustment),
        (auto.scale_jitter, &auto.scale_jitter_adjustment),
        (auto.angle, &auto.angle_adjustment),
        (auto.angle_jitter, &auto.angle_jitter_adjustment),
    ]
    .into_iter()
    .enumerate()
    {
        write_animated_property(
            writer,
            auto.animations.get(index + 2),
            adjustment.as_ref(),
            |wire| wire.write_f32(value),
        )?;
    }
    write_animated_property(
        writer,
        auto.animations.get(10),
        auto.switcher_adjustment.as_ref(),
        |wire| wire.write_i32(auto.switcher),
    )?;
    let manual = if patches.selections_parked {
        &[][..]
    } else {
        &patches.manual
    };
    writer.write_i32(i32::try_from(manual.len()).map_err(|_| BinaryError::Count)?)?;
    for placement in manual {
        writer.write_f32(placement.point[0])?;
        writer.write_f32(placement.point[1])?;
        writer.write_i32(placement.patch)?;
        writer.write_i32(placement.sequence)?;
        writer.write_f32(placement.angle)?;
        writer.write_f32(placement.size[0])?;
        writer.write_f32(placement.size[1])?;
    }
    write_animated_property(
        writer,
        animation(3),
        patches.repeat_adjustment.as_ref(),
        |writer| {
            writer.write_i32(patches.repeat[0])?;
            writer.write_i32(patches.repeat[1])
        },
    )?;
    Ok(())
}

pub(in crate::pointwise::canvas) fn write_animated_property<E>(
    writer: &mut BinaryWriter,
    animation: Option<&AnimationRecord>,
    adjustment: Option<&NumericLink>,
    write: impl FnOnce(&mut BinaryWriter) -> Result<(), BinaryError>,
) -> Result<(), NodeWireError<E>> {
    write_property(
        writer,
        animation,
        adjustment.map(NumericLink::record),
        write,
    )
}

fn write_property<E>(
    writer: &mut BinaryWriter,
    animation: Option<&AnimationRecord>,
    adjustment: Option<&AdjustmentRecord>,
    write: impl FnOnce(&mut BinaryWriter) -> Result<(), BinaryError>,
) -> Result<(), NodeWireError<E>> {
    writer.write_bool(animation.is_some())?;
    if let Some(animation) = animation {
        animation
            .save(writer)
            .map_err(LivePropertyWireError::Payload)?;
    }
    writer.write_bool(adjustment.is_some())?;
    if let Some(adjustment) = adjustment {
        adjustment
            .clone()
            .save(writer)
            .map_err(LivePropertyWireError::Payload)?;
    }
    Ok(write(writer)?)
}

fn write_source<E>(
    writer: &mut BinaryWriter,
    source: &Source,
    animation: Option<&AnimationRecord>,
) -> Result<(), NodeWireError<E>> {
    write_animated_property(writer, animation, None, |wire| {
        wire.write_bool(matches!(source, Source::Input))
    })?;
    if !matches!(source, Source::Input) {
        writer.write_bool(matches!(source, Source::Asset(_)))?;
    }
    if let Source::Asset(asset) = source {
        asset
            .save(writer)
            .map_err(LivePropertyWireError::Environment)?;
    }
    Ok(())
}

fn write_animated_color<E>(
    writer: &mut BinaryWriter,
    color: [f64; 4],
    animation: Option<&AnimationRecord>,
    adjustment: Option<&ColorLink>,
) -> Result<(), NodeWireError<E>> {
    write_property(
        writer,
        animation,
        adjustment.map(ColorLink::record),
        |wire| {
            for value in color {
                wire.write_f64(value)?;
            }
            Ok(())
        },
    )
}

fn read_raw_color(reader: &mut BinaryCursor<'_>) -> Result<[f64; 4], BinaryError> {
    let mut color = [0.; 4];
    for value in &mut color {
        *value = reader.read_f64()?;
    }
    Ok(color)
}

fn read_raw_pair(reader: &mut BinaryCursor<'_>) -> Result<[i32; 2], BinaryError> {
    Ok([reader.read_i32()?, reader.read_i32()?])
}
struct SynthesisReader<'a, 'b> {
    reader: &'a mut BinaryCursor<'b>,
    version: f64,
}

pub(in crate::pointwise::canvas) fn read_unadjusted_animation<E>(
    reader: &mut BinaryCursor<'_>,
    version: f64,
    slots: usize,
) -> Result<Option<AnimationRecord>, NodeWireError<E>> {
    SynthesisReader { reader, version }.animation_only(slots)
}

pub(in crate::pointwise::canvas) fn read_numeric_bindings<E>(
    reader: &mut BinaryCursor<'_>,
    version: f64,
    bounds: &[[f32; 2]],
) -> Result<(Option<AnimationRecord>, Option<NumericLink>), NodeWireError<E>> {
    SynthesisReader { reader, version }.numeric_bindings(bounds)
}

impl<'data> SynthesisReader<'_, 'data> {
    fn animation<E>(&mut self, slots: usize) -> Result<Option<AnimationRecord>, NodeWireError<E>> {
        if self.version < 3.7 || !self.reader.read_bool()? {
            return Ok(None);
        }
        let mut animation = AnimationRecord::new(slots);
        animation
            .load(self.reader, self.version)
            .map_err(LivePropertyWireError::Payload)?;
        Ok(Some(animation))
    }

    fn animation_only<E>(
        &mut self,
        slots: usize,
    ) -> Result<Option<AnimationRecord>, NodeWireError<E>> {
        let animation = self.animation(slots)?;
        if self.version >= 3.7 && self.reader.read_bool()? {
            return Err(NodeWireError::UnsupportedCapability(
                "Adjusted Synthesis layer property",
            ));
        }
        Ok(animation)
    }

    fn stored_property<T, E>(
        &mut self,
        animation: &mut Option<AnimationRecord>,
        slots: usize,
        read: impl FnOnce(&mut BinaryCursor<'data>) -> Result<T, BinaryError>,
    ) -> Result<T, NodeWireError<E>> {
        *animation = self.animation_only(slots)?;
        Ok(read(self.reader)?)
    }

    fn numeric_bindings<E>(
        &mut self,
        bounds: &[[f32; 2]],
    ) -> Result<(Option<AnimationRecord>, Option<NumericLink>), NodeWireError<E>> {
        let animation = self.animation(bounds.len())?;
        if self.version < 3.7 || !self.reader.read_bool()? {
            return Ok((animation, None));
        }
        let mut record = AdjustmentRecord::new(bounds);
        record
            .load(self.reader, self.version)
            .map_err(LivePropertyWireError::Payload)?;
        if !matches!(record.property_type, 0 | 4)
            && !(bounds.len() == 2 && record.property_type == 1)
        {
            return Err(NodeWireError::UnsupportedCapability(
                "Non-numeric Synthesis layer adjustment",
            ));
        }
        Ok((animation, Some(NumericLink::new(record))))
    }

    fn color_property<E>(&mut self) -> Result<StoredColor, NodeWireError<E>> {
        let animation = self.animation(1)?;
        let adjustment = if self.version >= 3.7 && self.reader.read_bool()? {
            let mut record = AdjustmentRecord::new(&[[0., 1.]]);
            record
                .load(self.reader, self.version)
                .map_err(LivePropertyWireError::Payload)?;
            if record.property_type != 6 {
                return Err(NodeWireError::UnsupportedCapability(
                    "Non-Color Synthesis adjustment",
                ));
            }
            Some(ColorLink::new(record))
        } else {
            None
        };
        Ok((animation, adjustment, read_raw_color(self.reader)?))
    }

    fn layer_color<E>(&mut self) -> Result<LayerColor, NodeWireError<E>> {
        let mut animations = std::array::from_fn(|_| None);
        let (tint_animation, tint_adjustment, tint) = self.color_property()?;
        animations[0] = tint_animation;
        let mut adjustments = [0.; 6];
        let mut links = std::array::from_fn(|_| None);
        for (slot, field) in SynthesisColorField::LAYER.into_iter().enumerate() {
            let bounds = SynthesisNumberField::LayerColor(field)
                .mapping_range()
                .map(|bound| bound as f32);
            (animations[slot + 1], links[slot]) = self.numeric_bindings(&[bounds])?;
            adjustments[slot] = self.reader.read_f32()?;
        }
        Ok(LayerColor {
            tint,
            tint_adjustment,
            adjustments,
            animations: PropertyAnimations::from_records(animations),
            number_links: NumberLinks::from_records(links),
        })
    }

    fn count(&mut self) -> Result<i32, BinaryError> {
        let count = self.reader.read_i32()?;
        if count < 0 {
            return Err(BinaryError::Count);
        }
        Ok(count)
    }

    fn source<E>(
        &mut self,
        version: f64,
        animation: &mut Option<AnimationRecord>,
    ) -> Result<Source, NodeWireError<E>> {
        if self.stored_property(animation, 1, BinaryCursor::read_bool)? {
            Ok(Source::Input)
        } else if self.reader.read_bool()? {
            Ok(Source::Asset(Arc::new(
                ImageAsset::read(self.reader, version)
                    .map_err(LivePropertyWireError::Environment)?,
            )))
        } else {
            Ok(Source::Missing)
        }
    }

    fn strength<E>(
        &mut self,
        animation: &mut Option<AnimationRecord>,
    ) -> Result<(f64, Option<NumericLink>), NodeWireError<E>> {
        let (curve, adjustment) = self.numeric_bindings(&[[0., 100.]])?;
        *animation = curve;
        let amount = self.reader.read_f64()?;
        let amount = if self.version < 3.7 {
            100. - amount
        } else {
            amount
        };
        Ok((amount, adjustment))
    }

    fn repeat_pattern<E>(&mut self, version: f64) -> Result<Pattern, NodeWireError<E>> {
        let mut animations = std::array::from_fn(|_| None);
        let source = self.source(version, &mut animations[0])?;
        let columns = self.count()? as usize;
        let rows = self.count()? as usize;
        let length = columns.checked_mul(rows).ok_or(BinaryError::Count)?;
        let available = self.reader.length()? - self.reader.position()?;
        if length > available as usize / 8 {
            return Err(BinaryError::End.into());
        }
        let mut points = Vec::new();
        points
            .try_reserve_exact(length)
            .map_err(|_| BinaryError::Allocation)?;
        for _ in 0..length {
            points.push([self.reader.read_f32()?, self.reader.read_f32()?]);
        }
        let equalize = self.stored_property(&mut animations[1], 1, BinaryCursor::read_bool)?;
        let (strength, strength_adjustment) = self.strength(&mut animations[2])?;
        let (feather_animation, feather_adjustment) = self.numeric_bindings(&[[0., 30.]])?;
        animations[3] = feather_animation;
        let feather = self.reader.read_f64()?;
        let (frequency_animation, frequency_adjustment) = self.numeric_bindings(&[[1., 100.]])?;
        animations[4] = frequency_animation;
        let frequency = self.reader.read_i32()?;
        Ok(Pattern {
            source,
            columns,
            rows,
            points: points.into_boxed_slice(),
            equalize,
            strength,
            strength_adjustment,
            feather: if version < 3.7 {
                legacy_repeat_feather(feather)
            } else {
                feather
            },
            feather_adjustment,
            frequency,
            frequency_adjustment,
            animations: PropertyAnimations::from_records(animations),
        })
    }

    fn photo_blend<E>(&mut self, version: f64) -> Result<PhotoBlend, NodeWireError<E>> {
        let mut animations = std::array::from_fn(|_| None);
        let source = self.source(version, &mut animations[0])?;
        let mut rectangle = [0.; 4];
        for value in &mut rectangle {
            *value = self.reader.read_f32()?;
        }
        let equalize = self.stored_property(&mut animations[1], 1, BinaryCursor::read_bool)?;
        let (strength, strength_adjustment) = self.strength(&mut animations[2])?;
        let mode = self.stored_property(&mut animations[3], 1, BinaryCursor::read_i32)?;
        let mut shaping = [0.; 5];
        let mut shaping_links = std::array::from_fn(|_| None);
        for (index, value) in shaping.iter_mut().enumerate() {
            (animations[index + 4], shaping_links[index]) = self.numeric_bindings(&[[0., 100.]])?;
            *value = self.reader.read_f64()?;
        }
        let [stretch_x, stretch_y, feather_x, feather_y, star_feather] = shaping;
        let (repeat_animation, repeat_adjustment) = self.numeric_bindings(&[[0., 100.]; 2])?;
        animations[9] = repeat_animation;
        let repeat = read_raw_pair(self.reader)?;
        // Blend Mode is constructed at OneSide and omitted from saved properties.
        Ok(PhotoBlend {
            source,
            rectangle,
            equalize,
            strength,
            strength_adjustment,
            mode,
            stretch: [stretch_x, stretch_y],
            edge_feather: [feather_x, feather_y],
            star_feather,
            shaping_links: NumberLinks::from_records(shaping_links),
            repeat,
            repeat_adjustment,
            animations: PropertyAnimations::from_records(animations),
        })
    }

    fn patches<E>(&mut self, version: f64) -> Result<Patches, NodeWireError<E>> {
        let mut animations = std::array::from_fn(|_| None);
        let source = self.source(version, &mut animations[0])?;
        let equalize = self.stored_property(&mut animations[1], 1, BinaryCursor::read_bool)?;
        let (strength, strength_adjustment) = self.strength(&mut animations[2])?;
        let selections = self.list(22, Self::patch_selection)?;
        let automatic = self.automatic()?;
        let manual = self.list(28, |wire| {
            Ok(Placement {
                identity: Arc::new(()),
                point: wire.float_pair()?,
                patch: wire.reader.read_i32()?,
                sequence: wire.reader.read_i32()?,
                angle: wire.reader.read_f32()?,
                size: wire.float_pair()?,
            })
        })?;
        let (repeat_animation, repeat_adjustment) = self.numeric_bindings(&[[1., 100.]; 2])?;
        animations[3] = repeat_animation;
        let repeat = read_raw_pair(self.reader)?;
        Ok(Patches {
            source,
            equalize,
            strength,
            strength_adjustment,
            selections,
            selections_parked: false,
            automatic,
            manual,
            repeat,
            repeat_adjustment,
            animations: PropertyAnimations::from_records(animations),
        })
    }

    fn automatic<E>(&mut self) -> Result<Automatic, NodeWireError<E>> {
        let enabled = self.reader.read_bool()?;
        let mut animations = std::array::from_fn(|_| None);
        let (frequency_animation, frequency_adjustment) =
            self.numeric_bindings(&[[1., 100.]; 2])?;
        animations[0] = frequency_animation;
        let frequency = read_raw_pair(self.reader)?;
        let (offset_animation, offset_adjustment) = self.numeric_bindings(&[[0., 100.]; 2])?;
        animations[1] = offset_animation;
        let offset = self.float_pair()?;

        let bounds = [
            [0., 100.],     // Density
            [0., 100.],     // Slide
            [0., 100.],     // Jitter X
            [0., 100.],     // Jitter Y
            [-100., 1000.], // Scale
            [0., 100.],     // Scale jitter
            [0., 360.],     // Angle
            [0., 100.],     // Angle jitter
        ];
        let mut scalars = [0.; 8];
        let mut adjustments = std::array::from_fn(|_| None);
        for (index, ((value, adjustment), bounds)) in scalars
            .iter_mut()
            .zip(&mut adjustments)
            .zip(bounds)
            .enumerate()
        {
            let (animation, link) = self.numeric_bindings(&[bounds])?;
            animations[index + 2] = animation;
            *adjustment = link;
            *value = self.reader.read_f32()?;
        }
        let [
            density,
            slide,
            x,
            y,
            scale,
            scale_jitter,
            angle,
            angle_jitter,
        ] = scalars;
        let [
            density_adjustment,
            slide_adjustment,
            x_adjustment,
            y_adjustment,
            scale_adjustment,
            scale_jitter_adjustment,
            angle_adjustment,
            angle_jitter_adjustment,
        ] = adjustments;
        let (switcher_animation, switcher_adjustment) = self.numeric_bindings(&[[0.5, 4.5]])?;
        animations[10] = switcher_animation;
        let switcher = self.reader.read_i32()?;
        Ok(Automatic {
            enabled,
            frequency,
            frequency_adjustment,
            offset,
            offset_adjustment,
            density,
            density_adjustment,
            slide,
            slide_adjustment,
            position_jitter: [x, y],
            position_jitter_adjustments: [x_adjustment, y_adjustment],
            scale,
            scale_adjustment,
            scale_jitter,
            scale_jitter_adjustment,
            angle,
            angle_adjustment,
            angle_jitter,
            angle_jitter_adjustment,
            switcher,
            switcher_adjustment,
            animations: PropertyAnimations::from_records(animations),
        })
    }

    fn patch_selection<E>(&mut self) -> Result<Selection, NodeWireError<E>> {
        let mut animations = std::array::from_fn(|_| None);
        let mut links = std::array::from_fn(|_| None);
        let mut bindings = |wire: &mut Self, index: usize| {
            let bounds = SynthesisNumberField::patch_fields(0)[index].mapping_range();
            (animations[index], links[index]) =
                wire.numeric_bindings(&[bounds.map(|bound| bound as f32)])?;
            Ok::<_, NodeWireError<E>>(())
        };
        bindings(self, 0)?;
        let frequency = self.reader.read_i32()?;
        bindings(self, 1)?;
        let feather = self.reader.read_f32()?;
        bindings(self, 2)?;
        let jitter = self.reader.read_f32()?;
        bindings(self, 3)?;
        let jitter_frequency = self.reader.read_i32()?;
        let mut shadow = [0.; 2];
        if self.version > 3.56 {
            for (index, value) in shadow.iter_mut().enumerate() {
                bindings(self, index + 4)?;
                *value = self.reader.read_f32()?;
            }
        }
        let [shadow_strength, shadow_feather] = shadow;
        let mut knots = self.list(31, |wire| {
            Ok(Knot {
                point: wire.float_pair()?,
                incoming: wire.float_pair()?,
                outgoing: wire.float_pair()?,
                mode: wire.reader.read_i32()?,
                flags: [
                    wire.reader.read_bool()?,
                    wire.reader.read_bool()?,
                    wire.reader.read_bool()?,
                ],
            })
        })?;
        for index in 0..knots.len() {
            let [incoming, outgoing] = Knot::canonical_handles(&knots, index);
            knots[index].incoming = incoming;
            knots[index].outgoing = outgoing;
        }
        let size_factor = if self.version < 3.7 { 2. } else { 1. };
        Ok(Selection {
            frequency,
            number_links: NumberLinks::from_records(links),
            feather: feather * size_factor,
            jitter: jitter * size_factor,
            jitter_frequency,
            shadow_strength,
            shadow_feather,
            animations: PropertyAnimations::from_records(animations),
            knots,
            flipped: [self.reader.read_bool()?, self.reader.read_bool()?],
        })
    }

    fn float_pair(&mut self) -> Result<[f32; 2], BinaryError> {
        Ok([self.reader.read_f32()?, self.reader.read_f32()?])
    }

    fn list<T, E>(
        &mut self,
        min_bytes: i32,
        mut read: impl FnMut(&mut Self) -> Result<T, NodeWireError<E>>,
    ) -> Result<Vec<T>, NodeWireError<E>> {
        let count = self.count()?;
        if count > (self.reader.length()? - self.reader.position()?) / min_bytes {
            return Err(BinaryError::End.into());
        }
        let mut values = Vec::new();
        values
            .try_reserve_exact(count as usize)
            .map_err(|_| BinaryError::Allocation)?;
        for _ in 0..count {
            values.push(read(self)?);
        }
        Ok(values)
    }
}

fn legacy_repeat_feather(stored: f64) -> f64 {
    let doubled = stored * 2.;
    if !(0. ..=100.).contains(&doubled) {
        return 2.;
    }
    crate::round_decimal_places(doubled, 1).expect("valid Feather precision")
}
