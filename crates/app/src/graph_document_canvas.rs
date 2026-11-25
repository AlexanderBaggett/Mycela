//! Spatial presentation and commands over the existing Navigation/held qi owners.
//! Positions are window-local rem coordinates, never a second editable graph.
use super::super::navigation::editable_contents;
use super::node_library::LibraryNodeDrag;
use super::*;
use gpui_kit::base::ElementExt;
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::{
    ElementId, MouseButton, PathBuilder, Pixels, Point, ScrollHandle, Subscription, canvas, point,
    px,
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
};

#[path = "graph_document_canvas_viewport.rs"]
mod viewport;
#[cfg(test)]
#[path = "graph_document_canvas_zoom_tests.rs"]
mod zoom_tests;
use viewport::{GraphBounds, PanState, Viewport, Zoom};

const CARD_WIDTH: f32 = 11.;
// Saved branch offsets use the original 18-rem card scale.
const BRANCH_UNITS_PER_REM: f32 = 110. / 18.;
const CANVAS_MARGIN: f32 = 2.;
const MAX_BRANCH_OFFSET: f32 = 8_192.;
const ZOOM_STEP: f32 = 0.2;
const CARD_TITLE_HEIGHT: f32 = 1.75;
const CARD_TITLE_CENTER_Y: f32 = CARD_TITLE_HEIGHT / 2.;
const CARD_GAP: f32 = 3.;
const INPUT_GAP: f32 = 1.;
const INPUT_WIRE_SPACE: f32 = 2.5;
const INPUT_ROW_HEIGHT: f32 = CARD_WIDTH + INPUT_WIRE_SPACE;
const OUTPUT_TERMINAL_SPACE: f32 = 4.5;

gpui_kit::actions!(
    graph_canvas,
    [
        ZoomIn, ZoomOut, ResetZoom, FitGraph, ClosePan, PanLeft, PanRight, PanUp, PanDown
    ]
);

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        gpui_kit::KeyBinding::new("secondary-=", ZoomIn, Some("GraphCanvas")),
        gpui_kit::KeyBinding::new("secondary--", ZoomOut, Some("GraphCanvas")),
        gpui_kit::KeyBinding::new("secondary-0", ResetZoom, Some("GraphCanvas")),
        gpui_kit::KeyBinding::new("secondary-shift-f", FitGraph, Some("GraphCanvas")),
        gpui_kit::KeyBinding::new("escape", ClosePan, Some("GraphCanvas")),
        gpui_kit::KeyBinding::new("left", PanLeft, Some("GraphCanvas")),
        gpui_kit::KeyBinding::new("right", PanRight, Some("GraphCanvas")),
        gpui_kit::KeyBinding::new("up", PanUp, Some("GraphCanvas")),
        gpui_kit::KeyBinding::new("down", PanDown, Some("GraphCanvas")),
    ]);
}
#[derive(Default)]
struct FrameGeometry {
    outputs: Vec<(u64, gpui_kit::Point<Pixels>)>,
    inputs: HashMap<String, (u64, Point<Pixels>, Point<Pixels>)>,
    receivers: HashMap<u64, Point<Pixels>>,
}
fn draw_wire(
    window: &mut Window,
    a: Point<Pixels>,
    b: Point<Pixels>,
    rem: Pixels,
    color: gpui_kit::Hsla,
) {
    let distance = ((a.y - b.y).abs() / 2.).max(rem * 0.5);
    let mut path = PathBuilder::stroke((rem / 14.).max(px(1.)));
    path.move_to(a);
    path.cubic_bezier_to(b, point(a.x, a.y + distance), point(b.x, b.y - distance));
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}
#[derive(Clone)]
pub(super) struct Card {
    id: u64,
    node: Node,
    key: String,
    position: [f32; 2],
    manual_offset: [f32; 2],
    offset: [f32; 2],
    inputs: InputLayout,
    receiver: Option<String>,
    texture_output: bool,
    collapsed: bool,
    hidden: bool,
}
impl Card {
    fn canvas_position(&self) -> [f32; 2] {
        [
            self.position[0] + self.manual_offset[0] + self.offset[0],
            self.position[1] + self.manual_offset[1] + self.offset[1],
        ]
    }

    fn height(&self) -> f32 {
        CARD_WIDTH
            + self.input_height()
            + if self.texture_output {
                OUTPUT_TERMINAL_SPACE
            } else {
                0.
            }
    }

    fn input_height(&self) -> f32 {
        if self.collapsed {
            0.
        } else {
            self.inputs.height()
        }
    }

    fn body_position(&self) -> [f32; 2] {
        let [x, y] = self.canvas_position();
        [x, y + self.input_height()]
    }

    fn extent(&self) -> ([f32; 2], f32) {
        let [x, y] = self.canvas_position();
        let width = if self.collapsed {
            CARD_WIDTH
        } else {
            self.inputs.width.max(CARD_WIDTH)
        };
        ([x - (width - CARD_WIDTH) / 2., y], width)
    }
}
struct CanvasLayout {
    group: Option<Node>,
    cards: Vec<Card>,
    offset: gpui_kit::Point<Pixels>,
    zoom: Zoom,
    zoom_padding: Point<Pixels>,
}
#[derive(Default)]
pub(super) struct CanvasState {
    pub visible: bool,
    cards: Vec<Card>,
    next: u64,
    scroll: ScrollHandle,
    viewport_size: gpui_kit::Size<Pixels>,
    zoom: Zoom,
    zoom_padding: Point<Pixels>,
    center_on_measure: bool,
    focus: Option<FocusHandle>,
    zoom_slider: Option<Entity<SliderState>>,
    _zoom_slider_subscription: Option<Subscription>,
    pan: PanState,
    groups: Vec<Node>,
    inactive_layouts: Vec<CanvasLayout>,
    branch_origin: [f32; 2],
    collapsed: HashSet<u64>,
    card_grab: Rc<Cell<Option<CardGrab>>>,
}
impl CanvasState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<GraphDocumentView>) -> Self {
        let slider = cx.new(|_| {
            SliderState::new()
                .min(Zoom::MIN.percent())
                .max(Zoom::MAX.percent())
                .step(1.)
                .default_value(Zoom::NORMAL.percent())
        });
        let subscription = cx.subscribe_in(&slider, window, |view, _, event, window, cx| {
            if let SliderEvent::Change(value) = event {
                view.change_canvas_zoom(Zoom::new(value.start() / 100.), None, window, cx);
            }
        });
        Self {
            focus: Some(cx.focus_handle().tab_stop(true)),
            zoom_slider: Some(slider),
            _zoom_slider_subscription: Some(subscription),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(super) fn card_id_for_test(&self, node: &Node) -> Option<u64> {
        self.cards
            .iter()
            .find(|card| card.node.same_reference(node))
            .map(|card| card.id)
    }

    #[cfg(test)]
    pub(super) fn card_position_for_test(&self, node: &Node) -> Option<[f32; 2]> {
        self.cards
            .iter()
            .find(|card| card.node.same_reference(node))
            .map(Card::canvas_position)
    }

    pub fn rebuild(&mut self, navigation: &Navigation) {
        let reachable: Vec<_> = navigation
            .rows
            .iter()
            .filter_map(|row| row.target.node())
            .collect();
        let is_reachable = |node: &Node| reachable.iter().any(|live| live.same_reference(node));
        if !self.collapsed.is_empty() {
            let live: HashSet<_> = reachable.iter().map(Node::occurrence_id).collect();
            self.collapsed.retain(|id| live.contains(id));
        }
        let group = self.groups.iter().rev().find(|group| is_reachable(group));
        let groups = group.map_or_else(Vec::new, |group| group_path(navigation, group));
        self.set_level(groups, navigation);
        // Keep saved placements only for nodes the document still owns.
        self.inactive_layouts.retain_mut(|layout| {
            layout.cards.retain(|card| is_reachable(&card.node));
            layout.group.as_ref().is_none_or(is_reachable)
        });
    }

    fn set_level(&mut self, groups: Vec<Node>, navigation: &Navigation) {
        if !same_level(self.groups.last(), groups.last()) {
            self.pan.stop_pointer_gestures();
            let previous = CanvasLayout {
                group: self.groups.last().cloned(),
                cards: std::mem::take(&mut self.cards),
                offset: self.scroll.offset(),
                zoom: self.zoom,
                zoom_padding: self.zoom_padding,
            };
            self.zoom = Zoom::NORMAL;
            self.zoom_padding = Point::default();
            let offset = self
                .inactive_layouts
                .iter()
                .position(|layout| same_level(layout.group.as_ref(), groups.last()))
                .map_or(point(Pixels::ZERO, Pixels::ZERO), |index| {
                    let layout = self.inactive_layouts.swap_remove(index);
                    self.cards = layout.cards;
                    self.zoom = layout.zoom;
                    self.zoom_padding = layout.zoom_padding;
                    layout.offset
                });
            self.inactive_layouts.push(previous);
            self.scroll.set_offset(offset);
        }
        self.groups = groups;
        self.rebuild_layout(navigation, false);
    }

    fn group(&self) -> Option<&Node> {
        self.groups.last()
    }

    pub(super) fn first_content_key(&self, navigation: &Navigation) -> Option<String> {
        let rows = contents_rows(navigation, self.group()?);
        rows.iter()
            .find(|row| row.target.node().is_some())
            .or_else(|| rows.first())
            .map(|row| row.key.clone())
    }

    pub(super) fn contains(&self, target: &Target) -> bool {
        self.group().is_none() || same_level(self.group(), group_scope(target).as_ref())
    }

    pub(super) fn clear_level(&mut self) {
        self.groups.clear();
        self.cards.clear();
        self.collapsed.clear();
        self.inactive_layouts.clear();
        self.zoom = Zoom::NORMAL;
        self.zoom_padding = Point::default();
        self.center_on_measure = false;
        self.pan = PanState::default();
        self.scroll.set_offset(point(Pixels::ZERO, Pixels::ZERO));
    }

    pub(super) fn follow_selection(&mut self, navigation: &Navigation, key: &str, target: &Target) {
        let scope = group_scope(target);
        if (self.visible || self.group().is_some()) && !same_level(self.group(), scope.as_ref()) {
            let groups = scope
                .as_ref()
                .map_or_else(Vec::new, |group| group_path(navigation, group));
            self.set_level(groups, navigation);
        }
        if self.visible && !self.collapsed.is_empty() {
            let mut changed = false;
            for parent in parent_keys(key).filter_map(|parent| navigation.target(parent)) {
                if same_level(scope.as_ref(), group_scope(&parent).as_ref())
                    && let Some(node) = parent.node()
                {
                    changed |= self.collapsed.remove(&node.occurrence_id());
                }
            }
            if changed {
                self.rebuild_layout(navigation, false);
            }
        }
    }

    fn rebuild_layout(&mut self, navigation: &Navigation, arrange: bool) {
        let group = self.group().cloned();
        let depth_offset = group
            .as_ref()
            .and_then(|group| group_key(navigation, group))
            .map_or(0, |key| key.matches("/slot-").count() + 1);
        let collapsed_keys: HashSet<_> = navigation
            .rows
            .iter()
            .filter(|row| {
                !self.collapsed.is_empty()
                    && row
                        .target
                        .node()
                        .is_some_and(|node| self.collapsed.contains(&node.occurrence_id()))
                    && same_level(group.as_ref(), group_scope(&row.target).as_ref())
            })
            .map(|row| row.key.as_str())
            .collect();
        let mut current: Vec<(Node, String, usize, bool)> = Vec::new();
        for row in &navigation.rows {
            if !same_level(group.as_ref(), group_scope(&row.target).as_ref()) {
                continue;
            }
            let Some(node) = row.target.node() else {
                continue;
            };
            let depth = row
                .key
                .matches("/slot-")
                .count()
                .saturating_sub(depth_offset);
            let hidden = !collapsed_keys.is_empty()
                && parent_keys(&row.key).any(|key| collapsed_keys.contains(key));
            if let Some(item) = current
                .iter_mut()
                .find(|(old, _, _, _)| old.same_reference(&node))
            {
                if (hidden, depth) < (item.3, item.2) {
                    item.1 = row.key.clone();
                    item.2 = depth;
                    item.3 = hidden;
                }
            } else {
                current.push((node, row.key.clone(), depth, hidden));
            }
        }
        let locations: HashMap<_, _> = current
            .iter()
            .enumerate()
            .map(|(index, (_, key, _, _))| (key.as_str(), index))
            .collect();
        let mut rows_by_parent = HashMap::<_, Vec<_>>::new();
        for row in &navigation.rows {
            if let Some((parent, _)) = row.key.rsplit_once('/') {
                rows_by_parent.entry(parent).or_default().push(row);
            }
        }
        let mut receiver = vec![None; current.len()];
        let mut inputs: Vec<_> = current
            .iter()
            .map(|(node, key, _, _)| {
                let rows = rows_by_parent.get(key.as_str());
                let sockets = canvas_ports(node)
                    .into_iter()
                    .filter_map(|slot| {
                        let row = rows?.iter().find(|row| {
                            matches!(&row.target, Target::Slot(target) if target.same_reference(&slot))
                        })?;
                        let child = locations.get(row.key.as_str()).copied();
                        if let Some(child) = child {
                            receiver[child] = Some(key.clone());
                        }
                        Some(InputSocket { key: row.key.clone(), child, left: 0. })
                    })
                    .collect();
                InputLayout { width: 0., sockets }
            })
            .collect();
        let collapsed: Vec<_> = current
            .iter()
            .map(|(node, _, _, _)| self.collapsed.contains(&node.occurrence_id()))
            .collect();
        let mut positions = vec![[0.; 2]; current.len()];
        let mut next_x = CANVAS_MARGIN;
        for (root, parent) in receiver.iter().enumerate() {
            if parent.is_some() {
                continue;
            }
            let span = measure_columns(root, &mut inputs);
            let body = [next_x + (span - CARD_WIDTH) / 2., 0.];
            place_cards(root, body, &inputs, &collapsed, &mut positions);
            next_x += span + CARD_GAP;
        }
        let output_key = group
            .is_none()
            .then(|| {
                navigation.rows.iter().find_map(|row| {
                    matches!(row.target, Target::Branch(0, _)).then_some(row.key.as_str())
                })
            })
            .flatten();
        let mut cards = Vec::new();
        for (index, ((node, key, _, hidden), inputs)) in current.into_iter().zip(inputs).enumerate()
        {
            let (id, manual_offset) =
                if let Some(old) = self.cards.iter().find(|old| old.node.same_reference(&node)) {
                    let offset = if arrange || receiver[index].is_some() {
                        [0.; 2]
                    } else {
                        old.manual_offset
                    };
                    (old.id, offset)
                } else {
                    self.next += 1;
                    (self.next, [0.; 2])
                };
            cards.push(Card {
                id,
                collapsed: collapsed[index],
                texture_output: output_key == Some(key.as_str()),
                node,
                key,
                position: positions[index],
                manual_offset,
                offset: [0.; 2],
                inputs,
                receiver: receiver[index].take(),
                hidden,
            });
        }
        let branches: Vec<_> = if let Some(group) = &group {
            contents_rows(navigation, group)
        } else {
            navigation
                .rows
                .iter()
                .filter(|row| matches!(row.target, Target::Branch(..)))
                .collect()
        };
        let positions = group.as_ref().and_then(Node::filter).map_or_else(
            || navigation.branch_positions.clone(),
            |group| group.group_content_positions(),
        );
        self.branch_origin = place_branches(&mut cards, &branches, &positions, [CANVAS_MARGIN, 0.]);
        // Only currently reachable nodes sponsor a graph. Retired menus/jobs
        // keep their own explicit held owners until their normal retirement.
        self.cards = cards;
    }

    fn visible_cards(&self) -> impl Iterator<Item = &Card> {
        self.cards.iter().filter(|card| !card.hidden)
    }

    pub(super) fn collapsed_rows(&self, navigation: &Navigation) -> Vec<usize> {
        if self.collapsed.is_empty() {
            return Vec::new();
        }
        navigation
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let id = row.target.node()?.occurrence_id();
                self.collapsed.contains(&id).then_some(index)
            })
            .collect()
    }

    pub(super) fn restore_collapsed_rows(&mut self, navigation: &Navigation, rows: &[usize]) {
        self.collapsed = rows
            .iter()
            .filter_map(|&index| {
                navigation
                    .rows
                    .get(index)?
                    .target
                    .node()
                    .map(|node| node.occurrence_id())
            })
            .collect();
        if !self.collapsed.is_empty() {
            self.rebuild_layout(navigation, false);
        }
    }
    fn arrange(&mut self, navigation: &Navigation) {
        self.rebuild_layout(navigation, true);
        self.zoom_padding = Point::default();
        self.scroll.set_offset(point(Pixels::ZERO, Pixels::ZERO));
    }

    fn viewport(&self, rem: Pixels) -> Viewport {
        Viewport::new(
            &GraphBounds::for_cards(&self.cards),
            self.viewport_size,
            rem,
            self.zoom,
        )
        .with_padding(self.zoom_padding)
    }

    fn socket_position(&self, key: &str) -> Option<[f32; 2]> {
        self.visible_cards()
            .filter(|card| !card.collapsed)
            .find_map(|card| {
                let socket = card
                    .inputs
                    .sockets
                    .iter()
                    .find(|socket| socket.key == key)?;
                let [x, y] = card.canvas_position();
                Some([x + socket.left, y])
            })
    }

    fn has_viewport(&self) -> bool {
        self.viewport_size.width > Pixels::ZERO && self.viewport_size.height > Pixels::ZERO
    }

    fn set_zoom(&mut self, zoom: Zoom, cursor: Option<Point<Pixels>>, rem: Pixels) -> bool {
        if self.zoom == zoom || !self.has_viewport() {
            return false;
        }
        let bounds = GraphBounds::for_cards(&self.cards);
        let old = Viewport::new(&bounds, self.viewport_size, rem, self.zoom)
            .with_padding(self.zoom_padding);
        let cursor = cursor.unwrap_or_else(|| old.center());
        let world = old.world_point(cursor, self.scroll.offset());
        self.zoom = zoom;
        self.anchor_at(
            world,
            cursor,
            Viewport::new(&bounds, self.viewport_size, rem, zoom),
        );
        true
    }

    fn anchor_at(&mut self, world: [f32; 2], cursor: Point<Pixels>, viewport: Viewport) {
        // A fitting axis normally cannot scroll. Add only the room needed
        // to preserve the anchor with the same native scroll offset.
        self.zoom_padding = viewport.padding_for(world, cursor);
        self.scroll.set_offset(
            viewport
                .with_padding(self.zoom_padding)
                .place_at(world, cursor),
        );
    }

    fn branch_position(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        let [origin_x, origin_y] = self.branch_origin;
        [x - origin_x, y - origin_y]
            .map(|axis| axis.clamp(-MAX_BRANCH_OFFSET, MAX_BRANCH_OFFSET) * BRANCH_UNITS_PER_REM)
    }

    fn fit(&mut self, rem: Pixels) {
        let bounds = GraphBounds::for_cards(&self.cards);
        self.zoom = bounds.fit(self.viewport_size, rem);
        self.zoom_padding = Point::default();
        let viewport = Viewport::new(&bounds, self.viewport_size, rem, self.zoom);
        self.scroll.set_offset(viewport.fit_offset());
    }
}

fn measure_columns(index: usize, inputs: &mut [InputLayout]) -> f32 {
    let mut next = 0.;
    let count = inputs[index].sockets.len();
    for port in 0..count {
        let width = inputs[index].sockets[port]
            .child
            .map_or(CARD_WIDTH, |child| measure_columns(child, inputs));
        inputs[index].sockets[port].left = next + (width - CARD_WIDTH) / 2.;
        next += width + INPUT_GAP;
    }
    let width = if count > 0 { next - INPUT_GAP } else { 0. };
    let inset = (CARD_WIDTH - width) / 2.;
    for socket in &mut inputs[index].sockets {
        socket.left += inset;
    }
    inputs[index].width = width;
    width.max(CARD_WIDTH)
}

fn place_cards(
    index: usize,
    body: [f32; 2],
    inputs: &[InputLayout],
    collapsed: &[bool],
    positions: &mut [[f32; 2]],
) {
    let [x, y] = body;
    let height = inputs[index].height();
    positions[index] = [x, if collapsed[index] { y } else { y - height }];
    for socket in &inputs[index].sockets {
        if let Some(child) = socket.child {
            place_cards(
                child,
                [x + socket.left, y - height],
                inputs,
                collapsed,
                positions,
            );
        }
    }
}

fn place_branches(
    cards: &mut [Card],
    branches: &[&super::super::navigation::Row],
    positions: &[[f32; 2]],
    default_origin: [f32; 2],
) -> [f32; 2] {
    let origin = branches
        .first()
        .and_then(|row| cards.iter().find(|card| card.key == row.key))
        .map_or(default_origin, |card| card.position);
    for (branch, position) in branches.iter().skip(1).zip(positions.iter().copied()) {
        // Newly created extra branches use zero until the user places them.
        if position == [0.; 2] || !position.into_iter().all(f32::is_finite) {
            continue;
        }
        let Some(root) = cards.iter().find(|card| card.key == branch.key) else {
            continue;
        };
        let [x, y] = root.position;
        let [dx, dy] = position
            .map(|axis| (axis / BRANCH_UNITS_PER_REM).clamp(-MAX_BRANCH_OFFSET, MAX_BRANCH_OFFSET));
        let offset = [origin[0] + dx - x, origin[1] + dy - y];
        for card in cards.iter_mut().filter(|card| {
            card.key == branch.key
                || card
                    .key
                    .strip_prefix(&branch.key)
                    .is_some_and(|tail| tail.starts_with('/'))
        }) {
            card.offset = offset;
        }
    }
    let moved_roots: Vec<_> = cards
        .iter()
        .filter(|card| card.manual_offset != [0.; 2])
        .map(|card| (card.key.clone(), card.manual_offset))
        .collect();
    for card in &mut *cards {
        if let Some((_, offset)) = moved_roots.iter().find(|(key, _)| {
            card.key
                .strip_prefix(key)
                .is_some_and(|tail| tail.starts_with('/'))
        }) {
            card.offset[0] += offset[0];
            card.offset[1] += offset[1];
        }
    }
    let mut shift = [0_f32; 2];
    for card in &*cards {
        let ([x, y], _) = card.extent();
        shift[0] = shift[0].max(CANVAS_MARGIN - x);
        shift[1] = shift[1].max(CANVAS_MARGIN - y);
    }
    for card in cards {
        card.offset[0] += shift[0];
        card.offset[1] += shift[1];
    }
    [origin[0] + shift[0], origin[1] + shift[1]]
}
#[derive(Clone)]
struct WireDrag {
    revision: u64,
    selection_serial: u64,
    node: Node,
    share: bool,
}
#[derive(Clone)]
struct CardDrag {
    key: String,
    wire: WireDrag,
    grab: Rc<Cell<Option<CardGrab>>>,
}

#[derive(Clone, Copy)]
struct CardGrab {
    card: u64,
    offset: Point<Pixels>,
}

impl CardDrag {
    fn grab_offset(&self) -> Option<Point<Pixels>> {
        self.grab
            .get()
            .filter(|grab| grab.card == self.wire.node.occurrence_id())
            .map(|grab| grab.offset)
    }
}

fn disconnected_branch(target: &Target, group: Option<&Node>) -> bool {
    match (target, group) {
        (Target::Branch(index, _), None) => *index > 0,
        (Target::Slot(slot), Some(group)) => group
            .contents()
            .iter()
            .skip(1)
            .any(|content| content.same_reference(slot)),
        _ => false,
    }
}
#[derive(Clone)]
struct GroupInput {
    group: Node,
    slot: mycela_engine::PointwiseSlot<ForegroundError>,
    source: Node,
    index: usize,
    label: String,
    revision: u64,
    selection_serial: u64,
}

fn group_scope(target: &Target) -> Option<Node> {
    let Target::Slot(slot) = target else {
        return None;
    };
    slot.reference_scope().ok().flatten()
}

fn same_level(first: Option<&Node>, second: Option<&Node>) -> bool {
    match (first, second) {
        (Some(first), Some(second)) => first.same_reference(second),
        (None, None) => true,
        _ => false,
    }
}

fn group_key<'a>(navigation: &'a Navigation, group: &Node) -> Option<&'a str> {
    navigation.rows.iter().find_map(|row| {
        row.target
            .node()
            .is_some_and(|node| node.same_reference(group))
            .then_some(row.key.as_str())
    })
}

fn group_path(navigation: &Navigation, group: &Node) -> Vec<Node> {
    let mut groups: Vec<Node> = Vec::new();
    let mut next = Some(group.clone());
    while let Some(group) = next {
        if groups.iter().any(|prior| prior.same_reference(&group)) {
            break;
        }
        next = group_key(navigation, &group)
            .and_then(|key| navigation.target(key))
            .as_ref()
            .and_then(group_scope);
        groups.push(group);
    }
    groups.reverse();
    groups
}

fn group_name(node: &Node) -> String {
    node.filter()
        .and_then(|group| group.parameter(2).ok())
        .filter(|property| property.name().iter().copied().eq("Name".encode_utf16()))
        .and_then(|name| name.text().ok().flatten())
        .filter(|name| !name.is_empty())
        .map_or_else(
            || String::from_utf16_lossy(&node.identity_name()),
            |name| String::from_utf16_lossy(&name),
        )
}

fn canvas_ports(node: &Node) -> Vec<mycela_engine::PointwiseSlot<ForegroundError>> {
    let mut slots = node.slots();
    let contents = node.contents();
    if !contents.is_empty()
        && let Ok(inputs) = node.reference_inputs()
    {
        // Earlier Mycela documents can carry nodes beyond the active count.
        // Keep those visible so an existing connection can still be edited.
        let mut index = 0;
        slots.retain(|slot| {
            let visible = index < inputs.len() || slot.node().is_some();
            index += 1;
            visible
        });
    }
    slots.extend(editable_contents(node));
    slots
}

fn parent_keys(key: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(key.rsplit_once('/').map(|(parent, _)| parent), |key| {
        key.rsplit_once('/').map(|(parent, _)| parent)
    })
}

#[derive(Clone)]
struct InputSocket {
    key: String,
    child: Option<usize>,
    left: f32,
}

#[derive(Clone)]
struct InputLayout {
    width: f32,
    sockets: Vec<InputSocket>,
}

impl InputLayout {
    fn height(&self) -> f32 {
        if self.sockets.is_empty() {
            0.
        } else {
            INPUT_ROW_HEIGHT
        }
    }
}

fn input_is_empty(slot: &mycela_engine::PointwiseSlot<ForegroundError>) -> bool {
    slot.node().is_none_or(|node| node.is_default_input())
}

fn card_button(id: impl Into<ElementId>, rem: Pixels, height: f32) -> Button {
    let text_size = (rem * 0.875).max(px(10.));
    Button::new(id)
        .with_size(gpui_kit::component::Size::Size(text_size / 0.875))
        .h(rem * height)
        .px(rem * 0.5)
}

fn contents_rows<'a>(
    navigation: &'a Navigation,
    node: &Node,
) -> Vec<&'a super::super::navigation::Row> {
    node.contents()
        .iter()
        .filter_map(|content| {
            navigation.rows.iter().find(
                |row| matches!(&row.target, Target::Slot(slot) if slot.same_reference(content)),
            )
        })
        .collect()
}

pub(super) struct GraphDragLabel(pub(super) String);
impl Render for GraphDragLabel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_2()
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            .child(self.0.clone())
    }
}

struct CardDragPreview {
    title: String,
    width: Pixels,
    height: Pixels,
}

impl Render for CardDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(self.width)
            .h(self.height)
            .p_2()
            .rounded_md()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .border_1()
            .border_color(cx.theme().primary)
            .opacity(0.84)
            .child(self.title.clone())
    }
}
/// UI admission policy: do not send a cycle to source-shaped recursive Parent
/// notification. It reads real slots/Contents, and never rewrites engine policy.
fn admits(source: &Node, destination: &Target, depth: usize) -> Result<(), String> {
    let forbidden = match destination {
        Target::Slot(slot) => Some(slot.parent()),
        Target::Branch(..) => None,
    };
    let mut pending = vec![(source.clone(), depth, false)];
    let mut active: Vec<Node> = Vec::new();
    let mut seen: Vec<Node> = Vec::new();
    while let Some((node, depth, leaving)) = pending.pop() {
        if leaving {
            active.pop();
            seen.push(node);
            continue;
        }
        if forbidden.as_ref().is_some_and(|p| p.same_reference(&node))
            || active.iter().any(|p| p.same_reference(&node))
        {
            return Err("This connection would create a cycle. Choose another node.".into());
        }
        if seen.iter().any(|old| old.same_reference(&node)) {
            continue;
        }
        if depth >= 128 || seen.len() + active.len() >= 4096 {
            return Err("This connection exceeds the editor’s supported graph size.".into());
        }
        active.push(node.clone());
        pending.push((node.clone(), depth, true));
        for child in node
            .slots()
            .into_iter()
            .chain(node.contents())
            .filter_map(|s| s.node())
        {
            pending.push((child, depth + 1, false));
        }
    }
    Ok(())
}
fn reachable(roots: Vec<Node>, wanted: &Node) -> bool {
    let mut pending = roots;
    let mut seen: Vec<Node> = Vec::new();
    while let Some(node) = pending.pop() {
        if node.same_reference(wanted) {
            return true;
        }
        if seen.iter().any(|n| n.same_reference(&node)) {
            continue;
        }
        if seen.len() >= 4096 {
            return false;
        }
        seen.push(node.clone());
        pending.extend(
            node.slots()
                .into_iter()
                .chain(node.contents())
                .filter_map(|s| s.node()),
        );
    }
    false
}
impl GraphDocumentView {
    pub(super) fn graph_view_menu(
        &self,
        unavailable: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let disabled = unavailable || !self.canvas.visible || self.drawing.is_some();
        let animate = !cx.reduce_motion() && !self.node_preview_animation_paused();
        let animation_disabled = disabled || cx.reduce_motion();
        let owner = cx.weak_entity();
        Button::new("graph-view-menu")
            .small()
            .ghost()
            .tab_index(0)
            .label("View")
            .dropdown_menu(move |mut menu, _, _| {
                for (label, forward) in [("Previous node", false), ("Next node", true)] {
                    let owner = owner.clone();
                    menu = menu.item(PopupMenuItem::new(label).disabled(disabled).on_click(
                        move |_, window, cx| {
                            let _ =
                                owner.update(cx, |view, cx| view.canvas_step(forward, window, cx));
                        },
                    ));
                }
                let center = owner.clone();
                let arrange = owner.clone();
                let animation = owner.clone();
                menu.item(
                    PopupMenuItem::new("Center selection")
                        .disabled(disabled)
                        .on_click(move |_, window, cx| {
                            let _ = center.update(cx, |view, cx| {
                                view.canvas_center(window);
                                cx.notify();
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("Arrange graph")
                        .disabled(disabled)
                        .on_click(move |_, _, cx| {
                            let _ = arrange.update(cx, |view, cx| {
                                view.canvas.arrange(&view.navigation);
                                cx.notify();
                            });
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Animate node previews")
                        .checked(animate)
                        .disabled(animation_disabled)
                        .on_click(move |_, _, cx| {
                            let _ = animation
                                .update(cx, |view, cx| view.toggle_node_preview_animation(cx));
                        }),
                )
            })
    }

    fn canvas_place_card(&mut self, drag: &CardDrag, window: &mut Window, cx: &mut Context<Self>) {
        if !self.canvas_current(drag.wire.revision, drag.wire.selection_serial, cx) {
            return;
        }
        let Some(target) = self.navigation.target(&drag.key).filter(|target| {
            target
                .node()
                .is_some_and(|node| drag.wire.node.same_reference(&node))
                && self.canvas_target_current(target)
        }) else {
            return;
        };
        let viewport = self.canvas.viewport(window.rem_size());
        let cursor = window.mouse_position()
            - self.canvas.scroll.bounds().origin
            - drag.grab_offset().unwrap_or_default();
        let position = viewport.world_point(cursor, self.canvas.scroll.offset());
        let docked = self
            .canvas
            .cards
            .iter()
            .any(|card| card.key == drag.key && card.receiver.is_some());
        if docked {
            if self.canvas.group().is_some() {
                self.error =
                    Some("Drop on another input to move this node within its group.".into());
                cx.notify();
                return;
            }
            self.ensure_history_binding();
            if self.history().is_none() {
                self.error = Some("This graph cannot detach the node from its input.".into());
                cx.notify();
                return;
            }
            let offset = self.canvas.branch_position(position);
            let stamp = self.begin_history_edit();
            let result = guard(&self.document).append_branch_at(drag.wire.node.clone(), offset);
            self.error = result.err().map(model::failure);
            self.finish_history_edit(stamp, self.error.is_none());
            if self.error.is_none() {
                self.revision += 1;
                self.selection_serial += 1;
                self.rebuild(window, cx);
                self.anchor_dragged_card(drag, cursor, window);
                self.start_preview(cx);
            }
            cx.notify();
            return;
        }
        if !disconnected_branch(&target, self.canvas.group()) {
            let Some(card) = self
                .canvas
                .cards
                .iter_mut()
                .find(|card| card.key == drag.key)
            else {
                return;
            };
            let before = card.canvas_position();
            for axis in 0..2 {
                card.manual_offset[axis] += position[axis] - before[axis];
            }
            self.canvas.rebuild_layout(&self.navigation, false);
            self.anchor_dragged_card(drag, cursor, window);
            cx.notify();
            return;
        }
        let position = self.canvas.branch_position(position);
        self.ensure_history_binding();
        let previously_dirty = self.dirty;
        let stamp = self.begin_history_edit();
        let result = match target {
            Target::Branch(index, _) => guard(&self.document)
                .edit_branch_position(index, position)
                .map_err(model::failure),
            Target::Slot(slot) => slot
                .parent()
                .filter()
                .expect("Group contents")
                .edit_group_content_position(&slot, position)
                .map_err(model::failure),
        };
        self.finish_history_edit(stamp, result.as_ref().is_ok_and(|changed| *changed));
        if matches!(result, Ok(false)) {
            self.dirty = previously_dirty;
        }
        self.status = None;
        self.error = result.err();
        self.selection_serial += 1;
        self.rebuild(window, cx);
        if self.error.is_none() {
            self.anchor_dragged_card(drag, cursor, window);
            cx.notify();
        }
    }

    fn anchor_dragged_card(&mut self, drag: &CardDrag, cursor: Point<Pixels>, window: &Window) {
        let Some(card) = self
            .canvas
            .cards
            .iter()
            .find(|card| card.node.same_reference(&drag.wire.node))
        else {
            return;
        };
        let viewport = Viewport::new(
            &GraphBounds::for_cards(&self.canvas.cards),
            self.canvas.viewport_size,
            window.rem_size(),
            self.canvas.zoom,
        );
        self.canvas
            .anchor_at(card.canvas_position(), cursor, viewport);
    }

    fn canvas_drop_on_card(
        &mut self,
        drag: &CardDrag,
        destination: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if drag.wire.node.same_reference(destination) {
            self.canvas_place_card(drag, window, cx);
            return;
        }
        if !self.canvas_current(drag.wire.revision, drag.wire.selection_serial, cx) {
            return;
        }
        let ports = canvas_ports(destination);
        let input = ports
            .iter()
            .find(|slot| input_is_empty(slot))
            .or_else(|| (ports.len() == 1).then(|| &ports[0]));
        let Some(input) = input else {
            self.error = Some(if ports.is_empty() {
                "This node has no inputs.".into()
            } else {
                "Drop the node onto a specific input slot to replace its connection.".into()
            });
            cx.notify();
            return;
        };
        let key = self.navigation.rows.iter().find_map(|row| {
            matches!(&row.target, Target::Slot(slot) if slot.same_reference(input))
                .then(|| row.key.clone())
        });
        if let Some(key) = key {
            self.canvas_connect(&key, &drag.wire, window, cx);
        }
    }

    fn canvas_enter_group(
        &mut self,
        key: &str,
        revision: u64,
        serial: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if revision != self.revision || serial != self.selection_serial {
            return;
        }
        let Some(target) = self
            .navigation
            .target(key)
            .filter(|target| self.canvas_target_current(target))
        else {
            return;
        };
        let Some(group) = target
            .node()
            .filter(|node| !editable_contents(node).is_empty())
        else {
            return;
        };
        let mut path = group_path(&self.navigation, &group);
        if path.is_empty() {
            path.push(group);
        }
        self.request(
            Intent::CanvasLevel {
                path,
                select_group: None,
            },
            window,
            cx,
        );
    }

    fn canvas_leave_group(&mut self, depth: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(group) = self.canvas.groups.get(depth).cloned() else {
            return;
        };
        self.request(
            Intent::CanvasLevel {
                path: self.canvas.groups[..depth].to_vec(),
                select_group: Some(group),
            },
            window,
            cx,
        );
    }

    pub(super) fn canvas_apply_level(
        &mut self,
        path: Vec<Node>,
        select_group: Option<Node>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if path.iter().chain(select_group.as_ref()).any(|group| {
            !group_key(&self.navigation, group)
                .and_then(|key| self.navigation.target(key))
                .is_some_and(|target| self.canvas_target_current(&target))
        }) {
            self.error = Some("The Group changed. Select it again before navigating.".into());
            self.rebuild(window, cx);
            return;
        }
        self.canvas.set_level(path, &self.navigation);
        self.selection_serial += 1;
        let selection = select_group
            .as_ref()
            .and_then(|group| group_key(&self.navigation, group).map(str::to_owned))
            .or_else(|| self.canvas.first_content_key(&self.navigation))
            .or_else(|| self.navigation.first());
        self.select(selection, window, cx);
        self.start_preview(cx);
    }

    pub(super) fn canvas_target_current(&self, target: &Target) -> bool {
        match target {
            Target::Slot(slot) => {
                let parent = slot.parent();
                let roots = guard(&self.document)
                    .branches()
                    .iter()
                    .flatten()
                    .cloned()
                    .collect();
                reachable(roots, &parent)
                    && parent
                        .slots()
                        .into_iter()
                        .chain(parent.contents())
                        .any(|s| s.same_reference(slot))
            }
            Target::Branch(ix, node) => {
                let current = guard(&self.document).branch(*ix);
                match (current, node) {
                    (Some(None), None) => true,
                    (Some(Some(a)), Some(b)) => a.same_reference(b),
                    _ => false,
                }
            }
        }
    }

    fn canvas_current(&mut self, revision: u64, serial: u64, cx: &mut Context<Self>) -> bool {
        if self.closed || self.is_busy() {
            return false;
        }
        if revision != self.revision || serial != self.selection_serial {
            self.error = Some("The graph selection changed. Choose the connection again.".into());
            cx.notify();
            return false;
        }
        if self.inspector.is_changed(cx) {
            self.error = Some("Correct or reset field text before changing a connection.".into());
            cx.notify();
            return false;
        }
        true
    }
    fn canvas_select(
        &mut self,
        key: String,
        revision: u64,
        serial: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() {
            return;
        }
        if revision != self.revision || serial != self.selection_serial {
            self.error =
                Some("The graph selection changed. Select its current occurrence again.".into());
            cx.notify();
            return;
        }
        let Some(target) = self.navigation.target(&key) else {
            return;
        };
        if !self.canvas_target_current(&target) {
            self.error = Some("The graph changed. Select its current occurrence again.".into());
            self.rebuild(window, cx);
            return;
        }
        self.selection_serial += 1;
        self.request(Intent::Select(key), window, cx);
    }

    fn canvas_toggle_inputs(
        &mut self,
        key: &str,
        revision: u64,
        serial: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.canvas_current(revision, serial, cx) {
            return;
        }
        let Some(card) = self.canvas.visible_cards().find(|card| card.key == key) else {
            return;
        };
        if self.has_field_drafts(cx) {
            self.error =
                Some("Correct or reset field text before hiding or showing inputs.".into());
            cx.notify();
            return;
        }
        let id = card.node.occurrence_id();
        let world = card.body_position();
        let rem = window.rem_size();
        let screen = self.canvas.viewport(rem).surface_point(world) + self.canvas.scroll.offset();
        let collapsing = !self.canvas.collapsed.remove(&id);
        if collapsing {
            self.canvas.collapsed.insert(id);
        }
        self.canvas.rebuild_layout(&self.navigation, false);
        let anchor = self
            .canvas
            .visible_cards()
            .find(|card| card.key == key)
            .map(Card::body_position);
        if self.canvas.has_viewport()
            && let Some(anchor) = anchor
        {
            let viewport = Viewport::new(
                &GraphBounds::for_cards(&self.canvas.cards),
                self.canvas.viewport_size,
                rem,
                self.canvas.zoom,
            );
            self.canvas.anchor_at(anchor, screen, viewport);
        }
        // Hidden occurrences remain available in List and in the render graph.
        if collapsing
            && self.selection.as_deref().is_some_and(|selected| {
                parent_keys(selected).any(|parent| parent == key)
                    && self.navigation.target(selected).is_some_and(|target| {
                        same_level(self.canvas.group(), group_scope(&target).as_ref())
                    })
            })
        {
            self.select(Some(key.into()), window, cx);
            self.start_preview(cx);
        }
        self.selection_serial += 1;
        cx.notify();
    }
    fn canvas_replace(
        &mut self,
        key: &str,
        revision: u64,
        serial: u64,
        identity: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.canvas_current(revision, serial, cx) {
            return;
        }
        let Some(target) = self.navigation.target(key) else {
            return;
        };
        if !self.canvas_target_current(&target) {
            self.error =
                Some("The input was replaced. Select its current occurrence again.".into());
            self.rebuild(window, cx);
            self.start_preview(cx);
            return;
        }
        let anchor = self.canvas_socket_screen(key, window);
        self.ensure_history_binding();
        self.select(Some(key.into()), window, cx);
        self.replace(identity, window, cx);
        self.restore_canvas_socket(key, anchor, window);
    }

    fn canvas_socket_screen(&self, key: &str, window: &Window) -> Option<Point<Pixels>> {
        let world = self.canvas.socket_position(key)?;
        let viewport = self.canvas.viewport(window.rem_size());
        Some(viewport.surface_point(world) + self.canvas.scroll.offset())
    }

    fn restore_canvas_socket(&mut self, key: &str, screen: Option<Point<Pixels>>, window: &Window) {
        if self.error.is_none()
            && let Some(screen) = screen
            && let Some(world) = self.canvas.socket_position(key)
        {
            let viewport = Viewport::new(
                &GraphBounds::for_cards(&self.canvas.cards),
                self.canvas.viewport_size,
                window.rem_size(),
                self.canvas.zoom,
            );
            self.canvas.anchor_at(world, screen, viewport);
        }
    }

    fn canvas_insert_below(
        &mut self,
        key: &str,
        revision: u64,
        serial: u64,
        identity: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.canvas_current(revision, serial, cx) {
            return;
        }
        let Some(target) = self.navigation.target(key) else {
            return;
        };
        let Some(old) = target
            .node()
            .filter(|_| self.canvas_target_current(&target))
        else {
            self.error = Some("The selected branch changed. Select a node again.".into());
            self.rebuild(window, cx);
            return;
        };
        if let Err(error) = admits(&old, &target, key.matches("/slot-").count() + 1) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let node = match new_graph_node(identity) {
            Ok(node) => node,
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        self.ensure_history_binding();
        let stamp = self.begin_history_edit();
        self.revision += 1;
        self.status = None;
        self.error = self.insert_below_target(&target, &node).err();
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        if self.canvas.visible {
            self.canvas.arrange(&self.navigation);
            self.canvas_center(window);
        }
        self.start_preview(cx);
    }

    pub(super) fn insert_below_picker(&self, cx: &Context<Self>) -> Option<gpui_kit::AnyElement> {
        let key = self.selection.clone()?;
        let node = self.selected_node()?;
        if node.is_default_input() {
            return None;
        }
        let owner = cx.weak_entity();
        let revision = self.revision;
        let serial = self.selection_serial;
        Some(
            Button::new("graph-insert-below")
                .small()
                .label("Insert Below…")
                .disabled(self.is_busy())
                .dropdown_menu(move |menu, _, _| {
                    let owner = owner.clone();
                    let key = key.clone();
                    node_catalog_choices(menu, NODES_WITH_INPUTS, move |identity, window, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            view.canvas_insert_below(&key, revision, serial, identity, window, cx);
                        });
                    })
                })
                .into_any_element(),
        )
    }
    fn canvas_connect(
        &mut self,
        key: &str,
        drag: &WireDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.canvas_current(drag.revision, drag.selection_serial, cx) {
            return;
        }
        let Some(target) = self.navigation.target(key) else {
            return;
        };
        if target
            .node()
            .is_some_and(|node| node.same_reference(&drag.node))
        {
            return;
        }
        if !self.canvas_target_current(&target) {
            self.error =
                Some("The input was replaced. Select its current occurrence again.".into());
            self.rebuild(window, cx);
            self.start_preview(cx);
            return;
        }
        let roots = guard(&self.document)
            .branches()
            .iter()
            .flatten()
            .cloned()
            .collect();
        if !reachable(roots, &drag.node) {
            self.error = Some("The source node is no longer in this document.".into());
            cx.notify();
            return;
        }
        if let Err(error) = admits(&drag.node, &target, 0) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let anchor = self.canvas_socket_screen(key, window);
        if drag.share {
            let Target::Slot(destination) = target else {
                return;
            };
            self.ensure_history_binding();
            let stamp = self.begin_history_edit();
            self.revision += 1;
            self.status = None;
            let result = guard(&self.document).reference_copy_output(&drag.node, &destination);
            self.error = result.err().map(model::failure);
            self.finish_history_edit(stamp, self.error.is_none());
            self.rebuild(window, cx);
            if self.error.is_none() {
                let selected = self
                    .navigation
                    .rows
                    .iter()
                    .find_map(|row| match &row.target {
                        Target::Slot(slot) if slot.same_reference(&destination) => {
                            Some(row.key.clone())
                        }
                        _ => None,
                    });
                if let Some(key) = selected.as_deref()
                    && let Some(target) = self.navigation.target(key)
                {
                    self.canvas.follow_selection(&self.navigation, key, &target);
                }
                self.select(selected, window, cx);
            }
            self.start_preview(cx);
        } else {
            self.commit_canvas_input(&target, &drag.node, window, cx);
        }
        self.restore_canvas_socket(key, anchor, window);
    }

    fn commit_canvas_input(
        &mut self,
        target: &Target,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ensure_history_binding();
        let stamp = self.begin_history_edit();
        self.revision += 1;
        self.status = None;
        self.error = self.edit_target(target, Some(node)).err();
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        self.start_preview(cx);
    }

    fn group_inputs(&self, group: &Node) -> Vec<GroupInput> {
        group
            .reference_inputs()
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let source = slot.node()?;
                Some(GroupInput {
                    group: group.clone(),
                    slot,
                    index,
                    label: format!(
                        "Input {} · {}",
                        index + 1,
                        if source.is_default_input() {
                            "Default preview input".into()
                        } else {
                            String::from_utf16_lossy(&source.identity_name())
                        }
                    ),
                    source,
                    revision: self.revision,
                    selection_serial: self.selection_serial,
                })
            })
            .collect()
    }

    fn insert_group_reference(
        &mut self,
        key: &str,
        input: &GroupInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.canvas_current(input.revision, input.selection_serial, cx) {
            return;
        }
        let Some(target) = self.navigation.target(key) else {
            return;
        };
        let current = group_scope(&target).is_some_and(|group| {
            group.same_reference(&input.group)
                && group.reference_inputs().is_ok_and(|slots| {
                    slots.get(input.index).is_some_and(|slot| {
                        slot.same_reference(&input.slot) && slot.node().is_some()
                    })
                })
        });
        if !self.canvas_target_current(&target) || !current {
            self.error = Some("Choose a connected input from this Group.".into());
            cx.notify();
            return;
        }
        let reference = mycela_engine::PointwiseFilter::new(
            mycela_engine::PointwiseKind::Reference,
        )
        .and_then(|reference| {
            reference
                .parameter(0)?
                .set_number((input.index + 1) as f64)?;
            Ok(reference.node())
        });
        match reference {
            Ok(reference) => {
                self.commit_canvas_input(&target, &reference, window, cx);
            }
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
            }
        }
    }

    fn group_input_menu(
        mut menu: PopupMenu,
        owner: &gpui_kit::WeakEntity<Self>,
        key: &str,
        inputs: &[GroupInput],
    ) -> PopupMenu {
        for input in inputs {
            let input = input.clone();
            let owner = owner.clone();
            let key = key.to_string();
            menu = menu.item(
                PopupMenuItem::new(input.label.clone()).on_click(move |_, w, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.insert_group_reference(&key, &input, w, cx)
                    });
                }),
            );
        }
        menu
    }

    pub(super) fn group_input_picker(&self, cx: &Context<Self>) -> Option<gpui_kit::AnyElement> {
        let key = self.selection.as_deref()?;
        let group = group_scope(&self.navigation.target(key)?)?;
        let inputs = self.group_inputs(&group);
        if inputs.is_empty() {
            return None;
        }
        let key = key.to_string();
        let owner = cx.weak_entity();
        Some(
            Button::new("graph-group-input-reference")
                .small()
                .label("Use group input…")
                .disabled(self.is_busy())
                .dropdown_menu(move |menu, _, _| {
                    Self::group_input_menu(menu, &owner, &key, &inputs)
                })
                .into_any_element(),
        )
    }

    fn canvas_insert_document_input(
        &mut self,
        identity: &str,
        revision: u64,
        serial: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.canvas_current(revision, serial, cx) || self.canvas.group().is_some() {
            return;
        }
        let input = match new_graph_node(identity) {
            Ok(node) => node,
            Err(error) => {
                self.error = Some(model::failure(error));
                cx.notify();
                return;
            }
        };
        self.ensure_history_binding();
        let stamp = self.begin_history_edit();
        self.revision += 1;
        self.status = None;
        self.error = guard(&self.document)
            .insert_document_input(&input)
            .err()
            .map(model::failure);
        self.finish_history_edit(stamp, self.error.is_none());
        self.rebuild(window, cx);
        if self.error.is_none() {
            self.select(self.navigation.first(), window, cx);
            self.canvas_center(window);
        }
        self.start_preview(cx);
    }

    pub(super) fn canvas_add_node(
        &mut self,
        identity: &str,
        revision: u64,
        serial: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.canvas_current(revision, serial, cx) {
            self.canvas_insert_library_node(identity, None, window, cx);
        }
    }

    fn canvas_drop_library_node(
        &mut self,
        drag: &LibraryNodeDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if drag.owner != cx.entity_id()
            || !self.canvas.has_viewport()
            || !self.canvas_current(drag.revision, drag.selection_serial, cx)
        {
            return;
        }
        let cursor = window.mouse_position() - self.canvas.scroll.bounds().origin;
        let viewport = self.canvas.viewport(window.rem_size());
        let offset = self.canvas.scroll.offset();
        if self
            .canvas
            .visible_cards()
            .any(|card| viewport.card_bounds(card, offset).contains(&cursor))
        {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before adding a node.".into());
            cx.notify();
            return;
        }
        self.canvas_insert_library_node(&drag.identity, Some(cursor), window, cx);
    }

    fn canvas_insert_library_node(
        &mut self,
        identity: &str,
        cursor: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use mycela_engine::GtxGraphError;

        let group = self.canvas.group().and_then(Node::filter);
        if let Some(group) = &group
            && !group_key(&self.navigation, &group.node())
                .and_then(|key| self.navigation.target(key))
                .is_some_and(|target| self.canvas_target_current(&target))
        {
            self.error = Some("The Group changed. Enter it again before adding a node.".into());
            self.rebuild(window, cx);
            return;
        }
        let node = match new_graph_node(identity) {
            Ok(node) => node,
            Err(error) => {
                self.error = Some(model::failure(error));
                cx.notify();
                return;
            }
        };
        let input_height = if canvas_ports(&node).is_empty() {
            0.
        } else {
            INPUT_ROW_HEIGHT
        };
        let position = cursor.map(|cursor| {
            let viewport = self.canvas.viewport(window.rem_size());
            let [x, y] = viewport.world_point(cursor, self.canvas.scroll.offset());
            self.canvas
                .branch_position([x - CARD_WIDTH / 2., y - CARD_TITLE_CENTER_Y - input_height])
        });
        let empty_canvas = self.canvas.cards.is_empty();
        if cursor.is_some() || group.is_some() {
            self.ensure_history_binding();
        }
        let stamp = self.begin_history_edit();
        self.revision += 1;
        self.status = None;
        let result = if let Some(group) = &group {
            let vacant = group
                .node()
                .contents()
                .into_iter()
                .find(|slot| slot.node().is_none());
            if let Some(slot) = vacant.filter(|_| cursor.is_none() || empty_canvas) {
                self.edit_target(&Target::Slot(slot), Some(&node))
            } else {
                self.edit_with_history_fallback(|_| {
                    group
                        .edit_append_group_branch(position.unwrap_or_default(), &node)
                        .map(|_| ())
                        .map_err(|error| match error {
                            PointwiseError::History(cause) => GtxGraphError::History(*cause),
                            error => GtxGraphError::Node(error.into()),
                        })
                })
            }
        } else if cursor.is_some() && empty_canvas {
            self.edit_target(&Target::Branch(0, None), Some(&node))
        } else {
            self.edit_with_history_fallback(|document| {
                document
                    .append_branch_at(node.clone(), position.unwrap_or_default())
                    .map(|_| ())
            })
        };
        self.finish_history_edit(stamp, result.is_ok());
        self.error = result.err();
        self.rebuild(window, cx);
        if self.error.is_none()
            && let Some(card) = self
                .canvas
                .cards
                .iter()
                .find(|card| card.node.same_reference(&node))
        {
            let key = card.key.clone();
            let [x, y] = card.canvas_position();
            self.select(Some(key), window, cx);
            if let Some(cursor) = cursor {
                let viewport = Viewport::new(
                    &GraphBounds::for_cards(&self.canvas.cards),
                    self.canvas.viewport_size,
                    window.rem_size(),
                    self.canvas.zoom,
                );
                self.canvas.anchor_at(
                    [x + CARD_WIDTH / 2., y + CARD_TITLE_CENTER_Y + input_height],
                    cursor,
                    viewport,
                );
                if let Some(focus) = &self.canvas.focus {
                    focus.focus(window, cx);
                }
            } else {
                self.canvas_center(window);
            }
        }
        self.start_preview(cx);
    }
    fn canvas_step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let cards: Vec<_> = self.canvas.visible_cards().collect();
        if cards.is_empty() || self.is_busy() {
            return;
        }
        let selected = self.selected_node();
        let old = selected
            .and_then(|s| cards.iter().position(|c| c.node.same_reference(&s)))
            .unwrap_or(0);
        let ix = if forward {
            (old + 1) % cards.len()
        } else {
            (old + cards.len() - 1) % cards.len()
        };
        let key = cards[ix].key.clone();
        self.canvas_select(
            key.clone(),
            self.revision,
            self.selection_serial,
            window,
            cx,
        );
        if self.modal || self.selection.as_deref() != Some(key.as_str()) {
            return;
        }
        self.canvas_center(window);
        cx.notify();
    }
    pub(super) fn canvas_center(&mut self, window: &Window) {
        if !self.canvas.has_viewport() {
            self.canvas.center_on_measure = true;
            return;
        }
        let Some(node) = self.selected_node() else {
            return;
        };
        let Some(card) = self
            .canvas
            .visible_cards()
            .find(|c| c.node.same_reference(&node))
        else {
            return;
        };
        let [x, y] = card.canvas_position();
        let viewport = self.canvas.viewport(window.rem_size());
        let center = [x + CARD_WIDTH / 2., y + card.height() / 2.];
        self.canvas
            .scroll
            .set_offset(viewport.place_at(center, viewport.center()));
    }

    fn change_canvas_zoom(
        &mut self,
        zoom: Zoom,
        cursor: Option<Point<Pixels>>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.canvas.set_zoom(zoom, cursor, window.rem_size()) {
            cx.notify();
        }
    }

    fn step_canvas_zoom(
        &mut self,
        delta: f32,
        cursor: Option<Point<Pixels>>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.change_canvas_zoom(
            Zoom::new(self.canvas.zoom.factor() + delta),
            cursor,
            window,
            cx,
        );
    }

    fn fit_canvas(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.canvas.has_viewport() && !self.canvas.cards.is_empty() {
            self.canvas.fit(window.rem_size());
            cx.notify();
        }
    }
    pub(super) fn connection_picker(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let sources = self
            .canvas
            .cards
            .iter()
            .map(|card| (card.node.clone(), card.id))
            .collect();
        self.connection_picker_with_sources(cx, false, sources)
    }

    pub(super) fn share_output_picker(&self, cx: &Context<Self>) -> Option<gpui_kit::AnyElement> {
        let target = self.navigation.target(self.selection.as_deref()?)?;
        if !matches!(target, Target::Slot(_)) {
            return None;
        }
        let sources = self.share_output_sources(&target);
        (!sources.is_empty()).then(|| self.connection_picker_with_sources(cx, true, sources))
    }

    fn share_output_sources(&self, target: &Target) -> Vec<(Node, u64)> {
        let scope = group_scope(target);
        if self.canvas.visible || same_level(self.canvas.group(), scope.as_ref()) {
            return self
                .canvas
                .cards
                .iter()
                .filter(|card| card.node.parent().is_some())
                .map(|card| (card.node.clone(), card.id))
                .collect();
        }
        // The List can inspect a nested input while the canvas still shows the
        // surrounding document. Its sharing candidates come from that input's Group.
        let mut seen = std::collections::HashSet::new();
        self.navigation
            .rows
            .iter()
            .filter(|row| same_level(scope.as_ref(), group_scope(&row.target).as_ref()))
            .filter_map(|row| row.target.node())
            .filter(|node| node.parent().is_some() && seen.insert(node.occurrence_id()))
            .map(|node| {
                let id = node.occurrence_id();
                (node, id)
            })
            .collect()
    }

    fn connection_picker_with_sources(
        &self,
        cx: &Context<Self>,
        share: bool,
        sources: Vec<(Node, u64)>,
    ) -> gpui_kit::AnyElement {
        let owner = cx.weak_entity();
        let key = self.selection.clone();
        let revision = self.revision;
        let serial = self.selection_serial;
        let label = if share {
            "Share output…"
        } else if self.history().is_some() {
            "Move node…"
        } else {
            "Connect existing…"
        };
        let id = if share {
            "graph-share-output"
        } else {
            "graph-connect-existing"
        };
        Button::new(id)
            .small()
            .label(label)
            .disabled(self.is_busy() || key.is_none() || sources.is_empty())
            .dropdown_menu(move |mut menu, _, _| {
                for (node, id) in &sources {
                    let owner = owner.clone();
                    let key = key.clone();
                    let drag = WireDrag {
                        revision,
                        selection_serial: serial,
                        node: node.clone(),
                        share,
                    };
                    menu = menu.item(
                        PopupMenuItem::new(format!(
                            "{} · {}",
                            String::from_utf16_lossy(&node.identity_name()),
                            id
                        ))
                        .on_click(move |_, w, cx| {
                            if let Some(key) = &key {
                                let _ =
                                    owner.update(cx, |v, cx| v.canvas_connect(key, &drag, w, cx));
                            }
                        }),
                    );
                }
                menu.scrollable(true)
            })
            .into_any_element()
    }
    fn canvas_socket_menu(
        &self,
        button: Button,
        row: &super::super::navigation::Row,
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        let menu_owner = cx.weak_entity();
        let menu_key = row.key.clone();
        let revision = self.revision;
        let serial = self.selection_serial;
        let empty = row.target.node().is_none();
        let insert_label = if empty {
            "Add input node"
        } else {
            "Replace input node"
        };
        let verb = if self.history().is_some() {
            "Move"
        } else {
            "Connect"
        };
        let sources: Vec<_> = self
            .canvas
            .visible_cards()
            .map(|card| (card.node.clone(), card.id))
            .collect();
        let group_inputs = group_scope(&row.target)
            .map(|group| self.group_inputs(&group))
            .unwrap_or_default();
        button
            .dropdown_menu(move |mut menu, window, cx| {
                let owner = menu_owner.clone();
                let key = menu_key.clone();
                menu = menu.item(
                    PopupMenuItem::new("Select input").on_click(move |_, w, cx| {
                        let _ = owner.update(cx, |v, cx| {
                            v.canvas_select(key.clone(), revision, serial, w, cx)
                        });
                    }),
                );
                let owner = menu_owner.clone();
                let key = menu_key.clone();
                menu = menu.item(
                    PopupMenuItem::new("Disconnect input")
                        .disabled(empty)
                        .on_click(move |_, w, cx| {
                            let _ = owner.update(cx, |v, cx| {
                                v.canvas_replace(&key, revision, serial, None, w, cx)
                            });
                        }),
                );
                for (source, id) in &sources {
                    let owner = menu_owner.clone();
                    let key = menu_key.clone();
                    let drag = WireDrag {
                        revision,
                        selection_serial: serial,
                        node: source.clone(),
                        share: false,
                    };
                    let label = format!(
                        "{verb} {} · {}",
                        String::from_utf16_lossy(&source.identity_name()),
                        id
                    );
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, w, cx| {
                        let _ = owner.update(cx, |v, cx| v.canvas_connect(&key, &drag, w, cx));
                    }));
                }
                if !group_inputs.is_empty() {
                    let owner = menu_owner.clone();
                    let key = menu_key.clone();
                    let inputs = group_inputs.clone();
                    menu = menu.submenu("Use group input", window, cx, move |menu, _, _| {
                        Self::group_input_menu(menu, &owner, &key, &inputs)
                    });
                }
                let owner = menu_owner.clone();
                let key = menu_key.clone();
                menu.separator()
                    .submenu(insert_label, window, cx, move |menu, _, _| {
                        let owner = owner.clone();
                        let key = key.clone();
                        node_catalog(menu, move |identity, window, cx| {
                            let _ = owner.update(cx, |view, cx| {
                                view.canvas_replace(
                                    &key,
                                    revision,
                                    serial,
                                    Some(identity),
                                    window,
                                    cx,
                                );
                            });
                        })
                    })
                    .scrollable(true)
            })
            .into_any_element()
    }

    fn canvas_socket_target(
        &self,
        key: &str,
        child: gpui_kit::AnyElement,
        frame: Rc<RefCell<FrameGeometry>>,
        receiver: u64,
        rem: Pixels,
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        let frame_key = key.to_owned();
        let wire_key = key.to_owned();
        let card_key = key.to_owned();
        let reference_key = key.to_owned();
        let library_key = key.to_owned();
        div()
            .id(format!("graph-drop-{}", key.replace('/', "-")))
            .relative()
            .flex_none()
            .w(rem * CARD_WIDTH)
            .h(rem * CARD_WIDTH)
            .on_prepaint(move |bounds, _, _| {
                let x = bounds.center().x;
                frame.borrow_mut().inputs.insert(
                    frame_key.clone(),
                    (receiver, point(x, bounds.top()), point(x, bounds.bottom())),
                );
            })
            .test_support()
            .on_drop::<CardDrag>(cx.listener(move |view, drag: &CardDrag, window, cx| {
                view.canvas_connect(&card_key, &drag.wire, window, cx)
            }))
            .on_drop::<WireDrag>(cx.listener(move |view, drag, window, cx| {
                view.canvas_connect(&wire_key, drag, window, cx)
            }))
            .on_drop::<GroupInput>(cx.listener(move |view, input, window, cx| {
                view.insert_group_reference(&reference_key, input, window, cx)
            }))
            .on_drop::<LibraryNodeDrag>(cx.listener(
                move |view, drag: &LibraryNodeDrag, window, cx| {
                    if drag.owner != cx.entity_id() {
                        return;
                    }
                    view.canvas_replace(
                        &library_key,
                        drag.revision,
                        drag.selection_serial,
                        Some(&drag.identity),
                        window,
                        cx,
                    );
                    if view.error.is_none()
                        && let Some(focus) = &view.canvas.focus
                    {
                        focus.focus(window, cx);
                    }
                },
            ))
            .child(child)
            .into_any_element()
    }

    fn texture_output_terminal(
        &self,
        source: Option<&Node>,
        rem: Pixels,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        use super::results::TEXTURE_OUTPUT_LABEL;
        let theme = cx.theme();
        let destination = source
            .map(|node| String::from_utf16_lossy(&node.identity_name()))
            .unwrap_or_else(|| "Not connected".into());
        div()
            .id("graph-texture-output")
            .aria_label(format!("{TEXTURE_OUTPUT_LABEL} · {destination}"))
            .test_support()
            .v_flex()
            .items_center()
            .justify_center()
            .w(rem * CARD_WIDTH)
            .h(rem * 2.6)
            .rounded_sm()
            .border_1()
            .border_color(theme.muted_foreground)
            .bg(theme.muted)
            .text_size((rem * 0.8).max(px(10.)))
            .child(div().font_semibold().child(TEXTURE_OUTPUT_LABEL))
            .child(
                div()
                    .w_full()
                    .px_1()
                    .truncate()
                    .text_center()
                    .text_color(theme.muted_foreground)
                    .child(destination),
            )
    }

    pub(super) fn render_canvas(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let window_rem = window.rem_size();
        let viewport = self.canvas.viewport(window_rem);
        let rem = viewport.rem;
        let measured = self.canvas.has_viewport();
        let focus = self.canvas.focus.clone();
        let theme = cx.theme().clone();
        let busy = self.is_busy();
        let library_drop_ready = !busy && !self.has_field_drafts(cx);
        let document_id = cx.entity_id();
        let revision = self.revision;
        let serial = self.selection_serial;
        let selected = self.selected_node();
        let input_bar = self
            .canvas
            .group()
            .cloned()
            .or_else(|| {
                selected
                    .as_ref()
                    .filter(|node| !node.contents().is_empty())
                    .cloned()
            })
            .or_else(|| {
                self.selection
                    .as_deref()
                    .and_then(|key| self.navigation.target(key))
                    .as_ref()
                    .and_then(group_scope)
            })
            .map(|group| self.group_inputs(&group))
            .unwrap_or_default();
        let cards: Vec<_> = self.canvas.visible_cards().cloned().collect();
        let owner = cx.weak_entity();
        let moving = self.history().is_some();
        let verb = if moving { "Move" } else { "Connect" };
        let branches: Vec<_> = if let Some(group) = self.canvas.group() {
            contents_rows(&self.navigation, group)
        } else {
            self.navigation
                .rows
                .iter()
                .filter(|row| matches!(row.target, Target::Branch(..)))
                .collect()
        };
        let branches = branches
            .into_iter()
            .map(|r| (r.key.clone(), r.label.clone()))
            .collect::<Vec<_>>();
        let branch_owner = owner.clone();
        let library_width = if window.viewport_size().width >= window_rem * 72. {
            rems(13.)
        } else {
            rems(11.)
        };
        let input_owner = owner.clone();
        let document_input = Button::new("graph-document-input")
            .small()
            .ghost()
            .label("Document input…")
            .tooltip("Insert an input into the document graph")
            .tab_index(0)
            .disabled(busy)
            .dropdown_menu(move |menu, _, _| {
                let owner = input_owner.clone();
                node_catalog(menu, move |identity, window, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.canvas_insert_document_input(identity, revision, serial, window, cx);
                    });
                })
            });
        let branch_picker = Button::new("graph-canvas-branches")
            .small()
            .label("Go to branch…")
            .tab_index(0)
            .disabled(busy)
            .dropdown_menu(move |mut menu, _, _| {
                for (key, label) in &branches {
                    let owner = branch_owner.clone();
                    let key = key.clone();
                    menu =
                        menu.item(PopupMenuItem::new(label.clone()).on_click(move |_, w, cx| {
                            let _ = owner.update(cx, |v, cx| {
                                v.canvas_select(key.clone(), revision, serial, w, cx)
                            });
                        }));
                }
                menu.scrollable(true)
            });
        let frame = Rc::new(RefCell::new(FrameGeometry::default()));
        let visible_card_ids: HashMap<_, _> = cards
            .iter()
            .map(|card| (card.key.clone(), card.id))
            .collect();
        let links: Vec<_> = cards
            .iter()
            .filter(|card| !card.collapsed)
            .flat_map(|card| &card.inputs.sockets)
            .filter(|socket| socket.child.is_none())
            .filter_map(|socket| {
                let source = self.navigation.target(&socket.key)?.node()?;
                let card = cards
                    .iter()
                    .find(|card| card.node.same_reference(&source))?;
                Some((card.id, socket.key.clone()))
            })
            .collect();
        let color = theme.muted_foreground;
        let painted = frame.clone();
        // All children prepaint before paint. These are actual native port
        // bounds, scoped to this frame/rem/scroll, not cached geometry guesses.
        let edges = canvas(
            move |_, _, _| (),
            move |_, _, window, _| {
                let geometry = painted.borrow();
                for (receiver, _, bottom) in geometry.inputs.values() {
                    if let Some(top) = geometry.receivers.get(receiver) {
                        let junction = point(top.x, top.y - rem * 0.65);
                        draw_wire(window, *bottom, junction, rem, color);
                    }
                }
                for top in geometry.receivers.values() {
                    draw_wire(window, point(top.x, top.y - rem * 0.65), *top, rem, color);
                }
                for (output, input) in links {
                    let Some((_, a)) = geometry.outputs.iter().find(|(id, _)| *id == output) else {
                        continue;
                    };
                    let Some((_, b, _)) = geometry.inputs.get(&input) else {
                        continue;
                    };
                    draw_wire(window, *a, *b, rem, color);
                }
            },
        )
        .absolute()
        .size_full();
        let offset = self.canvas.scroll.offset();
        let mut surface = div()
            .id("graph-canvas-surface")
            .relative()
            .flex_none()
            .w(viewport.surface.width)
            .h(viewport.surface.height)
            .on_scroll_wheel(cx.listener(Self::wheel_canvas_zoom))
            .child(edges);
        if cards.is_empty() {
            surface = surface.child(
                div()
                    .p_4()
                    .mt_6()
                    .text_color(theme.muted_foreground)
                    .child("Choose a node from the library."),
            );
        }
        let top_level = self.canvas.group().is_none();
        let root = top_level.then(|| self.texture_output()).flatten();
        if let Some(card) = cards.iter().find(|card| {
            card.texture_output
                && root
                    .as_ref()
                    .is_some_and(|node| node.same_reference(&card.node))
        }) {
            let [x, y] = card.body_position();
            let origin = viewport.surface_point([x, y + CARD_WIDTH]);
            surface = surface.child(
                div()
                    .absolute()
                    .left(origin.x)
                    .top(origin.y)
                    .w(rem * CARD_WIDTH)
                    .v_flex()
                    .items_center()
                    .child(div().w(px(1.)).h(rem * 1.2).bg(theme.muted_foreground))
                    .child(self.texture_output_terminal(root.as_ref(), rem, cx)),
            );
        }
        let mut preview_nodes = input_bar
            .iter()
            .map(|input| input.source.clone())
            .collect::<Vec<_>>();
        let mut output_handles = Vec::new();
        for (position, card) in cards.into_iter().enumerate() {
            if measured && !viewport.card_visible(&card, offset) {
                continue;
            }
            let top_left = viewport.surface_point(card.canvas_position());
            let is_group = !editable_contents(&card.node).is_empty();
            let title = if !card.node.contents().is_empty() {
                group_name(&card.node)
            } else {
                String::from_utf16_lossy(&card.node.identity_name())
            };
            let key = card.key.clone();
            let selected = selected
                .as_ref()
                .is_some_and(|n| n.same_reference(&card.node));
            if measured || position < 4 {
                preview_nodes.push(card.node.clone());
            }
            let payload = WireDrag {
                revision,
                selection_serial: serial,
                node: card.node.clone(),
                share: card.node.parent().is_some(),
            };
            let output_verb = if payload.share { "Share" } else { verb };
            let output_label = if payload.share {
                "Output · drag to share"
            } else {
                "Output · drag to an input"
            };
            let card_id = card.id;
            let output_frame = frame.clone();
            let body_origin = viewport.surface_point(card.body_position());
            let output = div()
                .absolute()
                .left(body_origin.x + rem * (CARD_WIDTH - 1.75) / 2.)
                .top(body_origin.y + rem * (CARD_WIDTH - 0.75))
                .w(rem * 1.75)
                .h(rem * 1.5)
                .flex()
                .items_center()
                .justify_center()
                .id(format!("graph-output-{}", card.id))
                .aria_label(output_label)
                .on_prepaint(move |bounds, _, _| {
                    output_frame
                        .borrow_mut()
                        .outputs
                        .push((card_id, bounds.center()));
                })
                .test_support()
                .child(
                    div()
                        .size(rem * 0.55)
                        .rounded_full()
                        .bg(theme.foreground)
                        .border_1()
                        .border_color(theme.background),
                );
            let output = if busy {
                output
            } else {
                output.block_mouse_except_scroll().cursor_grab().on_drag(
                    payload,
                    move |drag, _, _, cx| {
                        cx.new(|_| {
                            GraphDragLabel(format!(
                                "{output_verb} {}",
                                String::from_utf16_lossy(&drag.node.identity_name())
                            ))
                        })
                    },
                )
            };
            let title_key = key.clone();
            let mut header = div()
                .h_flex()
                .absolute()
                .top(px(1.))
                .left(px(1.))
                .right(px(1.))
                .h(rem * CARD_TITLE_HEIGHT)
                .bg(theme.background.opacity(0.94))
                .border_b_1()
                .border_color(theme.border)
                .child(
                    card_button(format!("graph-node-{}", card.id), rem, CARD_TITLE_HEIGHT)
                        .flex_1()
                        .min_w_0()
                        .ghost()
                        .font_semibold()
                        .text_color(if selected {
                            theme.primary
                        } else {
                            theme.foreground
                        })
                        .label(title.clone())
                        .tooltip(title.clone())
                        .selected(selected)
                        .disabled(busy)
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.canvas_select(title_key.clone(), revision, serial, window, cx)
                        })),
                );
            let ports = canvas_ports(&card.node);
            if !ports.is_empty() {
                let label = if card.collapsed {
                    "Show inputs"
                } else {
                    "Hide inputs"
                };
                header = header.child(
                    card_button(format!("graph-fold-{}", card.id), rem, CARD_TITLE_HEIGHT)
                        .w(rem * 1.75)
                        .flex_none()
                        .ghost()
                        .label(if card.collapsed { "+" } else { "−" })
                        .accessibility_label(format!("{label} for {title}"))
                        .tooltip(label)
                        .tab_index(0)
                        .disabled(busy)
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.canvas_toggle_inputs(&key, revision, serial, window, cx)
                        })),
                );
            }
            let input_height = card.input_height();
            let body_height = rem * CARD_WIDTH;
            let card_origin = Rc::new(Cell::new(
                self.canvas.scroll.bounds().origin + top_left + offset,
            ));
            let measured_origin = card_origin.clone();
            let hover_border = theme.primary;
            let drag_node = card.node.clone();
            let card_drag = CardDrag {
                key: card.key.clone(),
                wire: WireDrag {
                    revision,
                    selection_serial: serial,
                    node: card.node.clone(),
                    share: false,
                },
                grab: self.canvas.card_grab.clone(),
            };
            let grab = card_drag.grab.clone();
            let occurrence = card.node.occurrence_id();
            let receiver_frame = frame.clone();
            let mut node = div()
                .id(format!("graph-card-body-{}", card.id))
                .on_prepaint(move |bounds, _, _| {
                    if input_height > 0. {
                        receiver_frame
                            .borrow_mut()
                            .receivers
                            .insert(card_id, point(bounds.center().x, bounds.top()));
                    }
                })
                .test_support()
                .relative()
                .w_full()
                .h(body_height)
                .flex_none()
                .bg(theme.muted)
                .border_1()
                .border_color(if selected {
                    theme.primary
                } else {
                    theme.border
                })
                .rounded_sm()
                .when(!busy, |body| {
                    body.cursor_grab()
                        .on_mouse_down(MouseButton::Left, move |event, _, _| {
                            grab.set(Some(CardGrab {
                                card: occurrence,
                                offset: event.position - measured_origin.get(),
                            }));
                        })
                        .drag_over::<CardDrag>(move |style, drag, _, _| {
                            if library_drop_ready
                                && drag.wire.revision == revision
                                && drag.wire.selection_serial == serial
                            {
                                style.border_color(hover_border)
                            } else {
                                style
                            }
                        })
                        .when(card.receiver.is_some(), |body| {
                            body.drag_over::<LibraryNodeDrag>(move |style, drag, _, _| {
                                if library_drop_ready
                                    && drag.owner == document_id
                                    && drag.revision == revision
                                    && drag.selection_serial == serial
                                {
                                    style.border_color(hover_border)
                                } else {
                                    style
                                }
                            })
                        })
                        .on_drag(card_drag, move |drag, offset, _, cx| {
                            if drag.grab_offset().is_none() {
                                drag.grab.set(Some(CardGrab {
                                    card: occurrence,
                                    offset: offset + point(Pixels::ZERO, rem * input_height),
                                }));
                            }
                            cx.new(|_| CardDragPreview {
                                title: String::from_utf16_lossy(&drag.wire.node.identity_name()),
                                width: rem * CARD_WIDTH,
                                height: body_height,
                            })
                        })
                        .when(card.receiver.is_none(), |body| {
                            body.on_drop::<CardDrag>(cx.listener(move |view, drag, window, cx| {
                                view.canvas_drop_on_card(drag, &drag_node, window, cx)
                            }))
                        })
                })
                .child(
                    div()
                        .absolute()
                        .inset(px(1.))
                        .overflow_hidden()
                        .rounded_sm()
                        .child(self.render_node_preview(
                            &card.node,
                            &title,
                            body_height - px(4.),
                            cx,
                        )),
                )
                .child(header);
            if is_group {
                let group_key = card.key.clone();
                node = node.child(
                    card_button(format!("graph-enter-group-{}", card.id), rem, 1.75)
                        .absolute()
                        .bottom(rem * if card.receiver.is_some() { 2.1 } else { 0.35 })
                        .left(rem * 0.35)
                        .outline()
                        .label("Enter group")
                        .disabled(busy)
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.canvas_enter_group(&group_key, revision, serial, window, cx);
                        })),
                );
            }
            // Saved branch positions can overlap another card's face.
            // Keep wire handles above card art so those outputs remain draggable.
            output_handles.push(output);
            let mut sockets = Vec::new();
            for socket in card
                .inputs
                .sockets
                .iter()
                .filter(|socket| socket.child.is_none() && !card.collapsed)
            {
                let Some(row) = self
                    .navigation
                    .rows
                    .iter()
                    .find(|row| row.key == socket.key)
                else {
                    continue;
                };
                let empty = row.target.node().is_none();
                let name = row
                    .label
                    .split_once(": ")
                    .map_or(row.label.as_str(), |(name, _)| name);
                let state = if empty { "Empty" } else { "Connected" };
                let label = if empty {
                    format!("{name}: Empty")
                } else {
                    row.label.clone()
                };
                let port_content = div()
                    .v_flex()
                    .items_center()
                    .justify_center()
                    .w_full()
                    .min_w_0()
                    .gap(rem * 0.25)
                    .when(empty, |content| {
                        content.child(
                            div()
                                .size(rem * 1.5)
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .border_1()
                                .border_color(theme.muted_foreground)
                                .text_color(theme.muted_foreground)
                                .child("+"),
                        )
                    })
                    .child(
                        div()
                            .w_full()
                            .line_clamp(2)
                            .text_center()
                            .font_semibold()
                            .child(name.to_owned()),
                    )
                    .child(div().text_color(theme.muted_foreground).child(state));
                let drop_border = theme.primary;
                let drop_background = theme.accent;
                let port = card_button(
                    format!("graph-port-{}", socket.key.replace('/', "-")),
                    rem,
                    CARD_WIDTH,
                )
                .w(rem * CARD_WIDTH)
                .outline()
                .rounded(gpui_kit::component::button::ButtonRounded::Small)
                .bg(theme.muted)
                .accessibility_label(label.clone())
                .tooltip(label)
                .tab_index(0)
                .child(port_content)
                .disabled(busy)
                .drag_over::<CardDrag>(move |style, drag, _, _| {
                    if library_drop_ready
                        && drag.wire.revision == revision
                        && drag.wire.selection_serial == serial
                    {
                        style.bg(drop_background).border_color(drop_border)
                    } else {
                        style
                    }
                })
                .drag_over::<LibraryNodeDrag>(move |style, drag, _, _| {
                    if library_drop_ready
                        && drag.owner == document_id
                        && drag.revision == revision
                        && drag.selection_serial == serial
                    {
                        style.bg(drop_background).border_color(drop_border)
                    } else {
                        style
                    }
                });
                let port = div()
                    .size_full()
                    .child(self.canvas_socket_menu(port, row, cx))
                    .child(
                        div()
                            .absolute()
                            .left(gpui_kit::relative(0.5))
                            .ml(rem * -0.15)
                            .bottom(rem * -0.15)
                            .size(rem * 0.3)
                            .rounded_full()
                            .bg(theme.muted_foreground),
                    );
                sockets.push(div().absolute().top_0().left(rem * socket.left).child(
                    self.canvas_socket_target(
                        &socket.key,
                        port.into_any_element(),
                        frame.clone(),
                        card_id,
                        rem,
                        cx,
                    ),
                ));
            }
            let body = if let Some(receiver) = card
                .receiver
                .as_ref()
                .and_then(|key| visible_card_ids.get(key))
                && let Some(row) = self.navigation.rows.iter().find(|row| row.key == card.key)
            {
                let name = row
                    .label
                    .split_once(": ")
                    .map_or(row.label.as_str(), |(name, _)| name);
                let label = self.canvas_socket_menu(
                    card_button(
                        format!("graph-port-{}", card.key.replace('/', "-")),
                        rem,
                        1.45,
                    )
                    .max_w(rem * (CARD_WIDTH - 0.7))
                    .outline()
                    .label(name.to_owned())
                    .accessibility_label(row.label.clone())
                    .tooltip(row.label.clone())
                    .tab_index(0)
                    .disabled(busy),
                    row,
                    cx,
                );
                node = node.child(
                    div()
                        .absolute()
                        .bottom(rem * 0.35)
                        .left(rem * 0.35)
                        .child(label),
                );
                self.canvas_socket_target(
                    &card.key,
                    node.into_any_element(),
                    frame.clone(),
                    *receiver,
                    rem,
                    cx,
                )
            } else {
                node.into_any_element()
            };
            let mut stack = div()
                .id(("graph-card", card.id as usize))
                .test_support()
                .absolute()
                .left(top_left.x)
                .top(top_left.y)
                .w(rem * CARD_WIDTH)
                .v_flex()
                .on_prepaint(move |bounds, _, _| card_origin.set(bounds.origin));
            if input_height > 0. {
                stack = stack
                    .children(sockets)
                    .child(div().h(rem * input_height).flex_none())
                    .child(
                        div()
                            .absolute()
                            .top(rem * (input_height - 0.85))
                            .left(gpui_kit::relative(0.5))
                            .ml(rem * -0.2)
                            .size(rem * 0.4)
                            .rounded_full()
                            .bg(theme.muted_foreground),
                    );
            }
            surface = surface.child(stack.child(body));
        }
        surface = surface.children(output_handles);
        self.update_node_preview_scope(preview_nodes, cx);
        let graph_viewport = div()
            .relative()
            .flex_1()
            .h_full()
            .min_w_0()
            .min_h_0()
            .child(
                div()
                    .id("graph-canvas-scroll")
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .overflow_scroll()
                    .track_scroll(&self.canvas.scroll)
                    .on_prepaint({
                        let owner = cx.weak_entity();
                        move |bounds, window, cx| {
                            let _ = owner.update(cx, |view, cx| {
                                view.measure_canvas_viewport(bounds.size, window, cx);
                            });
                        }
                    })
                    .test_support()
                    .when_some(focus.clone(), |scroll, focus| {
                        scroll.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            focus.focus(window, cx);
                        })
                    })
                    .when(self.canvas.pan.dragging(), |scroll| {
                        scroll.cursor_grabbing()
                    })
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::begin_canvas_pan))
                    .on_mouse_move(cx.listener(Self::move_canvas_pan))
                    .on_mouse_up(MouseButton::Right, cx.listener(Self::end_canvas_pan))
                    .on_mouse_up_out(MouseButton::Right, cx.listener(Self::end_canvas_pan))
                    .on_drop::<CardDrag>(cx.listener(|view, drag, window, cx| {
                        view.canvas_place_card(drag, window, cx)
                    }))
                    .on_drop::<LibraryNodeDrag>(cx.listener(Self::canvas_drop_library_node))
                    .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
                    .child(surface),
            )
            .child(
                div()
                    .id("graph-canvas-scrollbars")
                    .absolute()
                    .inset_0()
                    .test_support()
                    .scrollbar(
                        &self.canvas.scroll,
                        gpui_kit::component::scroll::ScrollbarAxis::Both,
                    ),
            )
            .when(self.canvas.group().is_none(), |container| {
                container.child(
                    div()
                        .absolute()
                        .left(rems(2.))
                        .top(rems(0.25))
                        .child(document_input),
                )
            })
            .when(self.canvas.pan.overview && measured, |container| {
                container.child(self.canvas_minimap(&viewport, window.rem_size(), cx))
            });
        div()
            .id("graph-canvas")
            .v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .border_1()
            .border_color(theme.transparent)
            .key_context("GraphCanvas")
            .when_some(focus.clone(), |graph, focus| {
                graph
                    .track_focus(&focus)
                    .focus_visible(|style| style.border_color(theme.ring))
            })
            .on_action(cx.listener(|view, _: &ZoomIn, window, cx| {
                view.step_canvas_zoom(ZOOM_STEP, None, window, cx);
            }))
            .on_action(cx.listener(|view, _: &ZoomOut, window, cx| {
                view.step_canvas_zoom(-ZOOM_STEP, None, window, cx);
            }))
            .on_action(cx.listener(|view, _: &ResetZoom, window, cx| {
                view.change_canvas_zoom(Zoom::NORMAL, None, window, cx);
            }))
            .on_action(cx.listener(|view, _: &FitGraph, window, cx| view.fit_canvas(window, cx)))
            .on_action(cx.listener(|view, _: &ClosePan, _, cx| {
                if view.canvas.pan.overview {
                    view.toggle_canvas_overview(cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|view, _: &PanLeft, window, cx| {
                view.keyboard_canvas_pan([1., 0.], window, cx);
            }))
            .on_action(cx.listener(|view, _: &PanRight, window, cx| {
                view.keyboard_canvas_pan([-1., 0.], window, cx);
            }))
            .on_action(cx.listener(|view, _: &PanUp, window, cx| {
                view.keyboard_canvas_pan([0., 1.], window, cx);
            }))
            .on_action(cx.listener(|view, _: &PanDown, window, cx| {
                view.keyboard_canvas_pan([0., -1.], window, cx);
            }))
            .child(self.canvas_view_controls(busy, cx))
            .when(top_level && root.is_none(), |graph| {
                graph.child(
                    div()
                        .px_3()
                        .pb_2()
                        .child(self.texture_output_terminal(None, window_rem, cx)),
                )
            })
            .when(!self.canvas.groups.is_empty(), |view| {
                let current = self.canvas.groups.len();
                view.child(
                    div()
                        .id("graph-group-path")
                        .h_flex()
                        .flex_wrap()
                        .items_center()
                        .gap_1()
                        .px_3()
                        .pt_2()
                        .children(
                            std::iter::once("Document".to_owned())
                                .chain(self.canvas.groups.iter().map(group_name))
                                .enumerate()
                                .map(|(depth, name)| {
                                    if depth == current {
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .child(name)
                                            .into_any_element()
                                    } else {
                                        Button::new(format!("graph-group-level-{depth}"))
                                            .small()
                                            .label(name)
                                            .disabled(busy)
                                            .on_click(cx.listener(move |view, _, window, cx| {
                                                view.canvas_leave_group(depth, window, cx)
                                            }))
                                            .into_any_element()
                                    }
                                }),
                        ),
                )
            })
            .when(!input_bar.is_empty(), |this| {
                this.child(
                    div()
                        .h_flex()
                        .flex_wrap()
                        .gap_2()
                        .px_3()
                        .pb_2()
                        .text_sm()
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child("Group inputs"),
                        )
                        .children(input_bar.into_iter().map(|input| {
                            let label = input.label.clone();
                            let chip = div()
                                .id(format!("graph-group-input-{}", input.index + 1))
                                .test_support()
                                .aria_label(label.clone())
                                .h_flex()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(theme.muted)
                                .child(self.render_group_input_preview(
                                    &input.source,
                                    input.index,
                                    cx,
                                ))
                                .child(label);
                            if busy {
                                chip
                            } else {
                                chip.cursor_grab().on_drag(input, |input, _, _, cx| {
                                    cx.new(|_| GraphDragLabel(input.label.clone()))
                                })
                            }
                        })),
                )
            })
            .child(
                div()
                    .h_flex()
                    .items_start()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(graph_viewport)
                    .child(
                        div()
                            .id("graph-node-library")
                            .test_support()
                            .v_flex()
                            .w(library_width)
                            .h_full()
                            .min_h_0()
                            .flex_shrink_0()
                            .gap_2()
                            .p_3()
                            .border_l_1()
                            .border_color(theme.border)
                            .child(div().font_semibold().text_sm().child("Node library"))
                            .child(self.render_node_library(busy, cx))
                            .child(div().mt_3().font_semibold().text_sm().child("Branches"))
                            .child(branch_picker),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod ownership_laws {
    use super::*;
    use mycela_engine::{Gtx375GraphDocument, PointwiseNodeChangeHandler};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Probe(Arc<AtomicUsize>);
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[test]
    fn derived_cards_retire_real_owners_and_arrange_preserves_domain_ids() {
        let root = PointwiseNode::<ForegroundError>::invert();
        let held = root.slot(0).unwrap();
        let drops = Arc::new(AtomicUsize::new(0));
        let witness = Arc::new(Probe(drops.clone()));
        let weak = Arc::downgrade(&witness);
        root.add_change_handler(PointwiseNodeChangeHandler::new(move |_, _| {
            let _ = &witness;
            Ok(())
        }));
        let document = Arc::new(Mutex::new(
            Gtx375GraphDocument::new(vec![Some(root.clone())]).unwrap(),
        ));
        let mut nav = Navigation::default();
        nav.rebuild(&document);
        let mut canvas = CanvasState::default();
        canvas.rebuild(&nav);
        let id = canvas.cards[0].id;
        canvas.arrange(&nav);
        assert_eq!(canvas.cards[0].id, id);
        guard(&document)
            .set_branch(0, Some(PointwiseNode::invert()))
            .unwrap();
        drop(root);
        nav.rebuild(&document);
        canvas.rebuild(&nav);
        assert_ne!(canvas.cards[0].id, id);
        assert!(weak.upgrade().is_some());
        drop(held);
        assert!(weak.upgrade().is_none());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn admission_reads_real_topology_and_refuses_existing_or_new_cycles() {
        let a = PointwiseNode::<ForegroundError>::invert();
        let b = PointwiseNode::<ForegroundError>::invert();
        a.slot(0).unwrap().set_node_quiet(Some(&b)).unwrap();
        assert!(admits(&a, &Target::Slot(b.slot(0).unwrap()), 0).is_err());
        assert!(b.slot(0).unwrap().node().is_none());
        b.slot(0).unwrap().set_node_quiet(Some(&a)).unwrap();
        assert!(admits(&a, &Target::Branch(0, None), 0).is_err());
    }

    #[test]
    fn group_levels_restore_their_layout_and_retire_removed_cards() {
        use mycela_engine::{PointwiseFilter, PointwiseKind};

        let group = PointwiseFilter::<ForegroundError>::new(PointwiseKind::Group)
            .unwrap()
            .node();
        let content = group.content(0).unwrap();
        let inner = PointwiseNode::invert();
        let outer = PointwiseNode::invert();
        content.set_node_quiet(Some(&inner)).unwrap();
        let document = Arc::new(Mutex::new(
            Gtx375GraphDocument::new(vec![Some(group.clone()), Some(outer.clone())]).unwrap(),
        ));
        let mut navigation = Navigation::default();
        navigation.rebuild(&document);
        let mut canvas = CanvasState::default();
        canvas.rebuild(&navigation);
        let root_id = canvas.card_id_for_test(&outer);
        let root_card = canvas
            .cards
            .iter_mut()
            .find(|card| Some(card.id) == root_id)
            .unwrap();
        let manual_offset = [48., 19.];
        root_card.manual_offset = manual_offset;
        let root_offset = point(-gpui_kit::px(180.), -gpui_kit::px(100.));
        canvas.scroll.set_offset(root_offset);
        canvas.set_level(vec![group.clone()], &navigation);
        let inner_id = canvas.card_id_for_test(&inner);
        assert_ne!(
            inner_id,
            canvas.inactive_layouts[0]
                .cards
                .iter()
                .find(|card| card.node.same_reference(&inner))
                .map(|card| card.id)
        );
        let group_offset = point(-gpui_kit::px(50.), -gpui_kit::px(25.));
        canvas.scroll.set_offset(group_offset);
        canvas.set_level(Vec::new(), &navigation);
        assert_eq!(canvas.card_id_for_test(&outer), root_id);
        assert_eq!(
            canvas
                .cards
                .iter()
                .find(|card| Some(card.id) == root_id)
                .unwrap()
                .manual_offset,
            manual_offset,
        );
        assert_eq!(canvas.scroll.offset(), root_offset);
        canvas.set_level(vec![group.clone()], &navigation);
        assert_eq!(canvas.card_id_for_test(&inner), inner_id);
        assert_eq!(canvas.scroll.offset(), group_offset);
        canvas.set_level(Vec::new(), &navigation);
        content.set_node_quiet(None).unwrap();
        navigation.rebuild(&document);
        canvas.rebuild(&navigation);
        assert!(canvas.inactive_layouts[0].cards.is_empty());

        canvas.set_level(vec![group], &navigation);
        guard(&document).set_branch(0, None).unwrap();
        navigation.rebuild(&document);
        canvas.rebuild(&navigation);
        assert!(canvas.group().is_none());
        assert!(canvas.inactive_layouts.is_empty());
        assert_eq!(canvas.scroll.offset(), root_offset);
    }

    #[test]
    fn group_card_shows_active_and_occupied_input_ports() {
        use mycela_engine::{PointwiseFilter, PointwiseKind};

        let group = PointwiseFilter::<ForegroundError>::new(PointwiseKind::Group).unwrap();
        let node = group.node();
        let first = node.slot(0).unwrap();
        let second = node.slot(1).unwrap();
        let retained = node.slot(6).unwrap();
        let contents = node.content(0).unwrap();
        retained.set_node_quiet(Some(&Node::invert())).unwrap();
        let assert_ports = |expected: &[_]| {
            let actual = canvas_ports(&node);
            assert_eq!(actual.len(), expected.len());
            assert!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| actual.same_reference(expected))
            );
        };
        assert_ports(&[first.clone(), retained.clone(), contents.clone()]);
        group.parameter(0).unwrap().set_number(2.).unwrap();
        assert_ports(&[first, second, retained, contents]);

        let ordinary = Node::invert();
        assert_eq!(canvas_ports(&ordinary).len(), 1);
    }

    #[test]
    fn sharing_has_one_card_and_each_actual_occurrence_remains_distinct() {
        let parent = mycela_engine::PointwiseFilter::<ForegroundError>::new(
            mycela_engine::PointwiseKind::Add,
        )
        .unwrap()
        .node();
        let child = PointwiseNode::solid_color().unwrap();
        parent
            .slot(0)
            .unwrap()
            .set_node_quiet(Some(&child))
            .unwrap();
        parent
            .slot(1)
            .unwrap()
            .set_node_quiet(Some(&child))
            .unwrap();
        let document = Arc::new(Mutex::new(
            Gtx375GraphDocument::new(vec![Some(parent.clone())]).unwrap(),
        ));
        let mut nav = Navigation::default();
        nav.rebuild(&document);
        let mut canvas = CanvasState::default();
        canvas.rebuild(&nav);
        assert_eq!(canvas.cards.len(), 2);
        let slots = parent.slots();
        assert!(!slots[0].same_reference(&slots[1]));
        for slot in slots {
            assert!(
                nav.rows
                    .iter()
                    .any(|r| matches!(&r.target,Target::Slot(s)if s.same_reference(&slot)))
            );
        }
    }
}
