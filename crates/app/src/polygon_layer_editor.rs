//! Property drafts bound to the selected retained curve layer and its controls.
use super::*;
use crate::curve_modulation::{Control, layer_controls};
#[path = "polygon_custom_editor.rs"]
mod custom_editor;
#[path = "polygon_modulation.rs"]
mod modulation;
use custom_editor::CustomDraft;
use mycela_engine::CurveControlKind;
struct ScalarDraft {
    control: Control,
    label: String,
    field: Entity<InputState>,
    synced: String,
}
pub(crate) struct LayerEditor {
    pub(super) parameter: usize,
    pub(super) axis: i32,
    curve: Curve,
    layer: Layer,
    ancestry: Vec<(Layer, Control)>,
    scalars: Vec<ScalarDraft>,
    custom: Option<CustomDraft>,
}
impl LayerEditor {
    pub(super) fn new(
        parameter: usize,
        axis: i32,
        curve: Curve,
        layer: Layer,
        window: &mut Window,
        cx: &mut Context<PolygonSourceView>,
    ) -> Result<Self, String> {
        Self::with_ancestry(parameter, axis, curve, layer, Vec::new(), window, cx)
    }
    fn with_ancestry(
        parameter: usize,
        axis: i32,
        curve: Curve,
        layer: Layer,
        ancestry: Vec<(Layer, Control)>,
        window: &mut Window,
        cx: &mut Context<PolygonSourceView>,
    ) -> Result<Self, String> {
        let mut scalars = Vec::new();
        for control in layer_controls(&layer) {
            if !matches!(
                control.kind(),
                CurveControlKind::Scalar | CurveControlKind::Modulated
            ) {
                continue;
            }
            let value = control.scalar().map_err(|e| e.to_string())?.to_string();
            let label = control
                .name()
                .as_deref()
                .map(String::from_utf16_lossy)
                .unwrap_or_else(|| format!("Control {}", control.id()));
            let field = cx.new(|cx| InputState::new(window, cx).default_value(value.clone()));
            scalars.push(ScalarDraft {
                control,
                label,
                field,
                synced: value,
            });
        }
        let custom = CustomDraft::new(&layer, window, cx)?;
        Ok(Self {
            custom,
            parameter,
            axis,
            curve,
            layer,
            ancestry,
            scalars,
        })
    }
    pub(super) fn is_changed(&self, cx: &App) -> bool {
        self.scalars
            .iter()
            .any(|draft| draft.field.read(cx).value() != draft.synced)
            || self
                .custom
                .as_ref()
                .is_some_and(|draft| draft.is_changed(cx))
    }
    pub(super) fn is_layer(&self, curve: &Curve, layer: &Layer) -> bool {
        self.curve.same_reference(curve)
            && self
                .ancestry
                .first()
                .map_or(&self.layer, |(parent, _)| parent)
                .same_reference(layer)
    }
    fn valid(
        &self,
        node: &PointwiseNode<ForegroundError>,
        control: Option<&Control>,
    ) -> Result<(), String> {
        let filter = node.filter().ok_or("Polygon source is unavailable.")?;
        let parameter = filter
            .parameter(self.parameter as i32)
            .map_err(|e| e.to_string())?;
        let current = animation_axis(&parameter, self.axis)?
            .1
            .ok_or("The animation curve changed.")?;
        let root = self
            .ancestry
            .first()
            .map_or(&self.layer, |(parent, _)| parent);
        if !current.same_reference(&self.curve)
            || !current
                .layers()
                .iter()
                .flatten()
                .any(|v| v.same_reference(root))
        {
            return Err("The selected animation layer changed. Choose it again.".into());
        }
        for (index, (parent, control)) in self.ancestry.iter().enumerate() {
            let next = self
                .ancestry
                .get(index + 1)
                .map_or(&self.layer, |(layer, _)| layer);
            if !layer_controls(parent).any(|c| c.same_reference(control))
                || !control
                    .retained_child()
                    .is_some_and(|child| child.same_reference(next))
            {
                return Err("A modulation curve changed. Choose the layer again.".into());
            }
        }
        if let Some(control) = control
            && !layer_controls(&self.layer).any(|current| current.same_reference(control))
        {
            return Err("A layer control changed. Choose the layer again.".into());
        }
        Ok(())
    }
}
impl PolygonSourceView {
    pub(crate) fn sync_layer_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.layer_editor else {
            return;
        };
        if let Err(error) = editor.valid(&self.node, None) {
            self.error = Some(error);
            return;
        }
        let available: Vec<_> = layer_controls(&editor.layer).collect();
        if let Some(custom) = &mut editor.custom {
            if let Err(error) = custom.sync(&editor.layer, window, cx, false) {
                self.error = Some(error);
            }
        }
        for draft in &mut editor.scalars {
            if !available
                .iter()
                .any(|current| current.same_reference(&draft.control))
            {
                self.error = Some("A layer control changed. Choose the layer again.".into());
                continue;
            }
            if draft.field.read(cx).value() == draft.synced {
                if let Ok(value) = draft.control.scalar() {
                    let value = value.to_string();
                    if value != draft.synced {
                        draft
                            .field
                            .update(cx, |field, cx| field.set_value(value.clone(), window, cx));
                        draft.synced = value;
                    }
                }
            }
        }
    }
    fn apply_layer_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.presentation.is_none() {
            return;
        }
        let result = (|| {
            let editor = self
                .layer_editor
                .as_ref()
                .ok_or("Choose an animation layer first.")?;
            let mut changes = Vec::new();
            for draft in &editor.scalars {
                let text = draft.field.read(cx).value();
                if text == draft.synced {
                    continue;
                }
                editor.valid(&self.node, Some(&draft.control))?;
                if draft.control.kind() == CurveControlKind::Modulated && draft.control.is_active()
                {
                    return Err(format!("{} is controlled by another curve.", draft.label));
                }
                let value = text
                    .parse::<f32>()
                    .map_err(|_| format!("{} needs a number.", draft.label))?;
                if !value.is_finite() {
                    return Err(format!("{} needs a finite number.", draft.label));
                }
                if let Some([min, max]) = draft.control.bounds()
                    && !(min..=max).contains(&value)
                {
                    return Err(format!("{} must be between {min} and {max}.", draft.label));
                }
                changes.push((draft.control.clone(), value));
            }
            for (control, value) in changes {
                // A control callback can reenter and replace the layer or a sibling control.
                editor.valid(&self.node, Some(&control))?;
                control.set_scalar(value).map_err(|e| e.to_string())?;
            }
            Ok::<_, String>(())
        })();
        self.error = result.err();
        if self.error.is_none() {
            if let Some(editor) = &mut self.layer_editor {
                for draft in &mut editor.scalars {
                    if let Ok(value) = draft.control.scalar() {
                        draft.synced = value.to_string();
                        draft.field.update(cx, |field, cx| {
                            field.set_value(draft.synced.clone(), window, cx)
                        });
                    }
                }
            }
        }
        self.notify_animation();
        cx.notify();
    }
    fn revert_layer_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &mut self.layer_editor {
            for draft in &mut editor.scalars {
                draft.synced = draft.field.read(cx).value().to_string();
            }
            if let Some(custom) = &mut editor.custom {
                custom.discard_draft(cx);
            }
        }
        self.error = None;
        self.sync_layer_editor(window, cx);
        cx.notify();
    }
    fn toggle_layer_control(
        &mut self,
        control: &Control,
        expected_layer: &Layer,
        expected: i32,
        value: i32,
        cx: &mut Context<Self>,
    ) {
        if self.presentation.is_none() {
            return;
        }
        let result = (|| {
            let editor = self
                .layer_editor
                .as_ref()
                .ok_or("Choose an animation layer first.")?;
            if !editor.layer.same_reference(expected_layer) {
                return Err("The selected animation layer changed. Try again.".into());
            }
            editor.valid(&self.node, Some(control))?;
            match control.kind() {
                CurveControlKind::Toggle => {
                    if i32::from(control.toggle().map_err(|e| e.to_string())?) != expected {
                        return Err("The layer control changed. Try again.".into());
                    }
                    control.set_toggle(value != 0).map_err(|e| e.to_string())?;
                }
                CurveControlKind::Direction => {
                    if control.enumeration().map_err(|e| e.to_string())? != expected {
                        return Err("The layer control changed. Try again.".into());
                    }
                    control.set_enumeration(value).map_err(|e| e.to_string())?;
                }
                _ => return Err("The layer control is unavailable.".into()),
            }
            Ok::<_, String>(())
        })();
        self.error = result.err();
        self.notify_animation();
        cx.notify();
    }
    pub(super) fn render_layer_editor(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(editor) = &self.layer_editor else {
            return div().into_any_element();
        };
        let validity = editor.valid(&self.node, None);
        let disabled = self.presentation.is_none() || validity.is_err();
        let activation = self.presentation.as_ref().map(|v| Arc::downgrade(&v.0));
        let mut body = div().v_flex().gap_2().child(
            div()
                .font_semibold()
                .child(format!("{:?} layer", editor.layer.kind())),
        );
        if let Err(error) = validity {
            body = body.child(
                div()
                    .id("polygon-layer-stale")
                    .role(gpui_kit::Role::Alert)
                    .aria_label(error.clone())
                    .test_support()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error),
            );
        }
        let mut form = Form::new().columns(2);
        for draft in &editor.scalars {
            let controlled =
                draft.control.kind() == CurveControlKind::Modulated && draft.control.is_active();
            form = form.child(
                Field::new()
                    .label(if controlled {
                        format!("{} (driven by curve)", draft.label)
                    } else {
                        draft.label.clone()
                    })
                    .child(
                        Input::new(&draft.field)
                            .id(format!("polygon-layer-control-{}", draft.control.id()))
                            .aria_label(draft.label.clone())
                            .disabled(disabled || controlled),
                    ),
            );
        }
        body = body
            .child(form)
            .child(self.render_modulation_controls(editor, disabled, cx));
        if let Some(custom) = &editor.custom {
            body = body.child(self.render_custom_editor(custom, disabled, cx));
        }
        for control in layer_controls(&editor.layer) {
            let (label, selected) = match control.kind() {
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
            let Some(expected) = selected else {
                continue;
            };
            let id = control.id();
            let activation = activation.clone();
            let expected_layer = editor.layer.clone();
            body = body.child(
                Checkbox::new(format!("polygon-layer-toggle-{id}"))
                    .label(label)
                    .checked(expected != 0)
                    .disabled(disabled)
                    .on_change(cx.listener(move |view, checked, _, cx| {
                        if !activation
                            .as_ref()
                            .and_then(Weak::upgrade)
                            .is_some_and(|v| v.load(Ordering::Acquire))
                        {
                            return;
                        }
                        view.toggle_layer_control(
                            &control,
                            &expected_layer,
                            expected,
                            i32::from(*checked),
                            cx,
                        );
                    })),
            );
        }
        body.child(
            div()
                .h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("apply-polygon-layer")
                        .label("Apply layer")
                        .disabled(disabled)
                        .on_click(
                            cx.listener(|view, _, window, cx| {
                                view.apply_layer_controls(window, cx)
                            }),
                        ),
                )
                .child(
                    Button::new("revert-polygon-layer")
                        .label("Revert layer edits")
                        .disabled(self.presentation.is_none())
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.revert_layer_controls(window, cx)
                        })),
                )
                .child(
                    Button::new("close-polygon-layer")
                        .label("Close layer")
                        .disabled(self.presentation.is_none() || editor.is_changed(cx))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.layer_editor = None;
                            cx.notify();
                        })),
                ),
        )
        .into_any_element()
    }
}
