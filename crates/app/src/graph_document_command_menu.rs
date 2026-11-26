use super::recent_paths::RecentPaths;
use super::*;

impl GraphDocumentView {
    pub(super) fn document_menus(
        &self,
        busy: bool,
        results_visible: bool,
        can_undo: bool,
        can_redo: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let owner = cx.weak_entity();
        let edit_owner = owner.clone();
        let selected_node = self
            .selection
            .clone()
            .filter(|_| !busy && !results_visible && !self.overview_open);
        let node_commands = !busy && !results_visible && !self.overview_open;
        let can_copy = node_commands
            && self
                .selected_node()
                .is_some_and(|node| !node.is_default_input());
        let can_paste = node_commands && Self::has_copied_node(cx) && !self.has_field_drafts(cx);
        let workspace = self.workspace.clone();
        let file = Button::new("graph-open")
            .small()
            .ghost()
            .tab_index(0)
            .label("File")
            .dropdown_menu(move |menu, window, cx| {
                let import = owner.clone();
                let gtx = owner.clone();
                let save = owner.clone();
                let save_as = owner.clone();
                menu.when_some(workspace.clone(), |menu, workspace| {
                    menu.item(PopupMenuItem::new("New document").disabled(busy).on_click(
                        move |_, window, cx| {
                            let _ = workspace.update(cx, |view, cx| view.new_document(window, cx));
                        },
                    ))
                })
                .item(
                    PopupMenuItem::new("Open project…")
                        .disabled(busy)
                        .action(Box::new(OpenDocument)),
                )
                .item(
                    PopupMenuItem::new("Import Genetica GTX…")
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            let _ = import.update(cx, |view, cx| {
                                view.request(Intent::Open(DocumentFormat::Gtx), window, cx)
                            });
                        }),
                )
                .when(!busy && workspace.is_some(), |menu| {
                    Self::recent_open_commands(menu, &owner, window, cx)
                })
                .separator()
                .item(
                    PopupMenuItem::new("Save")
                        .disabled(busy)
                        .action(Box::new(SaveDocument))
                        .on_click(move |_, _, cx| {
                            let _ = save.update(cx, |view, cx| view.save(false, cx));
                        }),
                )
                .item(
                    PopupMenuItem::new("Save As…")
                        .disabled(busy)
                        .action(Box::new(SaveDocumentAs))
                        .on_click(move |_, _, cx| {
                            let _ = save_as.update(cx, |view, cx| view.save(true, cx));
                        }),
                )
                .item(
                    PopupMenuItem::new("Save Genetica GTX copy…")
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            let _ = gtx.update(cx, |view, cx| view.save_gtx_copy(window, cx));
                        }),
                )
                .when(workspace.is_some(), |menu| {
                    menu.item(
                        PopupMenuItem::new("Rename document…")
                            .disabled(busy)
                            .action(Box::new(RenameDocument)),
                    )
                })
                .when(!busy && workspace.is_some(), |menu| {
                    Self::recent_save_commands(menu, &owner, window, cx)
                })
                .when_some(workspace.clone(), |menu, workspace| {
                    let duplicate = workspace.clone();
                    let close = workspace.clone();
                    menu.separator()
                        .item(PopupMenuItem::new("Duplicate").disabled(busy).on_click(
                            move |_, window, cx| {
                                let _ = duplicate.update(cx, |view, cx| {
                                    view.duplicate_document(window, cx);
                                });
                            },
                        ))
                        .item(
                            PopupMenuItem::new("Close document")
                                .disabled(busy)
                                .on_click(move |_, window, cx| {
                                    let _ = close.update(cx, |view, cx| {
                                        view.close_document(window, cx);
                                    });
                                }),
                        )
                        .item(PopupMenuItem::new("Close All").disabled(busy).on_click(
                            move |_, window, cx| {
                                let _ = workspace.update(cx, |view, cx| view.close_all(window, cx));
                            },
                        ))
                })
            });
        let edit = Button::new("graph-edit-menu")
            .small()
            .ghost()
            .tab_index(0)
            .label("Edit")
            .dropdown_menu(move |menu, window, cx| {
                menu.item(
                    PopupMenuItem::new("Undo")
                        .disabled(!can_undo)
                        .action(Box::new(UndoGraphEdit)),
                )
                .item(
                    PopupMenuItem::new("Redo")
                        .disabled(!can_redo)
                        .action(Box::new(RedoGraphEdit)),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Copy node and inputs")
                        .disabled(!can_copy)
                        .action(Box::new(CopyNodeTree)),
                )
                .item(
                    PopupMenuItem::new("Paste node")
                        .disabled(!can_paste)
                        .action(Box::new(PasteNodeTree)),
                )
                .item(
                    PopupMenuItem::new("Paste into selection")
                        .disabled(!can_paste)
                        .action(Box::new(PasteNodeIntoSelection)),
                )
                .when_some(selected_node.clone(), |menu, key| {
                    let owner = edit_owner.clone();
                    menu.separator()
                        .submenu("Node actions", window, cx, move |menu, window, cx| {
                            Self::build_node_commands(menu, &owner, &key, window, cx)
                        })
                })
            });
        div()
            .id("graph-command-menu")
            .test_support()
            .role(gpui_kit::Role::MenuBar)
            .h_flex()
            .w_full()
            .flex_shrink_0()
            .gap_1()
            .px_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(file)
            .child(edit)
            .child(self.graph_view_menu(busy || results_visible || self.overview_open, cx))
            .child(self.render_result_menu(busy, results_visible, cx))
    }

    fn recent_open_commands(
        menu: PopupMenu,
        owner: &gpui_kit::WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let files = RecentPaths::files(cx).to_vec();
        let folders = RecentPaths::folders(cx).to_vec();
        if files.is_empty() && folders.is_empty() {
            return menu;
        }
        let owner = owner.clone();
        menu.submenu("Open Recent", window, cx, move |mut menu, window, cx| {
            for file in &files {
                let owner = owner.clone();
                let path = file.path().to_owned();
                let format = file.format();
                let mut label = file.label();
                let duplicate_name = files.iter().any(|other| {
                    other.path() != path && other.path().file_name() == path.file_name()
                });
                if duplicate_name && let Some(folder) = path.parent() {
                    label = format!("{label} · {}", folder.display());
                }
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.open_recent_file(format, path.clone(), cx)
                    });
                }));
            }
            if folders.is_empty() {
                return menu.scrollable(true);
            }
            for (label, format) in [
                ("Open project from folder", DocumentFormat::Mycela),
                ("Import GTX from folder", DocumentFormat::Gtx),
            ] {
                let owner = owner.clone();
                menu = Self::recent_folders(menu, label, &folders, window, cx, move |path, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.choose_open_in_folder(format, path, true, cx);
                    });
                });
            }
            menu.scrollable(true)
        })
    }

    fn recent_save_commands(
        menu: PopupMenu,
        owner: &gpui_kit::WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let folders = RecentPaths::folders(cx).to_vec();
        if folders.is_empty() {
            return menu;
        }
        let owner = owner.clone();
        Self::recent_folders(
            menu,
            "Save project in recent folder",
            &folders,
            window,
            cx,
            move |path, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.save_in_recent_folder(path, cx);
                });
            },
        )
    }

    fn recent_folders(
        menu: PopupMenu,
        label: &'static str,
        folders: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
        select: impl Fn(PathBuf, &mut App) + Clone + 'static,
    ) -> PopupMenu {
        let folders = folders.to_vec();
        menu.submenu(label, window, cx, move |mut menu, _, _| {
            for path in &folders {
                let select = select.clone();
                let label = path.to_string_lossy().into_owned();
                let path = path.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    select(path.clone(), cx);
                }));
            }
            menu.scrollable(true)
        })
    }
}
