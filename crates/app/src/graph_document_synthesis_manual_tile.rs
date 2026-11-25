//! Direct manipulation on the square Manual authoring tile.
use super::*;
use mycela_engine::{SynthesisPatchResizeHandle, SynthesisPlacementTransform};

const HANDLES: [SynthesisPatchResizeHandle; 8] = {
    use SynthesisPatchResizeHandle::*;
    [
        NorthWest, North, NorthEast, East, SouthEast, South, SouthWest, West,
    ]
};

#[derive(Clone, Copy)]
enum Part {
    Move,
    Resize(SynthesisPatchResizeHandle),
    Rotate,
}

impl Part {
    fn cursor(self, dragging: bool) -> CursorStyle {
        use SynthesisPatchResizeHandle::*;
        match self {
            Self::Move if dragging => CursorStyle::ClosedHand,
            Self::Move => CursorStyle::OpenHand,
            Self::Rotate => CursorStyle::Crosshair,
            Self::Resize(North | South) => CursorStyle::ResizeUpDown,
            Self::Resize(East | West) => CursorStyle::ResizeLeftRight,
            Self::Resize(NorthEast | SouthWest) => CursorStyle::ResizeUpRightDownLeft,
            Self::Resize(NorthWest | SouthEast) => CursorStyle::ResizeUpLeftDownRight,
        }
    }
}

#[derive(Clone, Copy)]
struct Hit {
    index: usize,
    part: Part,
    offset: [f32; 2],
}

#[derive(Clone, Copy)]
pub(super) struct TileDrag {
    hit: Hit,
    space: Bounds<Pixels>,
    start: Point<Pixels>,
    pointer: Point<Pixels>,
    modifiers: Modifiers,
}

impl TileDrag {
    pub(super) fn index(self) -> usize {
        self.hit.index
    }

    pub(super) fn track(&mut self, pointer: Point<Pixels>, modifiers: Modifiers) {
        self.pointer = pointer;
        self.modifiers = modifiers;
    }

    pub(super) fn moved(self) -> bool {
        let distance = self.pointer - self.start;
        distance.x.abs().max(distance.y.abs()) >= gpui_kit::px(5.)
    }

    pub(super) fn transform(self, outside: bool) -> SynthesisPlacementTransform {
        let delta = self.pointer - self.start;
        let delta = [
            delta.x / self.space.size.width,
            delta.y / self.space.size.height,
        ];
        match self.hit.part {
            Part::Move => SynthesisPlacementTransform::Move {
                delta: shift(delta, self.hit.offset),
                wrap: outside,
            },
            Part::Resize(handle) => SynthesisPlacementTransform::Resize {
                handle,
                delta,
                preserve_aspect: self.modifiers.shift,
                from_center: self.modifiers.control,
            },
            Part::Rotate => SynthesisPlacementTransform::Rotate {
                pointer: shift(
                    normalized(self.space, self.pointer),
                    self.hit.offset.map(|axis| -axis),
                ),
            },
        }
    }

    pub(super) fn project(
        self,
        edit: &SynthesisEdit,
        layer: usize,
        source: ImageGeometry,
    ) -> Option<SynthesisPlacementGeometry> {
        if self.moved() {
            edit.placement_for_drag(layer, self.index(), self.transform(false), source)
        } else {
            edit.placement_geometry(layer, self.index())
        }
    }
}

pub(super) enum TileEvent {
    Begin(Option<TileDrag>),
    Track(Point<Pixels>, Modifiers, Option<CursorStyle>),
    Release(Point<Pixels>, Modifiers, bool),
    Cancel,
}

struct Outline {
    index: usize,
    points: Vec<[f32; 2]>,
    bounds: [[f32; 2]; 2],
}

impl Outline {
    fn new(index: usize, points: Vec<[f32; 2]>) -> Option<Self> {
        let &first = points.first()?;
        let bounds = points.iter().fold([first; 2], |[min, max], point| {
            [
                [min[0].min(point[0]), min[1].min(point[1])],
                [max[0].max(point[0]), max[1].max(point[1])],
            ]
        });
        Some(Self {
            index,
            points,
            bounds,
        })
    }

    fn contains(&self, query: [f32; 2]) -> Option<[f32; 2]> {
        let [min, max] = self.bounds;
        // The original tests the authored contour, then its eight neighbors.
        for x in [0., -1., 1.] {
            for y in [0., -1., 1.] {
                let offset = [x, y];
                let point = [query[0] - x, query[1] - y];
                if (min[0]..=max[0]).contains(&point[0])
                    && (min[1]..=max[1]).contains(&point[1])
                    && polygon_contains(&self.points, point)
                {
                    return Some(offset);
                }
            }
        }
        None
    }
}

fn copies(min: f32, max: f32, low: f32, high: f32) -> std::ops::RangeInclusive<i32> {
    ((low - max).ceil() as i32).max(-1)..=((high - min).floor() as i32).min(1)
}

fn polygon_contains(points: &[[f32; 2]], [x, y]: [f32; 2]) -> bool {
    points
        .iter()
        .copied()
        .zip(points.iter().copied().cycle().skip(1))
        .fold(false, |inside, ([x0, y0], [x1, y1])| {
            inside ^ ((y0 > y) != (y1 > y) && x < x0 + (x1 - x0) * ((y - y0) / (y1 - y0)))
        })
}

fn normalized(space: Bounds<Pixels>, pointer: Point<Pixels>) -> [f32; 2] {
    let offset = pointer - space.origin;
    [offset.x / space.size.width, offset.y / space.size.height]
}

fn shift(left: [f32; 2], right: [f32; 2]) -> [f32; 2] {
    [left[0] + right[0], left[1] + right[1]]
}

fn wrapped_handles(
    value: SynthesisPlacementGeometry,
    source: ImageGeometry,
    space: Bounds<Pixels>,
    rem: Pixels,
) -> Vec<(Part, [f32; 2], [f32; 2])> {
    let resize = HANDLES.into_iter().filter_map(|handle| {
        value
            .resize_handle(handle, source)
            .map(|point| (Part::Resize(handle), point))
    });
    let rotate = value
        .rotation_handle(source, (rem / space.size.width) * 1.15)
        .map(|point| (Part::Rotate, point));
    resize
        .chain(rotate)
        .flat_map(|(part, [x, y])| {
            let xs = copies(x, x, 0., 1.);
            let ys = copies(y, y, 0., 1.);
            xs.flat_map(move |ox| {
                ys.clone().map(move |oy| {
                    let offset = [ox as f32, oy as f32];
                    (part, shift([x, y], offset), offset)
                })
            })
        })
        .collect()
}

fn hit_test(
    outlines: &[Outline],
    selection: Option<(usize, SynthesisPlacementGeometry)>,
    source: ImageGeometry,
    space: Bounds<Pixels>,
    position: Point<Pixels>,
    rem: Pixels,
) -> Option<Hit> {
    if !space.contains(&position) {
        return None;
    }
    if let Some((index, value)) = selection
        && let Some((part, _, offset)) = wrapped_handles(value, source, space, rem)
            .into_iter()
            .find(|(_, point, _)| {
                let delta = project(space, *point) - position;
                delta.x.abs().max(delta.y.abs()) <= rem * 0.4
            })
    {
        return Some(Hit {
            index,
            part,
            offset,
        });
    }
    let query = normalized(space, position);
    let selected = selection.map(|(index, _)| index);
    let hit = |outline: &Outline| {
        outline.contains(query).map(|offset| Hit {
            index: outline.index,
            part: Part::Move,
            offset,
        })
    };
    outlines
        .iter()
        .find(|outline| Some(outline.index) == selected)
        .and_then(hit)
        .or_else(|| {
            outlines
                .iter()
                .filter(|outline| Some(outline.index) != selected)
                .find_map(hit)
        })
}

#[derive(Default)]
pub(super) struct Tile {
    outlines: Rc<Vec<Outline>>,
}

pub(super) struct TileView<'a> {
    pub(super) inline: bool,
    pub(super) height: Pixels,
    pub(super) selected: Option<(usize, SynthesisPlacementGeometry)>,
    pub(super) projection: Option<Vec<[f32; 2]>>,
    pub(super) raster: Option<source::Raster>,
    pub(super) source: Option<ImageGeometry>,
    pub(super) navigation: &'a SourceNavigation,
    pub(super) drag: Option<TileDrag>,
    pub(super) hover: Option<CursorStyle>,
    pub(super) focus: FocusHandle,
    pub(super) disabled: bool,
}

impl Tile {
    pub(super) fn new(
        edit: &SynthesisEdit,
        layer: usize,
        source: Option<ImageGeometry>,
        rows: &[(usize, SynthesisPlacementGeometry)],
    ) -> Self {
        let outlines = source.map_or_else(Vec::new, |source| {
            rows.iter()
                .filter_map(|&(index, value)| {
                    Outline::new(index, edit.placement_outline(layer, value, source)?)
                })
                .collect()
        });
        Self {
            outlines: Rc::new(outlines),
        }
    }

    pub(super) fn render(&self, view: TileView<'_>, cx: &EditorContext) -> impl IntoElement {
        let TileView {
            inline,
            height,
            selected,
            projection,
            raster,
            source,
            navigation,
            drag,
            hover,
            focus,
            disabled,
        } = view;
        let measured = Rc::new(Cell::new(None::<GuideFrame>));
        let measure = measured.clone();
        let outlines = self.outlines.clone();
        let paint_outlines = outlines.clone();
        let view = navigation.view;
        let pan = navigation.pan.is_some();
        let theme = cx.theme();
        let (accent, foreground, background) = (theme.primary, theme.foreground, theme.background);
        let surface = canvas(
            move |bounds, window, _| {
                let rem = window.rem_size();
                let side = bounds.size.width.min(bounds.size.height) - rem;
                let frame = (side > Pixels::ZERO).then(|| {
                    let half = point(side / 2., side / 2.);
                    let fitted = Bounds::new(bounds.center() - half, size(side, side));
                    GuideFrame {
                        viewport: bounds,
                        fitted,
                        image: view.image_bounds(fitted),
                    }
                });
                measure.set(frame);
                (frame, rem)
            },
            move |bounds, (frame, rem), window, _| {
                let Some(frame) = frame else {
                    return;
                };
                let space = frame.image;
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    if let Some(raster) = &raster {
                        window
                            .paint_image(
                                space,
                                space,
                                Default::default(),
                                raster.image.clone(),
                                0,
                                false,
                            )
                            .ok();
                    }
                    window.with_content_mask(Some(ContentMask { bounds: space }), |window| {
                        let selected_index = selected.map(|(index, _)| index);
                        for outline in paint_outlines
                            .iter()
                            .rev()
                            .filter(|outline| Some(outline.index) != selected_index)
                        {
                            let [min, max] = outline.bounds;
                            if min[0] <= 1. && min[1] <= 1. && max[0] >= 0. && max[1] >= 0. {
                                let points: Vec<_> = outline
                                    .points
                                    .iter()
                                    .map(|&point| project(space, point))
                                    .collect();
                                let mut path = PathBuilder::stroke(rem / 16.);
                                path.add_polygon(&points, true);
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, foreground.opacity(0.46));
                                }
                            }
                        }
                        if let Some((index, value)) = selected
                            && let Some(source) = source
                        {
                            let projected = projection
                                .as_ref()
                                .and_then(|points| Outline::new(index, points.clone()));
                            let outline = projected.as_ref().or_else(|| {
                                paint_outlines.iter().find(|outline| outline.index == index)
                            });
                            if let Some(outline) = outline {
                                paint_outline(outline, space, rem, accent, background, window);
                            }
                            for (part, position, _) in wrapped_handles(value, source, space, rem) {
                                let center = project(space, position);
                                let side = rem * 0.46;
                                let mark = Bounds::new(
                                    center - point(side / 2., side / 2.),
                                    size(side, side),
                                );
                                window.paint_quad(fill(mark, foreground));
                                let face = mark.inset(rem / 12.);
                                window.paint_quad(fill(
                                    face,
                                    if matches!(part, Part::Rotate) {
                                        accent
                                    } else {
                                        background
                                    },
                                ));
                            }
                            if let (Some(north), Some(rotate)) = (
                                value.resize_handle(SynthesisPatchResizeHandle::North, source),
                                value.rotation_handle(source, (rem / space.size.width) * 1.15),
                            ) && [north, rotate]
                                .iter()
                                .flatten()
                                .all(|axis| (0. ..=1.).contains(axis))
                            {
                                let mut path = PathBuilder::stroke(rem / 16.);
                                path.move_to(project(space, north));
                                path.line_to(project(space, rotate));
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, accent);
                                }
                            }
                        }
                    });
                });
            },
        )
        .absolute()
        .size_full();

        let press = measured.clone();
        let movement = measured.clone();
        let pan_bounds = measured.clone();
        let scroll = measured.clone();
        let release = measured;
        let hover_outlines = outlines.clone();
        div()
            .relative()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .when(inline, |panel| panel.aspect_ratio(1.))
            .when(!inline, |panel| panel.h(height))
            .id(if inline {
                "synthesis-manual-tile"
            } else {
                "synthesis-full-manual-tile"
            })
            .test_support()
            .role(gpui_kit::Role::Group)
            .aria_label("Manual placement tile")
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .track_focus(&focus)
            .when_some(
                drag.map(|drag| drag.hit.part.cursor(true)).or(hover),
                |panel, cursor| panel.cursor(cursor),
            )
            .when(pan, |panel| panel.cursor(CursorStyle::ClosedHand))
            .child(crate::preview_background::checkerboard())
            .child(surface)
            .when(!disabled, |panel| {
                panel
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |editor, event: &MouseDownEvent, window, cx| {
                            let Some(frame) = press.get() else {
                                return;
                            };
                            let Some(source) = source else {
                                return;
                            };
                            let drag = hit_test(
                                &outlines,
                                selected,
                                source,
                                frame.image,
                                event.position,
                                window.rem_size(),
                            )
                            .map(|hit| TileDrag {
                                hit,
                                space: frame.image,
                                start: event.position,
                                pointer: event.position,
                                modifiers: event.modifiers,
                            });
                            editor.tile_event(TileEvent::Begin(drag), window, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |editor, event: &MouseDownEvent, _, cx| {
                            if let Some(frame) = pan_bounds.get() {
                                editor
                                    .navigate(SourceViewEvent::BeginPan(event.position, frame), cx);
                            }
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(|editor, _: &MouseUpEvent, _, cx| {
                            editor.navigate(SourceViewEvent::EndPan, cx)
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Right,
                        cx.listener(|editor, _: &MouseUpEvent, _, cx| {
                            editor.navigate(SourceViewEvent::EndPan, cx)
                        }),
                    )
                    .on_mouse_move(cx.listener(
                        move |editor, event: &MouseMoveEvent, window, cx| {
                            if editor.navigation.pan.is_some() {
                                editor.navigate(SourceViewEvent::Pan(event.position), cx);
                                return;
                            }
                            let hover = movement
                                .get()
                                .zip(source)
                                .and_then(|(frame, source)| {
                                    hit_test(
                                        &hover_outlines,
                                        selected,
                                        source,
                                        frame.image,
                                        event.position,
                                        window.rem_size(),
                                    )
                                })
                                .map(|hit| hit.part.cursor(false));
                            editor.tile_event(
                                TileEvent::Track(event.position, event.modifiers, hover),
                                window,
                                cx,
                            );
                        },
                    ))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |editor, event: &MouseUpEvent, window, cx| {
                            let outside = release
                                .get()
                                .is_some_and(|frame| !frame.viewport.contains(&event.position));
                            editor.tile_event(
                                TileEvent::Release(event.position, event.modifiers, outside),
                                window,
                                cx,
                            );
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |editor, event: &MouseUpEvent, window, cx| {
                            editor.tile_event(
                                TileEvent::Release(event.position, event.modifiers, true),
                                window,
                                cx,
                            );
                        }),
                    )
                    .on_key_down(cx.listener(
                        |editor, event: &gpui_kit::KeyDownEvent, window, cx| {
                            if event.keystroke.key == "escape" && editor.drag.is_some() {
                                cx.stop_propagation();
                                editor.tile_event(TileEvent::Cancel, window, cx);
                            }
                        },
                    ))
                    .on_scroll_wheel(cx.listener(move |editor, event: &ScrollWheelEvent, _, cx| {
                        let Some(frame) = scroll.get() else {
                            return;
                        };
                        let factor = match event.delta {
                            ScrollDelta::Lines(lines) => lines.y,
                            ScrollDelta::Pixels(delta) => delta.y.as_f32() / 60.,
                        }
                        .clamp(-1., 1.);
                        if factor != 0. {
                            let offset = event.position - frame.fitted.center();
                            cx.stop_propagation();
                            editor.navigate(
                                SourceViewEvent::Zoom(
                                    2_f32.powf(factor),
                                    [
                                        offset.x / frame.fitted.size.width,
                                        offset.y / frame.fitted.size.height,
                                    ],
                                ),
                                cx,
                            );
                        }
                    }))
            })
    }
}

fn paint_outline(
    outline: &Outline,
    space: Bounds<Pixels>,
    rem: Pixels,
    accent: Hsla,
    background: Hsla,
    window: &mut Window,
) {
    let [min, max] = outline.bounds;
    for x in copies(min[0], max[0], 0., 1.) {
        for y in copies(min[1], max[1], 0., 1.) {
            let offset = [x as f32, y as f32];
            let points: Vec<_> = outline
                .points
                .iter()
                .map(|&point| project(space, shift(point, offset)))
                .collect();
            for (stroke, color) in [(rem * 0.18, background), (rem * 0.1, accent)] {
                let mut path = PathBuilder::stroke(stroke);
                path.add_polygon(&points, true);
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
            }
        }
    }
}
