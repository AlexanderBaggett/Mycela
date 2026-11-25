//! Original zl.o public Preview and Floating render caches for Gradient.
use super::{
    GeneratorState, GradientGenerator, ImageGeneratorKind, PointwiseError, SharedImageMap, guard,
};
use crate::ordinary_events::{DelegateCore, HandlerCore};
use crate::{
    ImageError, ImageGeometry, PointwiseRenderOutcome, ProceduralExecution, RenderedImage,
};
use std::sync::Arc;

/// Public compatibility name for the shared Preview and Floating selectors.
pub use crate::pointwise::graph::PointwiseCachedRenderMode as GradientCachedRenderMode;

type RenderEvent<E> = HandlerCore<GradientGenerator<E>, RenderedImage, E>;
type RenderEvents<E> = DelegateCore<GradientGenerator<E>, RenderedImage, E>;

/// An observer of the selected public render mode, after its image is published.
pub struct GradientRenderHandler<E = ()>(Arc<RenderEvent<E>>);
impl<E> GradientRenderHandler<E> {
    pub fn new(
        callback: impl Fn(&GradientGenerator<E>, RenderedImage) -> Result<(), E> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(HandlerCore {
            callback: Box::new(callback),
        }))
    }
}

pub(in crate::pointwise) struct GradientRenderCaches<E> {
    images: [Option<RenderedImage>; 2],
    observers: [RenderEvents<E>; 2],
}
impl<E> GradientRenderCaches<E> {
    pub(super) fn new() -> Self {
        Self {
            images: [None, None],
            observers: std::array::from_fn(|_| DelegateCore::Absent),
        }
    }
}
impl<E> GeneratorState<E> {
    pub(in crate::pointwise) fn take_image_caches(&mut self) -> [Option<RenderedImage>; 4] {
        let [preview, floating] = std::mem::take(&mut self.render_caches.images);
        let final_image = self.final_cache.invalidate().map(RenderedImage::Still);
        [self.cache.take(), preview, floating, final_image]
    }
}

impl<E: 'static> GradientGenerator<E> {
    pub fn cached_render_image(&self, mode: GradientCachedRenderMode) -> Option<SharedImageMap> {
        self.cached_render_output(mode)?.still_image()
    }

    pub fn cached_render_output(&self, mode: GradientCachedRenderMode) -> Option<RenderedImage> {
        guard(&self.state()).render_caches.images[mode.index()].clone()
    }

    pub fn add_render_handler(
        &self,
        mode: GradientCachedRenderMode,
        handler: &GradientRenderHandler<E>,
    ) {
        let state = self.state();
        let mut state = guard(&state);
        let events = &mut state.render_caches.observers[mode.index()];
        *events = events.combine(&DelegateCore::Single(handler.0.clone()));
    }

    pub fn remove_render_handler(
        &self,
        mode: GradientCachedRenderMode,
        handler: &GradientRenderHandler<E>,
    ) {
        let old = {
            let state = self.state();
            let mut state = guard(&state);
            let events = &mut state.render_caches.observers[mode.index()];
            let next = events.remove(&DelegateCore::Single(handler.0.clone()));
            std::mem::replace(events, next)
        };
        drop(old);
    }

    pub(in crate::pointwise) fn copy_image_caches_from(&self, source: &Self) {
        let (rendered, preview) = {
            let state = source.state();
            let state = guard(&state);
            (
                state.cache.clone(),
                state.render_caches.images[GradientCachedRenderMode::Preview.index()].clone(),
            )
        };
        let old = {
            let state = self.state();
            let mut state = guard(&state);
            (
                std::mem::replace(&mut state.cache, rendered),
                std::mem::replace(
                    &mut state.render_caches.images[GradientCachedRenderMode::Preview.index()],
                    preview,
                ),
            )
        };
        drop(old);
    }

    /// Public source Preview or Floating render of an original Gradient.
    /// Cache hits reuse the same image root. Property commits invalidate both mode caches.
    pub fn render_cached(
        &self,
        mode: GradientCachedRenderMode,
        width: i32,
        height: i32,
        time: f64,
        execution: ProceduralExecution,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>> {
        self.render_cached_with_cancel(mode, width, height, time, execution, &|| false)
    }

    /// Render with a borrowed cancellation check. A cancelled render leaves
    /// the existing image caches alone and does not notify render observers.
    pub fn render_cached_with_cancel(
        &self,
        mode: GradientCachedRenderMode,
        width: i32,
        height: i32,
        time: f64,
        execution: ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>> {
        let outcome = self.prepare_cached_image(mode, width, height, time, execution, cancelled)?;
        if outcome.caught.is_empty()
            && let Some(image) = &outcome.image
        {
            self.publish_cached_image(mode, image, cancelled)?;
        }
        Ok(outcome)
    }

    // The graph may need to prepare this image's scene material before
    // publishing the mode cache and calling its observers.
    pub(in crate::pointwise) fn prepare_cached_image(
        &self,
        mode: GradientCachedRenderMode,
        width: i32,
        height: i32,
        time: f64,
        execution: ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>> {
        super::super::check_render_cancelled(cancelled)?;
        if self.kind() != ImageGeneratorKind::Gradient {
            return Err(PointwiseError::UnsupportedCapability(
                "non-Gradient public render",
            ));
        }
        let mut ancestor = self.node().parent();
        let mut seen = std::collections::BTreeSet::from([self.node().id]);
        while let Some(node) = ancestor {
            if !seen.insert(node.id) || node.is_clone_mode() {
                return Err(PointwiseError::UnsupportedCapability(
                    "Gradient public render ancestry",
                ));
            }
            ancestor = node.parent();
        }
        let existing = self.cached_render_output(mode);
        let rendered = (|| {
            let image = if let Some(image) = existing {
                image
            } else if mode == GradientCachedRenderMode::Preview && self.animated_preview_needed()? {
                let mut frames = Vec::with_capacity(RenderedImage::PREVIEW_FRAME_COUNT);
                let properties = self.parameters();
                for index in 0..RenderedImage::PREVIEW_FRAME_COUNT {
                    super::super::check_render_cancelled(cancelled)?;
                    let time = index as f64 / RenderedImage::PREVIEW_FRAME_COUNT as f64;
                    for property in &properties {
                        property.refresh_binding_for_preview(time)?;
                    }
                    frames.push(self.render_gradient(width, height, time, execution, cancelled)?);
                }
                let frames = frames
                    .try_into()
                    .map_err(|_| PointwiseError::InvalidOperation)?;
                RenderedImage::Animated(Arc::new(frames))
            } else {
                RenderedImage::Still(
                    self.render_gradient(width, height, time, execution, cancelled)?,
                )
            };
            for frame in image.frames() {
                super::super::check_render_cancelled(cancelled)?;
                frame.finish_graph_render()?;
            }
            Ok::<_, PointwiseError<E>>(image)
        })();
        let image = match rendered {
            Ok(image) => image,
            Err(PointwiseError::Cancelled) => return Err(PointwiseError::Cancelled),
            Err(error) if error.is_unsupported_capability() => return Err(error),
            Err(error @ PointwiseError::Image(ImageError::Allocation)) => return Err(error),
            Err(error) => {
                return Ok(PointwiseRenderOutcome {
                    image: Some(RenderedImage::Still(SharedImageMap::new(
                        ImageGeometry::new(width, height),
                    ))),
                    caught: vec![error],
                });
            }
        };

        Ok(PointwiseRenderOutcome {
            image: Some(image),
            caught: Vec::new(),
        })
    }

    pub(in crate::pointwise) fn publish_cached_image(
        &self,
        mode: GradientCachedRenderMode,
        image: &RenderedImage,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), PointwiseError<E>> {
        super::super::check_render_cancelled(cancelled)?;
        let old = {
            let state = self.state();
            let mut state = guard(&state);
            let previous = state.render_caches.images[mode.index()].replace(image.clone());
            let previous_rendered = if mode == GradientCachedRenderMode::Preview {
                state.cache.replace(image.clone())
            } else {
                None
            };
            (previous, previous_rendered)
        };
        drop(old);
        if mode == GradientCachedRenderMode::Preview {
            self.node().notify_rendered_image_changed()?;
        }
        let observers = guard(&self.state()).render_caches.observers[mode.index()].clone();
        observers
            .invoke(self, image.clone())
            .map_err(PointwiseError::Observer)
    }

    pub(in crate::pointwise) fn animated_preview_needed(&self) -> Result<bool, PointwiseError<E>> {
        if matches!(
            self.node().cached_rendered_output(),
            Some(RenderedImage::Animated(_))
        ) {
            return Ok(true);
        }
        for property in self.parameters() {
            if property.is_animated()? || property.has_animated_binding()? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
