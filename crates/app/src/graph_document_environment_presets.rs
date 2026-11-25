//! Browser for environment maps in the user’s preset folder.
#[path = "environment_preset_catalog.rs"]
mod catalog;

use super::{
    GraphDocumentView, Ticket, Work, environment::Target, environment_export, model,
    preferences::File as PreferenceFile,
};
use catalog::{Catalog, Preset};
use gpui_kit::base::{Disableable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, IndexPath, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
    form::{Field, Form},
    input::{Input, InputState},
    radio::{Radio, RadioGroup},
    select::{SearchableVec, Select, SelectEvent, SelectGroup, SelectItem, SelectState},
};
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Context, Entity, IntoElement, ObjectFit, Render,
    SharedString, Subscription, Window, div, img, prelude::*, rems,
};
use mycela_engine::{EnvironmentRecord, EnvironmentResourceLibrary};
use std::{path::PathBuf, sync::Arc};

pub(super) struct PreferredPresetFolder(PathBuf);
impl gpui_kit::Global for PreferredPresetFolder {}

fn user_folder(cx: &App) -> Option<PathBuf> {
    cx.try_global::<PreferredPresetFolder>()
        .map(|root| root.0.clone())
        .or_else(|| {
            std::env::var_os("MYCELA_ENVIRONMENT_PRESETS")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| PreferenceFile::PresetFolder.read_path(cx))
        .or_else(|| {
            dirs::data_dir().map(|path| path.join("Mycela").join("Environment Map Presets"))
        })
}

#[derive(Clone)]
struct Choice(Arc<Preset>);
impl PartialEq for Choice {
    fn eq(&self, other: &Self) -> bool {
        self.0.category == other.0.category && self.0.name == other.0.name
    }
}
impl SelectItem for Choice {
    type Value = Self;

    fn title(&self) -> SharedString {
        self.0.name.clone().into()
    }

    fn value(&self) -> &Self {
        self
    }

    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.0.name.to_lowercase().contains(&query)
            || self.0.category.to_lowercase().contains(&query)
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .h_flex()
            .items_center()
            .gap_2()
            .child(
                img(self.0.thumbnail.clone())
                    .size(rems(2.))
                    .object_fit(ObjectFit::Contain),
            )
            .child(self.title())
    }
}

type Choices = SelectState<SearchableVec<SelectGroup<Choice>>>;

#[derive(Clone, Copy)]
enum ApplyMode {
    Replace,
    Insert,
}

type SelectionKey = (String, String);

struct Browser {
    window: AnyWindowHandle,
    map: Arc<EnvironmentRecord>,
    library: EnvironmentResourceLibrary,
    root: Option<PathBuf>,
    presets: Entity<Choices>,
    mode: ApplyMode,
    save_open: bool,
    remove_confirmation: Option<Arc<Preset>>,
    category: Entity<InputState>,
    name: Entity<InputState>,
    ticket: Option<Ticket>,
    status: Option<String>,
    error: Option<String>,
    has_entries: bool,
    _selection: Subscription,
}

impl Browser {
    fn new(
        map: Arc<EnvironmentRecord>,
        library: EnvironmentResourceLibrary,
        root: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let presets = cx.new(|cx| {
            SelectState::new(SearchableVec::new(Vec::new()), None, window, cx).searchable(true)
        });
        let selection = cx.subscribe(&presets, |browser, _, event, cx| {
            if matches!(event, SelectEvent::Confirm(Some(_))) {
                browser.error = None;
                browser.remove_confirmation = None;
                cx.notify();
            }
        });
        Self {
            window: window.window_handle(),
            map,
            library,
            root,
            presets,
            mode: ApplyMode::Replace,
            save_open: false,
            remove_confirmation: None,
            category: cx.new(|cx| InputState::new(window, cx).default_value("My maps")),
            name: cx.new(|cx| InputState::new(window, cx)),
            ticket: None,
            status: None,
            error: None,
            has_entries: false,
            _selection: selection,
        }
    }

    fn selected<'a>(&self, cx: &'a App) -> Option<&'a Arc<Preset>> {
        self.presets
            .read(cx)
            .selected_value()
            .map(|choice| &choice.0)
    }

    fn begin(&mut self, status: &str, cx: &mut Context<Self>) -> Ticket {
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.remove_confirmation = None;
        self.status = Some(status.into());
        self.error = None;
        cx.notify();
        ticket
    }

    fn reload(
        &mut self,
        desired: Option<SelectionKey>,
        preference_error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let desired = desired.or_else(|| {
            self.selected(cx)
                .map(|preset| (preset.category.clone(), preset.name.clone()))
        });
        let ticket = self.begin("Loading presets…", cx);
        let executor = cx.background_executor().clone();
        let root = self.root.clone();
        let window = self.window;
        cx.spawn(async move |owner, cx| {
            let work = ticket.clone();
            let catalog = executor
                .spawn(async move { catalog::scan(root.as_deref(), &work) })
                .await;
            if !ticket.is_current() {
                return;
            }
            let _ = cx.update_window(window, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    view.finish_scan(catalog, desired, preference_error, window, cx);
                })
            });
        })
        .detach();
    }

    fn finish_scan(
        &mut self,
        catalog: Catalog,
        desired: Option<SelectionKey>,
        preference_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut groups: Vec<SelectGroup<Choice>> = Vec::new();
        let mut selected = None;
        self.has_entries = !catalog.entries.is_empty();
        for preset in catalog.entries {
            if groups
                .last()
                .is_none_or(|group| group.title.as_ref() != preset.category)
            {
                groups.push(SelectGroup::new(preset.category.clone()));
            }
            let section = groups.len() - 1;
            let group = &mut groups[section];
            if desired.as_ref().is_some_and(|(category, name)| {
                *category == preset.category && *name == preset.name
            }) {
                selected = Some(IndexPath {
                    section,
                    row: group.items.len(),
                    column: 0,
                });
            }
            group.items.push(Choice(preset));
        }
        let selected = selected.or_else(|| self.has_entries.then(IndexPath::default));
        self.presets.update(cx, |list, cx| {
            list.set_items(SearchableVec::new(groups), window, cx);
            list.set_selected_index(selected, window, cx);
        });
        self.ticket = None;
        self.error = preference_error.or(catalog.warning);
        self.status = (catalog.skipped > 0)
            .then(|| format!("Couldn’t read {} preset thumbnails.", catalog.skipped));
        cx.notify();
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        if self.ticket.is_some() {
            return;
        }
        let ticket = self.begin("Choose a user preset folder", cx);
        let prompt = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a user environment preset folder".into()),
        });
        cx.spawn(async move |owner, cx| {
            let result = prompt.await;
            if !ticket.is_current() {
                return;
            }
            let _ = owner.update(cx, |view, cx| {
                view.ticket = None;
                view.status = None;
                match result {
                    Ok(Ok(Some(mut paths))) if paths.len() == 1 => {
                        let root = paths.remove(0);
                        let preference_error = PreferenceFile::PresetFolder
                            .write_path(&root, cx)
                            .err()
                            .map(|error| format!("Couldn’t remember the preset folder. {error}"));
                        cx.set_global(PreferredPresetFolder(root.clone()));
                        view.root = Some(root);
                        view.reload(None, preference_error, cx);
                    }
                    Ok(Ok(None)) => {}
                    Ok(Ok(Some(_))) => view.error = Some("Choose one folder.".into()),
                    Ok(Err(error)) => {
                        view.error = Some(format!("Couldn’t open the folder picker. {error}"))
                    }
                    Err(_) => {
                        view.error =
                            Some("The folder picker closed before returning a folder.".into())
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.ticket.is_some() {
            return;
        }
        let category = self.category.read(cx).value().trim().to_owned();
        let name = self.name.read(cx).value().trim().to_owned();
        let Some(root) = self.root.clone() else {
            self.error = Some("Choose a user preset folder first.".into());
            cx.notify();
            return;
        };
        let path = match catalog::new_preset_path(&root, &category, &name) {
            Ok(path) => path,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let ticket = self.begin("Saving environment preset…", cx);
        let executor = cx.background_executor().clone();
        let map = self.map.clone();
        let library = self.library.clone();
        let desired = Some((category.clone(), name.clone()));
        cx.spawn(async move |owner, cx| {
            let work = ticket.clone();
            let result = executor
                .spawn(async move {
                    catalog::check_user_name(&root, &category, &name)?;
                    let Some(binary) = environment_export::encode_preset(&map, &library, &work)?
                    else {
                        return Ok(None);
                    };
                    if !work.is_current() {
                        return Ok(None);
                    }
                    let parent = path.parent().expect("preset category");
                    if parent
                        .symlink_metadata()
                        .is_ok_and(|metadata| metadata.file_type().is_symlink())
                    {
                        return Err("Choose a category stored in the preset folder.".into());
                    }
                    std::fs::create_dir_all(parent).map_err(model::failure)?;
                    model::save_new_bytes(binary.bytes(), path, &work)
                })
                .await;
            if !ticket.is_current() {
                return;
            }
            let _ = owner.update(cx, |view, cx| {
                view.ticket = None;
                view.status = None;
                match result {
                    Ok(Some(_)) => {
                        view.save_open = false;
                        view.reload(desired, None, cx);
                    }
                    Ok(None) => {}
                    Err(error) => view.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn delete_preset(&mut self, cx: &mut Context<Self>) {
        if self.ticket.is_some() {
            return;
        }
        let Some(preset) = self.remove_confirmation.take() else {
            return;
        };
        let ticket = self.begin("Deleting environment preset…", cx);
        let executor = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let work = ticket.clone();
            let result = executor
                .spawn(async move {
                    if work.is_current() {
                        preset.remove_from_disk()
                    } else {
                        Ok(())
                    }
                })
                .await;
            if !ticket.is_current() {
                return;
            }
            let _ = owner.update(cx, |view, cx| {
                view.ticket = None;
                view.status = None;
                match result {
                    Ok(()) => view.reload(None, None, cx),
                    Err(error) => view.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn request(&self, cx: &App) -> Result<(Arc<Preset>, ApplyMode), String> {
        if self.ticket.is_some() {
            return Err("Wait for the current preset operation to finish.".into());
        }
        if self.remove_confirmation.is_some() {
            return Err("Finish or cancel the preset deletion first.".into());
        }
        let preset = self
            .selected(cx)
            .cloned()
            .ok_or("Choose a preset to apply.")?;
        Ok((preset, self.mode))
    }

    fn retire(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            ticket.retire();
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.retire();
    }
}

impl Render for Browser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.ticket.is_some();
        let selected = self.selected(cx).cloned();
        let theme = cx.theme();
        let root = self.root.as_ref().map_or_else(
            || "Choose a folder for user presets".into(),
            |path| path.display().to_string(),
        );
        div().v_flex().gap_3()
            .child(div().h_flex().flex_wrap().items_center().gap_2()
                .child(Button::new("environment-preset-folder").label("Choose preset folder…").small()
                    .disabled(busy).on_click(cx.listener(|view, _, _, cx| view.choose_folder(cx))))
                .child(Button::new("environment-preset-refresh").label("Refresh").small().disabled(busy)
                    .on_click(cx.listener(|view, _, _, cx| view.reload(None, None, cx)))))
            .child(div().text_sm().text_color(theme.muted_foreground).child(root))
            .child(Field::new().label("Preset").child(
                Select::new(&self.presets).id("environment-preset-choice").accessibility_label("Environment preset")
                    .disabled(busy || !self.has_entries),
            ))
            .when_some(selected.clone().filter(|_| self.remove_confirmation.is_none()), |root, preset| root.child(
                Button::new("environment-preset-delete").label("Delete preset…").small().ghost()
                    .disabled(busy).on_click(cx.listener(move |view, _, _, cx| {
                        view.remove_confirmation = Some(preset.clone());
                        cx.notify();
                    })),
            ))
            .when_some(self.remove_confirmation.as_ref(), |root, preset| root.child(
                div().v_flex().gap_2()
                    .child(div().child(format!("Delete ‘{}’?", preset.name)))
                    .child(
                        div().id("environment-preset-delete-path").test_support()
                            .aria_label(preset.path.display().to_string())
                            .text_sm().text_color(theme.muted_foreground).whitespace_normal()
                            .child(preset.path.display().to_string()),
                    )
                    .child(div().h_flex().flex_wrap().gap_2()
                        .child(Button::new("environment-preset-cancel-delete").label("Keep preset").small()
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.remove_confirmation = None;
                                cx.notify();
                            })))
                        .child(Button::new("environment-preset-confirm-delete").label("Delete preset").small().danger()
                            .on_click(cx.listener(|view, _, _, cx| view.delete_preset(cx))))),
            ))
            .when(!busy && !self.has_entries, |root| root.child(div().text_sm().text_color(theme.muted_foreground)
                .child("No presets found. Choose a folder containing category folders with .emap files.")))
            .when_some(selected, |root, preset| root.child(
                div().h_flex().items_center().gap_3()
                    .child(img(preset.thumbnail.clone()).id("environment-preset-thumbnail").size(rems(6.))
                        .object_fit(ObjectFit::Contain).test_support())
                    .child(div().v_flex().min_w_0().gap_1()
                        .child(div().id("environment-preset-title").aria_label(preset.name.clone())
                            .test_support().font_semibold().child(preset.name.clone()))
                        .child(div().id("environment-preset-origin")
                            .aria_label("User preset")
                            .test_support()
                            .text_sm().text_color(theme.muted_foreground)
                            .child(preset.category.clone()))),
            ))
            .child(
                RadioGroup::horizontal("environment-preset-mode")
                    .child(Radio::new("environment-preset-replace").label("Replace map"))
                    .child(Radio::new("environment-preset-insert").label("Insert layers"))
                    .selected_index(Some(usize::from(matches!(self.mode, ApplyMode::Insert))))
                    .disabled(busy)
                    .on_change(cx.listener(|view, index, _, cx| {
                        view.mode = if *index == 0 { ApplyMode::Replace } else { ApplyMode::Insert };
                        cx.notify();
                    })),
            )
            .when(self.map.layer_count() != 0, |root| root.child(
                Button::new("environment-preset-save-form")
                    .label(if self.save_open { "Hide save fields" } else { "Save current map as preset…" })
                    .small().disabled(busy)
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.save_open = !view.save_open;
                        view.error = None;
                        cx.notify();
                    })),
            ))
            .when(self.save_open, |root| root.child(
                div().v_flex().gap_2()
                    .child(Form::new().columns(2)
                        .child(Field::new().label("Category").child(Input::new(&self.category)
                            .id("environment-preset-category").aria_label("Preset category").disabled(busy)))
                        .child(Field::new().label("Name").child(Input::new(&self.name)
                            .id("environment-preset-name").aria_label("Preset name").disabled(busy))))
                    .child(Button::new("environment-preset-save").label("Save new preset").small()
                        .disabled(busy).on_click(cx.listener(|view, _, _, cx| view.save(cx)))),
            ))
            .when_some(self.status.clone(), |root, status| root.child(div().text_sm().text_color(theme.muted_foreground).child(status)))
            .when_some(self.error.clone(), |root, error| root.child(div().id("environment-preset-error")
                .role(gpui_kit::Role::Alert).aria_label(error.clone()).test_support()
                .text_sm().text_color(theme.danger).child(error)))
    }
}

impl GraphDocumentView {
    pub(super) fn open_environment_presets(
        &mut self,
        target: &Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_edit_environment(target, cx) {
            return;
        }
        self.pause_preview();
        self.modal = true;
        let root = user_folder(cx);
        let browser = cx.new(|cx| {
            Browser::new(
                target.panel.value().clone(),
                self.environment_library.clone(),
                root,
                window,
                cx,
            )
        });
        browser.update(cx, |view, cx| view.reload(None, None, cx));
        let target = target.clone();
        let graph = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, _| {
            let confirm = browser.clone();
            let close = browser.clone();
            let graph_confirm = graph.clone();
            let graph_close = graph.clone();
            let target = target.clone();
            dialog
                .width(window.rem_size() * 38.)
                .title("Environment presets")
                .child(browser.clone())
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().child(Button::new("cancel").label("Cancel")))
                        .child(
                            DialogAction::new()
                                .child(Button::new("ok").primary().label("Apply preset")),
                        ),
                )
                .on_ok(move |_, _, cx| {
                    let result = confirm.read(cx).request(cx).and_then(|(preset, mode)| {
                        graph_confirm
                            .update(cx, |graph, cx| {
                                graph.begin_preset_apply(&target, preset, mode, cx)
                            })
                            .unwrap_or_else(|_| Err("The graph window closed.".into()))
                    });
                    match result {
                        Ok(()) => true,
                        Err(error) => {
                            confirm.update(cx, |view, cx| {
                                view.error = Some(error);
                                cx.notify();
                            });
                            false
                        }
                    }
                })
                .on_close(move |_, _, cx| {
                    close.update(cx, |view, _| view.retire());
                    let _ = graph_close.update(cx, |graph, cx| {
                        graph.modal = false;
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }

    fn begin_preset_apply(
        &mut self,
        target: &Target,
        preset: Arc<Preset>,
        mode: ApplyMode,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.closed
            || self.work.is_some()
            || !target.current(self)
            || self.inspector.is_changed(cx)
        {
            return Err("The selected map changed. Open its presets again.".into());
        }
        target.panel.validate()?;
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::LoadingEnvironment);
        self.error = None;
        self.status = None;
        let target = target.clone();
        let original = target.panel.value().clone();
        let window = self.window;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let result = executor
                .spawn(async move {
                    let selected = preset.load()?;
                    if matches!(mode, ApplyMode::Replace) {
                        return Ok(selected);
                    }
                    let mut map = original.as_ref().clone();
                    map.insert_preset_layers(selected).map_err(model::failure)?;
                    Ok(map)
                })
                .await;
            let _ = cx.update_window(window, |_, window, cx| {
                owner.update(cx, |graph, cx| {
                    if !graph.owns(&ticket) {
                        return;
                    }
                    graph.work = None;
                    graph.ticket = None;
                    ticket.retire();
                    match result {
                        Ok(map) if graph.can_edit_environment(&target, cx) => {
                            graph.apply_environment_change(&target.panel, map, 0, window, cx);
                        }
                        Ok(_) => {}
                        Err(error) => graph.error = Some(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
        Ok(())
    }
}
