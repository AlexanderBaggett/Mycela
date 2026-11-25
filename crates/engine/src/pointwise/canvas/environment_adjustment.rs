use super::{
    CanvasDocument, CanvasLayerEditor, CanvasStyle, PointwiseFilter, StyleEdit,
    style::{EnvironmentPaint, Paint, SynthesisPaint},
};
use crate::pointwise::property::WeakPointwiseFilter;
use crate::{
    ContextChangeArgument, EnvironmentRecord, HistoryError, PointwiseKind, SynthesisCatalogValue,
    SynthesisRecord,
};
use std::collections::BTreeMap;
use std::sync::Arc;

pub(crate) struct EnvironmentPaintEdit<E> {
    owner: WeakPointwiseFilter<E>,
    owner_id: u64,
    values: PaintOwnerValues,
}

enum PaintOwnerValues {
    Style {
        before: Arc<CanvasStyle>,
        after: Arc<CanvasStyle>,
    },
    Canvas {
        before: Arc<CanvasDocument>,
        after: Arc<CanvasDocument>,
    },
}

type SynthesisLabLink = ([u8; 16], i32, Vec<u16>);

impl PaintOwnerValues {
    fn retained_synthesis_paints(&self, after: bool) -> Vec<SynthesisRecord> {
        let mut records = Vec::new();
        let mut visit = |record: &SynthesisRecord| records.push(record.clone());
        match self {
            Self::Style {
                before,
                after: next,
            } => {
                let style = if after { next } else { before };
                style.visit_retained_synthesis_paints(&mut visit);
            }
            Self::Canvas {
                before,
                after: next,
            } => {
                let canvas = if after { next } else { before };
                for layer in &canvas.layers {
                    layer.style.visit_retained_synthesis_paints(&mut visit);
                }
            }
        }
        records
    }

    fn visit_materials(&self, after: bool, mut visit: impl FnMut(&SynthesisCatalogValue)) {
        let mut visit_style = |style: &CanvasStyle| {
            for pass in &style.passes {
                if let Paint::Synthesis(paint) = &pass.paint
                    && let Some(catalog) = paint.catalog()
                {
                    visit(catalog);
                }
            }
        };
        match self {
            Self::Style {
                before,
                after: next,
            } => visit_style(if after { next } else { before }),
            Self::Canvas {
                before,
                after: next,
            } => {
                for layer in &(if after { next } else { before }).layers {
                    visit_style(&layer.style);
                }
            }
        }
    }
}

impl<E> EnvironmentPaintEdit<E> {
    pub(crate) fn collect_draft_preview(
        &self,
        preview: &mut crate::pointwise::property::capture::DraftRenderOverrides,
    ) {
        match &self.values {
            PaintOwnerValues::Style { after, .. } => {
                preview.styles.insert(self.owner_id, after.clone());
            }
            PaintOwnerValues::Canvas { after, .. } => {
                preview.canvases.insert(self.owner_id, after.clone());
            }
        }
    }
}

impl SynthesisRecord {
    /// Returns one item per removed active component. Number and Selection links
    /// share one identity and use the common scalar tag in the result.
    pub(crate) fn removed_lab_links_between(
        before: impl IntoIterator<Item = Self>,
        after: impl IntoIterator<Item = Self>,
    ) -> Vec<([u8; 16], i32, Vec<u16>)> {
        let mut before = synthesis_lab_link_counts(before);
        for (key, retained) in synthesis_lab_link_counts(after) {
            if let Some(count) = before.get_mut(&key) {
                *count = count.saturating_sub(retained);
            }
        }
        before
            .into_iter()
            .flat_map(|(link, count)| std::iter::repeat_n(link, count))
            .collect()
    }
}

fn synthesis_lab_link_counts(
    records: impl IntoIterator<Item = SynthesisRecord>,
) -> BTreeMap<SynthesisLabLink, usize> {
    let mut counts = BTreeMap::new();
    for record in records {
        record.visit_color_adjustments(|(scope, tag, name)| {
            *counts.entry((scope, tag, name.to_vec())).or_default() += 1;
        });
        record.visit_numeric_adjustments(|adjustment| {
            let tag = match adjustment.property_type {
                0 | 4 => 0,
                1 => 1,
                _ => return,
            };
            for component in adjustment
                .components()
                .iter()
                .filter(|component| component.active && !component.name.is_empty())
            {
                *counts
                    .entry((adjustment.guid, tag, component.name.clone()))
                    .or_default() += 1;
            }
        });
    }
    counts
}

impl<E: 'static> CanvasLayerEditor<E> {
    pub(crate) fn belongs_to(&self, owner: &PointwiseFilter<E>) -> bool {
        self.owner.same_reference(owner)
    }
}

impl<E: 'static> PointwiseFilter<E> {
    pub(crate) fn environment_paint_adjuster_count(&self, key: ([u8; 16], &[u16])) -> usize {
        let mut count = 0;
        self.visit_environment_paint_adjusters(|other| count += usize::from(other == key));
        count
    }

    pub(crate) fn visit_environment_paint_adjusters(
        &self,
        mut visit: impl FnMut(([u8; 16], &[u16])),
    ) {
        self.visit_environment_paints(|_, key| {
            if let Some(key) = key {
                visit(key);
            }
        });
    }

    pub(crate) fn visit_environment_paints(
        &self,
        mut visit: impl FnMut(&EnvironmentRecord, Option<([u8; 16], &[u16])>),
    ) {
        self.visit_style_paints(|paint| {
            if let Paint::Environment(paint) = paint {
                visit(&paint.map, paint.adjuster_key());
            }
        });
    }

    pub(crate) fn visit_synthesis_paints(&self, mut visit: impl FnMut(&SynthesisRecord)) {
        self.visit_style_paints(|paint| {
            if let Paint::Synthesis(paint) = paint
                && paint.catalog().is_none()
            {
                visit(&paint.record());
            }
        });
    }

    pub(crate) fn visit_synthesis_material_paints(&self, mut visit: impl FnMut(&SynthesisPaint)) {
        self.visit_style_paints(|paint| {
            if let Paint::Synthesis(paint) = paint
                && paint.key().is_some()
            {
                visit(paint);
            }
        });
    }

    pub(crate) fn uses_synthesis_material(&self, value: &SynthesisCatalogValue) -> bool {
        if self
            .synthesis_catalog()
            .is_some_and(|catalog| catalog.same_reference(value))
        {
            return true;
        }
        let mut found = false;
        self.visit_synthesis_material_paints(|paint| {
            found |= paint
                .catalog()
                .is_some_and(|catalog| catalog.same_reference(value));
        });
        found
    }

    pub(crate) fn visit_parked_synthesis_paints(&self, mut visit: impl FnMut(&SynthesisRecord)) {
        self.visit_paint_styles(|style| {
            for pass in &style.passes {
                for paint in pass.parked_paints() {
                    if let Paint::Synthesis(paint) = paint {
                        visit(&paint.record());
                    }
                }
            }
        });
    }

    /// Visits both current and temporarily parked Synthesis effects in each pass.
    pub(crate) fn visit_retained_synthesis_paints(&self, mut visit: impl FnMut(&SynthesisRecord)) {
        self.visit_retained_synthesis_paints_with_inputs(|record, _| visit(record));
    }

    pub(crate) fn visit_retained_synthesis_paints_with_inputs(
        &self,
        mut visit: impl FnMut(&SynthesisRecord, Option<i32>),
    ) {
        self.visit_paint_styles_with_inputs(|style, input| {
            style.visit_retained_synthesis_paints(&mut |record| visit(record, input));
        });
    }

    fn visit_style_paints(&self, mut visit: impl FnMut(&Paint)) {
        self.visit_paint_styles(|style| {
            for pass in &style.passes {
                visit(&pass.paint);
            }
        });
    }

    fn visit_paint_styles(&self, mut visit_style: impl FnMut(&CanvasStyle)) {
        self.visit_paint_styles_with_inputs(|style, _| visit_style(style));
    }

    fn visit_paint_styles_with_inputs(
        &self,
        mut visit_style: impl FnMut(&CanvasStyle, Option<i32>),
    ) {
        match self.kind() {
            PointwiseKind::Style => {
                if let Some(style) = self.style() {
                    visit_style(&style, None);
                }
            }
            PointwiseKind::Canvas => {
                if let Some(canvas) = self.canvas_document() {
                    for layer in &canvas.layers {
                        visit_style(&layer.style, Some(layer.input));
                    }
                }
            }
            _ => {}
        }
    }
}

impl<E: 'static> EnvironmentPaintEdit<E> {
    pub(crate) fn from_style_edit(
        owner: &PointwiseFilter<E>,
        layer: Option<&CanvasLayerEditor<E>>,
        edit: StyleEdit,
    ) -> Option<Self> {
        let (before, after) = edit.into_replacement()?;
        let values = if let Some(layer) = layer {
            if !layer.owner.same_reference(owner) {
                return None;
            }
            let canvas = owner.canvas_document()?;
            let index = layer.index_in(&canvas)?;
            if !Arc::ptr_eq(&canvas.layers[index].style, &before) {
                return None;
            }
            let mut replacement = (*canvas).clone();
            replacement.layers[index].style = after;
            PaintOwnerValues::Canvas {
                before: canvas,
                after: Arc::new(replacement),
            }
        } else {
            if !Arc::ptr_eq(&owner.style()?, &before) {
                return None;
            }
            PaintOwnerValues::Style { before, after }
        };
        Some(Self {
            owner: owner.downgrade(),
            owner_id: owner.node().occurrence_id(),
            values,
        })
    }

    pub(crate) fn from_canvas_layer_removal(
        owner: &PointwiseFilter<E>,
        layer: &CanvasLayerEditor<E>,
    ) -> Option<Self> {
        if !layer.belongs_to(owner) {
            return None;
        }
        let (before, index) = layer.current_layer().ok()?;
        let mut after = (*before).clone();
        after.layers.remove(index);
        Some(Self {
            owner: owner.downgrade(),
            owner_id: owner.node().occurrence_id(),
            values: PaintOwnerValues::Canvas {
                before,
                after: Arc::new(after),
            },
        })
    }

    /// Returns one item per removed receiver, using the shared scalar tag for
    /// Number and Selection links. Parked effects remain part of the owner.
    pub(crate) fn removed_synthesis_lab_links(
        &self,
        retired_materials: impl IntoIterator<Item = SynthesisRecord>,
    ) -> Vec<([u8; 16], i32, Vec<u16>)> {
        SynthesisRecord::removed_lab_links_between(
            self.values
                .retained_synthesis_paints(false)
                .into_iter()
                .chain(retired_materials),
            self.values.retained_synthesis_paints(true),
        )
    }

    pub(crate) fn removed_synthesis_materials(&self) -> Vec<(SynthesisCatalogValue, usize)> {
        let mut previous = BTreeMap::new();
        self.values.visit_materials(false, |value| {
            previous
                .entry(value.lock_order())
                .or_insert_with(|| (value.clone(), 0_usize))
                .1 += 1;
        });
        self.values.visit_materials(true, |value| {
            if let Some((_, count)) = previous.get_mut(&value.lock_order()) {
                *count = count.saturating_sub(1);
            }
        });
        previous
            .into_values()
            .filter(|(_, count)| *count > 0)
            .collect()
    }

    pub(crate) fn stage<F>(
        owner: &PointwiseFilter<E>,
        mut map: impl FnMut(
            &Arc<EnvironmentRecord>,
            Option<([u8; 16], &[u16])>,
        ) -> Result<Option<Arc<EnvironmentRecord>>, F>,
    ) -> Result<Option<Self>, F> {
        Self::stage_paints(owner, false, |paint, _| {
            map_environment_record(paint, &mut map)
        })
    }

    pub(crate) fn rename(
        owner: &PointwiseFilter<E>,
        key: ([u8; 16], &[u16]),
        name: &[u16],
        replacement: Option<&Arc<EnvironmentRecord>>,
    ) -> Option<Self> {
        Self::stage_with(owner, |paint| {
            if paint.adjuster_key() != Some(key) {
                return Ok(None);
            }
            let mut paint = paint.clone();
            let adjustment = Arc::make_mut(paint.adjustment.as_mut().expect("matched Lab link"));
            adjustment.components_mut()[0].name = name.to_vec();
            if let Some(map) = replacement {
                paint.map = map.clone();
            }
            Ok(Some(paint))
        })
        .unwrap_or_else(|never: std::convert::Infallible| match never {})
    }

    pub(crate) fn relocate<F>(
        owner: &PointwiseFilter<E>,
        scope: Option<[u8; 16]>,
        mut resolve: impl FnMut(
            Option<([u8; 16], &[u16])>,
            &Arc<EnvironmentRecord>,
            Option<[u8; 16]>,
        ) -> Result<Option<Arc<EnvironmentRecord>>, F>,
    ) -> Result<Option<Self>, F> {
        Self::stage_with(owner, |paint| {
            let key = paint.adjuster_key();
            let map = resolve(key, &paint.map, scope)?;
            let new_group = scope.filter(|scope| key.is_some_and(|(current, _)| current != *scope));
            if new_group.is_none() && map.as_ref().is_none_or(|map| Arc::ptr_eq(map, &paint.map)) {
                return Ok(None);
            }
            let mut paint = paint.clone();
            if let Some(scope) = new_group {
                Arc::make_mut(paint.adjustment.as_mut().expect("matched Lab link")).guid = scope;
            }
            if let Some(map) = map {
                paint.map = map;
            }
            Ok(Some(paint))
        })
    }

    fn stage_with<F>(
        owner: &PointwiseFilter<E>,
        mut map: impl FnMut(&EnvironmentPaint) -> Result<Option<EnvironmentPaint>, F>,
    ) -> Result<Option<Self>, F> {
        Self::stage_paints(owner, false, |paint, _| match paint {
            Paint::Environment(paint) => {
                map(paint).map(|paint| paint.map(|paint| Paint::Environment(Box::new(paint))))
            }
            _ => Ok(None),
        })
    }

    /// Apply a Synthesis change to the same pending owner revision as the
    /// Environment changes. Publishing two replacements for one owner would
    /// make one of them stale and would expose an intermediate revision.
    pub(crate) fn stage_synthesis<F>(
        edits: &mut Vec<Self>,
        owner: &PointwiseFilter<E>,
        mut transform: impl FnMut(&SynthesisRecord) -> Result<Option<SynthesisRecord>, F>,
    ) -> Result<(), F> {
        Self::stage_synthesis_in_inputs(edits, owner, |record, _| transform(record))
    }

    pub(crate) fn stage_synthesis_in_inputs<F>(
        edits: &mut Vec<Self>,
        owner: &PointwiseFilter<E>,
        mut transform: impl FnMut(&SynthesisRecord, Option<i32>) -> Result<Option<SynthesisRecord>, F>,
    ) -> Result<(), F> {
        Self::stage_synthesis_with(edits, owner, true, |paint, input| {
            if paint.catalog().is_some() {
                return Ok(None);
            }
            Ok(transform(&paint.record(), input)?
                .filter(|replacement| !Arc::ptr_eq(&paint.local, replacement.texture()))
                .map(|replacement| SynthesisPaint {
                    local: replacement.texture().clone(),
                    ..paint.clone()
                }))
        })
    }

    pub(crate) fn stage_synthesis_materials<F>(
        edits: &mut Vec<Self>,
        owner: &PointwiseFilter<E>,
        mut transform: impl FnMut(&SynthesisPaint) -> Result<Option<SynthesisPaint>, F>,
    ) -> Result<(), F> {
        Self::stage_synthesis_with(edits, owner, false, |paint, _| transform(paint))
    }

    fn stage_synthesis_with<F>(
        edits: &mut Vec<Self>,
        owner: &PointwiseFilter<E>,
        include_parked: bool,
        mut transform: impl FnMut(&SynthesisPaint, Option<i32>) -> Result<Option<SynthesisPaint>, F>,
    ) -> Result<(), F> {
        let mut map = |paint: &Paint, input| {
            let Paint::Synthesis(paint) = paint else {
                return Ok(None);
            };
            Ok(transform(paint, input)?.map(Paint::Synthesis))
        };
        let owner_id = owner.node().occurrence_id();
        if let Some(edit) = edits.iter_mut().find(|edit| edit.owner_id == owner_id) {
            match &mut edit.values {
                PaintOwnerValues::Style { after, .. } => {
                    if let Some(style) =
                        after.map_paints(include_parked, &mut |paint| map(paint, None))?
                    {
                        *after = style;
                    }
                }
                PaintOwnerValues::Canvas { after, .. } => {
                    if let Some(canvas) = after
                        .map_layer_paints(include_parked, &mut |paint, input| {
                            map(paint, Some(input))
                        })?
                    {
                        *after = canvas;
                    }
                }
            }
        } else {
            edits.extend(Self::stage_paints(owner, include_parked, map)?);
        }
        Ok(())
    }

    fn stage_paints<F>(
        owner: &PointwiseFilter<E>,
        include_parked_synthesis: bool,
        mut map: impl FnMut(&Paint, Option<i32>) -> Result<Option<Paint>, F>,
    ) -> Result<Option<Self>, F> {
        let values = match owner.kind() {
            PointwiseKind::Style => {
                let Some(before) = owner.style() else {
                    return Ok(None);
                };
                let Some(after) =
                    before.map_paints(include_parked_synthesis, &mut |paint| map(paint, None))?
                else {
                    return Ok(None);
                };
                PaintOwnerValues::Style { before, after }
            }
            PointwiseKind::Canvas => {
                let Some(before) = owner.canvas_document() else {
                    return Ok(None);
                };
                let Some(after) = before
                    .map_layer_paints(include_parked_synthesis, &mut |paint, input| {
                        map(paint, Some(input))
                    })?
                else {
                    return Ok(None);
                };
                PaintOwnerValues::Canvas { before, after }
            }
            _ => return Ok(None),
        };
        Ok(Some(Self {
            owner: owner.downgrade(),
            owner_id: owner.node().occurrence_id(),
            values,
        }))
    }

    pub(crate) fn publish_all(
        edits: &[Self],
        undo: bool,
    ) -> Result<Vec<PointwiseFilter<E>>, HistoryError<E>> {
        let owners = edits
            .iter()
            .map(|edit| edit.owner.upgrade().ok_or(HistoryError::NullTarget))
            .collect::<Result<Vec<_>, _>>()?;
        for (index, (edit, owner)) in edits.iter().zip(&owners).enumerate() {
            if !edit.exchange(owner, undo) {
                Self::rollback_batch(&edits[..index], &owners[..index], undo);
                return Err(HistoryError::InputChanged);
            }
        }
        Ok(owners)
    }

    pub(crate) fn rollback_batch(edits: &[Self], owners: &[PointwiseFilter<E>], undo: bool) {
        for (edit, owner) in edits.iter().zip(owners).rev() {
            edit.exchange(owner, !undo);
        }
    }

    pub(crate) fn notify(owner: &PointwiseFilter<E>) -> Result<(), HistoryError<E>> {
        owner
            .node()
            .notify_property_change_with(ContextChangeArgument::shared_empty())
            .map_err(|error| HistoryError::Property(Box::new(error)))
    }

    fn exchange(&self, owner: &PointwiseFilter<E>, undo: bool) -> bool {
        match &self.values {
            PaintOwnerValues::Style { before, after } => {
                let (expected, replacement) = if undo {
                    (after, before)
                } else {
                    (before, after)
                };
                owner.exchange_style(expected, replacement.clone())
            }
            PaintOwnerValues::Canvas { before, after } => {
                let (expected, replacement) = if undo {
                    (after, before)
                } else {
                    (before, after)
                };
                owner.exchange_canvas_document(expected, replacement.clone())
            }
        }
    }
}

impl CanvasDocument {
    pub(in crate::pointwise) fn map_environments<F>(
        &self,
        map: &mut impl FnMut(
            &Arc<EnvironmentRecord>,
            Option<([u8; 16], &[u16])>,
        ) -> Result<Option<Arc<EnvironmentRecord>>, F>,
    ) -> Result<Option<Arc<Self>>, F> {
        self.map_paints(false, &mut |paint| map_environment_record(paint, map))
    }

    fn map_paints<F>(
        &self,
        include_parked_synthesis: bool,
        map: &mut impl FnMut(&Paint) -> Result<Option<Paint>, F>,
    ) -> Result<Option<Arc<Self>>, F> {
        self.map_layer_paints(include_parked_synthesis, &mut |paint, _| map(paint))
    }

    fn map_layer_paints<F>(
        &self,
        include_parked_synthesis: bool,
        map: &mut impl FnMut(&Paint, i32) -> Result<Option<Paint>, F>,
    ) -> Result<Option<Arc<Self>>, F> {
        let mut replacement = None;
        for (index, layer) in self.layers.iter().enumerate() {
            if let Some(style) = layer
                .style
                .map_paints(include_parked_synthesis, &mut |paint| {
                    map(paint, layer.input)
                })?
            {
                replacement.get_or_insert_with(|| self.clone()).layers[index].style = style;
            }
        }
        Ok(replacement.map(Arc::new))
    }
}

impl CanvasStyle {
    fn visit_retained_synthesis_paints(&self, visit: &mut impl FnMut(&SynthesisRecord)) {
        for pass in &self.passes {
            for paint in std::iter::once(&pass.paint).chain(pass.parked_paints()) {
                if let Paint::Synthesis(paint) = paint
                    && paint.catalog().is_none()
                {
                    visit(&paint.record());
                }
            }
        }
    }

    pub(in crate::pointwise) fn map_environments<F>(
        &self,
        map: &mut impl FnMut(
            &Arc<EnvironmentRecord>,
            Option<([u8; 16], &[u16])>,
        ) -> Result<Option<Arc<EnvironmentRecord>>, F>,
    ) -> Result<Option<Arc<Self>>, F> {
        self.map_paints(false, &mut |paint| map_environment_record(paint, map))
    }

    fn map_paints<F>(
        &self,
        include_parked_synthesis: bool,
        map: &mut impl FnMut(&Paint) -> Result<Option<Paint>, F>,
    ) -> Result<Option<Arc<Self>>, F> {
        let mut replacement = None;
        for (index, pass) in self.passes.iter().enumerate() {
            if let Some(paint) = map(&pass.paint)? {
                let style = replacement.get_or_insert_with(|| self.clone());
                style.passes[index].paint = paint;
            }
            if include_parked_synthesis {
                for (slot, paint) in pass.parked_paints().iter().enumerate() {
                    if matches!(paint, Paint::Synthesis(_))
                        && let Some(paint) = map(paint)?
                    {
                        let style = replacement.get_or_insert_with(|| self.clone());
                        style.passes[index].parked_paints_mut()[slot] = paint;
                    }
                }
            }
        }
        Ok(replacement.map(Arc::new))
    }
}

fn map_environment_record<F>(
    paint: &Paint,
    map: &mut impl FnMut(
        &Arc<EnvironmentRecord>,
        Option<([u8; 16], &[u16])>,
    ) -> Result<Option<Arc<EnvironmentRecord>>, F>,
) -> Result<Option<Paint>, F> {
    let Paint::Environment(paint) = paint else {
        return Ok(None);
    };
    Ok(map(&paint.map, paint.adjuster_key())?.map(|map| {
        Paint::Environment(Box::new(EnvironmentPaint {
            map,
            ..paint.as_ref().clone()
        }))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointwise::StylePaintKind;
    use crate::{AdjustmentRecord, SynthesisEdit, SynthesisLayerKind, SynthesisNumberField};

    fn scalar_link(tag: i32, name: &[u16]) -> AdjustmentRecord {
        let mut record = AdjustmentRecord::new(&[[0., 100.]]);
        record.property_type = tag;
        record.guid = [3; 16];
        let component = &mut record.components_mut()[0];
        component.name = name.to_vec();
        component.active = true;
        record
    }

    #[test]
    fn parked_synthesis_receivers_survive_effect_changes_and_receive_lab_updates() {
        use SynthesisNumberField::{PatchFrequency, Seed};

        let owner = PointwiseFilter::<()>::new(PointwiseKind::Style).unwrap();
        let name: Vec<_> = "Weight".encode_utf16().collect();
        let mut style = owner.style_edit().unwrap();
        style.set_paint(0, StylePaintKind::Synthesis).unwrap();
        let mut synthesis = style.synthesis_edit(0).unwrap();
        synthesis.add_layer(SynthesisLayerKind::Patches);
        assert_eq!(
            synthesis.set_number_adjustment(None, Seed, Some((scalar_link(0, &name), 40.))),
            Some(true)
        );
        assert_eq!(
            synthesis.set_number_adjustment(
                Some(0),
                PatchFrequency(0),
                Some((scalar_link(4, &name), 40.))
            ),
            Some(true)
        );
        style.set_synthesis(0, synthesis).unwrap();
        owner.edit_style(style, "Prepare linked paint").unwrap();

        let mut style = owner.style_edit().unwrap();
        style.set_paint(0, StylePaintKind::Solid).unwrap();
        let park = EnvironmentPaintEdit::from_style_edit(&owner, None, style).unwrap();
        assert!(park.removed_synthesis_lab_links([]).is_empty());
        EnvironmentPaintEdit::publish_all(&[park], false).unwrap();
        owner.visit_synthesis_paints(|_| panic!("Synthesis effect is parked"));
        let mut parked_count = 0;
        owner.visit_retained_synthesis_paints(|_| parked_count += 1);
        assert_eq!(parked_count, 1);

        let renamed: Vec<_> = "Placement".encode_utf16().collect();
        let mut edits = Vec::new();
        EnvironmentPaintEdit::stage_synthesis(&mut edits, &owner, |record| {
            record
                .rename_numeric_adjustments::<()>([3; 16], 0, &name, &renamed, |_, _| Ok(Some(61.)))
        })
        .unwrap();
        assert_eq!(edits.len(), 1);
        EnvironmentPaintEdit::publish_all(&edits, false).unwrap();

        let mut style = owner.style_edit().unwrap();
        style.set_paint(0, StylePaintKind::Synthesis).unwrap();
        let restore = EnvironmentPaintEdit::from_style_edit(&owner, None, style).unwrap();
        assert!(restore.removed_synthesis_lab_links([]).is_empty());
        EnvironmentPaintEdit::publish_all(&[restore], false).unwrap();
        let mut style = owner.style_edit().unwrap();
        let mut synthesis = style.synthesis_edit(0).unwrap();
        assert_eq!(synthesis.number_value(None, Seed), Some(61.));
        let selection = synthesis
            .number_adjustment(Some(0), PatchFrequency(0))
            .unwrap();
        assert_eq!(selection.property_type, 4);
        assert_eq!(selection.components()[0].name, renamed);
        assert_eq!(
            synthesis.number_value(Some(0), PatchFrequency(0)),
            Some(61.)
        );

        assert!(synthesis.remove_patch(0, 0));
        style.set_synthesis(0, synthesis).unwrap();
        let remove_patch = EnvironmentPaintEdit::from_style_edit(&owner, None, style).unwrap();
        assert_eq!(
            remove_patch.removed_synthesis_lab_links([]),
            [([3; 16], 0, renamed.clone())]
        );

        let mut style = owner.style_edit().unwrap();
        assert!(style.remove(0));
        let remove_pass = EnvironmentPaintEdit::from_style_edit(&owner, None, style).unwrap();
        assert_eq!(
            remove_pass.removed_synthesis_lab_links([]),
            [([3; 16], 0, renamed.clone()), ([3; 16], 0, renamed)]
        );
    }

    #[test]
    fn canvas_layer_removal_reports_parked_receivers_and_undo_restores_the_same_layer() {
        let owner = PointwiseFilter::<()>::new(PointwiseKind::Canvas).unwrap();
        let foreign = PointwiseFilter::new(PointwiseKind::Canvas).unwrap();
        let retained = owner.edit_add_canvas_layer().unwrap();
        let removed = owner.edit_add_canvas_layer().unwrap();
        let name: Vec<_> = "Seed".encode_utf16().collect();
        let mut style = removed.style_edit().unwrap();
        style.set_paint(0, StylePaintKind::Synthesis).unwrap();
        let mut synthesis = style.synthesis_edit(0).unwrap();
        assert_eq!(
            synthesis.set_number_adjustment(
                None,
                SynthesisNumberField::Seed,
                Some((scalar_link(0, &name), 40.)),
            ),
            Some(true)
        );
        style.set_synthesis(0, synthesis).unwrap();
        style.set_paint(0, StylePaintKind::Solid).unwrap();
        removed.edit_style(style, "Park linked paint").unwrap();

        assert!(EnvironmentPaintEdit::from_canvas_layer_removal(&foreign, &removed).is_none());
        let edit = EnvironmentPaintEdit::from_canvas_layer_removal(&owner, &removed).unwrap();
        assert_eq!(edit.removed_synthesis_lab_links([]), [([3; 16], 0, name)]);
        let edits = [edit];
        EnvironmentPaintEdit::publish_all(&edits, false).unwrap();
        assert!(EnvironmentPaintEdit::from_canvas_layer_removal(&owner, &removed).is_none());
        assert!(owner.canvas_layer_editors()[0].same_reference(&retained));
        EnvironmentPaintEdit::publish_all(&edits, true).unwrap();
        let layers = owner.canvas_layer_editors();
        assert!(layers[0].same_reference(&removed));
        assert!(layers[1].same_reference(&retained));
        let mut restored = 0;
        owner.visit_retained_synthesis_paints(|_| restored += 1);
        assert_eq!(restored, 1);
    }

    fn add_paints(edit: &mut StyleEdit) {
        edit.add(StylePaintKind::Environment).unwrap();
        edit.add(StylePaintKind::Synthesis).unwrap();
    }

    fn add_synthesis_layer(record: &SynthesisRecord) -> Result<Option<SynthesisRecord>, ()> {
        let mut edit = SynthesisEdit::new(record.texture().clone(), true);
        edit.add_layer(SynthesisLayerKind::Solid);
        Ok(edit
            .into_local_replacement(record.texture())
            .unwrap()
            .map(SynthesisRecord::from_texture))
    }

    #[test]
    fn synthesis_and_environment_publish_one_revision_per_paint_owner() {
        for kind in [PointwiseKind::Style, PointwiseKind::Canvas] {
            let owner = PointwiseFilter::<()>::new(kind).unwrap();
            let other_style = if kind == PointwiseKind::Style {
                let mut edit = owner.style_edit().unwrap();
                add_paints(&mut edit);
                owner.edit_style(edit, "Prepare paints").unwrap();
                None
            } else {
                owner.edit_add_canvas_layer().unwrap();
                let layer = owner.edit_add_canvas_layer().unwrap();
                let mut edit = layer.style_edit().unwrap();
                add_paints(&mut edit);
                layer.edit_style(edit, "Prepare paints").unwrap();
                Some(owner.canvas_document().unwrap().layers[1].style.clone())
            };
            let environment = Arc::new(EnvironmentRecord::default());
            let mut edits =
                EnvironmentPaintEdit::stage(&owner, |_, _| Ok::<_, ()>(Some(environment.clone())))
                    .unwrap()
                    .into_iter()
                    .collect::<Vec<_>>();
            EnvironmentPaintEdit::stage_synthesis(&mut edits, &owner, add_synthesis_layer).unwrap();
            assert_eq!(edits.len(), 1);

            let assert_paints = |edited| {
                let mut visits = 0;
                owner.visit_synthesis_paints(|texture| {
                    visits += 1;
                    assert_eq!(texture.layer_count(), usize::from(edited));
                });
                assert_eq!(visits, 1);
                owner.visit_environment_paints(|map, _| {
                    assert_eq!(std::ptr::eq(map, environment.as_ref()), edited);
                });
                if let Some(other_style) = &other_style {
                    let canvas = owner.canvas_document().unwrap();
                    assert!(Arc::ptr_eq(other_style, &canvas.layers[1].style));
                }
            };

            assert_paints(false);
            for undo in [false, true, false] {
                let affected = EnvironmentPaintEdit::publish_all(&edits, undo).unwrap();
                assert_eq!(affected.len(), 1);
                assert!(affected[0].same_reference(&owner));
                assert_paints(!undo);
            }
        }
    }

    #[test]
    fn synthesis_only_paint_batch_rolls_back_after_a_stale_owner() {
        let style = || {
            let owner = PointwiseFilter::<()>::new(PointwiseKind::Style).unwrap();
            let mut edit = owner.style_edit().unwrap();
            edit.add(StylePaintKind::Synthesis).unwrap();
            owner.edit_style(edit, "Prepare paint").unwrap();
            owner
        };
        let first = style();
        let second = style();
        let original = first.style().unwrap();
        let mut edits = Vec::new();
        for owner in [&first, &second] {
            EnvironmentPaintEdit::stage_synthesis(&mut edits, owner, add_synthesis_layer).unwrap();
        }
        assert_eq!(edits.len(), 2);
        let mut edit = second.style_edit().unwrap();
        edit.add(StylePaintKind::Solid).unwrap();
        second.edit_style(edit, "Change second style").unwrap();
        let changed = second.style().unwrap();

        assert!(matches!(
            EnvironmentPaintEdit::publish_all(&edits, false),
            Err(HistoryError::InputChanged)
        ));
        assert!(Arc::ptr_eq(&first.style().unwrap(), &original));
        assert!(Arc::ptr_eq(&second.style().unwrap(), &changed));
    }
}
