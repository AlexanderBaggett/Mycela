//! Coordinates for the active graph level. The existing ScrollHandle owns the offset.
use super::*;
use gpui_kit::{
    BorderStyle, Bounds, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Point, ScrollDelta,
    ScrollWheelEvent, Size, component::slider::Slider, fill, outline, size,
};
use std::cell::Cell;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Zoom(f32);

impl Zoom {
    pub(super) const MIN: Self = Self(0.5);
    pub(super) const MAX: Self = Self(2.);
    pub(super) const NORMAL: Self = Self(1.);

    pub(super) fn new(value: f32) -> Self {
        Self(value.clamp(Self::MIN.0, Self::MAX.0))
    }

    pub(super) fn factor(self) -> f32 {
        self.0
    }

    pub(super) fn percent(self) -> f32 {
        self.0 * 100.
    }
}

impl Default for Zoom {
    fn default() -> Self {
        Self::NORMAL
    }
}

pub(super) struct GraphBounds {
    min: [f32; 2],
    max: [f32; 2],
}

impl GraphBounds {
    pub(super) fn for_cards(cards: &[Card]) -> Self {
        let mut bounds = Self {
            min: [f32::INFINITY; 2],
            max: [f32::NEG_INFINITY; 2],
        };
        for card in cards.iter().filter(|card| !card.hidden) {
            let ([x, y], width) = card.extent();
            bounds.min[0] = bounds.min[0].min(x);
            bounds.min[1] = bounds.min[1].min(y);
            bounds.max[0] = bounds.max[0].max(x + width);
            bounds.max[1] = bounds.max[1].max(y + card.height());
        }
        if !bounds.min[0].is_finite() {
            return Self {
                min: [0.; 2],
                max: [CARD_WIDTH, 3.],
            };
        }
        bounds
    }

    pub(super) fn fit(&self, size: Size<Pixels>, rem: Pixels) -> Zoom {
        let [width, height] = self.padded_span();
        Zoom::new((size.width / (rem * width)).min(size.height / (rem * height)))
    }

    fn padded_span(&self) -> [f32; 2] {
        std::array::from_fn(|index| self.max[index] - self.min[index] + 2. * CANVAS_MARGIN)
    }
}

pub(super) struct Viewport {
    size: Size<Pixels>,
    pub(super) surface: Size<Pixels>,
    origin: Point<Pixels>,
    pub(super) rem: Pixels,
}

impl Viewport {
    pub(super) fn new(
        bounds: &GraphBounds,
        size: Size<Pixels>,
        base_rem: Pixels,
        zoom: Zoom,
    ) -> Self {
        let rem = base_rem * zoom.factor();
        let [width, height] = bounds.padded_span();
        let surface = gpui_kit::size(size.width.max(rem * width), size.height.max(rem * height));
        let origin = point(
            surface.width / 2. - rem * ((bounds.max[0] + bounds.min[0]) / 2.),
            surface.height / 2. - rem * ((bounds.max[1] + bounds.min[1]) / 2.),
        );
        Self {
            size,
            surface,
            origin,
            rem,
        }
    }

    pub(super) fn surface_point(&self, [x, y]: [f32; 2]) -> Point<Pixels> {
        self.origin + point(self.rem * x, self.rem * y)
    }

    pub(super) fn world_point(&self, cursor: Point<Pixels>, offset: Point<Pixels>) -> [f32; 2] {
        let point = cursor - self.origin - offset;
        [point.x / self.rem, point.y / self.rem]
    }

    pub(super) fn center(&self) -> Point<Pixels> {
        point(self.size.width / 2., self.size.height / 2.)
    }

    pub(super) fn place_at(&self, world: [f32; 2], screen: Point<Pixels>) -> Point<Pixels> {
        self.clamp_offset(screen - self.surface_point(world))
    }

    pub(super) fn padding_for(&self, world: [f32; 2], screen: Point<Pixels>) -> Point<Pixels> {
        let requested = screen - self.surface_point(world);
        let overflow = requested - self.clamp_offset(requested);
        point(
            overflow.x.abs().min(self.size.width * 2.),
            overflow.y.abs().min(self.size.height * 2.),
        )
    }

    pub(super) fn with_padding(mut self, padding: Point<Pixels>) -> Self {
        // Symmetric room leaves Fit's graph center in the center of the surface.
        self.surface.width += padding.x * 2.;
        self.surface.height += padding.y * 2.;
        self.origin += padding;
        self
    }

    fn clamp_offset(&self, offset: Point<Pixels>) -> Point<Pixels> {
        point(
            offset
                .x
                .clamp(self.size.width - self.surface.width, Pixels::ZERO),
            offset
                .y
                .clamp(self.size.height - self.surface.height, Pixels::ZERO),
        )
    }

    pub(super) fn fit_offset(&self) -> Point<Pixels> {
        point(
            (self.size.width - self.surface.width) / 2.,
            (self.size.height - self.surface.height) / 2.,
        )
    }

    pub(super) fn card_bounds(&self, card: &Card, offset: Point<Pixels>) -> Bounds<Pixels> {
        let (origin, width) = card.extent();
        Bounds::new(
            self.surface_point(origin) + offset,
            size(self.rem * width, self.rem * card.height()),
        )
    }

    pub(super) fn card_visible(&self, card: &Card, offset: Point<Pixels>) -> bool {
        let bounds = self.card_bounds(card, offset);
        bounds.right() >= Pixels::ZERO
            && bounds.bottom() >= Pixels::ZERO
            && bounds.left() <= self.size.width
            && bounds.top() <= self.size.height
    }
}

#[derive(Default)]
pub(super) struct PanState {
    pub(super) overview: bool,
    drag: Option<PanDrag>,
    overview_pointer_down: bool,
}

impl PanState {
    pub(super) fn dragging(&self) -> bool {
        self.drag.is_some_and(|drag| drag.active)
    }

    pub(super) fn stop_pointer_gestures(&mut self) {
        self.drag = None;
        self.overview_pointer_down = false;
    }
}

#[derive(Clone, Copy)]
struct PanDrag {
    start: Point<Pixels>,
    offset: Point<Pixels>,
    active: bool,
}

#[derive(Clone, Copy)]
struct OverviewMap {
    origin: Point<Pixels>,
    scale: f32,
}

impl OverviewMap {
    fn new(bounds: Bounds<Pixels>, surface: Size<Pixels>) -> Option<Self> {
        let scale = (bounds.size.width / surface.width).min(bounds.size.height / surface.height);
        if scale <= 0. {
            return None;
        }
        let origin = bounds.center() - point(surface.width, surface.height) * (scale / 2.);
        Some(Self { origin, scale })
    }

    fn map(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        Bounds::new(
            self.origin + bounds.origin * self.scale,
            size(
                bounds.size.width * self.scale,
                bounds.size.height * self.scale,
            ),
        )
    }

    fn graph_point(&self, position: Point<Pixels>) -> Point<Pixels> {
        (position - self.origin) / self.scale
    }
}

impl GraphDocumentView {
    pub(super) fn measure_canvas_viewport(
        &mut self,
        size: Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.canvas.viewport_size != size {
            self.canvas.viewport_size = size;
            if self.canvas.has_viewport() && std::mem::take(&mut self.canvas.center_on_measure) {
                self.canvas_center(window);
            }
            cx.notify();
        }
        if let Some(slider) = &self.canvas.zoom_slider {
            let percent = self.canvas.zoom.percent();
            if (slider.read(cx).value().start() - percent).abs() > 0.001 {
                slider.update(cx, |slider, cx| slider.set_value(percent, window, cx));
            }
        }
    }

    pub(super) fn wheel_canvas_zoom(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let step = match event.delta {
            ScrollDelta::Lines(lines) => lines.y.clamp(-1., 1.),
            ScrollDelta::Pixels(delta) => (delta.y.as_f32() / 60.).clamp(-1., 1.),
        };
        if step == 0. || !self.canvas.has_viewport() {
            return;
        }
        cx.stop_propagation();
        if !self.canvas.pan.dragging() {
            let cursor = event.position - self.canvas.scroll.bounds().origin;
            self.step_canvas_zoom(step * ZOOM_STEP, Some(cursor), window, cx);
        }
    }

    pub(super) fn begin_canvas_pan(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.canvas.pan.drag = Some(PanDrag {
            start: event.position,
            offset: self.canvas.scroll.offset(),
            active: false,
        });
    }

    pub(super) fn move_canvas_pan(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut drag) = self.canvas.pan.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Right) {
            self.canvas.pan.drag = None;
            if drag.active {
                cx.notify();
            }
            return;
        }
        let delta = event.position - drag.start;
        if !drag.active && delta.x.abs().max(delta.y.abs()) < px(5.) {
            return;
        }
        if !drag.active {
            drag.active = true;
            self.canvas.pan.drag = Some(drag);
            if let Some(focus) = &self.canvas.focus {
                focus.focus(window, cx);
            }
            cx.notify();
        }
        cx.stop_propagation();
        self.pan_canvas_to(drag.offset + delta, window, cx);
    }

    pub(super) fn end_canvas_pan(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.canvas.pan.drag.take().is_some_and(|drag| drag.active) {
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(super) fn keyboard_canvas_pan(
        &mut self,
        [x, y]: [f32; 2],
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = self.canvas.viewport_size;
        let step = point(viewport.width * x / 4., viewport.height * y / 4.);
        self.pan_canvas_to(self.canvas.scroll.offset() + step, window, cx);
    }

    pub(super) fn pan_canvas_to(
        &mut self,
        offset: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let next = self.canvas.viewport(window.rem_size()).clamp_offset(offset);
        if self.canvas.scroll.offset() != next {
            self.canvas.scroll.set_offset(next);
            cx.notify();
        }
    }

    pub(super) fn toggle_canvas_overview(&mut self, cx: &mut Context<Self>) {
        self.canvas.pan.overview = !self.canvas.pan.overview;
        self.canvas.pan.overview_pointer_down = false;
        cx.notify();
    }

    pub(super) fn canvas_view_controls(&self, busy: bool, cx: &Context<Self>) -> impl IntoElement {
        let zoom = self.canvas.zoom;
        let live = self.canvas.has_viewport();
        let empty = self.canvas.cards.is_empty();
        let group_level = self.canvas.groups.len();
        div()
            .id("graph-canvas-view-controls")
            .h_flex()
            .flex_wrap()
            .gap_2()
            .px_3()
            .py_2()
            .test_support()
            .when_some(self.canvas.zoom_slider.as_ref(), |toolbar, slider| {
                toolbar.child(div().text_sm().child("Zoom")).child(
                    div()
                        .id("graph-canvas-zoom-slider")
                        .test_support()
                        .role(gpui_kit::Role::Group)
                        .aria_label("Graph zoom")
                        .w(rems(7.5))
                        .child(Slider::new(slider).w_full().disabled(!live)),
                )
            })
            .child(
                Button::new("graph-canvas-zoom-reset")
                    .small()
                    .ghost()
                    .tab_index(0)
                    .label(format!("{:.0}%", zoom.percent()))
                    .accessibility_label(format!(
                        "Graph zoom {:.0}%. Reset to 100%",
                        zoom.percent()
                    ))
                    .tooltip("Reset graph zoom to 100%")
                    .disabled(!live)
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.change_canvas_zoom(Zoom::NORMAL, None, window, cx);
                    })),
            )
            .child(
                Button::new("graph-canvas-fit")
                    .small()
                    .tab_index(0)
                    .label("Fit")
                    .tooltip("Fit graph to view")
                    .disabled(!live || empty)
                    .on_click(cx.listener(|view, _, window, cx| view.fit_canvas(window, cx))),
            )
            .child(
                Button::new("graph-canvas-pan")
                    .small()
                    .tab_index(0)
                    .label("Pan")
                    .tooltip("Show the graph overview")
                    .toggled(self.canvas.pan.overview)
                    .disabled(!live || empty)
                    .on_click(cx.listener(|view, _, _, cx| view.toggle_canvas_overview(cx))),
            )
            .child(
                Button::new("graph-group-out")
                    .small()
                    .tab_index(0)
                    .label("Out")
                    .tooltip("Leave this Group")
                    .disabled(busy || group_level == 0)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.canvas_leave_group(group_level.saturating_sub(1), window, cx);
                    })),
            )
    }

    pub(super) fn canvas_minimap(
        &self,
        viewport: &Viewport,
        rem: Pixels,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let surface = viewport.surface;
        let inset = rem * 0.75;
        let width = (viewport.size.width - inset * 2.).clamp(Pixels::ZERO, rem * 13.);
        let height = (viewport.size.height - inset * 2.).clamp(Pixels::ZERO, rem * 8.);
        let selected = self.selected_node();
        let cards: Vec<_> = self
            .canvas
            .visible_cards()
            .map(|card| {
                let bounds = viewport.card_bounds(card, Point::default());
                let selected = selected
                    .as_ref()
                    .is_some_and(|node| node.same_reference(&card.node));
                (bounds, selected)
            })
            .collect();
        let offset = self.canvas.scroll.offset();
        let shown = Bounds::new(point(-offset.x, -offset.y), self.canvas.viewport_size);
        let (text, accent, border, background) = (
            cx.theme().muted_foreground,
            cx.theme().primary,
            cx.theme().border,
            cx.theme().popover,
        );
        let map = Rc::new(Cell::new(None));
        let measured = map.clone();
        let pressed = map.clone();
        let guide = canvas(
            move |bounds, _, _| {
                let mapping = OverviewMap::new(bounds, surface);
                measured.set(mapping);
                mapping
            },
            move |_, mapping, window, _| {
                if let Some(mapping) = mapping {
                    for (card, selected) in &cards {
                        let color = if *selected { accent } else { text };
                        window.paint_quad(fill(mapping.map(*card), color));
                    }
                    window.paint_quad(outline(mapping.map(shown), accent, BorderStyle::Solid));
                }
            },
        )
        .size_full();
        div()
            .id("graph-canvas-overview")
            .test_support()
            .aria_label("Graph overview. Click or drag to pan.")
            .absolute()
            .right(inset)
            .bottom(inset)
            .w(width)
            .h(height)
            .p_2()
            .rounded_md()
            .bg(background)
            .border_1()
            .border_color(border)
            .cursor_move()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    if let Some(mapping) = pressed.get() {
                        if let Some(focus) = &view.canvas.focus {
                            focus.focus(window, cx);
                        }
                        view.canvas.pan.overview_pointer_down = true;
                        view.pan_canvas_from_minimap(mapping, event.position, window, cx);
                    }
                }),
            )
            .on_mouse_move(
                cx.listener(move |view, event: &MouseMoveEvent, window, cx| {
                    if view.canvas.pan.overview_pointer_down
                        && event.pressed_button == Some(MouseButton::Left)
                        && let Some(mapping) = map.get()
                    {
                        view.pan_canvas_from_minimap(mapping, event.position, window, cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _: &MouseUpEvent, _, _| {
                    view.canvas.pan.overview_pointer_down = false;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _: &MouseUpEvent, _, _| {
                    view.canvas.pan.overview_pointer_down = false;
                }),
            )
            .child(guide)
    }

    fn pan_canvas_from_minimap(
        &mut self,
        mapping: OverviewMap,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let size = self.canvas.viewport_size;
        let center = point(size.width / 2., size.height / 2.);
        self.pan_canvas_to(center - mapping.graph_point(position), window, cx);
    }
}
