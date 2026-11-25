//! This window owns its history independently of other open graph windows.
use super::{GraphDocumentView, Node, Target, guard};
use crate::ForegroundError;
use gpui_kit::{Context, Window};
use mycela_engine::{
    ContextUndoInfo, GtxCatalogRecord, GtxGraphError, History, HistoryCommand, HistoryDirection,
    HistoryEffect, HistoryError, HistoryServices, HistoryTarget, PointwiseError,
    SolidColorProperty, WorkspaceHistoryRouter,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) type Router = WorkspaceHistoryRouter<ForegroundError>;
type DocumentHistory = History<ForegroundError>;

#[derive(Default)]
struct Host {
    batching: AtomicBool,
}

impl HistoryServices<ForegroundError> for Host {
    fn route_property_history(
        &self,
        _: &DocumentHistory,
        _: &HistoryTarget<ForegroundError>,
        _: Option<&ContextUndoInfo<ForegroundError>>,
    ) -> Result<Option<DocumentHistory>, HistoryError<ForegroundError>> {
        Err(HistoryError::MissingDependency(
            HistoryEffect::RoutePropertyHistory,
        ))
    }

    fn resolve_command_context(
        &self,
        _: &DocumentHistory,
        _: &HistoryCommand<ForegroundError>,
    ) -> Result<Option<HistoryTarget<ForegroundError>>, HistoryError<ForegroundError>> {
        // Navigation resolves the retained selection key against the graph
        // after travel. There is no separate Nodes View context in this window.
        Ok(None)
    }

    fn effect(
        &self,
        _: &DocumentHistory,
        _: Option<&HistoryCommand<ForegroundError>>,
        effect: HistoryEffect,
    ) -> Result<(), HistoryError<ForegroundError>> {
        match effect {
            HistoryEffect::SetGlobalBatch(value) => self.batching.store(value, Ordering::Release),
            HistoryEffect::BeforeTravel if self.batching.load(Ordering::Acquire) => {
                return Err(HistoryError::Service(ForegroundError::Operation(
                    "A document history operation is already running.".into(),
                )));
            }
            // There is no active contributor profile or resource browser in
            // this graph window. Existing GTX contributor data is retained.
            HistoryEffect::CaptureContributors
            | HistoryEffect::RestoreContributors
            | HistoryEffect::CaptureResourceSelection
            | HistoryEffect::RestoreResourceSelection
            | HistoryEffect::FlushResourceSelection
            // The view reads the current cursor and dirty state synchronously
            // before rendering. Its travel callback retires the preview job.
            | HistoryEffect::BeforeTravel
            | HistoryEffect::RefreshHistory
            | HistoryEffect::DirtyChanged => {}
            _ => return Err(HistoryError::MissingDependency(effect)),
        }
        Ok(())
    }

    fn pending_commands(
        &self,
        _: &DocumentHistory,
        _: &HistoryCommand<ForegroundError>,
    ) -> Result<Vec<HistoryCommand<ForegroundError>>, HistoryError<ForegroundError>> {
        Ok(Vec::new())
    }
}

pub(super) struct EditStamp {
    dirty: bool,
    history: Option<(DocumentHistory, u64)>,
}

impl GraphDocumentView {
    pub(super) fn history(&self) -> Option<DocumentHistory> {
        guard(&self.document).property_history()
    }

    pub(super) fn bind_history(&mut self) {
        let has_lab_controls = guard(&self.document).catalog().is_some_and(|catalog| {
            catalog.into_iter().any(|entry| {
                matches!(
                    entry,
                    GtxCatalogRecord::Numeric { .. } | GtxCatalogRecord::Environment { .. }
                )
            })
        });
        if !has_lab_controls
            && !self
                .navigation
                .rows
                .iter()
                .filter_map(|row| row.target.node())
                .any(|node| {
                    matches!(
                        node.kind(),
                        Some(
                            mycela_engine::PointwiseKind::MakeEnvironment
                                | mycela_engine::PointwiseKind::ConvertEnvironment
                                | mycela_engine::PointwiseKind::DiffuseEnvironment
                                | mycela_engine::PointwiseKind::ScatterEnvironment
                                | mycela_engine::PointwiseKind::Stereogram
                                | mycela_engine::PointwiseKind::RotateEnvironment
                                | mycela_engine::PointwiseKind::Canvas
                                | mycela_engine::PointwiseKind::TemplateLab(_)
                                | mycela_engine::PointwiseKind::WizardReference
                                | mycela_engine::PointwiseKind::Colorize
                                | mycela_engine::PointwiseKind::ColorizeG2
                                | mycela_engine::PointwiseKind::ToneCurve
                                | mycela_engine::PointwiseKind::Frequency
                                | mycela_engine::PointwiseKind::Fractalize
                                | mycela_engine::PointwiseKind::FreeTransform
                                | mycela_engine::PointwiseKind::Kaleidoscope
                                | mycela_engine::PointwiseKind::MoveRepeat
                                | mycela_engine::PointwiseKind::RotateTile
                                | mycela_engine::PointwiseKind::Distort
                                | mycela_engine::PointwiseKind::ChangeBci
                                | mycela_engine::PointwiseKind::ChangeHsb
                                | mycela_engine::PointwiseKind::Exposure
                                | mycela_engine::PointwiseKind::Selector
                                | mycela_engine::PointwiseKind::MatchColor
                                | mycela_engine::PointwiseKind::GetChannel
                                | mycela_engine::PointwiseKind::Add
                                | mycela_engine::PointwiseKind::Multiply
                                | mycela_engine::PointwiseKind::FromHsb
                                | mycela_engine::PointwiseKind::GaussianBlur
                                | mycela_engine::PointwiseKind::MotionBlur
                                | mycela_engine::PointwiseKind::ShapeBlur
                                | mycela_engine::PointwiseKind::Smear
                                | mycela_engine::PointwiseKind::VariableBlur
                                | mycela_engine::PointwiseKind::Sharpen
                                | mycela_engine::PointwiseKind::Atmosphere
                                | mycela_engine::PointwiseKind::PointLight
                                | mycela_engine::PointwiseKind::FullLight
                                | mycela_engine::PointwiseKind::Shadow
                                | mycela_engine::PointwiseKind::ShadowG2
                                | mycela_engine::PointwiseKind::Remap
                                | mycela_engine::PointwiseKind::Reflect
                                | mycela_engine::PointwiseKind::Refract
                                | mycela_engine::PointwiseKind::ReflectEnvironment
                                | mycela_engine::PointwiseKind::RefractEnvironment
                                | mycela_engine::PointwiseKind::Polygon
                                | mycela_engine::PointwiseKind::LegacyBricksBoards
                        )
                    ) || node.uv_texture_output().is_some()
                        || node.gradient_generator_handle().is_some()
                        || [SolidColorProperty::Color, SolidColorProperty::Transparency]
                            .into_iter()
                            .any(|property| {
                                node.retained_solid_animation(property)
                                    .ok()
                                    .flatten()
                                    .is_some_and(|animation| animation.is_active().unwrap_or(false))
                            })
                })
        {
            return;
        }
        self.ensure_history_binding();
    }

    pub(super) fn ensure_history_binding(&mut self) {
        let mut document = guard(&self.document);
        if document.property_history().is_some() {
            return;
        }
        let router = Arc::new(Router::new(Arc::new(Host::default())));
        if let Ok(history) = document.bind_property_history(router.clone()) {
            router.set_active_history(Some(&history));
            self.history_router = Some(router);
        }
    }

    pub(super) fn deactivate_history(&mut self) {
        if let Some(router) = self.history_router.take() {
            router.set_active_history(None);
        }
    }

    pub(super) fn begin_history_edit(&mut self) -> EditStamp {
        let history = self.history().map(|history| {
            let sequence = history.edit_sequence();
            (history, sequence)
        });
        let stamp = EditStamp {
            dirty: self.dirty,
            history,
        };
        self.dirty = true;
        stamp
    }

    pub(super) fn finish_history_edit(&mut self, stamp: EditStamp, fully_recorded: bool) {
        if fully_recorded
            && stamp.history.is_some_and(|(previous, sequence)| {
                self.history().is_some_and(|current| {
                    previous.same_reference(&current) && sequence != current.edit_sequence()
                })
            })
        {
            self.dirty = stamp.dirty;
        }
    }

    pub(super) fn history_available(&self, direction: HistoryDirection) -> bool {
        self.history().is_some_and(|history| match direction {
            HistoryDirection::Undo => history.cursor() > 0,
            HistoryDirection::Redo => usize::try_from(history.cursor())
                .is_ok_and(|cursor| cursor < history.commands().len()),
        })
    }

    pub(super) fn travel_history(
        &mut self,
        direction: HistoryDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closed || self.is_busy() || !self.history_available(direction) {
            return;
        }
        if self.has_field_drafts(cx) {
            self.error = Some("Correct or reset field text before using document history.".into());
            cx.notify();
            return;
        }
        let Some(history) = self.history() else {
            return;
        };
        self.revision += 1;
        self.cancel_preview();
        let result = match direction {
            HistoryDirection::Undo => history.undo(),
            HistoryDirection::Redo => history.redo(),
        };
        self.status = None;
        self.error = result.err().map(|error| error.to_string());
        self.rebuild(window, cx);
        self.start_preview(cx);
    }

    pub(super) fn edit_target(
        &mut self,
        target: &Target,
        node: Option<&Node>,
    ) -> Result<(), String> {
        self.edit_with_history_fallback(|document| match target {
            Target::Branch(index, _) => document.edit_branch(*index, node.cloned()),
            Target::Slot(slot) => document.edit_slot(slot, node),
        })
    }

    pub(super) fn edit_with_history_fallback<T>(
        &mut self,
        apply: impl Fn(&mut super::model::Document) -> Result<T, GtxGraphError<ForegroundError>>,
    ) -> Result<T, String> {
        let error = match apply(&mut guard(&self.document)) {
            Ok(result) => return Ok(result),
            Err(error) => error,
        };
        if !matches!(error, GtxGraphError::History(ref cause) if unsupported(cause))
            || self.history_router.is_none()
        {
            return Err(error.to_string());
        }

        self.dirty = self.is_dirty();
        self.deactivate_history();
        let mut document = guard(&self.document);
        document.unbind_property_history();
        let result = apply(&mut document).map_err(|error| error.to_string());
        self.status = Some("History cleared for this graph change.".into());
        result
    }

    pub(super) fn insert_below_target(
        &mut self,
        target: &Target,
        node: &Node,
    ) -> Result<(), String> {
        let apply = || match target {
            Target::Branch(index, _) => guard(&self.document).insert_below_branch(*index, node),
            Target::Slot(slot) => slot
                .insert_below(node)
                .map_err(|error| GtxGraphError::Node(error.into())),
        };
        let Err(error) = apply() else {
            return Ok(());
        };
        let unsupported_tree = matches!(
            &error,
            GtxGraphError::Node(mycela_engine::NodeWireError::Node(PointwiseError::History(cause))) if unsupported(cause)
        );
        if !unsupported_tree || self.history_router.is_none() {
            return Err(error.to_string());
        }
        self.dirty = self.is_dirty();
        self.deactivate_history();
        let mut document = guard(&self.document);
        document.unbind_property_history();
        let result = match target {
            Target::Branch(index, _) => document.insert_below_branch(*index, node),
            Target::Slot(slot) => slot
                .insert_below(node)
                .map_err(|error| GtxGraphError::Node(error.into())),
        };
        self.status = Some("History cleared for this graph change.".into());
        result.map_err(|error| error.to_string())
    }
}

fn unsupported(error: &HistoryError<ForegroundError>) -> bool {
    matches!(
        error,
        HistoryError::MissingDependency(HistoryEffect::PropertyTarget)
    )
}
