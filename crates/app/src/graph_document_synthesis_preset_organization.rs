//! Preset and category organization within the Synthesis browser.
use super::*;
use gpui_kit::component::menu::PopupMenuItem;

type CategorySelect = SelectState<SearchableVec<SharedString>>;

#[derive(Clone, Copy)]
enum Command {
    RenamePreset,
    MovePreset,
    RenameCategory,
    DeleteCategory,
}

impl Command {
    fn label(self) -> &'static str {
        match self {
            Self::RenamePreset => "Rename preset",
            Self::MovePreset => "Move preset",
            Self::RenameCategory => "Rename category",
            Self::DeleteCategory => "Delete category",
        }
    }
}

struct FormState {
    command: Command,
    preset: Option<Choice>,
}

pub(super) struct Editor {
    pub(super) categories: files::Categories,
    category: Entity<CategorySelect>,
    name: Entity<InputState>,
    form: Option<FormState>,
    _category_selection: Subscription,
}

impl Editor {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Browser>) -> Self {
        let category = cx.new(|cx| {
            SelectState::new(SearchableVec::new(Vec::new()), None, window, cx).searchable(true)
        });
        let category_selection = cx.subscribe(&category, |browser, _, event, cx| {
            if matches!(event, SelectEvent::Confirm(Some(_))) {
                browser.error = None;
                cx.notify();
            }
        });
        Self {
            categories: files::Categories::new(),
            category,
            name: cx.new(|cx| InputState::new(window, cx)),
            form: None,
            _category_selection: category_selection,
        }
    }

    pub(super) fn dismiss(&mut self) {
        self.form = None;
    }

    pub(super) fn is_open(&self) -> bool {
        self.form.is_some()
    }

    fn can_move(&self, preset: &Choice) -> bool {
        self.categories
            .values()
            .any(|folder| !folder.contains(preset))
    }

    fn read_category(&self, cx: &App) -> Result<files::Category, String> {
        self.category
            .read(cx)
            .selected_value()
            .and_then(|name| self.categories.get(name.as_ref()))
            .cloned()
            .ok_or_else(|| "Choose a user category.".into())
    }

    fn change(&self, cx: &App) -> Result<files::Change, String> {
        let form = self
            .form
            .as_ref()
            .ok_or("Choose a library command first.")?;
        let preset = || {
            form.preset
                .clone()
                .ok_or_else(|| String::from("Choose a user preset."))
        };
        let name = || self.name.read(cx).value().trim().to_owned();
        match form.command {
            Command::RenamePreset => Ok(files::Change::RenamePreset(preset()?, name())),
            Command::MovePreset => Ok(files::Change::MovePreset(
                preset()?,
                self.read_category(cx)?,
            )),
            Command::RenameCategory => Ok(files::Change::RenameCategory(
                self.read_category(cx)?,
                name(),
            )),
            Command::DeleteCategory => Ok(files::Change::DeleteCategory(self.read_category(cx)?)),
        }
    }
}

impl Browser {
    fn start_organization(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ticket.is_some() {
            return;
        }
        let preset = self
            .choices
            .read(cx)
            .selected_value()
            .filter(|preset| preset.editable())
            .cloned();
        match command {
            Command::RenamePreset if preset.is_none() => return,
            Command::MovePreset
                if !preset
                    .as_ref()
                    .is_some_and(|preset| self.organization.can_move(preset)) =>
            {
                return;
            }
            Command::RenameCategory | Command::DeleteCategory
                if self.organization.categories.is_empty() =>
            {
                return;
            }
            _ => {}
        }
        let mut preselected = None;
        let mut names = Vec::new();
        for (name, category) in &self.organization.categories {
            let source = preset
                .as_ref()
                .is_some_and(|preset| category.contains(preset));
            if source && matches!(command, Command::MovePreset) {
                continue;
            }
            let name = SharedString::from(name.clone());
            if source && matches!(command, Command::RenameCategory | Command::DeleteCategory) {
                preselected = Some(name.clone());
            }
            names.push(name);
        }
        if matches!(command, Command::RenameCategory) && preselected.is_none() {
            preselected = names.first().cloned();
        }
        self.organization.category.update(cx, |picker, cx| {
            picker.set_items(SearchableVec::new(names), window, cx);
            if let Some(name) = preselected {
                picker.set_selected_value(&name, window, cx);
            } else {
                picker.set_selected_index(None, window, cx);
            }
        });
        let name = preset
            .as_ref()
            .filter(|_| matches!(command, Command::RenamePreset))
            .map(|preset| preset.name.clone())
            .unwrap_or_default();
        self.organization
            .name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.save_open = false;
        self.delete_confirmation = None;
        self.error = None;
        self.organization.form = Some(FormState { command, preset });
        cx.on_next_frame(window, move |browser, window, cx| {
            if !browser.organization.is_open() {
                return;
            }
            if matches!(command, Command::RenamePreset) {
                browser.organization.name.update(cx, |input, cx| {
                    input.focus(window, cx);
                    input.select_all(window, cx);
                });
            } else {
                browser
                    .organization
                    .category
                    .update(cx, |picker, cx| picker.focus(window, cx));
            }
        });
        cx.notify();
    }

    fn organize(&mut self, cx: &mut Context<Self>) {
        if self.ticket.is_some() {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let change = match self.organization.change(cx) {
            Ok(change) => change,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let factory = self.factory.clone();
        let selected = self
            .choices
            .read(cx)
            .selected_value()
            .map(|preset| preset.path.clone());
        let deleting = matches!(&change, files::Change::DeleteCategory(_));
        let activity = if deleting {
            "Deleting preset category…"
        } else {
            "Updating preset library…"
        };
        let ticket = self.begin(activity, cx);
        let executor = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let worker = ticket.clone();
            let result = executor
                .spawn(async move { files::organize(change, &root, factory.as_deref(), &worker) })
                .await;
            if !ticket.is_current() {
                return;
            }
            let _ = owner.update(cx, |browser, cx| {
                browser.retire();
                match result {
                    Ok(Some(outcome)) => {
                        let path =
                            outcome.update_selection(selected.as_deref().map(PathBuf::as_path));
                        browser.reload(path, cx);
                    }
                    Ok(None) => {}
                    // A failed recursive removal can leave fewer entries than before.
                    Err(error) if deleting => browser.reload_with_error(None, Some(error), cx),
                    Err(error) => browser.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn organize_menu(
        &self,
        selected: Option<&Choice>,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let preset = selected.filter(|preset| preset.editable());
        let rename = preset.is_some();
        let move_preset = preset.is_some_and(|preset| self.organization.can_move(preset));
        let has_categories = !self.organization.categories.is_empty();
        if !files::ORGANIZE_SUPPORTED || !(rename || has_categories) {
            return None;
        }
        let owner = cx.weak_entity();
        Some(
            Button::new("synthesis-preset-organize")
                .small()
                .ghost()
                .label("Organize")
                .disabled(self.ticket.is_some())
                .dropdown_menu(move |mut menu, _, _| {
                    for (command, _) in [
                        (Command::RenamePreset, rename),
                        (Command::MovePreset, move_preset),
                        (Command::RenameCategory, has_categories),
                        (Command::DeleteCategory, has_categories),
                    ]
                    .into_iter()
                    .filter(|(_, enabled)| *enabled)
                    {
                        if matches!(command, Command::DeleteCategory) {
                            menu = menu.separator();
                        }
                        let owner = owner.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("{}…", command.label())).on_click(
                                move |_, window, cx| {
                                    let _ = owner.update(cx, |browser, cx| {
                                        browser.start_organization(command, window, cx);
                                    });
                                },
                            ),
                        );
                    }
                    menu
                }),
        )
    }

    pub(super) fn render_organization(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let form = self.organization.form.as_ref()?;
        let busy = self.ticket.is_some();
        let command = form.command;
        let deleting = matches!(command, Command::DeleteCategory);
        let delete_category = deleting
            .then(|| {
                self.organization
                    .category
                    .read(cx)
                    .selected_value()
                    .cloned()
            })
            .flatten();
        let needs_category = deleting && delete_category.is_none();
        let fields = Form::new()
            .when(!matches!(command, Command::RenamePreset), |fields| {
                let label = if matches!(command, Command::MovePreset) {
                    "Destination category"
                } else {
                    "User category"
                };
                fields.child(
                    Field::new().label(label).child(
                        Select::new(&self.organization.category)
                            .id("synthesis-organize-category")
                            .accessibility_label(label)
                            .disabled(busy),
                    ),
                )
            })
            .when(
                matches!(command, Command::RenamePreset | Command::RenameCategory),
                |fields| {
                    fields.child(
                        Field::new().label("New name").child(
                            Input::new(&self.organization.name)
                                .id("synthesis-organize-name")
                                .aria_label("New name")
                                .disabled(busy),
                        ),
                    )
                },
            );
        Some(
            div()
                .v_flex()
                .gap_2()
                .child(fields)
                .when_some(delete_category, |form, name| {
                    let message =
                        format!("Permanently delete ‘{name}’ and all its files and subfolders?");
                    form.child(
                        div()
                            .id("synthesis-category-delete-message")
                            .aria_label(message.clone())
                            .test_support()
                            .child(message),
                    )
                })
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("synthesis-organize-cancel")
                                .small()
                                .ghost()
                                .label("Cancel")
                                .disabled(busy)
                                .on_click(cx.listener(|browser, _, window, cx| {
                                    browser.organization.dismiss();
                                    browser.error = None;
                                    browser
                                        .choices
                                        .update(cx, |state, cx| state.focus(window, cx));
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("synthesis-organize-confirm")
                                .small()
                                .label(command.label())
                                .when(deleting, |button| button.danger())
                                .disabled(busy || needs_category)
                                .on_click(cx.listener(|browser, _, _, cx| browser.organize(cx))),
                        ),
                ),
        )
    }
}
