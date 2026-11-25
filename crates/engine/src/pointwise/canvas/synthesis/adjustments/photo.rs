use super::super::{photo_blend::PhotoBlend, repeat::Pattern};
use super::{NumericLink, Paint, SynthesisNumberField};
use SynthesisNumberField::{EqualizeStrength, RepeatFeather, RepeatFrequency};

impl Paint {
    pub(super) fn photo_number(
        &self,
        field: SynthesisNumberField,
    ) -> Option<(f64, Option<&NumericLink>)> {
        let (raw, link) = match (self, field) {
            (Self::Repeat(layer), RepeatFrequency) => {
                (f64::from(layer.frequency), &layer.frequency_adjustment)
            }
            (Self::Repeat(layer), RepeatFeather) => (layer.feather, &layer.feather_adjustment),
            (Self::Repeat(layer), EqualizeStrength) => (layer.strength, &layer.strength_adjustment),
            (Self::Blend(layer), EqualizeStrength) => (layer.strength, &layer.strength_adjustment),
            (Self::Patches(layer), EqualizeStrength) => {
                (layer.strength, &layer.strength_adjustment)
            }
            (Self::Blend(layer), field) => {
                let index = field.photo_blend_slot()?;
                return Some((
                    layer.shaping_values()[index],
                    layer.shaping_links.get(index),
                ));
            }
            _ => return None,
        };
        Some((raw, link.as_ref()))
    }

    pub(super) fn set_photo_number_link(
        &mut self,
        field: SynthesisNumberField,
        next: Option<NumericLink>,
        frozen: Option<f64>,
    ) -> Option<()> {
        let (raw, link) = match (self, field) {
            (Self::Repeat(layer), RepeatFrequency) => {
                layer.frequency_adjustment = next;
                if let Some(value) = frozen {
                    layer.frequency = value as i32;
                }
                return Some(());
            }
            (Self::Repeat(layer), RepeatFeather) => {
                (&mut layer.feather, &mut layer.feather_adjustment)
            }
            (Self::Repeat(layer), EqualizeStrength) => {
                (&mut layer.strength, &mut layer.strength_adjustment)
            }
            (Self::Blend(layer), EqualizeStrength) => {
                (&mut layer.strength, &mut layer.strength_adjustment)
            }
            (Self::Patches(layer), EqualizeStrength) => {
                (&mut layer.strength, &mut layer.strength_adjustment)
            }
            (Self::Blend(layer), field) => {
                let index = field.photo_blend_slot()?;
                layer.shaping_links.set(index, next);
                if let Some(value) = frozen {
                    let [stretch_x, stretch_y] = &mut layer.stretch;
                    let [feather_x, feather_y] = &mut layer.edge_feather;
                    let raw = [
                        stretch_x,
                        stretch_y,
                        feather_x,
                        feather_y,
                        &mut layer.star_feather,
                    ];
                    *raw.into_iter().nth(index)? = value;
                }
                return Some(());
            }
            _ => return None,
        };
        *link = next;
        if let Some(value) = frozen {
            *raw = value;
        }
        Some(())
    }
}

impl PhotoBlend {
    fn shaping_values(&self) -> [f64; 5] {
        [
            self.stretch[0],
            self.stretch[1],
            self.edge_feather[0],
            self.edge_feather[1],
            self.star_feather,
        ]
    }

    pub(in crate::pointwise::canvas::synthesis) fn effective_shaping(&self) -> [f64; 5] {
        let raw = self.shaping_values();
        std::array::from_fn(|index| {
            NumericLink::effective_value(raw[index], self.shaping_links.get(index))
        })
    }
}

impl Pattern {
    pub(in crate::pointwise::canvas::synthesis) fn effective_frequency(&self) -> i32 {
        NumericLink::effective_value(
            f64::from(self.frequency),
            self.frequency_adjustment.as_ref(),
        ) as i32
    }

    pub(in crate::pointwise::canvas::synthesis) fn effective_feather(&self) -> f64 {
        NumericLink::effective_value(self.feather, self.feather_adjustment.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointwise::canvas::synthesis::{SynthesisRecord, SynthesisTexture};
    use crate::{AdjustmentRecord, BinaryCursor, BinaryWriter, SynthesisEdit, SynthesisLayerKind};
    use std::sync::Arc;

    #[test]
    fn equalize_strength_links_follow_all_three_photo_layers_through_wire_and_clear() {
        let initial = Arc::new(SynthesisTexture::default());
        let mut draft = SynthesisEdit::new(initial.clone(), true);
        for kind in [
            SynthesisLayerKind::Repeating,
            SynthesisLayerKind::Blend,
            SynthesisLayerKind::Patches,
        ] {
            draft.add_layer(kind);
        }
        let mut adjustment = AdjustmentRecord::new(&[[0., 100.]]);
        let field = &mut adjustment.components_mut()[0];
        field.name = "Shared equalization".encode_utf16().collect();
        field.active = true;
        for layer in 0..3 {
            assert_eq!(
                draft.set_number_adjustment(
                    Some(layer),
                    EqualizeStrength,
                    Some((adjustment.clone(), 36.26)),
                ),
                Some(true)
            );
        }
        let texture = draft.into_local_replacement(&initial).unwrap().unwrap();
        let mut writer = BinaryWriter::default();
        texture.write::<()>(&mut writer).unwrap();
        let mut reader = BinaryCursor::new(writer.bytes()).unwrap();
        let record = SynthesisRecord::read(&mut reader, 3.75).unwrap();
        let resolved = record
            .map_numeric_adjustments::<()>(|_, _| Ok(Some(36.26)))
            .unwrap()
            .unwrap();
        let mut draft = SynthesisEdit::new(resolved.0, true);
        for layer in 0..3 {
            assert!(
                draft
                    .number_adjustment(Some(layer), EqualizeStrength)
                    .is_some()
            );
            assert_eq!(
                draft.number_value(Some(layer), EqualizeStrength),
                Some(36.3)
            );
            assert!(draft.clear_source(layer));
            assert!(
                draft
                    .number_adjustment(Some(layer), EqualizeStrength)
                    .is_none()
            );
            assert_eq!(
                draft.number_value(Some(layer), EqualizeStrength),
                Some(36.3)
            );
        }
    }
}
