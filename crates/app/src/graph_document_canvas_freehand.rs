//! A freehand drag stays in the view until release fits one Canvas edit.
use super::*;
use gpui_kit::{
    App, Entity, Subscription,
    component::slider::{Slider, SliderState},
    rems,
};
use std::rc::Rc;

const CLOSE_RADIUS: Pixels = px(7.5);

#[derive(Clone)]
pub(in super::super) struct Freehand {
    pub(super) source: Option<PenSource>,
    smoothness: Entity<SliderState>,
    _smoothness_observer: Rc<Subscription>,
    draft: Option<StrokeDraft>,
}

#[derive(Clone)]
struct StrokeDraft {
    space: TileSpace,
    start: Option<EndpointHit>,
    samples: Vec<[f32; 2]>,
    smoothness: f32,
}

impl Freehand {
    pub(super) fn new(source: Option<PenSource>, cx: &mut Context<GraphDocumentView>) -> Self {
        let smoothness = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(30.)
        });
        let observer = cx.observe(&smoothness, |_, _, cx| cx.notify());
        Self {
            source,
            smoothness,
            _smoothness_observer: Rc::new(observer),
            draft: None,
        }
    }

    pub(in super::super) fn refresh(&mut self) {
        self.source = self
            .source
            .as_ref()
            .and_then(|source| PenSource::new(source.editor.clone()));
        self.draft = None;
    }

    pub(super) fn cancel(&mut self) -> bool {
        self.draft.take().is_some()
    }

    pub(in super::super) fn control(&self, disabled: bool, cx: &App) -> impl IntoElement {
        div()
            .h_flex()
            .items_center()
            .gap_2()
            .child(div().text_sm().child("Smoothness"))
            .child(
                div()
                    .id("texture-canvas-freehand-smoothness")
                    .test_support()
                    .role(gpui_kit::Role::Group)
                    .aria_label("Freehand smoothness")
                    .w(rems(8.))
                    .child(
                        Slider::new(&self.smoothness).disabled(disabled || self.draft.is_some()),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .child(format!("{:.0}", self.smoothness.read(cx).value().start())),
            )
    }

    pub(super) fn press(&mut self, space: TileSpace, event: &MouseDownEvent, cx: &App) {
        let start = self
            .source
            .as_ref()
            .and_then(|source| source.hit(space, event.position, None));
        self.draft = Some(StrokeDraft {
            space,
            start,
            samples: vec![space.tile_point(event.position)],
            smoothness: self.smoothness.read(cx).value().start(),
        });
    }

    pub(super) fn moved(&mut self, position: Point<Pixels>) -> bool {
        self.draft
            .as_mut()
            .is_some_and(|draft| draft.sample(position))
    }

    pub(super) fn release(&mut self, event: &MouseUpEvent, commit: bool) -> CurveAction {
        let Some(mut draft) = self.draft.take().filter(|_| commit) else {
            return CurveAction::None;
        };
        draft.sample(event.position);
        let start = draft.start;
        let closed = start.is_none() && draft.closes_at(event.position);
        if let Some(source) = self.source.as_ref()
            && !closed
        {
            let end = source.hit(draft.space, event.position, start.map(|hit| hit.endpoint));
            if let Some(anchor) = start.or(end) {
                for sample in &mut draft.samples {
                    *sample = unshift(*sample, anchor.shift);
                }
                let mut proposed = (*source.path).clone();
                if proposed.attach_freehand(
                    &draft.samples,
                    draft.smoothness,
                    start.map(|hit| hit.endpoint),
                    end.map(|hit| hit.endpoint),
                ) && proposed != *source.path
                {
                    return CurveAction::Update(
                        source.editor.clone(),
                        source.path.clone(),
                        proposed,
                    );
                }
                return CurveAction::None;
            }
        }
        CanvasPath::from_freehand(&draft.samples, draft.smoothness, closed)
            .map_or(CurveAction::None, CurveAction::Insert)
    }

    pub(super) fn paint(
        &self,
        space: TileSpace,
        width: Pixels,
        colors: (Hsla, Hsla),
        window: &mut Window,
    ) {
        if let Some(source) = &self.source {
            source.paint_endpoints(space, width, colors, window);
        }
        let Some(draft) = self.draft.as_ref() else {
            return;
        };
        let start = space.window_point(draft.samples[0]);
        if draft.start.is_none() {
            let end = space.window_point(*draft.samples.last().unwrap());
            let radius = if draft.closes_at(end) {
                5. * width
            } else {
                3. * width
            };
            paint_marker(start, radius, colors.0, window);
        }
        if draft.samples.len() < 2 {
            return;
        }
        for (width, color) in [(3. * width, colors.1), (width, colors.0)] {
            let mut path = PathBuilder::stroke(width);
            path.move_to(start);
            for &sample in &draft.samples[1..] {
                path.line_to(space.window_point(sample));
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        }
    }
}

impl StrokeDraft {
    fn sample(&mut self, position: Point<Pixels>) -> bool {
        let sample = self.space.tile_point(position);
        if self.samples.last() == Some(&sample) {
            return false;
        }
        self.samples.push(sample);
        true
    }

    fn closes_at(&self, position: Point<Pixels>) -> bool {
        if self.samples.len() < 3 {
            return false;
        }
        let start = self.space.window_point(self.samples[0]);
        (position.x - start.x).abs() <= CLOSE_RADIUS && (position.y - start.y).abs() <= CLOSE_RADIUS
    }
}
