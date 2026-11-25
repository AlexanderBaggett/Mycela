//! Detached editor drafts share unmodified layers and source images with the graph.
use super::{
    Layer, LayerColor, Master, Paint, Source, SynthesisColorField, SynthesisColorTarget,
    SynthesisNumberField, SynthesisPairField, SynthesisTexture, patches, photo_blend::PhotoBlend,
    repeat::Pattern,
};
use crate::environment_record::ImageAsset;
use crate::pointwise::{PointwiseError, PointwiseFilter, PointwiseRenderEffects, SharedImageMap};
use crate::{
    AdjustmentRecord, BinaryCursor, BinaryWriter, EnvironmentBlendMode, EnvironmentImage,
    HistoryDirection, HistoryError, ImageGeometry, ProceduralExecution,
};
use std::sync::{Arc, Weak};

mod geometry;
pub use geometry::{
    SynthesisCropHandle, SynthesisGeometry, SynthesisPatchResizeHandle, SynthesisPatchShape,
    SynthesisPatchTransform, SynthesisPatchTransformBounds, SynthesisPatchTransformField,
    SynthesisPlacementClipboard, SynthesisPlacementField, SynthesisPlacementGeometry,
    SynthesisPlacementTransform,
};

const PRESET_MAGIC: &str = "GeneticaTexSynth";
const PRESET_VERSION: i32 = 383;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SynthesisLayerKind {
    Solid,
    Repeating,
    Blend,
    Patches,
}

impl SynthesisLayerKind {
    pub const ALL: [Self; 4] = [Self::Solid, Self::Repeating, Self::Blend, Self::Patches];

    pub fn label(self) -> &'static str {
        match self {
            Self::Solid => "Solid color",
            Self::Repeating => "Repeating pattern",
            Self::Blend => "Blend synthesis",
            Self::Patches => "Patches",
        }
    }

    fn paint(self) -> Paint {
        match self {
            Self::Solid => Paint::Solid(super::SolidPaint {
                color: [1.; 4],
                color_adjustment: None,
                animations: Default::default(),
            }),
            Self::Repeating => Paint::Repeat(Box::new(Pattern {
                source: Source::Input,
                columns: 3,
                rows: 3,
                points: [0.15, 0.5, 0.85]
                    .into_iter()
                    .flat_map(|x| [0.15, 0.5, 0.85].map(|y| [x, y]))
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                equalize: true,
                strength: 50.,
                strength_adjustment: None,
                feather: 2.,
                feather_adjustment: None,
                frequency: 5,
                frequency_adjustment: None,
                animations: Default::default(),
            })),
            Self::Blend => Paint::Blend(Box::new(PhotoBlend {
                source: Source::Input,
                rectangle: [0., 0., 1., 1.],
                equalize: true,
                strength: 50.,
                strength_adjustment: None,
                mode: 1,
                stretch: [20.; 2],
                edge_feather: [100.; 2],
                star_feather: 50.,
                shaping_links: Default::default(),
                repeat: [1; 2],
                repeat_adjustment: None,
                animations: Default::default(),
            })),
            Self::Patches => Paint::Patches(Box::new(patches::Patches {
                source: Source::Input,
                equalize: true,
                strength: 50.,
                strength_adjustment: None,
                selections: vec![geometry::new_selection()],
                selections_parked: false,
                automatic: patches::Automatic {
                    enabled: true,
                    frequency: [4; 2],
                    frequency_adjustment: None,
                    offset: [0.; 2],
                    offset_adjustment: None,
                    density: 100.,
                    density_adjustment: None,
                    slide: 0.,
                    slide_adjustment: None,
                    position_jitter: [20.; 2],
                    position_jitter_adjustments: [None, None],
                    scale: 0.,
                    scale_adjustment: None,
                    scale_jitter: 10.,
                    scale_jitter_adjustment: None,
                    angle: 0.,
                    angle_adjustment: None,
                    angle_jitter: 10.,
                    angle_jitter_adjustment: None,
                    switcher: 0,
                    switcher_adjustment: None,
                    animations: Default::default(),
                },
                manual: Vec::new(),
                repeat: [1; 2],
                repeat_adjustment: None,
                animations: Default::default(),
            })),
        }
    }
}

impl Paint {
    fn kind(&self) -> SynthesisLayerKind {
        match self {
            Self::Solid(_) => SynthesisLayerKind::Solid,
            Self::Repeat(_) => SynthesisLayerKind::Repeating,
            Self::Blend(_) => SynthesisLayerKind::Blend,
            Self::Patches(_) => SynthesisLayerKind::Patches,
        }
    }

    fn source(&self) -> Option<&Source> {
        match self {
            Self::Solid(_) => None,
            Self::Repeat(layer) => Some(&layer.source),
            Self::Blend(layer) => Some(&layer.source),
            Self::Patches(layer) => Some(&layer.source),
        }
    }

    fn source_mut(&mut self) -> Option<&mut Source> {
        match self {
            Self::Solid(_) => None,
            Self::Repeat(layer) => Some(&mut layer.source),
            Self::Blend(layer) => Some(&mut layer.source),
            Self::Patches(layer) => Some(&mut layer.source),
        }
    }
}

#[derive(Clone, Copy)]
pub struct SynthesisField {
    label: &'static str,
    value: f64,
    range: [f64; 2],
    whole: bool,
    number_field: Option<SynthesisNumberField>,
    pair_field: Option<(SynthesisPairField, usize)>,
    color_target: Option<SynthesisColorTarget>,
    automatic_placement: bool,
}

impl SynthesisField {
    pub fn label(self) -> &'static str {
        self.label
    }
    pub fn value(self) -> f64 {
        self.value
    }
    pub fn range(self) -> [f64; 2] {
        self.range
    }
    pub fn is_whole(self) -> bool {
        self.whole
    }
    /// Identifies an authored field that can receive a scalar Lab Number link.
    pub fn number_field(self) -> Option<SynthesisNumberField> {
        self.number_field
    }
    /// Identifies the axis of a paired Lab receiver. X is 0 and Y is 1.
    pub fn pair_field(self) -> Option<(SynthesisPairField, usize)> {
        self.pair_field
    }
    /// The Color property edited by this RGB component, when present.
    pub fn color_target(self) -> Option<SynthesisColorTarget> {
        self.color_target
    }
    /// Whether the editor should enable this field only with Automatic Placement on.
    pub fn requires_automatic_placement(self) -> bool {
        self.automatic_placement
    }
}

/// A draft is tied to the exact texture revision it was opened from.
#[derive(Clone)]
pub struct SynthesisEdit {
    before: Arc<SynthesisTexture>,
    value: SynthesisTexture,
    changed: bool,
    from_preset: bool,
    local_controls: bool,
}

impl super::SynthesisRecord {
    pub(crate) fn draft(&self) -> SynthesisEdit {
        SynthesisEdit::new(self.texture().clone(), true)
    }

    pub(crate) fn apply_draft(&self, edit: SynthesisEdit) -> Result<Option<Self>, &'static str> {
        edit.into_local_replacement(self.texture())
            .map(|next| next.map(Self::from_texture))
    }
}

impl SynthesisEdit {
    pub const ERODE_MODES: [&'static str; 6] = [
        "Upper Brightness",
        "Upper Darkness",
        "Lower Brightness",
        "Lower Darkness",
        "Upper Brighter",
        "Upper Darker",
    ];

    pub(in crate::pointwise::canvas) fn new(
        before: Arc<SynthesisTexture>,
        local_controls: bool,
    ) -> Self {
        Self {
            value: (*before).clone(),
            before,
            changed: false,
            from_preset: false,
            local_controls,
        }
    }

    pub(in crate::pointwise::canvas) fn into_local_replacement(
        self,
        expected: &Arc<SynthesisTexture>,
    ) -> Result<Option<Arc<SynthesisTexture>>, &'static str> {
        if !self.local_controls || !Arc::ptr_eq(&self.before, expected) {
            return Err("The texture changed. Revert edits before continuing.");
        }
        Ok(self.changed.then(|| Arc::new(self.value)))
    }

    pub fn replaces_scale_and_seed(&self) -> bool {
        self.from_preset && !self.local_controls
    }

    pub(crate) fn same_revision(&self, other: &Self) -> bool {
        self.local_controls == other.local_controls && Arc::ptr_eq(&self.before, &other.before)
    }

    /// The current effective value, which includes the resolved Lab Number.
    pub fn number_value(&self, layer: Option<usize>, field: SynthesisNumberField) -> Option<f64> {
        if matches!(
            field,
            SynthesisNumberField::Seed | SynthesisNumberField::Scale
        ) && !self.local_controls
        {
            return None;
        }
        let (raw, link) = self.value.number(layer, field)?;
        Some(link.map_or(raw, |link| link.effective(raw)))
    }

    /// The retained adjustment wire record for this field, if one is present.
    pub fn number_adjustment(
        &self,
        layer: Option<usize>,
        field: SynthesisNumberField,
    ) -> Option<&AdjustmentRecord> {
        if matches!(
            field,
            SynthesisNumberField::Seed | SynthesisNumberField::Scale
        ) && !self.local_controls
        {
            return None;
        }
        Some(self.value.number(layer, field)?.1?.record())
    }

    /// Installs a resolved link or unlinks while keeping the latest effective value.
    /// Returns `None` for an invalid address, link record or mapped value.
    pub(crate) fn set_number_adjustment(
        &mut self,
        layer: Option<usize>,
        field: SynthesisNumberField,
        link: Option<(AdjustmentRecord, f64)>,
    ) -> Option<bool> {
        self.set_number_adjustment_with_input_geometry(layer, field, link, None)
    }

    pub(crate) fn set_number_adjustment_with_input_geometry(
        &mut self,
        layer: Option<usize>,
        field: SynthesisNumberField,
        link: Option<(AdjustmentRecord, f64)>,
        input: Option<crate::ImageGeometry>,
    ) -> Option<bool> {
        if matches!(
            field,
            SynthesisNumberField::Seed | SynthesisNumberField::Scale
        ) && !self.local_controls
        {
            return None;
        }
        let changed = self
            .value
            .set_number_adjustment_with_input_geometry(layer, field, link, input)?;
        self.changed |= changed;
        Some(changed)
    }

    pub fn color_value(&self, target: SynthesisColorTarget) -> Option<[f64; 3]> {
        self.value.color_value(target)
    }

    pub fn is_color_linked(&self, target: SynthesisColorTarget) -> bool {
        self.color_link_key(target).is_some()
    }

    pub(crate) fn color_link_key(
        &self,
        target: SynthesisColorTarget,
    ) -> Option<super::color_link::SynthesisColorKey<'_>> {
        super::color_link::color_key(self.value.color_adjustment(target)?)
    }

    pub(crate) fn set_color_adjustment(
        &mut self,
        target: SynthesisColorTarget,
        link: Option<(AdjustmentRecord, [f64; 3])>,
    ) -> Option<bool> {
        let changed = self.value.set_color_adjustment(target, link)?;
        self.changed |= changed;
        Some(changed)
    }

    /// Set an unlinked Color's RGB channels as one authoring action.
    pub fn set_color_value(&mut self, target: SynthesisColorTarget, rgb: [f64; 3]) -> bool {
        if self.is_color_linked(target) || !rgb.iter().all(|value| (0. ..=1.).contains(value)) {
            return false;
        }
        let Some((raw, _)) = self.value.color_slot_mut(target) else {
            return false;
        };
        if raw[1..] == rgb {
            return false;
        }
        raw[1..].copy_from_slice(&rgb);
        self.changed = true;
        true
    }

    /// Returns the current effective X and Y values of a paired property.
    pub fn pair_value(&self, layer: usize, field: SynthesisPairField) -> Option<[f64; 2]> {
        self.value.pair_value(layer, field)
    }

    /// Returns the shared retained record for both axes of a paired property.
    pub fn pair_adjustment(
        &self,
        layer: usize,
        field: SynthesisPairField,
    ) -> Option<&AdjustmentRecord> {
        self.value.pair_adjustment(layer, field)
    }

    /// Installs a paired link and freezes any removed axis at its last effective value.
    pub(crate) fn set_pair_adjustment(
        &mut self,
        layer: usize,
        field: SynthesisPairField,
        link: Option<(AdjustmentRecord, [Option<f64>; 2])>,
    ) -> Option<bool> {
        let changed = self.value.set_pair_adjustment(layer, field, link)?;
        self.changed |= changed;
        Some(changed)
    }

    fn retain_linked_numbers(
        &self,
        layer: Option<usize>,
        fields: &[SynthesisField],
        values: &[f64],
    ) -> Vec<f64> {
        let mut values = values.to_vec();
        for (field, value) in fields.iter().zip(&mut values) {
            let scalar = field
                .number_field()
                .and_then(|number| self.number_adjustment(layer, number))
                .and_then(|record| record.components().first());
            let paired = layer
                .zip(field.pair_field())
                .is_some_and(|(layer, (pair, axis))| {
                    self.pair_adjustment(layer, pair).is_some_and(|record| {
                        record
                            .components()
                            .iter()
                            .enumerate()
                            .any(|(index, component)| {
                                (matches!(
                                    pair,
                                    SynthesisPairField::PhotoRepeat
                                        | SynthesisPairField::ManualRepeat
                                ) || index == pair.component_index(axis))
                                    && component.active
                                    && !component.name.is_empty()
                            })
                    })
                });
            if scalar.is_some_and(|component| component.active && !component.name.is_empty())
                || paired
                || field
                    .color_target()
                    .is_some_and(|target| self.is_color_linked(target))
            {
                *value = field.value();
            }
        }
        values
    }
    pub fn layers(&self) -> usize {
        self.value.layers.len()
    }

    pub fn layer(&self, index: usize) -> Option<(usize, SynthesisLayerKind)> {
        let layer = self.value.layers.get(index)?;
        Some((Arc::as_ptr(&layer.identity) as usize, layer.paint.kind()))
    }

    pub fn add_layer(&mut self, kind: SynthesisLayerKind) {
        let blend = super::super::CanvasStyle::default().passes.remove(0).blend;
        self.value.layers.insert(
            0,
            Arc::new(Layer {
                identity: Arc::new(()),
                blend,
                color_controls: LayerColor::default(),
                paint: kind.paint(),
            }),
        );
        self.changed = true;
    }

    pub fn remove_layer(&mut self, index: usize) -> bool {
        if index >= self.layers() {
            return false;
        }
        self.value.layers.remove(index);
        self.changed = true;
        true
    }

    pub fn clear_layers(&mut self) -> bool {
        if self.value.layers.is_empty() {
            return false;
        }
        self.value.layers.clear();
        self.changed = true;
        true
    }

    pub fn set_layer_kind(&mut self, index: usize, kind: SynthesisLayerKind) -> bool {
        let Some(layer) = self.value.layers.get_mut(index) else {
            return false;
        };
        if layer.paint.kind() == kind {
            return false;
        }
        let mut paint = kind.paint();
        if let Some(source) = paint.source_mut() {
            *source = match &layer.paint {
                Paint::Solid(_) => Source::Input,
                Paint::Repeat(layer) => layer.source.clone(),
                Paint::Blend(layer) => layer.source.clone(),
                Paint::Patches(layer) => layer.source.clone(),
            };
        }
        Arc::make_mut(layer).paint = paint;
        self.changed = true;
        true
    }

    pub fn move_layer(&mut self, index: usize, to: usize) -> bool {
        if index == to || index >= self.layers() || to >= self.layers() {
            return false;
        }
        let layer = self.value.layers.remove(index);
        self.value.layers.insert(to, layer);
        self.changed = true;
        true
    }

    pub fn source_label(&self, index: usize) -> Option<String> {
        let source = self.layer_source(index)?;
        Some(match source {
            Source::Input => "Graph input".into(),
            Source::Missing => "No image".into(),
            Source::Asset(asset) => match asset.as_ref() {
                ImageAsset::Embedded(_) => "Embedded image".into(),
                ImageAsset::Library { path, .. } => String::from_utf16_lossy(path),
            },
        })
    }

    pub fn set_input_source(&mut self, index: usize) -> bool {
        self.set_source(index, Source::Input)
    }

    pub fn set_image_source(&mut self, index: usize, image: EnvironmentImage) -> bool {
        self.set_source(index, Source::Asset(Arc::new(ImageAsset::Embedded(image))))
    }

    /// Clears the assigned photo or graph input and freezes its dependent Lab values.
    pub fn clear_source(&mut self, index: usize) -> bool {
        self.set_source(index, Source::Missing)
    }

    fn layer_source(&self, index: usize) -> Option<&Source> {
        self.value.layers.get(index)?.paint.source()
    }

    pub fn layer_has_source(&self, index: usize) -> bool {
        matches!(
            self.layer_source(index),
            Some(Source::Input | Source::Asset(_))
        )
    }

    pub fn layer_uses_input(&self, index: usize) -> bool {
        matches!(self.layer_source(index), Some(Source::Input))
    }

    /// Geometry and color-control edits keep the same underlying guide source.
    pub fn same_layer_source(&self, index: usize, other: &Self, other_index: usize) -> bool {
        match (self.layer_source(index), other.layer_source(other_index)) {
            (Some(Source::Input), Some(Source::Input)) => true,
            (Some(Source::Asset(left)), Some(Source::Asset(right))) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }

    /// Read the layer's unadjusted image asset for a guide at a bounded size.
    /// Graph Input layers are rendered by the caller using their graph context.
    pub fn layer_asset_preview<E>(
        &self,
        index: usize,
        max_side: i32,
        library: Option<&crate::EnvironmentResourceLibrary>,
        execution: ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let Some(Source::Asset(asset)) = self.layer_source(index) else {
            return Err(PointwiseError::Argument);
        };
        crate::pointwise::check_render_cancelled(cancelled)?;
        let dimensions = asset.source_geometry()?;
        let largest = dimensions.width().max(dimensions.height());
        if max_side < 1 || dimensions.width() < 1 || dimensions.height() < 1 {
            return Err(PointwiseError::Argument);
        }
        let width = if largest <= max_side {
            dimensions.width()
        } else {
            ((i64::from(dimensions.width()) * i64::from(max_side)) / i64::from(largest)).max(1)
                as i32
        };
        let image = asset.synthesis_source(width, library, execution, cancelled)?;
        crate::pointwise::check_render_cancelled(cancelled)?;
        let actual = image.geometry();
        let largest = actual.width().max(actual.height());
        if largest <= max_side {
            Ok(image)
        } else {
            // The minimum request width and a missing library image can yield
            // a larger raster than the proportional width would imply.
            let fit =
                |side| ((i64::from(side) * i64::from(max_side)) / i64::from(largest)).max(1) as i32;
            crate::pointwise::image_resize::resize_clamped(
                &image,
                fit(actual.width()),
                fit(actual.height()),
                execution,
            )
        }
    }

    /// Load the unadjusted asset for the full-size photo editor. Embedded
    /// photos keep their native aspect and use at most the requested width.
    /// Library photos use the highest tier present in the configured library.
    pub fn layer_asset_editor_preview<E>(
        &self,
        index: usize,
        embedded_width: i32,
        library: Option<&crate::EnvironmentResourceLibrary>,
        execution: ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let Some(Source::Asset(asset)) = self.layer_source(index) else {
            return Err(PointwiseError::Argument);
        };
        if embedded_width < 1 {
            return Err(PointwiseError::Argument);
        }
        crate::pointwise::check_render_cancelled(cancelled)?;
        let image = asset.synthesis_editor_source(embedded_width, library, execution, cancelled)?;
        crate::pointwise::check_render_cancelled(cancelled)?;
        Ok(image)
    }

    pub fn equalizes_source(&self, index: usize) -> Option<bool> {
        match &self.value.layers.get(index)?.paint {
            Paint::Solid(_) => None,
            Paint::Repeat(layer) => Some(layer.equalize),
            Paint::Blend(layer) => Some(layer.equalize),
            Paint::Patches(layer) => Some(layer.equalize),
        }
    }

    pub fn set_equalize_source(&mut self, index: usize, value: bool) -> bool {
        if self.equalizes_source(index).is_none_or(|old| old == value) {
            return false;
        }
        if !value {
            self.set_number_adjustment(Some(index), SynthesisNumberField::EqualizeStrength, None);
        }
        match &mut Arc::make_mut(&mut self.value.layers[index]).paint {
            Paint::Solid(_) => return false,
            Paint::Repeat(layer) => layer.equalize = value,
            Paint::Blend(layer) => layer.equalize = value,
            Paint::Patches(layer) => layer.equalize = value,
        }
        self.changed = true;
        true
    }

    pub fn automatic_placement(&self, index: usize) -> Option<bool> {
        let Paint::Patches(layer) = &self.value.layers.get(index)?.paint else {
            return None;
        };
        Some(layer.automatic.enabled)
    }

    pub fn set_automatic_placement(&mut self, index: usize, value: bool) -> bool {
        if self
            .automatic_placement(index)
            .is_none_or(|old| old == value)
        {
            return false;
        }
        if !value {
            for field in SynthesisNumberField::AUTOMATIC_FIELDS {
                self.set_number_adjustment(Some(index), field, None);
            }
            for field in SynthesisPairField::AUTOMATIC {
                self.set_pair_adjustment(index, field, None);
            }
        }
        let Paint::Patches(layer) = &mut Arc::make_mut(&mut self.value.layers[index]).paint else {
            return false;
        };
        layer.automatic.enabled = value;
        self.changed = true;
        true
    }

    pub fn effect_choice(
        &self,
        index: usize,
    ) -> Option<(&'static str, &'static [&'static str], usize)> {
        match &self.value.layers.get(index)?.paint {
            Paint::Blend(layer) => Some((
                "Synthesis mode",
                &["Fold Edges", "Star Blend"],
                layer.mode.saturating_sub(1) as usize,
            )),
            Paint::Patches(layer) => Some((
                "Patch switcher",
                &patches::PATCH_SWITCHERS,
                layer.automatic.effective_switcher() as usize,
            )),
            _ => None,
        }
    }

    /// Identifies the active Photo Blend shaping mode.
    pub fn photo_blend_is_fold(&self, index: usize) -> Option<bool> {
        let Paint::Blend(layer) = &self.value.layers.get(index)?.paint else {
            return None;
        };
        Some(layer.mode != 2)
    }

    /// Describes the Patches choice receiver for the field-local Lab editor.
    pub fn patch_switcher_field(&self, index: usize) -> Option<SynthesisField> {
        let field = SynthesisNumberField::PatchSwitcher;
        let (value, _) = self.value.number(Some(index), field)?;
        Some(SynthesisField {
            label: "Patch switcher",
            value,
            range: field.range(),
            whole: true,
            number_field: Some(field),
            pair_field: None,
            color_target: None,
            automatic_placement: true,
        })
    }

    pub fn set_effect_choice(&mut self, index: usize, selected: usize) -> bool {
        if self
            .effect_choice(index)
            .is_none_or(|(_, choices, old)| selected >= choices.len() || old == selected)
        {
            return false;
        }
        if self.photo_blend_is_fold(index).is_some() {
            use SynthesisNumberField::PhotoStarFeather;
            for field in SynthesisNumberField::PHOTO_BLEND_FIELDS {
                if (field == PhotoStarFeather) == (selected == 0) {
                    self.set_number_adjustment(Some(index), field, None);
                }
            }
        }
        match &mut Arc::make_mut(&mut self.value.layers[index]).paint {
            Paint::Blend(layer) => layer.mode = selected as i32 + 1,
            Paint::Patches(layer)
                if !layer
                    .automatic
                    .switcher_adjustment
                    .as_ref()
                    .is_some_and(|link| link.is_active()) =>
            {
                layer.automatic.switcher = selected as i32;
            }
            _ => return false,
        }
        self.changed = true;
        true
    }

    fn set_source(&mut self, index: usize, value: Source) -> bool {
        let Some(layer) = self.value.layers.get_mut(index) else {
            return false;
        };
        let paint = &mut Arc::make_mut(layer).paint;
        let Some(source) = paint.source() else {
            return false;
        };
        if matches!(
            (source, &value),
            (Source::Input, Source::Input) | (Source::Missing, Source::Missing)
        ) {
            return false;
        }
        let clearing = matches!(value, Source::Missing);
        if let Paint::Patches(patches) = paint {
            if !clearing
                && matches!(patches.source, Source::Missing)
                && patches.selections.is_empty()
            {
                patches.selections.push(geometry::new_selection());
            }
            patches.selections_parked = clearing && matches!(patches.source, Source::Input);
        }
        *paint.source_mut().expect("non-solid paint") = value;
        if clearing {
            for field in SynthesisNumberField::PHOTO_FIELDS {
                self.set_number_adjustment(Some(index), field, None);
            }
        }
        self.changed = true;
        true
    }

    pub fn blend_mode(&self, index: usize) -> Option<EnvironmentBlendMode> {
        let mode = self.number_value(Some(index), SynthesisNumberField::BlendMode)? as i32;
        EnvironmentBlendMode::ALL
            .get(usize::try_from(mode).ok()?)
            .copied()
    }

    pub fn erode_mode(&self, index: usize) -> Option<usize> {
        let mode = self.number_value(Some(index), SynthesisNumberField::ErodeMode)? as i32;
        let mode = usize::try_from(mode).ok()?;
        (mode < Self::ERODE_MODES.len()).then_some(mode)
    }

    /// Describes one of the Layer Blend choices for its local Lab link menu.
    pub fn blend_choice_field(
        &self,
        index: usize,
        field: SynthesisNumberField,
    ) -> Option<SynthesisField> {
        let label = match field {
            SynthesisNumberField::BlendMode => "Blend mode",
            SynthesisNumberField::ErodeMode => "Erode mode",
            _ => return None,
        };
        let (value, _) = self.value.number(Some(index), field)?;
        Some(SynthesisField {
            label,
            value,
            range: field.range(),
            whole: true,
            number_field: Some(field),
            pair_field: None,
            color_target: None,
            automatic_placement: false,
        })
    }

    pub fn set_blend_mode(&mut self, index: usize, value: EnvironmentBlendMode) -> bool {
        let Some(mode) = EnvironmentBlendMode::ALL
            .iter()
            .position(|mode| *mode == value)
        else {
            return false;
        };
        self.set_blend_choice(index, SynthesisNumberField::BlendMode, mode)
    }

    pub fn set_erode_mode(&mut self, index: usize, mode: usize) -> bool {
        mode < Self::ERODE_MODES.len()
            && self.set_blend_choice(index, SynthesisNumberField::ErodeMode, mode)
    }

    fn set_blend_choice(&mut self, index: usize, field: SynthesisNumberField, mode: usize) -> bool {
        let Some((raw, link)) = self.value.number(Some(index), field) else {
            return false;
        };
        if raw == mode as f64 || link.is_some_and(super::NumericLink::is_active) {
            return false;
        }
        Arc::make_mut(&mut self.value.layers[index])
            .blend
            .set_raw_synthesis_number(field, mode as f64);
        self.changed = true;
        true
    }

    pub fn fields(&self, layer: Option<usize>) -> Vec<SynthesisField> {
        let mut view = self.value.clone();
        let mut fields = Vec::new();
        apply_fields(&mut view, layer, self.local_controls, |field| {
            fields.push(field);
            field.value
        });
        fields
    }

    pub fn set_fields(&mut self, layer: Option<usize>, values: &[f64]) -> Result<(), &'static str> {
        let fields = self.fields(layer);
        let mut values = self.retain_linked_numbers(layer, &fields, values);
        if !changed_fields(&fields, &values, "The selected layer changed.")? {
            return Ok(());
        }
        for (field, value) in fields.iter().zip(&mut values) {
            if *value == field.value {
                continue;
            }
            if matches!(field.pair_field(), Some((SynthesisPairField::Move, _))) {
                *value = SynthesisPairField::Move
                    .normalize(*value)
                    .ok_or(field.label)?;
            }
            if let Some(
                number @ (SynthesisNumberField::EqualizeStrength
                | SynthesisNumberField::RepeatFeather
                | SynthesisNumberField::BlendOpacity
                | SynthesisNumberField::PhotoStretchX
                | SynthesisNumberField::PhotoStretchY
                | SynthesisNumberField::PhotoFeatherX
                | SynthesisNumberField::PhotoFeatherY
                | SynthesisNumberField::PhotoStarFeather),
            ) = field.number_field()
            {
                *value = number.normalize(*value).ok_or(field.label)?;
            }
        }
        if fields
            .iter()
            .zip(&values)
            .all(|(field, value)| field.value == *value)
        {
            return Ok(());
        }
        let mut values = values.into_iter();
        apply_fields(&mut self.value, layer, self.local_controls, |field| {
            values.next().unwrap_or(field.value)
        });
        self.changed = true;
        Ok(())
    }

    /// Replace the draft's texture with a standalone Genetica preset.
    pub fn load_preset(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.apply_preset(bytes, false)
    }

    /// Insert the preset's layers on top and adopt its master controls.
    pub fn insert_preset_layers(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.apply_preset(bytes, true)
    }

    pub fn read_preset_thumbnail(
        reader: &mut BinaryCursor<'_>,
    ) -> Result<Vec<u8>, crate::NodeWireError> {
        let (_, size) = read_preset_header(reader)?;
        Ok(reader.read_bytes(size)?)
    }

    fn apply_preset(&mut self, bytes: &[u8], insert: bool) -> Result<(), String> {
        let read = || -> Result<SynthesisTexture, crate::NodeWireError> {
            let mut cursor = BinaryCursor::new(bytes)?;
            let (encoding, thumbnail) = read_preset_header(&mut cursor)?;
            let payload = cursor.position()? + thumbnail;
            cursor.seek(i64::from(payload))?;
            let texture = SynthesisTexture::read(&mut cursor, f64::from(encoding) / 100.)?;
            if cursor.position()? != cursor.length()? {
                return Err(crate::BinaryError::Count.into());
            }
            Ok(texture)
        };
        let mut texture =
            read().map_err(|error| format!("Couldn’t open the Synthesis preset. {error:?}"))?;
        if insert {
            texture
                .layers
                .try_reserve(self.value.layers.len())
                .map_err(|_| "There is not enough memory to insert the preset layers.")?;
            texture.layers.extend(self.value.layers.iter().cloned());
        }
        self.value = texture;
        self.changed = true;
        self.from_preset = true;
        Ok(())
    }

    pub const PRESET_THUMBNAIL_SIZE: i32 = 128;

    pub fn uses_input(&self) -> bool {
        self.value.uses_input()
    }

    /// Freezes this editor revision's texture for an off-thread preview.
    pub fn record(&self) -> super::SynthesisRecord {
        let texture = if self.changed {
            Arc::new(self.value.clone())
        } else {
            self.before.clone()
        };
        super::SynthesisRecord::from_texture(texture)
    }

    /// The thumbnail may use the current graph input. The saved Texture still
    /// refers to the input slot of whichever node opens the preset.
    pub fn render_preset_thumbnail<E: 'static>(
        &self,
        input: Option<&SharedImageMap>,
        execution: ProceduralExecution,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let size = Self::PRESET_THUMBNAIL_SIZE;
        let geometry = ImageGeometry::new(size, size);
        let fallback = if input.is_none() && self.uses_input() {
            Some(super::editor_input_sample(geometry)?)
        } else {
            None
        };
        self.value.render_style(
            input.or(fallback.as_ref()),
            geometry,
            0.,
            0,
            execution,
            effects,
        )
    }

    /// Write the standalone Genetica container with an encoded JPEG thumbnail.
    pub fn save_preset(
        &self,
        thumbnail: &[u8],
        writer: &mut BinaryWriter,
    ) -> Result<(), crate::NodeWireError> {
        let length = i32::try_from(thumbnail.len()).map_err(|_| crate::BinaryError::Count)?;
        writer.write_utf16(&PRESET_MAGIC.encode_utf16().collect::<Vec<_>>())?;
        writer.write_i32(PRESET_VERSION)?;
        writer.write_i32(length)?;
        writer.write_bytes(thumbnail)?;
        self.value.write(writer)
    }
}

fn read_preset_header(cursor: &mut BinaryCursor<'_>) -> Result<(i32, i32), crate::NodeWireError> {
    if !cursor
        .read_string()?
        .into_iter()
        .eq(PRESET_MAGIC.encode_utf16())
    {
        return Err(crate::BinaryError::LengthEncoding.into());
    }
    let version = cursor.read_i32()?;
    let size = cursor.read_i32()?;
    if size < 0 || size > cursor.length()? - cursor.position()? {
        return Err(crate::BinaryError::Count.into());
    }
    Ok((version, size))
}

fn changed_fields(
    fields: &[SynthesisField],
    values: &[f64],
    missing: &'static str,
) -> Result<bool, &'static str> {
    if fields.len() != values.len() || fields.is_empty() {
        return Err(missing);
    }
    let mut changed = false;
    for (field, &value) in fields.iter().zip(values) {
        if value != field.value {
            if !value.is_finite()
                || !(field.range[0]..=field.range[1]).contains(&value)
                || (field.whole && value.fract() != 0.)
            {
                return Err(field.label);
            }
            changed = true;
        }
    }
    Ok(changed)
}

fn apply_fields(
    texture: &mut SynthesisTexture,
    layer: Option<usize>,
    local_controls: bool,
    mut update: impl FnMut(SynthesisField) -> f64,
) {
    macro_rules! controls {
        ($($value:expr => $($automatic:ident)? $label:literal, $min:expr, $max:expr $(, $whole:literal)? $(=> $number:expr)? $(, pair $pair:expr)? $(, color $color:expr)?;)*) => {{
            $({
                let field = &mut $value;
                *field = update(SynthesisField {
                    label: $label, value: f64::from(*field), range: [$min,$max], whole: controls!(@whole $($whole)?),
                    number_field: controls!(@number $($number)?),
                    pair_field: controls!(@number $($pair)?),
                    color_target: controls!(@number $($color)?),
                    automatic_placement: controls!(@automatic $($automatic)?),
                }) as _;
            })*
        }};
        (@whole $whole:literal) => { $whole };
        (@whole) => { false };
        (@number $field:expr) => { Some($field) };
        (@number) => { None };
        (@automatic auto) => { true };
        (@automatic) => { false };
    }
    macro_rules! color {
        ($color:expr, $target:expr) => {
            controls! {
                $color[1] => "Red", 0., 1., color $target;
                $color[2] => "Green", 0., 1., color $target;
                $color[3] => "Blue", 0., 1., color $target;
            }
        };
    }
    let Some(index) = layer else {
        let Master {
            brightness,
            contrast,
            hue,
            saturation,
            sharpen,
            sharpen_radius,
            color,
            scale,
            seed,
            ..
        } = &mut texture.master;
        controls! {
            *brightness => "Brightness", -100., 100. => SynthesisNumberField::MasterColor(SynthesisColorField::Brightness);
            *contrast => "Contrast", -100., 100. => SynthesisNumberField::MasterColor(SynthesisColorField::Contrast);
            *hue => "Hue", -360., 360. => SynthesisNumberField::MasterColor(SynthesisColorField::Hue);
            *saturation => "Saturation", -100., 1000. => SynthesisNumberField::MasterColor(SynthesisColorField::Saturation);
            *sharpen => "Sharpen", 0., 100. => SynthesisNumberField::MasterColor(SynthesisColorField::Sharpen);
            *sharpen_radius => "Sharpen radius", 0., 100.;
        }
        color!(color, SynthesisColorTarget::Master);
        if local_controls {
            controls! {
                *scale => "Texture Scale", -10., 10. => SynthesisNumberField::Scale;
                *seed => "Texture Seed", 0., 1000., true => SynthesisNumberField::Seed;
            }
        }
        return;
    };
    let Some(layer) = texture.layers.get_mut(index) else {
        return;
    };
    let Layer {
        blend,
        color_controls,
        paint,
        ..
    } = Arc::make_mut(layer);
    match paint {
        Paint::Solid(paint) => color!(paint.color, SynthesisColorTarget::Solid(index)),
        Paint::Repeat(layer) => controls! {
            layer.frequency => "Frequency", 1., 100., true => SynthesisNumberField::RepeatFrequency;
            layer.feather => "Feather", 0., 100. => SynthesisNumberField::RepeatFeather;
            layer.strength => "Equalize strength", 0., 100. => SynthesisNumberField::EqualizeStrength;
        },
        Paint::Blend(layer) => controls! {
            layer.rectangle[0] => "Crop left", 0., 1.;
            layer.rectangle[1] => "Crop top", 0., 1.;
            layer.rectangle[2] => "Crop width", -1., 1.;
            layer.rectangle[3] => "Crop height", -1., 1.;
            layer.stretch[0] => "Stretch X", 0., 100. => SynthesisNumberField::PhotoStretchX;
            layer.stretch[1] => "Stretch Y", 0., 100. => SynthesisNumberField::PhotoStretchY;
            layer.edge_feather[0] => "Feather X", 0., 100. => SynthesisNumberField::PhotoFeatherX;
            layer.edge_feather[1] => "Feather Y", 0., 100. => SynthesisNumberField::PhotoFeatherY;
            layer.star_feather => "Star feather", 0., 100. => SynthesisNumberField::PhotoStarFeather;
            layer.repeat[1] => "Horizontal repeats", 1., 100., true, pair (SynthesisPairField::PhotoRepeat, 0);
            layer.repeat[0] => "Vertical repeats", 1., 100., true, pair (SynthesisPairField::PhotoRepeat, 1);
            layer.strength => "Equalize strength", 0., 100. => SynthesisNumberField::EqualizeStrength;
        },
        Paint::Patches(layer) => controls! {
            layer.automatic.frequency[0] => auto "Horizontal frequency", 1., 100., true, pair (SynthesisPairField::GridFrequency, 0);
            layer.automatic.frequency[1] => auto "Vertical frequency", 1., 100., true, pair (SynthesisPairField::GridFrequency, 1);
            layer.automatic.offset[0] => auto "Move X", 0., 100., pair (SynthesisPairField::Move, 0);
            layer.automatic.offset[1] => auto "Move Y", 0., 100., pair (SynthesisPairField::Move, 1);
            layer.automatic.density => auto "Density", 0., 100. => SynthesisNumberField::Density;
            layer.automatic.slide => auto "Slide", 0., 100. => SynthesisNumberField::Slide;
            layer.automatic.position_jitter[0] => auto "Position jitter X", 0., 100. => SynthesisNumberField::PositionJitterX;
            layer.automatic.position_jitter[1] => auto "Position jitter Y", 0., 100. => SynthesisNumberField::PositionJitterY;
            layer.automatic.scale => auto "Patch scale", -100., 1000. => SynthesisNumberField::PatchScale;
            layer.automatic.scale_jitter => auto "Patch scale jitter", 0., 100. => SynthesisNumberField::ScaleJitter;
            layer.automatic.angle => auto "Patch angle", -360., 360. => SynthesisNumberField::PatchAngle;
            layer.automatic.angle_jitter => auto "Patch angle jitter", 0., 100. => SynthesisNumberField::AngleJitter;
            layer.repeat[0] => "Horizontal repeats", 1., 100., true, pair (SynthesisPairField::ManualRepeat, 0);
            layer.repeat[1] => "Vertical repeats", 1., 100., true, pair (SynthesisPairField::ManualRepeat, 1);
            layer.strength => "Equalize strength", 0., 100. => SynthesisNumberField::EqualizeStrength;
        },
    }
    if !matches!(paint, Paint::Solid(_)) {
        let [
            brightness,
            contrast,
            hue,
            saturation,
            jitter_brightness,
            jitter_hue,
        ] = &mut color_controls.adjustments;
        controls! {
        *brightness => "Layer brightness", -100., 100. => SynthesisNumberField::LayerColor(SynthesisColorField::Brightness);
        *contrast => "Layer contrast", -100., 100. => SynthesisNumberField::LayerColor(SynthesisColorField::Contrast);
        *hue => "Layer hue", -360., 360. => SynthesisNumberField::LayerColor(SynthesisColorField::Hue);
        *saturation => "Layer saturation", -100., 1000. => SynthesisNumberField::LayerColor(SynthesisColorField::Saturation);
        color_controls.tint[1] => "Tint red", 0., 1., color SynthesisColorTarget::Layer(index);
        color_controls.tint[2] => "Tint green", 0., 1., color SynthesisColorTarget::Layer(index);
        color_controls.tint[3] => "Tint blue", 0., 1., color SynthesisColorTarget::Layer(index);
        }
        if !matches!(paint, Paint::Blend(_)) {
            controls! {
                *jitter_brightness => "Brightness jitter", 0., 100. => SynthesisNumberField::LayerColor(SynthesisColorField::BrightnessJitter);
                *jitter_hue => "Hue jitter", 0., 100. => SynthesisNumberField::LayerColor(SynthesisColorField::HueJitter);
            }
        }
    }
    controls! {
        blend.opacity => "Opacity", 0., 100. => SynthesisNumberField::BlendOpacity;
        blend.erosion => "Erode", 0., 100. => SynthesisNumberField::BlendErode;
        blend.erosion_sharpness => "Erode sharpness", 0., 100. => SynthesisNumberField::BlendSharpness;
    }
}

impl<E: 'static> PointwiseFilter<E> {
    pub fn synthesis_edit(&self) -> Option<SynthesisEdit> {
        Some(SynthesisEdit::new(self.synthesis_texture()?, false))
    }

    /// The sampled standalone graph Seed for a Manual placement editor task.
    pub fn synthesis_manual_preview_seed_at(&self, time: f64) -> Result<i32, PointwiseError<E>> {
        if self.kind() != crate::PointwiseKind::Synthesis {
            return Err(PointwiseError::InvalidCast);
        }
        Ok(self.parameter(1)?.effective_seed_at(time)? as i32)
    }

    /// Capture a standalone Texture with the owning node's current Scale and Seed.
    pub fn synthesis_preset_snapshot(&self) -> Result<SynthesisEdit, PointwiseError<E>> {
        let mut edit = self.synthesis_edit().ok_or(PointwiseError::InvalidCast)?;
        edit.value.master.scale = self.parameter(0)?.number();
        edit.value.master.seed = self.parameter(1)?.number() as i32;
        Ok(edit)
    }

    pub fn edit_synthesis(
        &self,
        edit: SynthesisEdit,
        label: &str,
    ) -> Result<bool, PointwiseError<E>> {
        let Some(edit) = self.prepare_synthesis_edit(edit)? else {
            return Ok(false);
        };
        let node = self.node();
        let slot = node.texture_node_logic().and_then(|logic| logic.slot());
        edit.publish(HistoryDirection::Redo)
            .map_err(|_| PointwiseError::InvalidOperation)?;
        if let Some((slot, history)) = slot.and_then(|slot| Some((slot.clone(), slot.history()?))) {
            let sequence = history.edit_sequence();
            let result = history.record_graph_edit(
                slot.history_target(),
                label.encode_utf16().collect(),
                Box::new(edit.clone()),
            );
            if let Err(error) = result {
                if history.edit_sequence() == sequence {
                    let _ = edit.publish(HistoryDirection::Undo);
                }
                return Err(PointwiseError::History(Box::new(error)));
            }
        }
        self.notify_synthesis_change()?;
        Ok(true)
    }

    pub(crate) fn prepare_synthesis_edit(
        &self,
        edit: SynthesisEdit,
    ) -> Result<Option<TextureEdit<E>>, PointwiseError<E>> {
        if edit.local_controls {
            return Err(PointwiseError::InvalidOperation);
        }
        if !edit.changed {
            return Ok(None);
        }
        let before = edit.before;
        let after = Arc::new(edit.value);
        let controls = edit
            .from_preset
            .then(|| {
                self.synthesis_preset_controls([after.master.scale, f64::from(after.master.seed)])
            })
            .transpose()?;
        let material = self.synthesis_catalog();
        let mut embedded = Vec::new();
        if let Some(material) = &material {
            let sponsor = self.node().sponsor;
            let replacement = super::SynthesisRecord::from_texture(after.clone());
            for core in sponsor.nodes() {
                let node = crate::pointwise::PointwiseNode {
                    id: core.id,
                    sponsor: sponsor.clone(),
                };
                if let Some(filter) = node.filter() {
                    crate::pointwise::EnvironmentPaintEdit::stage_synthesis_materials(
                        &mut embedded,
                        &filter,
                        |paint| {
                            Ok::<_, std::convert::Infallible>(
                                paint.with_material_record(material, &replacement),
                            )
                        },
                    )
                    .unwrap_or_else(|never| match never {});
                }
            }
        }
        Ok(Some(TextureEdit {
            owner: Arc::downgrade(&self.node().core()),
            before,
            after,
            controls,
            material,
            embedded: Arc::new(embedded),
        }))
    }
}

pub(crate) struct TextureEdit<E> {
    owner: Weak<crate::pointwise::graph::NodeCore<E>>,
    before: Arc<SynthesisTexture>,
    after: Arc<SynthesisTexture>,
    controls: Option<([f64; 2], [f64; 2])>,
    material: Option<super::SynthesisCatalogValue>,
    embedded: Arc<Vec<crate::pointwise::EnvironmentPaintEdit<E>>>,
}

impl<E> Clone for TextureEdit<E> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            before: self.before.clone(),
            after: self.after.clone(),
            controls: self.controls,
            material: self.material.clone(),
            embedded: self.embedded.clone(),
        }
    }
}

impl<E: 'static> TextureEdit<E> {
    pub(crate) fn records(&self) -> [super::SynthesisRecord; 2] {
        [self.before.clone(), self.after.clone()].map(super::SynthesisRecord::from_texture)
    }

    pub(crate) fn publish(
        &self,
        direction: HistoryDirection,
    ) -> Result<PointwiseFilter<E>, HistoryError<E>> {
        let owner = self.owner.upgrade().ok_or(HistoryError::BranchChanged)?;
        let sponsor = crate::pointwise::region::Sponsor::for_node(&owner)
            .ok_or(HistoryError::BranchChanged)?;
        let node = crate::pointwise::PointwiseNode {
            id: owner.id,
            sponsor,
        };
        let filter = node.filter().ok_or(HistoryError::BranchChanged)?;
        let matches_material = match (&self.material, filter.synthesis_catalog()) {
            (None, None) => true,
            (Some(expected), Some(current)) => expected.same_reference(&current),
            _ => false,
        };
        if !matches_material {
            return Err(HistoryError::InputChanged);
        }
        let (expected, restored, controls) = match direction {
            HistoryDirection::Undo => (
                &self.after,
                &self.before,
                self.controls.map(|(before, after)| (after, before)),
            ),
            HistoryDirection::Redo => (&self.before, &self.after, self.controls),
        };
        let undo = matches!(direction, HistoryDirection::Undo);
        let owners = crate::pointwise::EnvironmentPaintEdit::publish_all(&self.embedded, undo)?;
        if !filter.exchange_synthesis_texture(expected, restored.clone(), controls) {
            crate::pointwise::EnvironmentPaintEdit::rollback_batch(&self.embedded, &owners, undo);
            return Err(HistoryError::BranchChanged);
        }
        Ok(filter)
    }

    pub(crate) fn notify(filter: &PointwiseFilter<E>) -> Result<(), HistoryError<E>> {
        filter
            .notify_synthesis_change()
            .map_err(|error| HistoryError::Property(Box::new(error)))
    }
}

impl<E: 'static> crate::history::HistoryGraphEdit<E> for TextureEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        Self::notify(&self.publish(direction)?)
    }
}
