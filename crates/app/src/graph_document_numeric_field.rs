use gpui_kit::base::{StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    input::{Input, InputEvent, InputState},
    slider::{Slider, SliderEvent, SliderScale, SliderState},
};
use gpui_kit::{
    App, AppContext, ElementId, Entity, IntoElement, RenderOnce, SharedString, Subscription,
    Window, div, prelude::*, rems,
};
use mycela_engine::History;
use std::sync::Arc;

/// Give the common editing interval most of the rail while retaining the legal endpoints.
#[derive(Clone, Copy)]
struct ThumbFocus {
    values: [f64; 4],
    track: [f64; 4],
}

impl ThumbFocus {
    fn new([minimum, maximum]: [f64; 2], [start, end]: [f64; 2]) -> Self {
        const OUTER_TRAVEL: f64 = 0.10;
        debug_assert!(minimum <= start && start < end && end <= maximum);
        let (left, right) = match (minimum < start, end < maximum) {
            (true, true) => (OUTER_TRAVEL / 2.0, OUTER_TRAVEL / 2.0),
            (true, false) => (OUTER_TRAVEL, 0.0),
            (false, true) => (0.0, OUTER_TRAVEL),
            (false, false) => (0.0, 0.0),
        };
        Self {
            values: [minimum, start, end, maximum],
            track: [0.0, left, 1.0 - right, 1.0],
        }
    }

    fn scale(value: f64, from: [f64; 4], to: [f64; 4]) -> f64 {
        let value = value.clamp(from[0], from[3]);
        let index = if value < from[1] {
            0
        } else if value > from[2] {
            2
        } else {
            1
        };
        let fraction = (value - from[index]) / (from[index + 1] - from[index]);
        to[index] + fraction * (to[index + 1] - to[index])
    }

    fn position(self, value: f64) -> f32 {
        Self::scale(value, self.values, self.track) as f32
    }

    fn value(self, position: f32) -> f64 {
        Self::scale(f64::from(position), self.track, self.values)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct NumericRange {
    minimum: f64,
    maximum: f64,
    step: f64,
}

impl NumericRange {
    pub(crate) const fn new(minimum: f64, maximum: f64, step: f64) -> Self {
        Self {
            minimum,
            maximum: if maximum < minimum { minimum } else { maximum },
            step,
        }
    }

    pub(crate) fn bounds<T: ToString>([minimum, maximum]: [T; 2], step: f64) -> Self {
        let exact = |value: T| value.to_string().parse::<f64>().expect("numeric endpoint");
        Self::new(exact(minimum), exact(maximum), step)
    }

    /// Extend a practical range to include a stored value. GPUI's slider uses f32.
    pub(crate) fn including(self, value: f64) -> Self {
        let limit = f64::from(f32::MAX) / 4.0;
        let value = value.clamp(-limit, limit);
        Self::new(self.minimum.min(value), self.maximum.max(value), self.step)
    }

    pub(in crate::graph_document) fn from_limits(
        limits: &mycela_engine::SharedLimits,
    ) -> Option<Self> {
        let limits = limits.snapshot();
        let minimum = limits.minimum.to_framework_f64();
        let maximum = limits.maximum.to_framework_f64();
        let increment = limits.increment.to_framework_f64().abs();
        if minimum <= maximum {
            let step = if increment > 0.0 {
                increment
            } else {
                10_f64.powi(-limits.decimal_places.clamp(0, 10))
            };
            Some(Self::new(minimum, maximum, step))
        } else {
            None
        }
    }

    fn thumb_step(self) -> f32 {
        let offset = self.minimum / self.step;
        let step = if (offset - offset.round()).abs() < 0.000_001 {
            self.step
        } else {
            // GPUI snaps from zero. Preserve position precision so text() can
            // use the retained property's actual minimum as its step origin.
            self.step * 0.01
        };
        let magnitude = self.minimum.abs().max(self.maximum.abs());
        step.max(magnitude / (f64::from(f32::MAX) / 2.0)) as f32
    }

    fn text(self, value: f64, position: f32) -> String {
        if position <= 0.0 || value <= self.minimum {
            return self.minimum.to_string();
        }
        if position >= 1.0 || value >= self.maximum {
            return self.maximum.to_string();
        }
        let stepped = self.minimum + ((value - self.minimum) / self.step).round() * self.step;
        let digits = |number: f64| {
            number
                .to_string()
                .split_once('.')
                .map_or(0, |(_, decimal)| decimal.len())
        };
        let precision = digits(self.step).max(digits(self.minimum)).min(12);
        let text = format!(
            "{:.*}",
            precision,
            stepped.clamp(self.minimum, self.maximum)
        );
        if precision == 0 {
            text
        } else {
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        }
    }
}

struct SliderHandle {
    slider: Entity<SliderState>,
    last_text: SharedString,
    _subscription: Subscription,
    _observation: Subscription,
}

/// A node property retains the same input entity and commit path for both controls.
#[derive(IntoElement)]
pub(crate) struct NumericField {
    input: Entity<InputState>,
    id: ElementId,
    label: SharedString,
    range: Option<NumericRange>,
    disabled: bool,
    live: bool,
    logarithmic: bool,
    whole_numbers: bool,
    focus: Option<[f64; 2]>,
    submit_on_release: bool,
    history: Option<History<crate::ForegroundError>>,
}

impl NumericField {
    pub(crate) fn new(
        input: &Entity<InputState>,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        range: impl Into<Option<NumericRange>>,
    ) -> Self {
        Self {
            input: input.clone(),
            id: id.into(),
            label: label.into(),
            range: range.into(),
            disabled: false,
            live: false,
            logarithmic: false,
            whole_numbers: false,
            focus: None,
            submit_on_release: false,
            history: None,
        }
    }

    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub(in crate::graph_document) fn live(mut self, live: bool) -> Self {
        self.live = live;
        self
    }

    pub(in crate::graph_document) fn logarithmic(mut self, logarithmic: bool) -> Self {
        self.logarithmic = logarithmic;
        self
    }

    pub(in crate::graph_document) fn whole_numbers(mut self, whole_numbers: bool) -> Self {
        self.whole_numbers = whole_numbers;
        self
    }

    pub(super) fn focus_travel(mut self, bounds: [f64; 2]) -> Self {
        self.focus = Some(bounds);
        self
    }

    /// These editors already commit a completed text edit on Enter or blur.
    pub(super) fn submit_on_release(mut self) -> Self {
        self.submit_on_release = true;
        self
    }

    pub(super) fn history(mut self, history: Option<History<crate::ForegroundError>>) -> Self {
        self.history = history;
        self
    }
}

impl RenderOnce for NumericField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let input = Input::new(&self.input)
            .id(self.id.clone())
            .disabled(self.disabled)
            .aria_label(self.label);
        let Some(range) = self.range else {
            return input.into_any_element();
        };
        let focus = self
            .focus
            .map(|bounds| ThumbFocus::new([range.minimum, range.maximum], bounds));
        let (min, max, step) = if focus.is_some() {
            (0.0, 1.0, 0.000_001)
        } else {
            (
                range.minimum as f32,
                range.maximum as f32,
                range.thumb_step(),
            )
        };
        let thumb_value = move |value: f64| {
            let value = value.clamp(range.minimum, range.maximum);
            focus.map_or(value as f32, |focus| focus.position(value))
        };
        let disabled = self.disabled || min >= max;
        let text = self.input.read(cx).value();
        let numeric_text = text
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && (!self.whole_numbers || value.fract() == 0.0));
        let slider_id = match &self.id {
            ElementId::Name(id) => ElementId::from(format!("{id}:slider")),
            _ => ElementId::NamedChild(Arc::new(self.id.clone()), "slider".into()),
        };
        let slider_key = ElementId::NamedChild(
            Arc::new(("node-numeric-field", self.input.entity_id()).into()),
            format!(
                "{}:{}:{}:{:?}",
                range.minimum, range.maximum, range.step, self.focus
            )
            .into(),
        );
        let slider_handle = window.use_keyed_state(slider_key, cx, |window, cx| {
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(min.min(0.0))
                    .max(max)
                    .min(min)
                    .step(step)
                    .scale(if self.logarithmic && min > 0.0 && min < max {
                        SliderScale::Logarithmic
                    } else {
                        SliderScale::Linear
                    })
                    .default_value(thumb_value(numeric_text.unwrap_or(range.minimum)))
            });
            let input = self.input.downgrade();
            let mut changed = false;
            let subscription = window.subscribe(&slider, cx, move |slider, event, window, cx| {
                let value = match event {
                    SliderEvent::Change(value) if self.live => value,
                    SliderEvent::Release(value) if !self.live => value,
                    SliderEvent::Release(_) if changed => {
                        if let Some(history) = &self.history {
                            history.separate_next_edit();
                        }
                        changed = false;
                        return;
                    }
                    _ => return,
                };
                if let Some(input) = input.upgrade() {
                    let position = slider.read(cx).percentage().end;
                    let value =
                        focus.map_or(f64::from(value.start()), |focus| focus.value(position));
                    let text = range.text(value, position);
                    input.update(cx, |input, cx| {
                        if input.value().as_ref() != text {
                            if !changed && let Some(history) = &self.history {
                                history.separate_next_edit();
                            }
                            changed = self.live;
                            input.replace_all(text, window, cx);
                            if self.submit_on_release {
                                cx.emit(InputEvent::PressEnter {
                                    secondary: false,
                                    shift: false,
                                });
                            }
                        }
                    });
                }
            });
            let observation = cx.observe(&slider, |_, _, cx| cx.notify());
            SliderHandle {
                slider,
                last_text: text.clone(),
                _subscription: subscription,
                _observation: observation,
            }
        });
        let slider = slider_handle.read(cx).slider.clone();
        if slider_handle.read(cx).last_text != text {
            if let Some(value) = numeric_text
                && (range.minimum..=range.maximum).contains(&value)
                && slider.read(cx).value().start() != thumb_value(value)
            {
                slider.update(cx, |slider, cx| {
                    slider.set_value(thumb_value(value), window, cx)
                });
            }
            slider_handle.update(cx, |handle, _| handle.last_text = text);
        }
        let endpoint_color = cx.theme().muted_foreground;
        div()
            .h_flex()
            .items_start()
            .flex_wrap()
            .gap_2()
            .child(
                div()
                    .id(slider_id)
                    .test_support()
                    .v_flex()
                    .flex_1()
                    .min_w(rems(4.0))
                    .child(Slider::new(&slider).w_full().disabled(disabled))
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .gap_1()
                            .text_xs()
                            .text_color(endpoint_color)
                            .child(range.minimum.to_string())
                            .when(
                                range.minimum < 0.0 && range.minimum == -range.maximum,
                                |labels| labels.child("0"),
                            )
                            .child(range.maximum.to_string()),
                    ),
            )
            .child(div().w(rems(4.75)).flex_shrink_0().child(input.small()))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::ThumbFocus;

    #[test]
    fn synthesis_position_uses_most_of_the_track_for_one_tile() {
        let focus = ThumbFocus::new([0.0, 10_000.0], [0.0, 100.0]);
        assert_eq!(focus.value(0.0), 0.0);
        assert_eq!(focus.value(1.0), 10_000.0);
        assert_eq!(focus.position(100.0), 0.9);
        assert!((focus.position(71.0) - 0.639).abs() < 0.000_001);
        assert!((focus.value(focus.position(3000.0)) - 3000.0).abs() < 0.01);
    }

    #[test]
    fn synthesis_extent_preserves_zero_normal_sizes_and_both_tails() {
        let focus = ThumbFocus::new([-10_000.0, 10_000.0], [-100.0, 100.0]);
        assert_eq!(focus.value(0.0), -10_000.0);
        assert_eq!(focus.value(1.0), 10_000.0);
        assert_eq!(focus.position(0.0), 0.5);
        assert!((focus.position(58.0) - 0.761).abs() < 0.000_001);
        assert!((focus.value(focus.position(-3000.0)) + 3000.0).abs() < 0.01);
    }
}
