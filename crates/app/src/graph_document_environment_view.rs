//! View settings and a projected preview for the standalone environment export.
use super::{FileKind, Format, PreferenceFile, Ticket, model};
use crate::{
    ForegroundError, LIVE_PREVIEW_SIZE,
    export_render::generator_execution,
    preview_background::checkerboard_for,
    solid_color::map_preview::{self, PreviewSelection},
};
use gpui_kit::base::{StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, IndexPath, Sizable,
    button::Button,
    checkbox::Checkbox,
    collapsible::Collapsible,
    form::{Field, Form},
    input::{Input, InputEvent, InputState},
    select::{Select, SelectEvent, SelectItem, SelectState},
};
use gpui_kit::{
    App, AppContext, Context, Entity, IntoElement, ObjectFit, Render, RenderImage, Subscription,
    Window, div, img, prelude::*, rems,
};
use mycela_engine::{
    EnvironmentDisplayTone, EnvironmentImageProjection, EnvironmentRecord,
    EnvironmentRenderSettings, EnvironmentResourceLibrary, SharedImageMap, round_decimal_places,
};
use std::{sync::Arc, time::Duration};

const MAX_OUTPUT_WIDTH: i32 = 6000;

fn fit_width(projection: EnvironmentImageProjection, width: i32, previous: i32) -> i32 {
    let (step, minimum) = match projection {
        EnvironmentImageProjection::HorizontalCross => (4, 4),
        EnvironmentImageProjection::VerticalCross => (3, 6),
        _ => return width.clamp(4, MAX_OUTPUT_WIDTH),
    };
    let width = width.clamp(4, MAX_OUTPUT_WIDTH);
    let remainder = width % step;
    let lower = width - remainder;
    let next = if remainder != 0 && width > previous {
        lower + step
    } else {
        lower
    };
    next.clamp(minimum, MAX_OUTPUT_WIDTH)
}

#[derive(Clone, Copy)]
pub(in crate::graph_document::view) struct OutputPreferences {
    projection: EnvironmentImageProjection,
    width: i32,
    format: Format,
}

impl Default for OutputPreferences {
    fn default() -> Self {
        Self {
            projection: EnvironmentImageProjection::LatitudeLongitude,
            width: LIVE_PREVIEW_SIZE,
            format: Format::Png,
        }
    }
}

impl gpui_kit::Global for OutputPreferences {}

impl OutputPreferences {
    pub(super) fn current(cx: &App) -> Self {
        cx.try_global::<Self>()
            .copied()
            .or_else(|| Self::decode(&PreferenceFile::Export.read(cx)?))
            .unwrap_or_default()
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let mut fields = std::str::from_utf8(bytes).ok()?.split_whitespace();
        let projection =
            *EnvironmentImageProjection::ALL.get(fields.next()?.parse::<usize>().ok()?)?;
        let width = fields.next()?.parse().ok()?;
        let extension = fields.next()?;
        let format = Format::ALL
            .into_iter()
            .find(|format| format.extension() == extension)?;
        if fields.next().is_some() || !(4..=MAX_OUTPUT_WIDTH).contains(&width) {
            return None;
        }
        projection.output_geometry(width)?;
        Some(Self {
            projection,
            width,
            format,
        })
    }

    pub(super) fn save(self, cx: &App) -> Result<(), String> {
        let data = format!(
            "{} {} {}\n",
            self.projection as usize,
            self.width,
            self.format.extension()
        );
        PreferenceFile::Export.write(data.as_bytes(), cx)
    }
}

#[cfg(test)]
#[test]
fn export_preferences_reject_invalid_projection_widths_and_formats() {
    let prefs = OutputPreferences::decode(b"3 64 hdr\n").unwrap();
    assert_eq!(
        prefs.projection,
        EnvironmentImageProjection::HorizontalCross
    );
    assert_eq!(prefs.width, 64);
    assert!(prefs.format == Format::Hdr);
    for bad in [
        b"3 63 hdr".as_slice(),
        b"2 6001 png",
        b"0 512 unknown",
        b"8 64 png",
    ] {
        assert!(OutputPreferences::decode(bad).is_none());
    }
}

#[cfg(test)]
#[test]
fn export_width_fits_vertical_cross_in_the_direction_of_the_edit() {
    let projection = EnvironmentImageProjection::VerticalCross;
    assert_eq!(fit_width(projection, 47, 48), 45);
    assert_eq!(fit_width(projection, 47, 45), 48);
    assert_eq!(fit_width(projection, 4, 48), 6);
    assert_eq!(fit_width(projection, 5999, 5997), 6000);
}

#[derive(Clone)]
struct Choice<T> {
    title: &'static str,
    value: T,
}

impl<T: Clone + PartialEq> SelectItem for Choice<T> {
    type Value = T;
    fn title(&self) -> gpui_kit::SharedString {
        self.title.into()
    }
    fn value(&self) -> &T {
        &self.value
    }
}

struct ProjectedPreview {
    render: EnvironmentRenderSettings,
    projected: SharedImageMap,
    tone: EnvironmentDisplayTone,
    raster: Arc<RenderImage>,
}

struct ViewInputs {
    rotation: [Entity<InputState>; 3],
    exposure: Entity<InputState>,
    gamma: Entity<InputState>,
}

impl ViewInputs {
    fn values(&self, cx: &App) -> Result<([f64; 3], EnvironmentDisplayTone), String> {
        let parse = |input: &Entity<InputState>, label, min, max, decimals| {
            let value = input
                .read(cx)
                .value()
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| (min..=max).contains(value))
                .ok_or_else(|| format!("{label} must be between {min} and {max}."))?;
            round_decimal_places(value, decimals).map_err(model::failure)
        };
        let [heading, pitch, roll] = &self.rotation;
        let degrees = [
            parse(heading, "Heading", -360., 360., 1)?,
            parse(pitch, "Pitch", -360., 360., 1)?,
            parse(roll, "Roll", -360., 360., 1)?,
        ];
        Ok((
            degrees,
            EnvironmentDisplayTone {
                exposure: parse(&self.exposure, "View exposure", -20., 20., 2)?,
                gamma: parse(&self.gamma, "View gamma", 0.01, 100., 2)?,
            },
        ))
    }
}

pub(super) struct ExportSettings {
    map: Arc<EnvironmentRecord>,
    library: EnvironmentResourceLibrary,
    projection: Entity<SelectState<Vec<Choice<EnvironmentImageProjection>>>>,
    format: Entity<SelectState<Vec<Choice<Format>>>>,
    width: Entity<InputState>,
    committed_width: i32,
    selected_projection: EnvironmentImageProjection,
    remembered_widths: [Option<i32>; 5],
    view: ViewInputs,
    view_open: bool,
    export_rotation: bool,
    last_preview: Option<ProjectedPreview>,
    preview_ticket: Option<Ticket>,
    preview_error: Option<String>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ExportSettings {
    pub(super) fn new(
        map: Arc<EnvironmentRecord>,
        library: EnvironmentResourceLibrary,
        preferences: OutputPreferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let projection = cx.new(|cx| {
            SelectState::new(
                EnvironmentImageProjection::ALL
                    .map(|value| Choice {
                        title: value.label(),
                        value,
                    })
                    .to_vec(),
                Some(IndexPath::new(preferences.projection as usize)),
                window,
                cx,
            )
        });
        let format = cx.new(|cx| {
            SelectState::new(
                Format::ALL
                    .map(|value| Choice {
                        title: value.label(),
                        value,
                    })
                    .to_vec(),
                Some(IndexPath::new(preferences.format as usize)),
                window,
                cx,
            )
        });
        let width =
            cx.new(|cx| InputState::new(window, cx).default_value(preferences.width.to_string()));
        let view = ViewInputs {
            rotation: std::array::from_fn(|_| {
                cx.new(|cx| InputState::new(window, cx).default_value("0"))
            }),
            exposure: cx.new(|cx| InputState::new(window, cx).default_value("0")),
            gamma: cx.new(|cx| InputState::new(window, cx).default_value("1")),
        };
        let mut subscriptions = vec![
            cx.subscribe_in(&projection, window, |settings, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(projection)) = event {
                    settings.switch_projection(*projection, window, cx);
                }
            }),
            cx.subscribe_in(
                &width,
                window,
                |settings, _, event, window, cx| match event {
                    InputEvent::Change => {
                        settings.error = None;
                        settings.refresh_preview(true, cx);
                    }
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        settings.commit_width(window, cx);
                    }
                    _ => {}
                },
            ),
        ];
        for input in view.rotation.iter().chain([&view.exposure, &view.gamma]) {
            subscriptions.push(cx.subscribe(input, |settings, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    settings.error = None;
                    settings.refresh_preview(true, cx);
                }
            }));
        }
        Self {
            map,
            library,
            projection,
            format,
            width,
            committed_width: preferences.width,
            selected_projection: preferences.projection,
            remembered_widths: [None; 5],
            view,
            view_open: false,
            export_rotation: true,
            last_preview: None,
            preview_ticket: None,
            preview_error: None,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn width(&self, cx: &App) -> Result<i32, String> {
        self.width
            .read(cx)
            .value()
            .trim()
            .parse::<i32>()
            .ok()
            .filter(|width| (4..=MAX_OUTPUT_WIDTH).contains(width))
            .ok_or_else(|| format!("Enter a width from 4 to {MAX_OUTPUT_WIDTH} pixels."))
    }

    fn geometry(&self, cx: &App) -> Result<(i32, i32), String> {
        let projection = self.selected_projection;
        let width = fit_width(projection, self.width(cx)?, self.committed_width);
        let geometry = projection
            .output_geometry(width)
            .ok_or("Choose a valid output width.")?;
        Ok((width, geometry.height()))
    }

    fn commit_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok((width, _)) = self.geometry(cx) else {
            return;
        };
        if width != self.committed_width {
            self.remembered_widths = [None; 5];
            self.committed_width = width;
        }
        let value = width.to_string();
        if self.width.read(cx).value().as_ref() != value {
            self.width
                .update(cx, |input, cx| input.set_value(value, window, cx));
        }
    }

    fn switch_projection(
        &mut self,
        target: EnvironmentImageProjection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.selected_projection;
        if current == target {
            return;
        }
        self.commit_width(window, cx);
        let width = self.geometry(cx).ok().map(|(width, _)| width);
        if let Some(width) = width {
            self.remembered_widths[current as usize] = Some(width);
        }
        let equivalent = self.remembered_widths[target as usize]
            .or_else(|| width.and_then(|width| current.width_for_projection(width, target)))
            .unwrap_or(LIVE_PREVIEW_SIZE);
        let new_width = fit_width(target, equivalent, equivalent);
        self.selected_projection = target;
        self.committed_width = new_width;
        self.width.update(cx, |input, cx| {
            input.set_value(new_width.to_string(), window, cx)
        });
        self.error = None;
        self.refresh_preview(false, cx);
    }

    pub(super) fn preferences(&self, cx: &App) -> Option<OutputPreferences> {
        Some(OutputPreferences {
            width: self.geometry(cx).ok()?.0,
            projection: self.selected_projection,
            format: *self.format.read(cx).selected_value()?,
        })
    }

    pub(super) fn request(&self, cx: &App) -> Result<FileKind, String> {
        let (width, _) = self.geometry(cx)?;
        let (rotation, _) = self.view.values(cx)?;
        let format = *self
            .format
            .read(cx)
            .selected_value()
            .ok_or("Choose a file format.")?;
        Ok(FileKind::Image {
            render: EnvironmentRenderSettings {
                width,
                projection: self.selected_projection,
                source_width_limit: Some(MAX_OUTPUT_WIDTH),
                rotation: if self.export_rotation {
                    rotation
                } else {
                    [0.; 3]
                },
            },
            format,
        })
    }

    pub(super) fn set_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        cx.notify();
    }

    pub(super) fn retire_preview(&mut self) {
        if let Some(ticket) = self.preview_ticket.take() {
            ticket.retire();
        }
    }

    pub(super) fn refresh_preview(&mut self, debounce: bool, cx: &mut Context<Self>) {
        self.retire_preview();
        self.preview_error = None;
        let (Ok((width, _)), Ok((rotation, tone))) = (self.geometry(cx), self.view.values(cx))
        else {
            cx.notify();
            return;
        };
        let maximum = if self.selected_projection == EnvironmentImageProjection::VerticalCross {
            LIVE_PREVIEW_SIZE / 4 * 3
        } else {
            LIVE_PREVIEW_SIZE
        };
        let render = EnvironmentRenderSettings {
            width: width.min(maximum),
            projection: self.selected_projection,
            source_width_limit: Some(MAX_OUTPUT_WIDTH),
            rotation,
        };
        let cached = self
            .last_preview
            .as_ref()
            .filter(|preview| preview.render == render);
        if cached.is_some_and(|preview| preview.tone == tone) {
            cx.notify();
            return;
        }
        let cached = cached.map(|preview| preview.projected.clone());
        let map = self.map.clone();
        let library = self.library.clone();
        let executor = cx.background_executor().clone();
        let ticket = Ticket::new();
        self.preview_ticket = Some(ticket.clone());
        cx.spawn(async move |owner, cx| {
            if debounce {
                executor.timer(Duration::from_millis(120)).await;
            }
            if !ticket.is_current() {
                return;
            }
            let work = ticket.clone();
            let result = executor
                .spawn(async move {
                    let cancelled = || !work.is_current();
                    let projected = match cached {
                        Some(cached) => cached,
                        None => map
                            .render_projected_with::<ForegroundError>(
                                render,
                                generator_execution(),
                                Some(&library),
                                &cancelled,
                            )
                            .map_err(model::failure)?,
                    };
                    let display = tone
                        .apply::<ForegroundError>(&projected, &cancelled)
                        .map_err(model::failure)?;
                    let (_, raster) = map_preview::project(&display, &PreviewSelection::BaseColor)
                        .map_err(model::failure)?;
                    Ok::<_, String>(ProjectedPreview {
                        render,
                        tone,
                        projected,
                        raster,
                    })
                })
                .await;
            let _ = owner.update(cx, |settings, cx| {
                if !ticket.is_current() {
                    return;
                }
                settings.preview_ticket = None;
                match result {
                    Ok(preview) => settings.last_preview = Some(preview),
                    Err(error) => {
                        settings.preview_error =
                            Some(format!("Couldn’t render the preview: {error}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_view_settings(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let [heading, pitch, roll] = &self.view.rotation;
        let show = self.view_open;
        Collapsible::new()
            .open(show)
            .child(
                Button::new("environment-view-settings")
                    .label(if show {
                        "Hide view settings"
                    } else {
                        "View settings"
                    })
                    .small()
                    .on_click(cx.listener(|settings, _, _, cx| {
                        settings.view_open = !settings.view_open;
                        cx.notify();
                    })),
            )
            .content(
                div()
                    .v_flex()
                    .gap_2()
                    .child(
                        Form::new()
                            .columns(3)
                            .child(
                                Field::new().label("Heading").child(
                                    Input::new(heading)
                                        .id("environment-view-heading")
                                        .aria_label("View heading"),
                                ),
                            )
                            .child(
                                Field::new().label("Pitch").child(
                                    Input::new(pitch)
                                        .id("environment-view-pitch")
                                        .aria_label("View pitch"),
                                ),
                            )
                            .child(
                                Field::new().label("Roll").child(
                                    Input::new(roll)
                                        .id("environment-view-roll")
                                        .aria_label("View roll"),
                                ),
                            ),
                    )
                    .child(
                        Form::new()
                            .columns(2)
                            .child(
                                Field::new().label("Preview exposure").child(
                                    Input::new(&self.view.exposure)
                                        .id("environment-view-exposure")
                                        .aria_label("Preview exposure"),
                                ),
                            )
                            .child(
                                Field::new().label("Preview gamma").child(
                                    Input::new(&self.view.gamma)
                                        .id("environment-view-gamma")
                                        .aria_label("Preview gamma"),
                                ),
                            ),
                    )
                    .child(
                        Checkbox::new("environment-export-rotation")
                            .label("Include view rotation in export")
                            .checked(self.export_rotation)
                            .on_change(cx.listener(|settings, checked, _, cx| {
                                settings.export_rotation = *checked;
                                cx.notify();
                            })),
                    ),
            )
    }
}

impl Drop for ExportSettings {
    fn drop(&mut self) {
        self.retire_preview();
    }
}

impl Render for ExportSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let geometry = self.geometry(cx);
        let invalid = geometry
            .as_ref()
            .err()
            .cloned()
            .or_else(|| self.view.values(cx).err());
        let can_preview = invalid.is_none();
        let error = invalid.or_else(|| self.error.clone());
        let dimensions = geometry
            .ok()
            .map(|(width, height)| format!("{width} × {height} px"));
        let theme = cx.theme();
        let compact = window.viewport_size().width < window.rem_size() * 44.;
        let preview = self.last_preview.as_ref();
        let preview_size = preview.map(|preview| {
            let geometry = preview.projected.geometry();
            (geometry.width(), geometry.height())
        });
        let status = if self.preview_ticket.is_some() {
            "Updating preview…".to_owned()
        } else if let Some(error) = &self.preview_error {
            error.clone()
        } else if !can_preview {
            "Preview paused".to_owned()
        } else {
            preview_size.map_or_else(
                || "Preview".into(),
                |(width, height)| format!("Preview · {width} × {height}"),
            )
        };
        let preview = div()
            .v_flex()
            .gap_2()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .id("environment-preview-status")
                    .aria_label(status.clone())
                    .test_support()
                    .text_sm()
                    .text_color(if self.preview_error.is_some() {
                        theme.danger
                    } else {
                        theme.muted_foreground
                    })
                    .child(status),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(rems(14.))
                    .overflow_hidden()
                    .bg(theme.muted)
                    .rounded(theme.radius)
                    .when_some(
                        preview.zip(preview_size),
                        |panel, (preview, (width, height))| {
                            panel
                                .child(checkerboard_for(width as f32 / height as f32))
                                .child(
                                    img(preview.raster.clone())
                                        .id("environment-projected-preview")
                                        .absolute()
                                        .size_full()
                                        .object_fit(ObjectFit::Contain)
                                        .test_support(),
                                )
                        },
                    ),
            );
        let controls = div()
            .v_flex()
            .gap_2()
            .flex_1()
            .min_w_0()
            .child(
                Form::new()
                    .child(
                        Field::new().label("Projection").child(
                            Select::new(&self.projection)
                                .id("environment-export-projection")
                                .accessibility_label("Projection"),
                        ),
                    )
                    .child(
                        Field::new().label("Width").child(
                            Input::new(&self.width)
                                .id("environment-export-width")
                                .aria_label("Width in pixels"),
                        ),
                    )
                    .child(
                        Field::new().label("Format").child(
                            Select::new(&self.format)
                                .id("environment-export-format")
                                .accessibility_label("Format"),
                        ),
                    ),
            )
            .when_some(dimensions, |root, dimensions| {
                root.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(dimensions),
                )
            })
            .when_some(error, |root, error| {
                root.child(
                    div()
                        .id("environment-export-error")
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error),
                )
            });
        let view = self.render_view_settings(cx);
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .flex()
                    .gap_4()
                    .when(compact, |body| body.flex_col())
                    .child(preview)
                    .child(controls),
            )
            .child(view)
    }
}
