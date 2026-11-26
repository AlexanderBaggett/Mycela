//! Session results own final render snapshots and their displayed projections.
use super::{GraphDocumentView, Node, Ticket, Work, guard};
use crate::{
    export_render::{ExportRenderJob, ExportSource, RenderedScene},
    solid_color::{
        export_settings::ExportSettings,
        map_preview::{self, PreviewSelection},
        preview_export::PreviewPixels,
    },
};
use gpui_kit::base::{Disableable, Selectable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable, WindowExt,
    button::{Button, ButtonVariants, Toggle},
    dialog::{DialogAction, DialogClose, DialogFooter},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    App, AppContext, Bounds, Context, IntoElement, ParentElement, Pixels, Point, RenderImage,
    ScrollWheelEvent, Size, Styled, Window, canvas, div, prelude::*, px, size,
};
use mycela_engine::{PreviewBitmap, SharedImageMap};
use std::{cell::Cell, path::Path, rc::Rc, sync::Arc};

#[path = "graph_document_result_viewport.rs"]
mod viewport;
use viewport::{DenseTiles, Display, ZOOM_LEVELS, Zoom};

const SLOT_COUNT: usize = 6;
pub(super) const TEXTURE_OUTPUT_LABEL: &str = "Texture output";

#[derive(Default)]
pub(super) struct Results {
    slots: [Option<ResultFrame>; SLOT_COUNT],
    selected: usize,
    visible: bool,
    available: bool,
    display: Display,
    viewport_bounds: Rc<Cell<Bounds<Pixels>>>,
    dense_tiles: DenseTiles,
}

impl Results {
    pub(super) fn is_visible(&self) -> bool {
        self.visible
    }

    pub(super) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub(super) fn with_visibility(mut self, visible: bool) -> Self {
        self.set_visible(visible);
        self
    }

    fn current(&self) -> Option<&ResultFrame> {
        self.slots[self.selected].as_ref()
    }

    fn commit(&mut self, slot: usize, frame: ResultFrame) {
        self.slots[slot] = Some(frame);
        self.selected = slot;
        self.display.reset_position();
        self.visible = true;
        self.available = true;
    }
}

struct ResultFrame {
    source: String,
    time: f64,
    image: SharedImageMap,
    choices: Arc<[PreviewSelection]>,
    projected: Projection,
}

struct Projection {
    selection: PreviewSelection,
    bitmap: PreviewBitmap,
    raster: Arc<RenderImage>,
}

impl Projection {
    fn native_size(&self) -> Size<Pixels> {
        size(
            px(self.bitmap.width() as f32),
            px(self.bitmap.height() as f32),
        )
    }
}

impl ResultFrame {
    fn prepare(scene: RenderedScene, source: String, time: f64) -> Result<Self, String> {
        // Final graph caches may retain aliases. The slot owns independent data.
        let image = scene.image.deep_copy().map_err(|error| error.to_string())?;
        let choices = map_preview::choices(&image).into();
        let raster = map_preview::raster(&scene.bitmap).map_err(|error| error.to_string())?;
        Ok(Self {
            source,
            time,
            image,
            choices,
            projected: Projection {
                selection: scene.selection,
                bitmap: scene.bitmap,
                raster,
            },
        })
    }

    fn label(&self) -> String {
        format!(
            "{} · {} × {} · time {}",
            self.source,
            self.projected.bitmap.width(),
            self.projected.bitmap.height(),
            self.time
        )
    }
}

#[derive(Clone, Copy)]
pub(super) enum RenderTarget {
    Texture,
    Selection,
}

#[derive(Clone)]
struct ResultRequest {
    target: RenderTarget,
    occurrence: u64,
    revision: u64,
    slot: usize,
    source: String,
    time: f64,
    map: PreviewSelection,
}

impl GraphDocumentView {
    pub(super) fn texture_output(&self) -> Option<Node> {
        guard(&self.document).branch(0).flatten()
    }

    pub(super) fn result_target(&self, target: RenderTarget) -> Option<Node> {
        match target {
            RenderTarget::Texture => self.texture_output(),
            RenderTarget::Selection => self.selection_preview_target(),
        }
    }

    fn result_target_label(&self, target: RenderTarget) -> &'static str {
        match target {
            RenderTarget::Texture => "Render texture",
            RenderTarget::Selection if self.preview_lab_section => "Render Lab section",
            RenderTarget::Selection => "Render selected node",
        }
    }

    pub(super) fn result_source(&self, target: RenderTarget, node: &Node) -> String {
        let document = self.document_name();
        let label = match target {
            RenderTarget::Texture => {
                format!(
                    "{TEXTURE_OUTPUT_LABEL} · {}",
                    String::from_utf16_lossy(&node.identity_name())
                )
            }
            RenderTarget::Selection if self.preview_lab_section => self
                .lab_sections
                .selected_label()
                .unwrap_or("Lab section")
                .into(),
            RenderTarget::Selection => String::from_utf16_lossy(&node.identity_name()),
        };
        format!("{document} · {label}")
    }

    fn result_map(
        &self,
        target: RenderTarget,
        node: &Node,
        cx: &App,
    ) -> Result<PreviewSelection, String> {
        if matches!(target, RenderTarget::Selection)
            && self
                .preview_target()
                .is_some_and(|preview| preview.same_reference(node))
        {
            self.export_preview_map(cx)
        } else {
            Ok(PreviewSelection::BaseColor)
        }
    }

    fn open_result_render(
        &mut self,
        target: RenderTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() {
            return;
        }
        let Some(node) = self.result_target(target) else {
            if matches!(target, RenderTarget::Texture) {
                self.error = Some(format!(
                    "Connect a node to {TEXTURE_OUTPUT_LABEL} before rendering."
                ));
                cx.notify();
            }
            return;
        };
        let map = match self.result_map(target, &node, cx) {
            Ok(map) => map,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.pause_preview();
        let title = self.result_target_label(target);
        let request = ResultRequest {
            target,
            occurrence: node.occurrence_id(),
            revision: self.revision,
            slot: self.results.selected,
            source: self.result_source(target, &node),
            time: self.sample_time,
            map,
        };
        let destination = if self.results.current().is_some() {
            format!("Replace result in slot {}", request.slot + 1)
        } else {
            format!("Save to slot {}", request.slot + 1)
        };
        let description = format!(
            "{} · {} · time {}. {destination}.",
            request.source,
            request.map.label(),
            request.time
        );
        let settings = cx.new(|cx| ExportSettings::new(description, window, cx));
        let owner = cx.weak_entity();
        self.modal = true;
        window.open_dialog(cx, move |dialog, _, _| {
            let settings = settings.clone();
            let confirm = settings.clone();
            let owner = owner.clone();
            let close = owner.clone();
            let request = request.clone();
            dialog
                .title(title)
                .child(settings)
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Render")),
                        ),
                )
                .on_ok(move |_, _, cx| {
                    let result = confirm.read(cx).dimensions(cx).and_then(|(width, height)| {
                        owner
                            .update(cx, |view, cx| {
                                view.begin_result_render(&request, width, height, cx)
                            })
                            .unwrap_or_else(|_| Err("The graph window closed.".into()))
                    });
                    match result {
                        Ok(()) => true,
                        Err(error) => {
                            confirm.update(cx, |settings, cx| settings.set_error(error, cx));
                            false
                        }
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

    fn begin_result_render(
        &mut self,
        request: &ResultRequest,
        width: i32,
        height: i32,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let node = self
            .result_target(request.target)
            .filter(|node| node.occurrence_id() == request.occurrence)
            .ok_or("The render target changed. Choose it again.")?;
        if self.closed
            || self.work.is_some()
            || self.revision != request.revision
            || self.results.selected != request.slot
            || self.sample_time.to_bits() != request.time.to_bits()
            || self.result_map(request.target, &node, cx)? != request.map
        {
            return Err("The render settings changed. Open the render dialog again.".into());
        }
        let pending = ExportRenderJob::freeze_at_time(
            ExportSource::Graph {
                node: &node,
                camera: None,
                time: request.time,
            },
            None,
            &[],
            request.map.clone(),
            width,
            height,
            request.time,
        )
        .map_err(|error| error.to_string())?
        .with_graph_cache(self.export_cache.clone())
        .with_environment_library(self.environment_library.clone());
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::RenderingResult);
        self.error = None;
        self.status = None;
        let background = cx.background_executor().clone();
        let task_ticket = ticket.clone();
        let request = request.clone();
        cx.spawn(async move |owner, cx| {
            let slot = request.slot;
            let rendered = background
                .spawn(async move {
                    let job = pending.materialize().map_err(|error| error.to_string())?;
                    let scene = job
                        .render_scene(|| !task_ticket.is_current(), false)
                        .map_err(|error| error.to_string())?;
                    if !task_ticket.is_current() {
                        return Ok(None);
                    }
                    ResultFrame::prepare(scene, request.source, request.time).map(Some)
                })
                .await;
            let _ = owner.update(cx, |view, cx| {
                if !view.owns(&ticket) {
                    return;
                }
                view.ticket = None;
                view.work = None;
                ticket.retire();
                match rendered {
                    Ok(Some(frame)) => {
                        view.results.commit(slot, frame);
                        view.status = Some(format!("Rendered to slot {}.", slot + 1));
                    }
                    Ok(None) => {}
                    Err(error) => view.error = Some(format!("Couldn’t render the result: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        Ok(())
    }

    pub(super) fn cancel_result_render(&mut self, cx: &mut Context<Self>) {
        if self.work != Some(Work::RenderingResult) {
            return;
        }
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
        self.work = None;
        self.status = Some("Render cancelled.".into());
        cx.notify();
    }

    fn select_result_map(
        &mut self,
        slot: usize,
        selection: PreviewSelection,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() || self.results.selected != slot {
            return;
        }
        let Some(frame) = self.results.current() else {
            return;
        };
        if frame.projected.selection == selection || !frame.choices.contains(&selection) {
            return;
        }
        let image = frame.image.clone();
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::ProjectingResult);
        self.error = None;
        self.status = None;
        let background = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let projected = background
                .spawn(async move {
                    map_preview::project(&image, &selection).map(|(bitmap, raster)| Projection {
                        selection,
                        bitmap,
                        raster,
                    })
                })
                .await;
            let _ = owner.update(cx, |view, cx| {
                if !view.owns(&ticket) {
                    return;
                }
                view.work = None;
                view.ticket = None;
                ticket.retire();
                match projected {
                    Ok(projected) => {
                        if let Some(frame) = view.results.slots[slot].as_mut() {
                            frame.projected = projected;
                        }
                    }
                    Err(error) => view.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn export_result_png(&mut self, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() {
            return;
        }
        let Some(frame) = self.results.current() else {
            return;
        };
        let bitmap = frame.projected.bitmap.clone();
        let directory = self
            .path
            .as_ref()
            .or(self.imported_from.as_ref())
            .and_then(|path| path.parent())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let prompt = cx.prompt_for_new_path(&directory, Some("mycela-result.png"));
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::ChoosingExport);
        self.error = None;
        self.status = None;
        let background = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let selected = prompt.await;
            if !owner
                .read_with(cx, |view, _| view.owns(&ticket))
                .unwrap_or(false)
            {
                return;
            }
            let result = match selected {
                Ok(Ok(Some(path))) => {
                    let _ = owner.update(cx, |view, cx| {
                        view.work = Some(Work::Exporting);
                        cx.notify();
                    });
                    let work = ticket.clone();
                    background
                        .spawn(async move {
                            PreviewPixels::capture_rendered(&bitmap)?
                                .save_while(path, &|| work.is_current())
                        })
                        .await
                }
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(format!("Couldn’t open the save dialog: {error}")),
                Err(_) => Err("The save dialog closed before returning a result.".into()),
            };
            let _ = owner.update(cx, |view, cx| {
                if !view.owns(&ticket) {
                    return;
                }
                view.work = None;
                view.ticket = None;
                ticket.retire();
                match result {
                    Ok(Some(path)) => {
                        view.remember_export_folder(&path, cx);
                        view.status = Some(format!("Saved PNG to {}.", path.display()))
                    }
                    Ok(None) => {}
                    Err(error) => view.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_result_menu(
        &self,
        busy: bool,
        results_visible: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let owner = cx.weak_entity();
        let render_choices = [RenderTarget::Texture, RenderTarget::Selection].map(|target| {
            let node = self.result_target(target);
            let command = self.result_target_label(target);
            let label = match (target, node.as_ref()) {
                (RenderTarget::Texture, Some(output)) => {
                    let name = String::from_utf16_lossy(&output.identity_name());
                    format!("{command} ({name})…")
                }
                (RenderTarget::Texture, None) => format!("{command} (no output connected)"),
                (RenderTarget::Selection, _) => format!("{command}…"),
            };
            (target, label, node.is_some())
        });
        let sequence_choices = [RenderTarget::Texture, RenderTarget::Selection].map(|target| {
            let label = format!("{} animation…", self.result_target_label(target));
            (target, label, self.result_target(target).is_some())
        });
        let stop_available = self
            .sequence_progress
            .as_ref()
            .is_some_and(|progress| progress.running());
        let export_available = !busy && !results_visible && self.preview_target().is_some();
        Button::new("graph-render-result")
            .label("Render")
            .small()
            .ghost()
            .tab_index(0)
            .dropdown_menu(move |mut menu, _, _| {
                for (target, label, available) in render_choices.iter().cloned() {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .disabled(busy || !available)
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.open_result_render(target, window, cx);
                                });
                            }),
                    );
                }
                menu = menu.separator();
                for (target, label, available) in sequence_choices.iter().cloned() {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .disabled(busy || !available)
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.open_sequence(target, window, cx);
                                });
                            }),
                    );
                }
                if stop_available {
                    let owner = owner.clone();
                    menu =
                        menu.item(PopupMenuItem::new("Stop render").on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |view, cx| view.stop_sequence(cx));
                        }));
                }
                menu.separator().item(
                    PopupMenuItem::new("Export preview image…")
                        .disabled(!export_available)
                        .action(Box::new(super::ExportGraphImage)),
                )
            })
    }

    pub(super) fn render_results_navigation(
        &self,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .h_flex()
            .gap_2()
            .when(self.workspace.is_none() && self.results.available, |row| {
                row.child(
                    Button::new("graph-toggle-results")
                        .small()
                        .tab_index(0)
                        .label(if self.results.visible {
                            "Back to graph"
                        } else {
                            "Results"
                        })
                        .disabled(busy)
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.results.visible = !view.results.visible;
                            if view.results.visible {
                                view.pause_preview();
                            }
                            cx.notify();
                        })),
                )
            })
            .when(self.work == Some(Work::RenderingResult), |row| {
                row.child(
                    Button::new("graph-cancel-result")
                        .small()
                        .tab_index(0)
                        .label("Cancel render")
                        .on_click(cx.listener(|view, _, _, cx| view.cancel_result_render(cx))),
                )
            })
    }

    pub(super) fn render_results(
        &self,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let current = self.results.current();
        let active = self.results.selected;
        let theme = cx.theme();
        div()
            .id("graph-results-workspace")
            .v_flex()
            .size_full()
            .min_h_0()
            .p_3()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children((0..SLOT_COUNT).map(|index| {
                        let number = index + 1;
                        let occupied = self.results.slots[index].is_some();
                        let state = if occupied { "rendered" } else { "empty" };
                        Button::new(("graph-result-slot", index))
                            .small()
                            .tab_index(0)
                            .label(number.to_string())
                            .accessibility_label(format!("Slot {number}, {state}"))
                            .tooltip(format!("Slot {number} · {state}"))
                            .selected(index == active)
                            .disabled(busy)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                if view.results.selected != index {
                                    view.results.selected = index;
                                    view.results.display.reset_position();
                                    cx.notify();
                                }
                            }))
                    }))
                    .children(self.result_map_picker(current, active, busy, cx))
                    .when(current.is_some(), |tools| {
                        tools.child(self.result_zoom_menu(cx)).child(
                            Toggle::new("graph-result-tile")
                                .label("Tile image")
                                .tooltip("Tile image")
                                .small()
                                .checked(self.results.display.tiled)
                                .on_click(cx.listener(|view, tiled, _, cx| {
                                    view.results.display.set_tiled(*tiled);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("graph-export-result")
                            .small()
                            .tab_index(0)
                            .label("Export PNG…")
                            .disabled(busy || current.is_none())
                            .on_click(cx.listener(|view, _, _, cx| view.export_result_png(cx))),
                    )
                    .child(
                        Button::new("graph-clear-result")
                            .small()
                            .ghost()
                            .tab_index(0)
                            .label("Clear result")
                            .disabled(busy || current.is_none())
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.results.slots[view.results.selected] = None;
                                view.status = None;
                                view.error = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(current.map_or_else(
                        || format!("Slot {} is empty", active + 1),
                        ResultFrame::label,
                    )),
            )
            .when_some(current, |view, frame| {
                view.child(self.result_viewport(frame, cx))
            })
    }

    fn change_result_zoom(
        &mut self,
        zoom: Zoom,
        pivot: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let Some(frame) = self.results.current() else {
            return;
        };
        let native = frame.projected.native_size();
        let area = self.results.viewport_bounds.get();
        self.results
            .display
            .set_zoom(zoom, area, native, pivot.unwrap_or_else(|| area.center()));
        cx.notify();
    }

    fn result_zoom_menu(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let owner = cx.weak_entity();
        let current = self.results.display.zoom;
        let label = format!("Zoom: {}", current.label());
        Button::new("graph-result-zoom")
            .small()
            .tab_index(0)
            .label(label.clone())
            .accessibility_label(label)
            .dropdown_menu(move |mut menu, _, _| {
                for choice in
                    std::iter::once(Zoom::Fit).chain((0..ZOOM_LEVELS.len()).map(Zoom::Level))
                {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(choice.label())
                            .checked(choice == current)
                            .on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.change_result_zoom(choice, None, cx);
                                });
                            }),
                    );
                }
                menu.scrollable(true)
            })
    }

    fn result_viewport(&self, frame: &ResultFrame, cx: &Context<Self>) -> impl IntoElement + use<> {
        let bounds = self.results.viewport_bounds.clone();
        let dense = self.results.dense_tiles.clone();
        let display = self.results.display;
        let native = frame.projected.native_size();
        let raster = frame.projected.raster.clone();
        div()
            .id("graph-result-viewport")
            .test_support()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(move |view, event: &ScrollWheelEvent, _, cx| {
                let vertical = event.delta.pixel_delta(px(1.)).y;
                if vertical != px(0.) {
                    let area = view.results.viewport_bounds.get();
                    let next = view
                        .results
                        .display
                        .zoom
                        .step(area.size, native, vertical > px(0.));
                    view.change_result_zoom(next, Some(event.position), cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                canvas(
                    move |area, _, cx| {
                        bounds.set(area);
                        (cx.theme().background, cx.theme().muted)
                    },
                    move |area, (light, dark), window, _| {
                        let tile = display.tile(area, native);
                        let background = if display.tiled {
                            area
                        } else {
                            tile.intersect(&area)
                        };
                        viewport::paint_checkerboard(background, window, light, dark);
                        viewport::paint(area, tile, display.tiled, &raster, &dense, window);
                    },
                )
                .absolute()
                .size_full(),
            )
    }

    fn result_map_picker(
        &self,
        frame: Option<&ResultFrame>,
        slot: usize,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let frame = frame.filter(|frame| frame.choices.len() > 1)?;
        let choices = frame.choices.clone();
        let selected = frame.projected.selection.clone();
        let owner = cx.weak_entity();
        let label = selected.label();
        let button = Button::new("graph-result-map")
            .small()
            .tab_index(0)
            .label(label.clone())
            .accessibility_label(format!("Result: {label}"))
            .tooltip(label)
            .max_w_40()
            .dropdown_caret(true);
        Some(if busy {
            button.disabled(true).into_any_element()
        } else {
            button
                .dropdown_menu(move |mut menu, _, _| {
                    for selection in choices.iter() {
                        let owner = owner.clone();
                        let choice = selection.clone();
                        menu = menu.item(
                            PopupMenuItem::new(selection.label())
                                .checked(*selection == selected)
                                .on_click(move |_, _, cx| {
                                    let _ = owner.update(cx, |view, cx| {
                                        view.select_result_map(slot, choice.clone(), cx);
                                    });
                                }),
                        );
                    }
                    menu.scrollable(true)
                })
                .into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ForegroundError;
    use mycela_engine::{PointwiseFilter, PointwiseKind, PointwiseNode};

    #[test]
    fn retained_result_slots_hold_frozen_maps_after_source_edits() {
        let original = PointwiseNode::<ForegroundError>::solid_color().unwrap();
        original.set_solid_color([0.2, 0.4, 0.8]).unwrap();
        original.set_solid_transparency(0.25).unwrap();
        let store = PointwiseFilter::new(PointwiseKind::StoreMap).unwrap();
        let name: Vec<_> = "Height".encode_utf16().collect();
        store.parameter(0).unwrap().set_text(Some(&name)).unwrap();
        let node = store.node();
        node.slot(0).unwrap().set_node(Some(&original)).unwrap();
        let selection = PreviewSelection::Named {
            name,
            occurrence: 0,
        };
        let freeze = |map| {
            ExportRenderJob::freeze_at_time(
                ExportSource::Graph {
                    node: &node,
                    camera: None,
                    time: 0.25,
                },
                None,
                &[],
                map,
                7,
                3,
                0.25,
            )
            .unwrap()
        };

        let pending = freeze(selection.clone());
        original.set_solid_color([0.8, 0.4, 0.2]).unwrap();
        let scene = pending
            .materialize()
            .unwrap()
            .render_scene(|| false, false)
            .unwrap();
        let rendered_image = scene.image.clone();
        let mut results = Results::default();
        results.commit(
            0,
            ResultFrame::prepare(scene, "First".into(), 0.25).unwrap(),
        );
        assert!(results.is_visible());
        assert!(
            !results
                .current()
                .unwrap()
                .image
                .same_reference(&rendered_image)
        );
        assert_eq!(results.current().unwrap().projected.selection, selection);

        let later = freeze(PreviewSelection::BaseColor)
            .materialize()
            .unwrap()
            .render_scene(|| false, false)
            .unwrap();
        results.commit(
            1,
            ResultFrame::prepare(later, "Second".into(), 0.25).unwrap(),
        );
        assert_eq!(results.current().unwrap().source, "Second");
        results.selected = 0;
        let first = results.current().unwrap();
        let exported = PreviewPixels::capture_rendered(&first.projected.bitmap)
            .unwrap()
            .into_rgba();
        assert_eq!(exported, [51, 51, 51, 191].repeat(7 * 3));
        let base = map_preview::project_bitmap(&first.image, &PreviewSelection::BaseColor).unwrap();
        assert_eq!(base.argb_pixel(0, 0).unwrap(), 0xbf3366cc);
        results.slots[results.selected] = None;
        assert!(results.current().is_none());
        assert_eq!(results.slots[1].as_ref().unwrap().source, "Second");
    }
}

#[cfg(test)]
#[path = "graph_document_results_ui_tests.rs"]
mod ui_tests;
