//! Layout and coordinate scale for numeric labels on the curve plots.
use gpui_kit::base::{StyledExt, TestSupportExt};
use gpui_kit::component::ActiveTheme;
use gpui_kit::{App, Div, IntoElement, Pixels, div, prelude::*, px, relative, rems};

// Keep labels outside the line area. An interactive plot uses the same inset
// plus its one-pixel focus border when aligning the labels to the data.
pub(crate) const PLOT_INSET: Pixels = px(8.);
pub(crate) const INPUT_TICKS: [f32; 5] = [0., 0.25, 0.5, 0.75, 1.];
const INPUT_LABELS: [&str; 5] = ["0", "0.25", "0.5", "0.75", "1"];

pub(crate) struct CurveAxes {
    range: [f32; 2],
    output_ticks: Vec<(f32, String)>,
    output_guides: Vec<f32>,
}

impl CurveAxes {
    pub(crate) fn normalized(range: [f32; 2]) -> Self {
        let [low, high] = range;
        let midpoint = f64::midpoint(f64::from(low), f64::from(high)) as f32;
        let ticks = [low, midpoint, high];
        Self {
            range,
            output_ticks: ticks.map(|value| (value, compact_number(value))).into(),
            output_guides: ticks.into(),
        }
    }

    pub(crate) fn percent(range: [f32; 2]) -> Self {
        let [low, high] = range;
        let (grid_step, label_step) = match high - low {
            span if span < 200. => (10., 50.),
            span if span < 300. => (25., 50.),
            span if span < 500. => (25., 100.),
            span if span < 1000. => (50., 100.),
            _ => (100., 200.),
        };
        // Keep labels legible in a compact inspector even at the full ±1000% range.
        let label_step = label_step * ((high - low) / (5. * label_step)).ceil().max(1.);
        let ticks = |step: f32| {
            let first = (low / step).ceil() as i32;
            let last = (high / step).floor() as i32;
            (first..=last).map(move |index| index as f32 * step)
        };
        Self {
            range,
            output_ticks: ticks(label_step)
                .map(|value| (value, format!("{value:.0}%")))
                .collect(),
            output_guides: ticks(grid_step).collect(),
        }
    }

    pub(crate) fn output_guides(&self) -> &[f32] {
        &self.output_guides
    }

    pub(crate) fn frame(&self, plot: impl IntoElement, border: Pixels, cx: &App) -> Div {
        let inset = PLOT_INSET + border;
        let gutter = rems(4.25);
        let label_height = rems(1.);
        let label_width = rems(2.25);
        let labels = cx.theme().muted_foreground;
        let output_axis = div()
            .id("curve-output-axis")
            .test_support()
            .aria_label("Output scale")
            .relative()
            .w(gutter)
            .h_full()
            .flex_none()
            .child(
                div()
                    .absolute()
                    .top(inset)
                    .bottom(inset)
                    .left_0()
                    .right_1()
                    .children(self.output_ticks.iter().map(|(value, label)| {
                        div()
                            .id(label.clone())
                            .test_support()
                            .aria_label(label.clone())
                            .absolute()
                            .top(relative(1. - output_fraction(*value, self.range)))
                            .mt(label_height * -0.5)
                            .h(label_height)
                            .w_full()
                            .text_right()
                            .child(label.clone())
                    })),
            );
        let input_axis = div()
            .id("curve-input-axis")
            .test_support()
            .aria_label("Input scale")
            .relative()
            .ml(gutter)
            .h(label_height)
            .flex_none()
            .child(
                div()
                    .absolute()
                    .left(inset)
                    .right(inset)
                    .top_0()
                    .bottom_0()
                    .children(INPUT_TICKS.into_iter().zip(INPUT_LABELS).map(
                        |(fraction, label)| {
                            div()
                                .id(label)
                                .test_support()
                                .aria_label(label)
                                .absolute()
                                .left(relative(fraction))
                                .ml(label_width * -0.5)
                                .w(label_width)
                                .text_center()
                                .child(label)
                        },
                    )),
            );
        div()
            .size_full()
            .v_flex()
            .text_xs()
            .line_height(label_height)
            .text_color(labels)
            .child(
                div()
                    .h_flex()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .child(output_axis)
                    .child(div().min_w_0().h_full().flex_1().child(plot)),
            )
            .child(input_axis)
    }
}

pub(crate) fn output_fraction(value: f32, [low, high]: [f32; 2]) -> f32 {
    let low = f64::from(low);
    ((f64::from(value) - low) / (f64::from(high) - low)) as f32
}

fn compact_number(value: f32) -> String {
    if value == 0. {
        "0".into()
    } else if !(0.001..10_000.).contains(&value.abs()) {
        format!("{value:.2e}")
    } else {
        format!("{value:.3}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}
