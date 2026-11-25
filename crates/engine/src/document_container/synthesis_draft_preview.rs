use super::{
    Gtx375GraphDocument, GtxGraphError, MaterialDraft, SynthesisAdjuster, SynthesisAdjusterBatch,
    graph_nodes, stale_lab_material, stale_material_paint,
};
use crate::pointwise::EnvironmentPaintEdit;
use crate::pointwise::property::capture::DraftRenderOverrides;
use crate::{CanvasLayerEditor, PointwiseNode, RenderFreezeSession, StyleEdit, SynthesisEdit};
use std::sync::Arc;

/// The live owner against which a detached Synthesis Source edit was opened.
pub enum SynthesisDraftPreviewTarget<'a, E> {
    Standalone {
        owner: &'a PointwiseNode<E>,
        material: Option<&'a SynthesisAdjuster>,
    },
    Style {
        owner: &'a PointwiseNode<E>,
        canvas: Option<&'a CanvasLayerEditor<E>>,
        expected: &'a StyleEdit,
        pass: usize,
        material: Option<&'a SynthesisAdjuster>,
    },
    LabMaterial {
        selected: Option<&'a PointwiseNode<E>>,
        material: &'a SynthesisAdjuster,
    },
}

impl<E: 'static> Gtx375GraphDocument<E> {
    /// Creates a render-only overlay for a pending Source edit. The caller can
    /// freeze the editor and its visible graph consumers in this one session.
    /// A valid unmodified draft returns `None`.
    pub fn preview_synthesis_draft(
        &self,
        target: SynthesisDraftPreviewTarget<'_, E>,
        draft: SynthesisEdit,
    ) -> Result<Option<RenderFreezeSession<E>>, GtxGraphError<E>> {
        use SynthesisDraftPreviewTarget::*;

        let branches = self.branches();
        let mut overrides = DraftRenderOverrides::default();
        let shared = match target {
            Standalone { owner, material } => {
                if !graph_nodes(&branches).any(|node| node.same_reference(owner))
                    || draft.replaces_scale_and_seed()
                {
                    return Err(stale_texture());
                }
                let filter = owner.filter().ok_or_else(stale_texture)?;
                let expected = filter.synthesis_edit().ok_or_else(stale_texture)?;
                if !draft.same_revision(&expected) {
                    return Err(stale_texture());
                }
                let catalog = self.synthesis_adjuster(&filter);
                let value = match (material, catalog) {
                    (Some(expected), Some(actual)) if expected.same_reference(&actual) => {
                        Some(actual.value)
                    }
                    (None, None) if !filter.is_synthesis_adjusted() => None,
                    _ => return Err(stale_lab_material()),
                };
                let before = expected.record();
                let after = draft.record();
                if Arc::ptr_eq(before.texture(), after.texture()) {
                    return Ok(None);
                }
                if let Some(value) = value {
                    Some(MaterialDraft {
                        value,
                        before,
                        after,
                    })
                } else {
                    overrides.synthesis.insert(owner.occurrence_id(), after);
                    None
                }
            }
            Style {
                owner,
                canvas,
                expected,
                pass,
                material,
            } => {
                let (filter, mut style) =
                    Self::material_style_owner(&branches, owner, canvas, expected)?;
                if let Some(material) = material {
                    let actual = self
                        .style_synthesis_adjuster(&style, pass)
                        .ok_or_else(stale_material_paint)?;
                    if !actual.same_reference(material) {
                        return Err(stale_material_paint());
                    }
                    let Some(shared) = self.prepare_style_material_draft(&style, pass, draft)?
                    else {
                        return Ok(None);
                    };
                    Some(shared)
                } else {
                    if !style
                        .set_synthesis(pass, draft)
                        .map_err(GtxGraphError::SynthesisAdjuster)?
                    {
                        return Ok(None);
                    }
                    let paint = EnvironmentPaintEdit::from_style_edit(&filter, canvas, style)
                        .ok_or_else(stale_material_paint)?;
                    paint.collect_draft_preview(&mut overrides);
                    None
                }
            }
            LabMaterial { selected, material } => {
                let Some(shared) =
                    self.prepare_lab_material_draft(&branches, selected, material, draft)?
                else {
                    return Ok(None);
                };
                Some(shared)
            }
        };
        if let Some(MaterialDraft {
            value,
            before,
            after,
        }) = shared
        {
            SynthesisAdjusterBatch::replace_material(&branches, value, before, after)
                .collect_draft_preview(&mut overrides);
        }
        Ok(Some(RenderFreezeSession::with_draft_override(overrides)))
    }
}

fn stale_texture<E>() -> GtxGraphError<E> {
    GtxGraphError::SynthesisAdjuster("The Texture Synthesis editor target changed.")
}

#[cfg(test)]
#[path = "synthesis_draft_preview_tests.rs"]
mod tests;
