//! A direction picker for the light and mixer fields of the selected layer.
use super::{GraphDocumentView, Target};
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
    form::{Field, Form},
    input::{Input, InputState},
};
use gpui_kit::{
    App, AppContext, Bounds, Context, Corners, DispatchPhase, Entity, IntoElement, MouseButton,
    MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point, Render, Subscription, Window, canvas,
    div, fill, point, prelude::*, px, rems, size,
};
use mycela_engine::{EnvironmentField, EnvironmentNumericReceiver, EnvironmentRecord};
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy)]
pub(super) enum Direction {
    Light,
    Mixer,
}

impl Direction {
    fn label(self) -> &'static str {
        match self {
            Self::Light => "Light direction",
            Self::Mixer => "Mixer direction",
        }
    }

    fn fields(self, layer: usize) -> [EnvironmentField; 2] {
        match self {
            Self::Light => [
                EnvironmentField::Rotation(layer),
                EnvironmentField::Elevation(layer),
            ],
            Self::Mixer => [
                EnvironmentField::MixerRotation(layer),
                EnvironmentField::MixerElevation(layer),
            ],
        }
    }
}

fn fields_linked(map: &EnvironmentRecord, fields: [EnvironmentField; 2]) -> bool {
    fields.into_iter().any(|field| {
        map.numeric_adjuster_key(EnvironmentNumericReceiver::Field(field))
            .is_some()
    })
}

struct Picker {
    inputs: [Entity<InputState>; 2],
    surface: Rc<Cell<Bounds<Pixels>>>,
    dragging: bool,
    error: Option<String>,
    _subscriptions: [Subscription; 2],
}

impl Picker {
    fn new(values: [f64; 2], window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inputs = values
            .map(|value| cx.new(|cx| InputState::new(window, cx).default_value(value.to_string())));
        let subscriptions = inputs
            .each_ref()
            .map(|input| cx.observe(input, |_, _, cx| cx.notify()));
        Self {
            inputs,
            surface: Rc::new(Cell::new(Bounds::default())),
            dragging: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn angles(&self, cx: &App) -> Result<[f64; 2], &'static str> {
        let [rotation, elevation] = self
            .inputs
            .each_ref()
            .map(|input| input.read(cx).value().trim().parse::<f64>().ok());
        let rotation = rotation
            .filter(|value| (0. ..=360.).contains(value))
            .ok_or("Enter a rotation from 0° to 360°.")?;
        let elevation = elevation
            .filter(|value| (0. ..=90.).contains(value))
            .ok_or("Enter an elevation from 0° to 90°.")?;
        Ok([rotation, elevation])
    }

    fn radius(bounds: Bounds<Pixels>, rem: Pixels) -> Pixels {
        bounds.size.width.min(bounds.size.height) / 2. - rem * 0.3125
    }

    fn set_position(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let surface = self.surface.get();
        let radius = Self::radius(surface, window.rem_size());
        if radius <= px(0.) {
            return;
        }
        let offset = position - surface.center();
        let x = f64::from(offset.x / radius);
        let y = f64::from(-offset.y / radius);
        let distance = x.hypot(y).min(1.);
        let rotation = if y == 0. && x >= 0. {
            0.
        } else {
            y.atan2(x).to_degrees().rem_euclid(360.).round_ties_even()
        };
        let elevation = distance.acos().to_degrees().round_ties_even();
        for (input, value) in self.inputs.iter().zip([rotation, elevation]) {
            let value = value.to_string();
            if input.read(cx).value().as_ref() != value {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
        }
        self.error = None;
        cx.notify();
    }
}

impl Render for Picker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let surface = self.surface.clone();
        let angles = self.angles(cx).ok();
        let rem = window.rem_size();
        let theme = cx.theme();
        let accent = theme.primary;
        let marker_border = theme.background;
        let mover = cx.weak_entity();
        let releaser = mover.clone();
        let guide = canvas(
            move |bounds, _, _| surface.set(bounds),
            move |bounds, (), window, _| {
                let mover = mover.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture
                        && event.pressed_button == Some(MouseButton::Left)
                    {
                        let _ = mover.update(cx, |picker, cx| {
                            if picker.dragging {
                                picker.set_position(event.position, window, cx);
                            }
                        });
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        let _ = releaser.update(cx, |picker, cx| {
                            if std::mem::take(&mut picker.dragging) {
                                picker.set_position(event.position, window, cx);
                            }
                        });
                    }
                });
                let Some([rotation, elevation]) = angles else {
                    return;
                };
                let radius = Self::radius(bounds, rem);
                if radius <= px(0.) {
                    return;
                }
                let distance = radius * elevation.to_radians().cos() as f32;
                let (sin, cos) = rotation.to_radians().sin_cos();
                let center = bounds.center();
                let position = center + point(distance * cos as f32, -distance * sin as f32);
                let mut line = PathBuilder::stroke(rem * 0.0625);
                line.move_to(center);
                line.line_to(position);
                if let Ok(path) = line.build() {
                    window.paint_path(path, accent);
                }
                let grip = rem * 0.3125;
                window.paint_quad(
                    fill(
                        Bounds::new(position - point(grip, grip), size(grip * 2., grip * 2.)),
                        accent,
                    )
                    .corner_radii(Corners::all(grip))
                    .border_widths(rem * 0.0625)
                    .border_color(marker_border),
                );
            },
        )
        .absolute()
        .inset_0()
        .size_full();
        let error = self
            .angles(cx)
            .err()
            .map(str::to_owned)
            .or_else(|| self.error.clone());
        div()
            .v_flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .id("environment-direction-surface")
                    .role(gpui_kit::Role::Group)
                    .aria_label("Direction. Center is overhead and edge is the horizon.")
                    .test_support()
                    .relative()
                    .w(rems(9.))
                    .h(rems(9.))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.muted)
                    .cursor_crosshair()
                    .child(guide)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|picker, event: &gpui_kit::MouseDownEvent, window, cx| {
                            picker.dragging = true;
                            picker.set_position(event.position, window, cx);
                        }),
                    ),
            )
            .child(
                Form::new()
                    .columns(2)
                    .child(
                        Field::new().label("Rotation").child(
                            Input::new(&self.inputs[0])
                                .id("environment-direction-rotation")
                                .aria_label("Rotation in degrees"),
                        ),
                    )
                    .child(
                        Field::new().label("Elevation").child(
                            Input::new(&self.inputs[1])
                                .id("environment-direction-elevation")
                                .aria_label("Elevation in degrees"),
                        ),
                    ),
            )
            .when_some(error, |element, error| {
                element.child(div().text_sm().text_color(theme.danger).child(error))
            })
    }
}

impl GraphDocumentView {
    pub(super) fn environment_direction_button(
        &self,
        target: &Target,
        direction: Direction,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<Button> {
        let fields = direction.fields(target.panel.selected()?);
        let map = target.panel.value();
        map.field(fields[0])?;
        map.field(fields[1])?;
        let id = match direction {
            Direction::Light => "graph-environment-light-direction",
            Direction::Mixer => "graph-environment-mixer-direction",
        };
        let target = target.clone();
        Some(
            Button::new(id)
                .small()
                .label(format!("{}…", direction.label()))
                .disabled(busy || !target.panel.is_editable() || fields_linked(map, fields))
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.open_environment_direction(&target, direction, window, cx);
                })),
        )
    }

    fn open_environment_direction(
        &mut self,
        target: &Target,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_edit_environment(target, cx) {
            return;
        }
        let Some(layer) = target.panel.selected() else {
            return;
        };
        let fields = direction.fields(layer);
        let map = target.panel.value();
        let [Some(rotation), Some(elevation)] = fields.map(|field| map.field(field)) else {
            return;
        };
        if fields_linked(map, fields) {
            return;
        }
        let picker = cx.new(|cx| Picker::new([rotation, elevation], window, cx));
        let owner = cx.weak_entity();
        let target = target.clone();
        self.pause_preview();
        self.modal = true;
        window.open_dialog(cx, move |dialog, _, _| {
            let apply = owner.clone();
            let close = owner.clone();
            let target = target.clone();
            let draft = picker.clone();
            dialog
                .title(direction.label())
                .child(picker.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").label("Apply").primary()),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let Ok(angles) = draft.read(cx).angles(cx) else {
                        return false;
                    };
                    let result = apply.update(cx, |view, cx| {
                        view.apply_environment_direction(&target, layer, fields, angles, window, cx)
                    });
                    match result {
                        Ok(Err(error)) => {
                            draft.update(cx, |picker, cx| {
                                picker.error = Some(error);
                                cx.notify();
                            });
                            false
                        }
                        _ => true,
                    }
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }

    fn apply_environment_direction(
        &mut self,
        target: &Target,
        layer: usize,
        fields: [EnvironmentField; 2],
        values: [f64; 2],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !target.current(self) {
            return Err("The layer changed. Reopen the direction picker.".into());
        }
        target.panel.validate()?;
        let mut map = target.panel.value().as_ref().clone();
        if fields_linked(&map, fields) {
            return Err("Edit the linked direction in Lab adjusters.".into());
        }
        let rotation_changed = map
            .set_field(fields[0], values[0])
            .map_err(|error| error.to_string())?;
        let elevation_changed = map
            .set_field(fields[1], values[1])
            .map_err(|error| error.to_string())?;
        if rotation_changed || elevation_changed {
            self.apply_environment_change(&target.panel, map, layer, window, cx);
            if let Some(error) = self.error.clone() {
                return Err(error);
            }
        }
        Ok(())
    }
}
