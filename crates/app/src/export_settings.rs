//! Export dimensions are editable text; committed texture state stays with the
//! workspace. The immutable render job is captured only when Export is confirmed.
use gpui_kit::base::{StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme,
    form::{Field, Form},
    input::{Input, InputState},
};
use gpui_kit::{App, AppContext, Context, Entity, IntoElement, Render, Window, div, prelude::*};

pub(crate) fn checked_dimensions(width: i32, height: i32) -> Result<(i32, i32), String> {
    if width <= 0 || height <= 0 {
        return Err("Width and height must be positive whole numbers.".into());
    }
    let pixels = width.checked_mul(height).ok_or_else(||
        "This size exceeds the renderer’s signed 32-bit pixel indexing. Choose dimensions whose product fits that layout.".to_string())?;
    let bytes = usize::try_from(pixels)
        .ok()
        .and_then(|pixels| pixels.checked_mul(4))
        .filter(|bytes| *bytes <= isize::MAX as usize);
    if bytes.is_none() {
        return Err("The RGBA byte layout is not addressable on this platform.".into());
    }
    Ok((width, height))
}

pub(crate) struct ExportSettings {
    width: Entity<InputState>,
    height: Entity<InputState>,
    description: String,
    error: Option<String>,
}
impl ExportSettings {
    pub(crate) fn new(description: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            width: cx.new(|cx| InputState::new(window, cx).default_value("1024")),
            height: cx.new(|cx| InputState::new(window, cx).default_value("1024")),
            description,
            error: None,
        }
    }
    pub(crate) fn dimensions(&self, cx: &App) -> Result<(i32, i32), String> {
        let parse = |input: &Entity<InputState>| {
            input.read(cx).value().trim().parse::<i32>()
            .map_err(|_| "Width and height must be positive whole numbers within the signed 32-bit range.".to_string())
        };
        checked_dimensions(parse(&self.width)?, parse(&self.height)?)
    }
    pub(crate) fn set_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        cx.notify();
    }
}
impl Render for ExportSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("export-image-settings")
            .role(gpui_kit::Role::Group)
            .aria_label("Export image dimensions")
            .test_support()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.description.clone()),
            )
            .child(
                Form::new()
                    .columns(2)
                    .child(
                        Field::new().label("Width").child(
                            Input::new(&self.width)
                                .id("export-image-width")
                                .aria_label("Export width in pixels"),
                        ),
                    )
                    .child(
                        Field::new().label("Height").child(
                            Input::new(&self.height)
                                .id("export-image-height")
                                .aria_label("Export height in pixels"),
                        ),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("export-image-error")
                        .role(gpui_kit::Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
    }
}
