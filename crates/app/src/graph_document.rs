//! Live GTX document capability. Engine owners hold committed data; the view
//! owns navigation, draft text and versioned asynchronous work only.
#[path = "graph_document_inspector.rs"]
pub(crate) mod inspector;
#[path = "graph_document_model.rs"]
mod model;
#[path = "graph_document_navigation.rs"]
mod navigation;
#[path = "graph_document_view.rs"]
mod view;
pub(crate) use model::LiveDocument;
pub use view::{GraphDocumentView, open_graph_document_window};
pub(crate) use view::{OpenGraphDocument, OpenNodeEditor, open_node_editor_window};
pub(crate) use view::{SourceAnimationFrame, open_solid_animation_window};

pub(crate) use view::custom::{CustomAction, CustomDraft, render_custom_points};
pub(crate) use view::modulation::{ModulationAction, ModulationTarget, render_layer_modulation};
