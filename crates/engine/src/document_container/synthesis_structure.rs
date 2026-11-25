//! Reconcile Lab definitions when a Synthesis edit removes receivers.
use super::super::lab_preset_scope::{LabProjection, PresetRefresh};
use super::*;
use crate::SynthesisRecord;

impl<E: 'static> Gtx375GraphDocument<E> {
    /// Applies a Style or Canvas paint edit when it retires a Lab definition.
    /// A false result leaves the draft for the ordinary edit path.
    pub fn edit_style_removing_unused_synthesis_adjusters(
        &mut self,
        owner: &PointwiseNode<E>,
        canvas: Option<&CanvasLayerEditor<E>>,
        edit: &StyleEdit,
        label: &str,
    ) -> Result<bool, GtxGraphError<E>> {
        let filter = owner.filter().ok_or_else(stale_paint)?;
        let Some(paint) = EnvironmentPaintEdit::from_style_edit(&filter, canvas, edit.clone())
        else {
            return Ok(false);
        };
        self.finish_synthesis_removal(owner, SynthesisLabTarget::Paint(paint), label)
    }

    /// Removes a Canvas layer along with the Lab definitions it leaves unused.
    pub fn remove_canvas_layer_with_synthesis_adjusters(
        &mut self,
        owner: &PointwiseNode<E>,
        layer: &CanvasLayerEditor<E>,
    ) -> Result<bool, GtxGraphError<E>> {
        let filter = owner.filter().ok_or_else(stale_paint)?;
        let paint = EnvironmentPaintEdit::from_canvas_layer_removal(&filter, layer)
            .ok_or_else(stale_paint)?;
        self.finish_synthesis_removal(
            owner,
            SynthesisLabTarget::Paint(paint),
            "Delete Canvas layer",
        )
    }

    /// Applies a standalone Texture edit that retires Lab definitions. The
    /// Texture may be local, or shared by multiple receivers of one Material.
    pub fn edit_synthesis_removing_unused_adjusters(
        &mut self,
        owner: &PointwiseNode<E>,
        edit: &SynthesisEdit,
        label: &str,
    ) -> Result<bool, GtxGraphError<E>> {
        let filter = owner.filter().ok_or_else(stale_paint)?;
        let Some(texture) = filter
            .prepare_synthesis_edit(edit.clone())
            .map_err(|error| GtxGraphError::Node(error.into()))?
        else {
            return Ok(false);
        };
        self.finish_synthesis_removal(owner, SynthesisLabTarget::Texture(texture), label)
    }

    fn finish_synthesis_removal(
        &mut self,
        owner: &PointwiseNode<E>,
        target: SynthesisLabTarget<E>,
        label: &str,
    ) -> Result<bool, GtxGraphError<E>> {
        let branches = self.branches();
        let (mut retired, removed) = match &target {
            SynthesisLabTarget::Paint(paint) => {
                let (retired, records) = self
                    .retired_synthesis_materials(&branches, paint.removed_synthesis_materials());
                (retired, paint.removed_synthesis_lab_links(records))
            }
            SynthesisLabTarget::Texture(texture) => {
                let [before, after] = texture.records();
                (
                    Vec::new(),
                    SynthesisRecord::removed_lab_links_between([before], [after]),
                )
            }
            _ => unreachable!(),
        };
        if retired.is_empty() && removed.is_empty() {
            return Ok(false);
        }
        if !graph_nodes(&branches).any(|node| node.same_reference(owner)) {
            return Err(stale_paint());
        }
        let keys: Vec<_> = removed
            .iter()
            .map(|(group, tag, name)| (*group, *tag, name.as_slice()))
            .collect();
        retired.extend(self.retired_synthesis_number_links(&branches, &keys));
        retired.extend(self.retired_synthesis_color_links(&branches, &keys));
        if retired.is_empty() {
            return Ok(false);
        }
        let projected: Vec<_> = self
            .catalog
            .entries()
            .filter_map(|(entry, marker)| {
                (!retired.iter().any(|old| Arc::ptr_eq(old, marker))).then_some(entry)
            })
            .collect();
        let projection = LabProjection::read(&branches, None, None, crate::PointwiseSlot::node);
        let refresh = PresetRefresh::stage_projected(&mut self.presets, &projection, &projected);
        retired.extend(refresh.retired);
        self.commit_synthesis_lab_edit(target, refresh.added, retired, label)
    }
}
