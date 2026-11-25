use super::*;
use crate::curve_layer_menu::{Layer, LayerAction, apply_layer_action, render_curve_layer_menu};
use crate::curve_modulation::{
    Control, ModulationAction, ModulationTarget, control_label, is_remap, layer_controls,
    open_child_path, render_modulation_menu,
};
use crate::custom_curve_editor::{CustomAction, CustomDraft};
use crate::graph_document::view::numeric_field::{NumericField, NumericRange};
use crate::tone_curve;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::{AnyElement, rems};
use mycela_engine::{CurveControlKind, SharedCurve};

#[path = "graph_document_custom_plot.rs"]
mod plot;

type Curve = SharedCurve<ForegroundError>;

#[derive(Clone, Copy)]
enum CurveEditorMode {
    Mapping([f32; 2]),
    Remap,
}

impl CurveEditorMode {
    fn bounds(self) -> [f32; 2] {
        match self {
            Self::Mapping(bounds) => bounds,
            Self::Remap => [0., 1.],
        }
    }
}

struct ScalarField {
    control: Control,
    label: String,
    input: Entity<InputState>,
}

struct LayerFields {
    layer: Layer,
    ancestry: Vec<(Layer, Control)>,
    scalars: Vec<ScalarField>,
    custom: Option<CustomDraft>,
    point_drag: Option<crate::custom_curve_plot::PointDrag>,
    _subscriptions: Vec<Subscription>,
}

impl LayerFields {
    fn new(
        layer: Layer,
        ancestry: Vec<(Layer, Control)>,
        window: &mut Window,
        cx: &mut Context<MappingEditor>,
    ) -> Result<Self, String> {
        let custom = CustomDraft::new(&layer, window, cx)?;
        let mut subscriptions = Vec::new();
        let scalars = layer_controls(&layer)
            .filter(|control| !is_remap(control))
            .filter_map(|control| {
                let value = control.scalar().ok()?;
                let input =
                    cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()));
                subscriptions.push(watch_input(&input, cx));
                Some(ScalarField {
                    label: control_label(&control),
                    control,
                    input,
                })
            })
            .collect();
        Ok(Self {
            layer,
            ancestry,
            scalars,
            custom,
            point_drag: None,
            _subscriptions: subscriptions,
        })
    }

    fn root(&self) -> &Layer {
        self.ancestry
            .first()
            .map_or(&self.layer, |(parent, _)| parent)
    }

    fn apply(&self, cx: &App) -> Result<(), String> {
        let mut changes = Vec::new();
        for field in &self.scalars {
            if field.control.kind() == CurveControlKind::Modulated && field.control.is_active() {
                continue;
            }
            let value = field
                .input
                .read(cx)
                .value()
                .trim()
                .parse::<f32>()
                .map_err(|_| format!("Enter a number for {}.", field.label))?;
            if field.control.scalar().map_err(model::failure)? == value {
                continue;
            }
            if !value.is_finite() {
                return Err(format!("Enter a finite value for {}.", field.label));
            }
            if let Some([lo, hi]) = field.control.bounds()
                && !(lo..=hi).contains(&value)
            {
                return Err(format!("{} must be between {lo} and {hi}.", field.label));
            }
            changes.push((&field.control, value));
        }
        for (control, value) in changes {
            control.set_scalar(value).map_err(model::failure)?;
        }
        Ok(())
    }
}

fn watch_input(input: &Entity<InputState>, cx: &mut Context<MappingEditor>) -> Subscription {
    cx.subscribe(input, |editor, _, event, cx| {
        if matches!(event, InputEvent::Change) {
            let _ = editor.update_curve(cx);
            cx.notify();
        }
    })
}

struct MappingEditor {
    curve: Curve,
    mode: CurveEditorMode,
    outputs: [Entity<InputState>; 2],
    original: [f32; 2],
    layer: Option<LayerFields>,
    error: Option<String>,
    plot_focus: FocusHandle,
    _subscriptions: [Subscription; 2],
}

impl MappingEditor {
    fn new(
        curve: Curve,
        mode: CurveEditorMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let original = [curve.minimum(), curve.maximum()];
        let outputs = original
            .map(|value| cx.new(|cx| InputState::new(window, cx).default_value(value.to_string())));
        let subscriptions = outputs.each_ref().map(|input| watch_input(input, cx));
        Self {
            curve,
            mode,
            outputs,
            original,
            layer: None,
            error: None,
            plot_focus: cx.focus_handle().tab_stop(true),
            _subscriptions: subscriptions,
        }
    }

    fn update_curve(&self, cx: &App) -> Result<(), String> {
        let [lo, hi] = self.mode.bounds();
        let mut values = [0.; 2];
        for (index, input) in self.outputs.iter().enumerate() {
            let value = input
                .read(cx)
                .value()
                .trim()
                .parse::<f32>()
                .map_err(|_| "Enter a number for each output bound.")?;
            if !value.is_finite() || (value != self.original[index] && !(lo..=hi).contains(&value))
            {
                return Err(format!("Outputs must be between {lo} and {hi}."));
            }
            values[index] = value;
        }
        if let Some(layer) = &self.layer {
            layer.apply(cx)?;
        }
        self.curve.set_minimum(values[0]);
        self.curve.set_maximum(values[1]);
        Ok(())
    }

    fn layer_action(&mut self, action: LayerAction, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.update_curve(cx).and_then(|()| {
            if let LayerAction::Edit(index) = action {
                let layer = self
                    .curve
                    .layers()
                    .get(index)
                    .and_then(Clone::clone)
                    .ok_or("Choose an available layer.")?;
                if self
                    .layer
                    .as_ref()
                    .is_some_and(|fields| fields.layer.same_reference(&layer))
                {
                    return Ok(());
                }
                self.apply_typed_coordinates(window, cx)?;
                self.layer = Some(LayerFields::new(layer, Vec::new(), window, cx)?);
            } else {
                let curve = &self.curve;
                apply_layer_action(curve, action, |_| Ok(()))?;
                if let Some(fields) = &self.layer
                    && !curve
                        .layers()
                        .into_iter()
                        .flatten()
                        .any(|layer| layer.same_reference(fields.root()))
                {
                    self.layer = None;
                }
            }
            Ok(())
        });
        self.error = result.err();
        cx.notify();
    }

    fn apply_typed_coordinates(&mut self, window: &mut Window, cx: &mut App) -> Result<(), String> {
        if let Some(fields) = &mut self.layer
            && let Some(custom) = &mut fields.custom
            && custom.is_changed(cx)
        {
            custom.apply(&fields.layer, CustomAction::Move, window, cx)?;
        }
        Ok(())
    }

    fn custom_action(
        &mut self,
        expected: &Layer,
        action: CustomAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self
            .layer
            .as_mut()
            .filter(|fields| fields.layer.same_reference(expected))
            .and_then(|fields| fields.custom.as_mut())
            .ok_or_else(|| "The selected Custom layer changed.".to_owned())
            .and_then(|draft| draft.apply(expected, action, window, cx));
        self.error = result.err();
        cx.notify();
    }

    fn modulation_action(
        &mut self,
        target: &ModulationTarget,
        action: ModulationAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = (|| {
            self.update_curve(cx)?;
            let fields = self.layer.as_ref().ok_or("Choose a curve layer first.")?;
            let child = target.apply(&fields.layer, action, || Ok(()))?;
            if let Some(child) = child {
                let ancestry =
                    open_child_path(&fields.ancestry, &fields.layer, target.control(), &child)?;
                self.apply_typed_coordinates(window, cx)?;
                self.layer = Some(LayerFields::new(child, ancestry, window, cx)?);
            }
            Ok::<_, String>(())
        })();
        self.error = result.err();
        cx.notify();
    }

    fn parent_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = (|| {
            self.update_curve(cx)?;
            self.apply_typed_coordinates(window, cx)?;
            let mut ancestry = self
                .layer
                .as_ref()
                .ok_or("Choose a curve layer first.")?
                .ancestry
                .clone();
            let (layer, _) = ancestry.pop().ok_or("The parent layer is unavailable.")?;
            self.layer = Some(LayerFields::new(layer, ancestry, window, cx)?);
            Ok::<_, String>(())
        })();
        self.error = result.err();
        cx.notify();
    }

    fn render_layer(&self, fields: &LayerFields, cx: &Context<Self>) -> AnyElement {
        let form = Form::new()
            .columns(2)
            .children(fields.scalars.iter().map(|field| {
                let modulated = field.control.kind() == CurveControlKind::Modulated
                    && field.control.is_active();
                let label = if modulated {
                    format!("{} (driven by curve)", field.label)
                } else {
                    field.label.clone()
                };
                Field::new().label(label).child(
                    NumericField::new(
                        &field.input,
                        format!("mapping-layer-control-{}", field.control.id()),
                        field.label.clone(),
                        field
                            .control
                            .bounds()
                            .map(|bounds| NumericRange::bounds(bounds, 0.01)),
                    )
                    .disabled(modulated),
                )
            }));
        div()
            .v_flex()
            .gap_2()
            .when(!fields.ancestry.is_empty(), |panel| {
                panel.child(
                    Button::new("mapping-modulation-back")
                        .small()
                        .label("Back to parent layer")
                        .on_click(
                            cx.listener(|editor, _, window, cx| editor.parent_layer(window, cx)),
                        ),
                )
            })
            .child(
                div()
                    .font_semibold()
                    .child(fields.ancestry.last().map_or_else(
                        || format!("{:?} layer", fields.layer.kind()),
                        |(_, control)| {
                            format!(
                                "{} · {:?} layer",
                                control_label(control),
                                fields.layer.kind()
                            )
                        },
                    )),
            )
            .children(layer_controls(&fields.layer).filter_map(|control| {
                let (label, checked) = match control.kind() {
                    CurveControlKind::Toggle => (control_label(&control), control.toggle().ok()?),
                    CurveControlKind::Direction => {
                        ("Reverse direction".into(), control.enumeration().ok()? != 0)
                    }
                    _ => return None,
                };
                Some(
                    Checkbox::new(format!("mapping-layer-toggle-{}", control.id()))
                        .label(label)
                        .checked(checked)
                        .on_change(cx.listener(move |editor, value, _, cx| {
                            let result = if control.kind() == CurveControlKind::Toggle {
                                control.set_toggle(*value)
                            } else {
                                control.set_enumeration(i32::from(*value))
                            };
                            editor.error = result.err().map(model::failure);
                            cx.notify();
                        })),
                )
            }))
            .when_some(fields.custom.as_ref(), |panel, custom| {
                let owner = cx.weak_entity();
                let layer = fields.layer.clone();
                panel
                    .child(self.render_custom_plot(fields, custom, cx))
                    .child(custom.render(
                        "mapping-custom",
                        &fields.layer,
                        false,
                        move |action, window, cx| {
                            let _ = owner.update(cx, |editor, cx| {
                                editor.custom_action(&layer, action, window, cx)
                            });
                        },
                    ))
            })
            .child(
                div().h_flex().flex_wrap().gap_2().children(
                    layer_controls(&fields.layer)
                        .filter(|control| control.kind() == CurveControlKind::Modulated)
                        .map(|control| {
                            let owner = cx.weak_entity();
                            render_modulation_menu(
                                "mapping-modulation",
                                &fields.layer,
                                &control,
                                false,
                                false,
                                move |target, action, window, cx| {
                                    let _ = owner.update(cx, |editor, cx| {
                                        editor.modulation_action(target, action, window, cx)
                                    });
                                },
                            )
                        }),
                ),
            )
            .child(form)
            .into_any_element()
    }
}

impl Render for MappingEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let curve = &self.curve;
        let empty_remap = matches!(self.mode, CurveEditorMode::Remap) && curve.layers().is_empty();
        let preview = if empty_remap {
            Some((0..=64).map(|index| index as f32 / 64.).collect())
        } else {
            tone_curve::samples(curve).ok()
        };
        let owner = cx.weak_entity();
        let menu = render_curve_layer_menu(
            "numeric-mapping",
            curve,
            &curve.layers(),
            false,
            cx,
            move |action, window, cx| {
                let _ = owner.update(cx, |editor, cx| editor.layer_action(action, window, cx));
            },
        );
        let [minimum, maximum] = &self.outputs;
        let output_range = NumericRange::bounds(self.mode.bounds(), 0.01);
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .id("mapping-preview")
                    .role(gpui_kit::Role::Image)
                    .aria_label("Curve from input to output")
                    .test_support()
                    .h(rems(6.))
                    .border_1()
                    .border_color(theme.border)
                    .when_some(preview, |preview, samples| {
                        preview.child(tone_curve::graph(samples, cx))
                    }),
            )
            .when(empty_remap, |form| {
                form.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("An empty curve passes its input through."),
                )
            })
            .child(
                Form::new()
                    .columns(2)
                    .child(
                        Field::new()
                            .label("Minimum output")
                            .child(NumericField::new(
                                minimum,
                                "mapping-output-min",
                                "Minimum output",
                                output_range,
                            )),
                    )
                    .child(
                        Field::new()
                            .label("Maximum output")
                            .child(NumericField::new(
                                maximum,
                                "mapping-output-max",
                                "Maximum output",
                                output_range,
                            )),
                    ),
            )
            .child(menu)
            .when_some(self.layer.as_ref(), |form, fields| {
                form.child(self.render_layer(fields, cx))
            })
            .when_some(self.error.as_ref(), |form, error| {
                form.child(
                    div()
                        .id("numeric-mapping-error")
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error.clone()),
                )
            })
    }
}

impl GraphDocumentView {
    pub(super) fn open_numeric_mapping(
        &mut self,
        target: &LinkTarget,
        axis: LabNumericAxis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        let result = self.check_numeric_link(target, cx).and_then(|()| {
            guard(&self.document)
                .numeric_property_mapping(&target.property, axis)
                .map(Arc::new)
                .map_err(model::failure)
        });
        let mapping = match result {
            Ok(mapping) => mapping,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let target = target.clone();
        let title = match axis {
            LabNumericAxis::Whole => "Edit adjuster mapping",
            LabNumericAxis::X => "Edit X adjuster mapping",
            LabNumericAxis::Y => "Edit Y adjuster mapping",
        };
        self.open_adjuster_mapping_editor(
            mapping.curve().clone(),
            mapping.bounds(),
            title,
            move |view, _, window, cx| {
                view.check_numeric_link(&target, cx)?;
                view.commit_lab_adjuster_change(window, cx, |document| {
                    document.edit_numeric_property_mapping(&mapping)
                })
            },
            window,
            cx,
        );
    }

    pub(in crate::graph_document::view) fn open_adjuster_mapping_editor(
        &mut self,
        curve: Curve,
        bounds: [f32; 2],
        title: &'static str,
        apply: impl Fn(&mut Self, &Curve, &mut Window, &mut Context<Self>) -> Result<(), String>
        + Clone
        + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_curve_editor(
            curve,
            CurveEditorMode::Mapping(bounds),
            title,
            apply,
            window,
            cx,
        );
    }

    pub(in crate::graph_document::view) fn open_remap_editor(
        &mut self,
        curve: Curve,
        title: &'static str,
        apply: impl Fn(&mut Self, &Curve, &mut Window, &mut Context<Self>) -> Result<(), String>
        + Clone
        + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_curve_editor(curve, CurveEditorMode::Remap, title, apply, window, cx);
    }

    fn open_curve_editor(
        &mut self,
        curve: Curve,
        mode: CurveEditorMode,
        title: &'static str,
        apply: impl Fn(&mut Self, &Curve, &mut Window, &mut Context<Self>) -> Result<(), String>
        + Clone
        + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pause_preview();
        self.modal = true;
        let form = cx.new(|cx| MappingEditor::new(curve, mode, window, cx));
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let submit = form.clone();
            let edit = owner.clone();
            let close = owner.clone();
            let apply = apply.clone();
            dialog
                .title(title)
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Apply")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let result = submit
                        .update(cx, |editor, cx| {
                            editor.update_curve(cx)?;
                            editor.apply_typed_coordinates(window, cx)?;
                            Ok(editor.curve.clone())
                        })
                        .and_then(|curve| {
                            edit.update(cx, |view, cx| apply(view, &curve, window, cx))
                                .unwrap_or_else(|_| Err("The graph window closed.".into()))
                        });
                    if let Err(error) = result {
                        submit.update(cx, |editor, cx| {
                            editor.error = Some(error);
                            cx.notify();
                        });
                        return false;
                    }
                    true
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
}
