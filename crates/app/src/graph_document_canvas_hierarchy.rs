//! The editable Canvas layer and shape tree.
use super::{selection::SelectionItem, *};
use gpui_kit::component::{
    WindowExt,
    checkbox::Checkbox,
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::InputState,
    menu::PopupMenu,
};
use gpui_kit::{App, ElementId, UniformListScrollHandle, WeakEntity, uniform_list};
use mycela_engine::CanvasItemPlacement;

#[derive(Default)]
pub(super) struct Organizer {
    collapsed: HashSet<usize>,
    scroll: UniformListScrollHandle,
}

impl Organizer {
    pub(super) fn refresh(
        &mut self,
        node: &Node,
        selection: &[SelectionItem],
    ) -> Vec<SelectionItem> {
        let rows = tree_rows(node);
        let live: HashSet<_> = rows.iter().map(Row::id).collect();
        self.collapsed.retain(|id| live.contains(id));
        let items: HashMap<_, _> = rows
            .into_iter()
            .filter_map(|row| Some((row.id(), (row.layer, row.item?))))
            .collect();
        selection
            .iter()
            .filter_map(|(_, item)| items.get(&item.identity()).cloned())
            .collect()
    }
}

#[derive(Clone)]
struct Row {
    layer: Layer,
    item: Option<CanvasItem>,
}

impl Row {
    fn id(&self) -> usize {
        self.item
            .as_ref()
            .map_or_else(|| self.layer.identity(), CanvasItem::identity)
    }

    fn label(&self) -> &str {
        self.item
            .as_ref()
            .map_or_else(|| self.layer.label(), CanvasItem::label)
    }

    fn depth(&self) -> usize {
        self.item.as_ref().map_or(0, |item| item.depth() + 1)
    }

    fn container(&self) -> bool {
        self.item.as_ref().is_none_or(CanvasItem::is_group)
    }

    fn child_placement(&self) -> CanvasItemPlacement {
        self.item
            .clone()
            .map_or(CanvasItemPlacement::Layer, CanvasItemPlacement::Into)
    }
}

fn tree_rows(node: &Node) -> Vec<Row> {
    canvas_layers(node)
        .into_iter()
        .flat_map(|layer| {
            std::iter::once(Row {
                layer: layer.clone(),
                item: None,
            })
            .chain(layer.items().into_iter().map(move |item| Row {
                layer: layer.clone(),
                item: Some(item),
            }))
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Command {
    Group,
    Ungroup,
    Move(isize),
    Visibility(bool),
    Rename,
    Delete,
    DrawIntoGroup,
}

#[derive(Clone)]
struct Target {
    owner: WeakEntity<GraphDocumentView>,
    node: Node,
    selection: Option<String>,
    revision: u64,
}

impl Target {
    fn is_current(&self, view: &GraphDocumentView, cx: &Context<GraphDocumentView>) -> bool {
        view.is_original_selection(&self.selection, &Some(self.node.clone()), self.revision)
            && view.drawing_current(cx)
            && !view.has_field_drafts(cx)
    }

    fn run(&self, command: Command, window: &mut Window, cx: &mut App) {
        let _ = self.owner.update(cx, |view, cx| {
            if self.is_current(view, cx) {
                view.edit_canvas_organized_items(self, command, window, cx);
            }
        });
    }

    fn action(&self, label: &str, command: Command, enabled: bool) -> PopupMenuItem {
        let target = self.clone();
        PopupMenuItem::new(label.to_owned())
            .disabled(!enabled)
            .on_click(move |_, window, cx| target.run(command, window, cx))
    }

    fn move_items(
        &self,
        items: &[SelectionItem],
        layer: &Layer,
        placement: &CanvasItemPlacement,
        window: &mut Window,
        cx: &mut App,
    ) {
        let _ = self.owner.update(cx, |view, cx| {
            if !self.is_current(view, cx) {
                return;
            }
            if view.apply_canvas_shape_change(
                false,
                |filter| filter.edit_move_canvas_items(items, layer, placement),
                window,
                cx,
            ) && let Some(tool) = &mut view.drawing
            {
                tool.organizer.collapsed.remove(&layer.identity());
                if let CanvasItemPlacement::Into(group) = placement {
                    tool.organizer.collapsed.remove(&group.identity());
                }
                let selection = tool.organizer.refresh(&self.node, items);
                view.choose_hierarchy_items(selection, window, cx);
            }
        });
    }
}

impl GraphDocumentView {
    fn choose_hierarchy_items(
        &mut self,
        items: Vec<SelectionItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(filter) = self.drawing.as_ref().and_then(|tool| tool.target.filter()) else {
            return;
        };
        let available = filter
            .canvas_shape_editors()
            .into_iter()
            .map(|shape| (shape.identity(), shape))
            .collect();
        self.choose_canvas_selection(
            selection::Selection::from_items(items, &available),
            window,
            cx,
        );
    }

    fn edit_canvas_organized_items(
        &mut self,
        target: &Target,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tool) = self.drawing.as_mut() else {
            return;
        };
        let selection = tool.selection.items.clone();
        let Some((layer, primary)) = selection.last().cloned() else {
            return;
        };
        if matches!(command, Command::Rename) {
            self.rename_canvas_item(target, layer, primary, window, cx);
            return;
        }
        if matches!(command, Command::DrawIntoGroup) {
            if let Some(destination) = group_destinations(&layer).find(|dest| {
                matches!(dest, Destination::Group(_, item, _) if item.identity() == primary.identity())
            }) {
                tool.destination = destination;
                cx.notify();
            }
            return;
        }

        let mut grouped = None;
        let applied = self.apply_canvas_shape_edit(
            false,
            |filter| match command {
                Command::Group => {
                    let items: Vec<_> = selection.iter().map(|(_, item)| item.clone()).collect();
                    grouped = Some(layer.edit_group_items(&items, &primary)?);
                    Ok(())
                }
                Command::Ungroup => layer.edit_ungroup_item(&primary),
                Command::Move(offset) => layer.edit_move_item(&primary, offset),
                Command::Visibility(visible) => layer.edit_item_visibility(&primary, visible),
                Command::Delete => filter.edit_remove_canvas_items(&selection),
                Command::Rename | Command::DrawIntoGroup => unreachable!(),
            },
            window,
            cx,
        );
        if applied
            && let Some(group) = grouped
            && let Some(tool) = &mut self.drawing
        {
            tool.organizer.collapsed.insert(group.identity());
            self.choose_hierarchy_items(vec![(layer, group)], window, cx);
        }
    }

    fn rename_canvas_item(
        &mut self,
        target: &Target,
        layer: Layer,
        item: CanvasItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = cx.new(|cx| InputState::new(window, cx).default_value(item.name()));
        let owner = cx.weak_entity();
        let target = target.clone();
        self.pause_preview();
        self.modal = true;
        window.open_dialog(cx, move |dialog, _, _| {
            let state = state.clone();
            let input = state.clone();
            let owner = owner.clone();
            let close = owner.clone();
            let item = item.clone();
            let layer = layer.clone();
            let target = target.clone();
            dialog
                .title("Rename Canvas item")
                .child(
                    Input::new(&state)
                        .id("texture-canvas-item-name")
                        .aria_label("Name"),
                )
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Apply")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let name = input.read(cx).value().to_string();
                    let _ = owner.update(cx, |view, cx| {
                        view.modal = false;
                        if target.is_current(view, cx) {
                            view.apply_canvas_shape_change(
                                false,
                                |_| layer.edit_item_name(&item, &name),
                                window,
                                cx,
                            );
                        }
                    });
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

    pub(in crate::graph_document::view) fn render_canvas_hierarchy(
        &self,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let tool = self.drawing.as_ref()?;
        let organizer = &tool.organizer;
        let disabled = busy || self.has_field_drafts(cx);
        let panel = div()
            .id("texture-canvas-hierarchy")
            .test_support()
            .flex_1()
            .size_full()
            .min_h_0()
            .p_3()
            .v_flex()
            .gap_2()
            .child(div().font_semibold().child("Shapes"));
        let node = tool.target.clone();
        let rows = tree_rows(&node);
        if rows.is_empty() {
            return Some(
                panel
                    .child(div().text_sm().child("Draw a shape or add a layer."))
                    .into_any_element(),
            );
        }
        let selection = tool.selection.items.clone();
        let primary = selection.last();
        let single = primary
            .filter(|_| selection.len() == 1)
            .map(|(_, item)| item.clone());
        let can_group = primary.is_some_and(|(layer, item)| {
            selection
                .iter()
                .all(|(source, _)| source.same_reference(layer))
                && layer.can_group_items(
                    &selection
                        .iter()
                        .map(|(_, item)| item.clone())
                        .collect::<Vec<_>>(),
                    item,
                )
        });
        let target = Target {
            owner: cx.weak_entity(),
            node,
            selection: self.selection.clone(),
            revision: self.revision,
        };
        let collapsed = Rc::new(organizer.collapsed.clone());
        let mut hidden_below = None;
        let visible: Vec<_> = rows
            .iter()
            .filter(|row| {
                let depth = row.depth();
                if hidden_below.is_some_and(|parent| depth > parent) {
                    return false;
                }
                hidden_below = collapsed.contains(&row.id()).then_some(depth);
                true
            })
            .cloned()
            .collect();
        let row_target = target.clone();
        let selected = Rc::new(selection);
        let selected_rows = selected.clone();
        let rows = uniform_list(
            "texture-canvas-item-list",
            visible.len(),
            move |range, _, cx| {
                visible[range]
                    .iter()
                    .map(|row| {
                        render_row(
                            row,
                            &row_target,
                            &selected_rows,
                            collapsed.contains(&row.id()),
                            disabled,
                            cx,
                        )
                    })
                    .collect()
            },
        )
        .track_scroll(&organizer.scroll)
        .flex_1()
        .min_h_0()
        .w_full()
        .test_support();
        let group_target = target.clone();
        let ungroup_target = target.clone();
        let delete_target = target.clone();
        let move_target = target.clone();
        Some(
            panel
                .child(rows)
                .child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("texture-canvas-group")
                                .small()
                                .label("Group")
                                .disabled(disabled || !can_group)
                                .on_click(move |_, window, cx| {
                                    group_target.run(Command::Group, window, cx)
                                }),
                        )
                        .child(
                            Button::new("texture-canvas-ungroup")
                                .small()
                                .label("Ungroup")
                                .disabled(
                                    disabled || !single.as_ref().is_some_and(CanvasItem::is_group),
                                )
                                .on_click(move |_, window, cx| {
                                    ungroup_target.run(Command::Ungroup, window, cx)
                                }),
                        )
                        .child(
                            Button::new("texture-canvas-item-actions")
                                .small()
                                .label("Item actions")
                                .disabled(disabled || single.is_none())
                                .dropdown_menu(move |menu, _, _| match &single {
                                    Some(item) => item_actions(menu, &target, item),
                                    None => menu,
                                }),
                        )
                        .child(
                            Button::new("texture-canvas-move-items")
                                .small()
                                .label("Move to")
                                .disabled(disabled || selected.is_empty())
                                .dropdown_menu({
                                    let selected = selected.clone();
                                    move |menu, _, _| move_menu(menu, &move_target, &selected)
                                }),
                        )
                        .child(
                            Button::new("texture-canvas-delete-items")
                                .small()
                                .ghost()
                                .label("Delete selected")
                                .disabled(disabled || selected.is_empty())
                                .on_click(move |_, window, cx| {
                                    delete_target.run(Command::Delete, window, cx)
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn item_actions(menu: PopupMenu, target: &Target, item: &CanvasItem) -> PopupMenu {
    let menu = menu
        .item(target.action("Rename…", Command::Rename, true))
        .item(target.action("Move up", Command::Move(-1), item.can_move(-1)))
        .item(target.action("Move down", Command::Move(1), item.can_move(1)))
        .item(target.action(
            if item.is_visible() { "Hide" } else { "Show" },
            Command::Visibility(!item.is_visible()),
            true,
        ));
    if item.is_group() {
        menu.item(target.action("Set as drawing destination", Command::DrawIntoGroup, true))
    } else {
        menu
    }
}

fn move_menu(mut menu: PopupMenu, target: &Target, items: &[SelectionItem]) -> PopupMenu {
    let Some(filter) = target.node.filter() else {
        return menu;
    };
    for layer in canvas_layers(&target.node) {
        let groups = group_destinations(&layer).filter_map(|choice| match choice {
            Destination::Group(_, item, label) => Some((label, CanvasItemPlacement::Into(item))),
            _ => None,
        });
        for (label, placement) in
            std::iter::once((layer.label().to_owned(), CanvasItemPlacement::Layer)).chain(groups)
        {
            let enabled = filter.can_move_canvas_items(items, &layer, &placement);
            let items = items.to_vec();
            let target = target.clone();
            let layer = layer.clone();
            menu = menu.item(PopupMenuItem::new(label).disabled(!enabled).on_click(
                move |_, window, cx| {
                    target.move_items(&items, &layer, &placement, window, cx);
                },
            ));
        }
    }
    menu.scrollable(true)
}

#[derive(Clone)]
struct ItemDrag {
    target: Target,
    items: Vec<SelectionItem>,
}

struct DragLabel(String);
impl Render for DragLabel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_2()
            .rounded_md()
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .child(self.0.clone())
    }
}

fn render_row(
    row: &Row,
    target: &Target,
    selection: &[SelectionItem],
    collapsed: bool,
    disabled: bool,
    cx: &App,
) -> AnyElement {
    let id = row.id();
    let mut element = div()
        .relative()
        .h_flex()
        .items_center()
        .h_7()
        .w_full()
        .pl(rems(0.625 * row.depth().min(8) as f32));
    if row.container() {
        let caption = if collapsed { "Expand" } else { "Collapse" };
        let toggle = target.clone();
        element = element.child(
            Button::new(ElementId::named_usize("texture-canvas-expand-item", id))
                .small()
                .ghost()
                .label(if collapsed { "▸" } else { "▾" })
                .tooltip(format!("{caption} {}", row.label()))
                .disabled(disabled)
                .on_click(move |_, _, cx| {
                    let _ = toggle.owner.update(cx, |view, cx| {
                        if toggle.is_current(view, cx)
                            && let Some(tool) = &mut view.drawing
                        {
                            if !tool.organizer.collapsed.remove(&id) {
                                tool.organizer.collapsed.insert(id);
                            }
                            cx.notify();
                        }
                    });
                }),
        );
    }
    if let Some(item) = row.item.clone() {
        let current = selection
            .iter()
            .any(|(_, selected)| selected.identity() == id);
        let selection_target = target.clone();
        let entry = (row.layer.clone(), item.clone());
        let mut label = row.label().to_owned();
        if !item.is_visible() {
            label.push_str(" · Hidden");
        }
        let items = if current {
            selection.to_vec()
        } else {
            vec![entry.clone()]
        };
        let preview = if items.len() == 1 {
            row.label().to_owned()
        } else {
            format!("{} items", items.len())
        };
        element = element.child(
            Checkbox::new(ElementId::named_usize("texture-canvas-item", id))
                .label(label)
                .checked(current)
                .disabled(disabled)
                .on_click(move |checked, window, cx| {
                    let _ = selection_target.owner.update(cx, |view, cx| {
                        if selection_target.is_current(view, cx)
                            && let Some(tool) = &view.drawing
                        {
                            let mut items = tool.selection.items.clone();
                            items.retain(|(_, selected)| selected.identity() != id);
                            if *checked {
                                items.push(entry.clone());
                            }
                            view.choose_hierarchy_items(items, window, cx);
                        }
                    });
                }),
        );
        if !disabled {
            element = element.child(
                div()
                    .id(ElementId::named_usize("texture-canvas-drag-item", id))
                    .ml_auto()
                    .px_2()
                    .cursor_grab()
                    .text_color(cx.theme().muted_foreground)
                    .aria_label(format!("Drag {}", row.label()))
                    .child("⠿")
                    .on_drag(
                        ItemDrag {
                            target: target.clone(),
                            items,
                        },
                        move |_, _, _, cx| cx.new(|_| DragLabel(preview.clone())),
                    )
                    .test_support(),
            );
            let edge = rems(if row.container() { 0.25 } else { 0.875 });
            let before = div()
                .id(ElementId::named_usize("texture-canvas-drop-before", id))
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h(edge);
            let after = div()
                .id(ElementId::named_usize("texture-canvas-drop-after", id))
                .absolute()
                .bottom_0()
                .left_0()
                .w_full()
                .h(edge);
            element = element
                .child(
                    drop_zone(
                        before,
                        target,
                        &row.layer,
                        CanvasItemPlacement::Before(item.clone()),
                        cx,
                    )
                    .test_support(),
                )
                .child(
                    drop_zone(
                        after,
                        target,
                        &row.layer,
                        CanvasItemPlacement::After(item),
                        cx,
                    )
                    .test_support(),
                );
        }
    } else {
        element = element.child(
            div()
                .text_sm()
                .font_semibold()
                .child(row.label().to_owned()),
        );
    }
    if !disabled && row.container() {
        let edge = rems(if row.item.is_some() { 0.25 } else { 0. });
        let middle = div()
            .id(ElementId::named_usize("texture-canvas-drop-into", id))
            .absolute()
            .top(edge)
            .bottom(edge)
            .left_0()
            .w_full();
        element = element.child(drop_zone(
            middle,
            target,
            &row.layer,
            row.child_placement(),
            cx,
        ));
    }
    element
        .id(ElementId::named_usize("texture-canvas-organizer-row", id))
        .test_support()
        .into_any_element()
}

fn drop_zone(
    element: gpui_kit::Stateful<gpui_kit::Div>,
    target: &Target,
    layer: &Layer,
    placement: CanvasItemPlacement,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let filter = target.node.filter();
    let accepted_layer = layer.clone();
    let accepted_placement = placement.clone();
    let layer = layer.clone();
    let highlight = cx.theme().primary.opacity(0.18);
    let allowed = move |drag: &ItemDrag| {
        filter.as_ref().is_some_and(|filter| {
            filter.can_move_canvas_items(&drag.items, &accepted_layer, &accepted_placement)
        })
    };
    let hover = allowed.clone();
    element
        .drag_over::<ItemDrag>(move |style, drag, _, _| {
            if hover(drag) {
                style.bg(highlight)
            } else {
                style
            }
        })
        .can_drop(move |value, _, _| value.downcast_ref::<ItemDrag>().is_some_and(&allowed))
        .on_drop(move |drag: &ItemDrag, window, cx| {
            drag.target
                .move_items(&drag.items, &layer, &placement, window, cx);
        })
}
