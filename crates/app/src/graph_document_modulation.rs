//! Nested modulation navigation over the graph inspector's retained layer path.
use super::super::inspector::LayerSelection;
use super::{GraphDocumentView, animation::AnimationTarget};
use crate::ForegroundError;
use gpui_kit::base::{Disableable, StyledExt};
use gpui_kit::component::{
    button::Button,
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{App, Context, IntoElement, Window, div, prelude::*};
use mycela_engine::{AnalyticCurveKind, CurveControlHandle, CurveControlKind, CurveLayerHandle};

type Layer = CurveLayerHandle<ForegroundError>;
#[derive(Clone)]
pub(crate) enum ModulationAction {
    Constant,
    Existing,
    New(AnalyticCurveKind),
    Noise,
    Custom,
    Open,
    Back,
}
#[derive(Clone)]
pub(crate) struct ModulationTarget {
    selection: LayerSelection,
    control: Option<CurveControlHandle<ForegroundError>>,
    active: bool,
    child: Option<Layer>,
    mode: Option<i32>,
}
fn same_child(a: Option<&Layer>, b: Option<&Layer>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_reference(b),
        _ => false,
    }
}
impl GraphDocumentView {
    fn graph_modulation_action(
        &mut self,
        target: &AnimationTarget,
        expected: &ModulationTarget,
        action: ModulationAction,
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
            self.error = Some("Correct or reset field text before changing modulation.".into());
            cx.notify();
            return;
        }
        let valid = (|| -> Result<(), String> {
            let current = self
                .inspector
                .layer
                .as_ref()
                .ok_or("Choose an animation layer first.")?;
            expected.validate(current)
        })();
        if let Err(error) = valid {
            self.error = Some(error);
            cx.notify();
            return;
        }
        if matches!(action, ModulationAction::Open | ModulationAction::Back) {
            self.error = expected
                .navigate(&action)
                .and_then(|next| {
                    self.inspector
                        .select_layer(next.expect("navigation action"), window, cx)
                })
                .err();
            self.observe_drafts(cx);
            cx.notify();
            return;
        }
        let Some(control) = expected.control.as_ref() else {
            return;
        };
        if expected.selection.parameter.records_curve_history() {
            self.ensure_history_binding();
        }
        let stamp = self.begin_history_edit();
        self.revision += 1;
        let revision = self.revision;
        self.status = None;
        let result = (|| -> Result<(), String> {
            let edit = expected
                .selection
                .parameter
                .animation_edit(Some(&expected.selection.animation))?;
            let copy = edit
                .as_ref()
                .map(|edit| expected.selection.on_animation(&edit.owner))
                .transpose()?;
            let control = match &copy {
                Some(copy) => expected.selection.copy_control(control, copy)?,
                None => control.clone(),
            };
            apply_modulation(&control, action, |child| {
                if edit.is_none() {
                    expected.selection.validate(Some(&control))?;
                }
                if !self.is_original_selection(&target.selection, &target.node, revision)
                    || control.modulation_mode() != expected.mode
                    || !control
                        .retained_child()
                        .is_some_and(|current| current.same_reference(child))
                {
                    return Err("The modulation changed while applying the curve. Earlier changes were kept.".into());
                }
                Ok(())
            })?;
            if let Some(edit) = edit {
                edit.publish(Some(&self.document))?;
                self.inspector.layer = copy;
            } else {
                expected.selection.validate(Some(&control))?;
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
    pub(super) fn render_graph_modulation(
        &self,
        selection: &LayerSelection,
        target: &AnimationTarget,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let owner = cx.entity().downgrade();
        let target = target.clone();
        render_layer_modulation(
            "graph-modulation",
            selection,
            disabled,
            move |expected, action, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.graph_modulation_action(&target, expected, action, window, cx)
                });
            },
        )
    }
}

impl ModulationTarget {
    pub(crate) fn validate(&self, current: &LayerSelection) -> Result<(), String> {
        if !current.same_path(&self.selection) {
            return Err("The selected layer changed. Open the menu again.".into());
        }
        current.validate(self.control.as_ref())?;
        if let Some(control) = &self.control
            && (control.kind() != CurveControlKind::Modulated
                || control.is_active() != self.active
                || control.modulation_mode() != self.mode
                || !same_child(control.retained_child().as_ref(), self.child.as_ref()))
        {
            return Err("The modulation changed. Open the menu again.".into());
        }
        Ok(())
    }

    pub(crate) fn navigate(
        &self,
        action: &ModulationAction,
    ) -> Result<Option<LayerSelection>, String> {
        let mut next = self.selection.clone();
        match action {
            ModulationAction::Open => {
                let control = self
                    .control
                    .as_ref()
                    .ok_or("The modulation control is unavailable.")?;
                let child = control
                    .retained_child()
                    .ok_or("The modulation curve is unavailable.")?;
                if child.same_reference(&next.layer)
                    || next
                        .ancestry
                        .iter()
                        .any(|(parent, _)| parent.same_reference(&child))
                {
                    return Err(
                        "This modulation contains a cycle and cannot be opened here.".into(),
                    );
                }
                next.ancestry.push((next.layer.clone(), control.clone()));
                next.layer = child;
            }
            ModulationAction::Back => {
                let (parent, _) = next
                    .ancestry
                    .pop()
                    .ok_or("The parent layer is unavailable.")?;
                next.layer = parent;
            }
            _ => return Ok(None),
        }
        Ok(Some(next))
    }

    pub(crate) fn edit_copy(
        &self,
        action: ModulationAction,
    ) -> Result<(super::super::inspector::AnimationEdit, LayerSelection), String> {
        let original = self
            .control
            .as_ref()
            .ok_or("The modulation control is unavailable.")?;
        let edit = self
            .selection
            .parameter
            .animation_edit(Some(&self.selection.animation))?
            .ok_or("The Gradient animation is unavailable.")?;
        let selection = self.selection.on_animation(&edit.owner)?;
        let control = self.selection.copy_control(original, &selection)?;
        apply_modulation(&control, action, |_| Ok(()))?;
        Ok((edit, selection))
    }
}

fn apply_modulation(
    control: &CurveControlHandle<ForegroundError>,
    action: ModulationAction,
    after_install: impl FnOnce(&Layer) -> Result<(), String>,
) -> Result<(), String> {
    match action {
        ModulationAction::Constant => control.set_active(false),
        ModulationAction::Existing => {
            if control.retained_child().is_none() {
                return Err("The modulation curve is unavailable.".into());
            }
            control.set_active(true);
        }
        ModulationAction::New(_) | ModulationAction::Noise | ModulationAction::Custom => {
            let child = match action {
                ModulationAction::New(kind) => Layer::new(kind),
                ModulationAction::Noise => Layer::new_noise().map_err(|e| e.to_string())?,
                ModulationAction::Custom => Layer::new_custom(),
                _ => unreachable!(),
            };
            control.set_child(Some(&child)).map_err(|e| e.to_string())?;
            after_install(&child)?;
            control.set_active(true);
        }
        _ => unreachable!("navigation was handled before editing"),
    }
    Ok(())
}

pub(crate) fn render_layer_modulation(
    id: &str,
    selection: &LayerSelection,
    disabled: bool,
    on_action: impl Fn(&ModulationTarget, ModulationAction, &mut Window, &mut App) + Clone + 'static,
) -> gpui_kit::AnyElement {
    let mut content = div().v_flex().gap_2();
    if !selection.ancestry.is_empty() {
        let expected = ModulationTarget {
            selection: selection.clone(),
            control: None,
            active: false,
            child: None,
            mode: None,
        };
        content = content.child(
            Button::new(format!("{id}-back"))
                .label("Back to parent layer")
                .disabled(disabled)
                .on_click({
                    let on_action = on_action.clone();
                    move |_, window, cx| {
                        on_action(&expected, ModulationAction::Back, window, cx);
                    }
                }),
        );
    }
    for control in selection
        .controls()
        .into_iter()
        .filter(|c| c.kind() == CurveControlKind::Modulated)
    {
        let name = control
            .name()
            .as_deref()
            .map(String::from_utf16_lossy)
            .unwrap_or_else(|| format!("Control {}", control.id()));
        let id = format!("{id}-{}", control.id());
        let expected = ModulationTarget {
            selection: selection.clone(),
            active: control.is_active(),
            child: control.retained_child(),
            mode: control.modulation_mode(),
            control: Some(control),
        };
        let label = format!(
            "{name}: {}",
            if expected.active { "curve" } else { "constant" }
        );
        let callback = |action: ModulationAction| {
            let on_action = on_action.clone();
            let expected = expected.clone();
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                on_action(&expected, action.clone(), window, cx);
            }
        };
        let mut entries = vec![
            ("Use constant", callback(ModulationAction::Constant)),
            ("Use existing curve", callback(ModulationAction::Existing)),
            ("Edit curve", callback(ModulationAction::Open)),
        ];
        entries.extend(
            [
                ("New Sine", ModulationAction::New(AnalyticCurveKind::Sine)),
                (
                    "New Linear",
                    ModulationAction::New(AnalyticCurveKind::Linear),
                ),
                ("New Arc", ModulationAction::New(AnalyticCurveKind::Arc)),
                ("New Bell", ModulationAction::New(AnalyticCurveKind::Bell)),
                ("New Gain", ModulationAction::New(AnalyticCurveKind::Gain)),
                ("New Steps", ModulationAction::New(AnalyticCurveKind::Steps)),
                ("New Noise", ModulationAction::Noise),
                ("New Custom", ModulationAction::Custom),
            ]
            .into_iter()
            .map(|(name, action)| (name, callback(action))),
        );
        let has_child = expected.child.is_some();
        content = content.child(
            Button::new(id)
                .label(label)
                .disabled(disabled)
                .dropdown_menu(move |mut menu, _, _| {
                    for (index, (name, cb)) in entries.iter().enumerate() {
                        if !has_child && (index == 1 || index == 2) {
                            continue;
                        }
                        menu = menu.item(PopupMenuItem::new(*name).on_click(cb.clone()));
                    }
                    menu.scrollable(true)
                }),
        );
    }
    content.into_any_element()
}
