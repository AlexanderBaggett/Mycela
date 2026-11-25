//! Stop editing for Colorize, Environment gradients, and animated palettes.
use super::super::inspector::{AnimationOwner, AnimationParameter};
use super::environment::EnvironmentPanel;
use super::{GraphDocumentView, adjuster_name::AdjusterNameKind, guard, model};
use crate::ForegroundError;
use crate::graph_document::view::numeric_field::{NumericField, NumericRange};
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    form::Field,
    input::InputState,
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{App, AppContext, Context, Entity, IntoElement, Window, div, prelude::*};
use mycela_engine::{
    GradientAdjusterLink, GradientAnimation, GradientEntry, GradientPointList, LabColorAdjuster,
    PointwiseParameter, PointwiseParameterValue, RgbColor, SharedGradient,
};
use std::sync::Arc;

pub(super) struct ColorAdjusterMenu {
    pub id: &'static str,
    pub kind: AdjusterNameKind,
    pub label: String,
    pub linked: Option<LabColorAdjuster>,
    pub choices: Vec<LabColorAdjuster>,
    pub has_link: bool,
    pub disabled: bool,
    pub edit_linked: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnvironmentGradientKind {
    Recolor,
    Mask,
}

impl EnvironmentGradientKind {
    pub(crate) fn store(
        self,
        map: &mut mycela_engine::EnvironmentRecord,
        index: usize,
        gradient: &SharedGradient,
    ) -> Result<(), mycela_engine::EnvironmentEditError> {
        match self {
            Self::Recolor => map.set_recolor_gradient(index, gradient),
            Self::Mask => map.set_mask_gradient(index, gradient),
        }
    }
}

#[derive(Clone)]
enum GradientOwner {
    Colorize(PointwiseParameter<ForegroundError>),
    Environment(EnvironmentPanel, usize, EnvironmentGradientKind),
    Palette(AnimationParameter, GradientAnimation<ForegroundError>),
    Draft,
}

#[derive(Clone)]
pub(crate) struct GradientEditor {
    owner: GradientOwner,
    gradient: SharedGradient,
    colors: GradientPointList,
    alphas: GradientPointList,
    pub entries: Arc<[GradientEntry]>,
    pub position: Entity<InputState>,
    loaded_position: String,
    pub(super) ramp: Result<Arc<[u32]>, String>,
}
impl GradientEditor {
    pub(super) fn new_draft(
        gradient: SharedGradient,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self, String> {
        Self::build(GradientOwner::Draft, gradient, window, cx)
    }

    pub(crate) fn width(&self) -> i32 {
        self.gradient.width()
    }

    pub fn new(
        parameter: PointwiseParameter<ForegroundError>,
        gradient: SharedGradient,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self, String> {
        Self::build(GradientOwner::Colorize(parameter), gradient, window, cx)
    }
    pub(crate) fn new_environment(
        panel: EnvironmentPanel,
        index: usize,
        kind: EnvironmentGradientKind,
        gradient: SharedGradient,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self, String> {
        Self::build(
            GradientOwner::Environment(panel, index, kind),
            gradient,
            window,
            cx,
        )
    }
    pub(crate) fn environment(
        &self,
    ) -> Option<(&EnvironmentPanel, usize, EnvironmentGradientKind)> {
        match &self.owner {
            GradientOwner::Environment(panel, index, kind) => Some((panel, *index, *kind)),
            _ => None,
        }
    }
    pub(crate) fn is_mask(&self) -> bool {
        matches!(
            self.environment(),
            Some((_, _, EnvironmentGradientKind::Mask))
        )
    }
    pub(crate) fn control_id(&self, suffix: &str) -> String {
        if self.is_mask() {
            format!("graph-mask-gradient-{suffix}")
        } else {
            format!("graph-gradient-{suffix}")
        }
    }
    pub(crate) fn copy_for_edit(&self) -> Result<SharedGradient, String> {
        self.validate()?;
        self.gradient.source_clone().map_err(|e| e.to_string())
    }
    pub(crate) fn is_colorize(&self) -> bool {
        matches!(self.owner, GradientOwner::Colorize(_))
    }

    pub(super) fn commit_colorize(
        &self,
        gradient: SharedGradient,
        document: &model::LiveDocument,
    ) -> Result<(), String> {
        let GradientOwner::Colorize(parameter) = &self.owner else {
            return Err("The selected node no longer has a Colorize gradient.".into());
        };
        self.validate()?;
        gradient.rebuild().map_err(|error| error.to_string())?;
        guard(document)
            .edit_gradient_property(parameter, &self.gradient, gradient)
            .map_err(|error| error.to_string())
    }
    pub(crate) fn new_animation_palette(
        parameter: AnimationParameter,
        animation: GradientAnimation<ForegroundError>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self, String> {
        let gradient = animation.gradient().map_err(|e| e.to_string())?;
        Self::build(
            GradientOwner::Palette(parameter, animation),
            gradient,
            window,
            cx,
        )
    }
    fn build(
        owner: GradientOwner,
        gradient: SharedGradient,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self, String> {
        let colors = gradient
            .color_points()
            .ok_or("The gradient color list is unavailable.")?;
        let alphas = gradient
            .alpha_points()
            .ok_or("The gradient opacity list is unavailable.")?;
        let palette = matches!(owner, GradientOwner::Palette(..));
        let entries = Self::read_entries(&colors, &alphas, palette)?;
        // Sampling a detached source copy avoids mutating or sorting the live
        // gradient while the inspector builds its visual representation.
        let ramp = (|| -> Result<Arc<[u32]>, String> {
            let width = gradient.width();
            if !(1..=65536).contains(&width) {
                return Err("Gradient ramp is unavailable at this width.".into());
            }
            let copy = gradient.source_clone().map_err(|e| e.to_string())?;
            copy.rebuild().map_err(|e| e.to_string())?;
            (0..64)
                .map(|index| {
                    let color = copy
                        .sample(f64::from(index) / 63.0)
                        .map_err(|e| e.to_string())?;
                    let [a, r, g, b] = color.to_argb_bytes();
                    Ok(u32::from_be_bytes([
                        r,
                        g,
                        b,
                        if palette { u8::MAX } else { a },
                    ]))
                })
                .collect::<Result<Vec<_>, String>>()
                .map(Arc::from)
        })();
        let loaded_position = (gradient.width() / 2).to_string();
        let position =
            cx.new(|cx| InputState::new(window, cx).default_value(loaded_position.clone()));
        Ok(Self {
            owner,
            gradient,
            colors,
            alphas,
            entries: entries.into(),
            position,
            loaded_position,
            ramp,
        })
    }
    pub(super) fn render_ramp(
        &self,
        id: impl Into<gpui_kit::ElementId>,
        label: &'static str,
        cx: &App,
    ) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        match &self.ramp {
            Ok(samples) => div()
                .id(id)
                .role(gpui_kit::Role::Image)
                .aria_label(label)
                .test_support()
                .h_8()
                .w_full()
                .flex()
                .overflow_hidden()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.muted)
                .children(
                    samples
                        .iter()
                        .copied()
                        .map(|color| div().flex_1().h_full().bg(gpui_kit::rgba(color))),
                )
                .into_any_element(),
            Err(message) => div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(message.clone())
                .into_any_element(),
        }
    }

    pub(crate) fn animation(
        &self,
    ) -> Option<(&AnimationParameter, &GradientAnimation<ForegroundError>)> {
        match &self.owner {
            GradientOwner::Palette(parameter, animation) => Some((parameter, animation)),
            GradientOwner::Colorize(_) | GradientOwner::Environment(..) | GradientOwner::Draft => {
                None
            }
        }
    }
    pub(crate) fn is_animation_palette(&self) -> bool {
        self.animation().is_some()
    }
    pub fn is_changed(&self, cx: &App) -> bool {
        self.position.read(cx).value() != self.loaded_position
    }
    fn validate(&self) -> Result<(), String> {
        let current = match &self.owner {
            GradientOwner::Colorize(parameter) => match parameter.value() {
                PointwiseParameterValue::Gradient(current) => current,
                _ => None,
            },
            GradientOwner::Environment(panel, ..) => {
                panel.validate()?;
                Some(self.gradient.clone())
            }
            GradientOwner::Draft => Some(self.gradient.clone()),
            GradientOwner::Palette(parameter, animation) => match parameter.retained()? {
                Some(AnimationOwner::Legacy(current))
                    if current.same_reference(animation)
                        && current.is_active().map_err(|e| e.to_string())? =>
                {
                    current.retained_gradient()
                }
                _ => None,
            },
        };
        if !current.is_some_and(|current| current.same_reference(&self.gradient)) {
            return Err("The gradient changed. Revert fields before continuing.".into());
        }
        if !self
            .gradient
            .color_points()
            .is_some_and(|v| v.same_reference(&self.colors))
            || !self
                .gradient
                .alpha_points()
                .is_some_and(|v| v.same_reference(&self.alphas))
        {
            return Err("The gradient stop lists changed. Revert fields before continuing.".into());
        }
        Ok(())
    }
    pub(crate) fn copy_stop(
        &self,
        point: &GradientEntry,
        copy: &SharedGradient,
    ) -> Result<GradientEntry, String> {
        let index = self
            .gradient
            .point_reference_index(point)
            .map_err(|e| e.to_string())?;
        if index < 0 {
            return Err("The gradient stop changed. Revert fields before continuing.".into());
        }
        let points = match point {
            GradientEntry::Color(_) => copy.color_points(),
            GradientEntry::Alpha(_) => copy.alpha_points(),
            _ => return Err("The gradient stop is unsupported.".into()),
        };
        points
            .ok_or("The gradient stop list is unavailable.")?
            .entry(index)
            .map_err(|e| e.to_string())
    }
    fn validate_point(&self, point: &GradientEntry) -> Result<(), String> {
        self.validate()?;
        if self
            .gradient
            .point_reference_index(point)
            .map_err(|e| e.to_string())?
            < 0
        {
            return Err("The gradient stop changed. Revert fields before continuing.".into());
        }
        Ok(())
    }
    pub fn stop_value(&self, point: &GradientEntry, component: usize) -> Result<String, String> {
        self.validate_point(point)?;
        match point {
            GradientEntry::Color(p) => {
                if component == 0 {
                    Ok(p.coordinate().to_string())
                } else {
                    p.color()
                        .argb()
                        .get(component)
                        .map(ToString::to_string)
                        .ok_or("The color component is unavailable.".into())
                }
            }
            GradientEntry::Alpha(p) => match component {
                0 => Ok(p.coordinate().to_string()),
                1 => Ok(p.value().to_string()),
                _ => Err("The opacity component is unavailable.".into()),
            },
            _ => Err("The gradient stop is unsupported.".into()),
        }
    }
    pub fn set_stop_value(
        &self,
        point: &GradientEntry,
        component: usize,
        text: &str,
    ) -> Result<(), String> {
        self.validate_point(point)?;
        Self::write_stop(point, component, text)?;
        self.gradient.rebuild().map_err(|e| e.to_string())
    }
    pub(crate) fn write_stop(
        point: &GradientEntry,
        component: usize,
        text: &str,
    ) -> Result<(), String> {
        if component == 0 {
            let value = text
                .parse::<i32>()
                .map_err(|_| "Position needs a whole number.")?;
            match point {
                GradientEntry::Color(p) => p.set_coordinate(value),
                GradientEntry::Alpha(p) => p.set_coordinate(value),
                _ => return Err("The gradient stop is unsupported.".into()),
            }
        } else {
            let value = text
                .parse::<f64>()
                .map_err(|_| "Stop values need finite numbers.")?;
            if !value.is_finite() {
                return Err("Stop values need finite numbers.".into());
            }
            match point {
                GradientEntry::Color(p) if component < 4 => {
                    let old = p.color();
                    let mut argb = old.argb();
                    argb[component] = value;
                    p.set_color(RgbColor::from_argb(argb).grayscale(old.is_grayscale()));
                }
                GradientEntry::Alpha(p) if component == 1 => p.set_value(value),
                _ => return Err("The gradient component is unavailable.".into()),
            }
        }
        Ok(())
    }
    fn same_entries(&self) -> Result<bool, String> {
        self.validate()?;
        let current = Self::read_entries(&self.colors, &self.alphas, self.is_animation_palette())?;
        Ok(current.len() == self.entries.len()
            && current
                .iter()
                .zip(self.entries.iter())
                .all(|(a, b)| a.same_reference(b)))
    }
    fn read_entries(
        colors: &GradientPointList,
        alphas: &GradientPointList,
        palette: bool,
    ) -> Result<Vec<GradientEntry>, String> {
        let mut entries = Vec::new();
        for list in std::iter::once(colors).chain((!palette).then_some(alphas)) {
            for index in 0..list.len() {
                entries.push(list.entry(index as i32).map_err(|e| e.to_string())?);
            }
        }
        Ok(entries)
    }
}
#[derive(Clone)]
enum Action {
    AddColor,
    AddOpacity,
    Remove(GradientEntry),
}
impl Action {
    fn apply(
        self,
        editor: &GradientEditor,
        gradient: &SharedGradient,
        position: i32,
        detached: bool,
    ) -> Result<(), String> {
        if matches!(self, Self::AddColor | Self::AddOpacity) {
            gradient.rebuild().map_err(|e| e.to_string())?;
        }
        match self {
            Self::AddColor => gradient.insert_sampled_color(position).map(|_| ()),
            Self::AddOpacity => gradient.insert_sampled_alpha(position).map(|_| ()),
            Self::Remove(point) => {
                editor.validate_point(&point)?;
                let point = if detached {
                    editor.copy_stop(&point, gradient)?
                } else {
                    point
                };
                match point {
                    GradientEntry::Color(point) => gradient.remove_color(Some(point)),
                    GradientEntry::Alpha(point) => gradient.remove_alpha(Some(point)),
                    _ => return Err("The gradient stop is unsupported.".into()),
                }
            }
        }
        .map_err(|e| e.to_string())
    }
}

impl GraphDocumentView {
    fn graph_gradient_action(
        &mut self,
        editor: &GradientEditor,
        target: (&Option<String>, &Option<super::model::Node>, u64),
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (selection, node, revision) = target;
        if self.closed || self.is_busy() || !self.is_original_selection(selection, node, revision) {
            return;
        }
        let environment = editor.environment();
        if let Some((panel, index, _)) = environment
            && !self.inspector.environment.as_ref().is_some_and(|current| {
                current.same_source(panel)
                    && Arc::ptr_eq(current.value(), panel.value())
                    && current.selected() == Some(index)
            })
        {
            return;
        }
        if self.inspector.fields_changed(cx)
            || (environment.is_some() && self.style.as_ref().is_some_and(|panel| panel.pending(cx)))
            || self
                .inspector
                .custom
                .as_ref()
                .is_some_and(|d| d.is_changed(cx))
        {
            self.error = Some("Correct or reset field text before changing gradient stops.".into());
            cx.notify();
            return;
        }
        if !editor.same_entries().unwrap_or(false) {
            self.error =
                Some("The gradient stops changed. Revert fields before continuing.".into());
            cx.notify();
            return;
        }
        if let Some((_, _, kind)) = environment {
            let other = match kind {
                EnvironmentGradientKind::Recolor => self.inspector.mask_gradient.as_ref(),
                EnvironmentGradientKind::Mask => self.inspector.gradient.as_ref(),
            };
            if other.is_some_and(|draft| draft.is_changed(cx)) {
                self.error =
                    Some("Add the other gradient stop or revert its position first.".into());
                cx.notify();
                return;
            }
        }
        let position = if matches!(action, Action::AddColor | Action::AddOpacity) {
            match editor.position.read(cx).value().parse::<i32>() {
                Ok(v) => v,
                Err(_) => {
                    self.error = Some("New stop position needs a whole number.".into());
                    cx.notify();
                    return;
                }
            }
        } else {
            if editor.is_changed(cx) {
                self.error = Some("Add a gradient stop or revert the new position first.".into());
                cx.notify();
                return;
            }
            0
        };
        if let Some((panel, index, kind)) = environment {
            let result = (|| {
                let gradient = editor.copy_for_edit()?;
                action.apply(editor, &gradient, position, true)?;
                gradient.rebuild().map_err(|e| e.to_string())?;
                let mut map = panel.value().as_ref().clone();
                kind.store(&mut map, index, &gradient)
                    .map_err(|e| e.to_string())?;
                Ok(map)
            })();
            match result {
                Ok(map) => self.apply_environment_change(panel, map, index, window, cx),
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                }
            }
            return;
        }
        if editor.is_colorize() {
            let result = (|| {
                let gradient = editor.copy_for_edit()?;
                action.apply(editor, &gradient, position, true)?;
                Ok(gradient)
            })();
            match result {
                Ok(gradient) => {
                    self.ensure_history_binding();
                    let stamp = self.begin_history_edit();
                    self.error = editor.commit_colorize(gradient, &self.document).err();
                    if self.error.is_none() {
                        self.finish_history_edit(stamp, true);
                        self.status = None;
                        self.revision += 1;
                        self.rebuild(window, cx);
                        self.start_preview(cx);
                    }
                }
                Err(error) => self.error = Some(error),
            }
            cx.notify();
            return;
        }
        let stamp = self.begin_history_edit();
        self.revision += 1;
        let revision = self.revision;
        self.status = None;
        let result = (|| -> Result<(), String> {
            let edit = match editor.animation() {
                Some((parameter, animation)) => {
                    parameter.animation_edit(Some(&AnimationOwner::Legacy(animation.clone())))?
                }
                None => None,
            };
            let gradient = match &edit {
                Some(edit) => edit.palette()?,
                None => editor.gradient.clone(),
            };
            action.apply(editor, &gradient, position, edit.is_some())?;
            if let Some(mut edit) = edit {
                editor.validate()?;
                edit.mark_palette_changed();
                if let Some(layer) =
                    edit.apply_staged(self.inspector.layer.as_ref(), &self.document)?
                {
                    self.inspector.layer = Some(layer);
                }
            } else {
                gradient.rebuild().map_err(|e| e.to_string())?;
                editor.validate()?;
            }
            Ok(())
        })();
        if !self.is_original_selection(selection, node, revision) {
            self.error = Some(result.err().unwrap_or_else(|| {
                "The selected node changed while editing. Earlier changes were kept.".into()
            }));
            self.retire_stale_preview();
            cx.notify();
            return;
        }
        if let Err(error) = result {
            self.error = Some(error);
            self.start_preview(cx);
            cx.notify();
            return;
        }
        self.error = None;
        self.finish_history_edit(stamp, editor.is_animation_palette());
        self.rebuild(window, cx);
        self.start_preview(cx);
    }
    pub(super) fn render_graph_gradient(
        &self,
        editor: &GradientEditor,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let selection = self.selection.clone();
        let node = self.inspector.owner.clone();
        let revision = self.revision;
        let owner = cx.entity().downgrade();
        let callback = |action: Action| {
            let selection = selection.clone();
            let node = node.clone();
            let editor = editor.clone();
            let owner = owner.clone();
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                let _ = owner.update(cx, |view, cx| {
                    view.graph_gradient_action(
                        &editor,
                        (&selection, &node, revision),
                        action.clone(),
                        window,
                        cx,
                    )
                });
            }
        };
        let entries: Vec<_> = editor
            .entries
            .iter()
            .enumerate()
            .map(|(index, point)| {
                let label = match point {
                    GradientEntry::Color(p) => format!("Color {} · {}", index + 1, p.coordinate()),
                    GradientEntry::Alpha(p) => {
                        format!("Opacity {} · {}", index + 1, p.coordinate())
                    }
                    _ => format!("Stop {}", index + 1),
                };
                (label, callback(Action::Remove(point.clone())))
            })
            .collect();
        let add_color = callback(Action::AddColor);
        let add_alpha = callback(Action::AddOpacity);
        let label = if editor.is_animation_palette() {
            "Color animation palette"
        } else if editor.is_mask() {
            "Mask gradient colors and opacity"
        } else {
            "Applied gradient colors and opacity"
        };
        let ramp = editor.render_ramp(editor.control_id("ramp"), label, cx);
        let adjuster = self.render_gradient_adjuster(editor, disabled, cx);
        div()
            .v_flex()
            .gap_2()
            .when(editor.is_mask(), |group| {
                group.child(div().font_semibold().child("Mask gradient"))
            })
            .child(ramp)
            .children(adjuster)
            .child(
                Field::new().label("New stop position").child(
                    NumericField::new(
                        &editor.position,
                        editor.control_id("new-position"),
                        "New gradient stop position",
                        NumericRange::new(
                            0.0,
                            f64::from(editor.width().saturating_sub(1).max(0)),
                            1.0,
                        ),
                    )
                    .disabled(disabled),
                ),
            )
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new(editor.control_id("add-color"))
                            .label("Add color stop")
                            .disabled(disabled)
                            .on_click(add_color),
                    )
                    .when(!editor.is_animation_palette(), |row| {
                        row.child(
                            Button::new(editor.control_id("add-opacity"))
                                .label("Add opacity stop")
                                .disabled(disabled)
                                .on_click(add_alpha),
                        )
                    })
                    .child(
                        Button::new(editor.control_id("remove"))
                            .label("Remove stop")
                            .disabled(disabled || entries.is_empty())
                            .dropdown_menu(move |mut menu, _, _| {
                                for (name, cb) in &entries {
                                    menu = menu.item(
                                        PopupMenuItem::new(name.clone()).on_click(cb.clone()),
                                    );
                                }
                                menu.scrollable(true)
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_gradient_adjuster(
        &self,
        editor: &GradientEditor,
        disabled: bool,
        cx: &Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        let GradientOwner::Colorize(property) = &editor.owner else {
            return None;
        };
        if !property.is_adjustable() {
            return None;
        }
        let (linked, choices) = {
            let document = guard(&self.document);
            (
                document.gradient_adjuster(property),
                document
                    .color_lab_adjusters(Some(&property.owner().node()))
                    .into_iter()
                    .filter(|control| control.type_label() == "Gradient")
                    .collect::<Vec<_>>(),
            )
        };
        let has_link = property
            .retained_adjustment()
            .is_some_and(|link| link.is_active().unwrap_or(false));
        let label = match &linked {
            Some(link) => format!("Adjuster · {}", link.name()),
            None if has_link => "Missing Gradient adjuster".into(),
            None => "Link Gradient adjuster".into(),
        };
        let selection = self.selection.clone();
        let node = self.inspector.owner.clone();
        let revision = self.revision;
        let editor = editor.clone();
        let change = move |view: &mut Self, link, window: &mut Window, cx: &mut Context<Self>| {
            if view.closed
                || view.work.is_some()
                || view.has_field_drafts(cx)
                || !view.is_original_selection(&selection, &node, revision)
            {
                return Err("The selected Gradient changed. Reopen its adjuster menu.".into());
            }
            editor.validate()?;
            let GradientOwner::Colorize(property) = &editor.owner else {
                return Err("The selected Gradient is unavailable.".into());
            };
            view.commit_lab_adjuster_change(window, cx, |document| {
                document.set_gradient_adjuster(property, link)
            })
        };
        Some(self.render_color_adjuster_menu(
            ColorAdjusterMenu {
                id: "graph-gradient-adjuster",
                kind: AdjusterNameKind::Gradient,
                label,
                linked,
                choices,
                has_link,
                disabled,
                edit_linked: false,
            },
            change,
            cx,
        ))
    }

    pub(super) fn render_color_adjuster_menu(
        &self,
        menu: ColorAdjusterMenu,
        change: impl Fn(
            &mut Self,
            GradientAdjusterLink,
            &mut Window,
            &mut Context<Self>,
        ) -> Result<(), String>
        + Clone
        + 'static,
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        let owner = cx.weak_entity();
        let ColorAdjusterMenu {
            id,
            kind,
            label,
            linked,
            choices,
            has_link,
            disabled,
            edit_linked,
        } = menu;
        let edit_target =
            edit_linked.then(|| (self.selection.clone(), self.selected_node(), self.revision));
        let focus = self.focus.clone();
        Button::new(id)
            .small()
            .ghost()
            .when(edit_linked, |button| {
                button
                    .min_w_0()
                    .max_w_full()
                    .tab_index(0)
                    .tooltip(label.clone())
            })
            .label(label)
            .disabled(disabled || self.has_field_drafts(cx))
            .dropdown_menu(move |mut menu, _, _| {
                if edit_linked {
                    menu = menu.action_context(focus.clone());
                }
                let create_owner = owner.clone();
                let create = change.clone();
                menu = menu.item(PopupMenuItem::new("New adjuster…").on_click(
                    move |_, window, cx| {
                        let create = create.clone();
                        let _ = create_owner.update(cx, |view, cx| {
                            if !view.is_busy() {
                                view.open_adjuster_name(
                                    kind,
                                    move |view, name, window, cx| {
                                        create(view, GradientAdjusterLink::Create(name), window, cx)
                                    },
                                    window,
                                    cx,
                                );
                            }
                        });
                    },
                ));
                let command = |item: PopupMenuItem, link: GradientAdjusterLink| {
                    let owner = owner.clone();
                    let change = change.clone();
                    item.on_click(move |_, window, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            view.error = change(view, link.clone(), window, cx).err();
                            cx.notify();
                        });
                    })
                };
                for control in &choices {
                    let checked = linked
                        .as_ref()
                        .is_some_and(|link| link.same_reference(control));
                    menu = menu.item(command(
                        PopupMenuItem::new(control.name().to_owned()).checked(checked),
                        GradientAdjusterLink::Existing(control.clone()),
                    ));
                }
                if has_link {
                    menu = menu.separator();
                    if let Some((selection, node, revision)) = &edit_target
                        && let Some(control) = &linked
                    {
                        let owner = owner.clone();
                        let selection = selection.clone();
                        let node = node.clone();
                        let revision = *revision;
                        let control = control.clone();
                        menu = menu.item(PopupMenuItem::new("Edit adjuster…").on_click(
                            move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    if view.is_original_selection(&selection, &node, revision) {
                                        view.open_color_lab_adjuster(
                                            node.clone(),
                                            control.clone(),
                                            window,
                                            cx,
                                        );
                                    }
                                });
                            },
                        ));
                    }
                    menu = menu.item(command(
                        PopupMenuItem::new("Unlink adjuster"),
                        GradientAdjusterLink::Unlink,
                    ));
                }
                menu.scrollable(true)
            })
            .into_any_element()
    }
}
