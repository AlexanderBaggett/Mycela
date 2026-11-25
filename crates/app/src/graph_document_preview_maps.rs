use super::{GraphDocumentView, Preview};
use crate::solid_color::map_preview::PreviewSelection;
use gpui_kit::base::Disableable;
use gpui_kit::component::{
    Sizable,
    button::Button,
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{App, Context, IntoElement, Styled};

#[derive(Default)]
pub(super) struct PreviewMaps {
    target: Option<u64>,
    requested: PreviewSelection,
}

impl PreviewMaps {
    pub(super) fn requested_for(&self, target: u64) -> PreviewSelection {
        if self.target == Some(target) {
            self.requested.clone()
        } else {
            PreviewSelection::BaseColor
        }
    }

    pub(super) fn publish_target(&mut self, target: u64) {
        self.requested = self.requested_for(target);
        self.target = Some(target);
    }

    pub(super) fn publish_still(&mut self, target: u64, preview: &Preview) {
        self.target = Some(target);
        self.requested = preview.selection.clone();
    }
}

impl GraphDocumentView {
    pub(super) fn supports_preview_maps(&self) -> bool {
        self.source_animation.is_none()
            && self.drawing.is_none()
            && self.embedded_environment.is_none()
            && self.embedded_noise.is_none()
            && self.environment_preview.is_none()
    }

    fn showing_graph_preview(&self, cx: &App) -> bool {
        self.supports_preview_maps() && self.levels_draft_preview(cx).is_none()
    }

    fn rendered_graph_preview(&self, cx: &App) -> Option<(&Preview, u64)> {
        let preview = self.preview.as_ref()?;
        if self.preview_pending || !self.showing_graph_preview(cx) {
            return None;
        }
        let target = self.preview_maps.target?;
        self.preview_target()
            .is_some_and(|node| node.occurrence_id() == target)
            .then_some((preview, target))
    }

    pub(super) fn render_preview_map_selector(
        &self,
        busy: bool,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if self.synthesis_preview_mode().is_some() {
            return None;
        }
        let (preview, target) = self
            .rendered_graph_preview(cx)
            .filter(|(preview, _)| preview.map_choices.len() >= 2)?;
        let shown = preview.selection.clone();
        let choices = preview.map_choices.clone();
        let revision = self.revision;
        let owner = cx.weak_entity();
        let label = shown.label();
        Some(
            Button::new("graph-preview-map-selector")
                .label(label.clone())
                .accessibility_label(format!("Preview: {label}"))
                .tooltip(label)
                .max_w_40()
                .min_w_0()
                .small()
                .tab_index(0)
                .disabled(busy)
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for selection in choices.iter() {
                        let owner = owner.clone();
                        let choice = selection.clone();
                        menu = menu.item(
                            PopupMenuItem::new(selection.label())
                                .checked(*selection == shown)
                                .on_click(move |_, _, cx| {
                                    let _ = owner.update(cx, |view, cx| {
                                        view.select_preview_map(
                                            choice.clone(),
                                            target,
                                            revision,
                                            cx,
                                        );
                                    });
                                }),
                        );
                    }
                    menu.scrollable(true)
                }),
        )
    }

    fn select_preview_map(
        &mut self,
        choice: PreviewSelection,
        target: u64,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() || self.revision != revision {
            return;
        }
        let Some((preview, current_target)) = self.rendered_graph_preview(cx) else {
            return;
        };
        if current_target != target
            || (choice == preview.selection && choice == self.preview_maps.requested)
            || !preview.map_choices.contains(&choice)
        {
            return;
        }
        self.preview_maps.requested = choice;
        self.start_preview_job(false, cx);
    }

    pub(super) fn export_preview_map(&self, cx: &App) -> Result<PreviewSelection, String> {
        if !self.showing_graph_preview(cx) {
            return Ok(PreviewSelection::BaseColor);
        }
        let displayed = self.preview.as_ref().map(|preview| &preview.selection);
        if displayed.is_some_and(|selection| selection.name().is_some())
            || self.preview_maps.requested.name().is_some()
        {
            if let Some((preview, _)) = self
                .rendered_graph_preview(cx)
                .filter(|_| self.preview_error.is_none())
            {
                return Ok(preview.selection.clone());
            }
            if self.preview_error.is_some() {
                return Err("The current graph preview could not render its map.".into());
            }
            return Err("Wait for the selected graph preview before exporting its map.".into());
        }
        Ok(PreviewSelection::BaseColor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ForegroundError;
    use gpui_kit::{
        AppContext, Bounds, TestAppContext, WindowBounds, WindowOptions, px, size,
        test::TestWindowExt,
    };
    use mycela_engine::{
        AnalyticCurveKind, BinaryWriter, CurveLayerHandle, Gtx375GraphDocument, PointwiseFilter,
        PointwiseKind, PointwiseNode,
    };

    #[gpui_kit::test]
    fn graph_named_map_preview_exports_the_selected_occurrence(cx: &mut TestAppContext) {
        use super::super::{Prepared, Ticket, scene_tests::assert_graph_preview_matches_png};

        cx.update(gpui_kit::init);
        let (handle, view) = cx
            .update(|cx| {
                gpui_kit::open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            Default::default(),
                            size(px(1400.), px(1200.)),
                        ))),
                        ..Default::default()
                    },
                    cx,
                    |window, cx| cx.new(|cx| GraphDocumentView::new(window, cx)),
                )
            })
            .unwrap();

        let add = PointwiseFilter::<ForegroundError>::new(PointwiseKind::Add).unwrap();
        let name: Vec<_> = "Height".encode_utf16().collect();
        for (slot, color) in [[0.2, 0.4, 0.1], [0.2, 0.2, 0.2]].into_iter().enumerate() {
            let solid = PointwiseNode::solid_color().unwrap();
            solid.set_solid_color(color).unwrap();
            let input = if slot == 1 {
                let exposure = PointwiseFilter::new(PointwiseKind::Exposure).unwrap();
                let stops = exposure.parameter(0).unwrap();
                stops.set_axis_animated(0, true).unwrap();
                let curve = stops.animation_curve(0).unwrap();
                curve.set_minimum(0.);
                curve.set_maximum(2.);
                curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
                exposure
                    .node()
                    .slot(0)
                    .unwrap()
                    .set_node(Some(&solid))
                    .unwrap();
                exposure.node()
            } else {
                solid
            };
            let store = PointwiseFilter::new(PointwiseKind::StoreMap).unwrap();
            store.parameter(0).unwrap().set_text(Some(&name)).unwrap();
            store
                .node()
                .slot(0)
                .unwrap()
                .set_node(Some(&input))
                .unwrap();
            add.node()
                .slot(slot as i32)
                .unwrap()
                .set_node(Some(&store.node()))
                .unwrap();
        }
        let unmapped = PointwiseNode::solid_color().unwrap();
        unmapped.set_solid_color([0.15, 0.45, 0.75]).unwrap();
        let mut document =
            Gtx375GraphDocument::new(vec![Some(add.node()), Some(unmapped)]).unwrap();
        let mut bytes = BinaryWriter::default();
        document.save_project(&mut bytes).unwrap();
        let prepared = Prepared::decode(
            "preview-maps.mycela".into(),
            bytes.bytes().to_vec(),
            &Ticket::new(),
            None,
        )
        .unwrap()
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| view.install(prepared, window, cx));
            window.render_frame(cx);
            window.click("graph-sample-time", cx);
            window.press("secondary-a", cx);
            window.input("0.5", cx);
            window.click("graph-set-sample-time", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("graph-preview-map-selector", cx);
            let mut menu = window.within("popup-menu");
            assert_eq!(menu.find(0usize).label(), Some("Base color"));
            assert_eq!(menu.find(1usize).label(), Some("Map: Height"));
            assert_eq!(menu.find(2usize).label(), Some("Map: Height (2)"));
            for _ in 0..4 {
                if menu.find(2usize).selected() == Some(true) {
                    break;
                }
                menu.press("down", cx);
            }
            assert_eq!(menu.find(2usize).selected(), Some(true));
            menu.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.sample_time, 0.5);
            assert_eq!(
                view.preview.as_ref().unwrap().selection,
                PreviewSelection::Named {
                    name,
                    occurrence: 1
                }
            );
            assert_eq!(
                view.preview_bitmap().unwrap().argb_pixel(0, 0).unwrap(),
                0xff666666
            );
        });
        assert_graph_preview_matches_png(cx, handle, &view, "named-height-second");

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("branch-2", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("graph-preview-map-selector").is_none());
            assert_eq!(
                view.read(cx).export_preview_map(cx).unwrap(),
                PreviewSelection::BaseColor
            );
            window.click("branch-1", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(
                view.preview.as_ref().unwrap().selection,
                PreviewSelection::BaseColor
            );
            assert_ne!(
                view.preview_bitmap().unwrap().argb_pixel(0, 0).unwrap(),
                0xff666666
            );
        });
    }
}
