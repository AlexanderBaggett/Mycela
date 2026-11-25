//! Menu commands for a curve driving a layer control.
use crate::{ForegroundError, curve_layer_menu::Layer};
use gpui_kit::base::Disableable;
use gpui_kit::component::{
    button::Button,
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{AnyElement, App, IntoElement, Window};
use mycela_engine::{AnalyticCurveKind, CurveControlHandle, CurveControlKind};
use std::rc::Rc;

pub(crate) type Control = CurveControlHandle<ForegroundError>;

#[derive(Clone)]
pub(crate) enum ModulationAction {
    Constant,
    Existing,
    New(AnalyticCurveKind),
    Noise,
    Custom,
    Open,
}

#[derive(Clone)]
pub(crate) struct ModulationTarget {
    layer: Layer,
    control: Control,
    active: bool,
    child: Option<Layer>,
    mode: Option<i32>,
}

impl ModulationTarget {
    fn new(layer: &Layer, control: &Control) -> Self {
        Self {
            layer: layer.clone(),
            control: control.clone(),
            active: control.is_active(),
            child: control.retained_child(),
            mode: control.modulation_mode(),
        }
    }

    pub(crate) fn control(&self) -> &Control {
        &self.control
    }

    pub(crate) fn apply(
        &self,
        layer: &Layer,
        action: ModulationAction,
        verify_after_attach: impl FnOnce() -> Result<(), String>,
    ) -> Result<Option<Layer>, String> {
        if !layer.same_reference(&self.layer) {
            return Err("The selected layer changed. Open the menu again.".into());
        }
        let control = &self.control;
        if !layer_controls(layer).any(|current| current.same_reference(control))
            || control.kind() != CurveControlKind::Modulated
            || control.is_active() != self.active
            || control.modulation_mode() != self.mode
            || !same_child(control.retained_child().as_ref(), self.child.as_ref())
        {
            return Err("The modulation changed. Open the menu again.".into());
        }
        match action {
            ModulationAction::Constant => control.set_active(false),
            ModulationAction::Existing => {
                if self.child.is_none() {
                    return Err("The modulation curve is unavailable.".into());
                }
                control.set_active(true);
            }
            ModulationAction::Open => {
                return self
                    .child
                    .clone()
                    .map(Some)
                    .ok_or_else(|| "The modulation curve is unavailable.".into());
            }
            ModulationAction::New(_) | ModulationAction::Noise | ModulationAction::Custom => {
                let child = match action {
                    ModulationAction::New(kind) => Layer::new(kind),
                    ModulationAction::Noise => Layer::new_noise().map_err(|e| e.to_string())?,
                    ModulationAction::Custom => Layer::new_custom(),
                    _ => unreachable!(),
                };
                control.set_child(Some(&child)).map_err(|e| e.to_string())?;
                verify_after_attach()?;
                if control.modulation_mode() != self.mode
                    || !control
                        .retained_child()
                        .is_some_and(|current| current.same_reference(&child))
                {
                    return Err("The modulation changed while applying the curve.".into());
                }
                control.set_active(true);
            }
        }
        Ok(None)
    }
}

pub(crate) fn layer_controls(layer: &Layer) -> impl Iterator<Item = Control> {
    layer
        .controls()
        .into_iter()
        .flatten()
        .chain(layer.shape_controls())
}

pub(crate) fn control_label(control: &Control) -> String {
    control
        .name()
        .as_deref()
        .map(String::from_utf16_lossy)
        .unwrap_or_else(|| format!("Control {}", control.id()))
}

pub(crate) fn is_remap(control: &Control) -> bool {
    control.kind() == CurveControlKind::Modulated && matches!(control.id(), 9 | 10)
}

fn same_child(a: Option<&Layer>, b: Option<&Layer>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_reference(b),
        _ => false,
    }
}

pub(crate) fn open_child_path(
    ancestry: &[(Layer, Control)],
    parent: &Layer,
    control: &Control,
    child: &Layer,
) -> Result<Vec<(Layer, Control)>, String> {
    if child.same_reference(parent)
        || ancestry
            .iter()
            .any(|(ancestor, _)| ancestor.same_reference(child))
    {
        return Err("This modulation contains a cycle and cannot be opened here.".into());
    }
    let mut path = ancestry.to_vec();
    path.push((parent.clone(), control.clone()));
    Ok(path)
}

pub(crate) fn render_modulation_menu(
    prefix: &str,
    layer: &Layer,
    control: &Control,
    disabled: bool,
    edit_dormant: bool,
    on_action: impl Fn(&ModulationTarget, ModulationAction, &mut Window, &mut App) + 'static,
) -> AnyElement {
    use ModulationAction as Action;
    let target = ModulationTarget::new(layer, control);
    let remap = is_remap(control);
    let status = if target.active {
        "Curve"
    } else if remap {
        "Off"
    } else {
        "Constant"
    };
    let label = format!("{} · {status}", control_label(control));
    let has_child = target.child.is_some();
    let on_action = Rc::new(on_action);
    Button::new(format!("{prefix}-{}", control.id()))
        .label(label)
        .disabled(disabled)
        .dropdown_menu(move |mut menu, _, _| {
            let mut entries = vec![(
                if remap {
                    "Disable remap"
                } else {
                    "Use constant"
                },
                Action::Constant,
            )];
            if has_child {
                entries.push(("Use existing curve", Action::Existing));
                if target.active || edit_dormant {
                    entries.push(("Edit curve", Action::Open));
                }
            } else {
                entries.push(("Use default curve", Action::New(AnalyticCurveKind::Linear)));
            }
            entries.extend([
                ("New Sine", Action::New(AnalyticCurveKind::Sine)),
                ("New Linear", Action::New(AnalyticCurveKind::Linear)),
                ("New Arc", Action::New(AnalyticCurveKind::Arc)),
                ("New Bell", Action::New(AnalyticCurveKind::Bell)),
                ("New Gain", Action::New(AnalyticCurveKind::Gain)),
                ("New Steps", Action::New(AnalyticCurveKind::Steps)),
                ("New Noise", Action::Noise),
                ("New Custom", Action::Custom),
            ]);
            for (label, action) in entries {
                let target = target.clone();
                // The popup owns the callback independently of its trigger render.
                let callback = on_action.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                    callback(&target, action.clone(), window, cx);
                }));
            }
            menu.scrollable(true)
        })
        .into_any_element()
}
