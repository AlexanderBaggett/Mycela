//! Connect Custom point edits to the Polygon source's retained layer.
use super::*;
use crate::custom_curve_editor::CustomAction;
pub(super) use crate::custom_curve_editor::CustomDraft;

impl PolygonSourceView {
    fn custom_action(
        &mut self,
        expected: &Layer,
        action: CustomAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.presentation.is_none() {
            return;
        }
        let result = (|| {
            let editor = self
                .layer_editor
                .as_mut()
                .ok_or("Choose an animation layer first.")?;
            editor.valid(&self.node, None)?;
            if !editor.layer.same_reference(expected) {
                return Err("The selected animation layer changed. Choose it again.".into());
            }
            editor
                .custom
                .as_mut()
                .ok_or("This layer has no custom points.")?
                .apply(&editor.layer, action, window, cx)
        })();
        self.error = result.err();
        self.notify_animation();
        cx.notify();
    }

    pub(super) fn render_custom_editor(
        &self,
        draft: &CustomDraft,
        disabled: bool,
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        let Some(editor) = &self.layer_editor else {
            return div().into_any_element();
        };
        let owner = cx.weak_entity();
        let marker = self.presentation.as_ref().map(|v| Arc::downgrade(&v.0));
        let layer = editor.layer.clone();
        draft.render(
            "polygon-custom",
            &editor.layer,
            disabled,
            move |action, window, cx| {
                if marker
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .is_some_and(|v| v.load(Ordering::Acquire))
                {
                    let _ = owner.update(cx, |view, cx| {
                        view.custom_action(&layer, action, window, cx)
                    });
                }
            },
        )
    }
}
