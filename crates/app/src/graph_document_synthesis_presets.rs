//! Browse Synthesis presets supplied by the user or an existing installation.
#[path = "graph_document_synthesis_preset_files.rs"]
mod files;
#[path = "graph_document_synthesis_preset_organization.rs"]
mod organization;
#[cfg(all(test, target_os = "linux"))]
#[path = "graph_document_synthesis_preset_organization_tests.rs"]
mod organization_tests;

use super::*;
use gpui_kit::component::{
    radio::{Radio, RadioGroup},
    select::{SearchableVec, Select, SelectEvent, SelectGroup, SelectItem, SelectState},
};
use gpui_kit::{ObjectFit, RenderImage, SharedString, img, rems};
use mycela_engine::BinaryCursor;
use std::{collections::BTreeMap, fs, io::Read};

const THUMBNAIL_PREFIX_LIMIT: u64 = 1024 * 1024;

struct PreferredSynthesisFolder(PathBuf);
impl gpui_kit::Global for PreferredSynthesisFolder {}

struct PreferredFactoryFolder(PathBuf);
impl gpui_kit::Global for PreferredFactoryFolder {}

fn library_folder(cx: &App) -> Option<PathBuf> {
    cx.try_global::<PreferredSynthesisFolder>()
        .map(|folder| folder.0.clone())
        .or_else(|| {
            std::env::var_os("MYCELA_SYNTHESIS_PRESETS")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            dirs::data_dir()
                .map(|path| path.join("Mycela/Texture Synthesis Presets"))
                .filter(|path| path.is_dir())
        })
}

fn factory_folder(cx: &App) -> Option<PathBuf> {
    cx.try_global::<PreferredFactoryFolder>()
        .map(|folder| folder.0.clone())
        .or_else(|| {
            std::env::var_os("MYCELA_SYNTHESIS_FACTORY_PRESETS")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        })
}

#[derive(Clone)]
struct Choice {
    path: Arc<PathBuf>,
    category: SharedString,
    name: SharedString,
    thumbnail: Option<Arc<RenderImage>>,
    factory: bool,
    stamp: Option<files::FileStamp>,
}

impl Choice {
    fn editable(&self) -> bool {
        !self.factory && self.stamp.is_some()
    }
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

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .h_flex()
            .gap_2()
            .when_some(self.thumbnail.as_ref(), |row, thumbnail| {
                row.child(
                    img(thumbnail.clone())
                        .size(rems(1.5))
                        .object_fit(ObjectFit::Contain),
                )
            })
            .child(self.title())
    }
}

fn thumbnail(path: &Path) -> Result<Arc<RenderImage>, String> {
    let mut prefix = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(THUMBNAIL_PREFIX_LIMIT).read_to_end(&mut prefix))
        .map_err(model::failure)?;
    let mut reader = BinaryCursor::new(&prefix).map_err(model::failure)?;
    let jpeg = SynthesisEdit::read_preset_thumbnail(&mut reader).map_err(model::failure)?;
    model::decode_preset_thumbnail(jpeg, Some(80))
}

type Groups = BTreeMap<String, Vec<Choice>>;

fn discover(root: &Path, factory: Option<&Path>, ticket: &Ticket) -> std::io::Result<Groups> {
    let mut groups = BTreeMap::new();
    let mut folders = vec![(root.canonicalize()?, String::from("Uncategorized"), true)];
    while let Some((folder, category, at_root)) = folders.pop() {
        for file in fs::read_dir(folder)? {
            if !ticket.is_current() {
                return Ok(groups);
            }
            let file = file?;
            let kind = file.file_type()?;
            let path = file.path();
            if at_root && kind.is_dir() && !file.file_name().to_string_lossy().starts_with('.') {
                folders.push((path, file.file_name().to_string_lossy().into_owned(), false));
            } else if kind.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("tsyn"))
            {
                let Some(name) = path.file_stem() else {
                    continue;
                };
                groups
                    .entry(category.clone())
                    .or_insert_with(Vec::new)
                    .push(Choice {
                        factory: factory.is_some_and(|factory| path.starts_with(factory)),
                        stamp: file
                            .metadata()
                            .ok()
                            .map(|value| files::FileStamp::read(&value)),
                        name: name.to_string_lossy().into_owned().into(),
                        thumbnail: thumbnail(&path).ok(),
                        path: Arc::new(path),
                        category: category.clone().into(),
                    });
            }
        }
    }
    Ok(groups)
}

fn discover_libraries(
    user: Option<&Path>,
    factory: Option<&Path>,
    ticket: &Ticket,
) -> (Groups, files::Categories, Option<String>) {
    let factory = factory.map(Path::canonicalize).transpose();
    let mut warning = None;
    let factory = factory.unwrap_or_else(|error| {
        warning = Some(format!("Couldn’t read the factory preset folder. {error}"));
        None
    });
    let mut groups = BTreeMap::new();
    let user = user.filter(|user| {
        !factory.as_ref().is_some_and(|factory| {
            user.canonicalize()
                .is_ok_and(|user| user.starts_with(factory))
        })
    });
    for root in [factory.as_deref(), user].into_iter().flatten() {
        match discover(root, factory.as_deref(), ticket) {
            Ok(entries) => {
                for (category, choices) in entries {
                    let group: &mut Vec<Choice> = groups.entry(category).or_default();
                    group.extend(choices);
                }
            }
            Err(error) => warning = Some(format!("Couldn’t read {}. {error}", root.display())),
        }
    }
    for choices in groups.values_mut() {
        choices.sort_by_cached_key(|choice| {
            (
                choice.name.to_lowercase(),
                choice.name.clone(),
                !choice.factory,
                choice.path.clone(),
            )
        });
        choices.dedup_by(|a, b| a.path == b.path);
    }
    let categories = match user.map(|user| files::categories(user, factory.as_deref(), ticket)) {
        Some(Ok(categories)) => categories,
        Some(Err(error)) => {
            warning = Some(format!("Couldn’t read the user preset categories. {error}"));
            files::Categories::new()
        }
        None => files::Categories::new(),
    };
    (groups, categories, warning)
}

type Choices = SelectState<SearchableVec<SelectGroup<Choice>>>;

struct Browser {
    window: AnyWindowHandle,
    root: Option<PathBuf>,
    factory: Option<PathBuf>,
    graph: WeakEntity<GraphDocumentView>,
    target: Target,
    choices: Entity<Choices>,
    ticket: Option<Ticket>,
    activity: Option<&'static str>,
    has_entries: bool,
    insert: bool,
    save_open: bool,
    category: Entity<InputState>,
    name: Entity<InputState>,
    delete_confirmation: Option<Choice>,
    organization: organization::Editor,
    error: Option<String>,
    _selection: Subscription,
}

impl Browser {
    fn new(
        root: Option<PathBuf>,
        graph: WeakEntity<GraphDocumentView>,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let choices = cx.new(|cx| {
            SelectState::new(SearchableVec::new(Vec::new()), None, window, cx).searchable(true)
        });
        let selection = cx.subscribe(&choices, |browser, _, event, cx| {
            if matches!(event, SelectEvent::Confirm(Some(_))) {
                browser.error = None;
                browser.delete_confirmation = None;
                browser.organization.dismiss();
                cx.notify();
            }
        });
        Self {
            window: window.window_handle(),
            root,
            factory: factory_folder(cx),
            graph,
            target,
            choices,
            ticket: None,
            activity: None,
            has_entries: false,
            insert: false,
            save_open: false,
            category: cx.new(|cx| InputState::new(window, cx).default_value("My textures")),
            name: cx.new(|cx| InputState::new(window, cx)),
            delete_confirmation: None,
            organization: organization::Editor::new(window, cx),
            error: None,
            _selection: selection,
        }
    }

    fn retire(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
        self.activity = None;
    }

    fn begin(&mut self, activity: &'static str, cx: &mut Context<Self>) -> Ticket {
        self.retire();
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.activity = Some(activity);
        self.error = None;
        self.delete_confirmation = None;
        cx.notify();
        ticket
    }

    fn reload(&mut self, select_path: Option<PathBuf>, cx: &mut Context<Self>) {
        self.reload_with_error(select_path, None, cx);
    }

    fn reload_with_error(
        &mut self,
        select_path: Option<PathBuf>,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.root.is_none() && self.factory.is_none() {
            return;
        }
        self.organization.dismiss();
        let select_path = select_path.map(Arc::new).or_else(|| {
            self.choices
                .read(cx)
                .selected_value()
                .map(|choice| choice.path.clone())
        });
        let root = self.root.clone();
        let factory = self.factory.clone();
        let ticket = self.begin("Loading presets…", cx);
        let executor = cx.background_executor().clone();
        let window = self.window;
        cx.spawn(async move |owner, cx| {
            let worker = ticket.clone();
            let (entries, categories, warning) = executor
                .spawn(
                    async move { discover_libraries(root.as_deref(), factory.as_deref(), &worker) },
                )
                .await;
            if !ticket.is_current() {
                return;
            }
            let _ = cx.update_window(window, |_, window, cx| {
                owner.update(cx, |browser, cx| {
                    browser.retire();
                    let selected = entries
                        .values()
                        .flatten()
                        .find(|choice| select_path.as_ref() == Some(&choice.path))
                        .or_else(|| entries.values().flatten().next())
                        .cloned();
                    browser.error = error.or(warning);
                    browser.organization.categories = categories;
                    browser.has_entries = selected.is_some();
                    let groups: Vec<_> = entries
                        .into_iter()
                        .map(|(category, choices)| {
                            let mut group = SelectGroup::new(category);
                            group.items = choices;
                            group
                        })
                        .collect();
                    browser.choices.update(cx, |state, cx| {
                        state.set_items(SearchableVec::new(groups), window, cx);
                        if let Some(selected) = selected {
                            state.set_selected_value(&selected, window, cx);
                        } else {
                            state.set_selected_index(None, window, cx);
                        }
                    });
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }

    fn choose_folder(&mut self, factory: bool, cx: &mut Context<Self>) {
        self.organization.dismiss();
        let label = if factory {
            "Choose a factory preset folder"
        } else {
            "Choose a user preset folder"
        };
        let prompt = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(label.into()),
        });
        let ticket = self.begin(label, cx);
        cx.spawn(async move |owner, cx| {
            let result = prompt.await;
            if !ticket.is_current() {
                return;
            }
            let _ = owner.update(cx, |browser, cx| {
                browser.retire();
                match result {
                    Ok(Ok(Some(mut paths))) if paths.len() == 1 => {
                        let root = paths.remove(0);
                        if factory {
                            cx.set_global(PreferredFactoryFolder(root.clone()));
                            browser.factory = Some(root);
                        } else {
                            cx.set_global(PreferredSynthesisFolder(root.clone()));
                            browser.root = Some(root);
                        }
                        browser.reload(None, cx);
                    }
                    Ok(Ok(None)) => {}
                    _ => {
                        browser.error = Some("Couldn’t select the Synthesis preset folder.".into())
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.ticket.is_some() {
            return;
        }
        let category = self.category.read(cx).value().trim().to_owned();
        let name = self.name.read(cx).value().trim().to_owned();
        if !files::valid_name(&category) || !files::valid_name(&name) {
            self.error = Some(
                "Enter a category and preset name using ordinary file name characters.".into(),
            );
            cx.notify();
            return;
        }
        let Some(root) = self.root.clone() else {
            self.error = Some("Choose a user preset folder first.".into());
            cx.notify();
            return;
        };
        let export = self
            .graph
            .update(cx, |graph, cx| {
                graph.capture_synthesis_preset(&self.target, cx)
            })
            .unwrap_or_else(|_| Err("The graph window closed.".into()));
        let export = match export {
            Ok(export) => export,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let factory = self.factory.clone();
        let ticket = self.begin("Saving Synthesis preset…", cx);
        let executor = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let worker = ticket.clone();
            let result = executor
                .spawn(async move {
                    files::save_new(export, &root, factory.as_deref(), &category, &name, &worker)
                })
                .await;
            if !ticket.is_current() {
                return;
            }
            let _ = owner.update(cx, |browser, cx| {
                browser.retire();
                match result {
                    Ok(Some(path)) => {
                        browser.save_open = false;
                        browser.reload(Some(path), cx);
                    }
                    Ok(None) => {}
                    Err(error) => browser.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        if self.ticket.is_some() {
            return;
        }
        let Some(preset) = self.delete_confirmation.take() else {
            return;
        };
        let ticket = self.begin("Deleting Synthesis preset…", cx);
        let executor = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let worker = ticket.clone();
            let result = executor
                .spawn(async move { files::delete(&preset, &worker) })
                .await;
            if !ticket.is_current() {
                return;
            }
            let _ = owner.update(cx, |browser, cx| {
                browser.retire();
                match result {
                    Ok(()) => browser.reload(None, cx),
                    Err(error) => browser.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn selection(&self, cx: &App) -> Result<Choice, String> {
        if self.ticket.is_some() {
            return Err("Wait for the preset folder to finish loading.".into());
        }
        if self.delete_confirmation.is_some() {
            return Err("Finish or cancel the preset deletion first.".into());
        }
        if self.organization.is_open() {
            return Err("Finish or cancel the library change first.".into());
        }
        self.choices
            .read(cx)
            .selected_value()
            .cloned()
            .ok_or_else(|| "Choose a preset to apply.".into())
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.retire();
    }
}

impl Render for Browser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.choices.read(cx).selected_value().cloned();
        let deletable = selected.clone().filter(Choice::editable);
        let organize = self.organize_menu(selected.as_ref(), cx);
        let busy = self.ticket.is_some();
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
                        Button::new("synthesis-preset-folder")
                            .small()
                            .label("Choose user folder…")
                            .disabled(busy)
                            .on_click(
                                cx.listener(|browser, _, _, cx| browser.choose_folder(false, cx)),
                            ),
                    )
                    .child(
                        Button::new("synthesis-preset-factory-folder")
                            .small()
                            .label("Choose factory folder…")
                            .disabled(busy)
                            .on_click(
                                cx.listener(|browser, _, _, cx| browser.choose_folder(true, cx)),
                            ),
                    )
                    .child(
                        Button::new("synthesis-preset-refresh")
                            .small()
                            .label("Refresh")
                            .disabled(busy || self.root.is_none() && self.factory.is_none())
                            .on_click(cx.listener(|browser, _, _, cx| browser.reload(None, cx))),
                    ),
            )
            .when_some(self.root.as_ref(), |body, folder| {
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
                        .id("synthesis-preset-choice")
                        .accessibility_label("Synthesis preset")
                        .disabled(busy || !self.has_entries),
                ),
            )
            .when_some(selected, |body, preset| {
                body.child(
                    div()
                        .h_flex()
                        .gap_3()
                        .when_some(preset.thumbnail, |row, thumbnail| {
                            row.child(
                                img(thumbnail)
                                    .id("synthesis-preset-thumbnail")
                                    .size(rems(5.))
                                    .object_fit(ObjectFit::Contain)
                                    .test_support(),
                            )
                        })
                        .child(
                            div()
                                .v_flex()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    div()
                                        .id("synthesis-preset-title")
                                        .aria_label(preset.name.clone())
                                        .test_support()
                                        .font_semibold()
                                        .child(preset.name),
                                )
                                .child(div().text_sm().text_color(theme.muted_foreground).child(
                                    format!(
                                        "{} · {}",
                                        preset.category,
                                        if preset.factory { "Factory" } else { "User" }
                                    ),
                                )),
                        ),
                )
            })
            .when(self.has_entries, |body| {
                body.child(
                    RadioGroup::horizontal("synthesis-preset-mode")
                        .child(Radio::new("synthesis-preset-replace").label("Replace texture"))
                        .child(Radio::new("synthesis-preset-insert").label("Insert layers"))
                        .selected_index(Some(usize::from(self.insert)))
                        .disabled(busy)
                        .on_change(cx.listener(|browser, index, _, cx| {
                            browser.insert = *index == 1;
                            cx.notify();
                        })),
                )
            })
            .when(self.root.is_some() || deletable.is_some(), |body| {
                body.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .flex_wrap()
                        .when(self.root.is_some(), |row| {
                            row.child(
                                Button::new("synthesis-preset-save-form")
                                    .small()
                                    .ghost()
                                    .label(if self.save_open {
                                        "Hide save fields"
                                    } else {
                                        "Save current texture…"
                                    })
                                    .disabled(busy || self.delete_confirmation.is_some())
                                    .on_click(cx.listener(|browser, _, _, cx| {
                                        browser.organization.dismiss();
                                        browser.save_open = !browser.save_open;
                                        browser.error = None;
                                        cx.notify();
                                    })),
                            )
                        })
                        .when_some(organize, |row, menu| row.child(menu))
                        .when_some(
                            deletable.filter(|_| self.delete_confirmation.is_none()),
                            |row, preset| {
                                row.child(
                                    Button::new("synthesis-preset-delete")
                                        .small()
                                        .ghost()
                                        .label("Delete preset…")
                                        .disabled(busy)
                                        .on_click(cx.listener(move |browser, _, _, cx| {
                                            browser.organization.dismiss();
                                            browser.save_open = false;
                                            browser.delete_confirmation = Some(preset.clone());
                                            browser.error = None;
                                            cx.notify();
                                        })),
                                )
                            },
                        ),
                )
            })
            .when(self.save_open, |body| {
                body.child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(
                            Form::new()
                                .columns(2)
                                .child(
                                    Field::new().label("Category").child(
                                        Input::new(&self.category)
                                            .id("synthesis-preset-category")
                                            .aria_label("Preset category")
                                            .disabled(busy),
                                    ),
                                )
                                .child(
                                    Field::new().label("Name").child(
                                        Input::new(&self.name)
                                            .id("synthesis-preset-name")
                                            .aria_label("Preset name")
                                            .disabled(busy),
                                    ),
                                ),
                        )
                        .child(
                            Button::new("synthesis-preset-save")
                                .small()
                                .label("Save new preset")
                                .disabled(busy)
                                .on_click(cx.listener(|browser, _, _, cx| browser.save(cx))),
                        ),
                )
            })
            .when_some(self.render_organization(cx), |body, form| body.child(form))
            .when_some(self.delete_confirmation.as_ref(), |body, preset| {
                body.child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(div().child(format!(
                            "Delete ‘{}’ from the user preset folder?",
                            preset.name
                        )))
                        .child(
                            div()
                                .h_flex()
                                .gap_2()
                                .flex_wrap()
                                .child(
                                    Button::new("synthesis-preset-keep")
                                        .small()
                                        .label("Keep preset")
                                        .on_click(cx.listener(|browser, _, _, cx| {
                                            browser.delete_confirmation = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("synthesis-preset-confirm-delete")
                                        .small()
                                        .danger()
                                        .label("Delete preset")
                                        .on_click(
                                            cx.listener(|browser, _, _, cx| browser.delete(cx)),
                                        ),
                                ),
                        ),
                )
            })
            .when_some(self.activity, |body, activity| {
                body.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(activity),
                )
            })
            .when(!busy && !self.has_entries && self.error.is_none(), |body| {
                body.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Choose a folder containing .tsyn files or preset categories."),
                )
            })
            .when_some(self.error.as_ref(), |body, error| {
                body.child(
                    div()
                        .id("synthesis-preset-error")
                        .aria_label(error.clone())
                        .test_support()
                        .role(gpui_kit::Role::Alert)
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error.clone()),
                )
            })
    }
}

impl GraphDocumentView {
    pub(super) fn open_synthesis_presets(
        &mut self,
        target: &Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() || !target.current(self) || self.has_field_drafts(cx) {
            return;
        }
        let root = library_folder(cx);
        let graph = cx.weak_entity();
        let browser = cx.new(|cx| Browser::new(root, graph.clone(), target.clone(), window, cx));
        browser.update(cx, |browser, cx| browser.reload(None, cx));
        self.pause_preview();
        self.modal = true;
        let target = target.clone();
        window.open_dialog(cx, move |dialog, window, _| {
            let confirm = browser.clone();
            let close = browser.clone();
            let apply_graph = graph.clone();
            let close_graph = graph.clone();
            let target = target.clone();
            dialog.title("Synthesis presets").width(window.rem_size() * 34.).child(browser.clone())
                .footer(DialogFooter::new()
                    .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                    .child(DialogAction::new().child(Button::new("ok").primary().label("Apply preset"))))
                .on_ok(move |_, _, cx| {
                    let state = confirm.read(cx);
                    let insert = state.insert;
                    let result = state.selection(cx).and_then(|preset| {
                        apply_graph.update(cx, |graph, cx| {
                            if graph.closed || !target.current(graph) || graph.has_field_drafts(cx) {
                                return Err("The selected texture changed. Open its preset browser again.".into());
                            }
                            graph.load_synthesis_file(target.clone(), FileKind::Preset { insert }, Some(preset.path.as_ref().clone()), cx);
                            Ok(())
                        }).unwrap_or_else(|_| Err("The graph window closed.".into()))
                    });
                    if let Err(error) = result {
                        confirm.update(cx, |browser, cx| { browser.error = Some(error); cx.notify(); });
                        return false;
                    }
                    true
                })
                .on_close(move |_, _, cx| {
                    close.update(cx, |browser, _| browser.retire());
                    let _ = close_graph.update(cx, |graph, cx| { graph.modal = false; cx.notify(); });
                })
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesis_library_discovers_factory_presets_and_thumbnails() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../genetica/Texture Synthesis Presets");
        let canonical = root.canonicalize().unwrap();
        let entries = discover(&root, Some(&canonical), &Ticket::new()).unwrap();
        assert!(entries.values().flatten().all(|preset| preset.factory));
        assert_eq!(entries.values().map(Vec::len).sum::<usize>(), 493);
        assert!(
            entries
                .values()
                .flatten()
                .all(|preset| preset.thumbnail.is_some())
        );
        assert!(
            entries["Rock"]
                .iter()
                .any(|preset| preset.name == "Sandstone 3")
        );
    }
}
