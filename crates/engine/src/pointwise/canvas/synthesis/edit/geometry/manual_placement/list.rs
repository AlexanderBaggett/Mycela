use super::super::{SynthesisEdit, close_sequence_gap, next_sequence, patches, scale_placement};
use std::sync::Arc;

/// A copied Manual placement and its source extent for pasting into another texture.
#[derive(Clone, Debug)]
pub struct SynthesisPlacementClipboard {
    placement: patches::Placement,
    source_size: Option<[f32; 2]>,
}

impl SynthesisEdit {
    /// Inserts a new copy of the chosen patch at the front of the Manual list.
    pub fn create_manual_placement(
        &mut self,
        layer: usize,
        patch: usize,
        center: [f32; 2],
    ) -> Option<usize> {
        if !center.into_iter().all(|value| (0.0..=1.0).contains(&value)) {
            return None;
        }
        let patches = self.patches_mut(layer)?;
        let selection = patches.selections.get(patch)?;
        let size = signed_selection_size(
            selection,
            patches::selection_size(selection).unwrap_or([0.4; 2]),
        );
        let patch = i32::try_from(patch).ok()?;
        let sequence = next_sequence(&patches.manual, patch);
        patches.manual.insert(
            0,
            patches::Placement {
                identity: Arc::new(()),
                point: center,
                patch,
                sequence,
                angle: 0.,
                size,
            },
        );
        self.changed = true;
        Some(0)
    }

    /// Moves the copy to another row, retaining its per-patch copy number.
    pub fn move_manual_placement(&mut self, layer: usize, index: usize, to: usize) -> bool {
        let Some(patches) = self.patches_mut(layer) else {
            return false;
        };
        if index == to || index.max(to) >= patches.manual.len() {
            return false;
        }
        let placement = patches.manual.remove(index);
        patches.manual.insert(to, placement);
        self.changed = true;
        true
    }

    pub fn copy_manual_placement(
        &self,
        layer: usize,
        index: usize,
    ) -> Option<SynthesisPlacementClipboard> {
        let patches = self.patches(layer)?;
        let placement = patches.manual.get(index)?.clone();
        let source_size = usize::try_from(placement.patch)
            .ok()
            .and_then(|patch| patches.selections.get(patch))
            .and_then(selection_size);
        Some(SynthesisPlacementClipboard {
            placement,
            source_size,
        })
    }

    /// Inserts a copied placement before the selected row, or at the front.
    pub fn paste_manual_placement(
        &mut self,
        layer: usize,
        selected: Option<usize>,
        copied: &SynthesisPlacementClipboard,
    ) -> Option<usize> {
        let patches = self.patches_mut(layer)?;
        let mut placement = copied.placement.clone();
        let patch = usize::try_from(placement.patch)
            .ok()
            .filter(|&patch| patch < patches.selections.len())
            .unwrap_or(0);
        let selection = patches.selections.get(patch)?;
        placement.patch = i32::try_from(patch).ok()?;
        if patches
            .manual
            .iter()
            .any(|peer| peer.patch == placement.patch && peer.sequence == i32::MAX)
        {
            return None;
        }
        adapt_copy_size(
            &mut placement,
            copied.source_size,
            selection_size(selection),
        );
        for peer in &mut patches.manual {
            if peer.patch == placement.patch && peer.sequence >= placement.sequence {
                peer.sequence += 1;
            }
        }
        placement.identity = Arc::new(());
        let index = selected
            .unwrap_or(0)
            .min(patches.manual.len().saturating_sub(1));
        patches.manual.insert(index, placement);
        self.changed = true;
        Some(index)
    }

    /// Removes a copy and returns the preceding selectable row, if any.
    pub fn delete_manual_placement(&mut self, layer: usize, index: usize) -> Option<Option<usize>> {
        if !self.remove_placement(layer, index) {
            return None;
        }
        let has_copies = !self.patches(layer)?.manual.is_empty();
        Some(has_copies.then_some(index.saturating_sub(1)))
    }

    /// Assigns the selected copy to a different source patch of the same layer.
    pub fn set_manual_placement_patch(&mut self, layer: usize, index: usize, patch: usize) -> bool {
        let Some(patches) = self.patches_mut(layer) else {
            return false;
        };
        let (Some(placement), Some(selection), Ok(new_patch)) = (
            patches.manual.get(index),
            patches.selections.get(patch),
            i32::try_from(patch),
        ) else {
            return false;
        };
        if placement.patch == new_patch {
            return false;
        }
        let old = placement.clone();
        let before = usize::try_from(old.patch)
            .ok()
            .and_then(|patch| patches.selections.get(patch))
            .and_then(selection_size);
        let sequence = next_sequence(&patches.manual, new_patch);
        let placement = &mut patches.manual[index];
        adapt_copy_size(placement, before, selection_size(selection));
        placement.patch = new_patch;
        placement.sequence = sequence;
        close_sequence_gap(&mut patches.manual, &old);
        self.changed = true;
        true
    }
}

fn selection_size(selection: &patches::Selection) -> Option<[f32; 2]> {
    Some(signed_selection_size(
        selection,
        patches::selection_size(selection)?,
    ))
}

fn signed_selection_size(selection: &patches::Selection, extent: [f32; 2]) -> [f32; 2] {
    [0, 1].map(|axis| {
        if selection.flipped[axis] {
            -extent[axis]
        } else {
            extent[axis]
        }
    })
}

fn adapt_copy_size(
    placement: &mut patches::Placement,
    before: Option<[f32; 2]>,
    after: Option<[f32; 2]>,
) {
    if let (Some(before), Some(after)) = (before, after) {
        // A negative source extent is the legacy uninitialized sentinel.
        scale_placement(
            placement,
            before.map(|value| if value >= 0. { value } else { f32::NAN }),
            after,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointwise::canvas::synthesis::SynthesisTexture;
    use crate::{SynthesisLayerKind, SynthesisPatchShape, SynthesisPlacementField};

    fn draft() -> SynthesisEdit {
        let mut edit = SynthesisEdit::new(Arc::new(SynthesisTexture::default()), true);
        edit.add_layer(SynthesisLayerKind::Patches);
        edit
    }

    #[test]
    fn manual_copy_creation_reordering_and_deletion_keep_per_patch_numbers() {
        let mut edit = draft();
        let second_patch = edit
            .add_patch_shape(0, SynthesisPatchShape::Rectangle, None, None)
            .unwrap();
        edit.create_manual_placement(0, 0, [0.25, 0.75]).unwrap();
        let first = edit.placement_geometry(0, 0).unwrap();
        edit.create_manual_placement(0, second_patch, [0.5; 2])
            .unwrap();
        edit.create_manual_placement(0, 0, [0.75, 0.25]).unwrap();
        let later = edit.placement_geometry(0, 0).unwrap();
        assert_eq!(first.copy_number(), 1);
        assert_eq!(later.copy_number(), 2);
        assert_eq!(later.center(), [0.75, 0.25]);
        assert_ne!(later.identity(), first.identity());
        assert!(edit.move_manual_placement(0, 0, 2));
        assert_eq!(edit.placement_geometry(0, 1), Some(first));
        assert_eq!(edit.placement_geometry(0, 2), Some(later));
        assert!(!edit.move_manual_placement(0, 2, 3));
        assert_eq!(edit.delete_manual_placement(0, 1), Some(Some(0)));
        let remaining = edit.placement_geometry(0, 1).unwrap();
        assert_eq!(remaining.identity(), later.identity());
        assert_eq!(remaining.copy_number(), 1);
        assert_eq!(edit.delete_manual_placement(0, 1), Some(Some(0)));
        assert_eq!(edit.delete_manual_placement(0, 0), Some(None));
        assert_eq!(edit.delete_manual_placement(0, 0), None);
    }

    #[test]
    fn manual_clipboard_copies_snapshot_and_retargets_missing_source_patch() {
        let mut edit = draft();
        edit.add_layer(SynthesisLayerKind::Patches);
        let other_patch = edit
            .add_patch_shape(0, SynthesisPatchShape::Triangle, None, None)
            .unwrap();
        edit.create_manual_placement(0, other_patch, [0.25, 0.75])
            .unwrap();
        let initial = edit.placement_geometry(0, 0).unwrap();
        assert!(edit.set_manual_placement_field(0, 0, SynthesisPlacementField::Angle, 17.));
        let copied = edit.copy_manual_placement(0, 0).unwrap();
        assert!(edit.set_manual_placement_field(0, 0, SynthesisPlacementField::Angle, 45.));

        assert_eq!(edit.paste_manual_placement(0, Some(0), &copied), Some(0));
        let pasted = edit.placement_geometry(0, 0).unwrap();
        let original = edit.placement_geometry(0, 1).unwrap();
        assert_eq!(pasted.angle(), 17.);
        assert_eq!(original.angle(), 45.);
        assert_eq!(original.copy_number(), 2);
        assert_eq!(original.identity(), initial.identity());
        assert_ne!(pasted.identity(), initial.identity());
        assert_eq!(pasted.copy_number(), 1);
        assert_eq!(pasted.patch(), other_patch);

        assert_eq!(edit.paste_manual_placement(1, None, &copied), Some(0));
        let cross_layer = edit.placement_geometry(1, 0).unwrap();
        let expected = edit.create_manual_placement(1, 0, [0.5; 2]).unwrap();
        assert_eq!(cross_layer.patch(), 0);
        assert_eq!(cross_layer.angle(), 17.);
        assert_eq!(cross_layer.center(), initial.center());
        assert_eq!(
            cross_layer.size(),
            edit.placement_geometry(1, expected).unwrap().size()
        );
        assert_ne!(cross_layer.identity(), pasted.identity());

        assert!(edit.set_manual_placement_patch(0, 0, 0));
        let reassigned = edit.placement_geometry(0, 0).unwrap();
        assert_eq!(reassigned.patch(), 0);
        assert_eq!(reassigned.copy_number(), 1);
        assert_eq!(reassigned.identity(), pasted.identity());
        assert_eq!(reassigned.center(), pasted.center());
        assert_eq!(reassigned.angle(), pasted.angle());
        assert_eq!(reassigned.size(), cross_layer.size());
        assert_eq!(edit.placement_geometry(0, 1).unwrap().copy_number(), 1);
        assert!(!edit.set_manual_placement_patch(0, 0, 0));
    }

    #[test]
    fn manual_use_patch_applies_signed_new_extent_and_retains_a_prior_negative_axis() {
        let mut edit = draft();
        let turned = edit.add_patch(0).unwrap();
        edit.create_manual_placement(0, 0, [0.35, 0.5]).unwrap();
        let original = edit.placement_geometry(0, 0).unwrap();
        assert!(edit.flip_patch(0, turned, 0));
        assert!(edit.set_manual_placement_patch(0, 0, turned));
        let reversed = edit.placement_geometry(0, 0).unwrap();
        let [width, height] = original.size();
        assert_eq!(reversed.size(), [-width, height]);

        assert!(edit.set_manual_placement_patch(0, 0, 0));
        let restored = edit.placement_geometry(0, 0).unwrap();
        assert_eq!(restored.size(), reversed.size());
        assert_eq!(restored.identity(), original.identity());
    }

    #[test]
    fn manual_inspector_keeps_signed_sizes_and_wraps_displayed_center() {
        use SynthesisPlacementField::*;
        let mut edit = draft();
        edit.create_manual_placement(0, 0, [0.5; 2]).unwrap();
        let base = edit.placement_geometry(0, 0).unwrap();
        assert!(edit.set_manual_placement_field(0, 0, Width, -25.126));
        assert!(edit.set_manual_placement_field(0, 0, CenterX, 125.));
        assert!(edit.set_manual_placement_field(0, 0, Angle, 375.));
        let copy = edit.placement_geometry(0, 0).unwrap();
        assert_eq!(copy.identity(), base.identity());
        assert_eq!(copy.field(Width), -25.13);
        assert_eq!(copy.field(Height), base.field(Height));
        assert_eq!(copy.field(CenterX), 25.);
        assert_eq!(copy.authored_field(CenterX), 125.);
        assert_eq!(copy.center(), [1.25, 0.5]);
        assert_eq!(copy.angle(), 360.);
        assert!(!edit.set_manual_placement_field(0, 0, CenterX, f64::NAN));
        assert!(!edit.set_manual_placement_field(0, 0, Width, -25.13));
        assert!(edit.set_manual_placement_field(0, 0, CenterX, 25.));
        assert_eq!(edit.placement_geometry(0, 0).unwrap().center(), [0.25, 0.5]);
        assert!(edit.set_manual_placement_field(0, 0, Width, 0.));
        assert_eq!(edit.placement_geometry(0, 0).unwrap().size()[0], 0.);
    }
}
