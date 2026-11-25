//! Shared whole-contour handles and hit testing for both Patches source views.
use super::*;
use crate::graph_document::view::numeric_field::{NumericField, NumericRange};
use mycela_engine::{
    SynthesisPatchResizeHandle, SynthesisPatchTransform, SynthesisPatchTransformBounds,
    SynthesisPatchTransformField,
};

pub(in crate::graph_document::view::synthesis) struct TransformNumbers {
    inputs: [(SynthesisPatchTransformField, Entity<InputState>); 4],
    _observers: Vec<Subscription>,
}

impl TransformNumbers {
    pub(super) fn new<T: 'static>(
        bounds: SynthesisPatchTransformBounds,
        dispatch: impl Fn(&mut T, SynthesisPatchTransformField, f64, &mut Window, &mut Context<T>)
        + 'static,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Self {
        use SynthesisPatchTransformField::*;
        let inputs = [CenterX, CenterY, Width, Height].map(|field| {
            let [min, max] = field.limits();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(format!("{:.2}", bounds.field_percent(field)))
                    .min(min)
                    .max(max)
                    .step(0.01)
            });
            (field, input)
        });
        let dispatch = Rc::new(dispatch);
        let observers = inputs
            .iter()
            .map(|(field, input)| {
                let field = *field;
                let dispatch = dispatch.clone();
                cx.subscribe_in(input, window, move |view, input, event, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur)
                        && let Ok(value) = input.read(cx).value().trim().parse::<f64>()
                        && value.is_finite()
                    {
                        dispatch(view, field, value, window, cx);
                    }
                })
            })
            .collect();
        Self {
            inputs,
            _observers: observers,
        }
    }

    pub(super) fn sync(
        &self,
        bounds: SynthesisPatchTransformBounds,
        window: &mut Window,
        cx: &mut App,
    ) {
        for (field, input) in &self.inputs {
            let text = format!("{:.2}", bounds.field_percent(*field));
            if text != input.read(cx).value().as_ref() {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
    }

    pub(super) fn focused(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<(SynthesisPatchTransformField, Option<f64>)> {
        self.inputs.iter().find_map(|(field, input)| {
            let input = input.read(cx);
            gpui_kit::Focusable::focus_handle(input, cx)
                .is_focused(window)
                .then(|| {
                    (
                        *field,
                        input
                            .value()
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .filter(|value| value.is_finite()),
                    )
                })
        })
    }

    pub(super) fn render(&self, inline: bool, disabled: bool) -> Form {
        let id = if inline {
            "synthesis-patch-transform-field"
        } else {
            "synthesis-full-patch-transform-field"
        };
        Form::new().columns(if inline { 2 } else { 4 }).children(
            self.inputs
                .iter()
                .enumerate()
                .map(|(index, (field, input))| {
                    let label = match field {
                        SynthesisPatchTransformField::CenterX => "Center X (%)",
                        SynthesisPatchTransformField::CenterY => "Center Y (%)",
                        SynthesisPatchTransformField::Width => "Width (%)",
                        SynthesisPatchTransformField::Height => "Height (%)",
                    };
                    let limits = field.limits();
                    Field::new().label(label).child(
                        NumericField::new(
                            input,
                            (id, index),
                            label,
                            NumericRange::bounds(limits, 0.01),
                        )
                        .focus_travel(limits.map(|limit| limit.clamp(-100.0, 100.0)))
                        .submit_on_release()
                        .disabled(disabled),
                    )
                }),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::graph_document::view::synthesis) enum PatchPart {
    Body,
    Resize(SynthesisPatchResizeHandle),
    Rotate,
}

impl PatchPart {
    pub(super) fn transform(
        self,
        drag: Drag,
        position: [f32; 2],
        modifiers: Modifiers,
    ) -> SynthesisPatchTransform {
        match self {
            Self::Body => SynthesisPatchTransform::Translate { delta: position },
            Self::Resize(handle) => SynthesisPatchTransform::Resize {
                handle,
                delta: position,
                preserve_aspect: modifiers.shift,
                from_center: modifiers.control,
            },
            Self::Rotate => SynthesisPatchTransform::Rotate {
                pointer: position,
                fitted_size: [
                    drag.space.size.width.as_f32(),
                    drag.space.size.height.as_f32(),
                ],
            },
        }
    }

    pub(super) fn cursor(self, dragging: bool) -> CursorStyle {
        use SynthesisPatchResizeHandle::*;
        match self {
            Self::Body | Self::Rotate if dragging => CursorStyle::ClosedHand,
            Self::Body | Self::Rotate => CursorStyle::OpenHand,
            Self::Resize(North | South) => CursorStyle::ResizeUpDown,
            Self::Resize(West | East) => CursorStyle::ResizeLeftRight,
            Self::Resize(NorthWest | SouthEast) => CursorStyle::ResizeUpLeftDownRight,
            Self::Resize(NorthEast | SouthWest) => CursorStyle::ResizeUpRightDownLeft,
        }
    }
}

pub(super) struct TransformBox {
    pub(super) selection: usize,
    pub(super) bounds: SynthesisPatchTransformBounds,
    pub(super) angle: f32,
}

impl TransformBox {
    fn screen_min_extent(&self, frame: GuideFrame) -> Pixels {
        (self.bounds.size[0].abs() * frame.image.size.width)
            .min(self.bounds.size[1].abs() * frame.image.size.height)
    }

    fn handles(&self, frame: GuideFrame) -> [(PatchPart, Point<Pixels>); 9] {
        use SynthesisPatchResizeHandle::*;
        let [x, y] = self.bounds.origin;
        let [width, height] = self.bounds.size;
        let x_middle = x + width / 2.;
        let y_middle = y + height / 2.;
        let top = project(frame.image, [x_middle, y.min(y + height)]);
        let above = top.y - gpui_kit::px(13.);
        let rotate = point(
            top.x,
            if above >= frame.viewport.top() + gpui_kit::px(5.) {
                above
            } else {
                top.y + gpui_kit::px(13.)
            },
        );
        let [nw, n, ne, w, e, sw, s, se] = [
            (NorthWest, [x, y]),
            (North, [x_middle, y]),
            (NorthEast, [x + width, y]),
            (West, [x, y_middle]),
            (East, [x + width, y_middle]),
            (SouthWest, [x, y + height]),
            (South, [x_middle, y + height]),
            (SouthEast, [x + width, y + height]),
        ]
        .map(|(handle, position)| (PatchPart::Resize(handle), project(frame.image, position)));
        let handles = [nw, n, ne, w, e, sw, s, se, (PatchPart::Rotate, rotate)];
        if self.angle == 0. {
            return handles;
        }
        let pivot = project(frame.image, self.bounds.pivot);
        let (sin, cos) = self.angle.sin_cos();
        handles.map(|(part, center)| {
            let offset = center - pivot;
            (
                part,
                pivot
                    + point(
                        offset.x * cos - offset.y * sin,
                        offset.x * sin + offset.y * cos,
                    ),
            )
        })
    }

    pub(super) fn hit(
        &self,
        frame: GuideFrame,
        cursor: Point<Pixels>,
    ) -> Option<(Handle, [f32; 2])> {
        let resize_hit_radius = gpui_kit::px(8.).min(self.screen_min_extent(frame) * 0.38);
        let (part, position, _) = self
            .handles(frame)
            .into_iter()
            .filter_map(|(part, position)| {
                let delta = position - cursor;
                let distance = delta.x.abs().max(delta.y.abs());
                let radius = if part == PatchPart::Rotate {
                    gpui_kit::px(8.)
                } else {
                    resize_hit_radius
                };
                (frame.viewport.contains(&position) && distance <= radius)
                    .then_some((part, position, distance))
            })
            .min_by_key(|&(_, _, distance)| distance)?;
        let start = if part == PatchPart::Rotate {
            let [x, y] = self.bounds.pivot;
            let radius = ((position.y - project(frame.image, [x, y]).y) / frame.image.size.height)
                .abs()
                .max(0.04);
            [x, y - radius]
        } else {
            [0.; 2]
        };
        Some((
            Handle::PatchTransform {
                selection: self.selection,
                part,
            },
            start,
        ))
    }

    pub(super) fn paint(
        &self,
        frame: GuideFrame,
        stroke: Pixels,
        highlighted: Option<PatchPart>,
        (accent, background): (Hsla, Hsla),
        window: &mut Window,
    ) {
        let handles = self.handles(frame);
        let mut lines = PathBuilder::stroke(stroke);
        lines.add_polygon(&[0, 2, 7, 5].map(|index| handles[index].1), true);
        lines.move_to(handles[if self.bounds.size[1] < 0. { 6 } else { 1 }].1);
        lines.line_to(handles[8].1);
        if let Ok(path) = lines.build() {
            window.paint_path(path, accent);
        }
        for (part, center) in handles {
            let radius = if part == PatchPart::Rotate {
                4.5 * stroke
            } else {
                (3.5 * stroke).min((self.screen_min_extent(frame) * 0.2).max(stroke))
            };
            let rounded = if part == PatchPart::Rotate {
                radius
            } else {
                Pixels::ZERO
            };
            let square = |r| Bounds::new(center - point(r, r), size(r * 2., r * 2.));
            window.paint_quad(fill(square(radius), accent).corner_radii(rounded));
            if Some(part) != highlighted && radius > stroke {
                window.paint_quad(fill(square(radius - stroke), background).corner_radii(rounded));
            }
        }
    }
}

pub(super) fn contains(outline: &[[f32; 2]], [x, y]: [f32; 2]) -> bool {
    let Some(&last) = outline.last() else {
        return false;
    };
    let mut inside = false;
    let mut previous = last;
    for &next in outline {
        let [x0, y0] = previous;
        let [x1, y1] = next;
        if (y0 > y) != (y1 > y) && x < (x1 - x0) * (y - y0) / (y1 - y0) + x0 {
            inside = !inside;
        }
        previous = next;
    }
    inside
}
