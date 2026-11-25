//! Environment map authoring in the graph inspector.
#[path = "graph_document_environment_adjusters.rs"]
mod adjusters;
#[path = "graph_document_environment_converter.rs"]
mod converter;
#[path = "graph_document_environment_direction.rs"]
mod direction;
#[path = "graph_document_environment_editor.rs"]
pub(super) mod editor;

use super::preferences::File as PreferenceFile;
use super::*;
use mycela_engine::{
    BinaryCursor, EnvironmentAdjuster, EnvironmentBlendMode, EnvironmentColorField,
    EnvironmentImage, EnvironmentImageProjection, EnvironmentLayerKind, EnvironmentLight,
    EnvironmentMaskFill, EnvironmentMaskHemisphere, EnvironmentNumericReceiver, EnvironmentPreset,
    EnvironmentRecord, EnvironmentResourceLibrary, EnvironmentTransformFilter, PointwiseParameter,
    PointwiseParameterValue,
};

pub(super) struct PreferredLibrary(EnvironmentResourceLibrary);

impl gpui_kit::Global for PreferredLibrary {}

pub(super) struct PreferredSquareProjection(EnvironmentImageProjection);

impl gpui_kit::Global for PreferredSquareProjection {}

impl PreferredSquareProjection {
    fn current(cx: &App) -> EnvironmentImageProjection {
        if let Some(preference) = cx.try_global::<Self>() {
            return preference.0;
        }
        match PreferenceFile::EnvironmentSquareProjection
            .read(cx)
            .as_deref()
        {
            Some(b"angular\n") => EnvironmentImageProjection::LightProbe,
            _ => EnvironmentImageProjection::MirroredBall,
        }
    }

    fn remember(projection: EnvironmentImageProjection, cx: &mut App) -> Result<(), String> {
        let setting = match projection {
            EnvironmentImageProjection::LightProbe => b"angular\n".as_slice(),
            EnvironmentImageProjection::MirroredBall => b"mirrored-ball\n".as_slice(),
            _ => return Ok(()),
        };
        cx.set_global(Self(projection));
        PreferenceFile::EnvironmentSquareProjection.write(setting, cx)
    }
}

pub(super) fn preferred_library(cx: &App) -> EnvironmentResourceLibrary {
    if let Some(preference) = cx.try_global::<PreferredLibrary>() {
        return preference.0.clone();
    }
    std::env::var_os("MYCELA_RESOURCE_LIBRARY")
        .and_then(|directory| EnvironmentResourceLibrary::open(directory).ok())
        .or_else(|| {
            EnvironmentResourceLibrary::open(PreferenceFile::ResourceLibrary.read_path(cx)?).ok()
        })
        .unwrap_or_default()
}

#[derive(Clone)]
pub(crate) struct EnvironmentPanel {
    owner: MapOwner,
    map: Arc<EnvironmentRecord>,
    selected: Option<usize>,
    supplier: Option<String>,
    adjuster: Option<EnvironmentAdjuster>,
}

#[derive(Clone)]
enum MapOwner {
    Parameter(PointwiseParameter<ForegroundError>),
    Style(style::EnvironmentOwner),
}

impl EnvironmentPanel {
    pub(in crate::graph_document) fn new(
        parameter: PointwiseParameter<ForegroundError>,
        adjuster: Option<EnvironmentAdjuster>,
    ) -> Result<Self, String> {
        let supplier = parameter
            .environment_binding_source()
            .map_err(model::failure)?
            .map(|source| {
                format!(
                    "{} (variable {})",
                    String::from_utf16_lossy(&source.name()),
                    source
                        .wizard_variable_index()
                        .unwrap_or(parameter.binding_index())
                )
            });
        let map = match (&supplier, &adjuster) {
            (None, Some(adjuster)) => adjuster.value(),
            _ => parameter.effective_environment().map_err(model::failure)?,
        };
        let selected = (map.layer_count() > 0).then_some(0);
        Ok(Self {
            owner: MapOwner::Parameter(parameter),
            map,
            selected,
            supplier,
            adjuster,
        })
    }

    pub(super) fn for_style(
        owner: style::EnvironmentOwner,
        map: Arc<EnvironmentRecord>,
        adjuster: Option<EnvironmentAdjuster>,
    ) -> Self {
        Self {
            owner: MapOwner::Style(owner),
            selected: (map.layer_count() > 0).then_some(0),
            map,
            supplier: None,
            adjuster,
        }
    }

    pub(super) fn is_style(&self) -> bool {
        matches!(self.owner, MapOwner::Style(_))
    }

    fn parameter(&self) -> Option<&PointwiseParameter<ForegroundError>> {
        match &self.owner {
            MapOwner::Parameter(parameter) => Some(parameter),
            MapOwner::Style(_) => None,
        }
    }

    pub(in crate::graph_document) fn same_source(&self, other: &Self) -> bool {
        match (&self.owner, &other.owner) {
            (MapOwner::Parameter(a), MapOwner::Parameter(b)) => a.same_reference(b),
            (MapOwner::Style(a), MapOwner::Style(b)) => a.same_pass(b),
            _ => false,
        }
    }

    pub(in crate::graph_document) fn wizard_variable_index(&self) -> Option<i32> {
        self.parameter()?.wizard_variable_index()
    }

    fn label(&self) -> String {
        self.parameter().map_or_else(
            || "Environment map".into(),
            |parameter| String::from_utf16_lossy(&parameter.name()),
        )
    }

    pub(in crate::graph_document) fn value(&self) -> &Arc<EnvironmentRecord> {
        &self.map
    }

    pub(in crate::graph_document) fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub(in crate::graph_document) fn select(&mut self, index: usize) {
        self.selected = self
            .map
            .layer_count()
            .checked_sub(1)
            .map(|last| index.min(last));
    }

    pub(in crate::graph_document) fn is_editable(&self) -> bool {
        self.supplier.is_none() && (self.adjuster.is_some() || !self.has_adjuster_link())
    }

    pub(super) fn has_adjuster_link(&self) -> bool {
        match &self.owner {
            MapOwner::Style(owner) => owner.has_adjuster_link(),
            MapOwner::Parameter(parameter) => parameter
                .retained_adjustment()
                .is_some_and(|adjustment| adjustment.is_active().unwrap_or(true)),
        }
    }

    pub(super) fn same_adjuster(&self, other: &Self) -> bool {
        match (&self.adjuster, &other.adjuster) {
            (Some(a), Some(b)) => a.same_reference(b) && a.name() == b.name(),
            (None, None) => self.has_adjuster_link() == other.has_adjuster_link(),
            _ => false,
        }
    }

    pub(in crate::graph_document) fn validate(&self) -> Result<(), String> {
        let current = match &self.owner {
            MapOwner::Style(_) if !self.is_editable() => {
                return Err("The linked Environment adjuster is unavailable.".into());
            }
            MapOwner::Style(owner) => owner.map()?,
            MapOwner::Parameter(parameter) => self.current_parameter_value(parameter)?,
        };
        if Arc::ptr_eq(&current, &self.map) {
            Ok(())
        } else {
            Err("The environment map changed. Revert fields before continuing.".into())
        }
    }

    fn current_parameter_value(
        &self,
        parameter: &PointwiseParameter<ForegroundError>,
    ) -> Result<Arc<EnvironmentRecord>, String> {
        if parameter
            .environment_binding_source()
            .map_err(model::failure)?
            .is_some()
        {
            return Err("Edit the Environment variable in its Wizard Group.".into());
        }
        if !self.is_editable() {
            return Err("This environment map is controlled by an adjuster.".into());
        }
        if let Some(adjuster) = &self.adjuster {
            Ok(adjuster.value())
        } else if let PointwiseParameterValue::Environment(current) = parameter.value() {
            Ok(current)
        } else {
            Err("This property no longer has an Environment map.".into())
        }
    }

    pub(super) fn commit(
        &self,
        map: EnvironmentRecord,
        document: &Arc<Mutex<model::Document>>,
    ) -> Result<(), String> {
        self.validate()?;
        if guard(document)
            .edit_environment_removing_unused_adjusters(self.map_target(), &self.map, &map)
            .map_err(model::failure)?
        {
            return Ok(());
        }
        let parameter = match &self.owner {
            MapOwner::Style(owner) => {
                return owner.commit(&self.map, self.adjuster.as_ref(), map, document);
            }
            MapOwner::Parameter(parameter) => parameter,
        };
        let replaced = if let Some(adjuster) = &self.adjuster {
            guard(document)
                .edit_environment_adjuster(parameter, adjuster, &self.map, map)
                .map_err(model::failure)?
        } else {
            parameter
                .compare_and_set_environment(&self.map, Arc::new(map))
                .map_err(model::failure)?
        };
        if !replaced {
            return Err("The environment map changed. Revert fields before continuing.".into());
        }
        Ok(())
    }

    fn preview_override(
        &self,
        map: Arc<EnvironmentRecord>,
        document: &model::Document,
    ) -> model::EnvironmentPreview {
        let edit = match (&self.owner, &self.adjuster) {
            (_, Some(adjuster)) => document.preview_environment_adjuster(adjuster, map),
            (MapOwner::Style(owner), None) => owner.preview_environment(map),
            (MapOwner::Parameter(property), None) => {
                mycela_engine::EnvironmentRenderOverride::for_properties(
                    vec![property.clone()],
                    map,
                )
            }
        };
        Arc::new(edit)
    }
}

#[derive(Clone)]
pub(super) struct Target {
    key: Option<String>,
    node: Option<Node>,
    revision: u64,
    pub(super) panel: EnvironmentPanel,
}

impl Target {
    pub(super) fn for_panel(view: &GraphDocumentView, panel: &EnvironmentPanel) -> Self {
        Self {
            key: view.selection.clone(),
            node: view.inspector.owner.clone(),
            revision: view.revision,
            panel: panel.clone(),
        }
    }

    pub(super) fn current(&self, view: &GraphDocumentView) -> bool {
        view.is_original_selection(&self.key, &self.node, self.revision)
            && view.inspector.environment.as_ref().is_some_and(|panel| {
                panel.same_source(&self.panel)
                    && Arc::ptr_eq(panel.value(), self.panel.value())
                    && panel.same_adjuster(&self.panel)
                    && panel.selected() == self.panel.selected()
            })
    }
}

#[derive(Clone, Copy)]
enum Command {
    Select(usize),
    Add(EnvironmentLayerKind),
    SetGenerator(EnvironmentLayerKind),
    SetLight(EnvironmentLight),
    SetRecolor(bool),
    SetMaskHemisphere(EnvironmentMaskHemisphere),
    SetMaskFill(EnvironmentMaskFill),
    SetBlendMode(EnvironmentBlendMode),
    SetMixer(bool),
    SetProjection(EnvironmentImageProjection),
    SetTransformFilter(Option<usize>, EnvironmentTransformFilter),
    OpenFile(EnvironmentFile),
    Move(usize),
    Delete,
    Clear,
}

#[derive(Clone, Copy)]
enum EnvironmentFile {
    Map,
    AddImage,
    ReplaceImage(usize),
}

fn command_item(
    label: impl Into<gpui_kit::SharedString>,
    command: Command,
    target: &Target,
    owner: &gpui_kit::WeakEntity<GraphDocumentView>,
) -> PopupMenuItem {
    let target = target.clone();
    let owner = owner.clone();
    PopupMenuItem::new(label).on_click(move |_, window, cx| {
        let _ = owner.update(cx, |view, cx| {
            view.edit_environment(&target, command, window, cx)
        });
    })
}

fn transform_filter_button(
    layer: Option<usize>,
    target: &Target,
    owner: &gpui_kit::WeakEntity<GraphDocumentView>,
    disabled: bool,
) -> impl IntoElement + use<> {
    let (id, name) = if layer.is_some() {
        ("graph-environment-layer-filter", "Layer filter")
    } else {
        ("graph-environment-master-filter", "Master filter")
    };
    let current = target.panel.value().transform_filter(layer);
    let choice = current.map_or("Unknown", EnvironmentTransformFilter::label);
    let target = target.clone();
    let owner = owner.clone();
    Button::new(id)
        .small()
        .label(format!("{name} · {choice}"))
        .disabled(disabled)
        .dropdown_menu(move |mut menu, _, _| {
            for filter in EnvironmentTransformFilter::ALL {
                menu = menu.item(
                    command_item(
                        filter.label(),
                        Command::SetTransformFilter(layer, filter),
                        &target,
                        &owner,
                    )
                    .checked(current == Some(filter)),
                );
            }
            menu
        })
}

fn mask_controls(
    index: usize,
    target: &Target,
    owner: &gpui_kit::WeakEntity<GraphDocumentView>,
    disabled: bool,
) -> impl IntoElement + use<> {
    let hemisphere = target
        .panel
        .value()
        .mask_hemisphere(index)
        .unwrap_or_default();
    let fill = target.panel.value().mask_fill(index).unwrap_or_default();
    let hemisphere_target = target.clone();
    let hemisphere_owner = owner.clone();
    let fill_target = target.clone();
    let fill_owner = owner.clone();
    div()
        .h_flex()
        .flex_wrap()
        .gap_2()
        .child(
            Button::new("graph-environment-mask")
                .small()
                .label(format!("Mask · {}", hemisphere.label()))
                .disabled(disabled)
                .dropdown_menu(move |mut menu, _, _| {
                    for choice in EnvironmentMaskHemisphere::ALL {
                        menu = menu.item(
                            command_item(
                                choice.label(),
                                Command::SetMaskHemisphere(choice),
                                &hemisphere_target,
                                &hemisphere_owner,
                            )
                            .checked(hemisphere == choice),
                        );
                    }
                    menu
                }),
        )
        .when(hemisphere != EnvironmentMaskHemisphere::Off, |row| {
            row.child(
                Button::new("graph-environment-mask-fill")
                    .small()
                    .label(format!("Mask fill · {}", fill.label()))
                    .disabled(disabled)
                    .dropdown_menu(move |mut menu, _, _| {
                        for choice in EnvironmentMaskFill::ALL {
                            menu = menu.item(
                                command_item(
                                    choice.label(),
                                    Command::SetMaskFill(choice),
                                    &fill_target,
                                    &fill_owner,
                                )
                                .checked(fill == choice),
                            );
                        }
                        menu
                    }),
            )
        })
}

impl GraphDocumentView {
    pub(super) fn render_environment_controls(
        &self,
        panel: &EnvironmentPanel,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let target = Target::for_panel(self, panel);
        let selected = panel.selected();
        let count = panel.value().layer_count();
        let editable = panel.is_editable();
        let owner = cx.weak_entity();
        let add_owner = owner.clone();
        let add_target = target.clone();
        let open_target = target.clone();
        let preset_target = target.clone();
        let save_target = target.clone();
        let export_target = target.clone();
        let layers_owner = owner.clone();
        let layers_target = target.clone();
        let actions_owner = owner.clone();
        let actions_target = target.clone();
        let selected_kind = selected.and_then(|index| panel.value().layer_kind(index));
        let generator_target = target.clone();
        let generator_owner = owner.clone();
        let light = match selected_kind {
            Some(EnvironmentLayerKind::Light(light)) => Some(light),
            _ => None,
        };
        let image_layer = selected.filter(|_| selected_kind == Some(EnvironmentLayerKind::Image));
        let projection_owner = owner.clone();
        let projection_target = target.clone();
        let replace_target = target.clone();
        let label = |value: &EnvironmentRecord, index| {
            let kind = value
                .layer_kind(index)
                .map_or("Layer", EnvironmentLayerKind::label);
            format!("{} · {kind}", index + 1)
        };
        let field_target = target.clone();
        let recolor_target = target.clone();
        let blend_target = target.clone();
        let mixer_target = target.clone();
        let blend_owner = owner.clone();
        let library_directory = self
            .environment_library
            .root()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Choose your Genetica Resource Library folder".into());
        let library_path = selected.and_then(|index| panel.value().library_image_path(index));
        use EnvironmentNumericReceiver as NumericField;
        let linked = |field| panel.value().numeric_adjuster_key(field).is_some();
        let menu = |field, id: &str| {
            self.render_environment_numeric_adjuster(panel, field, id.to_owned(), busy, cx)
        };
        let master_filter = transform_filter_button(
            None,
            &target,
            &owner,
            busy || !editable || linked(NumericField::Filter(None)),
        );
        let master_link = menu(
            NumericField::Filter(None),
            "graph-environment-master-filter:lab-link",
        );
        let layer_filter = selected.map(|index| {
            transform_filter_button(
                Some(index),
                &target,
                &owner,
                busy || !editable || linked(NumericField::Filter(Some(index))),
            )
        });
        let layer_link = selected.and_then(|index| {
            menu(
                NumericField::Filter(Some(index)),
                "graph-environment-layer-filter:lab-link",
            )
        });
        let light_linked = selected.is_some_and(|index| linked(NumericField::Light(index)));
        let light_link = selected.and_then(|index| {
            menu(
                NumericField::Light(index),
                "graph-environment-light-type:lab-link",
            )
        });
        let light_direction = light.and_then(|_| {
            self.environment_direction_button(&target, direction::Direction::Light, busy, cx)
        });
        let mixer_direction = selected
            .filter(|&index| panel.value().mixer_enabled(index))
            .and_then(|_| {
                self.environment_direction_button(&target, direction::Direction::Mixer, busy, cx)
            });
        let blend_linked = selected.is_some_and(|index| linked(NumericField::Blend(index)));
        let blend_link = selected.and_then(|index| {
            menu(
                NumericField::Blend(index),
                "graph-environment-blend:lab-link",
            )
        });
        let layer_mask =
            selected.map(|index| mask_controls(index, &target, &owner, busy || !editable));
        let opening_target = target.clone();
        let map_editor = self.embedded_environment.is_some();
        div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .justify_between()
                    .gap_2()
                    .child(div().font_semibold().child(panel.label()))
                    .when(!map_editor && panel.can_link_adjuster(), |row| {
                        row.child(self.render_environment_adjuster_menu(&target, busy, cx))
                    }),
            )
            .when(
                !panel.is_style() && self.inspector.environment_panels.len() > 1,
                |element| {
                    let target = target.clone();
                    let owner = owner.clone();
                    let choices = self.inspector.environment_panels.clone();
                    element.child(
                        Button::new("graph-environment-variable")
                            .small()
                            .label("Choose Environment variable")
                            .disabled(busy)
                            .dropdown_menu(move |mut menu, _, _| {
                                for (index, choice) in choices.iter().enumerate() {
                                    let selected = target.panel.same_source(choice);
                                    let owner = owner.clone();
                                    let target = target.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(choice.label())
                                            .checked(selected)
                                            .on_click(move |_, window, cx| {
                                                let _ = owner.update(cx, |view, cx| {
                                                    view.choose_environment_panel(
                                                        &target, index, window, cx,
                                                    )
                                                });
                                            }),
                                    );
                                }
                                menu
                            }),
                    )
                },
            )
            .when_some(
                panel.adjuster.as_ref().filter(|_| editable),
                |element, adjuster| {
                    element.child(div().text_sm().text_color(theme.muted_foreground).child(
                        format!(
                            "Editing “{}” for all linked properties",
                            String::from_utf16_lossy(adjuster.name())
                        ),
                    ))
                },
            )
            .when(!editable, |element| {
                element.child(div().text_sm().text_color(theme.muted_foreground).child(
                    panel.supplier.as_ref().map_or_else(
                        || "The linked Environment adjuster is unavailable.".into(),
                        |supplier| {
                            format!(
                                "Supplied by {supplier}. Edit that variable in its Wizard Group."
                            )
                        },
                    ),
                ))
            })
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .when(!map_editor, |row| {
                        row.child(
                            Button::new("graph-environment-edit-map")
                                .small()
                                .label("Edit map…")
                                .disabled(busy || !editable)
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.open_environment_editor(&opening_target, cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("graph-environment-add")
                            .small()
                            .label("Add layer")
                            .disabled(busy || !editable)
                            .dropdown_menu(move |menu, _, _| {
                                menu.item(command_item(
                                    "Light",
                                    Command::Add(EnvironmentLayerKind::Light(
                                        EnvironmentLight::Soft,
                                    )),
                                    &add_target,
                                    &add_owner,
                                ))
                                .item(command_item(
                                    "Solid color",
                                    Command::Add(EnvironmentLayerKind::Color),
                                    &add_target,
                                    &add_owner,
                                ))
                                .item(command_item(
                                    "Image…",
                                    Command::OpenFile(EnvironmentFile::AddImage),
                                    &add_target,
                                    &add_owner,
                                ))
                            }),
                    )
                    .child(
                        Button::new("graph-environment-open")
                            .small()
                            .label("Open map…")
                            .disabled(busy || !editable)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.choose_environment_file(
                                    open_target.clone(),
                                    EnvironmentFile::Map,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new("graph-environment-presets")
                            .small()
                            .label("Presets…")
                            .disabled(busy || !editable)
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.open_environment_presets(&preset_target, window, cx);
                            })),
                    )
                    .child(
                        Button::new("graph-environment-save")
                            .small()
                            .label("Save map…")
                            .disabled(busy || !editable || count == 0)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.save_environment_map(&save_target, cx);
                            })),
                    )
                    .child(
                        Button::new("graph-environment-export")
                            .small()
                            .label("Export map…")
                            .disabled(busy || !editable || count == 0)
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.open_environment_export(&export_target, window, cx);
                            })),
                    ),
            )
            .when(!map_editor, |element| {
                element.child(
                    Button::new("graph-environment-library")
                        .small()
                        .label("Resource library…")
                        .tooltip(library_directory)
                        .disabled(busy)
                        .on_click(cx.listener(|view, _, _, cx| view.choose_resource_library(cx))),
                )
            })
            .when_some(library_path, |element, path| {
                element.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(path),
                )
            })
            .when(selected.is_none(), |element| {
                element.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("No layers"),
                )
            })
            .when_some(selected, |element, index| {
                element.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("graph-environment-layers")
                                .small()
                                .label(label(panel.value(), index))
                                .disabled(busy || !editable)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for layer in 0..count {
                                        menu = menu.item(
                                            command_item(
                                                label(layers_target.panel.value(), layer),
                                                Command::Select(layer),
                                                &layers_target,
                                                &layers_owner,
                                            )
                                            .checked(layer == index),
                                        );
                                    }
                                    menu.scrollable(true)
                                }),
                        )
                        .child(
                            Button::new("graph-environment-layer-actions")
                                .small()
                                .label("Layer actions")
                                .disabled(busy || !editable)
                                .dropdown_menu(move |mut menu, _, _| {
                                    if index > 0 {
                                        menu = menu.item(command_item(
                                            "Move layer up",
                                            Command::Move(index - 1),
                                            &actions_target,
                                            &actions_owner,
                                        ));
                                    }
                                    if index + 1 < count {
                                        menu = menu.item(command_item(
                                            "Move layer down",
                                            Command::Move(index + 1),
                                            &actions_target,
                                            &actions_owner,
                                        ));
                                    }
                                    menu.item(command_item(
                                        "Delete layer",
                                        Command::Delete,
                                        &actions_target,
                                        &actions_owner,
                                    ))
                                    .item(command_item(
                                        "Clear layers",
                                        Command::Clear,
                                        &actions_target,
                                        &actions_owner,
                                    ))
                                }),
                        ),
                )
            })
            .when_some(selected_kind, |element, kind| {
                let light_kind =
                    EnvironmentLayerKind::Light(light.unwrap_or(EnvironmentLight::Soft));
                let choices = [
                    (EnvironmentLayerKind::Image, "Image"),
                    (light_kind, "Light"),
                    (EnvironmentLayerKind::Color, "Solid color"),
                ];
                let name = if light.is_some() {
                    "Light"
                } else {
                    kind.label()
                };
                element.child(
                    Button::new("graph-environment-generator")
                        .small()
                        .label(format!("Generator · {name}"))
                        .disabled(busy || !editable)
                        .dropdown_menu(move |mut menu, _, _| {
                            for (value, label) in choices {
                                menu = menu.item(
                                    command_item(
                                        label,
                                        Command::SetGenerator(value),
                                        &generator_target,
                                        &generator_owner,
                                    )
                                    .checked(value == kind),
                                );
                            }
                            menu
                        }),
                )
            })
            .when_some(light, |element, light| {
                element.child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("graph-environment-light-type")
                                .small()
                                .label(light.label())
                                .tooltip("Light type")
                                .disabled(busy || !editable || light_linked)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for kind in EnvironmentLight::ALL {
                                        menu = menu.item(
                                            command_item(
                                                kind.label(),
                                                Command::SetLight(kind),
                                                &target,
                                                &owner,
                                            )
                                            .checked(kind == light),
                                        );
                                    }
                                    menu
                                }),
                        )
                        .when_some(light_link, |row, menu| row.child(menu))
                        .when_some(light_direction, |row, button| row.child(button)),
                )
            })
            .when_some(image_layer, |element, index| {
                let current = panel.value().image_projection(index);
                element.child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(div().text_sm().child("Projection"))
                        .child(
                            Button::new("graph-environment-projection")
                                .small()
                                .label(
                                    current.map_or(
                                        "Choose projection",
                                        EnvironmentImageProjection::label,
                                    ),
                                )
                                .disabled(busy || !editable)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for projection in EnvironmentImageProjection::ALL {
                                        menu = menu.item(
                                            command_item(
                                                projection.label(),
                                                Command::SetProjection(projection),
                                                &projection_target,
                                                &projection_owner,
                                            )
                                            .checked(current == Some(projection)),
                                        );
                                    }
                                    menu
                                }),
                        )
                        .child(
                            Button::new("graph-environment-replace-image")
                                .small()
                                .label("Replace image…")
                                .disabled(busy || !editable)
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.choose_environment_file(
                                        replace_target.clone(),
                                        EnvironmentFile::ReplaceImage(index),
                                        cx,
                                    );
                                })),
                        ),
                )
            })
            .when_some(layer_mask, |element, controls| element.child(controls))
            .when_some(selected, |element, index| {
                element.child(
                    Checkbox::new("graph-environment-recolor")
                        .label("Recolor with gradient")
                        .checked(panel.value().recolor_enabled(index))
                        .disabled(busy || !editable)
                        .on_change(cx.listener(move |view, enabled, window, cx| {
                            view.edit_environment(
                                &recolor_target,
                                Command::SetRecolor(*enabled),
                                window,
                                cx,
                            );
                        })),
                )
            })
            .when_some(
                selected.filter(|_| !map_editor && editable),
                |element, index| {
                    let color = (selected_kind == Some(EnvironmentLayerKind::Color))
                        .then_some(EnvironmentColorField::LayerColor(index));
                    let recolor = panel
                        .value()
                        .recolor_enabled(index)
                        .then_some(EnvironmentColorField::Recolor(index));
                    let fields = color
                        .into_iter()
                        .chain(std::iter::once(EnvironmentColorField::Colorize(index)))
                        .chain(recolor);
                    element.child(div().h_flex().flex_wrap().gap_2().children(fields.map(
                        |field| {
                            self.render_environment_field_adjuster(&field_target, field, busy, cx)
                        },
                    )))
                },
            )
            .when_some(selected, |element, index| {
                let current = panel.value().blend_mode(index);
                let label = current.map_or("Choose blend mode", EnvironmentBlendMode::label);
                element.child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("graph-environment-blend")
                                .small()
                                .label(format!("Blend · {label}"))
                                .disabled(busy || !editable || blend_linked)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for mode in EnvironmentBlendMode::ALL {
                                        menu = menu.item(
                                            command_item(
                                                mode.label(),
                                                Command::SetBlendMode(mode),
                                                &blend_target,
                                                &blend_owner,
                                            )
                                            .checked(current == Some(mode)),
                                        );
                                    }
                                    menu.scrollable(true)
                                }),
                        )
                        .when_some(blend_link, |row, menu| row.child(menu))
                        .child(
                            Checkbox::new("graph-environment-mixer")
                                .label("Use mixer")
                                .checked(panel.value().mixer_enabled(index))
                                .disabled(busy || !editable)
                                .on_change(cx.listener(move |view, enabled, window, cx| {
                                    view.edit_environment(
                                        &mixer_target,
                                        Command::SetMixer(*enabled),
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .when_some(mixer_direction, |row, button| row.child(button)),
                )
            })
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(master_filter)
                    .when_some(master_link, |row, menu| row.child(menu))
                    .when_some(layer_filter, |row, button| row.child(button))
                    .when_some(layer_link, |row, menu| row.child(menu)),
            )
    }

    pub(super) fn can_edit_environment(&mut self, target: &Target, cx: &mut Context<Self>) -> bool {
        if self.is_busy() || !target.current(self) {
            return false;
        }
        let error = if self.has_field_drafts(cx) {
            Some("Correct or reset field text before changing the map.".into())
        } else {
            target.panel.validate().err()
        };
        if let Some(error) = error {
            self.error = Some(error);
            cx.notify();
            return false;
        }
        true
    }

    fn choose_environment_panel(
        &mut self,
        target: &Target,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() || !target.current(self) {
            return;
        }
        self.error = if self.inspector.is_changed(cx) {
            Some("Correct or reset field text before choosing a variable.".into())
        } else {
            self.inspector
                .select_environment_parameter(index, 0, window, cx)
                .err()
        };
        self.observe_drafts(cx);
        cx.notify();
    }

    pub(super) fn add_wizard_environment_variable(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        if self.inspector.is_changed(cx) {
            self.error = Some("Correct or reset field text before adding a variable.".into());
            cx.notify();
            return;
        }
        let Some(group) = self
            .selected_node()
            .and_then(|node| node.filter())
            .filter(|filter| filter.kind() == mycela_engine::PointwiseKind::WizardGroup)
        else {
            return;
        };
        match group.add_wizard_environment_variable() {
            Ok(Some(index)) => {
                self.dirty = true;
                self.revision += 1;
                self.status = None;
                self.error = None;
                self.rebuild(window, cx);
                if let Some(panel) = self
                    .inspector
                    .environment_panels
                    .iter()
                    .position(|panel| panel.wizard_variable_index() == Some(index))
                {
                    self.error = self
                        .inspector
                        .select_environment_parameter(panel, 0, window, cx)
                        .err();
                    self.observe_drafts(cx);
                }
                self.start_preview(cx);
            }
            Ok(None) => {
                self.error = Some("This Wizard Group already uses all 20 variable slots.".into())
            }
            Err(error) => self.error = Some(model::failure(error)),
        }
        cx.notify();
    }

    fn edit_environment(
        &mut self,
        target: &Target,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Command::OpenFile(file) = command {
            self.choose_environment_file(target.clone(), file, cx);
            return;
        }
        if !self.can_edit_environment(target, cx) {
            return;
        }
        if let Command::Select(index) = command {
            self.error = self
                .inspector
                .select_environment_layer(index, window, cx)
                .err();
            self.observe_drafts(cx);
            self.refresh_environment_layer_preview(cx);
            cx.notify();
            return;
        }
        let panel = &target.panel;
        let mut proposed = panel.value().as_ref().clone();
        let index = panel.selected();
        let result = match command {
            Command::Add(kind) => proposed.add_layer(kind).map(|()| 0),
            Command::SetGenerator(kind) => {
                let Some(index) = index else {
                    return;
                };
                match proposed.set_layer_generator(index, kind) {
                    Ok(false) => return,
                    Ok(true) => Ok(index),
                    Err(error) => Err(error),
                }
            }
            Command::SetLight(kind) => {
                let Some(index) = index else {
                    return;
                };
                if proposed.layer_kind(index) == Some(EnvironmentLayerKind::Light(kind)) {
                    return;
                }
                proposed.set_light(index, kind).map(|()| index)
            }
            Command::SetRecolor(enabled) => {
                let Some(index) = index else {
                    return;
                };
                if proposed.recolor_enabled(index) == enabled {
                    return;
                }
                proposed.set_recolor_enabled(index, enabled).map(|()| index)
            }
            Command::SetMaskHemisphere(hemisphere) => {
                let Some(index) = index else {
                    return;
                };
                if proposed.mask_hemisphere(index) == Some(hemisphere) {
                    return;
                }
                proposed
                    .set_mask_hemisphere(index, hemisphere)
                    .map(|()| index)
            }
            Command::SetMaskFill(fill) => {
                let Some(index) = index else {
                    return;
                };
                if proposed.mask_fill(index) == Some(fill) {
                    return;
                }
                proposed.set_mask_fill(index, fill).map(|()| index)
            }
            Command::SetBlendMode(mode) => {
                let Some(index) = index else {
                    return;
                };
                if proposed.blend_mode(index) == Some(mode) {
                    return;
                }
                proposed.set_blend_mode(index, mode).map(|()| index)
            }
            Command::SetMixer(enabled) => {
                let Some(index) = index else {
                    return;
                };
                if proposed.mixer_enabled(index) == enabled {
                    return;
                }
                proposed.set_mixer_enabled(index, enabled).map(|()| index)
            }
            Command::SetProjection(next) => {
                let Some(index) = index else {
                    return;
                };
                if proposed.image_projection(index) == Some(next) {
                    self.remember_environment_square_projection(next, cx);
                    return;
                }
                proposed.set_image_projection(index, next).map(|()| index)
            }
            Command::SetTransformFilter(layer, filter) => {
                if proposed.transform_filter(layer) == Some(filter) {
                    return;
                }
                proposed
                    .set_transform_filter(layer, filter)
                    .map(|()| index.unwrap_or(0))
            }
            Command::Move(to) => {
                let Some(index) = index else {
                    return;
                };
                proposed.move_layer(index, to).map(|()| to)
            }
            Command::Delete => {
                let Some(index) = index else {
                    return;
                };
                proposed.remove_layer(index).map(|()| index)
            }
            Command::Clear => {
                proposed.clear_layers();
                Ok(0)
            }
            Command::Select(_) | Command::OpenFile(_) => unreachable!(),
        };
        match result {
            Ok(index) => {
                self.apply_environment_change(panel, proposed, index, window, cx);
                if self.error.is_none()
                    && let Command::SetProjection(next) = command
                {
                    self.remember_environment_square_projection(next, cx);
                }
            }
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
            }
        }
    }

    fn remember_environment_square_projection(
        &mut self,
        projection: EnvironmentImageProjection,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = PreferredSquareProjection::remember(projection, cx) {
            self.error = Some(format!(
                "Couldn’t remember the square image projection. {error}"
            ));
            cx.notify();
        }
    }

    pub(super) fn apply_environment_change(
        &mut self,
        panel: &EnvironmentPanel,
        map: EnvironmentRecord,
        selected: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ensure_history_binding();
        if let Some(history) = self.history() {
            history.separate_next_edit();
        }
        let stamp = self.begin_history_edit();
        self.revision += 1;
        self.status = None;
        self.error = panel.commit(map, &self.document).err();
        if self.error.is_none() {
            let key = self.selection.clone();
            self.select(key, window, cx);
            self.error = self
                .inspector
                .select_environment_layer(selected, window, cx)
                .err();
            self.observe_drafts(cx);
            if self.embedded_environment.is_some() {
                self.focus.focus(window, cx);
            }
            self.finish_history_edit(stamp, self.error.is_none());
        }
        self.start_preview(cx);
        cx.notify();
    }

    fn choose_resource_library(&mut self, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() {
            return;
        }
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::LoadingEnvironment);
        self.error = None;
        cx.notify();
        let prompt = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a Resource Library folder".into()),
        });
        let background = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let selected = prompt.await;
            if !ticket.is_current() {
                return;
            }
            let result = match selected {
                Ok(Ok(Some(mut paths))) if paths.len() == 1 => {
                    let path = paths.remove(0);
                    background
                        .spawn(async move {
                            EnvironmentResourceLibrary::open(path)
                                .map(Some)
                                .map_err(|error| {
                                    format!("Couldn’t open the Resource Library folder. {error}")
                                })
                        })
                        .await
                }
                Ok(Ok(None)) => Ok(None),
                Ok(Ok(Some(_))) => Err("Choose one Resource Library folder.".into()),
                Ok(Err(error)) => Err(format!("Couldn’t open the folder picker. {error}")),
                Err(_) => Err("The folder picker closed without a selection.".into()),
            };
            let _ = owner.update(cx, |view, cx| {
                if !view.owns(&ticket) {
                    return;
                }
                view.work = None;
                view.ticket = None;
                match result {
                    Ok(Some(library)) => {
                        let root = library.root().expect("opened library has a root");
                        view.error = PreferenceFile::ResourceLibrary
                            .write_path(root, cx)
                            .err()
                            .map(|error| {
                                format!("Couldn’t remember the Resource Library folder. {error}")
                            });
                        cx.set_global(PreferredLibrary(library.clone()));
                        view.environment_library = library;
                        view.status = Some("Resource Library folder updated.".into());
                        view.start_preview(cx);
                    }
                    Ok(None) => {}
                    Err(error) => view.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_environment_file(
        &mut self,
        target: Target,
        file: EnvironmentFile,
        cx: &mut Context<Self>,
    ) {
        if !self.can_edit_environment(&target, cx) {
            return;
        }
        let square_projection = PreferredSquareProjection::current(cx);
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::LoadingEnvironment);
        self.error = None;
        cx.notify();
        let prompt = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(
                match file {
                    EnvironmentFile::Map => "Open environment map",
                    EnvironmentFile::AddImage | EnvironmentFile::ReplaceImage(_) => {
                        "Choose layer image"
                    }
                }
                .into(),
            ),
        });
        let executor = cx.background_executor().clone();
        let handle = self.window;
        cx.spawn(async move |owner, cx| {
            let selected = prompt.await;
            if !ticket.is_current() {
                return;
            }
            let result = match selected {
                Ok(Ok(Some(paths))) => match paths.as_slice() {
                    [path] => {
                        let path = path.clone();
                        let base = target.panel.value().clone();
                        executor
                            .spawn(async move {
                                read_environment_file(&path, file, &base, square_projection)
                            })
                            .await
                            .map(Some)
                    }
                    _ => Err("Choose a single file.".into()),
                },
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(format!("Couldn’t open the file picker. {error}")),
                Err(_) => Err("The file picker closed without a selection.".into()),
            };
            let _ = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if !view.owns(&ticket) {
                        return;
                    }
                    view.work = None;
                    view.ticket = None;
                    match result {
                        Ok(Some((map, selected))) if view.can_edit_environment(&target, cx) => {
                            view.apply_environment_change(&target.panel, map, selected, window, cx);
                        }
                        Err(error) => view.error = Some(error),
                        _ => {}
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }
}

fn read_environment_file(
    path: &Path,
    file: EnvironmentFile,
    current: &EnvironmentRecord,
    square_projection: EnvironmentImageProjection,
) -> Result<(EnvironmentRecord, usize), String> {
    let metadata = path
        .metadata()
        .map_err(|e| format!("Couldn’t open the file. {e}"))?;
    if !metadata.is_file() {
        return Err("Choose a regular file.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn’t read the file. {e}"))?;
    if matches!(file, EnvironmentFile::Map) {
        return read_map(&bytes).map(|map| (map, 0));
    }
    let image = EnvironmentImage::from_file_bytes(
        bytes,
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default(),
    )
    .map_err(|e| format!("Couldn’t decode the layer image. {e}"))?;
    let projection = image
        .inferred_projection(square_projection)
        .map_err(|e| format!("Couldn’t read the layer image dimensions. {e}"))?;
    let mut proposed = current.clone();
    let index = match file {
        EnvironmentFile::AddImage => {
            proposed
                .add_layer(EnvironmentLayerKind::Image)
                .map_err(|e| e.to_string())?;
            0
        }
        EnvironmentFile::ReplaceImage(index) => index,
        EnvironmentFile::Map => unreachable!(),
    };
    proposed
        .set_layer_image(index, image)
        .map_err(|e| e.to_string())?;
    proposed
        .set_image_projection(index, projection)
        .map_err(|e| e.to_string())?;
    Ok((proposed, index))
}

pub(super) fn read_map(bytes: &[u8]) -> Result<EnvironmentRecord, String> {
    let mut reader = BinaryCursor::new(bytes).map_err(|e| e.to_string())?;
    let map = EnvironmentPreset::read(&mut reader)
        .map_err(|e| format!("Couldn’t read the environment map. {e}"))?;
    if reader.position().map_err(|e| e.to_string())? as usize != bytes.len() {
        return Err("The environment map contains unrecognized trailing data.".into());
    }
    Ok(map.into_environment())
}
