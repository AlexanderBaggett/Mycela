use super::{GraphDocumentView, model};
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Textarea, TextareaState},
    tab::{Tab, TabBar},
};
use gpui_kit::{AnyElement, Context, Entity, Role, Window, div, prelude::*, rems};
use mycela_engine::ShaderProgram;
use std::sync::Arc;

struct ShaderEditor {
    input: Entity<TextareaState>,
    reference: bool,
    error: Option<String>,
}

impl Render for ShaderEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .v_flex()
            .gap_3()
            .child(
                TabBar::new("shader-tabs")
                    .selected_index(usize::from(self.reference))
                    .child(Tab::new().label("Code"))
                    .child(Tab::new().label("Reference"))
                    .on_click(cx.listener(|editor, index, _, cx| {
                        editor.reference = *index == 1;
                        cx.notify();
                    })),
            )
            .child(if self.reference {
                shader_reference(cx)
            } else {
                div()
                    .id("shader-source")
                    .test_support()
                    .child(
                        Textarea::new(&self.input)
                            .aria_label("WGSL source")
                            .h(rems(19.))
                            .font_family("monospace"),
                    )
                    .into_any_element()
            })
            .when_some(
                self.error.as_ref().filter(|_| !self.reference),
                |body, error| {
                    body.child(
                        div()
                            .id("shader-error")
                            .test_support()
                            .role(Role::Alert)
                            .h(rems(6.))
                            .overflow_y_scroll()
                            .text_sm()
                            .font_family("monospace")
                            .text_color(cx.theme().danger)
                            .child(error.clone()),
                    )
                },
            )
    }
}

fn shader_reference(cx: &Context<ShaderEditor>) -> AnyElement {
    let theme = cx.theme();
    let example = "requires swizzle_assignment;\n\n@group(1) @binding(0) var source: texture_2d<f32>;\n\nfn shade(uv: vec2f) -> vec4f {\n    var color = sample_input(source, uv);\n    color.rg = color.gr;\n    return color;\n}";
    div().id("shader-reference").test_support().h(rems(19.)).overflow_y_scroll()
        .v_flex().gap_3().pr_3()
        .children([
            ("Entry point", "Define fn shade(uv: vec2f) -> vec4f. It runs for each pixel. Return red, green, blue, and opacity. Coordinates start at the top left."),
            ("Syntax", "Use let for immutable locals, var for mutable locals, and fn for helpers. WGSL supports f32, i32, u32, bool, vectors such as vec2f and vec4f, matrices, structs, and arrays. Use if, switch, for, while, or loop for control flow."),
            ("Built-in values", "uv is the normalized position at the pixel center. pixel is the position in pixels. resolution holds width and height. texel_size is 1.0 / resolution. time is the graph's sample time from 0 to 1."),
            ("Graph inputs", "Declare each texture with @group(1) @binding(n) var name: texture_2d<f32>; using a distinct binding number. Apply creates up to 16 ports named after these variables. Ports with the same name keep their connections. Disconnected inputs sample as transparent black."),
            ("Functions", "sample_input(image, uv) samples a graph input with linear filtering and repeat wrapping. WGSL also supplies textureLoad and textureDimensions, mix, clamp, smoothstep, fract, sin, cos, length, dot, normalize, and fragment derivatives such as dpdx, dpdy and fwidth."),
            ("Swizzle assignment", "Add requires swizzle_assignment; at the top to assign several vector components together. For example, color.rg = color.gr swaps red and green. Use distinct destination components."),
        ].into_iter().map(|(heading, body)| div().v_flex().gap_1()
            .child(div().font_semibold().child(heading))
            .child(div().text_sm().text_color(theme.muted_foreground).child(body))))
        .child(div().p_2().rounded_md().bg(theme.muted).text_sm()
            .font_family("monospace").child(example))
        .into_any_element()
}

impl GraphDocumentView {
    pub(super) fn render_shader_controls(
        &self,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let node = self.selected_node()?;
        node.shader_program()?;
        Some(
            Button::new("graph-edit-shader")
                .small()
                .label("Edit shader…")
                .disabled(busy)
                .on_click(cx.listener(|view, _, window, cx| {
                    view.open_shader_editor(window, cx);
                }))
                .into_any_element(),
        )
    }

    fn open_shader_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() || self.has_field_drafts(cx) {
            return;
        }
        let Some(node) = self.selected_node() else {
            return;
        };
        let Some(program) = node.shader_program() else {
            return;
        };
        let editor = cx.new(|cx| ShaderEditor {
            input: cx.new(|cx| {
                TextareaState::new(window, cx).default_value(program.source().to_owned())
            }),
            reference: false,
            error: program.is_imported_pixel_script().then(|| {
                "This is the original PixelScript C#. Replace it with WGSL to use this node.".into()
            }),
        });
        let owner = cx.weak_entity();
        let revision = self.revision;
        self.modal = true;
        window.open_dialog(cx, move |dialog, window, _| {
            let inputs = editor.clone();
            let apply = owner.clone();
            let close = owner.clone();
            let target = node.clone();
            dialog
                .title("Shader")
                .width(window.rem_size() * 42.)
                .child(editor.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Apply")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let source = inputs.read(cx).input.read(cx).value();
                    let program = match ShaderProgram::compile(&source) {
                        Ok(program) => Arc::new(program),
                        Err(error) => {
                            inputs.update(cx, |editor, cx| {
                                editor.error = Some(error);
                                editor.reference = false;
                                cx.notify();
                            });
                            return false;
                        }
                    };
                    apply
                        .update(cx, |view, cx| {
                            if view.revision != revision
                                || !view
                                    .selected_node()
                                    .is_some_and(|current| current.same_reference(&target))
                            {
                                return true;
                            }
                            if target
                                .shader_program()
                                .is_some_and(|current| current.source() == program.source())
                            {
                                return true;
                            }
                            view.ensure_history_binding();
                            let stamp = view.begin_history_edit();
                            match target.edit_shader_program(program) {
                                Ok(false) => {}
                                Ok(true) => {
                                    view.revision += 1;
                                    view.error = None;
                                    view.status = None;
                                    view.finish_history_edit(stamp, true);
                                    view.rebuild(window, cx);
                                    view.start_preview(cx);
                                }
                                Err(error) => {
                                    view.finish_history_edit(stamp, false);
                                    inputs.update(cx, |editor, cx| {
                                        editor.error = Some(model::failure(error));
                                        editor.reference = false;
                                        cx.notify();
                                    });
                                    return false;
                                }
                            }
                            true
                        })
                        .unwrap_or(true)
                })
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |view, cx| {
                        view.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }
}
