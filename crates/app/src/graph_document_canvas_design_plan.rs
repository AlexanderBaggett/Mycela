//! External reference images used only by the Canvas editor.
use super::GraphDocumentView;
use gpui_kit::base::{Disableable, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    AnyElement, Context, ObjectFit, PathPromptOptions, RenderImage, div, img, prelude::*,
};
use image::{DynamicImage, ImageFormat, ImageReader, RgbaImage, imageops};
use mycela_engine::CanvasDesignPlan;
use std::{
    fs::File,
    io::{Cursor, Read},
    path::Path,
    sync::Arc,
};

#[derive(Default)]
pub(super) struct DesignPlanView {
    scope: Arc<()>,
    load: Option<PlanLoad>,
    hidden: bool,
    choosing: bool,
}

struct PlanLoad {
    plan: CanvasDesignPlan,
    images: Option<Result<PlanImages, String>>,
}

struct PlanImages {
    background: Arc<RenderImage>,
    overlay: Arc<RenderImage>,
}

impl GraphDocumentView {
    pub(in super::super) fn refresh_canvas_design_plan(&mut self, cx: &mut Context<Self>) {
        let Some(tool) = self.drawing.as_mut() else {
            return;
        };
        let state = &mut tool.design_plan;
        let plan = tool
            .target
            .filter()
            .and_then(|filter| filter.canvas_design_plan());
        if state.load.as_ref().map(|load| &load.plan) == plan.as_ref() {
            return;
        }
        state.load = plan.as_ref().map(|plan| PlanLoad {
            plan: plan.clone(),
            images: None,
        });
        let Some(plan) = plan else {
            state.hidden = false;
            return;
        };
        let scope = state.scope.clone();
        let background = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let requested = plan.clone();
            let result = background
                .spawn(async move {
                    let path = requested
                        .path()
                        .ok_or("The design plan has no file path.")?;
                    load_image(Path::new(&path), Some(&requested)).map(|(_, images)| images)
                })
                .await;
            let _ = owner.update(cx, |view, cx| {
                if let Some(tool) = &mut view.drawing
                    && Arc::ptr_eq(&tool.design_plan.scope, &scope)
                    && let Some(load) = &mut tool.design_plan.load
                    && load.plan == plan
                    && load.images.is_none()
                {
                    load.images = Some(result);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn choose_canvas_design_plan(&mut self, cx: &mut Context<Self>) {
        if !self.drawing_current(cx) {
            return;
        }
        let Some(tool) = self
            .drawing
            .as_mut()
            .filter(|tool| !tool.design_plan.choosing)
        else {
            return;
        };
        tool.design_plan.choosing = true;
        let scope = tool.design_plan.scope.clone();
        let revision = self.revision;
        let handle = self.window;
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a Canvas design plan image".into()),
        });
        let background = cx.background_executor().clone();
        cx.notify();
        cx.spawn(async move |owner, cx| {
            let result = match prompt.await {
                Ok(Ok(Some(mut paths))) if paths.len() == 1 => {
                    let path = paths.remove(0);
                    background
                        .spawn(async move { load_image(&path, None).map(Some) })
                        .await
                }
                Ok(Ok(None)) => Ok(None),
                Ok(Ok(Some(_))) => Err("Choose one design plan image.".into()),
                Ok(Err(error)) => Err(format!("Couldn’t open the file picker: {error}")),
                Err(_) => Err("The file picker closed before returning a result.".into()),
            };
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    let Some(tool) = view
                        .drawing
                        .as_mut()
                        .filter(|tool| Arc::ptr_eq(&tool.design_plan.scope, &scope))
                    else {
                        return;
                    };
                    tool.design_plan.choosing = false;
                    cx.notify();
                    if view.revision != revision || !view.drawing_current(cx) {
                        return;
                    }
                    match result {
                        Ok(Some((plan, images))) => {
                            if view.has_field_drafts(cx) {
                                view.error = Some(
                                    "Correct or reset field text before changing the design plan."
                                        .into(),
                                );
                                return;
                            }
                            if let Some(tool) = &mut view.drawing {
                                tool.design_plan.hidden = false;
                                tool.design_plan.load = Some(PlanLoad {
                                    plan: plan.clone(),
                                    images: Some(Ok(images)),
                                });
                            }
                            view.apply_canvas_shape_change(
                                false,
                                |filter| filter.edit_canvas_design_plan(Some(plan)),
                                window,
                                cx,
                            );
                            view.refresh_canvas_design_plan(cx);
                        }
                        Ok(None) => {}
                        Err(error) => view.error = Some(error),
                    }
                });
            });
        })
        .detach();
    }

    pub(super) fn render_design_plan_menu(
        &self,
        disabled: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let state = &self.drawing.as_ref()?.design_plan;
        let has_plan = state.load.is_some();
        let hide = !state.hidden;
        let scope = state.scope.clone();
        let owner = cx.weak_entity();
        Some(
            Button::new("texture-canvas-design-plan")
                .small()
                .ghost()
                .label(if state.choosing {
                    "Opening design plan…"
                } else {
                    "Design plan"
                })
                .disabled(disabled || state.choosing)
                .dropdown_menu(move |menu, _, _| {
                    let set = owner.clone();
                    let toggle = owner.clone();
                    let clear = owner.clone();
                    let toggle_scope = scope.clone();
                    let clear_scope = scope.clone();
                    let set_scope = scope.clone();
                    menu.item(
                        PopupMenuItem::new(if has_plan {
                            "Replace image…"
                        } else {
                            "Set image…"
                        })
                        .on_click(move |_, _, cx| {
                            let _ = set.update(cx, |view, cx| {
                                if view.drawing.as_ref().is_some_and(|tool| {
                                    Arc::ptr_eq(&tool.design_plan.scope, &set_scope)
                                }) {
                                    view.choose_canvas_design_plan(cx);
                                }
                            });
                        }),
                    )
                    .item(
                        PopupMenuItem::new(if hide { "Hide image" } else { "Show image" })
                            .disabled(!has_plan)
                            .on_click(move |_, _, cx| {
                                let _ = toggle.update(cx, |view, cx| {
                                    if let Some(tool) = &mut view.drawing
                                        && Arc::ptr_eq(&tool.design_plan.scope, &toggle_scope)
                                    {
                                        tool.design_plan.hidden = hide;
                                        cx.notify();
                                    }
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("Clear image")
                            .disabled(!has_plan)
                            .on_click(move |_, window, cx| {
                                let _ = clear.update(cx, |view, cx| {
                                    if view.drawing.as_ref().is_some_and(|tool| {
                                        Arc::ptr_eq(&tool.design_plan.scope, &clear_scope)
                                    }) && view.drawing_current(cx)
                                    {
                                        view.apply_canvas_shape_change(
                                            false,
                                            |filter| filter.edit_canvas_design_plan(None),
                                            window,
                                            cx,
                                        );
                                    }
                                });
                            }),
                    )
                })
                .into_any_element(),
        )
    }

    pub(in super::super) fn render_design_plan_image(&self, overlay: bool) -> Option<AnyElement> {
        let state = &self.drawing.as_ref()?.design_plan;
        let images = state.load.as_ref()?.images.as_ref()?.as_ref().ok()?;
        if state.hidden {
            return None;
        }
        Some(
            img(if overlay {
                images.overlay.clone()
            } else {
                images.background.clone()
            })
            .absolute()
            .size_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        )
    }

    pub(in super::super) fn render_design_plan_status(
        &self,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let load = self.drawing.as_ref()?.design_plan.load.as_ref()?;
        let (text, color) = match &load.images {
            None => ("Loading design plan…", cx.theme().muted_foreground),
            Some(Err(error)) => (error.as_str(), cx.theme().danger),
            Some(Ok(_)) => return None,
        };
        Some(
            div()
                .id("texture-canvas-design-plan-status")
                .test_support()
                .text_sm()
                .text_color(color)
                .child(text.to_owned())
                .into_any_element(),
        )
    }
}

fn load_image(
    path: &Path,
    expected: Option<&CanvasDesignPlan>,
) -> Result<(CanvasDesignPlan, PlanImages), String> {
    let metadata = path
        .metadata()
        .map_err(|error| format!("Couldn’t find the design plan image: {error}"))?;
    let limit = i32::MAX as u64;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("Choose a regular design plan image file within the image size limit.".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(limit + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("Couldn’t read the design plan image: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err("The design plan image exceeds the image size limit.".into());
    }
    let plan = match expected {
        Some(plan) if plan.matches_image(&bytes) => plan.clone(),
        Some(_) => {
            return Err(
                "The design plan image has changed. Replace the image to update the reference."
                    .into(),
            );
        }
        None => CanvasDesignPlan::new(
            path.to_str()
                .ok_or("The image path must be valid Unicode.")?,
            &bytes,
        ),
    };
    let mut reader = ImageReader::new(Cursor::new(&bytes));
    if let Ok(format) = ImageFormat::from_path(path) {
        reader.set_format(format);
    }
    let reader = reader
        .with_guessed_format()
        .map_err(|error| format!("Couldn’t identify the design plan image: {error}"))?;
    let decoded = reader
        .decode()
        .map_err(|error| format!("Couldn’t decode the design plan image: {error}"))?;
    Ok((
        plan,
        make_plan_images(decoded, crate::LIVE_PREVIEW_SIZE as u32),
    ))
}

fn make_plan_images(decoded: DynamicImage, cap: u32) -> PlanImages {
    let (width, height) = (decoded.width(), decoded.height());
    let maximum = width.max(height);
    let image = if maximum > cap {
        let fit = |side| (u64::from(side) * u64::from(cap) / u64::from(maximum)).max(1) as u32;
        decoded
            .resize_exact(fit(width), fit(height), imageops::FilterType::Triangle)
            .into_rgba8()
    } else {
        decoded.into_rgba8()
    };
    let size = image.width().max(image.height());
    let mut bgra = RgbaImage::new(size, size);
    imageops::replace(
        &mut bgra,
        &image,
        i64::from((size - image.width()) / 2),
        i64::from((size - image.height()) / 2),
    );
    for pixel in bgra.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    let image = |pixels| Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
    let background = image(bgra.clone());
    for pixel in bgra.pixels_mut() {
        pixel.0[3] = ((u16::from(pixel.0[3]) * 2 + 2) / 5) as u8;
    }
    PlanImages {
        background,
        overlay: image(bgra),
    }
}

#[cfg(test)]
#[test]
fn canvas_design_plan_padding_and_alpha_match_original() {
    let evidence = include_str!("../../../../port/evidence/canvas-design-plan-original.tsv");
    let dimensions = |field: &str| {
        let (width, height) = field.split_once('x').unwrap();
        (
            width.parse::<u32>().unwrap(),
            height.parse::<u32>().unwrap(),
        )
    };
    for row in evidence.lines().skip(2) {
        let [name, source, limit, tile, _, coverage, center_transparency]: [&str; 7] =
            row.split('\t').collect::<Vec<_>>().try_into().unwrap();
        let (width, height) = dimensions(source);
        let plan = make_plan_images(
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(
                width,
                height,
                image::Rgba([210, 100, 30, 128]),
            )),
            limit.parse().unwrap(),
        );
        let (side, _) = dimensions(tile);
        let background = plan.background.as_bytes(0).unwrap();
        let overlay = plan.overlay.as_bytes(0).unwrap();
        let expected_alpha =
            ((1. - center_transparency.parse::<f64>().unwrap()) * 255.).round() as u8;
        for ((pixel, faded), expected) in background
            .chunks_exact(4)
            .zip(overlay.chunks_exact(4))
            .zip(coverage.bytes().filter(|byte| *byte != b'/'))
        {
            if expected == b'#' {
                assert_eq!(pixel, [30, 100, 210, expected_alpha], "{name}");
                assert_eq!(faded, [30, 100, 210, 51], "{name}");
            } else {
                assert_eq!(pixel, [0; 4], "{name}");
                assert_eq!(faded, [0; 4], "{name}");
            }
        }
        assert_eq!(background.len(), (side * side * 4) as usize, "{name}");
    }
}
