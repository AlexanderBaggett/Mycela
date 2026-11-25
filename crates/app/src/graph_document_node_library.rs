//! The canvas node catalog and its per-document tree state.
use super::{GraphDocumentView, canvas::GraphDragLabel};
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Icon, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    list::ListItem,
    tree::{Tree, TreeItem, TreeState},
};
use gpui_kit::{
    App, AppContext, Context, Entity, EntityId, IntoElement, SharedString, Subscription, Window,
    assets::IconName, div, prelude::*, rems,
};

const NODE_PREFIX: &str = "graph-library-node-";
const ADVANCED_ID: &str = "graph-library-category-advanced";
const IMAGE_ID: &str = "graph-library-category-image";
const THREE_D_ID: &str = "graph-library-category-3d";

pub(super) struct LibraryNodeDrag {
    pub(super) identity: String,
    pub(super) owner: EntityId,
    pub(super) revision: u64,
    pub(super) selection_serial: u64,
}

struct Family {
    id: &'static str,
    name: &'static str,
    nodes: &'static [&'static str],
}

// Reuse the original registry's families for matching identities.
const ADVANCED: Family = Family {
    id: ADVANCED_ID,
    name: "Advanced",
    nodes: &[
        "Canvas",
        "Cut & Tile Lab",
        "Fiber Lab",
        "Layer Lab",
        "Noise Lab",
        "Select Noise",
        "Select Pattern",
        "Shader",
        "Style",
        "Substance Lab",
        "Weather Lab",
        "Wood Lab",
    ],
};

const IMAGE_FAMILIES: &[Family] = &[
    Family {
        id: "graph-library-category-image-3d",
        name: "3D effects",
        nodes: &[
            "Caustics",
            "Full Light",
            "Point Light",
            "Reflect",
            "Refract",
            "Shadow",
            "Stereogram",
            "Transmit",
        ],
    },
    Family {
        id: "graph-library-category-blur",
        name: "Blur",
        nodes: &[
            "Art Blur",
            "Gaussian Blur",
            "Motion Blur",
            "Shape Blur",
            "Smear Blur",
            "Smart Blur",
            "Variable Blur",
        ],
    },
    Family {
        id: "graph-library-category-color",
        name: "Color",
        nodes: &[
            "Change BCI",
            "Change HSB",
            "Colorize",
            "Equalize",
            "Invert",
            "Levels",
            "Match Color",
            "Tone Curve",
        ],
    },
    Family {
        id: "graph-library-category-combine",
        name: "Combine",
        nodes: &["A + B", "A * B", "Combine", "From HSB"],
    },
    Family {
        id: "graph-library-category-effect-map",
        name: "Effect map",
        nodes: &[
            "Clear Maps",
            "Fix Map Contrast",
            "Layer Maps",
            "Merge Maps",
            "Store Map",
        ],
    },
    Family {
        id: "graph-library-category-environment",
        name: "Environment",
        nodes: &[
            "Convert Environment",
            "Diffuse Environment",
            "Exposure",
            "Make Environment",
            "Reflect Environment",
            "Refract Environment",
            "Rotate Environment",
            "Scatter Environment",
        ],
    },
    Family {
        id: "graph-library-category-filter",
        name: "Filter",
        nodes: &[
            "Fractalize",
            "Frequency",
            "Get Channel",
            "Matrix",
            "Pixelate",
            "Selector",
            "Sharpen",
        ],
    },
    Family {
        id: "graph-library-category-generate",
        name: "Generate",
        nodes: &[
            "Atmosphere",
            "Bricks & Boards",
            "Cells",
            "Gradient",
            "Hair",
            "Imported Image",
            "Noise",
            "Oklab Color",
            "Polygon",
            "Regions",
            "Solid Color",
            "Wood Rings",
        ],
    },
    Family {
        id: "graph-library-category-group",
        name: "Group",
        nodes: &["Group", "Reference"],
    },
    Family {
        id: "graph-library-category-transform",
        name: "Transform",
        nodes: &[
            "Distort",
            "Free Transform",
            "Kaleidoscope",
            "Move & Repeat",
            "Remap",
            "Rotate",
            "Rotate & Tile",
        ],
    },
];

const THREE_D_FAMILIES: &[Family] = &[
    Family {
        id: "graph-library-category-3d-generate",
        name: "Generate",
        nodes: &[
            "Cone",
            "Cube",
            "Cylinder",
            "Disc",
            "Import 3D",
            "Plane",
            "Sphere",
            "Torus",
        ],
    },
    Family {
        id: "graph-library-category-3d-edit",
        name: "Edit",
        nodes: &["Clear Lights", "Combine 3D", "Transform 3D"],
    },
    Family {
        id: "graph-library-category-3d-uv",
        name: "UV",
        nodes: &[
            "UV Ambient Occlusion",
            "UV Apply",
            "UV Curvature",
            "UV Environment Light",
            "UV Environment Occlusion",
            "UV Light",
            "UV Multi Project",
            "UV Project",
        ],
    },
];

impl Family {
    fn item(&self, query: &str) -> Option<TreeItem> {
        let mut leaves = self
            .nodes
            .iter()
            .filter(|name| query.is_empty() || name.to_ascii_lowercase().contains(query))
            .map(|&name| TreeItem::new(format!("{NODE_PREFIX}{name}"), name))
            .peekable();
        leaves.peek()?;
        Some(
            TreeItem::new(self.id, self.name)
                .expanded(!query.is_empty())
                .children(leaves),
        )
    }
}

fn library_items(query: &str) -> Vec<TreeItem> {
    let category = |id, name, families: &[Family], expanded| {
        let mut children = families
            .iter()
            .filter_map(|family| family.item(query))
            .peekable();
        children.peek()?;
        Some(
            TreeItem::new(id, name)
                .expanded(expanded)
                .children(children),
        )
    };
    [
        ADVANCED.item(query),
        category(IMAGE_ID, "Image Nodes", IMAGE_FAMILIES, true),
        category(THREE_D_ID, "3D Nodes", THREE_D_FAMILIES, !query.is_empty()),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub(super) struct NodeLibrary {
    tree: Entity<TreeState>,
    search: Entity<InputState>,
    roots: Vec<TreeItem>,
    query: String,
    selection_before_search: Option<SharedString>,
    _tree_changes: Subscription,
    _search_changes: Subscription,
}

impl NodeLibrary {
    pub(super) fn new(window: &mut Window, cx: &mut Context<GraphDocumentView>) -> Self {
        let roots = library_items("");
        let tree = cx.new(|cx| TreeState::new(cx).items(roots.clone()));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search nodes"));
        let tree_changes = cx.observe(&tree, |_, _, cx| cx.notify());
        let search_changes = cx.subscribe(&search, |view, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input.read(cx).value().trim().to_ascii_lowercase();
                view.node_library.filter(query, cx);
            }
        });
        Self {
            tree,
            search,
            roots,
            query: String::new(),
            selection_before_search: None,
            _tree_changes: tree_changes,
            _search_changes: search_changes,
        }
    }

    fn filter(&mut self, query: String, cx: &mut Context<GraphDocumentView>) {
        if self.query == query {
            return;
        }
        let previous = self
            .tree
            .read(cx)
            .selected_item()
            .map(|item| item.id.clone());
        if self.query.is_empty() {
            self.selection_before_search = previous.clone();
        }
        let (items, selection) = if query.is_empty() {
            // TreeItem clones share expansion state. Filtered results use fresh
            // items so their expanded ancestors cannot change the normal tree.
            (self.roots.clone(), self.selection_before_search.take())
        } else {
            (library_items(&query), previous)
        };
        self.query = query;
        self.tree.update(cx, |tree, cx| {
            tree.set_items(items, cx);
            let selected = selection.and_then(|id| tree.index_of(&id));
            tree.set_selected_index(selected, cx);
            tree.scroll_to_item(selected.unwrap_or(0), gpui_kit::ScrollStrategy::Top);
        });
    }

    fn selected_identity<'a>(&self, cx: &'a App) -> Option<&'a str> {
        self.tree
            .read(cx)
            .selected_item()?
            .id
            .strip_prefix(NODE_PREFIX)
    }
}

impl GraphDocumentView {
    pub(super) fn render_node_library(
        &self,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let library = &self.node_library;
        let matches = library.tree.read(cx).entry(0).is_some();
        let tree = library.tree.clone();
        let owner = cx.entity_id();
        let revision = self.revision;
        let selection_serial = self.selection_serial;
        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .gap_2()
            .child(
                Input::new(&library.search)
                    .id("graph-library-search")
                    .aria_label("Search nodes")
                    .small()
                    .disabled(busy),
            )
            .child(
                div()
                    .id("graph-library-tree")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .when(matches, |panel| {
                        panel.child(Tree::new(&library.tree, move |_, entry, _, _, cx| {
                            let tree = tree.clone();
                            let chevron = entry.is_folder().then(|| {
                                if entry.is_expanded() {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                }
                            });
                            let identity = entry.item().id.strip_prefix(NODE_PREFIX);
                            ListItem::new(entry.item().id.clone())
                                .accessibility_label(entry.item().label.clone())
                                .when_some(identity.filter(|_| !busy), |item, identity| {
                                    item.cursor_grab().on_drag(
                                        LibraryNodeDrag {
                                            identity: identity.to_owned(),
                                            owner,
                                            revision,
                                            selection_serial,
                                        },
                                        |drag, _, _, cx| {
                                            cx.new(|_| GraphDragLabel(drag.identity.clone()))
                                        },
                                    )
                                })
                                .child(
                                    div()
                                        .h_flex()
                                        .min_w_0()
                                        .gap_1()
                                        .pl(rems(entry.depth() as f32 * 0.5))
                                        .child(div().w_4().flex_none().when_some(
                                            chevron,
                                            |slot, icon| {
                                                slot.child(
                                                    Icon::new(icon)
                                                        .xsmall()
                                                        .text_color(cx.theme().muted_foreground),
                                                )
                                            },
                                        ))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .child(entry.item().label.clone()),
                                        ),
                                )
                                .on_click(move |_, window, cx| {
                                    tree.update(cx, |tree, cx| tree.focus(window, cx));
                                })
                        }))
                    })
                    .when(!matches, |panel| {
                        panel.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No matching nodes"),
                        )
                    }),
            )
            .child(
                div()
                    .h_flex()
                    .flex_none()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        Button::new("graph-library-browse")
                            .small()
                            .ghost()
                            .label("Browse")
                            .tooltip("Move keyboard focus to the node tree")
                            .tab_index(0)
                            .disabled(busy || !matches)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.node_library.tree.update(cx, |tree, cx| {
                                    if tree.selected_index().is_none() {
                                        tree.set_selected_index(Some(0), cx);
                                    }
                                    tree.focus(window, cx);
                                });
                            })),
                    )
                    .child(
                        Button::new("graph-library-insert")
                            .small()
                            .label("Add node")
                            .tab_index(0)
                            .disabled(busy || library.selected_identity(cx).is_none())
                            .on_click(cx.listener(|view, _, window, cx| {
                                if let Some(identity) =
                                    view.node_library.selected_identity(cx).map(str::to_owned)
                                {
                                    view.canvas_add_node(
                                        &identity,
                                        view.revision,
                                        view.selection_serial,
                                        window,
                                        cx,
                                    );
                                }
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        HistoryDirection, NODES_WITH_INPUTS, Node, STANDALONE_NODES, Target, model,
        open_node_editor_window,
    };
    use super::*;
    use gpui_kit::{ScrollDelta, TestAppContext, point, px, size, test::TestWindowExt};
    use std::sync::{Arc, Mutex};

    fn leaves(items: &[TreeItem]) -> Vec<&str> {
        items
            .iter()
            .flat_map(|item| {
                if item.is_folder() {
                    leaves(&item.children)
                } else {
                    vec![item.id.strip_prefix(NODE_PREFIX).unwrap()]
                }
            })
            .collect()
    }

    #[test]
    fn node_library_lists_each_insertable_node_once() {
        let roots = library_items("");
        let mut actual = leaves(&roots);
        let mut expected: Vec<_> = STANDALONE_NODES
            .iter()
            .chain(NODES_WITH_INPUTS)
            .copied()
            .collect();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected);
        assert!(actual.windows(2).all(|pair| pair[0] != pair[1]));

        let filtered = library_items("gauss");
        assert_eq!(leaves(&filtered), ["Gaussian Blur"]);
        assert!(filtered[0].is_expanded());
        assert!(filtered[0].children[0].is_expanded());
        assert!(library_items("no matching node").is_empty());
    }

    #[gpui_kit::test]
    fn node_library_drag_edits_only_the_target_slot_and_supports_undo(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (handle, view) = cx.update(open_node_editor_window).unwrap();
        cx.simulate_window_resize(handle, size(px(1520.), px(880.)));
        let parent = Node::invert();
        let slot = parent.slot(0).unwrap();
        let port = cx
            .update_window(handle, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.document = Arc::new(Mutex::new(
                        model::Document::new(vec![Some(parent)]).unwrap(),
                    ));
                    view.rebuild(window, cx);
                    let row = view
                        .navigation
                        .rows
                        .iter()
                        .find(|row| {
                            matches!(&row.target, Target::Slot(current) if current.same_reference(&slot))
                        })
                        .unwrap();
                    format!("graph-drop-{}", row.key.replace('/', "-"))
                })
            })
            .unwrap();
        cx.run_until_parked();

        cx.update_window(handle, |_, window, cx| {
            window.click("graph-library-search", cx);
            window.input("Solid Color", cx);
        })
        .unwrap();
        cx.run_until_parked();

        let leaf = "graph-library-node-Solid Color";
        cx.update_window(handle, |_, window, cx| {
            window.drag_to(leaf, "graph-library-category-generate", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(slot.node().is_none());
        cx.update(|cx| assert_eq!(view.read(cx).branch_count(), 1));

        let input_identity = || {
            slot.node()
                .map(|node| String::from_utf16_lossy(&node.identity_name()))
        };
        cx.update_window(handle, |_, window, cx| {
            window.drag_to(leaf, port.clone(), cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(input_identity().as_deref(), Some("Solid Color"));
        cx.update(|cx| assert_eq!(view.read(cx).branch_count(), 1));

        cx.update_window(handle, |_, window, cx| {
            window.click("graph-library-search", cx);
            window.press("secondary-a", cx);
            window.input("Oklab Color", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.drag_to("graph-library-node-Oklab Color", port, cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(input_identity().as_deref(), Some("Oklab Color"));

        let undo = |cx: &mut TestAppContext| {
            cx.update(|cx| assert!(view.read(cx).history_available(HistoryDirection::Undo)));
            cx.update_window(handle, |_, window, cx| window.click("graph-edit-menu", cx))
                .unwrap();
            cx.run_until_parked();
            cx.update_window(handle, |_, window, cx| {
                let mut menu = window.within("popup-menu");
                assert_eq!(menu.find(0).label(), Some("Undo"));
                menu.click(0, cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update(|cx| assert_eq!(view.read(cx).error, None));
        };
        undo(cx);
        assert_eq!(input_identity().as_deref(), Some("Solid Color"));
        undo(cx);
        assert_eq!(input_identity(), None);
    }

    #[gpui_kit::test]
    fn node_library_keyboard_search_and_scroll_leave_insertion_explicit(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, view) = cx.update(open_node_editor_window).unwrap();
        cx.simulate_window_resize(handle, size(px(880.), px(520.)));
        cx.run_until_parked();
        let initial_branches = cx.update(|cx| view.read(cx).branch_count());

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("graph-library-node-Canvas").is_none());
            window.click("graph-library-browse", cx);
            window.press("right", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("graph-library-node-Canvas").visible());
            window.press("down", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.node_library.selected_identity(cx), Some("Canvas"));
            assert_eq!(view.branch_count(), initial_branches);
        });

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("graph-library-search", cx);
            window.input("GaUsS", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("graph-library-node-Gaussian Blur").visible());
            assert!(window.try_find("graph-library-node-Canvas").is_none());
            window.press("secondary-a", cx);
            window.press("backspace", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let library = &view.read(cx).node_library;
            assert_eq!(library.selected_identity(cx), Some("Canvas"));
            assert!(library.tree.read(cx).entry(0).unwrap().is_expanded());
        });

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                view.read(cx)
                    .node_library
                    .tree
                    .read(cx)
                    .scroll_handle()
                    .is_scrollable()
            );
            let tree = window.find("graph-library-tree").bounds();
            let insert = window.find("graph-library-insert");
            assert!(insert.visible());
            assert!(tree.bottom() <= insert.bounds().top());
            window.scroll(
                "graph-library-tree",
                ScrollDelta::Lines(point(0., -12.)),
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(
                view.node_library
                    .tree
                    .read(cx)
                    .scroll_handle()
                    .0
                    .borrow()
                    .base_handle
                    .offset()
                    .y
                    < px(0.)
            );
            assert_eq!(view.branch_count(), initial_branches);
        });

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("graph-library-insert", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.branch_count(), initial_branches + 1);
            assert_eq!(
                view.selected_node().unwrap().kind(),
                Some(mycela_engine::PointwiseKind::Canvas)
            );
        });
    }
}
