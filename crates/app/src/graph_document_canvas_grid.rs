//! Canvas grid preferences and the guide painted over the editing tile.
use super::*;
use gpui_kit::component::{
    WindowExt,
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::InputState,
};
use gpui_kit::{App, Global, Role};
use mycela_engine::{CanvasGridKind, CanvasSnapGrid};
use std::{
    fs, io,
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct GridPreferences {
    kind: CanvasGridKind,
    density: [u16; 2],
    opacity: u8,
}

impl Default for GridPreferences {
    fn default() -> Self {
        Self {
            kind: CanvasGridKind::Regular,
            density: [50, 50],
            opacity: 0,
        }
    }
}

impl Global for GridPreferences {}

impl GridPreferences {
    pub(super) fn initialize(cx: &mut App) {
        if cx.try_global::<Self>().is_none() {
            let preferences = preferences_path()
                .and_then(|path| fs::read_to_string(path).ok())
                .and_then(|data| Self::decode(&data))
                .unwrap_or_default();
            cx.set_global(preferences);
        }
    }

    pub(super) fn current(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }

    pub(super) fn snap_grid(self) -> CanvasSnapGrid {
        CanvasSnapGrid::new(self.kind, self.density).expect("validated grid density")
    }

    fn decode(data: &str) -> Option<Self> {
        let mut fields = data.split_whitespace();
        let kind = match fields.next()? {
            "regular" => CanvasGridKind::Regular,
            "isometric" => CanvasGridKind::Isometric,
            _ => return None,
        };
        let density = [fields.next()?.parse().ok()?, fields.next()?.parse().ok()?];
        let opacity = fields.next()?.parse().ok()?;
        CanvasSnapGrid::new(kind, density)?;
        (opacity <= 100 && fields.next().is_none()).then_some(Self {
            kind,
            density,
            opacity,
        })
    }

    fn encode(self) -> String {
        let kind = match self.kind {
            CanvasGridKind::Regular => "regular",
            CanvasGridKind::Isometric => "isometric",
        };
        let [x, y] = self.density;
        format!("{kind} {x} {y} {}\n", self.opacity)
    }

    fn save(self) -> io::Result<()> {
        let Some(path) = preferences_path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT_WRITE.fetch_add(1, Ordering::Relaxed);
        let temporary = path.with_extension(format!("{}.{}.new", std::process::id(), sequence));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let result = file
            .write_all(self.encode().as_bytes())
            .and_then(|()| file.sync_all());
        drop(file);
        let result = result.and_then(|()| fs::rename(&temporary, path));
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

#[cfg(not(test))]
fn preferences_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("mycela").join("canvas-grid.conf"))
}

#[cfg(test)]
fn preferences_path() -> Option<PathBuf> {
    None
}

struct GridForm {
    kind: CanvasGridKind,
    density: [Entity<InputState>; 2],
    opacity: Entity<InputState>,
    error: Option<String>,
}

impl GridForm {
    fn values(&self, cx: &App) -> Result<GridPreferences, String> {
        let density = self
            .density
            .each_ref()
            .map(|input| input.read(cx).value().trim().parse::<u16>());
        let [Ok(x), Ok(y)] = density else {
            return Err("Enter whole numbers from 1 to 200 for both densities.".into());
        };
        if CanvasSnapGrid::new(self.kind, [x, y]).is_none() {
            return Err("Enter whole numbers from 1 to 200 for both densities.".into());
        }
        let opacity = self
            .opacity
            .read(cx)
            .value()
            .trim()
            .parse::<u8>()
            .ok()
            .filter(|value| *value <= 100)
            .ok_or("Enter a whole number from 0 to 100 for opacity.")?;
        Ok(GridPreferences {
            kind: self.kind,
            density: [x, y],
            opacity,
        })
    }
}

impl Render for GridForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_3()
            .child(
                div().h_flex().gap_2().children(
                    [
                        (
                            CanvasGridKind::Regular,
                            "texture-canvas-grid-regular",
                            "Regular",
                        ),
                        (
                            CanvasGridKind::Isometric,
                            "texture-canvas-grid-isometric",
                            "Isometric",
                        ),
                    ]
                    .map(|(kind, id, label)| {
                        Button::new(id)
                            .small()
                            .label(label)
                            .selected(self.kind == kind)
                            .on_click(cx.listener(move |form, _, _, cx| {
                                form.kind = kind;
                                cx.notify();
                            }))
                    }),
                ),
            )
            .child(
                Form::new()
                    .columns(2)
                    .child(
                        Field::new().label("X density").child(
                            Input::new(&self.density[0])
                                .id("texture-canvas-grid-x")
                                .aria_label("X density"),
                        ),
                    )
                    .child(
                        Field::new().label("Y density").child(
                            Input::new(&self.density[1])
                                .id("texture-canvas-grid-y")
                                .aria_label("Y density"),
                        ),
                    )
                    .child(
                        Field::new().label("Opacity (%)").child(
                            Input::new(&self.opacity)
                                .id("texture-canvas-grid-opacity")
                                .aria_label("Grid opacity"),
                        ),
                    ),
            )
            .when_some(self.error.clone(), |body, error| {
                body.child(
                    div()
                        .id("texture-canvas-grid-error")
                        .test_support()
                        .role(Role::Alert)
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
    }
}

impl GraphDocumentView {
    pub(super) fn open_canvas_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.drawing_current(cx) {
            return;
        }
        let prefs = GridPreferences::current(cx);
        let form = cx.new(|cx| GridForm {
            kind: prefs.kind,
            density: prefs.density.map(|value| {
                cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()))
            }),
            opacity: cx
                .new(|cx| InputState::new(window, cx).default_value(prefs.opacity.to_string())),
            error: None,
        });
        let owner = cx.weak_entity();
        self.modal = true;
        window.open_dialog(cx, move |dialog, _, _| {
            let inputs = form.clone();
            let owner = owner.clone();
            dialog
                .title("Canvas snap grid")
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Apply")),
                        ),
                )
                .on_ok(move |_, _, cx| {
                    let result = inputs.read(cx).values(cx).and_then(|preferences| {
                        preferences
                            .save()
                            .map_err(|error| format!("Could not save grid preferences. {error}"))?;
                        cx.set_global(preferences);
                        Ok(())
                    });
                    if let Err(error) = result {
                        inputs.update(cx, |form, cx| {
                            form.error = Some(error);
                            cx.notify();
                        });
                        return false;
                    }
                    true
                })
                .on_close(move |_, window, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.modal = false;
                        view.drawing_focus.focus(window, cx);
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }
}

pub(super) fn paint(
    space: TileSpace,
    preferences: GridPreferences,
    stroke: Pixels,
    color: Hsla,
    window: &mut Window,
) {
    if preferences.opacity == 0 {
        return;
    }
    let mut path = PathBuilder::stroke(stroke);
    let [x, y] = preferences.density.map(i32::from);
    let project = |column: i32, row: i32| {
        space.window_point([
            TILE * column as f32 / x as f32,
            TILE * row as f32 / y as f32,
        ])
    };
    let mut line = |[x1, y1]: [i32; 2], [x2, y2]: [i32; 2]| {
        path.move_to(project(x1, y1));
        path.line_to(project(x2, y2));
    };
    match preferences.kind {
        CanvasGridKind::Regular => {
            for column in 1..x {
                line([column, 0], [column, y]);
            }
            for row in 1..y {
                line([0, row], [x, row]);
            }
        }
        CanvasGridKind::Isometric => {
            for intercept in (1 - x)..y {
                let first = (-intercept).max(0);
                let last = (y - intercept).min(x);
                line([first, first + intercept], [last, last + intercept]);
                line([first, y - first - intercept], [last, y - last - intercept]);
            }
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color.opacity(f32::from(preferences.opacity) / 100.));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_grid_preferences_round_trip_and_validate() {
        let preferences = GridPreferences {
            kind: CanvasGridKind::Isometric,
            density: [3, 200],
            opacity: 45,
        };
        assert_eq!(
            GridPreferences::decode(&preferences.encode()),
            Some(preferences)
        );
        for invalid in [
            "regular 0 50 0",
            "regular 50 201 0",
            "regular 50 50 101",
            "isometric 5 4 40 extra",
        ] {
            assert!(GridPreferences::decode(invalid).is_none());
        }
    }
}
