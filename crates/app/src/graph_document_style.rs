//! Style pass authoring in the graph inspector.
use super::environment::EnvironmentPanel;
use super::*;
use crate::graph_document::view::numeric_field::{NumericField, NumericRange};
use gpui_kit::{AnyElement, SharedString, WeakEntity};
use mycela_engine::{
    BinaryCursor, BinaryWriter, CanvasLayerEditor, EnvironmentRecord, SharedCurve, StyleControl,
    StyleControlKind, StyleCurve, StyleEdit, StyleNoiseScope, StylePaintKind, StyleSection,
    SynthesisEdit,
};

type CanvasLayer = CanvasLayerEditor<ForegroundError>;

#[path = "graph_document_style_layers.rs"]
mod layers;
#[path = "graph_document_style_noise.rs"]
pub(super) mod noise_lab;

#[path = "graph_document_style_gradient.rs"]
pub(super) mod paint_gradient;
#[cfg(test)]
#[path = "graph_document_style_synthesis_retirement_ui_tests.rs"]
mod synthesis_retirement_ui_tests;

enum NestedSelection {
    Synthesis(synthesis::Location),
    Stop(usize),
}

pub(super) struct Panel {
    node: Node,
    edit: StyleEdit,
    pass: Option<usize>,
    section: StyleSection,
    canvas_layer: Option<CanvasLayer>,
    expanded: bool,
    controls: Vec<(StyleControl, Option<Entity<InputState>>)>,
    gradient: Option<Result<paint_gradient::Draft, String>>,
}

impl Panel {
    pub(super) fn canvas_layer(&self) -> Option<&CanvasLayer> {
        self.canvas_layer.as_ref()
    }

    fn new(
        node: Node,
        edit: StyleEdit,
        (pass, section): (Option<usize>, StyleSection),
        canvas_layer: Option<CanvasLayer>,
        expanded: bool,
        window: &mut Window,
        cx: &mut Context<GraphDocumentView>,
    ) -> Self {
        let controls = pass
            .map_or_else(Vec::new, |index| edit.controls(index, section))
            .into_iter()
            .map(|control| {
                let input = match control.kind() {
                    StyleControlKind::Number([minimum, maximum]) => Some(cx.new(|cx| {
                        InputState::new(window, cx)
                            .default_value(control.value().to_string())
                            .min(minimum)
                            .max(maximum)
                            .step(0.01)
                    })),
                    _ => None,
                };
                (control, input)
            })
            .collect();
        let gradient = pass
            .filter(|_| expanded && section == StyleSection::Effect)
            .and_then(|index| edit.gradient(index))
            .cloned()
            .map(|ramp| paint_gradient::Draft::new(ramp, window, cx));
        Self {
            node,
            edit,
            pass,
            section,
            canvas_layer,
            expanded,
            controls,
            gradient,
        }
    }

    pub(super) fn load(
        node: &Node,
        previous: Option<&Self>,
        window: &mut Window,
        cx: &mut Context<GraphDocumentView>,
    ) -> Option<Self> {
        let filter = node.filter()?;
        let previous = previous.filter(|panel| panel.node.same_reference(node));
        let mut canvas_layer = None;
        let edit = if let Some(edit) = filter.style_edit() {
            edit
        } else {
            let layers = filter.canvas_layer_editors();
            let layer = previous
                .and_then(|panel| panel.canvas_layer.as_ref())
                .and_then(|previous| layers.iter().find(|layer| layer.same_reference(previous)))
                .or_else(|| layers.first())?
                .clone();
            let edit = layer.style_edit()?;
            canvas_layer = Some(layer);
            edit
        };
        let pass = previous
            .filter(|panel| match (&panel.canvas_layer, &canvas_layer) {
                (Some(old), Some(new)) => old.same_reference(new),
                (None, None) => true,
                _ => false,
            })
            .and_then(|panel| edit.corresponding_pass(&panel.edit, panel.pass?))
            .or_else(|| edit.selected());
        let section = previous.map_or(StyleSection::Effect, |panel| panel.section);
        let expanded = canvas_layer.is_none() || previous.is_some_and(|panel| panel.expanded);
        let mut panel = Self::new(
            node.clone(),
            edit,
            (pass, section),
            canvas_layer,
            expanded,
            window,
            cx,
        );
        if let Some(previous) = previous.filter(|previous| previous.pass == pass) {
            panel.select_gradient(
                previous
                    .gradient()
                    .and_then(paint_gradient::Draft::selected),
                window,
                cx,
            );
        }
        Some(panel)
    }

    fn gradient(&self) -> Option<&paint_gradient::Draft> {
        self.gradient.as_ref()?.as_ref().ok()
    }

    fn select_gradient(
        &mut self,
        stop: Option<usize>,
        window: &mut Window,
        cx: &mut Context<GraphDocumentView>,
    ) {
        if let Some(Ok(gradient)) = &mut self.gradient
            && let Err(error) = gradient.select(stop, window, cx)
        {
            self.gradient = Some(Err(error));
        }
    }

    pub(super) fn canvas_expanded(&self) -> bool {
        self.canvas_layer.is_some() && self.expanded
    }

    pub(super) fn synthesis_draft(&self) -> Option<SynthesisEdit> {
        if !self.expanded || self.section != StyleSection::Effect {
            return None;
        }
        self.edit.synthesis_edit(self.pass?)
    }

    pub(super) fn synthesis_style_draft(&self) -> Option<(&StyleEdit, usize)> {
        Some((&self.edit, self.pass?))
    }

    fn environment_panel(&self, document: &model::Document) -> Option<EnvironmentPanel> {
        if !self.expanded || self.section != StyleSection::Effect {
            return None;
        }
        let pass = self.pass?;
        let map = self.edit.environment_map(pass)?.clone();
        Some(EnvironmentPanel::for_style(
            EnvironmentOwner {
                node: self.node.clone(),
                canvas_layer: self.canvas_layer.clone(),
                pass,
                linked: self.edit.has_environment_adjuster(pass),
            },
            map,
            document.style_environment_adjuster(&self.edit, pass),
        ))
    }

    pub(super) fn inputs(&self) -> impl Iterator<Item = Entity<InputState>> + '_ {
        self.controls
            .iter()
            .filter_map(|(_, input)| input.clone())
            .chain(
                self.gradient()
                    .into_iter()
                    .flat_map(paint_gradient::Draft::inputs),
            )
    }

    pub(super) fn pending(&self, cx: &App) -> bool {
        self.controls_pending(cx) || self.gradient().is_some_and(|gradient| gradient.pending(cx))
    }

    fn controls_pending(&self, cx: &App) -> bool {
        self.controls.iter().any(|(field, input)| {
            input.as_ref().is_some_and(|input| {
                input.read(cx).value().trim().parse::<f64>().ok() != Some(field.value())
            })
        })
    }

    fn values(&self, cx: &App) -> Result<Vec<f64>, String> {
        self.controls
            .iter()
            .map(|(field, input)| match input {
                None => Ok(field.value()),
                Some(input) => input
                    .read(cx)
                    .value()
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| format!("Enter a number for {}.", field.label())),
            })
            .collect()
    }
}

#[derive(Clone)]
pub(super) struct EnvironmentOwner {
    node: Node,
    canvas_layer: Option<CanvasLayer>,
    pass: usize,
    linked: bool,
}

impl EnvironmentOwner {
    pub(super) fn has_adjuster_link(&self) -> bool {
        self.linked
    }

    pub(super) fn map_target(&self) -> mycela_engine::EnvironmentMapTarget<'_, ForegroundError> {
        mycela_engine::EnvironmentMapTarget::Paint {
            owner: &self.node,
            layer: self.canvas_layer.as_ref(),
            pass: self.pass,
        }
    }

    pub(super) fn available_adjusters(
        &self,
        document: &model::Document,
    ) -> Vec<mycela_engine::EnvironmentAdjuster> {
        document.available_style_environment_adjusters(&self.node)
    }

    pub(super) fn set_adjuster(
        &self,
        document: &mut model::Document,
        expected: &Arc<EnvironmentRecord>,
        link: mycela_engine::EnvironmentAdjusterLink,
    ) -> Result<bool, mycela_engine::GtxGraphError<ForegroundError>> {
        document.set_style_environment_adjuster(
            &self.node,
            self.canvas_layer.as_ref(),
            self.pass,
            expected,
            link,
        )
    }

    pub(super) fn same_pass(&self, other: &Self) -> bool {
        self.pass == other.pass
            && self.node.same_reference(&other.node)
            && match (&self.canvas_layer, &other.canvas_layer) {
                (Some(layer), Some(other)) => layer.same_reference(other),
                (None, None) => true,
                _ => false,
            }
    }

    fn edit(&self) -> Option<StyleEdit> {
        match &self.canvas_layer {
            Some(layer) => layer.style_edit(),
            None => self.node.filter()?.style_edit(),
        }
    }

    pub(super) fn map(&self) -> Result<Arc<EnvironmentRecord>, String> {
        self.edit()
            .and_then(|edit| edit.environment_map(self.pass).cloned())
            .ok_or_else(|| "The selected Style pass no longer has an environment map.".into())
    }

    pub(super) fn preview_environment(
        &self,
        map: Arc<EnvironmentRecord>,
    ) -> mycela_engine::EnvironmentRenderOverride<ForegroundError> {
        mycela_engine::EnvironmentRenderOverride::for_style(
            self.node.clone(),
            self.canvas_layer.as_ref(),
            self.pass,
            map,
        )
    }

    pub(super) fn commit(
        &self,
        expected: &Arc<EnvironmentRecord>,
        adjuster: Option<&mycela_engine::EnvironmentAdjuster>,
        map: EnvironmentRecord,
        document: &LiveDocument,
    ) -> Result<(), String> {
        let mut edit = self
            .edit()
            .ok_or("The selected Style pass is unavailable.")?;
        if let Some(adjuster) = adjuster {
            let doc = guard(document);
            if !doc
                .style_environment_adjuster(&edit, self.pass)
                .is_some_and(|current| current.same_reference(adjuster))
                || !doc
                    .edit_environment_adjuster_value(adjuster, expected, map)
                    .map_err(model::failure)?
            {
                return Err("The linked Environment changed. Open its editor again.".into());
            }
            return Ok(());
        }
        edit.replace_environment_map(self.pass, expected, map)?;
        let label = "Edit Style environment map";
        match &self.canvas_layer {
            Some(layer) => layer.edit_style(edit, label),
            None => self
                .node
                .filter()
                .ok_or("The Style node is unavailable.")?
                .edit_style(edit, label),
        }
        .map_err(model::failure)?;
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct Target {
    key: Option<String>,
    node: Node,
    revision: u64,
    pass: Option<usize>,
    section: StyleSection,
    canvas_layer: Option<CanvasLayer>,
    expanded: bool,
}

impl Target {
    pub(super) fn synthesis_number_receiver(&self) -> Option<(&Node, Option<&CanvasLayer>, usize)> {
        Some((&self.node, self.canvas_layer.as_ref(), self.pass?))
    }

    pub(super) fn texture_input_label(&self) -> &'static str {
        if self.canvas_layer.is_some() {
            "Layer input"
        } else {
            "Input Texture"
        }
    }

    fn new(view: &GraphDocumentView, panel: &Panel) -> Self {
        Self {
            key: view.selection.clone(),
            node: panel.node.clone(),
            revision: view.revision,
            pass: panel.pass,
            section: panel.section,
            canvas_layer: panel.canvas_layer.clone(),
            expanded: panel.expanded,
        }
    }

    pub(super) fn current(&self, view: &GraphDocumentView) -> bool {
        view.is_original_selection(&self.key, &Some(self.node.clone()), self.revision)
            && view.style.as_ref().is_some_and(|panel| {
                let same_layer = match (&panel.canvas_layer, &self.canvas_layer) {
                    (Some(a), Some(b)) => a.same_reference(b),
                    (None, None) => true,
                    _ => false,
                };
                same_layer
                    && panel.expanded == self.expanded
                    && panel.pass == self.pass
                    && panel.section == self.section
                    && panel.node.same_reference(&self.node)
            })
    }
}

#[derive(Clone, Copy)]
enum Command {
    Select(usize),
    Section(StyleSection),
    Add(StylePaintKind),
    Paint(StylePaintKind),
    Move(usize),
    Duplicate,
    Remove,
    Control(usize, f64),
    Apply,
    Preset,
    Gradient(paint_gradient::Action),
    Curve(StyleCurve),
    OpenNoise(StyleNoiseScope),
    EnableNoise(StyleNoiseScope, bool),
}

#[derive(Clone)]
struct Actions {
    target: Target,
    owner: WeakEntity<GraphDocumentView>,
    disabled: bool,
}

impl Actions {
    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        command: Command,
        inactive: bool,
    ) -> Button {
        let Self {
            target,
            owner,
            disabled,
        } = self.clone();
        Button::new(id)
            .small()
            .label(label)
            .disabled(disabled || inactive)
            .on_click(move |_, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.style_command(&target, command, window, cx)
                });
            })
    }

    fn item(
        &self,
        label: impl Into<SharedString>,
        command: Command,
        checked: bool,
    ) -> PopupMenuItem {
        let Self { target, owner, .. } = self.clone();
        PopupMenuItem::new(label)
            .checked(checked)
            .on_click(move |_, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.style_command(&target, command, window, cx)
                });
            })
    }
}

impl GraphDocumentView {
    pub(super) fn sync_style_environment(
        &mut self,
        previous: Option<&EnvironmentPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .inspector
            .environment
            .as_ref()
            .filter(|panel| panel.is_style());
        let next = self
            .style
            .as_ref()
            .and_then(|panel| panel.environment_panel(&guard(&self.document)));
        let Some(next) = next else {
            if current.is_some() {
                self.inspector.environment = None;
                self.inspector.clear_environment_fields();
            }
            return;
        };
        if current.is_some_and(|panel| {
            panel.same_source(&next)
                && Arc::ptr_eq(panel.value(), next.value())
                && panel.same_adjuster(&next)
        }) {
            return;
        }
        let selected = current
            .or(previous)
            .filter(|panel| panel.same_source(&next))
            .and_then(EnvironmentPanel::selected)
            .unwrap_or(0);
        self.inspector.environment = Some(next);
        if let Err(error) = self
            .inspector
            .select_environment_layer(selected, window, cx)
        {
            self.error = Some(error);
        }
    }

    pub(super) fn current_style_target(&self) -> Option<Target> {
        Some(Target::new(self, self.style.as_ref()?))
    }

    pub(super) fn apply_style_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = &self.style {
            self.style_command(&Target::new(self, panel), Command::Apply, window, cx);
        }
    }

    pub(super) fn render_style_panel(
        &self,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let panel = self.style.as_ref()?;
        let synthesis_pending = self
            .synthesis
            .as_ref()
            .is_some_and(|panel| panel.pending(cx));
        let actions = Actions {
            target: Target::new(self, panel),
            owner: cx.weak_entity(),
            disabled: busy || panel.pending(cx) || synthesis_pending,
        };
        if !panel.expanded {
            return None;
        }
        let add = actions.clone();
        let select = actions.clone();
        let paint_menu = actions.clone();
        let pass = panel.pass;
        let count = panel.edit.count();
        let passes: Vec<_> = (0..count)
            .filter_map(|index| panel.edit.paint(index))
            .collect();
        let paint = pass.and_then(|index| panel.edit.paint(index));
        let noise = pass.zip(paint).and_then(|(pass, paint)| {
            let scope = StyleNoiseScope::for_section(panel.section, paint)?;
            Some((scope, panel.edit.has_noise_lab(pass, scope)))
        });
        let curves: Vec<_> = StyleCurve::ALL
            .into_iter()
            .filter(|curve| {
                curve.section() == panel.section
                    && pass.is_some_and(|pass| panel.edit.curve(pass, *curve).is_some())
            })
            .collect();
        let curve_menu = (!curves.is_empty()).then(|| {
            let actions = actions.clone();
            Button::new("style-curve")
                .small()
                .label("Edit curve…")
                .disabled(actions.disabled)
                .dropdown_menu(move |mut menu, _, _| {
                    for &curve in &curves {
                        menu = menu.item(actions.item(curve.label(), Command::Curve(curve), false));
                    }
                    menu
                })
        });
        let gradient_pending = panel
            .gradient()
            .is_some_and(|gradient| gradient.pending(cx));
        let gradient = panel.gradient.as_ref().map(|gradient| match gradient {
            Ok(gradient) => gradient
                .render(
                    &actions,
                    busy || synthesis_pending || panel.controls_pending(cx),
                    cx,
                )
                .into_any_element(),
            Err(error) => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(error.clone())
                .into_any_element(),
        });
        let toolbar = div()
            .h_flex()
            .gap_2()
            .flex_wrap()
            .child(
                Button::new("style-pass-picker")
                    .small()
                    .label(pass.zip(paint).map_or_else(
                        || "No passes".into(),
                        |(index, paint)| format!("{} · {}", index + 1, paint.label()),
                    ))
                    .disabled(actions.disabled || count == 0)
                    .dropdown_menu(move |mut menu, _, _| {
                        for (index, paint) in passes.iter().enumerate() {
                            menu = menu.item(select.item(
                                format!("{} · {}", index + 1, paint.label()),
                                Command::Select(index),
                                Some(index) == pass,
                            ));
                        }
                        menu
                    }),
            )
            .child(
                Button::new("style-add-pass")
                    .small()
                    .label("Add pass")
                    .disabled(actions.disabled)
                    .dropdown_menu(move |mut menu, _, _| {
                        for kind in StylePaintKind::ALL {
                            menu = menu.item(add.item(kind.label(), Command::Add(kind), false));
                        }
                        menu
                    }),
            )
            .child(actions.button("style-open-preset", "Open preset…", Command::Preset, false));
        let sections = [
            (StyleSection::Area, "style-area", "Area"),
            (StyleSection::Effect, "style-effect", "Effect"),
            (StyleSection::Blend, "style-blend", "Blend"),
        ];
        let form =
            Form::new()
                .columns(2)
                .children(panel.controls.iter().enumerate().filter_map(
                    |(index, (control, input))| {
                        let label = control.label();
                        match control.kind() {
                            StyleControlKind::Number(bounds) => Some(
                                Field::new().label(label).child(
                                    NumericField::new(
                                        input.as_ref()?,
                                        ("style-number", index),
                                        label,
                                        NumericRange::bounds(bounds, 0.01),
                                    )
                                    .disabled(busy || synthesis_pending || gradient_pending),
                                ),
                            ),
                            StyleControlKind::Toggle => {
                                let current = control.value() != 0.;
                                let Actions {
                                    target,
                                    owner,
                                    disabled,
                                } = actions.clone();
                                Some(
                                    Field::new().child(
                                        Checkbox::new(("style-toggle", index))
                                            .label(label)
                                            .checked(current)
                                            .disabled(disabled)
                                            .on_click(move |checked, window, cx| {
                                                let _ = owner.update(cx, |view, cx| {
                                                    view.style_command(
                                                        &target,
                                                        Command::Control(
                                                            index,
                                                            f64::from(u8::from(*checked)),
                                                        ),
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            }),
                                    ),
                                )
                            }
                            StyleControlKind::Choice(choices) => {
                                let current = control.value() as i32;
                                let actions = actions.clone();
                                Some(
                                    Field::new().label(label).child(
                                        Button::new(("style-choice", index))
                                            .small()
                                            .label(
                                                choices
                                                    .get(current as usize)
                                                    .copied()
                                                    .unwrap_or("Choose…"),
                                            )
                                            .disabled(actions.disabled)
                                            .dropdown_menu(move |mut menu, _, _| {
                                                for (value, label) in choices.iter().enumerate() {
                                                    menu = menu.item(actions.item(
                                                        *label,
                                                        Command::Control(index, value as f64),
                                                        value as i32 == current,
                                                    ));
                                                }
                                                menu
                                            }),
                                    ),
                                )
                            }
                        }
                    },
                ));
        Some(
            div()
                .v_flex()
                .gap_2()
                .child(div().font_semibold().child("Style passes"))
                .child(toolbar)
                .when_some(pass, |view, index| {
                    view.child(
                        div()
                            .h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(actions.button(
                                "style-pass-up",
                                "Move up",
                                Command::Move(index.saturating_sub(1)),
                                index == 0,
                            ))
                            .child(actions.button(
                                "style-pass-down",
                                "Move down",
                                Command::Move(index + 1),
                                index + 1 >= count,
                            ))
                            .child(actions.button(
                                "style-pass-duplicate",
                                "Duplicate",
                                Command::Duplicate,
                                false,
                            ))
                            .child(
                                actions
                                    .button(
                                        "style-pass-remove",
                                        "Remove pass",
                                        Command::Remove,
                                        false,
                                    )
                                    .ghost(),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .flex_wrap()
                            .children(sections.into_iter().map(|(section, id, label)| {
                                actions
                                    .button(id, label, Command::Section(section), false)
                                    .selected(panel.section == section)
                            })),
                    )
                    .when(panel.section == StyleSection::Effect, |view| {
                        view.child(
                            Button::new("style-effect-kind")
                                .small()
                                .label("Change effect")
                                .disabled(actions.disabled)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for kind in StylePaintKind::ALL {
                                        menu = menu.item(paint_menu.item(
                                            kind.label(),
                                            Command::Paint(kind),
                                            Some(kind) == paint,
                                        ));
                                    }
                                    menu
                                }),
                        )
                    })
                    .child(div().id(("style-pass-fields", index)).child(form))
                })
                .children(curve_menu)
                .when_some(noise, |view, (scope, enabled)| {
                    view.child(
                        div()
                            .h_flex()
                            .flex_wrap()
                            .gap_2()
                            .when(scope != StyleNoiseScope::Paint, |row| {
                                let state = if enabled { "Disable" } else { "Enable" };
                                let target = actions.target.clone();
                                let owner = actions.owner.clone();
                                row.child(
                                    Button::new("style-enable-noise")
                                        .small()
                                        .label(format!(
                                            "{state} {}",
                                            scope.label().to_ascii_lowercase()
                                        ))
                                        .disabled(actions.disabled)
                                        .on_click(move |_, window, cx| {
                                            let _ = owner.update(cx, |view, cx| {
                                                view.style_command(
                                                    &target,
                                                    Command::EnableNoise(scope, !enabled),
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }),
                                )
                            })
                            .when(enabled, |row| {
                                row.child(actions.button(
                                    "style-edit-noise",
                                    "Edit Noise Lab…",
                                    Command::OpenNoise(scope),
                                    false,
                                ))
                            }),
                    )
                })
                .children(gradient)
                .into_any_element(),
        )
    }

    fn style_command(
        &mut self,
        target: &Target,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy()
            || !target.current(self)
            || self
                .synthesis
                .as_ref()
                .is_some_and(|panel| panel.pending(cx))
        {
            return;
        }
        if let Command::Gradient(action) = command {
            self.style_gradient_command(target, action, window, cx);
            return;
        }
        let Some(panel) = self.style.as_ref() else {
            return;
        };
        if panel.pending(cx) && !matches!(command, Command::Apply) {
            return;
        }
        if !matches!(command, Command::Apply)
            && self
                .inspector
                .environment
                .as_ref()
                .is_some_and(EnvironmentPanel::is_style)
            && self.inspector.environment_fields_changed(cx)
        {
            self.error = Some("Correct or reset field text before changing the Style pass.".into());
            cx.notify();
            return;
        }
        if let Command::Curve(curve) = command {
            self.open_style_curve(target, curve, window, cx);
            return;
        }
        if let Command::OpenNoise(scope) = command {
            self.open_style_noise_lab(target.clone(), scope, cx);
            return;
        }
        let mut edit = panel.edit.clone();
        let pass = panel.pass;
        let section = panel.section;
        let canvas_layer = panel.canvas_layer.clone();
        let expanded = panel.expanded;
        let selection = match command {
            Command::Select(index) => Some((Some(index), section)),
            Command::Section(section) => Some((pass, section)),
            _ => None,
        };
        if let Some((pass, section)) = selection {
            self.style = Some(Panel::new(
                target.node.clone(),
                edit,
                (pass, section),
                canvas_layer,
                expanded,
                window,
                cx,
            ));
            self.sync_style_environment(None, window, cx);
            self.reload_synthesis(None, window, cx);
            self.observe_drafts(cx);
            cx.notify();
            return;
        }
        if matches!(command, Command::Preset) {
            self.open_style_preset(target.clone(), edit, cx);
            return;
        }
        let outcome: Result<Option<&str>, String> = (|| {
            Ok(match command {
                Command::Select(_)
                | Command::Section(_)
                | Command::Preset
                | Command::Gradient(_)
                | Command::Curve(_)
                | Command::OpenNoise(_) => unreachable!(),
                Command::EnableNoise(scope, enabled) => {
                    let Some(pass) = pass else { return Ok(None) };
                    edit.set_noise_enabled(pass, scope, enabled)?
                        .then_some(if enabled {
                            "Enable Style noise"
                        } else {
                            "Disable Style noise"
                        })
                }
                Command::Add(kind) => {
                    edit.add(kind)?;
                    Some("Add Style pass")
                }
                Command::Duplicate => pass
                    .and_then(|index| edit.duplicate(index))
                    .map(|_| "Duplicate Style pass"),
                Command::Remove => pass
                    .filter(|index| edit.remove(*index))
                    .map(|_| "Remove Style pass"),
                Command::Move(to) => pass
                    .filter(|index| edit.move_pass(*index, to))
                    .map(|_| "Move Style pass"),
                Command::Paint(kind) => {
                    let Some(index) = pass else { return Ok(None) };
                    edit.set_paint(index, kind)?.then_some("Change Style paint")
                }
                Command::Apply | Command::Control(..) => {
                    let Some(pass) = pass else { return Ok(None) };
                    let mut values = panel.values(cx)?;
                    if let Command::Control(index, new) = command {
                        let Some(value) = values.get_mut(index) else {
                            return Ok(None);
                        };
                        *value = new;
                    }
                    edit.set_controls(pass, section, &values).map_err(|label| {
                        let range =
                            panel
                                .controls
                                .iter()
                                .find_map(|(control, _)| match control.kind() {
                                    StyleControlKind::Number(range) if control.label() == label => {
                                        Some(range)
                                    }
                                    _ => None,
                                });
                        range.map_or_else(
                            || label.to_owned(),
                            |[min, max]| {
                                format!("Enter a number between {min} and {max} for {label}.")
                            },
                        )
                    })?;
                    if let Some(gradient) = panel.gradient()
                        && let Some(staged) = gradient.staged(cx)?
                    {
                        edit.set_gradient(pass, &staged)?;
                        return Ok(Some("Edit gradient stop"));
                    }
                    Some("Edit Style pass")
                }
            })
        })();
        match outcome {
            Ok(Some(label)) => self.commit_style(target, edit, label, None, window, cx),
            Ok(None) => {}
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn open_style_curve(
        &mut self,
        target: &Target,
        kind: StyleCurve,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pass) = target.pass else {
            return;
        };
        let Some(record) = self
            .style
            .as_ref()
            .and_then(|panel| panel.edit.curve(pass, kind))
        else {
            return;
        };
        let curve = SharedCurve::new(0., 1., false);
        let result = (|| {
            let mut bytes = BinaryWriter::default();
            record.save(&mut bytes).map_err(|error| error.to_string())?;
            let mut reader = BinaryCursor::new(bytes.bytes()).map_err(|error| error.to_string())?;
            curve
                .load(&mut reader, 3.75)
                .map_err(|error| error.to_string())
        })();
        if let Err(error) = result {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let target = target.clone();
        self.open_remap_editor(
            curve,
            kind.label(),
            move |view, curve, window, cx| {
                if !target.current(view) {
                    return Err("The selected pass changed. Open the curve again.".into());
                }
                let panel = view
                    .style
                    .as_ref()
                    .ok_or("The selected pass is unavailable.")?;
                let mut edit = panel.edit.clone();
                if edit.set_curve(pass, kind, curve)? {
                    crate::tone_curve::samples(curve)?;
                    view.commit_style(&target, edit, "Edit Style curve", None, window, cx);
                    if let Some(error) = &view.error {
                        return Err(error.clone());
                    }
                }
                Ok(())
            },
            window,
            cx,
        );
    }

    fn style_gradient_command(
        &mut self,
        target: &Target,
        action: paint_gradient::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.style.as_mut() else {
            return;
        };
        let Some((pass, gradient)) = panel.pass.zip(panel.gradient()) else {
            return;
        };
        if panel.controls_pending(cx) || !gradient.can_run(action, cx) {
            return;
        }
        if let paint_gradient::Action::Select(index) = action {
            panel.select_gradient(Some(index), window, cx);
            self.observe_drafts(cx);
            cx.notify();
            return;
        }
        let result = (|| {
            let (gradient, selected) = gradient.change(action, cx)?;
            let mut edit = panel.edit.clone();
            let changed = edit.set_gradient(pass, &gradient)?;
            Ok::<_, String>(changed.then_some((edit, selected.map(NestedSelection::Stop))))
        })();
        match result {
            Ok(Some((edit, selected))) => {
                let label = if matches!(action, paint_gradient::Action::Remove(_)) {
                    "Remove gradient stop"
                } else {
                    "Add gradient stop"
                };
                self.commit_style(target, edit, label, selected, window, cx);
            }
            Ok(None) => {}
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn commit_style(
        &mut self,
        target: &Target,
        edit: StyleEdit,
        label: &str,
        nested_selection: Option<NestedSelection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ensure_history_binding();
        if let Some(history) = self.history() {
            history.separate_next_edit();
        }
        let Some(filter) = target.node.filter() else {
            return;
        };
        let was_dirty = self.dirty;
        let stamp = self.begin_history_edit();
        self.error = None;
        let handled = guard(&self.document)
            .edit_style_removing_unused_synthesis_adjusters(
                &target.node,
                target.canvas_layer.as_ref(),
                &edit,
                label,
            )
            .map_err(model::failure);
        let result = match handled {
            Ok(false) => match &target.canvas_layer {
                Some(layer) => layer.edit_style(edit, label),
                None => filter.edit_style(edit, label),
            }
            .map_err(model::failure),
            other => other,
        };
        match result {
            Ok(true) => {
                self.revision += 1;
                self.status = None;
                let current = match &target.canvas_layer {
                    Some(layer) => layer.style_edit(),
                    None => filter.style_edit(),
                };
                let (synthesis_location, gradient_stop) = match nested_selection {
                    Some(NestedSelection::Synthesis(location)) => (Some(location), None),
                    Some(NestedSelection::Stop(stop)) => (None, Some(stop)),
                    None => (
                        None,
                        self.style
                            .as_ref()
                            .and_then(Panel::gradient)
                            .and_then(paint_gradient::Draft::selected),
                    ),
                };
                self.style = current.map(|edit| {
                    let pass = edit.selected();
                    let mut panel = Panel::new(
                        target.node.clone(),
                        edit,
                        (pass, target.section),
                        target.canvas_layer.clone(),
                        target.expanded,
                        window,
                        cx,
                    );
                    if gradient_stop.is_some() && pass == target.pass {
                        panel.select_gradient(gradient_stop, window, cx);
                    }
                    panel
                });
                if target.canvas_layer.is_some() {
                    let previous = self.inspector.canvas_area.take();
                    self.inspector
                        .drafts
                        .retain(|draft| draft.canvas_area_field().is_none());
                    self.inspector.canvas_areas = filter.canvas_area_editors();
                    let current = self
                        .inspector
                        .canvas_areas
                        .iter()
                        .find(|area| {
                            previous
                                .as_ref()
                                .is_some_and(|old| area.same_reference(&old.editor))
                        })
                        .or_else(|| self.inspector.canvas_areas.first());
                    if let Some(area) = current.cloned() {
                        self.error = self.inspector.select_canvas_area(area, window, cx).err();
                    }
                }
                self.sync_style_environment(None, window, cx);
                self.reload_synthesis(synthesis_location, window, cx);
                self.observe_drafts(cx);
                self.finish_history_edit(stamp, true);
                self.start_preview(cx);
            }
            Ok(false) => self.dirty = was_dirty,
            Err(error) => {
                self.dirty = was_dirty;
                self.error = Some(error);
            }
        }
        cx.notify();
    }

    pub(super) fn commit_style_synthesis(
        &mut self,
        target: &Target,
        synthesis: SynthesisEdit,
        location: synthesis::Location,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !target.current(self) {
            return;
        }
        let Some((panel, pass)) = self.style.as_ref().zip(target.pass) else {
            return;
        };
        let mut edit = panel.edit.clone();
        match edit.set_synthesis(pass, synthesis) {
            Ok(true) => self.commit_style(
                target,
                edit,
                label,
                Some(NestedSelection::Synthesis(location)),
                window,
                cx,
            ),
            Ok(false) => {}
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
            }
        }
    }

    fn open_style_preset(&mut self, target: Target, mut edit: StyleEdit, cx: &mut Context<Self>) {
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::LoadingStyle);
        self.error = None;
        let prompt = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open Style preset".into()),
        });
        let executor = cx.background_executor().clone();
        let handle = self.window;
        cx.spawn(async move |owner, cx| {
            let paths = prompt.await;
            if !ticket.is_current() {
                return;
            }
            let result = match paths {
                Ok(Ok(Some(mut paths))) if paths.len() == 1 => {
                    let path = paths.remove(0);
                    executor
                        .spawn(async move {
                            let bytes = std::fs::read(path).map_err(|error| {
                                format!("Couldn’t read the Style preset. {error}")
                            })?;
                            edit.load_preset(&bytes)?;
                            Ok(Some(edit))
                        })
                        .await
                }
                Ok(Ok(None)) => Ok(None),
                Ok(Ok(Some(_))) => Err("Choose a single Style preset.".to_owned()),
                Ok(Err(error)) => Err(format!("Couldn’t open the file picker. {error}")),
                Err(_) => Err("The file picker closed without a selection.".into()),
            };
            let _ = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if !view.owns(&ticket) {
                        return;
                    }
                    view.ticket = None;
                    view.work = None;
                    if target.current(view) {
                        match result {
                            Ok(Some(edit)) => view.commit_style(
                                &target,
                                edit,
                                "Open Style preset",
                                None,
                                window,
                                cx,
                            ),
                            Err(error) => view.error = Some(error),
                            _ => {}
                        }
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }
}
