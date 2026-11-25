use super::{
    LayerColor, Master, Paint, Source, SynthesisRecord, SynthesisTexture,
    patches::{Automatic, PATCH_SWITCHERS, PatchShapeLimit, Patches, Selection},
};
use crate::{AdjustmentRecord, BinaryWriter, EnvironmentBlendMode, pointwise::PointwiseError};
use std::sync::Arc;

mod blend;
mod color;
mod paired;
mod photo;
pub use color::SynthesisColorField;
pub use paired::SynthesisPairField;
use paired::active_component;

/// Identifies a scalar Lab receiver within a retained Synthesis draft.
///
/// Master controls use no layer index. A patch index is relative to the
/// selection order within the requested layer and draft.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SynthesisNumberField {
    Seed,
    Scale,
    MasterColor(SynthesisColorField),
    LayerColor(SynthesisColorField),
    EqualizeStrength,
    RepeatFrequency,
    RepeatFeather,
    PhotoStretchX,
    PhotoStretchY,
    PhotoFeatherX,
    PhotoFeatherY,
    PhotoStarFeather,
    BlendOpacity,
    BlendErode,
    BlendSharpness,
    BlendMode,
    ErodeMode,
    PatchFrequency(usize),
    PatchFeather(usize),
    PatchJitterShape(usize),
    PatchJitterFrequency(usize),
    PatchShadowStrength(usize),
    PatchShadowFeather(usize),
    PositionJitterX,
    PositionJitterY,
    AngleJitter,
    Density,
    Slide,
    PatchScale,
    ScaleJitter,
    PatchAngle,
    PatchSwitcher,
}

impl SynthesisNumberField {
    pub(super) const PHOTO_FIELDS: [Self; 3] = [
        Self::EqualizeStrength,
        Self::RepeatFrequency,
        Self::RepeatFeather,
    ];

    pub(crate) const PHOTO_BLEND_FIELDS: [Self; 5] = [
        Self::PhotoStretchX,
        Self::PhotoStretchY,
        Self::PhotoFeatherX,
        Self::PhotoFeatherY,
        Self::PhotoStarFeather,
    ];

    fn photo_fields() -> impl Iterator<Item = Self> {
        Self::PHOTO_FIELDS
            .into_iter()
            .chain(Self::PHOTO_BLEND_FIELDS)
    }

    fn photo_blend_slot(self) -> Option<usize> {
        Self::PHOTO_BLEND_FIELDS
            .iter()
            .position(|field| *field == self)
    }

    pub(in crate::pointwise::canvas) const BLEND_WIRE_FIELDS: [Option<Self>; 6] = [
        Some(Self::BlendMode),
        Some(Self::ErodeMode),
        None,
        Some(Self::BlendErode),
        Some(Self::BlendSharpness),
        Some(Self::BlendOpacity),
    ];

    pub(crate) const AUTOMATIC_FIELDS: [Self; 9] = [
        Self::Density,
        Self::Slide,
        Self::PositionJitterX,
        Self::PositionJitterY,
        Self::PatchScale,
        Self::ScaleJitter,
        Self::PatchAngle,
        Self::AngleJitter,
        Self::PatchSwitcher,
    ];

    // This order is also the property order in the patch-selection wire record.
    pub(super) const fn patch_fields(index: usize) -> [Self; 6] {
        [
            Self::PatchFrequency(index),
            Self::PatchFeather(index),
            Self::PatchJitterShape(index),
            Self::PatchJitterFrequency(index),
            Self::PatchShadowStrength(index),
            Self::PatchShadowFeather(index),
        ]
    }

    pub(crate) const fn patch_slot(self) -> Option<(usize, usize)> {
        Some(match self {
            Self::PatchFrequency(index) => (index, 0),
            Self::PatchFeather(index) => (index, 1),
            Self::PatchJitterShape(index) => (index, 2),
            Self::PatchJitterFrequency(index) => (index, 3),
            Self::PatchShadowStrength(index) => (index, 4),
            Self::PatchShadowFeather(index) => (index, 5),
            _ => return None,
        })
    }

    pub const fn range(self) -> [f64; 2] {
        match self {
            Self::Seed => [0., 1000.],
            Self::Scale => [-10., 10.],
            Self::MasterColor(field) | Self::LayerColor(field) => field.range(),
            Self::RepeatFrequency | Self::PatchJitterFrequency(_) => [1., 100.],
            Self::PatchScale => [-100., 1000.],
            Self::PatchAngle => [-360., 360.],
            Self::BlendMode => [0., (EnvironmentBlendMode::ALL.len() - 1) as f64],
            Self::ErodeMode => [0., (super::SynthesisEdit::ERODE_MODES.len() - 1) as f64],
            Self::PatchSwitcher => [0., (PATCH_SWITCHERS.len() - 1) as f64],
            _ => [0., 100.],
        }
    }

    pub(crate) const fn mapping_range(self) -> [f64; 2] {
        match self {
            Self::PatchAngle => [0., 360.],
            Self::MasterColor(SynthesisColorField::Hue)
            | Self::LayerColor(SynthesisColorField::Hue) => [0., 360.],
            Self::MasterColor(SynthesisColorField::Saturation) => [-100., 100.],
            Self::RepeatFeather | Self::PatchFeather(_) | Self::PatchJitterShape(_) => [0., 30.],
            Self::BlendMode => [0.5, EnvironmentBlendMode::ALL.len() as f64 + 0.5],
            Self::ErodeMode => [0.5, super::SynthesisEdit::ERODE_MODES.len() as f64 + 0.5],
            Self::PatchSwitcher => [0.5, PATCH_SWITCHERS.len() as f64 + 0.5],
            _ => self.range(),
        }
    }

    pub(crate) const fn mapping_limits(self) -> [f64; 2] {
        match self {
            Self::PatchAngle
            | Self::MasterColor(_)
            | Self::LayerColor(_)
            | Self::RepeatFeather
            | Self::PatchFeather(_)
            | Self::PatchJitterShape(_) => self.range(),
            _ => self.mapping_range(),
        }
    }

    fn color_index(self) -> Option<usize> {
        match self {
            Self::MasterColor(field) => SynthesisColorField::MASTER
                .iter()
                .position(|current| *current == field),
            Self::LayerColor(field) => SynthesisColorField::LAYER
                .iter()
                .position(|current| *current == field),
            _ => None,
        }
    }

    pub(crate) const fn choice_count(self) -> Option<usize> {
        match self {
            Self::PatchSwitcher => Some(PATCH_SWITCHERS.len()),
            Self::BlendMode => Some(EnvironmentBlendMode::ALL.len()),
            Self::ErodeMode => Some(super::SynthesisEdit::ERODE_MODES.len()),
            _ => None,
        }
    }

    pub(super) fn normalize(self, value: f64) -> Option<f64> {
        if !value.is_finite() {
            return None;
        }
        match self {
            Self::Seed => integer(value, 0, 1000),
            Self::Scale => (-10. ..=10.)
                .contains(&value)
                .then(|| crate::round_decimal_places(value, 3).ok())
                .flatten(),
            Self::EqualizeStrength
            | Self::RepeatFeather
            | Self::BlendOpacity
            | Self::PhotoStretchX
            | Self::PhotoStretchY
            | Self::PhotoFeatherX
            | Self::PhotoFeatherY
            | Self::PhotoStarFeather => {
                let value = crate::round_decimal_places(value, 1).ok()?;
                (0. ..=100.).contains(&value).then_some(value)
            }
            Self::RepeatFrequency => {
                let value = value.round_ties_even();
                (1. ..=100.).contains(&value).then_some(value)
            }
            Self::PatchFrequency(_) => integer(value, 1, 100),
            Self::PatchJitterFrequency(_) => integer(value, 1, 100).filter(|value| *value >= 1.),
            Self::PatchSwitcher | Self::BlendMode | Self::ErodeMode => {
                crate::pointwise::choice_index_from_numeric::<()>(value, self.choice_count()?)
                    .ok()
                    .map(f64::from)
            }
            _ => {
                let value = f64::from(value as f32);
                let [min, max] = self.range();
                (min..=max).contains(&value).then_some(value)
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(in crate::pointwise::canvas) struct NumberLinks(Option<Arc<[Option<NumericLink>; 6]>>);

impl NumberLinks {
    pub(in crate::pointwise::canvas) fn from_records(links: [Option<NumericLink>; 6]) -> Self {
        Self(links.iter().any(Option::is_some).then(|| Arc::new(links)))
    }

    pub(in crate::pointwise::canvas) fn get(&self, index: usize) -> Option<&NumericLink> {
        self.0.as_ref()?.get(index)?.as_ref()
    }

    fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    fn iter(&self) -> impl Iterator<Item = &NumericLink> {
        self.0.iter().flat_map(|links| links.iter().flatten())
    }

    pub(super) fn set(&mut self, index: usize, link: Option<NumericLink>) {
        if link.is_some() || self.0.is_some() {
            let values = Arc::make_mut(self.0.get_or_insert_with(|| Arc::new(Default::default())));
            values[index] = link;
            if values.iter().all(Option::is_none) {
                self.0 = None;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::pointwise::canvas) struct NumericLink {
    record: Arc<AdjustmentRecord>,
    resolved: [Option<f64>; 2],
}

impl NumericLink {
    pub(super) fn new(record: AdjustmentRecord) -> Self {
        Self {
            record: Arc::new(record),
            resolved: [None; 2],
        }
    }

    pub(super) fn record(&self) -> &AdjustmentRecord {
        &self.record
    }

    pub(super) fn effective(&self, raw: f64) -> f64 {
        self.resolved[0].unwrap_or(raw)
    }

    pub(super) fn effective_value(raw: f64, link: Option<&Self>) -> f64 {
        link.map_or(raw, |link| link.effective(raw))
    }

    pub(super) fn is_active(&self) -> bool {
        active_component(self.record(), 0).is_some()
    }
}

#[derive(Clone, Copy)]
enum NumericReceiver {
    Scalar(SynthesisNumberField, f64),
    PatchShape(SynthesisNumberField, f64, PatchShapeLimit),
    Pair(SynthesisPairField, [f64; 2]),
}

impl NumericReceiver {
    fn axes(self) -> std::ops::Range<usize> {
        0..match self {
            Self::Scalar(..) | Self::PatchShape(..) => 1,
            Self::Pair(..) => 2,
        }
    }

    fn resolved(self, axis: usize, value: f64) -> Option<Option<f64>> {
        let (value, raw) = match self {
            Self::Scalar(field, raw) => (field.normalize(value)?, raw),
            Self::PatchShape(field, raw, limit) => {
                (f64::from(limit.clamp(field.normalize(value)? as f32)), raw)
            }
            Self::Pair(field, raw) => (field.normalize(value)?, raw[axis]),
        };
        Some((value.to_bits() != raw.to_bits()).then_some(value))
    }
}

impl SynthesisRecord {
    /// A new whole-Texture Material captures the current values of its fields.
    pub(crate) fn without_numeric_adjustments(&self) -> Self {
        let mut linked = false;
        self.visit_numeric_adjustments(|_| linked = true);
        if !linked {
            return self.clone();
        }
        let mut texture = self.0.as_ref().clone();
        texture.set_number_adjustment(None, SynthesisNumberField::Seed, None);
        texture.set_number_adjustment(None, SynthesisNumberField::Scale, None);
        for field in SynthesisColorField::MASTER {
            texture.set_number_adjustment(None, SynthesisNumberField::MasterColor(field), None);
        }
        for index in 0..texture.layers.len() {
            for field in SynthesisNumberField::BLEND_WIRE_FIELDS
                .into_iter()
                .flatten()
            {
                texture.set_number_adjustment(Some(index), field, None);
            }
            for field in SynthesisNumberField::photo_fields() {
                texture.set_number_adjustment(Some(index), field, None);
            }
            for field in SynthesisColorField::LAYER {
                texture.set_number_adjustment(
                    Some(index),
                    SynthesisNumberField::LayerColor(field),
                    None,
                );
            }
            for field in SynthesisPairField::ALL {
                texture.set_pair_adjustment(index, field, None);
            }
            let Paint::Patches(patches) = &texture.layers[index].paint else {
                continue;
            };
            let fields = (0..patches.selections.len())
                .flat_map(SynthesisNumberField::patch_fields)
                .chain(SynthesisNumberField::AUTOMATIC_FIELDS);
            for field in fields {
                texture.set_number_adjustment(Some(index), field, None);
            }
        }
        Self(Arc::new(texture))
    }

    pub(crate) fn visit_numeric_adjustments(&self, mut visit: impl FnMut(&AdjustmentRecord)) {
        self.visit_numeric_adjustments_with_visibility(|record, _| visit(record));
    }

    pub(crate) fn visit_numeric_adjustments_with_visibility(
        &self,
        mut visit: impl FnMut(&AdjustmentRecord, bool),
    ) {
        if let Some(link) = &self.0.master.seed_adjustment {
            visit(link.record(), true);
        }
        if let Some(link) = &self.0.master.scale_adjustment {
            visit(link.record(), true);
        }
        for link in self.0.master.color_links.iter() {
            visit(link.record(), true);
        }
        for layer in &self.0.layers {
            for link in layer.blend.synthesis_links.iter() {
                visit(link.record(), true);
            }
            for link in layer.color_controls.number_links.iter() {
                visit(link.record(), true);
            }
            for field in SynthesisNumberField::photo_fields() {
                if let Some((_, Some(link))) = layer.paint.photo_number(field) {
                    visit(link.record(), true);
                }
            }
            for field in SynthesisPairField::ALL {
                if let Some((_, Some(link))) = layer.paint.pair(field) {
                    visit(link.record(), true);
                }
            }
            if let Paint::Patches(patches) = &layer.paint {
                for link in patches
                    .selections
                    .iter()
                    .flat_map(|selection| selection.number_links.iter())
                {
                    visit(link.record(), !patches.selections_parked);
                }
                for link in SynthesisNumberField::AUTOMATIC_FIELDS
                    .into_iter()
                    .filter_map(|field| patches.automatic.number(field)?.1)
                {
                    visit(link.record(), true);
                }
            }
        }
    }

    /// Stage a new render revision while leaving the authored values and link records intact.
    pub(crate) fn map_numeric_adjustments<E>(
        &self,
        resolve: impl FnMut(&AdjustmentRecord, usize) -> Result<Option<f64>, PointwiseError<E>>,
    ) -> Result<Option<Self>, PointwiseError<E>> {
        self.map_numeric_adjustments_with_input_geometry(None, resolve)
    }

    pub(crate) fn map_numeric_adjustments_with_input_geometry<E>(
        &self,
        input: Option<crate::ImageGeometry>,
        resolve: impl FnMut(&AdjustmentRecord, usize) -> Result<Option<f64>, PointwiseError<E>>,
    ) -> Result<Option<Self>, PointwiseError<E>> {
        self.resolve_numeric_adjustments(input, false, resolve)
    }

    pub(crate) fn restore_numeric_adjustments_with_saved_graph_shapes<E>(
        &self,
        resolve: impl FnMut(&AdjustmentRecord, usize) -> Result<Option<f64>, PointwiseError<E>>,
    ) -> Result<Option<Self>, PointwiseError<E>> {
        self.resolve_numeric_adjustments(None, true, resolve)
    }

    fn resolve_numeric_adjustments<E>(
        &self,
        input: Option<crate::ImageGeometry>,
        preserve_graph_shapes: bool,
        mut resolve: impl FnMut(&AdjustmentRecord, usize) -> Result<Option<f64>, PointwiseError<E>>,
    ) -> Result<Option<Self>, PointwiseError<E>> {
        self.edit_numeric_links(input, preserve_graph_shapes, &mut |link, receiver| {
            let mut resolved = link.resolved;
            for axis in receiver.axes() {
                if active_component(link.record(), axis).is_some()
                    && let Some(value) = resolve(link.record(), axis)?
                    && let Some(value) = receiver.resolved(axis, value)
                {
                    resolved[axis] = value;
                }
            }
            Ok((resolved != link.resolved).then(|| NumericLink {
                record: link.record.clone(),
                resolved,
            }))
        })
    }

    pub(crate) fn uses_connected_patch_shape_adjuster(&self, keys: &[([u8; 16], &[u16])]) -> bool {
        self.0.layers.iter().any(|layer| {
            let Paint::Patches(patches) = &layer.paint else {
                return false;
            };
            matches!(patches.source, Source::Input)
                && patches.selections.iter().any(|selection| {
                    [1, 2].into_iter().any(|slot| {
                        selection.number_links.get(slot).is_some_and(|link| {
                            let record = link.record();
                            matches!(record.property_type, 0 | 4)
                                && active_component(record, 0)
                                    .is_some_and(|name| keys.contains(&(record.guid, name)))
                        })
                    })
                })
        })
    }

    pub(crate) fn relocate_numeric_adjustments<E>(
        &self,
        group: [u8; 16],
        mut destination: impl FnMut(&AdjustmentRecord, usize) -> Result<Option<f64>, E>,
    ) -> Result<Option<Self>, E> {
        self.edit_numeric_links(None, false, &mut |link, receiver| {
            if link.record().guid == group {
                return Ok(None);
            }
            let mut resolved = [None; 2];
            let mut moved = false;
            for axis in receiver.axes() {
                if active_component(link.record(), axis).is_some()
                    && let Some(value) = destination(link.record(), axis)?
                {
                    moved = true;
                    resolved[axis] = receiver
                        .resolved(axis, value)
                        .unwrap_or(link.resolved[axis]);
                }
            }
            if !moved {
                return Ok(None);
            }
            let mut record = link.record().clone();
            record.guid = group;
            Ok(Some(NumericLink {
                record: Arc::new(record),
                resolved,
            }))
        })
    }

    pub(crate) fn rename_numeric_adjustments<E>(
        &self,
        group: [u8; 16],
        tag: i32,
        previous: &[u16],
        requested: &[u16],
        mut resolve: impl FnMut(&AdjustmentRecord, usize) -> Result<Option<f64>, E>,
    ) -> Result<Option<Self>, E> {
        self.edit_numeric_links(None, false, &mut |link, receiver| {
            let record = link.record();
            if record.guid != group
                || !matches!((tag, record.property_type), (0 | 4, 0 | 4) | (1, 1))
            {
                return Ok(None);
            }
            let matches =
                |axis| active_component(record, axis).is_some_and(|name| name == previous);
            if !receiver.axes().any(matches) {
                return Ok(None);
            }
            let mut record = record.clone();
            for axis in receiver.axes().filter(|&axis| matches(axis)) {
                record.components_mut()[axis].name = requested.to_vec();
            }
            let mut resolved = link.resolved;
            for axis in receiver.axes().filter(|&axis| matches(axis)) {
                if let Some(value) = resolve(&record, axis)?
                    && let Some(value) = receiver.resolved(axis, value)
                {
                    resolved[axis] = value;
                }
            }
            Ok(Some(NumericLink {
                record: Arc::new(record),
                resolved,
            }))
        })
    }

    fn edit_numeric_links<E>(
        &self,
        input: Option<crate::ImageGeometry>,
        preserve_graph_shapes: bool,
        edit: &mut impl FnMut(&NumericLink, NumericReceiver) -> Result<Option<NumericLink>, E>,
    ) -> Result<Option<Self>, E> {
        let mut texture = None;
        let master = &self.0.master;
        if let Some(link) = edit_link(
            master.seed_adjustment.as_ref(),
            f64::from(master.seed),
            SynthesisNumberField::Seed,
            edit,
        )? {
            texture
                .get_or_insert_with(|| self.0.as_ref().clone())
                .master
                .seed_adjustment = Some(link);
        }
        if let Some(link) = edit_link(
            master.scale_adjustment.as_ref(),
            master.scale,
            SynthesisNumberField::Scale,
            edit,
        )? {
            texture
                .get_or_insert_with(|| self.0.as_ref().clone())
                .master
                .scale_adjustment = Some(link);
        }
        for (index, field) in SynthesisColorField::MASTER.into_iter().enumerate() {
            if let Some(link) = edit_link(
                master.color_links.get(index),
                f64::from(master.color_values()[index]),
                SynthesisNumberField::MasterColor(field),
                edit,
            )? {
                texture
                    .get_or_insert_with(|| self.0.as_ref().clone())
                    .master
                    .color_links
                    .set(index, Some(link));
            }
        }
        for (index, layer) in self.0.layers.iter().enumerate() {
            for (slot, field) in SynthesisNumberField::BLEND_WIRE_FIELDS
                .into_iter()
                .enumerate()
            {
                if let Some(field) = field
                    && let Some((raw, Some(link))) = layer.blend.synthesis_number(field)
                    && let Some(link) = edit(link, NumericReceiver::Scalar(field, raw))?
                {
                    let layer = &mut texture
                        .get_or_insert_with(|| self.0.as_ref().clone())
                        .layers[index];
                    Arc::make_mut(layer)
                        .blend
                        .synthesis_links
                        .set(slot, Some(link));
                }
            }
            for (slot, field) in SynthesisColorField::LAYER.into_iter().enumerate() {
                if let Some(link) = edit_link(
                    layer.color_controls.number_links.get(slot),
                    f64::from(layer.color_controls.adjustments[slot]),
                    SynthesisNumberField::LayerColor(field),
                    edit,
                )? {
                    let layer = &mut texture
                        .get_or_insert_with(|| self.0.as_ref().clone())
                        .layers[index];
                    Arc::make_mut(layer)
                        .color_controls
                        .number_links
                        .set(slot, Some(link));
                }
            }
            if let Paint::Patches(patches) = &layer.paint
                && let Some(patches) =
                    patches.edit_numeric_links(input, preserve_graph_shapes, edit)?
            {
                let layer = &mut texture
                    .get_or_insert_with(|| self.0.as_ref().clone())
                    .layers[index];
                Arc::make_mut(layer).paint = Paint::Patches(Box::new(patches));
            }
            for field in SynthesisNumberField::photo_fields() {
                if let Some((raw, Some(link))) = layer.paint.photo_number(field)
                    && let Some(link) = edit(link, NumericReceiver::Scalar(field, raw))?
                {
                    let layer = &mut texture
                        .get_or_insert_with(|| self.0.as_ref().clone())
                        .layers[index];
                    Arc::make_mut(layer)
                        .paint
                        .set_photo_number_link(field, Some(link), None)
                        .expect("retained photo property");
                }
            }
            if let Some((raw, Some(link))) = layer.paint.pair(SynthesisPairField::PhotoRepeat)
                && let Some(link) = edit(
                    link,
                    NumericReceiver::Pair(SynthesisPairField::PhotoRepeat, raw),
                )?
                && let Paint::Blend(blend) = &mut Arc::make_mut(
                    &mut texture
                        .get_or_insert_with(|| self.0.as_ref().clone())
                        .layers[index],
                )
                .paint
            {
                blend.repeat_adjustment = Some(link);
            }
        }
        Ok(texture.map(|texture| Self(Arc::new(texture))))
    }
}

impl Patches {
    fn edit_numeric_links<E>(
        &self,
        input: Option<crate::ImageGeometry>,
        preserve_graph_shapes: bool,
        edit: &mut impl FnMut(&NumericLink, NumericReceiver) -> Result<Option<NumericLink>, E>,
    ) -> Result<Option<Self>, E> {
        let mut patches: Option<Self> = None;
        let mut source = None;
        for (index, selection) in self.selections.iter().enumerate() {
            for (slot, field) in SynthesisNumberField::patch_fields(index)
                .into_iter()
                .enumerate()
            {
                if preserve_graph_shapes
                    && matches!(self.source, Source::Input)
                    && matches!(slot, 1 | 2)
                {
                    continue;
                }
                let Some(link) = selection.number_links.get(slot) else {
                    continue;
                };
                let raw = selection.raw_number(slot);
                let mut receiver = NumericReceiver::Scalar(field, raw);
                if matches!(slot, 1 | 2) {
                    let current = patches
                        .as_ref()
                        .map_or(selection, |patches| &patches.selections[index]);
                    let geometry = *source
                        .get_or_insert_with(|| self.selection_source_geometry_with_input(input));
                    if let Some(limit) = Self::shape_limit(geometry, current, slot) {
                        receiver = NumericReceiver::PatchShape(field, raw, limit);
                    }
                }
                if let Some(link) = edit(link, receiver)? {
                    patches.get_or_insert_with(|| self.clone()).selections[index]
                        .number_links
                        .set(slot, Some(link));
                }
            }
        }
        for field in SynthesisNumberField::AUTOMATIC_FIELDS {
            if let Some((raw, Some(link))) = self.automatic.number(field)
                && let Some(replacement) = edit(link, NumericReceiver::Scalar(field, raw))?
                && let Some(adjustment) = patches
                    .get_or_insert_with(|| self.clone())
                    .automatic
                    .adjustment_mut(field)
            {
                *adjustment = Some(replacement);
            }
        }
        for field in SynthesisPairField::AUTOMATIC {
            let (raw, link) = self.automatic.pair(field);
            if let Some(link) = link
                && let Some(replacement) = edit(link, NumericReceiver::Pair(field, raw))?
            {
                *patches
                    .get_or_insert_with(|| self.clone())
                    .automatic
                    .pair_link_mut(field) = Some(replacement);
            }
        }
        if let Some(link) = self.repeat_adjustment.as_ref()
            && let Some(replacement) = edit(
                link,
                NumericReceiver::Pair(SynthesisPairField::ManualRepeat, self.repeat.map(f64::from)),
            )?
        {
            patches
                .get_or_insert_with(|| self.clone())
                .repeat_adjustment = Some(replacement);
        }
        Ok(patches)
    }
}

fn edit_link<E>(
    link: Option<&NumericLink>,
    raw: f64,
    field: SynthesisNumberField,
    edit: &mut impl FnMut(&NumericLink, NumericReceiver) -> Result<Option<NumericLink>, E>,
) -> Result<Option<NumericLink>, E> {
    match link {
        Some(link) => edit(link, NumericReceiver::Scalar(field, raw)),
        None => Ok(None),
    }
}

fn integer(value: f64, places: i32, maximum: i32) -> Option<f64> {
    let rounded = crate::round_decimal_places(value, places).ok()?;
    let integer = crate::framework_cast::integer(rounded);
    (0..=maximum)
        .contains(&integer)
        .then_some(f64::from(integer))
}

impl Master {
    pub(super) fn effective_seed(&self) -> i32 {
        self.seed_adjustment
            .as_ref()
            .and_then(|link| link.resolved[0])
            .map_or(self.seed, |value| value as i32)
    }

    pub(super) fn effective_scale(&self) -> f64 {
        self.scale_adjustment
            .as_ref()
            .map_or(self.scale, |link| link.effective(self.scale))
    }
}

impl Selection {
    fn raw_number(&self, slot: usize) -> f64 {
        match slot {
            0 => f64::from(self.frequency),
            1 => f64::from(self.feather),
            2 => f64::from(self.jitter),
            3 => f64::from(self.jitter_frequency),
            4 => f64::from(self.shadow_strength),
            5 => f64::from(self.shadow_feather),
            _ => unreachable!("patch selection property"),
        }
    }

    fn set_raw_number(&mut self, slot: usize, value: f64) {
        match slot {
            0 => self.frequency = value as i32,
            1 => self.feather = value as f32,
            2 => self.jitter = value as f32,
            3 => self.jitter_frequency = value as i32,
            4 => self.shadow_strength = value as f32,
            5 => self.shadow_feather = value as f32,
            _ => unreachable!("patch selection property"),
        }
    }

    pub(super) fn effective_number(&self, slot: usize) -> f64 {
        NumericLink::effective_value(self.raw_number(slot), self.number_links.get(slot))
    }

    pub(super) fn effective_frequency(&self) -> i32 {
        self.effective_number(0) as i32
    }
}

impl Automatic {
    fn number(&self, field: SynthesisNumberField) -> Option<(f64, Option<&NumericLink>)> {
        use SynthesisNumberField::*;
        let (raw, adjustment) = match field {
            Density => (self.density, &self.density_adjustment),
            Slide => (self.slide, &self.slide_adjustment),
            PatchScale => (self.scale, &self.scale_adjustment),
            ScaleJitter => (self.scale_jitter, &self.scale_jitter_adjustment),
            PatchAngle => (self.angle, &self.angle_adjustment),
            PatchSwitcher => {
                return Some((f64::from(self.switcher), self.switcher_adjustment.as_ref()));
            }
            PositionJitterX | PositionJitterY => {
                let axis = usize::from(field == PositionJitterY);
                (
                    self.position_jitter[axis],
                    &self.position_jitter_adjustments[axis],
                )
            }
            AngleJitter => (self.angle_jitter, &self.angle_jitter_adjustment),
            _ => return None,
        };
        Some((f64::from(raw), adjustment.as_ref()))
    }

    fn number_mut(
        &mut self,
        field: SynthesisNumberField,
    ) -> Option<(&mut f32, &mut Option<NumericLink>)> {
        use SynthesisNumberField::*;
        Some(match field {
            Density => (&mut self.density, &mut self.density_adjustment),
            Slide => (&mut self.slide, &mut self.slide_adjustment),
            PatchScale => (&mut self.scale, &mut self.scale_adjustment),
            ScaleJitter => (&mut self.scale_jitter, &mut self.scale_jitter_adjustment),
            PatchAngle => (&mut self.angle, &mut self.angle_adjustment),
            PositionJitterX | PositionJitterY => {
                let axis = usize::from(field == PositionJitterY);
                (
                    &mut self.position_jitter[axis],
                    &mut self.position_jitter_adjustments[axis],
                )
            }
            AngleJitter => (&mut self.angle_jitter, &mut self.angle_jitter_adjustment),
            _ => return None,
        })
    }

    fn adjustment_mut(&mut self, field: SynthesisNumberField) -> Option<&mut Option<NumericLink>> {
        if field == SynthesisNumberField::PatchSwitcher {
            Some(&mut self.switcher_adjustment)
        } else {
            self.number_mut(field).map(|(_, adjustment)| adjustment)
        }
    }

    pub(super) fn effective_density(&self) -> f32 {
        effective_float(self.density, self.density_adjustment.as_ref())
    }

    pub(super) fn effective_slide(&self) -> f32 {
        effective_float(self.slide, self.slide_adjustment.as_ref())
    }

    pub(super) fn effective_scale(&self) -> f32 {
        effective_float(self.scale, self.scale_adjustment.as_ref())
    }

    pub(super) fn effective_scale_jitter(&self) -> f32 {
        effective_float(self.scale_jitter, self.scale_jitter_adjustment.as_ref())
    }

    pub(super) fn effective_angle(&self) -> f32 {
        effective_float(self.angle, self.angle_adjustment.as_ref())
    }

    pub(super) fn effective_switcher(&self) -> i32 {
        self.switcher_adjustment
            .as_ref()
            .map_or(self.switcher, |link| {
                link.effective(f64::from(self.switcher)) as i32
            })
    }

    pub(super) fn effective_position_jitter(&self) -> [f32; 2] {
        std::array::from_fn(|axis| {
            self.position_jitter_adjustments[axis]
                .as_ref()
                .and_then(|link| link.resolved[0])
                .map_or(self.position_jitter[axis], |value| value as f32)
        })
    }

    pub(super) fn effective_angle_jitter(&self) -> f32 {
        self.angle_jitter_adjustment
            .as_ref()
            .and_then(|link| link.resolved[0])
            .map_or(self.angle_jitter, |value| value as f32)
    }
}

fn effective_float(raw: f32, link: Option<&NumericLink>) -> f32 {
    link.and_then(|link| link.resolved[0])
        .map_or(raw, |value| value as f32)
}

impl SynthesisTexture {
    pub(super) fn number(
        &self,
        layer: Option<usize>,
        field: SynthesisNumberField,
    ) -> Option<(f64, Option<&NumericLink>)> {
        use SynthesisNumberField::*;

        if field == Seed {
            return layer.is_none().then_some((
                f64::from(self.master.seed),
                self.master.seed_adjustment.as_ref(),
            ));
        }
        if field == Scale {
            return layer
                .is_none()
                .then_some((self.master.scale, self.master.scale_adjustment.as_ref()));
        }
        if matches!(field, MasterColor(_)) && layer.is_none() {
            let index = field.color_index()?;
            return Some((
                f64::from(self.master.color_values()[index]),
                self.master.color_links.get(index),
            ));
        }
        if matches!(field, LayerColor(_)) {
            let colors = &self.layers.get(layer?)?.color_controls;
            let index = field.color_index()?;
            return Some((
                f64::from(colors.adjustments[index]),
                colors.number_links.get(index),
            ));
        }
        let layer = self.layers.get(layer?)?;
        if let Some(blend) = layer.blend.synthesis_number(field) {
            return Some(blend);
        }
        let paint = &layer.paint;
        if SynthesisNumberField::PHOTO_FIELDS.contains(&field) || field.photo_blend_slot().is_some()
        {
            return paint.photo_number(field);
        }
        let Paint::Patches(patches) = paint else {
            return None;
        };
        if let Some((patch, slot)) = field.patch_slot() {
            let selection = patches.selections.get(patch)?;
            return Some((selection.raw_number(slot), selection.number_links.get(slot)));
        }
        patches.automatic.number(field)
    }

    pub(super) fn set_number_adjustment(
        &mut self,
        layer: Option<usize>,
        field: SynthesisNumberField,
        link: Option<(AdjustmentRecord, f64)>,
    ) -> Option<bool> {
        self.set_number_adjustment_with_input_geometry(layer, field, link, None)
    }

    pub(super) fn set_number_adjustment_with_input_geometry(
        &mut self,
        layer: Option<usize>,
        field: SynthesisNumberField,
        link: Option<(AdjustmentRecord, f64)>,
        input: Option<crate::ImageGeometry>,
    ) -> Option<bool> {
        use SynthesisNumberField::*;

        let (raw, current) = self.number(layer, field)?;
        let (next, frozen) = match link {
            Some((record, mapped)) => {
                if !matches!(record.property_type, 0 | 4)
                    || !matches!(record.components(), [component] if component.active && !component.name.is_empty())
                {
                    return None;
                }
                let mut value = field.normalize(mapped)?;
                if let (Some((patch, slot @ (1 | 2))), Some(layer)) = (field.patch_slot(), layer)
                    && let Paint::Patches(patches) = &self.layers.get(layer)?.paint
                    && let Some(limit) = Patches::shape_limit(
                        patches.selection_source_geometry_with_input(input),
                        patches.selections.get(patch)?,
                        slot,
                    )
                {
                    value = f64::from(limit.clamp(value as f32));
                }
                let resolved = (value.to_bits() != raw.to_bits()).then_some(value);
                if current.is_some_and(|current| {
                    current.resolved[0] == resolved
                        && same_number_adjustment(current.record(), &record)
                }) {
                    return Some(false);
                }
                (
                    Some(NumericLink {
                        record: Arc::new(record),
                        resolved: [resolved, None],
                    }),
                    None,
                )
            }
            None => {
                let Some(current) = current else {
                    return Some(false);
                };
                (None, current.resolved[0])
            }
        };

        if field == Seed {
            self.master.seed_adjustment = next;
            if let Some(value) = frozen {
                self.master.seed = value as i32;
            }
            return Some(true);
        }
        if field == Scale {
            self.master.scale_adjustment = next;
            if let Some(value) = frozen {
                self.master.scale = value;
            }
            return Some(true);
        }
        if matches!(field, MasterColor(_)) {
            let index = field.color_index()?;
            self.master.color_links.set(index, next);
            if let Some(value) = frozen {
                *self.master.color_value_mut(index) = value as f32;
            }
            return Some(true);
        }
        if matches!(field, LayerColor(_)) {
            let colors = &mut Arc::make_mut(self.layers.get_mut(layer?)?).color_controls;
            let index = field.color_index()?;
            colors.number_links.set(index, next);
            if let Some(value) = frozen {
                colors.adjustments[index] = value as f32;
            }
            return Some(true);
        }
        let layer = Arc::make_mut(self.layers.get_mut(layer?)?);
        if let Some(slot) = field.blend_slot() {
            layer.blend.synthesis_links.set(slot, next);
            if let Some(value) = frozen {
                layer.blend.set_raw_synthesis_number(field, value);
            }
            return Some(true);
        }
        let paint = &mut layer.paint;
        if SynthesisNumberField::PHOTO_FIELDS.contains(&field) || field.photo_blend_slot().is_some()
        {
            paint.set_photo_number_link(field, next, frozen)?;
            return Some(true);
        }
        let Paint::Patches(patches) = paint else {
            return None;
        };
        if let Some((patch, slot)) = field.patch_slot() {
            let selection = patches.selections.get_mut(patch)?;
            selection.number_links.set(slot, next);
            if let Some(value) = frozen {
                selection.set_raw_number(slot, value);
            }
            return Some(true);
        }
        match field {
            PatchSwitcher => {
                patches.automatic.switcher_adjustment = next;
                if let Some(value) = frozen {
                    patches.automatic.switcher = value as i32;
                }
            }
            field => {
                let (raw, adjustment) = patches.automatic.number_mut(field)?;
                *adjustment = next;
                if let Some(value) = frozen {
                    *raw = value as f32;
                }
            }
        }
        Some(true)
    }

    pub(in crate::pointwise) fn append_render_values(&self, out: &mut Vec<u64>) {
        out.extend([
            self.master.effective_seed() as u64,
            self.master.effective_scale().to_bits(),
            self.layers.len() as u64,
        ]);
        out.extend(self.master.effective_tint().map(f64::to_bits));
        out.extend(
            self.master
                .effective_color_values()
                .map(|value| u64::from(value.to_bits())),
        );
        for layer in &self.layers {
            let tint = match &layer.paint {
                Paint::Solid(solid) => solid.effective_color(),
                _ => layer.color_controls.effective_tint(),
            };
            out.extend(tint.map(f64::to_bits));
            out.extend(
                SynthesisNumberField::BLEND_WIRE_FIELDS
                    .into_iter()
                    .flatten()
                    .filter_map(|field| layer.blend.synthesis_number(field))
                    .map(|(raw, link)| NumericLink::effective_value(raw, link).to_bits()),
            );
            out.extend(
                layer
                    .color_controls
                    .effective_color_values()
                    .map(|value| u64::from(value.to_bits())),
            );
            out.extend(SynthesisNumberField::photo_fields().map(|field| {
                layer.paint.photo_number(field).map_or(0, |(raw, link)| {
                    NumericLink::effective_value(raw, link).to_bits()
                })
            }));
            if let Paint::Blend(blend) = &layer.paint {
                out.extend(blend.effective_repeat().map(|count| count as u64));
            }
            let Paint::Patches(patches) = &layer.paint else {
                out.push(0);
                continue;
            };
            out.extend([1, patches.selections.len() as u64]);
            for selection in &patches.selections {
                out.extend((0..6).map(|slot| selection.effective_number(slot).to_bits()));
            }
            out.extend(patches.effective_repeat().map(|count| count as u64));
            out.extend(
                patches
                    .automatic
                    .effective_position_jitter()
                    .map(|value| u64::from(value.to_bits())),
            );
            out.push(u64::from(
                patches.automatic.effective_angle_jitter().to_bits(),
            ));
            out.extend(
                [
                    patches.automatic.effective_density(),
                    patches.automatic.effective_slide(),
                    patches.automatic.effective_scale(),
                    patches.automatic.effective_scale_jitter(),
                    patches.automatic.effective_angle(),
                ]
                .map(|value| u64::from(value.to_bits())),
            );
            out.push(patches.automatic.effective_switcher() as u64);
            out.extend(
                patches
                    .automatic
                    .effective_frequency()
                    .map(|value| value as u64),
            );
            out.extend(
                patches
                    .automatic
                    .effective_offset()
                    .map(|value| u64::from(value.to_bits())),
            );
        }
    }
}

fn same_number_adjustment(left: &AdjustmentRecord, right: &AdjustmentRecord) -> bool {
    let mut left_wire = BinaryWriter::default();
    let mut right_wire = BinaryWriter::default();
    left.clone().save(&mut left_wire).is_ok()
        && right.clone().save(&mut right_wire).is_ok()
        && left_wire.bytes() == right_wire.bytes()
}

impl super::super::style::CanvasStyle {
    pub(in crate::pointwise) fn append_synthesis_render_values(&self, out: &mut Vec<u64>) {
        use super::super::style::Paint;

        out.push(self.passes.len() as u64);
        for pass in &self.passes {
            match &pass.paint {
                Paint::Synthesis(paint) => {
                    out.push(1);
                    paint.texture().append_render_values(out);
                }
                _ => out.push(0),
            }
        }
    }
}

impl super::super::CanvasDocument {
    pub(in crate::pointwise) fn append_synthesis_render_values(&self, out: &mut Vec<u64>) {
        out.push(self.layers.len() as u64);
        for layer in &self.layers {
            layer.style.append_synthesis_render_values(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BinaryCursor, SynthesisEdit, SynthesisGeometry, SynthesisLayerKind};

    fn link(id: u8) -> Option<NumericLink> {
        let mut record = AdjustmentRecord::new(&[[0., 100.]]);
        record.guid = [id; 16];
        record.components_mut()[0].name = "Receiver".encode_utf16().collect();
        record.components_mut()[0].active = true;
        Some(NumericLink::new(record))
    }

    #[test]
    fn typed_synthesis_number_links_keep_raw_values_until_unlinked() {
        use SynthesisNumberField::*;

        let initial = Arc::new(SynthesisTexture::default());
        let mut edit = SynthesisEdit::new(initial.clone(), true);
        let standalone = SynthesisEdit::new(initial.clone(), false);
        assert!(edit.same_revision(&SynthesisEdit::new(initial, true)));
        assert!(!edit.same_revision(&standalone));
        assert_eq!(standalone.number_value(None, Seed), None);
        edit.add_layer(SynthesisLayerKind::Solid);
        edit.add_layer(SynthesisLayerKind::Patches);
        assert_eq!(edit.number_value(Some(0), Seed), None);
        assert_eq!(edit.number_value(Some(1), AngleJitter), None);
        assert_eq!(edit.number_value(Some(0), PatchFrequency(1)), None);

        let patch = SynthesisGeometry::Patch {
            selection: 0,
            knot: 0,
        };
        for (layer, field, raw) in [
            (None, Seed, 0.),
            (Some(0), PatchFrequency(0), 100.),
            (Some(0), PositionJitterX, 20.),
            (Some(0), PositionJitterY, 20.),
            (Some(0), AngleJitter, 10.),
            (Some(0), Density, 100.),
            (Some(0), Slide, 0.),
            (Some(0), PatchScale, 0.),
            (Some(0), ScaleJitter, 10.),
        ] {
            let mut record = AdjustmentRecord::new(&[field.range().map(|limit| limit as f32)]);
            let component = &mut record.components_mut()[0];
            component.name = "Linked".encode_utf16().collect();
            component.active = true;
            assert_eq!(edit.number_value(layer, field), Some(raw));
            assert_eq!(
                edit.set_number_adjustment(layer, field, Some((record.clone(), f64::NAN))),
                None
            );
            assert_eq!(
                edit.set_number_adjustment(layer, field, Some((record.clone(), 42.))),
                Some(true)
            );
            assert_eq!(
                edit.set_number_adjustment(layer, field, Some((record, 42.))),
                Some(false)
            );
            assert_eq!(edit.number_value(layer, field), Some(42.));

            let fields = |edit: &SynthesisEdit| {
                if let PatchFrequency(_) = field {
                    edit.geometry_fields(layer.unwrap(), patch)
                } else {
                    edit.fields(layer)
                }
            };
            let field_list = fields(&edit);
            let index = field_list
                .iter()
                .position(|item| item.number_field() == Some(field))
                .unwrap();
            let mut values: Vec<_> = field_list.iter().map(|item| item.value()).collect();
            assert_eq!(values[index], raw);
            values[index] = 53.;
            if let PatchFrequency(_) = field {
                edit.set_geometry_fields(layer.unwrap(), patch, &values)
                    .unwrap();
            } else {
                edit.set_fields(layer, &values).unwrap();
            }
            assert_eq!(fields(&edit)[index].value(), raw);
            assert!(edit.number_adjustment(layer, field).is_some());
            assert_eq!(edit.set_number_adjustment(layer, field, None), Some(true));
            assert_eq!(edit.number_value(layer, field), Some(42.));
            assert!(edit.number_adjustment(layer, field).is_none());
            assert_eq!(fields(&edit)[index].value(), 42.);
            assert_eq!(edit.set_number_adjustment(layer, field, None), Some(false));
        }
    }

    #[test]
    fn typed_synthesis_number_links_keep_each_receiver_and_selection_mapping_on_reopen() {
        use SynthesisNumberField::*;

        let initial = Arc::new(SynthesisTexture::default());
        let mut edit = SynthesisEdit::new(initial.clone(), true);
        edit.add_layer(SynthesisLayerKind::Patches);
        assert_eq!(edit.add_patch(0), Some(1));
        let layer = Some(0);
        let receivers = [
            (PatchFrequency(1), 100., 0.),
            (PositionJitterX, 20., 12.5),
            (PositionJitterY, 20., 75.25),
            (AngleJitter, 10., 100.),
            (Density, 100., 25.),
            (Slide, 0., 90.),
            (PatchScale, 0., -25.),
            (ScaleJitter, 10., 100.),
        ];
        for (index, (field, raw, mapped)) in receivers.into_iter().enumerate() {
            let bounds = field.range();
            let mut record = AdjustmentRecord::new(&[bounds.map(|value| value as f32)]);
            record.property_type = if index == 0 { 4 } else { 0 };
            record.guid = [index as u8; 16];
            let component = &mut record.components_mut()[0];
            component.name = format!("Receiver {index}").encode_utf16().collect();
            component.active = true;
            assert_eq!(edit.number_value(layer, field), Some(raw));
            assert_eq!(
                edit.set_number_adjustment(layer, field, Some((record.clone(), bounds[1] + 1.))),
                None
            );
            assert_eq!(
                edit.set_number_adjustment(layer, field, Some((record.clone(), mapped))),
                Some(true)
            );
            let mut dual = record.clone();
            dual.property_type = 1;
            assert_eq!(
                edit.set_number_adjustment(layer, field, Some((dual, mapped))),
                None
            );
            assert_eq!(edit.number_value(layer, field), Some(mapped));
        }

        // An imported inactive Auto section can still carry wire links.
        let mut texture = edit.into_local_replacement(&initial).unwrap().unwrap();
        let Paint::Patches(patches) =
            &mut Arc::make_mut(&mut Arc::make_mut(&mut texture).layers[0]).paint
        else {
            panic!("expected a Patches layer");
        };
        patches.automatic.enabled = false;
        let authored = SynthesisRecord::from_texture(texture);
        let mut writer = BinaryWriter::default();
        authored.write(&mut writer).unwrap();
        let mut reader = BinaryCursor::new(writer.bytes()).unwrap();
        let reopened = SynthesisRecord::read(&mut reader, 3.75).unwrap();
        assert_eq!(reader.position().unwrap() as usize, writer.bytes().len());
        let reopened_edit = SynthesisEdit::new(reopened.texture().clone(), true);
        assert_eq!(reopened_edit.automatic_placement(0), Some(false));
        assert_eq!(
            reopened_edit.number_value(layer, PatchFrequency(0)),
            Some(100.)
        );
        assert!(
            reopened_edit
                .number_adjustment(layer, PatchFrequency(0))
                .is_none()
        );
        for (index, (field, raw, _)) in receivers.into_iter().enumerate() {
            assert_eq!(reopened_edit.number_value(layer, field), Some(raw));
            let adjustment = reopened_edit.number_adjustment(layer, field).unwrap();
            assert_eq!(adjustment.guid, [index as u8; 16]);
            assert_eq!(adjustment.property_type, if index == 0 { 4 } else { 0 });
            assert_eq!(
                String::from_utf16(&adjustment.components()[0].name).unwrap(),
                format!("Receiver {index}")
            );
        }

        let resolved = reopened
            .map_numeric_adjustments::<()>(|record, _| {
                Ok(Some(receivers[usize::from(record.guid[0])].2))
            })
            .unwrap()
            .unwrap();
        let mut resolved_edit = SynthesisEdit::new(resolved.texture().clone(), true);
        for (field, _, mapped) in receivers {
            assert_eq!(resolved_edit.number_value(layer, field), Some(mapped));
            assert_eq!(
                resolved_edit.set_number_adjustment(layer, field, None),
                Some(true)
            );
            assert_eq!(resolved_edit.number_value(layer, field), Some(mapped));
            assert!(resolved_edit.number_adjustment(layer, field).is_none());
        }
    }

    #[test]
    fn linked_automatic_controls_render_like_direct_values() {
        use crate::{EnvironmentImage, PointwiseRenderEffects, ProceduralExecution};
        use SynthesisNumberField::*;

        let mut tga = vec![0; 18];
        tga[2] = 2;
        tga[12] = 64;
        tga[14] = 64;
        tga[16] = 24;
        for y in 0..64u8 {
            for x in 0..64u8 {
                tga.extend([x * 4, y * 4, (x ^ y) * 4]);
            }
        }
        let mut original = SynthesisEdit::new(Arc::new(SynthesisTexture::default()), false);
        original.add_layer(SynthesisLayerKind::Patches);
        assert_eq!(original.add_patch(0), Some(1));
        for (knot, point) in [[0.1, 0.1], [0.4, 0.1], [0.4, 0.4], [0.1, 0.4]]
            .into_iter()
            .enumerate()
        {
            assert!(original.move_geometry_point(
                0,
                SynthesisGeometry::Patch { selection: 1, knot },
                point
            ));
        }
        let geometry = SynthesisGeometry::Patch {
            selection: 1,
            knot: 0,
        };
        let fields = original.geometry_fields(0, geometry);
        let weight = fields
            .iter()
            .position(|field| field.number_field() == Some(PatchFrequency(1)))
            .unwrap();
        let mut values: Vec<_> = fields.iter().map(|field| field.value()).collect();
        values[weight] = 0.;
        original.set_geometry_fields(0, geometry, &values).unwrap();
        assert!(
            original.set_image_source(0, EnvironmentImage::from_file_bytes(tga, "tga").unwrap())
        );
        let render = |edit: &SynthesisEdit| {
            let mut capacity = |_| Ok(true);
            let mut preflight = |_, _| Ok(());
            let mut effects = PointwiseRenderEffects::<()>::new(&mut capacity, &mut preflight);
            let image = edit
                .render_preset_thumbnail(None, ProceduralExecution::CallingThread, &mut effects)
                .unwrap();
            (0..4)
                .flat_map(|channel| image.channel(channel, true).unwrap().values())
                .collect::<Vec<_>>()
        };

        let baseline = render(&original);
        for (field, value) in [
            (Density, 0.),
            (Slide, 50.),
            (PatchScale, -50.),
            (ScaleJitter, 100.),
            (PatchAngle, 90.),
        ] {
            let fields = original.fields(Some(0));
            let index = fields
                .iter()
                .position(|item| item.number_field() == Some(field))
                .unwrap();
            let mut values: Vec<_> = fields.iter().map(|item| item.value()).collect();
            values[index] = value;
            let mut direct = original.clone();
            direct.set_fields(Some(0), &values).unwrap();
            let expected = render(&direct);
            assert_ne!(baseline, expected, "{field:?} must change visible output");

            let mut link =
                AdjustmentRecord::new(&[field.mapping_range().map(|bound| bound as f32)]);
            let component = &mut link.components_mut()[0];
            component.name = "Auto control".encode_utf16().collect();
            component.active = true;
            let mut linked = original.clone();
            assert_eq!(
                linked.set_number_adjustment(Some(0), field, Some((link, value))),
                Some(true)
            );
            assert_eq!(
                render(&linked),
                expected,
                "{field:?} must use the mapped value"
            );
        }

        for (field, values) in [
            (SynthesisPairField::GridFrequency, [7., 13.]),
            (SynthesisPairField::Move, [25., 50.]),
        ] {
            let fields = original.fields(Some(0));
            let mut direct_values: Vec<_> = fields.iter().map(|field| field.value()).collect();
            for (index, descriptor) in fields.iter().enumerate() {
                if let Some((pair, axis)) = descriptor.pair_field()
                    && pair == field
                {
                    direct_values[index] = values[axis];
                }
            }
            let mut direct = original.clone();
            direct.set_fields(Some(0), &direct_values).unwrap();
            let expected = render(&direct);
            assert_ne!(baseline, expected, "{field:?} must change visible output");

            let mut link = AdjustmentRecord::new(&[field.range().map(|bound| bound as f32); 2]);
            link.property_type = 1;
            for component in link.components_mut() {
                component.name = "Auto control".encode_utf16().collect();
                component.active = true;
            }
            let mut linked = original.clone();
            assert_eq!(
                linked.set_pair_adjustment(0, field, Some((link, values.map(Some)))),
                Some(true)
            );
            assert_eq!(render(&linked), expected);
        }

        let mut direct = original.clone();
        assert!(direct.set_effect_choice(0, 2));
        let expected = render(&direct);
        assert!(
            baseline != expected,
            "Columns must change visible patch placement"
        );

        let field = PatchSwitcher;
        let mut adjustment =
            AdjustmentRecord::new(&[field.mapping_range().map(|bound| bound as f32)]);
        let component = &mut adjustment.components_mut()[0];
        component.name = "Patch order".encode_utf16().collect();
        component.active = true;
        assert_eq!(
            original.set_number_adjustment(Some(0), field, Some((adjustment, 3.))),
            Some(true)
        );
        assert_eq!(original.patch_switcher_field(0).unwrap().value(), 0.);
        assert_eq!(original.effect_choice(0).unwrap().2, 2);
        assert!(!original.set_effect_choice(0, 1));
        assert_eq!(render(&original), expected);
        assert_eq!(
            original.set_number_adjustment(Some(0), field, None),
            Some(true)
        );
        assert_eq!(original.patch_switcher_field(0).unwrap().value(), 2.);
        assert_eq!(render(&original), expected);
    }

    #[test]
    fn synthesis_numeric_adjustments_preserve_raw_wire_and_detached_revisions() {
        let initial = Arc::new(SynthesisTexture::default());
        let mut edit = SynthesisEdit::new(initial.clone(), true);
        edit.add_layer(SynthesisLayerKind::Solid);
        edit.add_layer(SynthesisLayerKind::Patches);
        let mut texture = edit.into_local_replacement(&initial).unwrap().unwrap();
        let authored = Arc::make_mut(&mut texture);
        authored.master.seed_adjustment = link(1);
        let Paint::Patches(patches) = &mut Arc::make_mut(&mut authored.layers[0]).paint else {
            panic!("expected a Patches layer");
        };
        patches.selections[0].number_links.set(0, link(2));
        patches.automatic.position_jitter_adjustments = [link(3), link(4)];
        patches.automatic.angle_jitter_adjustment = link(5);
        let before = SynthesisRecord::from_texture(texture);
        let resolved = before
            .map_numeric_adjustments::<()>(|record, _| {
                Ok(Some(
                    [12.5, 5.96, 1.1, 2.2, 3.3][usize::from(record.guid[0] - 1)],
                ))
            })
            .unwrap()
            .unwrap();

        let state = |record: &SynthesisRecord| {
            let Paint::Patches(patches) = &record.0.layers[0].paint else {
                panic!("expected a Patches layer");
            };
            (
                record.0.master.effective_seed(),
                patches.selections[0].effective_frequency(),
                patches.automatic.effective_position_jitter(),
                patches.automatic.effective_angle_jitter(),
            )
        };
        assert_eq!(state(&before), (0, 100, [20., 20.], 10.));
        assert_eq!(state(&resolved), (12, 6, [1.1, 2.2], 3.3));
        assert!(Arc::ptr_eq(&before.0.layers[1], &resolved.0.layers[1]));
        let mut records = Vec::new();
        resolved.visit_numeric_adjustments(|adjustment| records.push(adjustment.guid[0]));
        assert_eq!(records, [1, 2, 3, 4, 5]);

        let wire = |record: &SynthesisRecord| {
            let mut writer = BinaryWriter::default();
            record.write(&mut writer).unwrap();
            writer.bytes().to_vec()
        };
        let render_values = |record: &SynthesisRecord| {
            let mut words = Vec::new();
            record.0.append_render_values(&mut words);
            words
        };
        assert_eq!(wire(&before), wire(&resolved));
        assert_ne!(render_values(&before), render_values(&resolved));
        assert!(
            resolved
                .map_numeric_adjustments::<()>(|record, _| Ok(match record.guid[0] {
                    2 => Some(6.1),
                    3 => Some(-0.1),
                    _ => None,
                }))
                .unwrap()
                .is_none()
        );
        let partial = resolved
            .map_numeric_adjustments::<()>(|record, _| Ok((record.guid[0] == 3).then_some(20.)))
            .unwrap()
            .unwrap();
        assert_eq!(state(&partial), (12, 6, [20., 2.2], 3.3));
        assert_eq!(state(&resolved), (12, 6, [1.1, 2.2], 3.3));
    }
}
