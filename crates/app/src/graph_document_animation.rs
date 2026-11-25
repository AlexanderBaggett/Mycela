//! Graph inspector edits of actual retained filter animation owners.
use super::super::inspector::{AnimationOwner, AnimationToggle, LayerSelection};
use super::{GraphDocumentView, model::Node};
use crate::ForegroundError;
use crate::curve_layer_menu::{
    Layer, LayerAction, apply_layer_action, render_curve_layer_menu, same_layers,
};
use gpui_kit::{Context, Window};
use mycela_engine::{
    AnalyticCurveKind, CurveControlHandle, CurveControlKind, CurveLayerHandle, SharedCurve,
};

fn same_animation(a: Option<&AnimationOwner>, b: Option<&AnimationOwner>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_reference(b),
        _ => false,
    }
}
fn same_curve(
    a: Option<&SharedCurve<ForegroundError>>,
    b: Option<&SharedCurve<ForegroundError>>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_reference(b),
        _ => false,
    }
}
#[derive(Clone)]
pub(super) struct AnimationTarget {
    pub selection: Option<String>,
    pub node: Option<Node>,
    pub revision: u64,
    pub item: AnimationToggle,
}
impl GraphDocumentView {
    pub(super) fn toggle_graph_animation(
        &mut self,
        target: &AnimationTarget,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selection = &target.selection;
        let node = &target.node;
        let revision = target.revision;
        let item = &target.item;
        if self.closed || self.is_busy() || !self.is_original_selection(selection, node, revision) {
            return;
        }
        if self.inspector.is_changed(cx) {
            self.error = Some("Correct or reset field text before changing animation.".into());
            cx.notify();
            return;
        }
        let parameter = &item.parameter;
        let current = parameter.retained();
        let status = parameter.axis(item.axis);
        if !current
            .as_ref()
            .is_ok_and(|v| same_animation(v.as_ref(), item.animation.as_ref()))
            || !status.as_ref().is_ok_and(|(active, curve)| {
                *active == item.enabled && same_curve(curve.as_ref(), item.curve.as_ref())
            })
        {
            self.error = Some("The animation changed. Revert fields before continuing.".into());
            cx.notify();
            return;
        }
        if enabled == item.enabled {
            return;
        }
        if let Some(reason) = parameter.animation_link_conflict() {
            self.error = Some(reason.into());
            cx.notify();
            return;
        }
        if parameter.is_solid_animation() || parameter.records_curve_history() {
            self.ensure_history_binding();
            if let Some(history) = self.history() {
                history.separate_next_edit();
            }
        }
        let stamp = self.begin_history_edit();
        self.revision += 1;
        let revision = self.revision;
        self.status = None;
        self.error = None;
        let result = (|| -> Result<(), String> {
            let edit = parameter.animation_edit(item.animation.as_ref())?;
            let owner = if let Some(edit) = &edit {
                edit.owner.clone()
            } else if enabled {
                parameter.ensure()?
            } else {
                parameter
                    .retained()?
                    .ok_or("The animation is unavailable.")?
            };
            let existing = owner.curve(item.axis)?;
            if enabled && existing.is_none() {
                owner.ensure_curve(item.axis)?;
                let curve = owner
                    .curve(item.axis)?
                    .ok_or("The animation curve is unavailable.")?;
                curve
                    .insert(0, Some(&CurveLayerHandle::new(AnalyticCurveKind::Linear)))
                    .map_err(|e| e.to_string())?;
            }
            if let Some(edit) = edit {
                let layer = edit.copy_selection(self.inspector.layer.as_ref())?;
                edit.set_axis(item.axis, enabled)?;
                edit.publish(Some(&self.document))?;
                if layer.is_some() {
                    self.inspector.layer = layer;
                }
                return Ok(());
            }
            let expected_curve = owner.curve(item.axis)?;
            if !self.is_original_selection(selection, node, revision)
                || !parameter
                    .retained()?
                    .as_ref()
                    .is_some_and(|v| v.same_reference(&owner))
            {
                return Err(
                    "The selected animation changed while editing. Earlier changes were kept."
                        .into(),
                );
            }
            parameter.set_axis(&owner, item.axis, enabled)?;
            let current = parameter.retained()?;
            let status = parameter.axis(item.axis)?;
            if !self.is_original_selection(selection, node, revision)
                || !current.as_ref().is_some_and(|v| v.same_reference(&owner))
                || status.0 != enabled
                || !same_curve(status.1.as_ref(), expected_curve.as_ref())
            {
                return Err(
                    "The selected animation changed while editing. Earlier changes were kept."
                        .into(),
                );
            }
            Ok(())
        })();
        if !self.is_original_selection(selection, node, revision) {
            self.error = Some(result.err().unwrap_or_else(|| {
                "The selected node changed while editing. Earlier changes were kept.".into()
            }));
            self.retire_stale_preview();
            cx.notify();
            return;
        }
        self.error = result.err();
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        self.start_preview(cx);
    }
}

impl GraphDocumentView {
    fn edit_graph_layers(
        &mut self,
        target: &AnimationTarget,
        curve: &SharedCurve<ForegroundError>,
        layers: &[Option<Layer>],
        action: LayerAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed
            || self.is_busy()
            || !self.is_original_selection(&target.selection, &target.node, target.revision)
        {
            return;
        }
        if self.inspector.is_changed(cx) {
            self.error =
                Some("Correct or reset field text before changing animation layers.".into());
            cx.notify();
            return;
        }
        let parameter = &target.item.parameter;
        let valid = || -> Result<bool, String> {
            let owner = parameter.retained()?;
            let (enabled, actual) = parameter.axis(target.item.axis)?;
            Ok(enabled
                && same_animation(owner.as_ref(), target.item.animation.as_ref())
                && same_curve(actual.as_ref(), Some(curve)))
        };
        if !valid().unwrap_or(false) || !same_layers(&curve.layers(), layers) {
            self.error = Some("The animation layers changed. Open the menu again.".into());
            cx.notify();
            return;
        }
        if let LayerAction::Edit(index) = action {
            let result = (|| {
                let layer = layers
                    .get(index)
                    .and_then(Option::as_ref)
                    .ok_or("The animation layer is unavailable.")?
                    .clone();
                let animation = target
                    .item
                    .animation
                    .clone()
                    .ok_or("The animation is unavailable.")?;
                self.inspector.select_layer(
                    LayerSelection {
                        parameter: parameter.clone(),
                        animation,
                        axis: target.item.axis,
                        curve: curve.clone(),
                        layer,
                        ancestry: Vec::new(),
                    },
                    window,
                    cx,
                )
            })();
            self.error = result.err();
            self.observe_drafts(cx);
            cx.notify();
            return;
        }
        if parameter.records_curve_history() {
            self.ensure_history_binding();
        }
        let stamp = self.begin_history_edit();
        self.revision += 1;
        let revision = self.revision;
        self.status = None;
        let result = (|| -> Result<(), String> {
            let edit = parameter.animation_edit(target.item.animation.as_ref())?;
            let selected_layer = edit
                .as_ref()
                .map(|edit| edit.copy_selection(self.inspector.layer.as_ref()))
                .transpose()?
                .flatten();
            let working_curve = match &edit {
                Some(edit) => edit
                    .owner
                    .curve(target.item.axis)?
                    .ok_or("The animation curve is unavailable.")?,
                None => curve.clone(),
            };
            apply_layer_action(&working_curve, action, |remaining| {
                if !self.is_original_selection(&target.selection, &target.node, revision)
                    || !valid()?
                    || !same_layers(&working_curve.layers(), remaining)
                {
                    return Err(
                        "The animation changed while moving the layer. Earlier changes were kept."
                            .into(),
                    );
                }
                Ok(())
            })?;
            if !self.is_original_selection(&target.selection, &target.node, revision) || !valid()? {
                return Err(
                    "The selected animation changed while editing. Earlier changes were kept."
                        .into(),
                );
            }
            if let Some(edit) = edit {
                edit.publish(Some(&self.document))?;
                if selected_layer.is_some() {
                    self.inspector.layer = selected_layer;
                }
            }
            Ok(())
        })();
        if !self.is_original_selection(&target.selection, &target.node, revision) {
            self.error = Some(result.err().unwrap_or_else(|| {
                "The selected node changed while editing. Earlier changes were kept.".into()
            }));
            self.retire_stale_preview();
            cx.notify();
            return;
        }
        self.error = result.err();
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        self.start_preview(cx);
    }
    pub(super) fn render_graph_layers(
        &self,
        target: AnimationTarget,
        curve: &SharedCurve<ForegroundError>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let id = target.item.id.clone();
        let owner = cx.entity().downgrade();
        let curve = curve.clone();
        let layers = curve.layers();
        render_curve_layer_menu(&id, &curve, &layers, disabled, cx, {
            let curve = curve.clone();
            let layers = layers.clone();
            move |action, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.edit_graph_layers(&target, &curve, &layers, action, window, cx)
                });
            }
        })
    }
}

impl GraphDocumentView {
    pub(super) fn toggle_graph_layer_control(
        &mut self,
        selection: &LayerSelection,
        control: &CurveControlHandle<ForegroundError>,
        values: (i32, i32),
        target: &AnimationTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (expected, value) = values;
        if self.closed
            || self.is_busy()
            || !self.is_original_selection(&target.selection, &target.node, target.revision)
        {
            return;
        }
        if self.inspector.is_changed(cx) {
            self.error =
                Some("Correct or reset field text before changing a layer control.".into());
            cx.notify();
            return;
        }
        let check = (|| -> Result<(), String> {
            let current = self
                .inspector
                .layer
                .as_ref()
                .ok_or("Choose the animation layer again.")?;
            if !current.same_path(selection) {
                return Err("The selected layer changed. Choose it again.".into());
            }
            selection.validate(Some(control))?;
            let actual = match control.kind() {
                CurveControlKind::Toggle => i32::from(control.toggle().map_err(|e| e.to_string())?),
                CurveControlKind::Direction => control.enumeration().map_err(|e| e.to_string())?,
                _ => return Err("The layer control is unavailable.".into()),
            };
            if actual != expected {
                return Err("The layer control changed. Try again.".into());
            }
            Ok(())
        })();
        if let Err(error) = check {
            self.error = Some(error);
            cx.notify();
            return;
        }
        if expected == value {
            return;
        }
        if selection.parameter.records_curve_history() {
            self.ensure_history_binding();
        }
        let stamp = self.begin_history_edit();
        self.revision += 1;
        let revision = self.revision;
        self.status = None;
        let result = (|| -> Result<(), String> {
            let edit = selection
                .parameter
                .animation_edit(Some(&selection.animation))?;
            let copy = edit
                .as_ref()
                .map(|edit| selection.on_animation(&edit.owner))
                .transpose()?;
            let edited_control = match &copy {
                Some(copy) => selection.copy_control(control, copy)?,
                None => control.clone(),
            };
            match edited_control.kind() {
                CurveControlKind::Toggle => edited_control.set_toggle(value != 0),
                CurveControlKind::Direction => edited_control.set_enumeration(value),
                _ => unreachable!(),
            }
            .map_err(|e| e.to_string())?;
            if let Some(edit) = edit {
                edit.publish(Some(&self.document))?;
                self.inspector.layer = copy;
            } else {
                selection.validate(Some(control))?;
            }
            Ok(())
        })();
        if !self.is_original_selection(&target.selection, &target.node, revision) {
            self.error = Some(result.err().unwrap_or_else(|| {
                "The selected node changed while editing. Earlier changes were kept.".into()
            }));
            self.retire_stale_preview();
            cx.notify();
            return;
        }
        self.error = result.err();
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        self.start_preview(cx);
    }
}
