use super::output::{Naming, SequenceSettings};
use crate::{image_file::Format, solid_color::export_settings::checked_dimensions};
use gpui_kit::base::{StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, IndexPath, Sizable,
    checkbox::Checkbox,
    form::{Field, Form},
    input::{Input, InputEvent, InputState},
    select::{Select, SelectItem, SelectState},
    slider::{Slider, SliderEvent, SliderState, SliderValue},
};
use gpui_kit::{
    App, AppContext, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div,
    prelude::*,
};
use mycela_engine::ImageGeometry;

const DEFAULT_COUNT: u32 = 32;
const MAX_COUNT: u32 = 10_000;

#[derive(Clone)]
struct Choice<T> {
    title: SharedString,
    value: T,
}

impl<T: Clone + PartialEq> SelectItem for Choice<T> {
    type Value = T;
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    fn value(&self) -> &T {
        &self.value
    }
}

fn input(value: u32, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()))
}

fn number<T: std::str::FromStr>(input: &Entity<InputState>, cx: &App) -> Option<T> {
    input.read(cx).value().trim().parse().ok()
}

fn field(label: &'static str, id: &'static str, state: &Entity<InputState>) -> Field {
    Field::new()
        .label(label)
        .child(Input::new(state).id(id).small().aria_label(label))
}

pub(super) struct Settings {
    source: String,
    width: Entity<InputState>,
    height: Entity<InputState>,
    count: Entity<InputState>,
    first: Entity<InputState>,
    last: Entity<InputState>,
    frame_limit: u32,
    range: Entity<SliderState>,
    _range_subscription: Subscription,
    all: bool,
    antialias: bool,
    color_only: bool,
    effect_maps: bool,
    format: Entity<SelectState<Vec<Choice<Format>>>>,
    compression: Entity<SliderState>,
    padding: Entity<InputState>,
    numbering: Entity<SelectState<Vec<Choice<u32>>>>,
    naming: Entity<SelectState<Vec<Choice<Naming>>>>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl Settings {
    pub(super) fn new(
        source: String,
        previous: Option<SequenceSettings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let limit = previous.map_or(DEFAULT_COUNT, |v| v.frame_count);
        let start = previous.map_or(1, |v| v.first);
        let end = previous.map_or(limit, |v| v.last);
        let width = input(
            previous.map_or(256, |v| v.geometry.width() as u32),
            window,
            cx,
        );
        let height = input(
            previous.map_or(256, |v| v.geometry.height() as u32),
            window,
            cx,
        );
        let count = input(limit, window, cx);
        let first = input(start, window, cx);
        let last = input(end, window, cx);
        let padding = input(previous.map_or(3, |v| v.padding as u32), window, cx);
        let format = cx.new(|cx| {
            SelectState::new(
                Format::ALL
                    .map(|value| Choice {
                        title: value.label().into(),
                        value,
                    })
                    .to_vec(),
                Some(IndexPath::new(
                    previous
                        .and_then(|v| Format::ALL.iter().position(|format| *format == v.format))
                        .unwrap_or(0),
                )),
                window,
                cx,
            )
        });
        let compression = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(previous.map_or(10., |v| f32::from(100 - v.jpeg_quality)))
        });
        let numbering = cx.new(|cx| {
            SelectState::new(
                [0, 1]
                    .map(|value| Choice {
                        title: value.to_string().into(),
                        value,
                    })
                    .to_vec(),
                Some(IndexPath::new(
                    previous.map_or(1, |v| v.start_number as usize),
                )),
                window,
                cx,
            )
        });
        let naming = cx.new(|cx| {
            SelectState::new(
                Naming::ALL
                    .map(|value| Choice {
                        title: value.filename("Name", 1, 3, "ext").into(),
                        value,
                    })
                    .to_vec(),
                Some(IndexPath::new(
                    previous
                        .and_then(|v| Naming::ALL.iter().position(|naming| *naming == v.naming))
                        .unwrap_or(0),
                )),
                window,
                cx,
            )
        });
        let (range, range_subscription) = Self::range_slider(limit, start, end, window, cx);
        let mut subscriptions = vec![
            cx.observe(&format, |_, _, cx| cx.notify()),
            cx.observe(&numbering, |_, _, cx| cx.notify()),
            cx.observe(&naming, |_, _, cx| cx.notify()),
            cx.observe(&compression, |_, _, cx| cx.notify()),
        ];
        for field in [&count, &first, &last, &padding, &width, &height] {
            subscriptions.push(
                cx.subscribe_in(field, window, |view, field, event, window, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }
                    view.error = None;
                    if *field == view.count {
                        view.update_frame_count(window, cx);
                    } else if *field == view.first || *field == view.last {
                        if let Ok((first, last)) = view.frame_range(view.frame_limit, cx) {
                            view.range.update(cx, |range, cx| {
                                range.set_value((first as f32, last as f32), window, cx)
                            });
                        }
                    }
                    cx.notify();
                }),
            );
        }
        Self {
            source,
            width,
            height,
            count,
            first,
            last,
            frame_limit: limit,
            range,
            _range_subscription: range_subscription,
            all: start == 1 && end == limit,
            antialias: previous.is_none_or(|v| v.antialias),
            color_only: previous.is_some_and(|v| v.color_only),
            effect_maps: previous.is_some_and(|v| v.effect_maps),
            format,
            compression,
            padding,
            numbering,
            naming,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn range_slider(
        limit: u32,
        first: u32,
        last: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<SliderState>, Subscription) {
        let range = cx.new(|_| {
            SliderState::new()
                .min(1.)
                .max(limit as f32)
                .step(1.)
                .default_value((first as f32, last as f32))
        });
        let subscription = cx.subscribe_in(&range, window, |view, _, event, window, cx| {
            if let SliderEvent::Change(SliderValue::Range(first, last)) = event {
                view.set_range(*first as u32, *last as u32, window, cx);
                view.error = None;
                cx.notify();
            }
        });
        (range, subscription)
    }

    fn set_range(&self, first: u32, last: u32, window: &mut Window, cx: &mut App) {
        self.first.update(cx, |input, cx| {
            input.set_value(first.to_string(), window, cx)
        });
        self.last.update(cx, |input, cx| {
            input.set_value(last.to_string(), window, cx)
        });
    }

    fn update_frame_count(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(count) =
            number::<u32>(&self.count, cx).filter(|count| (1..=MAX_COUNT).contains(count))
        else {
            return;
        };
        if count == self.frame_limit {
            return;
        }
        let first = number::<u32>(&self.first, cx).unwrap_or(1).clamp(1, count);
        let previous_last = number::<u32>(&self.last, cx).unwrap_or(self.frame_limit);
        let last = if self.all || previous_last == self.frame_limit {
            count
        } else {
            previous_last.clamp(first, count)
        };
        self.frame_limit = count;
        self.set_range(first, last, window, cx);
        let (range, subscription) = Self::range_slider(count, first, last, window, cx);
        self.range = range;
        self._range_subscription = subscription;
    }

    fn frame_range(&self, count: u32, cx: &App) -> Result<(u32, u32), String> {
        if self.all {
            return Ok((1, count));
        }
        match (
            number::<u32>(&self.first, cx),
            number::<u32>(&self.last, cx),
        ) {
            (Some(first), Some(last))
                if (1..=count).contains(&first) && (first..=count).contains(&last) =>
            {
                Ok((first, last))
            }
            _ => Err(format!("Enter an inclusive frame range from 1 to {count}.")),
        }
    }

    pub(super) fn request(&self, cx: &App) -> Result<SequenceSettings, String> {
        let width = number(&self.width, cx).ok_or("Enter a positive whole number for width.")?;
        let height = number(&self.height, cx).ok_or("Enter a positive whole number for height.")?;
        let (width, height) = checked_dimensions(width, height)?;
        let count = number(&self.count, cx)
            .filter(|count| (1..=MAX_COUNT).contains(count))
            .ok_or("Enter a frame count from 1 to 10,000.")?;
        let (first, last) = self.frame_range(count, cx)?;
        let padding = number(&self.padding, cx)
            .filter(|padding| (1..=4).contains(padding))
            .ok_or("Enter a number padding from 1 to 4.")?;
        let SliderValue::Single(compression) = self.compression.read(cx).value() else {
            unreachable!("single compression control");
        };
        SequenceSettings {
            geometry: ImageGeometry::new(width, height),
            antialias: self.antialias,
            color_only: self.color_only,
            effect_maps: self.effect_maps,
            frame_count: count,
            first,
            last,
            format: *self
                .format
                .read(cx)
                .selected_value()
                .unwrap_or(&Format::Png),
            jpeg_quality: (100 - compression as u8).max(1),
            padding,
            start_number: *self.numbering.read(cx).selected_value().unwrap_or(&1),
            naming: *self
                .naming
                .read(cx)
                .selected_value()
                .unwrap_or(&Naming::Suffix),
        }
        .validate()
    }

    pub(super) fn show_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        cx.notify();
    }
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let format = *self
            .format
            .read(cx)
            .selected_value()
            .unwrap_or(&Format::Png);
        let total = number::<u32>(&self.count, cx).filter(|count| (1..=MAX_COUNT).contains(count));
        let selected = total
            .and_then(|count| self.frame_range(count, cx).ok())
            .map(|(first, last)| last - first + 1);
        let naming = *self
            .naming
            .read(cx)
            .selected_value()
            .unwrap_or(&Naming::Suffix);
        let number_start = *self.numbering.read(cx).selected_value().unwrap_or(&1);
        let example_padding = number::<usize>(&self.padding, cx)
            .filter(|v| (1..=4).contains(v))
            .unwrap_or(3)
            .max(selected.unwrap_or(1).to_string().len());
        let filename_example =
            naming.filename("Name", number_start, example_padding, format.extension());
        let section = |label: &'static str| div().text_sm().font_semibold().child(label);
        let info = |value: String| {
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(value)
        };
        div()
            .id("animation-sequence-settings")
            .test_support()
            .v_flex()
            .gap_3()
            .child(info(self.source.clone()))
            .child(section("Render settings"))
            .child(
                Form::new()
                    .columns(3)
                    .child(field("Width (px)", "sequence-width", &self.width))
                    .child(field("Height (px)", "sequence-height", &self.height))
                    .child(field("Number of frames", "sequence-count", &self.count)),
            )
            .child(
                div()
                    .h_flex()
                    .gap_4()
                    .flex_wrap()
                    .child(
                        Checkbox::new("sequence-antialias")
                            .small()
                            .label("Anti-alias")
                            .checked(self.antialias)
                            .on_click(cx.listener(|settings, checked, _, cx| {
                                settings.antialias = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("sequence-color-only")
                            .small()
                            .label("Color only")
                            .checked(self.color_only)
                            .on_click(cx.listener(|settings, checked, _, cx| {
                                settings.color_only = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("sequence-effect-maps")
                            .small()
                            .label("Effect maps")
                            .checked(self.effect_maps)
                            .on_click(cx.listener(|settings, checked, _, cx| {
                                settings.effect_maps = *checked;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                Checkbox::new("sequence-all")
                    .small()
                    .label("Render all frames")
                    .checked(self.all)
                    .on_click(cx.listener(|settings, checked, _, cx| {
                        settings.all = *checked;
                        settings.error = None;
                        cx.notify();
                    })),
            )
            .when(!self.all, |root| {
                root.child(
                    Form::new()
                        .columns(2)
                        .child(field("First frame", "sequence-first", &self.first))
                        .child(field("Last frame", "sequence-last", &self.last)),
                )
                .when(self.frame_limit > 1, |root| {
                    root.child(Slider::new(&self.range).w_full())
                })
            })
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .pt_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(section("Image sequence"))
                    .child(
                        Form::new()
                            .columns(2)
                            .child(
                                Field::new().label("File format").child(
                                    Select::new(&self.format)
                                        .id("sequence-format")
                                        .small()
                                        .accessibility_label("File format"),
                                ),
                            )
                            .child(
                                Field::new().label("Naming pattern").child(
                                    Select::new(&self.naming)
                                        .id("sequence-naming")
                                        .small()
                                        .accessibility_label("Naming pattern"),
                                ),
                            )
                            .child(field("Number padding", "sequence-padding", &self.padding))
                            .child(
                                Field::new().label("Start numbering at").child(
                                    Select::new(&self.numbering)
                                        .id("sequence-numbering")
                                        .small()
                                        .accessibility_label("Start numbering at"),
                                ),
                            ),
                    )
                    .when(format == Format::Jpeg, |root| {
                        root.child(info(format!(
                            "JPEG compression {}",
                            self.compression.read(cx).value()
                        )))
                        .child(Slider::new(&self.compression).w_full())
                    })
                    .child(info(match selected {
                        Some(1) => format!("1 frame · e.g. {filename_example}"),
                        Some(count) => format!("{count} frames · e.g. {filename_example}"),
                        None => format!("Example · {filename_example}"),
                    })),
            )
            .when_some(self.error.clone(), |root, error| {
                root.child(
                    div()
                        .id("sequence-error")
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error),
                )
            })
    }
}
