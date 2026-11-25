use super::synthesis::{PatchInputRequests, PatchInputSizes};
use super::*;
use gpui_kit::WeakEntity;
use mycela_engine::{
    EnvironmentResourceLibrary, GtxGraphError, ImageGeometry, LabPreset, LabPresetOperation,
    LabPresetSection,
};

struct PresetBrowser {
    graph: WeakEntity<GraphDocumentView>,
    node: Option<Node>,
    section: LabPresetSection,
    selected: Option<LabPreset>,
    name: Entity<InputState>,
    message: Option<String>,
    error: Option<String>,
    patch_input_ticket: Option<Ticket>,
}

impl Drop for PresetBrowser {
    fn drop(&mut self) {
        self.cancel_patch_inputs();
    }
}

impl PresetBrowser {
    fn perform(
        &mut self,
        operation: LabPresetOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.patch_input_ticket.is_some() {
            return;
        }
        if let LabPresetOperation::Apply(preset) = operation {
            self.prepare_apply(preset, window, cx);
        } else {
            self.commit(operation, &[], window, cx);
        }
    }

    fn patch_inputs(
        &self,
        preset: &LabPreset,
        cx: &App,
    ) -> Result<(PatchInputRequests, EnvironmentResourceLibrary), String> {
        let graph = self.graph.upgrade().ok_or("The graph window closed.")?;
        let graph = graph.read(cx);
        graph.check_lab_selection(self.node.as_ref())?;
        let inputs = guard(&graph.document)
            .connected_patch_inputs_for_lab_preset(self.node.as_ref(), preset)
            .map_err(model::failure)?;
        Ok((
            graph.capture_synthesis_patch_inputs(inputs),
            graph.environment_library.clone(),
        ))
    }

    fn prepare_apply(&mut self, preset: LabPreset, window: &mut Window, cx: &mut Context<Self>) {
        match self.patch_inputs(&preset, cx) {
            Ok((requests, library)) => {
                self.capture_for_apply(preset, requests, library, window, cx)
            }
            Err(error) => self.apply_failed(error, cx),
        }
    }

    fn capture_for_apply(
        &mut self,
        preset: LabPreset,
        requests: PatchInputRequests,
        library: EnvironmentResourceLibrary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ticket = Ticket::new();
        if requests.is_ready() {
            let sizes = requests.render(&library, &ticket);
            self.commit(
                LabPresetOperation::Apply(preset),
                &sizes.dimensions(),
                window,
                cx,
            );
            return;
        }
        self.patch_input_ticket = Some(ticket.clone());
        self.message = None;
        self.error = None;
        let worker = ticket.clone();
        let handle = window.window_handle();
        cx.spawn(async move |owner, cx| {
            let sizes = cx
                .background_spawn(async move { requests.render(&library, &worker) })
                .await;
            if ticket.is_current() {
                let _ = cx.update_window(handle, |_, window, cx| {
                    owner.update(cx, |browser, cx| {
                        browser.finish_capture(preset, sizes, window, cx);
                    })
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn finish_capture(
        &mut self,
        preset: LabPreset,
        sizes: PatchInputSizes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_patch_inputs();
        match self.patch_inputs(&preset, cx) {
            Ok((current, _)) if current.matches(&sizes) => {
                self.commit(
                    LabPresetOperation::Apply(preset),
                    &sizes.dimensions(),
                    window,
                    cx,
                );
            }
            Ok((current, library)) => self.capture_for_apply(preset, current, library, window, cx),
            Err(error) => self.apply_failed(error, cx),
        }
    }

    fn cancel_patch_inputs(&mut self) {
        if let Some(ticket) = self.patch_input_ticket.take() {
            ticket.retire();
        }
    }

    fn apply_failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.message = None;
        self.error = Some(error);
        cx.notify();
    }

    fn commit(
        &mut self,
        operation: LabPresetOperation,
        input_sizes: &[(u64, ImageGeometry)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected_name = match &operation {
            LabPresetOperation::Save(name) => Some(name.trim().to_owned()),
            LabPresetOperation::Remove(_) => None,
            LabPresetOperation::Apply(preset) | LabPresetOperation::Replace(preset) => {
                Some(preset.name().to_owned())
            }
        };
        let message = match operation {
            LabPresetOperation::Apply(_) => "Preset applied.",
            LabPresetOperation::Save(_) => "Preset saved.",
            LabPresetOperation::Replace(_) => "Preset replaced.",
            LabPresetOperation::Remove(_) => "Preset removed.",
        };
        let saved = matches!(operation, LabPresetOperation::Save(_));
        let result = self
            .graph
            .update(cx, |graph, cx| {
                graph.edit_lab_preset(self.node.as_ref(), operation, input_sizes, window, cx)
            })
            .unwrap_or_else(|_| Err("The graph window closed.".into()));
        match result {
            Ok(section) => {
                self.selected = section
                    .presets
                    .iter()
                    .find(|preset| Some(preset.name()) == selected_name.as_deref())
                    .cloned();
                if saved {
                    self.name.update(cx, |input, cx| {
                        input.set_value(section.suggested_name.clone(), window, cx);
                    });
                }
                self.section = section;
                self.message = Some(message.into());
                self.error = None;
            }
            Err(error) => {
                self.message = None;
                self.error = Some(error);
            }
        }
        cx.notify();
    }
}

impl Render for PresetBrowser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let presets = self.section.presets.clone();
        let current = self.selected.clone();
        let owner = cx.weak_entity();
        let selected = self.selected.is_some();
        let can_capture = self.section.can_capture;
        let preparing = self.patch_input_ticket.is_some();
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(self.section.name.clone()),
            )
            .when(!presets.is_empty(), |container| {
                container
                    .child(
                        Button::new("lab-preset-select")
                            .disabled(preparing)
                            .label(
                                self.selected
                                    .as_ref()
                                    .map_or("Choose preset", LabPreset::name)
                                    .to_owned(),
                            )
                            .dropdown_menu(move |mut menu, _, _| {
                                for preset in &presets {
                                    let checked = current
                                        .as_ref()
                                        .is_some_and(|current| current.same_reference(preset));
                                    let preset = preset.clone();
                                    let owner = owner.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(preset.name().to_owned())
                                            .checked(checked)
                                            .on_click(move |_, _, cx| {
                                                let _ = owner.update(cx, |browser, cx| {
                                                    browser.selected = Some(preset.clone());
                                                    browser.error = None;
                                                    browser.message = None;
                                                    cx.notify();
                                                });
                                            }),
                                    );
                                }
                                menu
                            }),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                Button::new("lab-preset-apply")
                                    .primary()
                                    .label("Apply")
                                    .disabled(!selected || preparing)
                                    .on_click(cx.listener(|browser, _, window, cx| {
                                        if let Some(preset) = browser.selected.clone() {
                                            browser.perform(
                                                LabPresetOperation::Apply(preset),
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            )
                            .child(
                                Button::new("lab-preset-replace")
                                    .label("Replace with current values")
                                    .disabled(!selected || !can_capture || preparing)
                                    .on_click(cx.listener(|browser, _, window, cx| {
                                        if let Some(preset) = browser.selected.clone() {
                                            browser.perform(
                                                LabPresetOperation::Replace(preset),
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            )
                            .child(
                                Button::new("lab-preset-remove")
                                    .ghost()
                                    .label("Remove preset")
                                    .disabled(!selected || preparing)
                                    .on_click(cx.listener(|browser, _, window, cx| {
                                        if let Some(preset) = browser.selected.clone() {
                                            browser.perform(
                                                LabPresetOperation::Remove(preset),
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            ),
                    )
            })
            .when(can_capture, |container| {
                container.child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(
                            Field::new().label("New preset name").child(
                                Input::new(&self.name)
                                    .id("lab-preset-name")
                                    .aria_label("Lab preset name"),
                            ),
                        )
                        .child(
                            Button::new("lab-preset-save")
                                .label("Save current values")
                                .disabled(preparing)
                                .on_click(cx.listener(|browser, _, window, cx| {
                                    let name = browser.name.read(cx).value().to_string();
                                    browser.perform(LabPresetOperation::Save(name), window, cx);
                                })),
                        ),
                )
            })
            .when(
                !can_capture && self.section.presets.is_empty(),
                |container| {
                    container.child("Link an adjuster in this section to save a Lab preset.")
                },
            )
            .when(preparing, |container| {
                container.child(
                    div()
                        .id("lab-preset-patch-input-status")
                        .role(gpui_kit::Role::Status)
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Preparing patch source previews…"),
                )
            })
            .when_some(self.message.clone(), |container, message| {
                container.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(message),
                )
            })
            .when_some(self.error.clone(), |container, error| {
                container.child(
                    div()
                        .id("lab-preset-error")
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error),
                )
            })
    }
}

impl GraphDocumentView {
    pub(super) fn open_lab_presets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() {
            return;
        }
        if self.inspector.is_changed(cx) {
            self.error = Some("Correct or reset field text before opening Lab presets.".into());
            cx.notify();
            return;
        }
        let node = self.selected_node();
        let section = guard(&self.document).lab_presets(node.as_ref());
        let owner = cx.weak_entity();
        let name =
            cx.new(|cx| InputState::new(window, cx).default_value(section.suggested_name.clone()));
        let graph = owner.clone();
        let browser = cx.new(|_| PresetBrowser {
            graph,
            node,
            section,
            selected: None,
            name,
            message: None,
            error: None,
            patch_input_ticket: None,
        });
        self.pause_preview();
        self.modal = true;
        window.open_dialog(cx, move |dialog, _, _| {
            let close = owner.clone();
            let cancel = browser.clone();
            dialog
                .title("Lab presets")
                .child(browser.clone())
                .footer(DialogFooter::new().child(
                    DialogClose::new().child(Button::new("close-lab-presets").label("Close")),
                ))
                .on_close(move |_, _, cx| {
                    cancel.update(cx, |browser, _| browser.cancel_patch_inputs());
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }

    pub(super) fn check_lab_selection(&self, node: Option<&Node>) -> Result<(), String> {
        let selected = self.selected_node();
        let same = match (selected.as_ref(), node) {
            (None, None) => true,
            (Some(actual), Some(expected)) => actual.same_reference(expected),
            _ => false,
        };
        if self.closed || !same {
            return Err("The selected Lab section changed.".into());
        }
        Ok(())
    }

    fn edit_lab_preset(
        &mut self,
        node: Option<&Node>,
        operation: LabPresetOperation,
        input_sizes: &[(u64, ImageGeometry)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<LabPresetSection, String> {
        self.check_lab_selection(node)?;
        self.ensure_history_binding();
        if let Some(history) = self.history() {
            history.separate_next_edit();
        }
        let dirty = self.dirty;
        let stamp = self.begin_history_edit();
        let render = matches!(operation, LabPresetOperation::Apply(_));
        let result = match operation {
            LabPresetOperation::Apply(preset) => guard(&self.document)
                .apply_lab_preset_with_patch_input_sizes(node, &preset, input_sizes),
            _ => guard(&self.document).edit_lab_preset(node, operation),
        };
        if let Err(error) = result {
            if matches!(error, GtxGraphError::LabPreset(_)) {
                self.dirty = dirty;
            }
            return Err(model::failure(error));
        }
        self.finish_history_edit(stamp, true);
        self.error = None;
        self.status = None;
        if render {
            self.revision += 1;
            self.rebuild(window, cx);
            self.start_preview(cx);
        }
        cx.notify();
        Ok(guard(&self.document).lab_presets(node))
    }
}
