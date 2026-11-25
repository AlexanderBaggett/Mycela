//! Preset authoring over the active document's first-input chain.
use super::{
    GraphDocumentView, Intent, Target, Ticket, Work, guard,
    overview_presets::{Catalog, Kind, Preset},
    preferences,
};
use gpui_kit::base::{Disableable, Selectable, StyledExt, TestSupportExt};
use gpui_kit::component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    scroll::ScrollableElement,
};
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, IntoElement, ObjectFit, RenderImage,
    Subscription, Window, div, img, prelude::*, rems,
};
use mycela_engine::OverviewPresetMode;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

const PAGE_SIZE: usize = 24;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Texture,
    Append,
    Replace,
}

impl Mode {
    fn kind(self) -> Kind {
        match self {
            Self::Texture => Kind::Texture,
            Self::Append | Self::Replace => Kind::Filter,
        }
    }

    fn operation(self) -> OverviewPresetMode {
        match self {
            Self::Texture => OverviewPresetMode::ReplaceTexture,
            Self::Append => OverviewPresetMode::AppendFilter,
            Self::Replace => OverviewPresetMode::ReplaceFilter,
        }
    }
}

enum CatalogState {
    Unloaded,
    Loading(Ticket),
    Ready(Catalog),
    Failed(String),
}

pub(super) struct Overview {
    catalog: CatalogState,
    mode: Mode,
    search: Entity<InputState>,
    _search_subscription: Subscription,
    category: Option<Vec<String>>,
    user_root: Option<PathBuf>,
    show_properties: bool,
    page: usize,
    thumbnails: HashMap<PathBuf, Arc<RenderImage>>,
    thumbnail_ticket: Option<Ticket>,
}

impl Overview {
    pub(super) fn new(window: &mut Window, cx: &mut Context<GraphDocumentView>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search presets"));
        let search_subscription = cx.subscribe(&search, |view, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                view.overview.page = 0;
                view.refresh_overview_thumbnails(cx);
            }
        });
        Self {
            catalog: CatalogState::Unloaded,
            mode: Mode::Texture,
            search,
            _search_subscription: search_subscription,
            category: None,
            user_root: preferences::File::OverviewPresetFolder.read_path(cx),
            show_properties: false,
            page: 0,
            thumbnails: HashMap::new(),
            thumbnail_ticket: None,
        }
    }

    pub(super) fn retire(&mut self) {
        if let CatalogState::Loading(ticket) = &self.catalog {
            ticket.retire();
        }
        if let Some(ticket) = self.thumbnail_ticket.take() {
            ticket.retire();
        }
    }

    fn entries(&self) -> &[Preset] {
        match &self.catalog {
            CatalogState::Ready(catalog) => catalog.entries(self.mode.kind()),
            _ => &[],
        }
    }

    fn matches<'a>(&'a self, cx: &App) -> Vec<&'a Preset> {
        let query = self.search.read(cx).value();
        self.entries()
            .iter()
            .filter(|entry| {
                self.category
                    .as_ref()
                    .is_none_or(|category| entry.categories().starts_with(category))
                    && entry.matches(query.trim())
            })
            .collect()
    }

    fn visible<'a>(&'a self, matches: &[&'a Preset]) -> Vec<&'a Preset> {
        matches
            .iter()
            .skip(self.page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .copied()
            .collect()
    }
}

struct Stage {
    key: String,
    name: String,
    has_node: bool,
}

impl GraphDocumentView {
    fn overview_stages(&self, branch: &str) -> Vec<Stage> {
        let mut seen = HashSet::new();
        let mut stages = Vec::new();
        let mut key = branch.to_owned();
        while let Some(target) = self.navigation.target(&key) {
            let node = target.node();
            if let Some(node) = &node
                && !seen.insert(node.occurrence_id())
            {
                break;
            }
            stages.push(Stage {
                key: key.clone(),
                name: node.as_ref().map_or_else(
                    || "Empty texture".into(),
                    |node| String::from_utf16_lossy(&node.identity_name()),
                ),
                has_node: node.is_some(),
            });
            let Some(node) = node else { break };
            let next = if node.contents().is_empty() {
                node.slot(0).ok()
            } else {
                node.reference_inputs()
                    .ok()
                    .and_then(|inputs| inputs.into_iter().next())
            };
            let Some(next) = next else { break };
            let Some(row) = self.navigation.rows.iter().find(|row| {
                row.key
                    .rsplit_once('/')
                    .is_some_and(|(parent, _)| parent == key)
                    && matches!(&row.target, Target::Slot(slot) if slot.same_reference(&next))
            }) else {
                break;
            };
            key.clone_from(&row.key);
        }
        stages.reverse();
        stages
    }

    pub(super) fn load_overview_catalog(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.overview.catalog,
            CatalogState::Loading(_) | CatalogState::Ready(_)
        ) {
            return;
        }
        let ticket = Ticket::new();
        self.overview.catalog = CatalogState::Loading(ticket.clone());
        let user_root = self.overview.user_root.clone();
        let background = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let job = ticket.clone();
            let catalog = background
                .spawn(async move {
                    match user_root {
                        Some(root) => Catalog::discover_with_user(Some(&root), &job),
                        None => Catalog::discover(&job),
                    }
                })
                .await;
            if !ticket.is_current() {
                return;
            }
            let _ = owner.update(cx, |view, cx| {
                view.overview.catalog = match catalog {
                    Ok(Some(catalog)) => CatalogState::Ready(catalog),
                    Ok(None) => CatalogState::Unloaded,
                    Err(error) => CatalogState::Failed(error),
                };
                view.refresh_overview_thumbnails(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn refresh_overview_thumbnails(&mut self, cx: &mut Context<Self>) {
        if let Some(ticket) = self.overview.thumbnail_ticket.take() {
            ticket.retire();
        }
        self.overview.thumbnails.clear();
        let entries: Vec<_> = self
            .overview
            .visible(&self.overview.matches(cx))
            .into_iter()
            .cloned()
            .collect();
        if !entries.is_empty() {
            let ticket = Ticket::new();
            self.overview.thumbnail_ticket = Some(ticket.clone());
            let background = cx.background_executor().clone();
            cx.spawn(async move |owner, cx| {
                let job = ticket.clone();
                let thumbnails = background
                    .spawn(async move {
                        entries
                            .into_iter()
                            .take_while(|_| job.is_current())
                            .filter_map(|entry| {
                                let image = entry.thumbnail().ok().flatten()?;
                                Some((entry.path().to_owned(), image))
                            })
                            .collect()
                    })
                    .await;
                if ticket.is_current() {
                    let _ = owner.update(cx, |view, cx| {
                        view.overview.thumbnails = thumbnails;
                        view.overview.thumbnail_ticket = None;
                        cx.notify();
                    });
                }
            })
            .detach();
        }
        cx.notify();
    }

    fn set_overview_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if mode != self.overview.mode {
            self.overview.mode = mode;
            self.overview.category = None;
            self.overview.page = 0;
            self.refresh_overview_thumbnails(cx);
        }
    }

    pub(super) fn update_overview_stage_mode(&mut self, key: &str, cx: &mut Context<Self>) {
        let branch = key.split('/').next().unwrap_or_default();
        let stages = self.overview_stages(branch);
        if let Some(index) = stages.iter().position(|stage| stage.key == key) {
            self.set_overview_mode(
                if index == 0 {
                    Mode::Texture
                } else {
                    Mode::Replace
                },
                cx,
            );
        }
    }

    fn choose_overview_preset_folder(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() || self.closed {
            return;
        }
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::ChoosingOverviewPresetFolder);
        let prompt = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a preset folder".into()),
        });
        let background = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let result = match prompt.await {
                Ok(Ok(Some(mut folders))) if folders.len() == 1 => {
                    let folder = folders.remove(0);
                    let job = ticket.clone();
                    background
                        .spawn(async move {
                            Catalog::discover_with_user(Some(&folder), &job)
                                .map(|catalog| catalog.map(|catalog| (folder, catalog)))
                        })
                        .await
                }
                Ok(Ok(None)) => Ok(None),
                Ok(Ok(Some(_))) => Err("Choose one preset folder.".into()),
                Ok(Err(error)) => Err(format!("Couldn’t open the folder picker: {error}")),
                Err(_) => Ok(None),
            };
            let _ = owner.update(cx, |view, cx| {
                if !view.owns(&ticket) {
                    return;
                }
                view.work = None;
                view.ticket = None;
                match result {
                    Ok(Some((folder, catalog))) => {
                        view.error = preferences::File::OverviewPresetFolder
                            .write_path(&folder, cx)
                            .err();
                        view.overview.retire();
                        view.overview.catalog = CatalogState::Ready(catalog);
                        view.overview.user_root = Some(folder);
                        view.overview.category = None;
                        view.overview.page = 0;
                        view.refresh_overview_thumbnails(cx);
                    }
                    Ok(None) => {}
                    Err(error) => view.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_overview(
        &self,
        busy: bool,
        inspector: AnyElement,
        preview: AnyElement,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let selection = self.selection.as_deref();
        let branches = || {
            self.navigation
                .rows
                .iter()
                .filter(|row| matches!(row.target, Target::Branch(..)))
        };
        let selected_branch = selection
            .and_then(|key| key.split('/').next())
            .and_then(|key| branches().find(|row| row.key == key))
            .or_else(|| branches().next());
        let revision = self.revision;
        let branch_buttons = branches().map(|row| {
            let key = row.key.clone();
            Button::new(format!("overview-branch-{key}"))
                .small()
                .label(row.label.replacen(": ", " · ", 1))
                .selected(selected_branch.is_some_and(|selected| row.key == selected.key))
                .tab_index(0)
                .disabled(busy)
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.select_overview_target(&key, revision, window, cx);
                }))
        });
        let stages = selected_branch.map_or_else(Vec::new, |row| self.overview_stages(&row.key));
        let stage_buttons = stages.iter().map(|stage| {
            let key = stage.key.clone();
            Button::new(format!("overview-stage-{key}"))
                .small()
                .label(stage.name.clone())
                .selected(selection == Some(key.as_str()))
                .tab_index(0)
                .disabled(busy)
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.select_overview_target(&key, revision, window, cx);
                }))
        });
        let principal = selected_branch.filter(|row| matches!(row.target, Target::Branch(0, _)));
        let stage_filter = stages
            .iter()
            .skip(1)
            .find(|stage| Some(stage.key.as_str()) == selection);
        let command = match self.overview.mode {
            Mode::Texture => stages.first().map(|stage| stage.key.clone()),
            Mode::Append => principal
                .filter(|row| row.target.node().is_some())
                .map(|row| row.key.clone()),
            Mode::Replace => stage_filter
                .filter(|stage| stage.has_node)
                .map(|stage| stage.key.clone()),
        };

        div()
            .id("graph-overview-workspace")
            .test_support()
            .v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(theme.background)
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .child(div().font_semibold().child("Texture steps"))
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("overview-properties")
                                    .small()
                                    .label(if self.overview.show_properties {
                                        "Show presets"
                                    } else {
                                        "Properties"
                                    })
                                    .tab_index(0)
                                    .disabled(busy)
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.overview.show_properties =
                                            !view.overview.show_properties;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("graph-overview-open-nodes")
                                    .small()
                                    .label("Open Nodes")
                                    .tab_index(0)
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|view, _, _, cx| view.show_nodes_workspace(cx)),
                                    ),
                            ),
                    ),
            )
            .when(branches().count() > 1, |overview| {
                overview.child(
                    div()
                        .h_10()
                        .min_w_0()
                        .flex_none()
                        .overflow_x_scrollbar()
                        .id("overview-branch-scroll")
                        .child(div().h_flex().gap_2().px_4().children(branch_buttons)),
                )
            })
            .child(
                div()
                    .h_11()
                    .flex_none()
                    .min_w_0()
                    .border_b_1()
                    .border_color(theme.border)
                    .overflow_x_scrollbar()
                    .id("overview-stage-scroll")
                    .child(div().h_flex().gap_2().px_4().children(stage_buttons)),
            )
            .when(self.overview.show_properties, |overview| {
                use gpui_kit::component::resizable::{h_resizable, resizable_panel};
                let rem = window.rem_size();
                overview.child(
                    div().flex_1().min_h_0().child(
                        h_resizable("overview-properties-panels")
                            .child(
                                resizable_panel()
                                    .size_range(rem * 13. ..gpui_kit::Pixels::MAX)
                                    .child(preview),
                            )
                            .child(
                                resizable_panel()
                                    .size(rem * 22.)
                                    .size_range(rem * 16. ..rem * 34.)
                                    .child(inspector),
                            ),
                    ),
                )
            })
            .when(
                !self.overview.show_properties && principal.is_some(),
                |overview| {
                    overview
                        .child(self.render_overview_modes(
                            busy,
                            stage_filter.is_some(),
                            principal.is_some_and(|row| row.target.node().is_some()),
                            cx,
                        ))
                        .child(self.render_overview_presets(command, busy, cx))
                },
            )
            .when(
                !self.overview.show_properties && principal.is_none(),
                |overview| {
                    overview.child(
                        div()
                            .px_4()
                            .py_3()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("Edit additional branches in Nodes."),
                    )
                },
            )
    }

    fn render_overview_modes(
        &self,
        busy: bool,
        can_replace: bool,
        can_append: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let modes = [
            (
                Mode::Texture,
                "overview-texture-presets",
                "Texture presets",
                true,
            ),
            (
                Mode::Append,
                "overview-add-filter",
                "Add filter",
                can_append,
            ),
            (
                Mode::Replace,
                "overview-replace-filter",
                "Replace selected filter",
                can_replace,
            ),
        ];
        div()
            .h_flex()
            .flex_wrap()
            .gap_2()
            .px_4()
            .py_2()
            .children(modes.map(|(mode, id, label, available)| {
                Button::new(id)
                    .small()
                    .label(label)
                    .selected(mode == self.overview.mode)
                    .tab_index(0)
                    .disabled(busy || !available)
                    .on_click(cx.listener(move |view, _, _, cx| view.set_overview_mode(mode, cx)))
            }))
            .when(self.selected_node().is_some(), |row| {
                row.child(
                    Button::new("overview-lab-presets")
                        .small()
                        .ghost()
                        .label("Lab presets…")
                        .tab_index(0)
                        .disabled(busy)
                        .on_click(
                            cx.listener(|view, _, window, cx| view.open_lab_presets(window, cx)),
                        ),
                )
            })
    }

    fn render_overview_presets(
        &self,
        command: Option<String>,
        busy: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let mode = self.overview.mode;
        let theme = cx.theme();
        let ready = matches!(self.overview.catalog, CatalogState::Ready(_));
        let categories: Vec<_> = self
            .overview
            .entries()
            .iter()
            .flat_map(|entry| {
                (1..=entry.categories().len()).map(|depth| entry.categories()[..depth].to_vec())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let label = self
            .overview
            .category
            .as_ref()
            .map_or_else(|| "All categories".into(), |path| path.join(" / "));
        let owner = cx.weak_entity();
        let matches = self.overview.matches(cx);
        let tiles = self.overview.visible(&matches);
        let page = self.overview.page;
        let pages = matches.len().div_ceil(PAGE_SIZE);
        let revision = self.revision;
        let thumbnail = self.preview.as_ref().map(|preview| preview.raster.clone());
        let info = match &self.overview.catalog {
            CatalogState::Unloaded | CatalogState::Loading(_) => Some("Loading presets…".into()),
            CatalogState::Failed(error) => Some(error.clone()),
            CatalogState::Ready(_) if self.overview.entries().is_empty() => {
                Some("No presets in this library.".into())
            }
            CatalogState::Ready(_) if matches.is_empty() => Some("No matching presets.".into()),
            CatalogState::Ready(_) => None,
        };

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .px_4()
            .pb_3()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        Button::new("overview-preset-category")
                            .small()
                            .label(label)
                            .tab_index(0)
                            .disabled(busy || !ready || categories.is_empty())
                            .dropdown_menu(move |mut menu, _, _| {
                                for (label, category) in
                                    std::iter::once(("All categories".into(), None)).chain(
                                        categories
                                            .iter()
                                            .map(|path| (path.join(" / "), Some(path.clone()))),
                                    )
                                {
                                    let owner = owner.clone();
                                    menu = menu.item(PopupMenuItem::new(label).on_click(
                                        move |_, _, cx| {
                                            let _ = owner.update(cx, |view, cx| {
                                                view.overview.category = category.clone();
                                                view.overview.page = 0;
                                                view.refresh_overview_thumbnails(cx);
                                            });
                                        },
                                    ));
                                }
                                menu.scrollable(true)
                            }),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.overview.search)
                                .id("overview-preset-search")
                                .aria_label("Search presets")
                                .small()
                                .disabled(busy || !ready),
                        ),
                    )
                    .child(
                        Button::new("overview-preset-folder")
                            .small()
                            .label("Preset folder…")
                            .tab_index(0)
                            .disabled(busy)
                            .on_click(
                                cx.listener(|view, _, _, cx| {
                                    view.choose_overview_preset_folder(cx)
                                }),
                            ),
                    ),
            )
            .when_some(info, |panel, info| {
                panel.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(info),
                )
            })
            .child(
                div()
                    .h_flex()
                    .items_stretch()
                    .gap_3()
                    .min_h_0()
                    .flex_1()
                    .child(
                        div()
                            .id("overview-preset-scroll")
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .child(
                                div()
                                    .h_flex()
                                    .flex_wrap()
                                    .content_start()
                                    .gap_2()
                                    .pb_2()
                                    .children(tiles.into_iter().map(|entry| {
                                        let preset = entry.clone();
                                        let key = command.clone();
                                        let image =
                                            self.overview.thumbnails.get(entry.path()).cloned();
                                        let name = entry.name().to_owned();
                                        let category = entry.categories().join(" / ");
                                        Button::new(format!(
                                            "overview-preset-{}",
                                            entry.path().display()
                                        ))
                                        .outline()
                                        .w(rems(9.))
                                        .h(rems(6.5))
                                        .tab_index(0)
                                        .accessibility_label(name.clone())
                                        .tooltip(format!("{name} · {category}"))
                                        .disabled(busy || key.is_none())
                                        .child(
                                            div()
                                                .v_flex()
                                                .items_center()
                                                .min_w_0()
                                                .gap_1()
                                                .child(div().size(rems(4.)).when_some(
                                                    image,
                                                    |tile, image| {
                                                        tile.child(
                                                            img(image)
                                                                .size_full()
                                                                .object_fit(ObjectFit::Contain),
                                                        )
                                                    },
                                                ))
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .max_w(rems(8.))
                                                        .truncate()
                                                        .text_sm()
                                                        .child(name),
                                                ),
                                        )
                                        .on_click(
                                            cx.listener(move |view, _, _, cx| {
                                                if let Some(key) = &key {
                                                    view.apply_overview_choice(
                                                        mode,
                                                        key,
                                                        revision,
                                                        preset.clone(),
                                                        cx,
                                                    );
                                                }
                                            }),
                                        )
                                    })),
                            ),
                    )
                    .when_some(thumbnail, |row, image| {
                        row.child(
                            div()
                                .flex_none()
                                .w(rems(10.))
                                .v_flex()
                                .gap_1()
                                .child(div().text_sm().child("Preview"))
                                .child(
                                    img(image)
                                        .id("overview-step-preview")
                                        .size(rems(10.))
                                        .object_fit(ObjectFit::Contain),
                                ),
                        )
                    }),
            )
            .when(pages > 1, |panel| {
                panel.child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("overview-presets-previous")
                                .small()
                                .label("Previous page")
                                .disabled(busy || page == 0)
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.overview.page = view.overview.page.saturating_sub(1);
                                    view.refresh_overview_thumbnails(cx);
                                })),
                        )
                        .child(div().text_sm().child(format!("{} / {pages}", page + 1)))
                        .child(
                            Button::new("overview-presets-next")
                                .small()
                                .label("Next page")
                                .disabled(busy || page + 1 >= pages)
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.overview.page += 1;
                                    view.refresh_overview_thumbnails(cx);
                                })),
                        ),
                )
            })
    }

    fn apply_overview_choice(
        &mut self,
        mode: Mode,
        key: &str,
        revision: u64,
        preset: Preset,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() || self.closed || self.revision != revision || !self.overview_open {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error =
                Some("Correct or reset field text in Nodes before choosing a preset.".into());
            cx.notify();
            return;
        }
        let Some(target) = self.navigation.target(key) else {
            return;
        };
        if !self.canvas_target_current(&target) {
            return;
        }
        let slot = match target {
            Target::Branch(0, _) => None,
            Target::Slot(slot) => Some(slot),
            _ => return,
        };
        let ticket = Ticket::new();
        self.ticket = Some(ticket.clone());
        self.work = Some(Work::ApplyingOverviewPreset);
        self.error = None;
        self.status = None;
        cx.notify();
        let background = cx.background_executor().clone();
        let window = self.window;
        cx.spawn(async move |owner, cx| {
            let job = ticket.clone();
            let result = background
                .spawn(async move {
                    preset.read(&job).map(|document| {
                        document.map(|document| (preset.import_warning(&document), document))
                    })
                })
                .await;
            let _ = cx.update_window(window, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    if !view.owns(&ticket) {
                        return;
                    }
                    view.ticket = None;
                    view.work = None;
                    match result {
                        Ok(Some((warning, preset))) if view.revision == revision => {
                            view.ensure_history_binding();
                            let dirty = view.dirty;
                            let stamp = view.begin_history_edit();
                            let result = guard(&view.document).apply_overview_preset(
                                0,
                                slot.as_ref(),
                                preset,
                                mode.operation(),
                            );
                            match result {
                                Ok(node) => {
                                    view.finish_history_edit(stamp, true);
                                    view.status = warning;
                                    view.revision += 1;
                                    view.selection_serial += 1;
                                    view.rebuild(window, cx);
                                    let key = view.navigation.rows.iter().find_map(|row| {
                                        row.target
                                            .node()
                                            .is_some_and(|current| current.same_reference(&node))
                                            .then(|| row.key.clone())
                                    });
                                    view.select(key, window, cx);
                                    view.start_preview(cx);
                                }
                                Err(error) => {
                                    view.dirty = dirty;
                                    view.error = Some(error.to_string());
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(error) => view.error = Some(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn select_overview_target(
        &mut self,
        key: &str,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.revision != revision || self.navigation.target(key).is_none() || self.is_busy() {
            return;
        }
        if self.selection.as_deref() == Some(key) {
            self.update_overview_stage_mode(key, cx);
            return;
        }
        self.selection_serial += 1;
        self.request(Intent::Select(key.to_owned()), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ForegroundError;
    use gpui_kit::{AnyWindowHandle, ScrollStrategy, TestAppContext, test::TestWindowExt};
    use mycela_engine::{
        BinaryWriter, Gtx375GraphDocument, LabNumericValue, PointwiseFilter, PointwiseKind,
        PointwiseNode,
    };

    fn choose_preset(
        cx: &mut TestAppContext,
        handle: AnyWindowHandle,
        view: &Entity<GraphDocumentView>,
        term: &str,
    ) {
        cx.update_window(handle, |_, window, cx| {
            window.click("overview-preset-search", cx);
            window.press("secondary-a", cx);
            window.input(term, cx);
        })
        .unwrap();
        cx.run_until_parked();
        let id = cx.update(|cx| {
            let view = view.read(cx);
            let matches = view.overview.matches(cx);
            let entry = matches.iter().find(|entry| entry.name() == term).unwrap();
            format!("overview-preset-{}", entry.path().display())
        });
        cx.update_window(handle, |_, window, cx| window.click(id, cx))
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn overview_builds_and_replaces_the_filter_on_the_active_texture(cx: &mut TestAppContext) {
        use super::super::{Node, open_node_editor_window};

        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (handle, view) = cx.update(open_node_editor_window).unwrap();
        let initial = cx.update(|cx| view.read(cx).selected_node().unwrap());
        cx.update_window(handle, |_, window, cx| {
            window.within("graph-workspace-tabs").click(0, cx);
        })
        .unwrap();
        cx.run_until_parked();

        let choose = |cx: &mut TestAppContext, term| choose_preset(cx, handle, &view, term);
        let root = |cx: &mut TestAppContext| -> Node {
            cx.update(|cx| {
                let view = view.read(cx);
                assert!(view.error().is_none(), "{:?}", view.error());
                assert!(view.path().is_none());
                guard(&view.document).branch(0).unwrap().unwrap()
            })
        };

        choose(cx, "Leather");
        let texture = root(cx);
        assert!(!texture.same_reference(&initial));
        cx.update_window(handle, |_, window, cx| {
            window.click("overview-add-filter", cx)
        })
        .unwrap();
        cx.run_until_parked();
        choose(cx, "Emboss");
        let filter = root(cx);
        assert!(
            filter
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&texture)
        );

        cx.update_window(handle, |_, window, cx| {
            window.click("overview-replace-filter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        choose(cx, "Emboss");
        let replacement = root(cx);
        assert!(!replacement.same_reference(&filter));
        assert!(
            replacement
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&texture)
        );

        cx.update_window(handle, |_, window, cx| {
            window.click("graph-edit-menu", cx);
            let mut menu = window.within("popup-menu");
            assert_eq!(menu.find(0).label(), Some("Undo"));
            menu.click(0, cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(root(cx).same_reference(&filter));
        cx.update_window(handle, |_, window, cx| {
            window.click("overview-properties", cx);
            assert!(window.find("graph-sample-time").visible());
            assert_eq!(
                window.find("overview-properties").label(),
                Some("Show presets")
            );
            window.click("graph-overview-open-nodes", cx);
            assert!(window.find("graph-canvas-scroll").visible());
        })
        .unwrap();
        assert_eq!(cx.windows(), vec![handle]);
    }

    #[gpui_kit::test]
    #[ignore = "requires the installed Genetica preset libraries"]
    fn overview_applies_cracked_photo_and_edits_its_patch_frequency(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (handle, view) = cx.update(crate::open_desktop_window).unwrap();
        // Exercise the missing-library fallback even on hosts with an installed photo library.
        cx.update(|cx| view.update(cx, |view, _| view.environment_library = Default::default()));
        let click = |cx: &mut TestAppContext, id: &str| {
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                window.click(id.to_owned(), cx);
            })
            .unwrap();
            cx.run_until_parked();
        };
        let menu_item = |cx: &mut TestAppContext, id: &str, label: &str| {
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                window.click(id.to_owned(), cx);
                let mut menu = window.within("popup-menu");
                let entry = (0usize..64)
                    .find(|index| {
                        menu.try_find(*index)
                            .is_some_and(|row| row.label() == Some(label))
                    })
                    .unwrap_or_else(|| panic!("Missing {label} in {id}"));
                for _ in 0..64 {
                    menu.press("down", cx);
                    if menu.find(entry).selected() == Some(true) {
                        menu.press("enter", cx);
                        return;
                    }
                }
                panic!("Could not reach {label} in {id}");
            })
            .unwrap();
            cx.run_until_parked();
        };
        let root = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                let view = view.read(cx);
                assert_eq!(view.error(), None);
                guard(&view.document).branch(0).flatten().unwrap()
            })
        };
        let pixels = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                let view = view.read(cx);
                assert_eq!(view.preview_error(), None);
                assert!(!view.preview_pending);
                view.raster_pixels().unwrap().to_vec()
            })
        };
        let adjuster = |cx: &mut TestAppContext, name: &str| {
            cx.update(|cx| {
                let view = view.read(cx);
                guard(&view.document)
                    .numeric_lab_adjusters(view.selected_node().as_ref())
                    .into_iter()
                    .find(|control| control.name() == name)
                    .unwrap_or_else(|| panic!("Missing Lab adjuster {name}"))
                    .value()
            })
        };
        let set_number = |cx: &mut TestAppContext, label: &str, value: &str| {
            menu_item(cx, "graph-lab-adjusters", label);
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                window.click("lab-adjuster-number", cx);
                window.press("secondary-a", cx);
                window.input(value, cx);
            })
            .unwrap();
            click(cx, "ok");
        };

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("graph-workspace-tabs").click(0, cx);
        })
        .unwrap();
        cx.run_until_parked();
        choose_preset(cx, handle, &view, "Leather");
        let leather = root(cx);
        let leather_pixels = pixels(cx);
        click(cx, "overview-add-filter");
        choose_preset(cx, handle, &view, "Cracked Photo");
        let filter = root(cx);
        assert!(!filter.is_filter());
        assert!(
            filter
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&leather)
        );

        click(cx, "overview-properties");
        let cracked_pixels = pixels(cx);
        assert_ne!(cracked_pixels, leather_pixels);
        assert_eq!(adjuster(cx, "Cracks"), LabNumericValue::Number(30.));
        set_number(cx, "Cracks…", "100");
        assert_eq!(adjuster(cx, "Cracks"), LabNumericValue::Number(100.));
        assert_ne!(pixels(cx), cracked_pixels);
        menu_item(cx, "graph-edit-menu", "Undo");
        assert_eq!(pixels(cx), cracked_pixels);

        click(cx, "graph-overview-open-nodes");
        click(cx, "graph-view-list");
        let synthesis_row = cx.update(|cx| {
            let graph = view.read(cx);
            let row = graph
                .navigation
                .rows
                .iter()
                .find(|row| {
                    row.target
                        .node()
                        .is_some_and(|node| node.kind() == Some(PointwiseKind::Synthesis))
                })
                .unwrap();
            let index = graph
                .tree
                .read(cx)
                .index_of(&row.key.clone().into())
                .unwrap();
            let tree = graph.tree.clone();
            let key = row.key.clone();
            tree.update(cx, |tree, cx| {
                tree.scroll_to_item(index, ScrollStrategy::Center);
                cx.notify();
            });
            key
        });
        click(cx, &synthesis_row);
        let synthesis_pixels = pixels(cx);
        assert_eq!(adjuster(cx, "Tears"), LabNumericValue::Number(0.));
        set_number(cx, "Tears…", "100");
        assert_eq!(adjuster(cx, "Tears"), LabNumericValue::Number(100.));
        let more_tears = pixels(cx);
        assert_ne!(more_tears, synthesis_pixels);
        menu_item(cx, "graph-edit-menu", "Undo");
        assert_eq!(pixels(cx), synthesis_pixels);
        menu_item(cx, "graph-edit-menu", "Redo");
        assert_eq!(pixels(cx), more_tears);
        menu_item(cx, "graph-edit-menu", "Undo");
        menu_item(cx, "graph-edit-menu", "Undo");
        assert!(root(cx).same_reference(&leather));
        assert_eq!(pixels(cx), leather_pixels);
        assert_eq!(cx.windows(), vec![handle]);
    }

    #[gpui_kit::test]
    fn overview_opens_the_selected_stage_in_nodes_without_editing_the_graph(
        cx: &mut TestAppContext,
    ) {
        use super::super::{Prepared, Ticket, open_node_editor_window};

        cx.update(gpui_kit::init);
        cx.update(|cx| cx.set_reduce_motion(true));
        let (handle, view) = cx.update(open_node_editor_window).unwrap();

        let exposure = PointwiseFilter::<ForegroundError>::new(PointwiseKind::Exposure).unwrap();
        let solid = PointwiseNode::solid_color().unwrap();
        exposure
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&solid))
            .unwrap();
        let other = PointwiseFilter::new(PointwiseKind::Colorize).unwrap();
        let mut document =
            Gtx375GraphDocument::new(vec![Some(exposure.node()), Some(other.node())]).unwrap();
        let mut bytes = BinaryWriter::default();
        document.save_project(&mut bytes).unwrap();
        let prepared = Prepared::decode(
            "overview-example.mycela".into(),
            bytes.bytes().to_vec(),
            &Ticket::new(),
            None,
        )
        .unwrap()
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| view.install(prepared, window, cx));
        })
        .unwrap();
        cx.run_until_parked();

        let (first_branch, second_branch, solid_stage, initial) = cx.update(|cx| {
            let view = view.read(cx);
            let branch = |index| {
                view.navigation
                    .rows
                    .iter()
                    .find(
                        |row| matches!(row.target, Target::Branch(current, _) if current == index),
                    )
                    .unwrap()
                    .key
                    .clone()
            };
            let stage = view
                .navigation
                .rows
                .iter()
                .find(|row| {
                    matches!(row.target, Target::Slot(_)) && row.label.ends_with("Solid Color")
                })
                .unwrap()
                .key
                .clone();
            (branch(0), branch(1), stage, view.selection.clone())
        });

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.within("graph-workspace-tabs").click(0, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).selection, initial));

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("graph-overview-workspace").is_some());
            assert!(
                window
                    .try_find(format!("overview-stage-{solid_stage}"))
                    .is_some()
            );
            window.click(format!("overview-branch-{second_branch}"), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).selection.as_ref(), Some(&second_branch)));

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find(format!("overview-stage-{solid_stage}"))
                    .is_none()
            );
            assert!(
                window
                    .try_find(format!("overview-stage-{second_branch}"))
                    .is_some()
            );
            window.click(format!("overview-branch-{first_branch}"), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click(format!("overview-stage-{solid_stage}"), cx);
        })
        .unwrap();
        cx.run_until_parked();

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("graph-overview-open-nodes", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("graph-overview-workspace").is_none());
            assert!(window.try_find("graph-view-list").is_some());
            assert_eq!(view.read(cx).selection.as_ref(), Some(&solid_stage));
            assert!(!view.read(cx).is_dirty());
        })
        .unwrap();
    }
}
