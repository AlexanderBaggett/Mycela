use super::synthesis::{PreviewMode, TextureContext};
use super::{GraphDocumentView, Node, results::TEXTURE_OUTPUT_LABEL};
use gpui_kit::base::{Disableable, Selectable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    Sizable,
    button::Button,
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    AnyElement, Bounds, Context, Corners, InteractiveElement, IntoElement, ParentElement,
    RenderImage, Styled, StyledImage, canvas, div, img, point, size,
};
use std::sync::Arc;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum PreviewScope {
    #[default]
    Selection,
    TextureOutput,
    WholeTexture,
    CurrentLayer,
}

impl PreviewScope {
    fn label(self) -> &'static str {
        match self {
            Self::Selection => "Selection",
            Self::TextureOutput => TEXTURE_OUTPUT_LABEL,
            Self::WholeTexture => "Whole Texture",
            Self::CurrentLayer => "Current Layer",
        }
    }
}

#[derive(Default)]
pub(super) struct PreviewDisplay {
    scope: PreviewScope,
    tiled: bool,
}

impl PreviewDisplay {
    pub(super) fn pins_texture_output(&self) -> bool {
        self.scope == PreviewScope::TextureOutput
    }
}

impl GraphDocumentView {
    fn active_preview_scope(&self) -> PreviewScope {
        match (self.synthesis_preview_mode(), self.preview_display.scope) {
            (Some(PreviewMode::Current), _) => PreviewScope::CurrentLayer,
            (Some(PreviewMode::Whole), _) => PreviewScope::WholeTexture,
            (None, PreviewScope::Selection) if self.synthesis_whole_is_selection() => {
                PreviewScope::WholeTexture
            }
            (_, scope) => scope,
        }
    }

    pub(super) fn displayed_preview_scope(&self) -> &str {
        if self.preview_lab_section && !self.lab_material_editor_open() {
            self.lab_sections.selected_label().unwrap_or("Lab section")
        } else {
            self.active_preview_scope().label()
        }
    }

    fn supports_preview_display(&self) -> bool {
        self.supports_preview_maps()
            && (!self.preview_lab_section || self.lab_material_editor_open())
    }

    pub(super) fn preview_target(&self) -> Option<Node> {
        if self.preview_uses_texture_output() {
            self.texture_output()
        } else {
            self.selection_preview_target()
        }
    }

    pub(super) fn preview_uses_texture_output(&self) -> bool {
        self.supports_preview_display()
            && !self.preview_lab_section
            && self.preview_display.pins_texture_output()
    }

    fn set_preview_scope(&mut self, scope: PreviewScope, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() || !self.supports_preview_display() {
            return;
        }
        let (scope, synthesis) = match (scope, self.synthesis_preview_context()) {
            (PreviewScope::CurrentLayer, Some(_)) => (None, Some(PreviewMode::Current)),
            (PreviewScope::WholeTexture, Some(TextureContext::Standalone))
                if self.synthesis_whole_is_selection() =>
            {
                (Some(PreviewScope::Selection), None)
            }
            (PreviewScope::WholeTexture, Some(_)) => (None, Some(PreviewMode::Whole)),
            (PreviewScope::CurrentLayer | PreviewScope::WholeTexture, None) => return,
            (scope, _) => (Some(scope), None),
        };
        if let Some(synthesis) = synthesis {
            self.set_synthesis_preview_mode(Some(synthesis), cx);
            return;
        }
        let Some(scope) = scope else {
            return;
        };
        let was_synthesis = self.synthesis_preview_mode().is_some();
        let previous = self.preview_target().map(|node| node.occurrence_id());
        self.preview_display.scope = scope;
        self.set_synthesis_preview_mode(None, cx);
        let current = self.preview_target().map(|node| node.occurrence_id());
        if current != previous || was_synthesis {
            self.start_preview(cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn render_preview_display_controls(
        &self,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if !self.supports_preview_display() {
            return None;
        }
        let context = self.synthesis_preview_context();
        let whole_is_selection = self.synthesis_whole_is_selection();
        let scope = self.active_preview_scope();
        let scopes: &[_] = match context {
            Some(TextureContext::Standalone) if whole_is_selection => &[
                PreviewScope::WholeTexture,
                PreviewScope::CurrentLayer,
                PreviewScope::TextureOutput,
            ],
            Some(TextureContext::Standalone | TextureContext::Embedded) => &[
                PreviewScope::Selection,
                PreviewScope::WholeTexture,
                PreviewScope::CurrentLayer,
                PreviewScope::TextureOutput,
            ],
            Some(TextureContext::Lab) => &[PreviewScope::WholeTexture, PreviewScope::CurrentLayer],
            None => &[PreviewScope::Selection, PreviewScope::TextureOutput],
        };
        let owner = cx.weak_entity();
        let scope_button = Button::new("graph-preview-target")
            .small()
            .tab_index(0)
            .label(scope.label())
            .accessibility_label(format!("Preview target: {}", scope.label()))
            .dropdown_caret(true);
        let scope_button = if context.is_some() {
            scope_button.w(gpui_kit::rems(9.))
        } else {
            scope_button
        };
        let scope_button = if busy {
            scope_button.disabled(true).into_any_element()
        } else {
            scope_button
                .dropdown_menu(move |mut menu, _, _| {
                    for &choice in scopes {
                        let owner = owner.clone();
                        menu = menu.item(
                            PopupMenuItem::new(choice.label())
                                .checked(choice == scope)
                                .on_click(move |_, _, cx| {
                                    let _ = owner.update(cx, |view, cx| {
                                        view.set_preview_scope(choice, cx);
                                    });
                                }),
                        );
                    }
                    menu
                })
                .into_any_element()
        };
        Some(
            div().h_flex().gap_2().child(scope_button).child(
                Button::new("graph-preview-tile-image")
                    .small()
                    .tab_index(0)
                    .label("Tile image")
                    .selected(self.preview_display.tiled)
                    .disabled(busy)
                    .on_click(cx.listener(|view, _, _, cx| {
                        if view.supports_preview_display() && !view.is_busy() {
                            view.preview_display.tiled = !view.preview_display.tiled;
                            cx.notify();
                        }
                    })),
            ),
        )
    }

    pub(super) fn render_preview_raster(
        &self,
        raster: Arc<RenderImage>,
        aspect: f32,
    ) -> AnyElement {
        if self.supports_preview_display() && self.preview_display.tiled {
            tile_grid(raster, aspect).into_any_element()
        } else {
            img(raster)
                .absolute()
                .size_full()
                .object_fit(gpui_kit::ObjectFit::Contain)
                .into_any_element()
        }
    }
}

fn tile_grid(raster: Arc<RenderImage>, aspect: f32) -> impl IntoElement {
    const REPEAT: usize = 3;

    div()
        .id("graph-preview-tile-grid")
        .test_support()
        .absolute()
        .size_full()
        .child(
            canvas(
                move |_, _, _| raster,
                move |bounds, raster, window, _| {
                    let width = bounds.size.width.min(bounds.size.height * aspect);
                    let height = bounds.size.height.min(bounds.size.width / aspect);
                    let origin = bounds.origin
                        + point(
                            (bounds.size.width - width) / 2.,
                            (bounds.size.height - height) / 2.,
                        );
                    let tile = size(width / REPEAT as f32, height / REPEAT as f32);
                    for row in 0..REPEAT {
                        for column in 0..REPEAT {
                            let cell = Bounds::new(
                                origin + point(tile.width * column, tile.height * row),
                                tile,
                            );
                            let _ = window.paint_image(
                                cell,
                                cell,
                                Corners::default(),
                                raster.clone(),
                                0,
                                false,
                            );
                        }
                    }
                },
            )
            .size_full(),
        )
}

#[cfg(test)]
#[path = "graph_document_preview_display_tests.rs"]
mod tests;
