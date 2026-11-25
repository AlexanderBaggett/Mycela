use super::*;
use gpui_kit::{
    RenderImage,
    component::resizable::{ResizablePanel, resizable_panel},
};
use mycela_engine::PointwiseKind;

struct LabSection {
    node: Node,
    label: String,
}

#[derive(Default)]
pub(super) struct LabSections {
    sections: Vec<LabSection>,
    selected: usize,
}

#[derive(PartialEq)]
struct ThumbnailScope {
    section: u64,
    revision: u64,
    time: u64,
    library_root: Option<PathBuf>,
}

pub(super) struct LabThumbnail {
    scope: ThumbnailScope,
    ticket: Ticket,
    result: Option<Result<Arc<RenderImage>, String>>,
}

impl LabThumbnail {
    pub(super) fn raster(&self) -> Option<&Arc<RenderImage>> {
        self.result.as_ref()?.as_ref().ok()
    }
}

impl Drop for LabThumbnail {
    fn drop(&mut self) {
        self.ticket.retire();
    }
}

impl LabSections {
    pub(super) fn load(root: &Node, previous: Option<&Node>) -> Result<Self, String> {
        let Some(
            kind @ (PointwiseKind::TemplateLab(_)
            | PointwiseKind::WizardReference
            | PointwiseKind::WizardGroup),
        ) = root.kind()
        else {
            return Ok(Self::default());
        };
        let mut sections = vec![LabSection {
            node: root.clone(),
            label: section_name(root, kind).unwrap_or_else(|| kind.name().into()),
        }];
        Self::collect(root, "", &mut sections)?;
        let selected = previous
            .and_then(|node| {
                sections
                    .iter()
                    .position(|row| row.node.same_reference(node))
            })
            .unwrap_or_default();
        Ok(Self { sections, selected })
    }

    fn collect(root: &Node, prefix: &str, sections: &mut Vec<LabSection>) -> Result<(), String> {
        for child in root.lab_control_children().map_err(model::failure)? {
            let Some(filter) = child.filter() else {
                continue;
            };
            let kind = filter.kind();
            let mut label = section_name(&child, kind);
            let visible = match kind {
                PointwiseKind::TemplateLab(_) => true,
                PointwiseKind::WizardGroup => !filter
                    .wizard_dynamic_properties()
                    .map_err(model::failure)?
                    .is_empty(),
                PointwiseKind::WizardReference => match filter
                    .wizard_dynamic_properties()
                    .map_err(model::failure)?
                    .first()
                {
                    Some(property) => {
                        let entries = property.options().map_err(model::failure)?.entries();
                        let editable = entries.len() > 1;
                        if label.is_none() {
                            label = if let [Some(name)] = entries.as_slice() {
                                Some(String::from_utf16_lossy(name))
                            } else if editable {
                                let name = property.name();
                                Some(if name.is_empty() {
                                    "Options".into()
                                } else {
                                    String::from_utf16_lossy(&name)
                                })
                            } else {
                                None
                            };
                        }
                        editable
                    }
                    None => false,
                },
                _ => false,
            };
            let path = match (label, visible) {
                (Some(label), _) => section_path(prefix, &label),
                (None, true)
                    if prefix.is_empty() || sections.iter().any(|row| row.label == prefix) =>
                {
                    section_path(prefix, "Settings")
                }
                (None, _) => prefix.into(),
            };
            if visible {
                sections.push(LabSection {
                    node: child.clone(),
                    label: path.clone(),
                });
            }
            Self::collect(&child, &path, sections)?;
        }
        Ok(())
    }

    pub(super) fn selected(&self) -> Option<&Node> {
        self.sections
            .get(self.selected)
            .map(|section| &section.node)
    }

    pub(super) fn selected_label(&self) -> Option<&str> {
        self.sections
            .get(self.selected)
            .map(|section| section.label.as_str())
    }

    pub(super) fn editing_nested(&self) -> bool {
        self.selected != 0
    }

    #[cfg(test)]
    pub(super) fn id_named(&self, label: &str) -> Option<u64> {
        self.sections
            .iter()
            .find(|section| section.label == label)
            .map(|section| section.node.occurrence_id())
    }

    pub(super) fn is_current(&self, root: &Node) -> bool {
        if !self.editing_nested() {
            return true;
        }
        let mut node = self.selected().cloned();
        while let Some(current) = node {
            if current.same_reference(root) {
                return true;
            }
            node = current.parent().filter(|parent| {
                parent
                    .slots()
                    .into_iter()
                    .chain(parent.contents())
                    .any(|slot| {
                        slot.node()
                            .is_some_and(|child| child.same_reference(&current))
                    })
            });
        }
        false
    }
}

fn section_name(node: &Node, kind: PointwiseKind) -> Option<String> {
    node.filter()
        .and_then(|filter| filter.wizard_label())
        .map(|label| String::from_utf16_lossy(&label))
        .or_else(|| matches!(kind, PointwiseKind::TemplateLab(_)).then(|| kind.name().into()))
}

fn section_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.into()
    } else {
        format!("{prefix} / {name}")
    }
}

impl GraphDocumentView {
    pub(super) fn selection_preview_target(&self) -> Option<Node> {
        if self.preview_lab_section {
            self.lab_sections.selected().cloned()
        } else {
            self.selected_node()
        }
    }

    pub(super) fn render_selected_branch(&mut self, cx: &mut Context<Self>) {
        self.set_section_preview(self.lab_sections.editing_nested(), cx);
    }

    pub(super) fn set_section_preview(&mut self, section: bool, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() || self.source_animation.is_some() {
            return;
        }
        let section = section && self.lab_sections.editing_nested();
        if self.preview_lab_section != section {
            self.preview_lab_section = section;
            self.preview = None;
            self.revision += 1;
        }
        self.start_preview(cx);
    }

    pub(super) fn refresh_lab_thumbnail(&mut self, cx: &mut Context<Self>) {
        let Some(section) = self
            .lab_sections
            .selected()
            .filter(|_| !self.closed && self.lab_sections.sections.len() > 1)
        else {
            self.lab_thumbnail = None;
            return;
        };
        let scope = ThumbnailScope {
            section: section.occurrence_id(),
            revision: self.revision,
            time: self.sample_time.to_bits(),
            library_root: self.environment_library.root().map(Path::to_path_buf),
        };
        if self
            .lab_thumbnail
            .as_ref()
            .is_some_and(|thumbnail| thumbnail.scope == scope)
        {
            return;
        }
        let pending = self.freeze_preview(section, self.sample_time);
        let ticket = Ticket::new();
        self.lab_thumbnail = Some(LabThumbnail {
            scope,
            ticket: ticket.clone(),
            result: None,
        });
        let library = self.environment_library.clone();
        let background = cx.background_executor().clone();
        cx.spawn(async move |owner, cx| {
            let work = ticket.clone();
            let result = background
                .spawn(async move {
                    let frame = pending?.materialize()?;
                    node_previews::make_thumbnail(&frame, false, &work, &library)?
                        .into_iter()
                        .next()
                        .ok_or_else(|| "The Lab section has no preview.".into())
                })
                .await;
            if ticket.is_current() {
                let _ = owner.update(cx, |view, cx| {
                    if let Some(thumbnail) = view.lab_thumbnail.as_mut() {
                        thumbnail.result = Some(result);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn render_lab_section_panel(
        &self,
        busy: bool,
        rem: gpui_kit::Pixels,
        cx: &Context<Self>,
    ) -> Option<ResizablePanel> {
        if self.lab_sections.sections.len() < 2 {
            return None;
        }
        let active = &self.lab_sections.sections[self.lab_sections.selected];
        let revision = self.revision;
        let selected = active.node.occurrence_id();
        let thumbnail = self.lab_thumbnail.as_ref().filter(|thumbnail| {
            thumbnail.scope.section == selected && thumbnail.scope.revision == revision
        });
        let theme = cx.theme();
        let controls = self.lab_sections.sections.iter().map(|section| {
            let id = section.node.occurrence_id();
            Button::new(("graph-lab-section-row", id))
                .label(section.label.clone())
                .tooltip(section.label.clone())
                .w_full()
                .justify_start()
                .small()
                .ghost()
                .selected(id == selected)
                .tab_index(0)
                .disabled(busy)
                .on_click(cx.listener(move |view, _, window, cx| {
                    if view.revision == revision {
                        view.choose_lab_section(id, window, cx);
                    }
                }))
        });
        let mut image = div()
            .id("graph-lab-section-thumbnail")
            .test_support()
            .size(rems(6.))
            .relative()
            .overflow_hidden()
            .rounded(theme.radius)
            .bg(theme.muted);
        if let Some(raster) = self
            .draft_preview_thumbnail(selected)
            .or_else(|| thumbnail.and_then(LabThumbnail::raster))
        {
            image = image
                .role(gpui_kit::Role::Image)
                .aria_label(format!("{} preview", active.label))
                .child(crate::preview_background::checkerboard())
                .child(img(raster.clone()).absolute().size_full());
        } else if thumbnail.is_some_and(|thumbnail| thumbnail.result.is_some()) {
            image = image
                .flex()
                .items_center()
                .justify_center()
                .p_1()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Preview unavailable");
        }
        Some(
            resizable_panel()
                .size(rem * 12.)
                .size_range(rem * 9. ..rem * 22.)
                .child(
                    div()
                        .id("graph-lab-section-panel")
                        .test_support()
                        .v_flex()
                        .size_full()
                        .min_h_0()
                        .bg(theme.background)
                        .child(div().p_3().font_semibold().child("Lab sections"))
                        .child(div().px_3().pb_2().child(image))
                        .child(
                            div()
                                .id("graph-lab-section-scroll")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scrollbar()
                                .child(div().p_2().v_flex().gap_1().children(controls)),
                        ),
                ),
        )
    }

    fn choose_lab_section(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.is_busy() {
            return;
        }
        let Some(index) = self
            .lab_sections
            .sections
            .iter()
            .position(|row| row.node.occurrence_id() == id)
        else {
            return;
        };
        if self.lab_sections.selected == index {
            return;
        }
        if self.inspector.is_changed(cx) {
            self.error = Some("Correct or reset field text before changing Lab sections.".into());
            cx.notify();
            return;
        }
        let section = self.lab_sections.sections[index].node.clone();
        let was_previewing_section = self.preview_lab_section;
        self.revision += 1;
        self.error = None;
        self.select_with_lab_section(self.selection.clone(), Some(&section), window, cx);
        if was_previewing_section {
            self.preview = None;
        }
        if was_previewing_section || self.preview_pending || self.animated_preview_pending {
            self.start_preview(cx);
        } else {
            self.refresh_lab_thumbnail(cx);
        }
    }
}
