//! The standalone Gradient uses the graph inspector's curve editing operations.
use super::*;
use crate::{
    curve_layer_menu::{
        Layer, LayerAction, apply_layer_action, render_curve_layer_menu, same_layers,
    },
    graph_document::{
        CustomAction, CustomDraft, ModulationAction, ModulationTarget,
        inspector::{AnimationEdit, AnimationOwner, AnimationParameter, LayerSelection},
        render_custom_points, render_layer_modulation,
    },
};
use mycela_engine::{CurveControlHandle, CurveControlKind};

pub(super) enum GradientCurveEdit {
    Axis {
        parameter: AnimationParameter,
        expected: Option<AnimationOwner>,
        axis: usize,
        enabled: bool,
    },
    Fields {
        edits: Vec<AnimationEdit>,
        bounds: Vec<usize>,
    },
    Layers {
        edit: AnimationEdit,
        selection: Option<LayerSelection>,
    },
}

impl GradientCurveEdit {
    pub(super) fn applied_bounds(&self) -> &[usize] {
        match self {
            Self::Fields { bounds, .. } => bounds,
            _ => &[],
        }
    }

    // Runs on the foreground task outside the view borrow. Each publish can
    // reenter the source workspace and retire this activation.
    pub(super) fn publish(
        self,
        mut selection: Option<LayerSelection>,
        admits: impl Fn() -> bool,
    ) -> Result<Option<LayerSelection>, String> {
        let edits = match self {
            Self::Axis {
                parameter,
                expected,
                axis,
                enabled,
            } => {
                let edit = parameter
                    .animation_edit(expected.as_ref())?
                    .ok_or("The Gradient animation is unavailable.")?;
                if enabled && edit.owner.curve(axis)?.is_none() {
                    edit.owner.ensure_curve(axis)?;
                    let curve = edit
                        .owner
                        .curve(axis)?
                        .ok_or("The animation curve is unavailable.")?;
                    curve
                        .insert(0, Some(&Layer::new(AnalyticCurveKind::Linear)))
                        .map_err(|error| error.to_string())?;
                }
                edit.set_axis(axis, enabled)?;
                vec![edit]
            }
            Self::Fields { edits, .. } => edits,
            Self::Layers { edit, selection } => {
                if admits() {
                    edit.publish(None)?;
                }
                return Ok(selection);
            }
        };
        for edit in edits {
            if !admits() {
                break;
            }
            let next = edit.copy_selection(selection.as_ref())?;
            edit.publish(None)?;
            if next.is_some() {
                selection = next;
            }
        }
        Ok(selection)
    }
}

impl GradientSourceView {
    fn curve_edits_available(&self) -> bool {
        !self.pending
            && self.presentation.is_some()
            && self
                .input_owner
                .as_ref()
                .is_none_or(|owner| owner.is_current())
    }

    fn animation_parameter(&self, index: usize) -> (AnimationParameter, usize) {
        let (kind, axis) = parameter_axis(&self.model, index);
        (
            AnimationParameter::Generator(self.model.parameter(kind)),
            axis as usize,
        )
    }

    pub(super) fn toggle_source_animation(
        &mut self,
        index: usize,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.model.kind() != mycela_engine::ImageGeneratorKind::Gradient {
            self.submit(
                Edit::Animation {
                    axis: index,
                    enabled,
                },
                window,
                cx,
            );
            return;
        }
        if self.layer_inspector.is_changed(cx) {
            self.error = Some("Apply or revert layer edits before changing animation.".into());
            cx.notify();
            return;
        }
        let (parameter, axis) = self.animation_parameter(index);
        match parameter.retained() {
            Ok(expected) => self.submit(
                Edit::GradientCurve(GradientCurveEdit::Axis {
                    parameter,
                    expected,
                    axis,
                    enabled,
                }),
                window,
                cx,
            ),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    pub(super) fn stage_curve_fields(&self, cx: &App) -> Result<Option<GradientCurveEdit>, String> {
        if self
            .layer_inspector
            .custom
            .as_ref()
            .is_some_and(|point| point.is_changed(cx))
        {
            return Err("Add or move the point, or revert point edits first.".into());
        }
        let mut edits = Vec::new();
        let mut bounds = Vec::new();
        for (index, draft) in self.curves.iter().enumerate() {
            let text = self.curve_text(index, cx);
            if text == draft.synced {
                continue;
            }
            let (kind, axis) = parameter_axis(&self.model, index);
            let parameter = self.model.parameter(kind);
            let owner = parameter
                .retained_animation()
                .map_err(|error| error.to_string())?
                .ok_or("The animation changed. Revert curve edits before continuing.")?;
            let curve = owner
                .curves()
                .ok_or("The animation curves are unavailable.")?
                .value(axis)
                .map_err(|error| error.to_string())?
                .ok_or("The animation curve is unavailable.")?;
            if [curve.minimum().to_string(), curve.maximum().to_string()] != draft.synced {
                return Err(
                    "The curve bounds changed. Revert curve edits before continuing.".into(),
                );
            }
            let (Ok(lower), Ok(upper)) = (text[0].parse::<f32>(), text[1].parse::<f32>()) else {
                return Err(format!(
                    "Enter numbers for the {} curve bounds.",
                    axis_label(&self.model, index)
                ));
            };
            if !lower.is_finite() || !upper.is_finite() {
                return Err(format!(
                    "Enter finite bounds for the {} curve.",
                    axis_label(&self.model, index)
                ));
            }
            let edit = AnimationEdit::for_property(
                &mut edits,
                &AnimationParameter::Generator(parameter),
                &AnimationOwner::Legacy(owner),
            )?;
            let curve = edit
                .owner
                .curve(axis as usize)?
                .ok_or("The animation curve is unavailable.")?;
            curve.set_minimum(lower);
            curve.set_maximum(upper);
            bounds.push(index);
        }
        for draft in &self.layer_inspector.drafts {
            draft.stage_animation(&mut edits, cx)?;
        }
        Ok((!edits.is_empty()).then_some(GradientCurveEdit::Fields { edits, bounds }))
    }

    pub(super) fn sync_layer_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || self.layer_inspector.is_changed(cx) {
            return;
        }
        if self
            .layer_inspector
            .layer
            .as_ref()
            .is_some_and(|layer| layer.validate(None).is_err())
        {
            self.layer_inspector.close_layer();
        } else if let Err(error) = self.layer_inspector.sync_unchanged(window, cx) {
            self.error.get_or_insert(error);
        }
    }

    pub(super) fn set_layer_selection(
        &mut self,
        selection: Option<LayerSelection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(selection) = selection.filter(|layer| layer.validate(None).is_ok()) else {
            self.layer_inspector.close_layer();
            return Ok(());
        };
        if self
            .layer_inspector
            .layer
            .as_ref()
            .is_some_and(|current| current.same_path(&selection))
        {
            return Ok(());
        }
        self.layer_inspector.select_layer(selection, window, cx)
    }

    pub(super) fn curve_layer_section(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let disabled = !self.curve_edits_available();
        let id = self.control_id(format!("curve-{}", axis_key(&self.model, index)));
        let open = self.curve_axis == Some(index);
        let button = Button::new(format!("{id}:editor"))
            .label(if open {
                "Hide layers"
            } else {
                "Edit curve layers"
            })
            .accessibility_label(format!(
                "{} {} layers",
                if open { "Hide" } else { "Edit" },
                axis_label(&self.model, index)
            ))
            .disabled(disabled)
            .on_click(cx.listener(move |view, _, _, cx| {
                if !view.curve_edits_available() {
                    return;
                }
                if view.layer_inspector.is_changed(cx) {
                    view.error = Some("Apply or revert layer edits before changing curves.".into());
                } else {
                    view.curve_axis = (view.curve_axis != Some(index)).then_some(index);
                    view.layer_inspector.close_layer();
                    view.error = None;
                }
                cx.notify();
            }));
        let mut section = div().v_flex().gap_2().child(div().child(button));
        if open {
            let (parameter, axis) = self.animation_parameter(index);
            if let Ok(Some(owner)) = parameter.retained()
                && let Ok(Some(curve)) = owner.curve(axis)
            {
                let layers = curve.layers();
                let weak = cx.entity().downgrade();
                section = section.child(render_curve_layer_menu(
                    &id,
                    &curve,
                    &layers,
                    disabled,
                    cx,
                    {
                        let curve = curve.clone();
                        let layers = layers.clone();
                        move |action, window, cx| {
                            let _ = weak.update(cx, |view, cx| {
                                let result = view.edit_curve_layers(
                                    index,
                                    (&owner, &curve, &layers),
                                    action,
                                    window,
                                    cx,
                                );
                                view.error = result.err();
                                cx.notify();
                            });
                        }
                    },
                ));
            }
            if let Some(layer) = &self.layer_inspector.layer
                && layer.validate(None).is_ok()
            {
                section = section.child(self.selected_layer(&id, layer, disabled, cx));
            }
        }
        section.into_any_element()
    }

    fn selected_layer(
        &self,
        id: &str,
        selection: &LayerSelection,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let mut section = div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .font_medium()
                    .child(format!("{:?} layer", selection.layer.kind())),
            )
            .child(
                Form::new()
                    .columns(2)
                    .children(self.layer_inspector.drafts.iter().map(|draft| {
                        Field::new().label(draft.label.clone()).child(
                            Input::new(&draft.input)
                                .id(format!("{id}:{}", draft.id))
                                .aria_label(draft.label.clone())
                                .disabled(disabled || draft.is_layer_controlled()),
                        )
                    })),
            );
        for control in selection.controls() {
            let (label, expected) = match control.kind() {
                CurveControlKind::Toggle => (
                    control
                        .name()
                        .as_deref()
                        .map(String::from_utf16_lossy)
                        .unwrap_or_else(|| "Option".into()),
                    control.toggle().ok().map(i32::from),
                ),
                CurveControlKind::Direction => {
                    ("Reverse direction".into(), control.enumeration().ok())
                }
                _ => continue,
            };
            let Some(expected) = expected else { continue };
            let selection = selection.clone();
            section = section.child(
                Checkbox::new(format!("{id}:layer-toggle-{}", control.id()))
                    .label(label)
                    .checked(expected != 0)
                    .disabled(disabled)
                    .on_change(cx.listener(move |view, value, window, cx| {
                        view.error = view
                            .edit_layer_toggle(
                                &selection,
                                &control,
                                (expected, i32::from(*value)),
                                window,
                                cx,
                            )
                            .err();
                        cx.notify();
                    })),
            );
        }
        let owner = cx.entity().downgrade();
        section = section.child(render_layer_modulation(
            &format!("{id}:modulation"),
            selection,
            disabled,
            move |target, action, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.error = view.edit_modulation(target, action, window, cx).err();
                    cx.notify();
                });
            },
        ));
        if let Some(draft) = &self.layer_inspector.custom {
            let owner = cx.entity().downgrade();
            section = section.child(render_custom_points(
                &format!("{id}:custom"),
                &selection.layer,
                draft,
                None,
                disabled,
                {
                    let draft = draft.clone();
                    let selection = selection.clone();
                    move |action, window, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            view.error = view
                                .edit_custom_points(&selection, &draft, action, window, cx)
                                .err();
                            cx.notify();
                        });
                    }
                },
            ));
        }
        section.into_any_element()
    }

    fn edit_layer_toggle(
        &mut self,
        selection: &LayerSelection,
        control: &CurveControlHandle<ForegroundError>,
        (expected, value): (i32, i32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.curve_edits_available() || expected == value {
            return Ok(());
        }
        if self.layer_inspector.is_changed(cx) {
            return Err("Apply or revert layer edits before changing a layer control.".into());
        }
        self.validate_layer_path(selection)?;
        selection.validate(Some(control))?;
        let edit = selection
            .parameter
            .animation_edit(Some(&selection.animation))?
            .ok_or("The Gradient animation is unavailable.")?;
        let next = selection.on_animation(&edit.owner)?;
        let control = selection.copy_control(control, &next)?;
        match control.kind() {
            CurveControlKind::Toggle if control.toggle().ok().map(i32::from) == Some(expected) => {
                control.set_toggle(value != 0)
            }
            CurveControlKind::Direction if control.enumeration().ok() == Some(expected) => {
                control.set_enumeration(value)
            }
            _ => return Err("The layer control changed. Try again.".into()),
        }
        .map_err(|error| error.to_string())?;
        self.submit(
            Edit::GradientCurve(GradientCurveEdit::Layers {
                edit,
                selection: Some(next),
            }),
            window,
            cx,
        );
        Ok(())
    }

    fn validate_layer_path(&self, selection: &LayerSelection) -> Result<(), String> {
        if !self
            .layer_inspector
            .layer
            .as_ref()
            .is_some_and(|current| current.same_path(selection))
        {
            return Err("The selected layer changed. Choose it again.".into());
        }
        selection.validate(None)
    }

    fn edit_modulation(
        &mut self,
        target: &ModulationTarget,
        action: ModulationAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.curve_edits_available() {
            return Ok(());
        }
        if self.layer_inspector.is_changed(cx) {
            return Err("Apply or revert layer edits before changing modulation.".into());
        }
        let selection = self
            .layer_inspector
            .layer
            .as_ref()
            .ok_or("Choose an animation layer first.")?;
        target.validate(selection)?;
        if let Some(selection) = target.navigate(&action)? {
            return self.set_layer_selection(Some(selection), window, cx);
        }
        let (edit, selection) = target.edit_copy(action)?;
        self.submit(
            Edit::GradientCurve(GradientCurveEdit::Layers {
                edit,
                selection: Some(selection),
            }),
            window,
            cx,
        );
        Ok(())
    }

    fn edit_custom_points(
        &mut self,
        selection: &LayerSelection,
        draft: &CustomDraft,
        action: CustomAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.curve_edits_available() {
            return Ok(());
        }
        if self.layer_inspector.fields_changed(cx) {
            return Err("Apply or revert layer edits before editing points.".into());
        }
        self.validate_layer_path(selection)?;
        draft.validate(&selection.layer)?;
        if matches!(action, CustomAction::Select(_)) {
            draft.apply(&selection.layer, action, false, cx)?;
            return self
                .layer_inspector
                .select_layer(selection.clone(), window, cx);
        }
        let edit = selection
            .parameter
            .animation_edit(Some(&selection.animation))?
            .ok_or("The Gradient animation is unavailable.")?;
        let next = selection.on_animation(&edit.owner)?;
        draft.apply(&next.layer, action, true, cx)?;
        self.submit(
            Edit::GradientCurve(GradientCurveEdit::Layers {
                edit,
                selection: Some(next),
            }),
            window,
            cx,
        );
        Ok(())
    }

    fn edit_curve_layers(
        &mut self,
        index: usize,
        expected: (
            &AnimationOwner,
            &SharedCurve<ForegroundError>,
            &[Option<Layer>],
        ),
        action: LayerAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.curve_edits_available() || self.curve_axis != Some(index) {
            return Ok(());
        }
        if self.layer_inspector.is_changed(cx) {
            return Err("Apply or revert layer edits before changing layers.".into());
        }
        let (owner, curve, layers) = expected;
        let (parameter, axis) = self.animation_parameter(index);
        if !parameter
            .retained()?
            .is_some_and(|current| current.same_reference(owner))
            || !parameter.axis(axis)?.0
            || !owner
                .curve(axis)?
                .is_some_and(|actual| actual.same_reference(curve))
            || !same_layers(&curve.layers(), layers)
        {
            return Err("The animation layers changed. Open the menu again.".into());
        }
        if let LayerAction::Edit(index) = action {
            let layer = layers
                .get(index)
                .and_then(Option::as_ref)
                .ok_or("The animation layer is unavailable.")?
                .clone();
            let selection = LayerSelection {
                parameter,
                animation: owner.clone(),
                axis,
                curve: curve.clone(),
                layer,
                ancestry: Vec::new(),
            };
            return self.set_layer_selection(Some(selection), window, cx);
        }
        let edit = parameter
            .animation_edit(Some(owner))?
            .ok_or("The Gradient animation is unavailable.")?;
        let selection = edit.copy_selection(self.layer_inspector.layer.as_ref())?;
        let working_curve = edit
            .owner
            .curve(axis)?
            .ok_or("The animation curve is unavailable.")?;
        apply_layer_action(&working_curve, action, |_| Ok(()))?;
        self.submit(
            Edit::GradientCurve(GradientCurveEdit::Layers { edit, selection }),
            window,
            cx,
        );
        Ok(())
    }
}
