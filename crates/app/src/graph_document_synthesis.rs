//! Synthesis layers and controls in the graph inspector.
use super::*;
use gpui_kit::{AnyElement, WeakEntity};
use mycela_engine::{
    EnvironmentBlendMode, EnvironmentImage, ImageGeometry, LabAdjusterMetadata, SynthesisAdjuster,
    SynthesisEdit, SynthesisField, SynthesisGeometry, SynthesisLayerKind, SynthesisNumberField,
    SynthesisPairField,
};

#[cfg(test)]
#[path = "graph_document_synthesis_adjuster_ui_tests.rs"]
mod adjuster_ui_tests;
#[path = "graph_document_synthesis_adjusters.rs"]
mod adjusters;
#[path = "graph_document_synthesis_colors.rs"]
mod colors;
#[path = "graph_document_synthesis_export.rs"]
mod export;
#[path = "graph_document_synthesis_geometry.rs"]
mod geometry;
#[path = "graph_document_synthesis_pairs.rs"]
mod pairs;
#[path = "graph_document_synthesis_patch_list.rs"]
mod patch_list;
#[path = "graph_document_synthesis_presets.rs"]
mod presets;
#[path = "graph_document_synthesis_preview.rs"]
mod preview;
#[path = "graph_document_synthesis_source.rs"]
mod source;
use geometry::GeometryCommand;
use patch_list::{PatchList, PatchListView};
pub(super) use preview::{
    PreviewMode, PreviewState, TextureContext, render_synthesis_preview_frame,
};
pub(super) use source::{PatchInputRequests, PatchInputSizes};

pub(super) type Location = (Option<usize>, Option<SynthesisGeometry>);
type FieldDraft = (SynthesisField, Entity<InputState>);
type Fields = Vec<FieldDraft>;

pub(super) fn bind_patch_list_keys(cx: &mut App) {
    PatchList::bind_keys(cx);
    geometry::manual::bind_keys(cx);
}

fn is_blend_slider(field: SynthesisField) -> bool {
    matches!(
        field.number_field(),
        Some(
            SynthesisNumberField::BlendOpacity
                | SynthesisNumberField::BlendErode
                | SynthesisNumberField::BlendSharpness
        )
    )
}

enum PhotoShaping {
    Fold,
    Star,
    Repeat,
}

fn photo_shaping(field: SynthesisField) -> Option<PhotoShaping> {
    if matches!(
        field.pair_field(),
        Some((SynthesisPairField::PhotoRepeat, _))
    ) {
        return Some(PhotoShaping::Repeat);
    }
    match field.number_field() {
        Some(
            SynthesisNumberField::PhotoStretchX
            | SynthesisNumberField::PhotoStretchY
            | SynthesisNumberField::PhotoFeatherX
            | SynthesisNumberField::PhotoFeatherY,
        ) => Some(PhotoShaping::Fold),
        Some(SynthesisNumberField::PhotoStarFeather) => Some(PhotoShaping::Star),
        _ => None,
    }
}

fn is_manual_repeat(field: SynthesisField) -> bool {
    matches!(
        field.pair_field(),
        Some((SynthesisPairField::ManualRepeat, _))
    )
}

pub(super) struct Panel {
    pub(super) node: Node,
    edit: SynthesisEdit,
    layer: Option<usize>,
    fields: Fields,
    layer_color_open: bool,
    geometry: Option<SynthesisGeometry>,
    geometry_fields: Fields,
    editing_material: Option<SynthesisAdjuster>,
    lab_material: Option<LabMaterialContext>,
    drag: Option<geometry::Drag>,
    curve_hover: Option<geometry::GuideHover>,
    grid_selection: geometry::GridSelection,
    patch_list: PatchList,
    patch_transform_mode: bool,
    patch_numbers: Option<geometry::TransformNumbers>,
    manual: Option<geometry::manual::Inline>,
    source_focus: FocusHandle,
    source_navigation: geometry::SourceNavigation,
    guide_source: Option<source::Preview>,
}

#[derive(Clone)]
struct LabMaterialContext {
    selection: Option<String>,
    selected: Option<Node>,
    return_location: Option<Location>,
}

impl LabMaterialContext {
    fn current(&self, view: &GraphDocumentView) -> bool {
        view.selection == self.selection && view.check_lab_selection(self.selected.as_ref()).is_ok()
    }
}

impl Panel {
    pub(super) fn inputs(&self) -> impl Iterator<Item = Entity<InputState>> + '_ {
        self.fields
            .iter()
            .chain(&self.geometry_fields)
            .map(|(_, input)| input.clone())
    }
    pub(super) fn new(
        node: Node,
        edit: SynthesisEdit,
        (layer, geometry): Location,
        window: &mut Window,
        cx: &mut Context<GraphDocumentView>,
    ) -> Self {
        let geometry = if layer.is_some_and(|layer| edit.crop_rectangle(layer).is_some()) {
            Some(SynthesisGeometry::Crop)
        } else {
            geometry
        };
        let geometry = match layer.zip(geometry) {
            Some((layer, SynthesisGeometry::Patch { selection, knot })) => edit
                .patch_knots(layer, selection)
                .map(|points| SynthesisGeometry::Patch {
                    selection,
                    knot: knot.min(points.len().saturating_sub(1)),
                }),
            _ => geometry,
        };
        let fields = new_fields(edit.fields(layer), window, cx);
        let geometry_fields = layer
            .zip(geometry)
            .filter(|(_, geometry)| !matches!(geometry, SynthesisGeometry::Placement(_)))
            .map_or_else(Vec::new, |(layer, geometry)| {
                edit.geometry_fields(layer, geometry)
            });
        let geometry_fields = new_fields(geometry_fields, window, cx);
        let geometry = geometry.filter(|geometry| match geometry {
            SynthesisGeometry::Placement(index) => layer
                .and_then(|layer| edit.placement_geometry(layer, *index))
                .is_some(),
            _ => !geometry_fields.is_empty(),
        });
        let patch_list = PatchList::new(cx);
        patch_list.reveal(PatchList::selected(geometry));
        Self {
            node,
            edit,
            layer,
            fields,
            layer_color_open: false,
            geometry,
            geometry_fields,
            editing_material: None,
            lab_material: None,
            drag: None,
            curve_hover: None,
            grid_selection: geometry::GridSelection::new(geometry),
            patch_list,
            patch_transform_mode: false,
            patch_numbers: None,
            manual: None,
            source_focus: cx.focus_handle(),
            source_navigation: geometry::SourceNavigation::default(),
            guide_source: None,
        }
    }

    fn with_material(mut self, material: Option<SynthesisAdjuster>) -> Self {
        self.editing_material = material;
        self
    }

    fn keep_material_editor(self, panel: &Self) -> Self {
        self.with_material_context(panel.editing_material.clone(), panel.lab_material.clone())
    }

    fn with_material_context(
        mut self,
        material: Option<SynthesisAdjuster>,
        lab: Option<LabMaterialContext>,
    ) -> Self {
        self.editing_material = material;
        self.lab_material = lab;
        self
    }

    pub(super) fn selection_in(&self, edit: &SynthesisEdit) -> Option<usize> {
        let old = self.layer?;
        let id = self.edit.layer(old)?.0;
        (0..edit.layers())
            .find(|index| edit.layer(*index).is_some_and(|item| item.0 == id))
            .or_else(|| edit.layers().checked_sub(1).map(|last| old.min(last)))
    }

    pub(super) fn geometry_in(
        &self,
        edit: &SynthesisEdit,
        layer: Option<usize>,
    ) -> Option<SynthesisGeometry> {
        let old = self.edit.layer(self.layer?)?.0;
        (edit.layer(layer?)?.0 == old)
            .then_some(self.geometry)
            .flatten()
    }

    pub(super) fn pending(&self, cx: &App) -> bool {
        self.fields
            .iter()
            .chain(&self.geometry_fields)
            .any(|(field, input)| field_value(*field, input, cx) != Some(field.value()))
    }

    fn automatic_placement_off(&self) -> bool {
        self.layer
            .and_then(|layer| self.edit.automatic_placement(layer))
            == Some(false)
    }

    fn has_layer_source(&self) -> bool {
        self.layer
            .is_some_and(|layer| self.edit.layer_has_source(layer))
    }

    fn photo_blend_is_fold(&self) -> Option<bool> {
        self.edit.photo_blend_is_fold(self.layer?)
    }

    fn staged(
        &self,
        input_geometry: Option<mycela_engine::ImageGeometry>,
        cx: &App,
    ) -> Result<SynthesisEdit, String> {
        let mut edit = self.edit.clone();
        apply_fields(&self.fields, cx, |values| {
            edit.set_fields(self.layer, values)
        })?;
        if let Some((layer, geometry)) = self.layer.zip(self.geometry) {
            apply_fields(&self.geometry_fields, cx, |values| {
                edit.set_geometry_fields_with_input_geometry(
                    layer,
                    geometry,
                    values,
                    input_geometry,
                )
            })?;
        }
        Ok(edit)
    }
}

fn new_fields(
    fields: Vec<SynthesisField>,
    window: &mut Window,
    cx: &mut Context<GraphDocumentView>,
) -> Fields {
    fields
        .into_iter()
        .map(|field| {
            let [min, max] = field.range();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(field_text(field))
                    .min(min)
                    .max(max)
                    .step(if field.is_whole() { 1. } else { 0.01 })
            });
            (field, input)
        })
        .collect()
}

fn apply_fields(
    fields: &Fields,
    cx: &App,
    apply: impl FnOnce(&[f64]) -> Result<(), &'static str>,
) -> Result<(), String> {
    let values = fields
        .iter()
        .map(|(field, input)| {
            field_value(*field, input, cx)
                .ok_or_else(|| format!("Enter a number for {}.", field.label()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    apply(&values).map_err(|label| {
        fields
            .iter()
            .find(|(field, _)| field.label() == label)
            .map_or_else(
                || label.to_owned(),
                |(field, _)| {
                    let [min, max] = field.range();
                    let unit = if field.is_whole() {
                        "a whole number"
                    } else {
                        "a number"
                    };
                    format!("Enter {unit} between {min} and {max} for {label}.")
                },
            )
    })
}

fn field_text(field: SynthesisField) -> String {
    number_text(field.value())
}

fn number_text(value: f64) -> String {
    let single = value as f32;
    if f64::from(single) == value {
        single.to_string()
    } else {
        value.to_string()
    }
}

fn field_value(field: SynthesisField, input: &Entity<InputState>, cx: &App) -> Option<f64> {
    let text = input.read(cx).value();
    let text = text.trim();
    let parsed = text.parse::<f64>().ok()?;
    if parsed == field.value() || text == field_text(field) {
        Some(field.value())
    } else {
        Some(parsed)
    }
}

#[derive(Clone)]
struct Target {
    key: Option<String>,
    node: Node,
    revision: u64,
    layer: Option<usize>,
    geometry: Option<SynthesisGeometry>,
    style: Option<style::Target>,
    linked_material: bool,
    editing_material: Option<SynthesisAdjuster>,
    lab_material: bool,
}

impl Target {
    fn new(view: &GraphDocumentView, panel: &Panel) -> Self {
        let lab_material = panel.lab_material.is_some();
        let style = if lab_material {
            None
        } else {
            view.current_style_target()
        };
        let linked_material = if lab_material {
            true
        } else if style.is_some() {
            view.style
                .as_ref()
                .and_then(style::Panel::synthesis_style_draft)
                .is_some_and(|(style, pass)| style.is_synthesis_adjusted(pass))
        } else {
            panel
                .node
                .filter()
                .is_some_and(|filter| filter.is_synthesis_adjusted())
        };
        Self {
            key: view.selection.clone(),
            node: panel.node.clone(),
            revision: view.revision,
            layer: panel.layer,
            geometry: panel.geometry,
            style,
            linked_material,
            editing_material: panel.editing_material.clone(),
            lab_material,
        }
    }

    fn current(&self, view: &GraphDocumentView) -> bool {
        let current_owner = if self.lab_material {
            self.key == view.selection
                && self.revision == view.revision
                && view
                    .synthesis
                    .as_ref()
                    .and_then(|panel| panel.lab_material.as_ref())
                    .is_some_and(|origin| origin.current(view))
        } else {
            view.is_original_selection(&self.key, &Some(self.node.clone()), self.revision)
        };
        current_owner
            && self
                .style
                .as_ref()
                .is_none_or(|target| target.current(view))
            && view.synthesis.as_ref().is_some_and(|panel| {
                panel.layer == self.layer
                    && panel.geometry == self.geometry
                    && panel.node.same_reference(&self.node)
                    && (panel.lab_material.is_some() == self.lab_material)
                    && match (&self.editing_material, &panel.editing_material) {
                        (None, None) => true,
                        (Some(left), Some(right)) => left.same_reference(right),
                        _ => false,
                    }
            })
    }

    fn material_linked(&self) -> bool {
        self.linked_material
    }

    fn material_locked(&self) -> bool {
        self.material_linked() && self.editing_material.is_none()
    }
}

#[derive(Clone, Copy)]
enum Command {
    DoneMaterial,
    Select(Option<usize>),
    Add(SynthesisLayerKind),
    Type(SynthesisLayerKind),
    Move(usize),
    Delete,
    Clear,
    UseInput,
    ClearSource,
    Equalize(bool),
    Automatic(bool),
    Effect(usize),
    Geometry(GeometryCommand),
    Mode(EnvironmentBlendMode),
    ErodeMode(usize),
    Open(FileKind),
    BrowsePresets,
    SavePreset,
    Apply,
}

#[derive(Clone, Copy)]
enum FileKind {
    Photo,
    Preset { insert: bool },
}

fn menu_item(
    label: impl Into<gpui_kit::SharedString>,
    checked: bool,
    action: Command,
    target: &Target,
    owner: &WeakEntity<GraphDocumentView>,
) -> PopupMenuItem {
    let target = target.clone();
    let owner = owner.clone();
    PopupMenuItem::new(label)
        .checked(checked)
        .on_click(move |_, window, cx| {
            let _ = owner.update(cx, |view, cx| {
                view.synthesis_command(&target, action, window, cx)
            });
        })
}

fn synthesis_menu(
    button: Button,
    disabled: bool,
    build: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
) -> AnyElement {
    if disabled {
        button.disabled(true).into_any_element()
    } else {
        button.dropdown_menu(build).into_any_element()
    }
}

impl GraphDocumentView {
    pub(super) fn lab_material_editor_open(&self) -> bool {
        self.synthesis
            .as_ref()
            .is_some_and(|panel| panel.lab_material.is_some())
    }

    pub(super) fn open_lab_material(
        &mut self,
        selected: Option<&Node>,
        entry: &LabAdjusterMetadata,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() || self.check_lab_selection(selected).is_err() {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before editing the Material.".into());
            cx.notify();
            return;
        }
        let target = {
            let document = guard(&self.document);
            let node = selected
                .cloned()
                .or_else(|| document.branches().iter().flatten().next().cloned());
            node.zip(document.lab_synthesis_adjuster(selected, entry))
        };
        let Some((node, material)) = target else {
            self.error = Some("This Material is no longer available in this Lab section.".into());
            cx.notify();
            return;
        };
        let edit = material.synthesis_edit();
        let location = ((edit.layers() > 0).then_some(0), None);
        let context = LabMaterialContext {
            selection: self.selection.clone(),
            selected: selected.cloned(),
            return_location: self
                .synthesis
                .as_ref()
                .map(|panel| (panel.layer, panel.geometry)),
        };
        let panel = Panel::new(node, edit, location, window, cx)
            .with_material_context(Some(material), Some(context));
        self.error = None;
        self.install_synthesis_panel(Some(panel), window, cx);
        self.refresh_synthesis_guide_source(cx);
        self.refresh_synthesis_preview(cx);
        self.observe_drafts(cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_lab_material_heading(&self) -> Option<AnyElement> {
        let panel = self.synthesis.as_ref()?;
        panel.lab_material.as_ref()?;
        let material = panel.editing_material.as_ref()?;
        let scope = format!(
            "Shared Material · {}",
            String::from_utf16_lossy(material.name())
        );
        Some(
            div()
                .id("synthesis-shared-material-scope")
                .aria_label(scope.clone())
                .test_support()
                .font_semibold()
                .child(scope)
                .into_any_element(),
        )
    }

    pub(super) fn reload_synthesis(
        &mut self,
        location: Option<Location>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(panel) = self.synthesis.as_ref()
            && let Some(origin) = panel
                .lab_material
                .as_ref()
                .filter(|origin| origin.current(self))
            && let Some(material) = panel.editing_material.as_ref()
        {
            let material = guard(&self.document)
                .active_lab_synthesis_adjuster(origin.selected.as_ref(), material);
            if let Some(material) = material {
                let edit = material.synthesis_edit();
                let location = location.unwrap_or_else(|| {
                    let selected = panel.selection_in(&edit);
                    (selected, panel.geometry_in(&edit, selected))
                });
                let next = Panel::new(panel.node.clone(), edit, location, window, cx)
                    .with_material_context(Some(material), Some(origin.clone()));
                self.install_synthesis_panel(Some(next), window, cx);
                self.refresh_synthesis_guide_source(cx);
                self.refresh_synthesis_preview(cx);
                return;
            }
        }
        let next = self.inspector.owner.as_ref().and_then(|node| {
            let filter = node.filter()?;
            let edit = filter
                .synthesis_edit()
                .or_else(|| self.style.as_ref()?.synthesis_draft())?;
            let previous = self
                .synthesis
                .as_ref()
                .filter(|panel| panel.lab_material.is_none() && panel.node.same_reference(node));
            let location = location.unwrap_or_else(|| {
                let selected = previous.map_or_else(
                    || (edit.layers() > 0).then_some(0),
                    |panel| panel.selection_in(&edit),
                );
                (
                    selected,
                    previous.and_then(|panel| panel.geometry_in(&edit, selected)),
                )
            });
            let material = previous
                .and_then(|panel| panel.editing_material.as_ref())
                .filter(|material| {
                    self.selected_synthesis_material(node)
                        .is_some_and(|linked| linked.same_reference(material))
                })
                .cloned();
            Some(Panel::new(node.clone(), edit, location, window, cx).with_material(material))
        });
        self.install_synthesis_panel(next, window, cx);
        self.refresh_synthesis_guide_source(cx);
        self.refresh_synthesis_preview(cx);
    }

    pub(super) fn apply_synthesis_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.synthesis.as_ref() else {
            return;
        };
        let target = Target::new(self, panel);
        self.synthesis_command(&target, Command::Apply, window, cx);
    }

    fn render_synthesis_fields<'a>(
        &self,
        panel: &Panel,
        target: &Target,
        (fields, group): (impl Iterator<Item = (usize, &'a FieldDraft)>, &'static str),
        (busy, links_disabled): (bool, bool),
        form: Form,
        cx: &Context<Self>,
    ) -> Form {
        form.columns(2)
            .children(fields.map(|(index, (field, input))| {
                let links = if let Some(pair) = field.pair_field() {
                    self.render_synthesis_pair_adjusters(panel, target, pair, links_disabled, cx)
                } else {
                    self.render_synthesis_number_adjuster(
                        panel,
                        target,
                        *field,
                        group,
                        links_disabled,
                        cx,
                    )
                };
                let (linked_value, menu) =
                    links.map_or((None, None), |links| (links.value, links.menu));
                let field_value = linked_value.unwrap_or_else(|| {
                    Input::new(input)
                        .id((group, index))
                        .small()
                        .disabled(busy || panel.number_controls_disabled(*field))
                        .into_any_element()
                });
                Field::new().label(field.label()).child(
                    div()
                        .v_flex()
                        .min_w_0()
                        .gap_1()
                        .child(field_value)
                        .children(menu),
                )
            }))
    }

    fn render_synthesis_layer_blend(
        &self,
        panel: &Panel,
        target: &Target,
        (busy, disabled): (bool, bool),
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let layer = panel.layer?;
        let blend = panel.edit.blend_mode(layer);
        let erode = panel.edit.erode_mode(layer);
        let choices = |id, title, field, selected, options: Vec<(&'static str, bool, Command)>| {
            let owner = cx.weak_entity();
            let command_target = target.clone();
            let focus = self.focus.clone();
            let choice = if panel.number_linked(field) {
                adjusters::render_synthesis_readout(id, selected, selected, cx)
            } else {
                let button = Button::new(id).small().label(selected);
                synthesis_menu(button, disabled, move |mut menu, _, _| {
                    menu = menu.action_context(focus.clone());
                    for &(label, checked, command) in &options {
                        menu =
                            menu.item(menu_item(label, checked, command, &command_target, &owner));
                    }
                    menu.scrollable(true)
                })
            };
            let adjuster = panel
                .edit
                .blend_choice_field(layer, field)
                .and_then(|field| {
                    self.render_synthesis_number_adjuster(
                        panel,
                        target,
                        field,
                        "synthesis-control",
                        disabled,
                        cx,
                    )
                })
                .and_then(|controls| controls.menu);
            Field::new().label(title).child(
                div()
                    .v_flex()
                    .min_w_0()
                    .gap_1()
                    .child(choice)
                    .children(adjuster),
            )
        };
        let form = Form::new()
            .child(choices(
                "synthesis-blend-mode",
                "Blend mode",
                SynthesisNumberField::BlendMode,
                blend.map_or("Choose mode", EnvironmentBlendMode::label),
                EnvironmentBlendMode::ALL
                    .into_iter()
                    .map(|value| (value.label(), blend == Some(value), Command::Mode(value)))
                    .collect(),
            ))
            .child(choices(
                "synthesis-erode-mode",
                "Erode mode",
                SynthesisNumberField::ErodeMode,
                erode
                    .and_then(|index| SynthesisEdit::ERODE_MODES.get(index).copied())
                    .unwrap_or("Choose mode"),
                SynthesisEdit::ERODE_MODES
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| (label, erode == Some(index), Command::ErodeMode(index)))
                    .collect(),
            ));
        let fields = panel
            .fields
            .iter()
            .enumerate()
            .filter(|(_, (field, _))| is_blend_slider(*field));
        Some(
            div()
                .id("synthesis-layer-blend")
                .v_flex()
                .gap_2()
                .py_2()
                .border_t_1()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(div().text_sm().font_semibold().child("Layer blend"))
                .child(self.render_synthesis_fields(
                    panel,
                    target,
                    (fields, "synthesis-control"),
                    (busy, disabled),
                    form,
                    cx,
                ))
                .into_any_element(),
        )
    }

    fn render_synthesis_effect_mode(
        &self,
        panel: &Panel,
        target: &Target,
        disabled: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let layer = panel.layer?;
        let (label, options, selected) = panel.edit.effect_choice(layer)?;
        let owner = cx.weak_entity();
        let command_target = target.clone();
        let focus = self.focus.clone();
        let choice_disabled = disabled
            || panel.automatic_placement_off()
            || panel.number_linked(SynthesisNumberField::PatchSwitcher);
        let button = Button::new("synthesis-effect-mode")
            .small()
            .label(options.get(selected).copied().unwrap_or("Choose mode"));
        let choice = synthesis_menu(button, choice_disabled, move |mut menu, _, _| {
            menu = menu.action_context(focus.clone());
            for (index, &name) in options.iter().enumerate() {
                menu = menu.item(menu_item(
                    name,
                    index == selected,
                    Command::Effect(index),
                    &command_target,
                    &owner,
                ));
            }
            menu
        });
        let adjuster = panel.edit.patch_switcher_field(layer).and_then(|field| {
            self.render_synthesis_number_adjuster(
                panel,
                target,
                field,
                "synthesis-control",
                disabled,
                cx,
            )
            .and_then(|control| control.menu)
        });
        Some(
            div()
                .h_flex()
                .items_center()
                .flex_wrap()
                .gap_2()
                .text_sm()
                .child(label)
                .child(choice)
                .children(adjuster)
                .into_any_element(),
        )
    }

    fn render_synthesis_photo_shaping(
        &self,
        panel: &Panel,
        target: &Target,
        (fold, effect): (bool, AnyElement),
        (busy, disabled): (bool, bool),
        cx: &Context<Self>,
    ) -> AnyElement {
        let fields = panel.fields.iter().enumerate();
        let shaping = fields
            .clone()
            .filter(|(_, (field, _))| match photo_shaping(*field) {
                Some(PhotoShaping::Fold) => fold,
                Some(PhotoShaping::Star) => !fold,
                Some(PhotoShaping::Repeat) | None => false,
            });
        let repeat = fields
            .filter(|(_, (field, _))| matches!(photo_shaping(*field), Some(PhotoShaping::Repeat)));
        div()
            .id("synthesis-photo-shaping")
            .v_flex()
            .gap_2()
            .child(div().text_sm().font_semibold().child("Photo shaping"))
            .child(effect)
            .child(self.render_synthesis_fields(
                panel,
                target,
                (shaping, "synthesis-control"),
                (busy, disabled),
                Form::new(),
                cx,
            ))
            .child(self.render_synthesis_fields(
                panel,
                target,
                (repeat, "synthesis-control"),
                (busy, disabled),
                Form::new(),
                cx,
            ))
            .into_any_element()
    }

    fn render_synthesis_manual_repeat(
        &self,
        panel: &Panel,
        target: &Target,
        availability: (bool, bool),
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        panel.edit.patch_counts(panel.layer?)?;
        let repeat = panel
            .fields
            .iter()
            .enumerate()
            .filter(|(_, (field, _))| is_manual_repeat(*field));
        Some(
            div()
                .id("synthesis-manual-repeat")
                .v_flex()
                .gap_2()
                .child(div().text_sm().font_semibold().child("Manual repeat"))
                .child(self.render_synthesis_fields(
                    panel,
                    target,
                    (repeat, "synthesis-control"),
                    availability,
                    Form::new(),
                    cx,
                ))
                .into_any_element(),
        )
    }

    pub(super) fn render_shared_material_footer(
        &self,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let panel = self.synthesis.as_ref()?;
        let material = panel.editing_material.as_ref()?;
        let target = Target::new(self, panel);
        let scope = format!(
            "Shared Material · {}",
            String::from_utf16_lossy(material.name())
        );
        Some(
            div()
                .h_flex()
                .items_center()
                .justify_between()
                .when(target.lab_material, |row| row.justify_end())
                .gap_2()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(cx.theme().border)
                .when(!target.lab_material, |row| {
                    row.child(
                        div()
                            .id("synthesis-shared-material-scope")
                            .aria_label(scope.clone())
                            .test_support()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(scope),
                    )
                })
                .child(
                    Button::new("synthesis-done-material")
                        .small()
                        .label("Done")
                        .flex_shrink_0()
                        .disabled(busy || panel.pending(cx))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.synthesis_command(&target, Command::DoneMaterial, window, cx);
                        })),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_synthesis_panel(
        &self,
        busy: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let panel = self.synthesis.as_ref()?;
        let target = Target::new(self, panel);
        let busy = busy || self.style.as_ref().is_some_and(|panel| panel.pending(cx));
        let owner = cx.weak_entity();
        let pending = panel.pending(cx);
        let panel_busy = busy || pending;
        let editing_material = target.editing_material.is_some();
        let material_adjuster = (!editing_material)
            .then(|| self.render_synthesis_adjuster(&target, busy || pending, cx))
            .flatten();
        let material_locked = target.material_locked();
        let busy = busy || material_locked;
        let disabled = busy || pending;
        let count = panel.edit.layers();
        let layer = panel.layer;
        let has_source = panel.has_layer_source();
        let chosen = layer.and_then(|index| panel.edit.layer(index));
        let crop = panel.geometry == Some(SynthesisGeometry::Crop);
        let selected_label = match (layer, chosen) {
            (Some(index), Some((_, kind))) => format!("{} · {}", index + 1, kind.label()),
            _ => "Master controls".into(),
        };
        let layers: Vec<_> = (0..count)
            .filter_map(|index| panel.edit.layer(index))
            .collect();
        let choose_target = target.clone();
        let choose_owner = owner.clone();
        let add_target = target.clone();
        let add_owner = owner.clone();
        let command = |id: &'static str, label: &'static str, action: Command, disabled: bool| {
            let target = target.clone();
            let owner = owner.clone();
            Button::new(id)
                .small()
                .label(label)
                .disabled(disabled)
                .on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.synthesis_command(&target, action, window, cx)
                    });
                })
        };
        let blend = self.render_synthesis_layer_blend(panel, &target, (busy, disabled), cx);
        let has_layer_color = chosen.is_some_and(|(_, kind)| kind != SynthesisLayerKind::Solid);
        let color = (!has_layer_color || panel.layer_color_open)
            .then(|| self.render_synthesis_color(panel, &target, (busy, disabled), cx))
            .flatten();
        let (color, layer_color) = if has_layer_color {
            (None, color)
        } else {
            (color, None)
        };
        let color_shortcut = has_layer_color.then(|| {
            self.synthesis_layer_color_button(&target, panel.layer_color_open, panel_busy, cx)
        });
        let manual_repeat =
            self.render_synthesis_manual_repeat(panel, &target, (busy, disabled), cx);
        let effect = self.render_synthesis_effect_mode(panel, &target, disabled, cx);
        let (effect, photo_controls) = match (panel.photo_blend_is_fold(), effect) {
            (Some(fold), Some(effect)) => (
                None,
                Some(self.render_synthesis_photo_shaping(
                    panel,
                    &target,
                    (fold, effect),
                    (busy, disabled),
                    cx,
                )),
            ),
            (_, effect) => (effect, None),
        };
        let type_target = target.clone();
        let type_owner = owner.clone();
        let type_focus = self.focus.clone();
        let toggle = |id, label, value, action: fn(bool) -> Command, unavailable: bool| {
            let target = target.clone();
            let owner = owner.clone();
            Checkbox::new(id)
                .label(label)
                .checked(value)
                .disabled(disabled || unavailable)
                .on_click(move |checked, window, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.synthesis_command(&target, action(*checked), window, cx)
                    });
                })
        };
        let fields = self.render_synthesis_fields(
            panel,
            &target,
            (
                panel
                    .fields
                    .iter()
                    .enumerate()
                    .filter(|(index, (field, _))| {
                        (!crop || *index >= 4)
                            && !is_blend_slider(*field)
                            && photo_shaping(*field).is_none()
                            && !is_manual_repeat(*field)
                            && field.color_target().is_none()
                    }),
                "synthesis-control",
            ),
            (busy, disabled),
            Form::new(),
            cx,
        );
        let source_tools = crop
            || chosen.is_some_and(|(_, kind)| {
                matches!(
                    kind,
                    SynthesisLayerKind::Patches | SynthesisLayerKind::Repeating
                )
            });
        let source_editor = if let Some(manual) = &panel.manual {
            Some(manual.render())
        } else {
            source_tools
                .then(|| geometry::render(self, panel, &target, disabled, busy, window, cx))
                .flatten()
        };
        let theme = cx.theme();
        let preset_commands = || {
            [
                command(
                    "synthesis-open-preset",
                    "Open preset…",
                    Command::Open(FileKind::Preset { insert: false }),
                    disabled || self.inspector.is_changed(cx),
                ),
                command(
                    "synthesis-browse-presets",
                    "Browse presets…",
                    Command::BrowsePresets,
                    disabled || self.inspector.is_changed(cx),
                ),
                command(
                    "synthesis-save-preset",
                    "Save preset…",
                    Command::SavePreset,
                    disabled || self.has_field_drafts(cx),
                ),
            ]
            .map(Button::ghost)
        };
        let layer_commands = || {
            layer.map(|index| {
                div()
                    .h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        command(
                            "synthesis-move-up",
                            "Move up",
                            Command::Move(index.saturating_sub(1)),
                            disabled || index == 0,
                        )
                        .ghost(),
                    )
                    .child(
                        command(
                            "synthesis-move-down",
                            "Move down",
                            Command::Move(index + 1),
                            disabled || index + 1 >= count,
                        )
                        .ghost(),
                    )
                    .child(
                        command(
                            "synthesis-remove-layer",
                            "Remove layer",
                            Command::Delete,
                            disabled,
                        )
                        .ghost(),
                    )
            })
        };
        Some(
            div()
                .v_flex()
                .gap_2()
                .when(!editing_material || color_shortcut.is_some(), |view| {
                    view.child(
                        div()
                            .h_flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .when(!editing_material, |row| {
                                row.child(div().flex_shrink_0().font_semibold().child(
                                    if target.style.is_some() {
                                        "Texture Synthesis"
                                    } else {
                                        "Texture"
                                    },
                                ))
                            })
                            .when_some(material_adjuster, |row, menu| {
                                row.child(div().flex_1().min_w_0().child(menu))
                            })
                            .children(color_shortcut),
                    )
                })
                .children(layer_color)
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(synthesis_menu(
                            Button::new("synthesis-layer-picker")
                                .small()
                                .label(selected_label),
                            disabled || count == 0,
                            move |mut menu, _, _| {
                                menu = menu.item(menu_item(
                                    "Master controls",
                                    layer.is_none(),
                                    Command::Select(None),
                                    &choose_target,
                                    &choose_owner,
                                ));
                                for (index, (_, kind)) in layers.iter().enumerate() {
                                    menu = menu.item(menu_item(
                                        format!("{} · {}", index + 1, kind.label()),
                                        layer == Some(index),
                                        Command::Select(Some(index)),
                                        &choose_target,
                                        &choose_owner,
                                    ));
                                }
                                menu
                            },
                        ))
                        .child(synthesis_menu(
                            Button::new("synthesis-add-layer")
                                .small()
                                .label("Add layer"),
                            disabled,
                            move |mut menu, _, _| {
                                for kind in SynthesisLayerKind::ALL {
                                    menu = menu.item(menu_item(
                                        kind.label(),
                                        false,
                                        Command::Add(kind),
                                        &add_target,
                                        &add_owner,
                                    ));
                                }
                                menu
                            },
                        ))
                        .when(!source_tools, |row| row.children(preset_commands())),
                )
                .when(count == 0, |view| {
                    view.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("Add a layer to build a texture."),
                    )
                })
                .when(!source_tools, |view| view.children(layer_commands()))
                .when_some(
                    layer.and_then(|ix| panel.edit.source_label(ix)),
                    |view, label| {
                        let is_input = label == "Graph input";
                        let input_label = target
                            .style
                            .as_ref()
                            .map_or("Graph input", style::Target::texture_input_label);
                        let label = if is_input { input_label.into() } else { label };
                        view.child(
                            div()
                                .v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .when(source_tools, |header| {
                                            header.h_flex().items_center().gap_2()
                                        })
                                        .when(!source_tools, |header| header.v_flex().gap_1())
                                        .child("Source")
                                        .child(
                                            div().text_color(theme.muted_foreground).child(label),
                                        ),
                                )
                                .child(
                                    div()
                                        .h_flex()
                                        .gap_2()
                                        .flex_wrap()
                                        .child(command(
                                            "synthesis-input-source",
                                            match input_label {
                                                "Input Texture" => "Use Input Texture",
                                                "Layer input" => "Use layer input",
                                                _ => "Use graph input",
                                            },
                                            Command::UseInput,
                                            disabled || is_input,
                                        ))
                                        .child(command(
                                            "synthesis-image-source",
                                            "Choose image…",
                                            Command::Open(FileKind::Photo),
                                            disabled,
                                        ))
                                        .when(panel.manual.is_none(), |row| {
                                            row.children(geometry::source_full_size_button(
                                                panel, &target, disabled, cx,
                                            ))
                                        })
                                        .child(
                                            command(
                                                "synthesis-clear-source",
                                                "Clear source",
                                                Command::ClearSource,
                                                disabled || !has_source,
                                            )
                                            .ghost(),
                                        ),
                                )
                                .children(source_editor),
                        )
                    },
                )
                .when(source_tools, |view| {
                    view.children(layer_commands()).child(
                        div()
                            .h_flex()
                            .gap_2()
                            .flex_wrap()
                            .children(preset_commands()),
                    )
                })
                .when_some(chosen, |view, (_, kind)| {
                    view.child(
                        div()
                            .h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(synthesis_menu(
                                Button::new("synthesis-layer-type")
                                    .small()
                                    .label("Change layer type"),
                                disabled,
                                move |mut menu, _, _| {
                                    menu = menu.action_context(type_focus.clone());
                                    for value in SynthesisLayerKind::ALL {
                                        menu = menu.item(menu_item(
                                            value.label(),
                                            value == kind,
                                            Command::Type(value),
                                            &type_target,
                                            &type_owner,
                                        ));
                                    }
                                    menu
                                },
                            ))
                            .child(
                                command(
                                    "synthesis-clear-layers",
                                    "Clear layers",
                                    Command::Clear,
                                    disabled,
                                )
                                .ghost(),
                            ),
                    )
                })
                .children(effect)
                .when_some(
                    layer.and_then(|index| panel.edit.equalizes_source(index)),
                    |view, value| {
                        view.child(toggle(
                            "synthesis-equalize",
                            "Equalize source brightness",
                            value,
                            Command::Equalize,
                            !has_source,
                        ))
                    },
                )
                .when_some(
                    layer.and_then(|index| panel.edit.automatic_placement(index)),
                    |view, value| {
                        view.child(toggle(
                            "synthesis-automatic",
                            "Automatic placement",
                            value,
                            Command::Automatic,
                            false,
                        ))
                    },
                )
                .child(
                    div()
                        .id(("synthesis-fields", chosen.map_or(0, |(id, _)| id)))
                        .v_flex()
                        .gap_2()
                        .when(!source_tools, |fields| {
                            fields.children(geometry::render(
                                self, panel, &target, disabled, busy, window, cx,
                            ))
                        })
                        .children(manual_repeat)
                        .children(photo_controls)
                        .children(color)
                        .children(blend)
                        .child(fields),
                )
                .into_any_element(),
        )
    }

    fn synthesis_command(
        &mut self,
        target: &Target,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy()
            || !target.current(self)
            || target.material_locked()
            || self.style.as_ref().is_some_and(|panel| panel.pending(cx))
        {
            return;
        }
        let Some(panel) = self.synthesis.as_ref() else {
            return;
        };
        if panel.pending(cx) && !matches!(command, Command::Apply) {
            return;
        }
        if matches!(command, Command::DoneMaterial) {
            if let Some(origin) = panel.lab_material.clone() {
                self.install_synthesis_panel(None, window, cx);
                self.reload_synthesis(origin.return_location, window, cx);
                self.observe_drafts(cx);
                self.focus.focus(window, cx);
                cx.notify();
                return;
            }
            if let Some(panel) = self.synthesis.as_mut() {
                panel.editing_material = None;
                panel.drag = None;
                panel.curve_hover = None;
                cx.notify();
            }
            return;
        }
        if matches!(command, Command::Automatic(false))
            && !target.lab_material
            && !(target.style.is_some() && target.editing_material.is_some())
            && let Some(layer) = panel.layer
            && (panel.linked_automatic_numbers(layer) || panel.linked_automatic_pairs(layer))
        {
            self.error = self
                .disable_linked_synthesis_placement(target, layer, window, cx)
                .err();
            cx.notify();
            return;
        }
        if let Command::Open(file) = command {
            self.load_synthesis_file(target.clone(), file, None, cx);
            return;
        }
        if let Command::BrowsePresets = command {
            self.open_synthesis_presets(target, window, cx);
            return;
        }
        if let Command::SavePreset = command {
            self.save_synthesis_preset(target, cx);
            return;
        }
        if let Command::Geometry(command) = command {
            self.synthesis_geometry_command(target, command, window, cx);
            return;
        }
        let mut edit = panel.edit.clone();
        let layer = panel.layer;
        let (label, selected) = match command {
            Command::Select(next) => {
                let next = Panel::new(panel.node.clone(), edit, (next, None), window, cx)
                    .keep_material_editor(panel);
                self.install_synthesis_panel(Some(next), window, cx);
                self.refresh_synthesis_guide_source(cx);
                self.refresh_synthesis_preview(cx);
                self.observe_drafts(cx);
                cx.notify();
                return;
            }
            Command::Apply => match panel.staged(self.synthesis_patch_input_geometry(panel), cx) {
                Ok(staged) => {
                    edit = staged;
                    ("Edit Synthesis controls", layer)
                }
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                    return;
                }
            },
            Command::Add(kind) => {
                edit.add_layer(kind);
                ("Add Synthesis layer", Some(0))
            }
            Command::Type(kind) => {
                if !layer.is_some_and(|index| edit.set_layer_kind(index, kind)) {
                    return;
                }
                ("Change Synthesis layer type", layer)
            }
            Command::Move(to) => {
                if !layer.is_some_and(|index| edit.move_layer(index, to)) {
                    return;
                }
                ("Move Synthesis layer", Some(to))
            }
            Command::Delete => {
                let Some(index) = layer.filter(|index| edit.remove_layer(*index)) else {
                    return;
                };
                (
                    "Remove Synthesis layer",
                    edit.layers().checked_sub(1).map(|last| index.min(last)),
                )
            }
            Command::Clear => {
                if !edit.clear_layers() {
                    return;
                }
                ("Clear Synthesis layers", None)
            }
            Command::UseInput => {
                let Some(index) = layer.filter(|index| edit.set_input_source(*index)) else {
                    return;
                };
                if edit.crop_rectangle(index).is_some()
                    && self.synthesis_guide_input(panel).is_some()
                {
                    edit.reset_crop_for_source_geometry(
                        index,
                        ImageGeometry::new(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE),
                    );
                }
                ("Change Synthesis source", layer)
            }
            Command::ClearSource => {
                if !layer.is_some_and(|index| edit.clear_source(index)) {
                    return;
                }
                ("Clear Synthesis source", layer)
            }
            Command::Mode(mode) => {
                if !layer.is_some_and(|index| edit.set_blend_mode(index, mode)) {
                    return;
                }
                ("Change Synthesis blend mode", layer)
            }
            Command::ErodeMode(mode) => {
                if !layer.is_some_and(|index| edit.set_erode_mode(index, mode)) {
                    return;
                }
                ("Change Synthesis erode mode", layer)
            }
            Command::Equalize(value) => {
                if !layer.is_some_and(|index| edit.set_equalize_source(index, value)) {
                    return;
                }
                ("Change Synthesis equalization", layer)
            }
            Command::Automatic(value) => {
                if !layer.is_some_and(|index| edit.set_automatic_placement(index, value)) {
                    return;
                }
                ("Change automatic patch placement", layer)
            }
            Command::Effect(index) => {
                if panel.automatic_placement_off()
                    || panel.number_linked(SynthesisNumberField::PatchSwitcher)
                    || !layer.is_some_and(|layer| edit.set_effect_choice(layer, index))
                {
                    return;
                }
                ("Change Synthesis mode", layer)
            }
            Command::DoneMaterial
            | Command::Open(_)
            | Command::BrowsePresets
            | Command::SavePreset
            | Command::Geometry(_) => unreachable!(),
        };
        let geometry = panel.geometry_in(&edit, selected);
        self.commit_synthesis(target, edit, (selected, geometry), label, window, cx);
    }

    fn commit_synthesis(
        &mut self,
        target: &Target,
        edit: SynthesisEdit,
        location: Location,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if target.lab_material {
            if target.current(self)
                && let Some(origin) = self
                    .synthesis
                    .as_ref()
                    .and_then(|panel| panel.lab_material.as_ref())
                && let Some(material) = target.editing_material.as_ref()
            {
                let selected = origin.selected.clone();
                match self.commit_lab_adjuster_change(window, cx, |document| {
                    document.edit_lab_synthesis_adjuster(selected.as_ref(), material, edit, label)
                }) {
                    Ok(()) => {
                        self.reload_synthesis(Some(location), window, cx);
                        self.observe_drafts(cx);
                    }
                    Err(error) => self.error = Some(error),
                }
                cx.notify();
            }
            return;
        }
        if let Some(style) = &target.style {
            if target.editing_material.is_some() {
                if target.current(self)
                    && let Some((node, canvas, pass)) = style.synthesis_number_receiver()
                    && let Some((expected, _)) = self
                        .style
                        .as_ref()
                        .and_then(style::Panel::synthesis_style_draft)
                {
                    let expected = expected.clone();
                    match self.commit_lab_adjuster_change(window, cx, |document| {
                        document.edit_style_synthesis_material(
                            node, canvas, &expected, pass, edit, label,
                        )
                    }) {
                        Ok(()) => {
                            self.reload_synthesis(Some(location), window, cx);
                            self.observe_drafts(cx);
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
            } else {
                self.commit_style_synthesis(style, edit, location, label, window, cx);
            }
            return;
        }
        if !target.current(self) || target.material_locked() {
            return;
        }
        let node = &target.node;
        let replaces_controls = edit.replaces_scale_and_seed();
        self.ensure_history_binding();
        if let Some(history) = self.history() {
            history.separate_next_edit();
        }
        let Some(filter) = node.filter() else {
            return;
        };
        let was_dirty = self.dirty;
        let stamp = self.begin_history_edit();
        self.error = None;
        let removal =
            guard(&self.document).edit_synthesis_removing_unused_adjusters(node, &edit, label);
        let change = match removal {
            Ok(false) => filter.edit_synthesis(edit, label).map_err(model::failure),
            result => result.map_err(model::failure),
        };
        match change {
            Ok(true) => {
                self.revision += 1;
                self.status = None;
                if replaces_controls {
                    self.select(self.selection.clone(), window, cx);
                }
                let next = filter.synthesis_edit().map(|edit| {
                    Panel::new(node.clone(), edit, location, window, cx)
                        .with_material(target.editing_material.clone())
                });
                self.install_synthesis_panel(next, window, cx);
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

    fn load_synthesis_file(
        &mut self,
        target: Target,
        file: FileKind,
        path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if matches!(file, FileKind::Preset { .. }) && self.has_field_drafts(cx) {
            return;
        }
        let Some(panel) = self.synthesis.as_ref() else {
            return;
        };
        let mut edit = panel.edit.clone();
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::LoadingSynthesis);
        self.error = None;
        let prompt = path.is_none().then(|| {
            cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(
                    match file {
                        FileKind::Preset { .. } => "Open Synthesis preset",
                        FileKind::Photo => "Choose Synthesis layer image",
                    }
                    .into(),
                ),
            })
        });
        let executor = cx.background_executor().clone();
        let handle = self.window;
        cx.spawn(async move |owner, cx| {
            let path = if let Some(prompt) = prompt {
                match prompt.await {
                    Ok(Ok(Some(mut paths))) if paths.len() == 1 => Ok(paths.pop()),
                    Ok(Ok(None)) => Ok(None),
                    Ok(Ok(Some(_))) => Err("Choose a single file.".into()),
                    Ok(Err(error)) => Err(format!("Couldn’t open the file picker. {error}")),
                    Err(_) => Err("The file picker closed without a selection.".into()),
                }
            } else {
                Ok(path)
            };
            if !ticket.is_current() {
                return;
            }
            let layer = target.layer;
            let result = match path {
                Ok(Some(path)) => {
                    executor
                        .spawn(async move {
                            let bytes = std::fs::read(&path)
                                .map_err(|error| format!("Couldn’t read the file. {error}"))?;
                            match file {
                                FileKind::Preset { insert } => {
                                    let label = if insert {
                                        edit.insert_preset_layers(&bytes)?;
                                        "Insert Synthesis preset layers"
                                    } else {
                                        edit.load_preset(&bytes)?;
                                        "Open Synthesis preset"
                                    };
                                    let selected = (edit.layers() > 0).then_some(0);
                                    Ok(Some((edit, selected, label)))
                                }
                                FileKind::Photo => {
                                    let index = layer.ok_or("Select a layer first.")?;
                                    let extension = path
                                        .extension()
                                        .and_then(|value| value.to_str())
                                        .unwrap_or_default();
                                    let image = EnvironmentImage::from_file_bytes(bytes, extension)
                                        .map_err(|error| {
                                            format!("Couldn’t decode the image. {error:?}")
                                        })?;
                                    if !edit.set_image_source(index, image) {
                                        return Err("This layer does not accept an image.".into());
                                    }
                                    edit.reset_crop_for_embedded_photo(index);
                                    Ok(Some((edit, layer, "Change Synthesis source")))
                                }
                            }
                        })
                        .await
                }
                Ok(None) => Ok(None),
                Err(error) => Err(error),
            };
            let _ = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if !view.owns(&ticket) {
                        return;
                    }
                    view.ticket = None;
                    view.work = None;
                    match result {
                        Ok(Some((edit, selected, label))) if target.current(view) => {
                            let geometry = match file {
                                FileKind::Photo => target.geometry,
                                FileKind::Preset { .. } => None,
                            };
                            view.commit_synthesis(
                                &target,
                                edit,
                                (selected, geometry),
                                label,
                                window,
                                cx,
                            )
                        }
                        Err(error) => view.error = Some(error),
                        _ => {}
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }
}
