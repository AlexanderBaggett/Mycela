//! The Gradient and Noise paint ramps share the graph's retained Gradient operations.
use super::super::gradient::GradientEditor;
use super::*;
use mycela_engine::{GradientEntry, GradientRecord, SharedGradient};

#[derive(Clone, Copy)]
pub(in crate::graph_document::view) enum Action {
    Select(usize),
    AddColor,
    AddOpacity,
    Remove(usize),
}

pub(in crate::graph_document::view) struct Draft {
    editor: GradientEditor,
    selected: Option<usize>,
    fields: Vec<(&'static str, String, Entity<InputState>)>,
}

impl Draft {
    pub(super) fn new(
        record: GradientRecord,
        window: &mut Window,
        cx: &mut Context<GraphDocumentView>,
    ) -> Result<Self, String> {
        Self::from_gradient(SharedGradient::from_wire_record(record), window, cx)
    }

    pub(in crate::graph_document::view) fn from_gradient(
        gradient: SharedGradient,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self, String> {
        let editor = GradientEditor::new_draft(gradient, window, cx)?;
        let mut draft = Self {
            editor,
            selected: None,
            fields: Vec::new(),
        };
        draft.select(Some(0), window, cx)?;
        Ok(draft)
    }

    pub(in crate::graph_document::view) fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub(in crate::graph_document::view) fn select(
        &mut self,
        selected: Option<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        let selected = self
            .editor
            .entries
            .len()
            .checked_sub(1)
            .map(|last| selected.unwrap_or(0).min(last));
        let fields = if let Some(entry) = selected.and_then(|index| self.editor.entries.get(index))
        {
            let labels: &[&str] = match entry {
                GradientEntry::Color(_) => &["Position", "Red", "Green", "Blue"],
                GradientEntry::Alpha(_) => &["Position", "Opacity"],
                _ => return Err("The gradient stop is unavailable.".into()),
            };
            labels
                .iter()
                .enumerate()
                .map(|(component, &label)| {
                    let value = self.editor.stop_value(entry, component)?;
                    let input =
                        cx.new(|cx| InputState::new(window, cx).default_value(value.clone()));
                    Ok((label, value, input))
                })
                .collect::<Result<Vec<_>, String>>()?
        } else {
            Vec::new()
        };
        self.selected = selected;
        self.fields = fields;
        Ok(())
    }

    pub(in crate::graph_document::view) fn inputs(
        &self,
    ) -> impl Iterator<Item = Entity<InputState>> + '_ {
        std::iter::once(self.editor.position.clone())
            .chain(self.fields.iter().map(|(_, _, input)| input.clone()))
    }

    pub(super) fn pending(&self, cx: &App) -> bool {
        self.stop_pending(cx) || self.editor.is_changed(cx)
    }

    pub(in crate::graph_document::view) fn stop_pending(&self, cx: &App) -> bool {
        self.fields
            .iter()
            .any(|(_, initial, input)| !same_value(input.read(cx).value().trim(), initial))
    }

    pub(in crate::graph_document::view) fn staged(
        &self,
        cx: &App,
    ) -> Result<Option<SharedGradient>, String> {
        if self.editor.is_changed(cx) {
            return Err("Add a stop or revert the new position before applying fields.".into());
        }
        if !self.stop_pending(cx) {
            return Ok(None);
        }
        let gradient = self.editor.copy_for_edit()?;
        let entry = self
            .selected
            .and_then(|index| self.editor.entries.get(index))
            .ok_or("The gradient stop is unavailable.")?;
        let stop = self.editor.copy_stop(entry, &gradient)?;
        for (component, (label, initial, input)) in self.fields.iter().enumerate() {
            let value = input.read(cx).value();
            let value = value.trim();
            if same_value(value, initial) {
                continue;
            }
            if component == 0 {
                position(value, self.editor.width())?;
            } else if !value
                .parse::<f64>()
                .is_ok_and(|value| (0. ..=1.).contains(&value))
            {
                return Err(format!("Enter a number between 0 and 1 for {label}."));
            }
            GradientEditor::write_stop(&stop, component, value)?;
        }
        Ok(Some(gradient))
    }

    pub(in crate::graph_document::view) fn change(
        &self,
        action: Action,
        cx: &App,
    ) -> Result<(SharedGradient, Option<usize>), String> {
        let gradient = self.editor.copy_for_edit()?;
        let selected = match action {
            Action::AddColor | Action::AddOpacity => {
                let width = self.editor.width();
                if !(1..=65536).contains(&width) {
                    return Err("The gradient width is too large to sample a new stop.".into());
                }
                let text = self.editor.position.read(cx).value();
                let position = position(text.trim(), width)?;
                // Initialize the canvas even when this is the first opacity stop.
                gradient.rebuild().map_err(|error| error.to_string())?;
                let (offset, point) = if matches!(action, Action::AddColor) {
                    (
                        0,
                        GradientEntry::Color(
                            gradient
                                .insert_sampled_color(position)
                                .map_err(|error| error.to_string())?,
                        ),
                    )
                } else {
                    let colors = gradient.color_count().map_err(|error| error.to_string())?;
                    (
                        colors,
                        GradientEntry::Alpha(
                            gradient
                                .insert_sampled_alpha(position)
                                .map_err(|error| error.to_string())?,
                        ),
                    )
                };
                usize::try_from(
                    gradient
                        .point_reference_index(&point)
                        .map_err(|error| error.to_string())?,
                )
                .ok()
                .map(|index| offset as usize + index)
            }
            Action::Remove(index) => {
                let entry = self
                    .editor
                    .entries
                    .get(index)
                    .ok_or("The gradient stop is unavailable.")?;
                match self.editor.copy_stop(entry, &gradient)? {
                    GradientEntry::Color(point) => gradient.remove_color(Some(point)),
                    GradientEntry::Alpha(point) => gradient.remove_alpha(Some(point)),
                    _ => return Err("The gradient stop is unavailable.".into()),
                }
                .map_err(|error| error.to_string())?;
                Some(index.saturating_sub(1))
            }
            Action::Select(_) => unreachable!("selection has no graph edit"),
        };
        Ok((gradient, selected))
    }

    pub(in crate::graph_document::view) fn can_run(&self, action: Action, cx: &App) -> bool {
        if self.stop_pending(cx) {
            return false;
        }
        match action {
            Action::AddColor | Action::AddOpacity => true,
            Action::Remove(index) => self.selected == Some(index) && !self.editor.is_changed(cx),
            Action::Select(_) => !self.editor.is_changed(cx),
        }
    }

    pub(super) fn render(&self, actions: &Actions, disabled: bool, cx: &App) -> impl IntoElement {
        let target = actions.target.clone();
        let owner = actions.owner.clone();
        self.render_controls("style-gradient", disabled, cx, move |action, window, cx| {
            let _ = owner.update(cx, |view, cx| {
                view.style_command(&target, Command::Gradient(action), window, cx)
            });
        })
    }

    pub(in crate::graph_document::view) fn render_controls(
        &self,
        id: &'static str,
        disabled: bool,
        cx: &App,
        dispatch: impl Fn(Action, &mut Window, &mut App) + Clone + 'static,
    ) -> impl IntoElement {
        let buttons_disabled = disabled || self.stop_pending(cx);
        let selection_disabled = buttons_disabled || self.editor.is_changed(cx);
        let button = |suffix, label, action, disabled| {
            let dispatch = dispatch.clone();
            Button::new(format!("{id}-{suffix}"))
                .small()
                .label(label)
                .disabled(disabled)
                .on_click(move |_, window, cx| dispatch(action, window, cx))
        };
        let selected = self.selected;
        let mut colors = 0;
        let mut alphas = 0;
        let stops: Vec<_> = self
            .editor
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let label = match entry {
                    GradientEntry::Color(point) => {
                        colors += 1;
                        format!("Color stop {colors} · {}", point.coordinate())
                    }
                    GradientEntry::Alpha(point) => {
                        alphas += 1;
                        format!("Opacity stop {alphas} · {}", point.coordinate())
                    }
                    _ => "Stop".into(),
                };
                (index, label)
            })
            .collect();
        let label = selected
            .and_then(|index| stops.get(index))
            .map_or("No stops".into(), |(_, label)| label.clone());
        let options = dispatch.clone();
        div()
            .v_flex()
            .gap_2()
            .when(id == "style-gradient", |group| {
                group.child(div().font_semibold().child("Gradient ramp"))
            })
            .child(
                self.editor
                    .render_ramp(format!("{id}-ramp"), "Gradient colors and opacity", cx),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new(format!("{id}-stop"))
                            .small()
                            .label(label)
                            .disabled(selection_disabled || stops.is_empty())
                            .dropdown_menu(move |mut menu, _, _| {
                                for (index, label) in &stops {
                                    let action = Action::Select(*index);
                                    let dispatch = options.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(label.clone())
                                            .checked(selected == Some(*index))
                                            .on_click(move |_, window, cx| {
                                                dispatch(action, window, cx)
                                            }),
                                    );
                                }
                                menu.scrollable(true)
                            }),
                    )
                    .when_some(selected, |row, index| {
                        row.child(
                            button(
                                "remove",
                                "Remove stop",
                                Action::Remove(index),
                                selection_disabled,
                            )
                            .ghost(),
                        )
                    }),
            )
            .when(selected.is_some(), |section| {
                section.child(
                    Form::new()
                        .columns(2)
                        .children(self.fields.iter().enumerate().map(
                            |(index, (label, _, input))| {
                                Field::new().label(*label).child(
                                    Input::new(input)
                                        .id(gpui_kit::ElementId::named_usize(
                                            format!("{id}-value"),
                                            index,
                                        ))
                                        .aria_label(*label)
                                        .small()
                                        .disabled(disabled || self.editor.is_changed(cx)),
                                )
                            },
                        )),
                )
            })
            .child(
                Field::new().label("New stop position").child(
                    Input::new(&self.editor.position)
                        .id(format!("{id}-new-position"))
                        .aria_label("New stop position")
                        .small()
                        .disabled(buttons_disabled),
                ),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(button(
                        "add-color",
                        "Add color stop",
                        Action::AddColor,
                        buttons_disabled,
                    ))
                    .child(button(
                        "add-opacity",
                        "Add opacity stop",
                        Action::AddOpacity,
                        buttons_disabled,
                    )),
            )
    }
}

fn position(text: &str, width: i32) -> Result<i32, String> {
    let maximum = width.saturating_sub(1);
    text.parse::<i32>()
        .ok()
        .filter(|value| (0..=maximum).contains(value))
        .ok_or_else(|| format!("Enter a whole position between 0 and {maximum}."))
}

fn same_value(a: &str, b: &str) -> bool {
    a == b
        || a.parse::<f64>()
            .ok()
            .zip(b.parse::<f64>().ok())
            .is_some_and(|(a, b)| a.to_bits() == b.to_bits())
}
