use super::{NumericLink, SynthesisNumberField};
use crate::pointwise::canvas::style::Blend;
use std::borrow::Cow;

use SynthesisNumberField::{BlendErode, BlendMode, BlendOpacity, BlendSharpness, ErodeMode};

impl SynthesisNumberField {
    pub(super) fn blend_slot(self) -> Option<usize> {
        Self::BLEND_WIRE_FIELDS
            .iter()
            .position(|field| *field == Some(self))
    }
}

impl Blend {
    pub(super) fn synthesis_number(
        &self,
        field: SynthesisNumberField,
    ) -> Option<(f64, Option<&NumericLink>)> {
        let index = field.blend_slot()?;
        let raw = match field {
            BlendMode => f64::from(self.mode) - 1.,
            ErodeMode => f64::from(self.erosion_mode),
            BlendOpacity => self.opacity,
            BlendErode => f64::from(self.erosion),
            BlendSharpness => f64::from(self.erosion_sharpness),
            _ => return None,
        };
        Some((raw, self.synthesis_links.get(index)))
    }

    pub(in crate::pointwise::canvas::synthesis) fn set_raw_synthesis_number(
        &mut self,
        field: SynthesisNumberField,
        value: f64,
    ) {
        match field {
            BlendMode => self.mode = value as i32 + 1,
            ErodeMode => self.erosion_mode = value as i32,
            BlendOpacity => self.opacity = value,
            BlendErode => self.erosion = value as f32,
            BlendSharpness => self.erosion_sharpness = value as f32,
            _ => unreachable!("Synthesis Blend property"),
        }
    }

    pub(in crate::pointwise::canvas::synthesis) fn effective_synthesis(&self) -> Cow<'_, Self> {
        if self.synthesis_links.is_empty() {
            return Cow::Borrowed(self);
        }
        let mut effective = self.clone();
        for (index, field) in SynthesisNumberField::BLEND_WIRE_FIELDS
            .into_iter()
            .enumerate()
        {
            if let Some(field) = field
                && let Some(value) = self
                    .synthesis_links
                    .get(index)
                    .and_then(|link| link.resolved[0])
            {
                effective.set_raw_synthesis_number(field, value);
            }
        }
        Cow::Owned(effective)
    }
}
