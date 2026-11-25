//! Bind shared modulation commands to the Polygon source's retained layer.
use super::*;
use crate::curve_modulation::{
    ModulationAction, ModulationTarget, open_child_path, render_modulation_menu,
};

impl PolygonSourceView {
    fn modulation_action(
        &mut self,
        expected: &ModulationTarget,
        action: ModulationAction,
        window: &mut Window,
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
            if editor.is_changed(cx) {
                return Err("Apply or revert layer edits before changing modulation.".into());
            }
            let control = expected.control();
            editor.valid(&self.node, Some(control))?;
            let child = expected.apply(&editor.layer, action, || {
                editor.valid(&self.node, Some(control))
            })?;
            if let Some(child) = child {
                let ancestry = open_child_path(&editor.ancestry, &editor.layer, control, &child)?;
                self.layer_editor = Some(LayerEditor::with_ancestry(
                    editor.parameter,
                    editor.axis,
                    editor.curve.clone(),
                    child,
                    ancestry,
                    window,
                    cx,
                )?);
            }
            Ok::<_, String>(())
        })();
        self.error = result.err();
        self.notify_animation();
        cx.notify();
    }

    fn modulation_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.presentation.is_none() {
            return;
        }
        let result = (|| {
            let editor = self
                .layer_editor
                .as_ref()
                .ok_or("Choose an animation layer first.")?;
            if editor.is_changed(cx) {
                return Err("Apply or revert layer edits before returning.".into());
            }
            editor.valid(&self.node, None)?;
            let mut ancestry = editor.ancestry.clone();
            let (parent, _) = ancestry.pop().ok_or("The parent layer is unavailable.")?;
            self.layer_editor = Some(LayerEditor::with_ancestry(
                editor.parameter,
                editor.axis,
                editor.curve.clone(),
                parent,
                ancestry,
                window,
                cx,
            )?);
            Ok::<_, String>(())
        })();
        self.error = result.err();
        cx.notify();
    }

    pub(super) fn render_modulation_controls(
        &self,
        editor: &LayerEditor,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let marker = self.presentation.as_ref().map(|v| Arc::downgrade(&v.0));
        let owner = cx.weak_entity();
        div()
            .v_flex()
            .gap_2()
            .when(!editor.ancestry.is_empty(), |content| {
                content.child(
                    Button::new("polygon-modulation-back")
                        .label("Back to parent layer")
                        .disabled(disabled)
                        .on_click(
                            cx.listener(|view, _, window, cx| view.modulation_back(window, cx)),
                        ),
                )
            })
            .children(
                layer_controls(&editor.layer)
                    .filter(|control| control.kind() == CurveControlKind::Modulated)
                    .map(|control| {
                        let owner = owner.clone();
                        let marker = marker.clone();
                        render_modulation_menu(
                            "polygon-modulation",
                            &editor.layer,
                            &control,
                            disabled,
                            true,
                            move |target, action, window, cx| {
                                if marker
                                    .as_ref()
                                    .and_then(Weak::upgrade)
                                    .is_some_and(|v| v.load(Ordering::Acquire))
                                {
                                    let _ = owner.update(cx, |view, cx| {
                                        view.modulation_action(target, action, window, cx)
                                    });
                                }
                            },
                        )
                    }),
            )
            .into_any_element()
    }
}
