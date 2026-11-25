//! Standalone preset I/O for graph curves and animation axes.
#[path = "graph_document_curve_library.rs"]
mod library;

use super::{animation::AnimationTarget, *};
use mycela_engine::{BinaryCursor, BinaryWriter, SharedCurve};

type Curve = SharedCurve<ForegroundError>;

#[derive(Clone, Copy)]
enum PresetAction {
    Browse,
    Replace,
    Insert,
    Save,
}

enum PresetResult {
    Opened(Curve),
    Saved(PathBuf),
}

impl GraphDocumentView {
    pub(super) fn render_curve_presets(
        &self,
        target: AnimationTarget,
        curve: &Curve,
        disabled: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let owner = cx.entity().downgrade();
        let curve = curve.clone();
        Button::new(format!("{}:presets", target.item.id))
            .small()
            .label("Curve presets")
            .disabled(disabled)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (label, action) in [
                    ("Browse presets…", PresetAction::Browse),
                    ("Replace layers from file…", PresetAction::Replace),
                    ("Insert layers from file…", PresetAction::Insert),
                    ("Save preset…", PresetAction::Save),
                ] {
                    let owner = owner.clone();
                    let target = target.clone();
                    let curve = curve.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .disabled(
                                matches!(action, PresetAction::Save) && curve.layers().is_empty(),
                            )
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.choose_curve_preset(
                                        target.clone(),
                                        curve.clone(),
                                        action,
                                        window,
                                        cx,
                                    )
                                });
                            }),
                    );
                }
                menu
            })
    }

    fn choose_curve_preset(
        &mut self,
        target: AnimationTarget,
        curve: Curve,
        action: PresetAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() || !self.current_preset_target(&target, &curve, cx) {
            return;
        }
        if matches!(action, PresetAction::Browse) {
            self.open_curve_library(target, curve, window, cx);
            return;
        }
        let save = matches!(action, PresetAction::Save);
        let writer = if save {
            let mut writer = BinaryWriter::default();
            if let Err(error) = curve.save_preset(&mut writer) {
                self.error = Some(format!("Couldn’t encode the curve preset. {error}"));
                cx.notify();
                return;
            }
            Some(writer)
        } else {
            None
        };
        let directory = self
            .path
            .as_deref()
            .or(self.imported_from.as_deref())
            .and_then(Path::parent)
            .unwrap_or(Path::new("."));
        let save_picker = save.then(|| cx.prompt_for_new_path(directory, Some("curve.curv")));
        let open_picker = (!save).then(|| {
            cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Open curve preset".into()),
            })
        });
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(if save {
            Work::SavingCurvePreset
        } else {
            Work::OpeningCurvePreset
        });
        self.status = None;
        self.error = None;
        let handle = self.window;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let chosen = if let Some(picker) = save_picker {
                picker.await.map(|result| result.map_err(model::failure))
            } else if let Some(picker) = open_picker {
                picker.await.map(|result| match result {
                    Ok(Some(mut paths)) if paths.len() == 1 => Ok(paths.pop()),
                    Ok(Some(_)) => Err("Choose a single curve preset.".into()),
                    Ok(None) => Ok(None),
                    Err(error) => Err(model::failure(error)),
                })
            } else {
                return;
            };
            if !ticket.is_current() {
                return;
            }
            let result = match chosen {
                Ok(Ok(Some(path))) => {
                    let worker = ticket.clone();
                    executor
                        .spawn(async move {
                            if let Some(writer) = writer {
                                write_preset(writer.bytes(), path, &worker)
                                    .map(|path| path.map(PresetResult::Saved))
                            } else {
                                read_preset(&path).map(|curve| Some(PresetResult::Opened(curve)))
                            }
                        })
                        .await
                }
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(format!("Couldn’t choose the preset file. {error}")),
                Err(_) => Err("The file picker closed before returning a result.".into()),
            };
            let _ = cx.update_window(handle, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if !view.owns(&ticket) {
                        return;
                    }
                    view.work = None;
                    view.ticket = None;
                    ticket.retire();
                    match result {
                        Ok(Some(PresetResult::Opened(preset))) => {
                            view.apply_curve_preset(&target, &curve, preset, action, window, cx);
                        }
                        Ok(Some(PresetResult::Saved(path))) => {
                            view.status = Some(format!("Saved curve preset to {}", path.display()));
                        }
                        Ok(None) => {}
                        Err(error) => view.error = Some(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }

    fn current_preset_target(&self, target: &AnimationTarget, curve: &Curve, cx: &App) -> bool {
        let item = &target.item;
        self.is_original_selection(&target.selection, &target.node, target.revision)
            && !self.has_field_drafts(cx)
            && item.animation.as_ref().is_some_and(|expected| {
                item.parameter.retained().is_ok_and(|current| {
                    current.is_some_and(|current| current.same_reference(expected))
                })
            })
            && item
                .parameter
                .axis(item.axis)
                .is_ok_and(|(enabled, current)| {
                    enabled && current.is_some_and(|current| current.same_reference(curve))
                })
    }

    fn apply_curve_preset(
        &mut self,
        target: &AnimationTarget,
        curve: &Curve,
        preset: Curve,
        action: PresetAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.current_preset_target(target, curve, cx) {
            self.error = Some("The selected curve changed while opening the preset.".into());
            return;
        }
        let Some(animation) = target.item.animation.as_ref() else {
            return;
        };
        let insert = matches!(action, PresetAction::Insert);
        if insert && preset.layers().is_empty() {
            self.status = Some("The preset has no layers to insert.".into());
            return;
        }
        self.ensure_history_binding();
        let was_dirty = self.dirty;
        let stamp = self.begin_history_edit();
        self.error = target
            .item
            .parameter
            .apply_curve_preset(
                &self.document,
                target.item.axis,
                animation,
                curve,
                &preset,
                insert,
            )
            .err();
        if self.error.is_some() && self.current_preset_target(target, curve, cx) {
            self.dirty = was_dirty;
            return;
        }
        self.revision += 1;
        self.finish_history_edit(stamp, self.error.is_none());
        self.inspector.layer = None;
        self.rebuild(window, cx);
        self.start_preview(cx);
    }
}

fn read_preset(path: &Path) -> Result<Curve, String> {
    if !path.metadata().map_err(model::failure)?.is_file() {
        return Err("Choose a regular .curv file.".into());
    }
    let bytes =
        std::fs::read(path).map_err(|error| format!("Couldn’t read the curve preset. {error}"))?;
    let mut cursor = BinaryCursor::new(&bytes).map_err(model::failure)?;
    Curve::read_preset(&mut cursor)
        .map_err(|error| format!("Couldn’t open the curve preset. {error}"))
}

fn write_preset(
    bytes: &[u8],
    mut path: PathBuf,
    ticket: &Ticket,
) -> Result<Option<PathBuf>, String> {
    if path.extension().is_none() {
        path.set_extension("curv");
        if path.try_exists().map_err(model::failure)? {
            return Err(format!(
                "Choose the full file name to replace {}.",
                path.display()
            ));
        }
        return model::save_new_bytes(bytes, path, ticket);
    }
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("curv"))
    {
        return Err("Choose a filename ending in .curv.".into());
    }
    model::save_bytes(bytes, path, ticket)
}
