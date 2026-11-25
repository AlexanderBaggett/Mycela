use super::super::photo_blend::PhotoBlend;
use super::{AdjustmentRecord, Arc, Automatic, NumericLink, Paint, Patches, SynthesisTexture};

/// A two-axis Synthesis property with one retained GTX adjustment record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SynthesisPairField {
    GridFrequency,
    Move,
    PhotoRepeat,
    ManualRepeat,
}

impl SynthesisPairField {
    pub const AUTOMATIC: [Self; 2] = [Self::GridFrequency, Self::Move];
    pub const ALL: [Self; 4] = [
        Self::GridFrequency,
        Self::Move,
        Self::PhotoRepeat,
        Self::ManualRepeat,
    ];

    pub const fn range(self) -> [f64; 2] {
        match self {
            Self::GridFrequency | Self::ManualRepeat => [1., 100.],
            Self::Move | Self::PhotoRepeat => [0., 100.],
        }
    }

    /// Returns the saved component for an inspector axis. Use 0 for X and 1 for Y.
    pub const fn component_index(self, inspector_axis: usize) -> usize {
        match self {
            Self::PhotoRepeat => inspector_axis ^ 1,
            _ => inspector_axis,
        }
    }

    pub(crate) fn swap_axes_if_needed<T: Copy>(self, axes: [T; 2]) -> [T; 2] {
        [axes[self.component_index(0)], axes[self.component_index(1)]]
    }

    pub(in crate::pointwise::canvas::synthesis) fn normalize(self, value: f64) -> Option<f64> {
        if !value.is_finite() {
            return None;
        }
        let [low, high] = self.range();
        match self {
            Self::GridFrequency | Self::PhotoRepeat => {
                let rounded = crate::round_decimal_places(value, 1).ok()?;
                let integer = f64::from(crate::framework_cast::integer(rounded));
                (low..=high).contains(&integer).then_some(integer)
            }
            Self::ManualRepeat => {
                let rounded = value.round_ties_even();
                (low..=high).contains(&rounded).then_some(rounded)
            }
            Self::Move => {
                let typed = f64::from(value as f32);
                if !(low..=high).contains(&typed) {
                    return None;
                }
                let rounded = crate::round_decimal_places(typed, 1).ok()?;
                Some(f64::from(rounded as f32))
            }
        }
    }
}

pub(super) fn active_component(record: &AdjustmentRecord, axis: usize) -> Option<&[u16]> {
    record
        .components()
        .get(axis)
        .filter(|component| component.active && !component.name.is_empty())
        .map(|component| component.name.as_slice())
}

impl NumericLink {
    fn effective_pair(&self, raw: [f64; 2]) -> [f64; 2] {
        std::array::from_fn(|axis| self.resolved[axis].unwrap_or(raw[axis]))
    }
}

impl Automatic {
    pub(super) fn pair(&self, field: SynthesisPairField) -> ([f64; 2], Option<&NumericLink>) {
        match field {
            SynthesisPairField::GridFrequency => (
                self.frequency.map(f64::from),
                self.frequency_adjustment.as_ref(),
            ),
            SynthesisPairField::Move => {
                (self.offset.map(f64::from), self.offset_adjustment.as_ref())
            }
            SynthesisPairField::PhotoRepeat | SynthesisPairField::ManualRepeat => {
                unreachable!("not an automatic placement property")
            }
        }
    }

    pub(super) fn pair_link_mut(&mut self, field: SynthesisPairField) -> &mut Option<NumericLink> {
        match field {
            SynthesisPairField::GridFrequency => &mut self.frequency_adjustment,
            SynthesisPairField::Move => &mut self.offset_adjustment,
            SynthesisPairField::PhotoRepeat | SynthesisPairField::ManualRepeat => {
                unreachable!("not an automatic placement property")
            }
        }
    }

    pub(in crate::pointwise::canvas::synthesis) fn effective_frequency(&self) -> [i32; 2] {
        self.effective_pair(SynthesisPairField::GridFrequency)
            .map(|value| value as i32)
    }

    pub(in crate::pointwise::canvas::synthesis) fn effective_offset(&self) -> [f32; 2] {
        self.effective_pair(SynthesisPairField::Move)
            .map(|value| value as f32)
    }

    fn effective_pair(&self, field: SynthesisPairField) -> [f64; 2] {
        let (raw, link) = self.pair(field);
        link.map_or(raw, |link| link.effective_pair(raw))
    }
}

impl PhotoBlend {
    pub(in crate::pointwise::canvas::synthesis) fn effective_repeat(&self) -> [i32; 2] {
        let raw = self.repeat.map(f64::from);
        self.repeat_adjustment
            .as_ref()
            .map_or(raw, |link| link.effective_pair(raw))
            .map(|value| value as i32)
    }
}

impl Patches {
    pub(in crate::pointwise::canvas::synthesis) fn effective_repeat(&self) -> [i32; 2] {
        let raw = self.repeat.map(f64::from);
        self.repeat_adjustment
            .as_ref()
            .map_or(raw, |link| link.effective_pair(raw))
            .map(|value| value as i32)
    }
}

impl Paint {
    pub(super) fn pair(
        &self,
        field: SynthesisPairField,
    ) -> Option<([f64; 2], Option<&NumericLink>)> {
        match (self, field) {
            (Self::Blend(blend), SynthesisPairField::PhotoRepeat) => Some((
                blend.repeat.map(f64::from),
                blend.repeat_adjustment.as_ref(),
            )),
            (
                Self::Patches(patches),
                SynthesisPairField::GridFrequency | SynthesisPairField::Move,
            ) => Some(patches.automatic.pair(field)),
            (Self::Patches(patches), SynthesisPairField::ManualRepeat) => Some((
                patches.repeat.map(f64::from),
                patches.repeat_adjustment.as_ref(),
            )),
            _ => None,
        }
    }
}

impl SynthesisTexture {
    pub(in crate::pointwise::canvas::synthesis) fn pair_value(
        &self,
        layer: usize,
        field: SynthesisPairField,
    ) -> Option<[f64; 2]> {
        let (raw, link) = self.layers.get(layer)?.paint.pair(field)?;
        let effective = link.map_or(raw, |link| link.effective_pair(raw));
        Some(field.swap_axes_if_needed(effective))
    }

    pub(in crate::pointwise::canvas::synthesis) fn pair_adjustment(
        &self,
        layer: usize,
        field: SynthesisPairField,
    ) -> Option<&AdjustmentRecord> {
        self.layers
            .get(layer)?
            .paint
            .pair(field)?
            .1
            .map(NumericLink::record)
    }

    pub(in crate::pointwise::canvas::synthesis) fn set_pair_adjustment(
        &mut self,
        layer: usize,
        field: SynthesisPairField,
        link: Option<(AdjustmentRecord, [Option<f64>; 2])>,
    ) -> Option<bool> {
        let (raw, current) = self.layers.get(layer)?.paint.pair(field)?;
        let mut authored = raw;
        let replacement = if let Some((record, mapped)) = link {
            if !matches!(record.property_type, 0 | 1 | 4)
                || record.components().len() != 2
                || (0..2).all(|axis| active_component(&record, axis).is_none())
            {
                return None;
            }
            let mut resolved = [None; 2];
            for (axis, mapped) in mapped.into_iter().enumerate() {
                let Some(name) = active_component(&record, axis) else {
                    if let Some(effective) = current.and_then(|link| link.resolved[axis]) {
                        authored[axis] = effective;
                    }
                    continue;
                };
                resolved[axis] = if let Some(mapped) = mapped {
                    let effective = field.normalize(mapped)?;
                    (effective.to_bits() != raw[axis].to_bits()).then_some(effective)
                } else {
                    current
                        .filter(|current| {
                            current.record().guid == record.guid
                                && matches!(
                                    (current.record().property_type, record.property_type),
                                    (0 | 4, 0 | 4) | (1, 1)
                                )
                                && active_component(current.record(), axis) == Some(name)
                        })
                        .and_then(|current| current.resolved[axis])
                };
            }
            if current.is_some_and(|current| {
                current.resolved == resolved
                    && super::same_number_adjustment(current.record(), &record)
            }) {
                return Some(false);
            }
            Some(NumericLink {
                record: Arc::new(record),
                resolved,
            })
        } else {
            let Some(current) = current else {
                return Some(false);
            };
            authored = current.effective_pair(raw);
            None
        };

        match (&mut Arc::make_mut(self.layers.get_mut(layer)?).paint, field) {
            (Paint::Patches(patches), SynthesisPairField::GridFrequency) => {
                patches.automatic.frequency = authored.map(|value| value as i32);
                patches.automatic.frequency_adjustment = replacement;
            }
            (Paint::Patches(patches), SynthesisPairField::Move) => {
                patches.automatic.offset = authored.map(|value| value as f32);
                patches.automatic.offset_adjustment = replacement;
            }
            (Paint::Patches(patches), SynthesisPairField::ManualRepeat) => {
                patches.repeat = authored.map(|value| value as i32);
                patches.repeat_adjustment = replacement;
            }
            (Paint::Blend(blend), SynthesisPairField::PhotoRepeat) => {
                blend.repeat = authored.map(|value| value as i32);
                blend.repeat_adjustment = replacement;
            }
            _ => return None,
        }
        Some(true)
    }
}

#[cfg(test)]
mod tests {
    use super::super::SynthesisRecord;
    use super::*;
    use crate::{BinaryCursor, BinaryWriter, SynthesisEdit, SynthesisLayerKind};

    fn link(field: SynthesisPairField, tag: i32, names: [Option<&str>; 2]) -> AdjustmentRecord {
        let mut record = AdjustmentRecord::new(&[field.range().map(|bound| bound as f32); 2]);
        record.property_type = tag;
        record.mode = i32::from(tag == 1);
        for (component, name) in record.components_mut().iter_mut().zip(names) {
            if let Some(name) = name {
                component.active = true;
                component.name = name.encode_utf16().collect();
            }
        }
        record
    }

    fn bytes(record: &SynthesisRecord) -> BinaryWriter {
        let mut writer = BinaryWriter::default();
        record.write(&mut writer).unwrap();
        writer
    }

    fn linked_pair_fixture() -> SynthesisRecord {
        use SynthesisPairField::*;

        let initial = Arc::new(SynthesisTexture::default());
        let mut edit = SynthesisEdit::new(initial.clone(), true);
        edit.add_layer(SynthesisLayerKind::Patches);
        let mut frequency = link(GridFrequency, 0, [Some("Columns"), None]);
        frequency.components_mut()[1].curve_mut().wrap = true;
        assert_eq!(
            edit.set_pair_adjustment(0, GridFrequency, Some((frequency, [Some(12.96), None]))),
            Some(true)
        );
        let mut movement = link(Move, 1, [Some("Translation"), Some("Translation")]);
        movement.components_mut()[0].curve_mut().maximum = 33.3;
        movement.components_mut()[1].curve_mut().maximum = 80.;
        assert_eq!(
            edit.set_pair_adjustment(0, Move, Some((movement, [Some(4.0293), Some(29.36)]))),
            Some(true)
        );
        SynthesisRecord::from_texture(edit.into_local_replacement(&initial).unwrap().unwrap())
    }

    #[test]
    fn paired_synthesis_wire_keeps_defaults_axes_inactive_curve_and_dual_record() {
        use SynthesisPairField::*;

        let linked = linked_pair_fixture();
        let effective = SynthesisEdit::new(linked.texture().clone(), true);
        let moved = [4., f64::from(29.4_f32)];
        assert_eq!(effective.pair_value(0, GridFrequency), Some([13., 4.]));
        assert_eq!(effective.pair_value(0, Move), Some(moved));
        let expected_wire = bytes(&linked);
        let mut reader = BinaryCursor::new(expected_wire.bytes()).unwrap();
        let reopened = SynthesisRecord::read(&mut reader, 3.75).unwrap();
        assert_eq!(
            reader.position().unwrap() as usize,
            expected_wire.bytes().len()
        );
        let draft = SynthesisEdit::new(reopened.texture().clone(), true);
        assert_eq!(draft.pair_value(0, GridFrequency), Some([4., 4.]));
        assert_eq!(draft.pair_value(0, Move), Some([0., 0.]));
        let frequency = draft.pair_adjustment(0, GridFrequency).unwrap();
        assert_eq!((frequency.property_type, frequency.mode), (0, 0));
        assert_eq!(frequency.components().len(), 2);
        assert!(!frequency.components()[1].active);
        assert!(frequency.components()[1].curve.as_ref().unwrap().wrap);
        let movement = draft.pair_adjustment(0, Move).unwrap();
        assert_eq!((movement.property_type, movement.mode), (1, 1));
        assert_eq!(active_component(movement, 0), active_component(movement, 1));
        assert_eq!(
            movement.components()[0].curve.as_ref().unwrap().maximum,
            33.3
        );
        assert_eq!(
            movement.components()[1].curve.as_ref().unwrap().maximum,
            80.
        );

        let mut visited = Vec::new();
        let resolved = reopened
            .map_numeric_adjustments::<()>(|record, axis| {
                visited.push((record.property_type, axis));
                Ok(match (record.property_type, axis) {
                    (0, 0) => Some(12.96),
                    (1, 0) => Some(4.0293),
                    (1, 1) => Some(29.36),
                    _ => unreachable!("inactive axis is not sampled"),
                })
            })
            .unwrap()
            .unwrap();
        assert_eq!(visited, [(0, 0), (1, 0), (1, 1)]);
        assert_eq!(bytes(&reopened).bytes(), expected_wire.bytes());
        assert_eq!(bytes(&resolved).bytes(), expected_wire.bytes());
        let mut detached = SynthesisEdit::new(resolved.texture().clone(), true);
        assert!(detached.set_automatic_placement(0, false));
        assert!(detached.pair_adjustment(0, GridFrequency).is_none());
        assert!(detached.pair_adjustment(0, Move).is_none());
        assert_eq!(detached.pair_value(0, GridFrequency), Some([13., 4.]));
        assert_eq!(detached.pair_value(0, Move), Some(moved));
        let fields = detached.fields(Some(0));
        let mut values: Vec<_> = fields.iter().map(|field| field.value()).collect();
        let move_x = fields
            .iter()
            .position(|field| field.pair_field() == Some((Move, 0)))
            .unwrap();
        values[move_x] = 12.46;
        detached.set_fields(Some(0), &values).unwrap();
        assert_eq!(detached.pair_value(0, Move), Some([12.5, moved[1]]));
        let material = resolved.without_numeric_adjustments();
        let material = SynthesisEdit::new(material.texture().clone(), true);
        assert!(material.pair_adjustment(0, GridFrequency).is_none());
        assert!(material.pair_adjustment(0, Move).is_none());
        assert_eq!(material.pair_value(0, GridFrequency), Some([13., 4.]));
        assert_eq!(material.pair_value(0, Move), Some(moved));
    }

    #[test]
    fn paired_synthesis_removing_one_axis_freezes_it_and_keeps_other_axis_live() {
        use SynthesisPairField::GridFrequency;

        let baseline = linked_pair_fixture();
        let mut draft = SynthesisEdit::new(baseline.texture().clone(), true);
        let field = GridFrequency;
        let mut adjustment = link(field, 4, [Some("Columns"), Some("Rows")]);
        assert_eq!(
            draft.set_pair_adjustment(0, field, Some((adjustment.clone(), [None, Some(6.94)]))),
            Some(true)
        );
        assert_eq!(draft.pair_value(0, field), Some([13., 6.]));
        adjustment.components_mut()[0].active = false;
        let mut invalid = adjustment.clone();
        invalid.property_type = 2;
        assert_eq!(
            draft.set_pair_adjustment(0, field, Some((invalid, [None; 2]))),
            None
        );
        assert_eq!(
            draft.set_pair_adjustment(0, field, Some((adjustment, [None; 2]))),
            Some(true)
        );
        assert_eq!(draft.pair_value(0, field), Some([13., 6.]));

        let fields = draft.fields(Some(0));
        let axes = [0, 1].map(|axis| {
            fields
                .iter()
                .position(|item| item.pair_field() == Some((field, axis)))
                .unwrap()
        });
        assert_eq!(axes.map(|index| fields[index].value()), [13., 4.]);
        let mut values: Vec<_> = fields.iter().map(|item| item.value()).collect();
        values[axes[0]] = 8.;
        values[axes[1]] = 9.;
        draft.set_fields(Some(0), &values).unwrap();
        assert_eq!(draft.pair_value(0, field), Some([8., 6.]));
        assert_eq!(
            axes.map(|index| draft.fields(Some(0))[index].value()),
            [8., 4.]
        );
    }

    #[test]
    fn paired_synthesis_rename_and_relocation_visit_the_correct_link_components() {
        use SynthesisPairField::{GridFrequency, Move};

        let linked = linked_pair_fixture();
        let original: Vec<_> = "Translation".encode_utf16().collect();
        let renamed: Vec<_> = "Position".encode_utf16().collect();
        let wrong_tag = linked
            .rename_numeric_adjustments::<()>([0; 16], 0, &original, &renamed, |_, _| {
                panic!("a scalar control cannot rename a Dual receiver")
            })
            .unwrap();
        assert!(wrong_tag.is_none());
        let result = linked
            .rename_numeric_adjustments::<()>([0; 16], 1, &original, &renamed, |record, axis| {
                assert_eq!(active_component(record, 0), Some(renamed.as_slice()));
                assert_eq!(active_component(record, 1), Some(renamed.as_slice()));
                Ok((axis == 1).then_some(61.25))
            })
            .unwrap()
            .unwrap();
        let renamed_draft = SynthesisEdit::new(result.texture().clone(), true);
        assert_eq!(
            renamed_draft.pair_value(0, Move),
            Some([4., f64::from(61.2_f32)])
        );
        assert_eq!(renamed_draft.pair_value(0, GridFrequency), Some([13., 4.]));
        let mut axes = Vec::new();
        let relocated = result
            .relocate_numeric_adjustments::<()>([2; 16], |record, axis| {
                assert_eq!(record.guid, [0; 16]);
                axes.push((record.property_type, axis));
                Ok(match (record.property_type, axis) {
                    (1, 0) => Some(18.5),
                    _ => None,
                })
            })
            .unwrap()
            .unwrap();
        assert_eq!(axes, [(0, 0), (1, 0), (1, 1)]);
        let moved = SynthesisEdit::new(relocated.texture().clone(), true);
        assert_eq!(moved.pair_adjustment(0, Move).unwrap().guid, [2; 16]);
        assert_eq!(moved.pair_value(0, Move), Some([18.5, 0.]));
        assert_eq!(
            moved.pair_adjustment(0, GridFrequency).unwrap().guid,
            [0; 16]
        );
        assert_eq!(moved.pair_value(0, GridFrequency), Some([13., 4.]));
    }
}
