//! Browse and manage curve presets for the graph inspector.
use super::{super::preset_files as files, *};
use gpui_kit::component::{
    IndexPath,
    radio::{Radio, RadioGroup},
    select::{SearchableVec, Select, SelectEvent, SelectGroup, SelectItem, SelectState},
};
use gpui_kit::{SharedString, WeakEntity, rems};
use std::{collections::BTreeMap, fs};

#[derive(Clone)]
struct Folders {
    user: Option<PathBuf>,
    factory: Option<PathBuf>,
}

impl gpui_kit::Global for Folders {}

impl Folders {
    fn current(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_else(|| Self {
            user: configured_folder("MYCELA_CURVE_PRESETS")
                .or_else(|| dirs::data_dir().map(|path| path.join("Mycela/Curve Presets"))),
            factory: configured_folder("MYCELA_CURVE_FACTORY_PRESETS"),
        })
    }
}

fn configured_folder(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

#[derive(Clone)]
struct Choice {
    path: PathBuf,
    name: SharedString,
    category: SharedString,
    factory: bool,
    stamp: Option<files::FileStamp>,
}

impl PartialEq for Choice {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

impl SelectItem for Choice {
    type Value = Self;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &Self {
        self
    }

    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.name.to_lowercase().contains(&query) || self.category.to_lowercase().contains(&query)
    }

    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        div().h_flex().gap_2().child(self.title()).child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(if self.factory { "Factory" } else { "User" }),
        )
    }
}

fn discover(folders: Folders, ticket: &Ticket) -> (Vec<SelectGroup<Choice>>, Option<String>) {
    let mut groups: BTreeMap<String, Vec<Choice>> = BTreeMap::new();
    let mut warning = None;
    let factory = folders
        .factory
        .as_deref()
        .and_then(|path| match path.canonicalize() {
            Ok(path) => Some(path),
            Err(error) => {
                warning = Some(format!("Couldn’t open the factory curve folder. {error}"));
                None
            }
        });
    let user = folders
        .user
        .as_deref()
        .and_then(|path| match path.canonicalize() {
            Ok(path)
                if factory
                    .as_ref()
                    .is_none_or(|factory| !path.starts_with(factory)) =>
            {
                Some(path)
            }
            Ok(_) => None,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    warning = Some(format!("Couldn’t open the user curve folder. {error}"));
                }
                None
            }
        });
    for root in [factory.as_ref(), user.as_ref()].into_iter().flatten() {
        if let Err(error) = discover_folder(root, factory.as_deref(), &mut groups, ticket) {
            warning = Some(format!("Couldn’t read {}. {error}", root.display()));
        }
    }
    let groups = groups
        .into_iter()
        .map(|(category, mut choices)| {
            choices.sort_by_cached_key(|item| {
                (item.name.to_lowercase(), !item.factory, item.path.clone())
            });
            choices.dedup_by(|a, b| a.path == b.path);
            let mut group = SelectGroup::new(category);
            group.items = choices;
            group
        })
        .collect();
    (groups, warning)
}

fn discover_folder(
    root: &Path,
    factory: Option<&Path>,
    groups: &mut BTreeMap<String, Vec<Choice>>,
    ticket: &Ticket,
) -> std::io::Result<()> {
    let mut folders = vec![(root.to_path_buf(), "Uncategorized".to_owned(), true)];
    while let Some((dir, category, at_root)) = folders.pop() {
        for entry in fs::read_dir(dir)? {
            if !ticket.is_current() {
                return Ok(());
            }
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            let name = entry.file_name();
            if at_root && kind.is_dir() && !name.to_string_lossy().starts_with('.') {
                folders.push((path, name.to_string_lossy().into_owned(), false));
            } else if kind.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("curv"))
                && let Some(name) = path.file_stem()
            {
                groups.entry(category.clone()).or_default().push(Choice {
                    factory: factory.is_some_and(|factory| path.starts_with(factory)),
                    stamp: entry
                        .metadata()
                        .ok()
                        .map(|meta| files::FileStamp::read(&meta)),
                    name: name.to_string_lossy().into_owned().into(),
                    category: category.clone().into(),
                    path,
                });
            }
        }
    }
    Ok(())
}

type Choices = SelectState<SearchableVec<SelectGroup<Choice>>>;

struct Preview {
    path: PathBuf,
    curve: Curve,
    samples: Result<Vec<f32>, String>,
}

struct Overwrite {
    path: PathBuf,
    stamp: files::FileStamp,
    factory: Option<PathBuf>,
    writer: BinaryWriter,
}

enum SaveOutcome {
    Saved(PathBuf),
    Existing(Overwrite),
}

enum Confirmation {
    Delete(Choice),
    Overwrite(Overwrite),
}

impl Confirmation {
    fn prompt(&self) -> (String, &'static str, &'static str) {
        match self {
            Self::Delete(item) => (
                format!("Delete ‘{}’ from the user preset folder?", item.name),
                "curve-library-confirm-delete",
                "Delete preset",
            ),
            Self::Overwrite(item) => {
                let name = item
                    .path
                    .file_stem()
                    .unwrap_or(item.path.as_os_str())
                    .to_string_lossy();
                let category = item
                    .path
                    .parent()
                    .and_then(Path::file_name)
                    .unwrap_or_default()
                    .to_string_lossy();
                (
                    format!("Overwrite ‘{name}’ in ‘{category}’ with the current curve?"),
                    "curve-library-confirm-overwrite",
                    "Overwrite preset",
                )
            }
        }
    }
}

struct Browser {
    window: AnyWindowHandle,
    folders: Folders,
    graph: WeakEntity<GraphDocumentView>,
    target: AnimationTarget,
    curve: Curve,
    choices: Entity<Choices>,
    ticket: Option<Ticket>,
    preview_ticket: Option<Ticket>,
    preview: Option<Preview>,
    activity: Option<&'static str>,
    has_entries: bool,
    insert: bool,
    save_open: bool,
    category: Entity<InputState>,
    name: Entity<InputState>,
    confirmation: Option<Confirmation>,
    error: Option<String>,
    _selection: Subscription,
}

impl Browser {
    fn new(
        graph: WeakEntity<GraphDocumentView>,
        target: AnimationTarget,
        curve: Curve,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let choices = cx.new(|cx| {
            SelectState::new(SearchableVec::new(Vec::new()), None, window, cx).searchable(true)
        });
        let selection = cx.subscribe(&choices, |browser, _, event, cx| {
            if matches!(event, SelectEvent::Confirm(Some(_))) {
                browser.error = None;
                browser.confirmation = None;
                browser.load_preview(cx);
            }
        });
        Self {
            window: window.window_handle(),
            folders: Folders::current(cx),
            graph,
            target,
            curve,
            choices,
            ticket: None,
            preview_ticket: None,
            preview: None,
            activity: None,
            has_entries: false,
            insert: false,
            save_open: false,
            category: cx.new(|cx| InputState::new(window, cx).default_value("My curves")),
            name: cx.new(|cx| InputState::new(window, cx)),
            confirmation: None,
            error: None,
            _selection: selection,
        }
    }

    fn finish(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
        self.activity = None;
    }

    fn clear_preview(&mut self) {
        if let Some(ticket) = self.preview_ticket.take() {
            ticket.retire();
        }
        self.preview = None;
    }

    fn begin(&mut self, activity: &'static str, cx: &mut Context<Self>) -> Ticket {
        self.finish();
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.activity = Some(activity);
        self.error = None;
        self.confirmation = None;
        cx.notify();
        ticket
    }

    fn reload(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let path = path.or_else(|| self.selected(cx).map(|item| item.path.clone()));
        self.clear_preview();
        let folders = self.folders.clone();
        let ticket = self.begin("Loading curve presets…", cx);
        let window = self.window;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let worker = ticket.clone();
            let (groups, warning) = executor
                .spawn(async move { discover(folders, &worker) })
                .await;
            if !ticket.is_current() {
                return;
            }
            let _ = cx.update_window(window, |_, window, cx| {
                owner.update(cx, |browser, cx| {
                    browser.finish();
                    browser.error = warning;
                    browser.has_entries = !groups.is_empty();
                    let selected = groups.iter().enumerate().find_map(|(section, group)| {
                        group
                            .items
                            .iter()
                            .position(|item| Some(&item.path) == path.as_ref())
                            .map(|row| IndexPath {
                                section,
                                row,
                                column: 0,
                            })
                    });
                    browser.choices.update(cx, |state, cx| {
                        state.set_items(SearchableVec::new(groups), window, cx);
                        state.set_selected_index(
                            selected.or_else(|| browser.has_entries.then(Default::default)),
                            window,
                            cx,
                        );
                    });
                    browser.load_preview(cx);
                })
            });
        })
        .detach();
    }

    fn selected<'a>(&self, cx: &'a App) -> Option<&'a Choice> {
        self.choices.read(cx).selected_value()
    }

    fn load_preview(&mut self, cx: &mut Context<Self>) {
        self.clear_preview();
        if let Some(path) = self.selected(cx).map(|choice| choice.path.clone()) {
            let ticket = Ticket::new();
            self.preview_ticket = Some(ticket.clone());
            let executor = cx.background_executor().clone();
            cx.spawn(async move |owner, cx| {
                let result = executor
                    .spawn(async move {
                        let curve = read_preset(&path)?;
                        let samples = crate::tone_curve::samples(&curve);
                        Ok::<_, String>(Preview {
                            path,
                            curve,
                            samples,
                        })
                    })
                    .await;
                if !ticket.is_current() {
                    return;
                }
                let _ = owner.update(cx, |browser, cx| {
                    browser.clear_preview();
                    match result {
                        Ok(preview) => browser.preview = Some(preview),
                        Err(error) => browser.error = Some(error),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn choose_folder(&mut self, factory: bool, cx: &mut Context<Self>) {
        if self.ticket.is_some() || self.confirmation.is_some() {
            return;
        }
        let label = if factory {
            "Choose factory curve folder"
        } else {
            "Choose user curve folder"
        };
        let response = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(label.into()),
        });
        let ticket = self.begin(label, cx);
        cx.spawn(async move |owner, cx| {
            let result = response.await;
            if !ticket.is_current() {
                return;
            }
            let _ = owner.update(cx, |browser, cx| {
                browser.finish();
                match result {
                    Ok(Ok(Some(mut entries))) if entries.len() == 1 => {
                        if factory {
                            browser.folders.factory = entries.pop();
                        } else {
                            browser.folders.user = entries.pop();
                        }
                        cx.set_global(browser.folders.clone());
                        browser.reload(None, cx);
                    }
                    Ok(Ok(None)) => {}
                    _ => browser.error = Some("Couldn’t select the curve preset folder.".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn encode_current(&self, cx: &App) -> Result<BinaryWriter, String> {
        let current = self
            .graph
            .read_with(cx, |graph, cx| {
                graph.current_preset_target(&self.target, &self.curve, cx)
            })
            .unwrap_or(false);
        if !current || self.curve.layers().is_empty() {
            return Err("Choose a current curve with at least one layer to save.".into());
        }
        let mut writer = BinaryWriter::default();
        self.curve
            .save_preset(&mut writer)
            .map_err(model::failure)?;
        Ok(writer)
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.ticket.is_some() || self.confirmation.is_some() {
            return;
        }
        let name = self.name.read(cx).value().trim().to_owned();
        let category = self.category.read(cx).value().trim().to_owned();
        let request = (|| {
            if !files::valid_name(&name) || !files::valid_name(&category) {
                return Err(
                    "Enter a category and name using ordinary file name characters.".into(),
                );
            }
            let root = self
                .folders
                .user
                .clone()
                .ok_or("Choose a user curve folder first.")?;
            Ok((root, self.encode_current(cx)?))
        })();
        let (root, writer) = match request {
            Ok(values) => values,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let factory = self.folders.factory.clone();
        self.publish_preset(cx, move |ticket| {
            save_or_confirm(&root, factory, &category, &name, writer, ticket)
        });
    }

    fn publish_preset(
        &mut self,
        cx: &mut Context<Self>,
        save: impl FnOnce(&Ticket) -> Result<Option<SaveOutcome>, String> + Send + 'static,
    ) {
        let ticket = self.begin("Saving curve preset…", cx);
        let executor = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let worker = ticket.clone();
            let result = executor.spawn(async move { save(&worker) }).await;
            if !ticket.is_current() {
                return;
            }
            let _ = owner.update(cx, |browser, cx| {
                browser.finish();
                match result {
                    Ok(Some(SaveOutcome::Saved(path))) => {
                        browser.save_open = false;
                        browser.reload(Some(path), cx);
                    }
                    Ok(Some(SaveOutcome::Existing(item))) => {
                        browser.confirmation = Some(Confirmation::Overwrite(item));
                    }
                    Ok(None) => {}
                    Err(error) => browser.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if self.ticket.is_some() {
            return;
        }
        match self.confirmation.take() {
            Some(Confirmation::Delete(item)) if !item.factory => self.delete(item, cx),
            Some(Confirmation::Overwrite(item)) => self.publish_preset(cx, move |ticket| {
                let Overwrite {
                    path,
                    stamp,
                    factory,
                    writer,
                } = item;
                let verify = |path: &Path| {
                    files::verify_user_path(path, factory.as_deref())?;
                    files::verify_file(path, &stamp)
                };
                verify(&path)?;
                model::replace_bytes_if(writer.bytes(), path, ticket, verify)
                    .map(|path| path.map(SaveOutcome::Saved))
            }),
            _ => {}
        }
    }

    fn delete(&mut self, item: Choice, cx: &mut Context<Self>) {
        let Some(stamp) = item.stamp else {
            return;
        };
        let ticket = self.begin("Deleting curve preset…", cx);
        let executor = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let worker = ticket.clone();
            let result = executor
                .spawn(async move { files::delete(&item.path, &stamp, &worker) })
                .await;
            if !ticket.is_current() {
                return;
            }
            let _ = owner.update(cx, |browser, cx| {
                browser.finish();
                match result {
                    Ok(()) => browser.reload(None, cx),
                    Err(error) => browser.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ready(&self, cx: &App) -> Option<&Preview> {
        let path = &self.selected(cx)?.path;
        (!self.save_open && self.ticket.is_none() && self.confirmation.is_none())
            .then_some(self.preview.as_ref()?)
            .filter(|preview| &preview.path == path)
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.finish();
        self.clear_preview();
    }
}

impl Render for Browser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected(cx).cloned();
        let busy = self.ticket.is_some();
        let confirming = self.confirmation.is_some();
        let updating_preview = self.preview_ticket.is_some();
        let deletable = selected
            .as_ref()
            .filter(|item| !item.factory && item.stamp.is_some());
        let theme = cx.theme();
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("curve-library-user")
                            .small()
                            .label("Choose user folder…")
                            .disabled(busy || confirming)
                            .on_click(
                                cx.listener(|browser, _, _, cx| browser.choose_folder(false, cx)),
                            ),
                    )
                    .child(
                        Button::new("curve-library-factory")
                            .small()
                            .label("Choose factory folder…")
                            .disabled(busy || confirming)
                            .on_click(
                                cx.listener(|browser, _, _, cx| browser.choose_folder(true, cx)),
                            ),
                    )
                    .child(
                        Button::new("curve-library-refresh")
                            .small()
                            .label("Refresh")
                            .disabled(busy || confirming)
                            .on_click(cx.listener(|browser, _, _, cx| browser.reload(None, cx))),
                    ),
            )
            .when_some(self.folders.user.as_ref(), |body, folder| {
                body.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(format!("User folder · {}", folder.display())),
                )
            })
            .child(
                Field::new().label("Preset").child(
                    Select::new(&self.choices)
                        .id("curve-library-choice")
                        .accessibility_label("Curve preset")
                        .disabled(busy || !self.has_entries || confirming),
                ),
            )
            .when_some(selected.as_ref(), |body, item| {
                let origin = if item.factory { "Factory" } else { "User" };
                body.child(
                    div()
                        .id("curve-library-selected")
                        .test_support()
                        .aria_label(format!("{}, {}, {origin}", item.name, item.category))
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(format!("{} · {origin}", item.category)),
                )
            })
            .when_some(self.preview.as_ref(), |body, preview| {
                match &preview.samples {
                    Ok(samples) => body.child(
                        div()
                            .id("curve-library-preview")
                            .test_support()
                            .aria_label("Preset curve preview")
                            .h(rems(8.))
                            .px_2()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .child(crate::tone_curve::graph(samples.clone(), cx)),
                    ),
                    Err(error) => body.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(error.clone()),
                    ),
                }
            })
            .when(self.has_entries, |body| {
                body.child(
                    RadioGroup::horizontal("curve-library-mode")
                        .child(Radio::new("curve-library-replace").label("Replace layers"))
                        .child(Radio::new("curve-library-insert").label("Insert layers"))
                        .selected_index(Some(usize::from(self.insert)))
                        .disabled(busy || confirming)
                        .on_change(cx.listener(|browser, index, _, cx| {
                            browser.insert = *index == 1;
                            cx.notify();
                        })),
                )
            })
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("curve-library-save-form")
                            .small()
                            .ghost()
                            .label(if self.save_open {
                                "Hide save fields"
                            } else {
                                "Save current curve…"
                            })
                            .disabled(
                                busy || confirming
                                    || self.folders.user.is_none()
                                    || self.curve.layers().is_empty(),
                            )
                            .on_click(cx.listener(|browser, _, _, cx| {
                                browser.save_open = !browser.save_open;
                                browser.error = None;
                                cx.notify();
                            })),
                    )
                    .when_some(deletable.filter(|_| !confirming), |row, item| {
                        let item = item.clone();
                        row.child(
                            Button::new("curve-library-delete")
                                .small()
                                .ghost()
                                .label("Delete preset…")
                                .disabled(busy)
                                .on_click(cx.listener(move |browser, _, _, cx| {
                                    browser.save_open = false;
                                    browser.confirmation = Some(Confirmation::Delete(item.clone()));
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .when(self.save_open && !confirming, |body| {
                body.child(
                    Form::new()
                        .columns(2)
                        .child(
                            Field::new().label("Category").child(
                                Input::new(&self.category)
                                    .id("curve-library-category")
                                    .disabled(busy)
                                    .aria_label("Preset category"),
                            ),
                        )
                        .child(
                            Field::new().label("Name").child(
                                Input::new(&self.name)
                                    .id("curve-library-name")
                                    .disabled(busy)
                                    .aria_label("Preset name"),
                            ),
                        ),
                )
                .child(
                    Button::new("curve-library-save")
                        .small()
                        .label("Save preset")
                        .disabled(busy)
                        .on_click(cx.listener(|browser, _, _, cx| browser.save(cx))),
                )
            })
            .when_some(self.confirmation.as_ref(), |body, confirmation| {
                let (prompt, button, label) = confirmation.prompt();
                body.child(
                    div()
                        .id("curve-library-confirmation")
                        .test_support()
                        .aria_label(prompt.clone())
                        .child(prompt),
                )
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new("curve-library-keep")
                                .small()
                                .label("Keep preset")
                                .on_click(cx.listener(|browser, _, _, cx| {
                                    browser.confirmation = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(button)
                                .small()
                                .danger()
                                .label(label)
                                .on_click(cx.listener(|browser, _, _, cx| browser.confirm(cx))),
                        ),
                )
            })
            .when_some(
                self.activity
                    .or(updating_preview.then_some("Loading curve preview…")),
                |body, status| {
                    body.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(status),
                    )
                },
            )
            .when(!busy && !self.has_entries && self.error.is_none(), |body| {
                body.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Choose a folder containing .curv files or preset categories."),
                )
            })
            .when_some(self.error.as_ref(), |body, error| {
                body.child(
                    div()
                        .id("curve-library-error")
                        .aria_label(error.clone())
                        .test_support()
                        .role(gpui_kit::Role::Alert)
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error.clone()),
                )
            })
            .child(
                DialogFooter::new()
                    .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                    .child(
                        DialogAction::new().child(
                            Button::new("ok")
                                .primary()
                                .label("Apply preset")
                                .disabled(self.ready(cx).is_none()),
                        ),
                    ),
            )
    }
}

impl GraphDocumentView {
    pub(super) fn open_curve_library(
        &mut self,
        target: AnimationTarget,
        curve: Curve,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let graph = cx.weak_entity();
        let browser = cx.new(|cx| Browser::new(graph.clone(), target, curve, window, cx));
        browser.update(cx, |browser, cx| browser.reload(None, cx));
        self.pause_preview();
        self.modal = true;
        window.open_dialog(cx, move |dialog, window, _| {
            let confirm = browser.clone();
            let close = browser.clone();
            let close_graph = graph.clone();
            dialog
                .title("Curve presets")
                .width(window.rem_size() * 31.)
                .child(browser.clone())
                .on_ok(move |_, window, cx| {
                    let browser = confirm.read(cx);
                    let Some(preset) = browser.ready(cx).map(|preview| preview.curve.clone())
                    else {
                        return false;
                    };
                    let target = browser.target.clone();
                    let curve = browser.curve.clone();
                    let action = if browser.insert {
                        PresetAction::Insert
                    } else {
                        PresetAction::Replace
                    };
                    let result = browser
                        .graph
                        .clone()
                        .update(cx, |graph, cx| {
                            graph.error = None;
                            graph.status = None;
                            graph.apply_curve_preset(&target, &curve, preset, action, window, cx);
                            graph.error.take().map_or(Ok(()), Err)
                        })
                        .unwrap_or_else(|_| Err("The graph window closed.".into()));
                    if let Err(error) = result {
                        confirm.update(cx, |browser, cx| {
                            browser.error = Some(error);
                            cx.notify();
                        });
                        return false;
                    }
                    true
                })
                .on_close(move |_, _, cx| {
                    close.update(cx, |browser, _| {
                        browser.finish();
                        browser.clear_preview();
                    });
                    let _ = close_graph.update(cx, |graph, cx| {
                        graph.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }
}

fn save_or_confirm(
    root: &Path,
    factory: Option<PathBuf>,
    category: &str,
    name: &str,
    writer: BinaryWriter,
    ticket: &Ticket,
) -> Result<Option<SaveOutcome>, String> {
    if !ticket.is_current() {
        return Ok(None);
    }
    if !root.try_exists().map_err(model::failure)? {
        // The default user library may not exist yet. Check the resolved parent before creating it.
        let ancestor = root
            .ancestors()
            .find(|path| path.is_dir())
            .ok_or("Choose an existing user folder.")?;
        let path = ancestor.canonicalize().map_err(model::failure)?;
        files::verify_user_path(&path, factory.as_deref())?;
        fs::create_dir_all(root).map_err(model::failure)?;
    }
    match files::destination(
        root,
        factory.as_deref(),
        category,
        &format!("{name}.curv"),
        ticket,
    )? {
        Some(files::Destination::Vacant(path)) => {
            files::verify_parent(&path)?;
            model::save_new_bytes(writer.bytes(), path, ticket)
                .map(|path| path.map(SaveOutcome::Saved))
        }
        Some(files::Destination::Existing(path)) => {
            files::verify_parent(&path)?;
            let metadata = fs::symlink_metadata(&path).map_err(model::failure)?;
            if !metadata.is_file() {
                return Err("Choose the name of a regular preset file.".into());
            }
            let stamp = files::FileStamp::read(&metadata);
            Ok(Some(SaveOutcome::Existing(Overwrite {
                path,
                stamp,
                factory,
                writer,
            })))
        }
        None => Ok(None),
    }
}
