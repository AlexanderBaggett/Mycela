//! Mycela desktop editor and retained texture controls.
mod color_binding_effects;
mod colorize_gradient;
mod curve_axes;
mod curve_layer_menu;
mod curve_modulation;
mod custom_curve_editor;
mod custom_curve_plot;
mod encoded_storage;
mod export_render;
mod filter_pipeline;
mod filter_stack;
mod gradient_source;
mod graph_document;
mod gtx_import;
mod image_file;
mod input_source;
mod live_invert;
mod memory_capacity;
mod preview_background;
mod scene_backend;
mod solid_color;
mod spatial_filter;
#[cfg(feature = "texture-workspace-test")]
mod texture_workspace_test;
mod tone_curve;
pub use filter_stack::FilterStackView;
use gpui_kit::{App, Entity};
pub use gradient_source::{GradientSourceView, TextureSourceKind};
pub use graph_document::{GraphDocumentView, open_graph_document_window};
pub use solid_color::{ForegroundCommand, ForegroundDispatcher, ForegroundError, SolidColorView};
#[cfg(feature = "texture-workspace-test")]
#[doc(hidden)]
pub use texture_workspace_test::open_main_window;

/// Pixel width and height requested by the desktop live previews.
pub const LIVE_PREVIEW_SIZE: i32 = 512;

/// Open the desktop document workspace with a new project.
pub fn open_desktop_window(
    cx: &mut App,
) -> Result<(gpui_kit::AnyWindowHandle, Entity<GraphDocumentView>), String> {
    encoded_storage::install().map_err(|e| e.to_string())?;
    graph_document::open_node_editor_window(cx)
}

#[cfg(test)]
#[path = "../tests/common/filter_catalog.rs"]
mod filter_catalog_test;
