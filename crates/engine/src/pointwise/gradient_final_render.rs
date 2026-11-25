//! Generator final clones (zl.g(gz.b) / s8) and Gradient's leaf path of zl.o.
use super::{
    GradientGenerator,
    GradientGeneratorParameterKind::{
        ForceSquareRatio, Image, Location, Mode, Size, UpsizeSmoothing,
    },
    ImageGeneratorKind,
};
use crate::pointwise::region::NodeId;
use crate::{
    ImageError, ImageGeometry, PointwiseError, ProceduralExecution, RenderedImage, SharedImageMap,
    pointwise::{check_render_cancelled, guard},
};

#[derive(Default)]
pub(in crate::pointwise) struct FinalImageCache {
    pub(in crate::pointwise) recipient: Option<NodeId>,
    pub(in crate::pointwise) enabled: bool,
    pub(in crate::pointwise) destruct_mode: bool,
    pub(in crate::pointwise) image: Option<SharedImageMap>,
    pub(in crate::pointwise) revision: u64,
}

impl FinalImageCache {
    fn clear_image(&mut self) -> Option<SharedImageMap> {
        let image = self.image.take();
        if image.is_some() {
            self.destruct_mode = false;
        }
        image
    }

    pub(in crate::pointwise) fn invalidate(&mut self) -> Option<SharedImageMap> {
        self.revision = self.revision.wrapping_add(1);
        self.recipient = None;
        self.clear_image()
    }

    pub(in crate::pointwise) fn read(
        &mut self,
        recipient: NodeId,
        geometry: ImageGeometry,
    ) -> Option<SharedImageMap> {
        if self.recipient != Some(recipient) {
            return None;
        }
        let image = self.image.clone()?;
        if image.is_disposed() {
            self.clear_image();
            return None;
        }
        let matches = image.geometry() == geometry;
        if self.destruct_mode || !matches {
            self.clear_image();
        }
        matches.then_some(image)
    }

    pub(in crate::pointwise) fn publish(
        &mut self,
        recipient: NodeId,
        image: &SharedImageMap,
    ) -> Option<SharedImageMap> {
        if self.recipient == Some(recipient) && self.enabled {
            return self.image.replace(image.clone());
        }
        None
    }
}

/// A real Gradient clone for final rendering. It retains its original node
/// while the original records the identity of the clone eligible for caching.
pub struct GradientFinalClone<E = ()> {
    generator: GradientGenerator<E>,
    original: GradientGenerator<E>,
}

impl<E> GradientFinalClone<E> {
    pub fn generator(&self) -> &GradientGenerator<E> {
        &self.generator
    }

    pub fn original(&self) -> &GradientGenerator<E> {
        &self.original
    }
}

impl<E> GradientGenerator<E> {
    /// Source ShouldFinalCache controls publication of newly rendered images.
    pub fn set_should_final_cache(&self, enabled: bool) {
        guard(&self.state()).final_cache.enabled = enabled;
    }

    /// Source DestructMode releases the retained final image after a cache hit.
    pub fn set_final_cache_destruct_mode(&self, destruct_mode: bool) {
        guard(&self.state()).final_cache.destruct_mode = destruct_mode;
    }

    pub fn cached_final_image(&self) -> Option<SharedImageMap> {
        guard(&self.state()).final_cache.image.clone()
    }
}

impl<E: 'static> GradientGenerator<E> {
    /// Source s8 starts with a fresh concrete node and calls CloneFrom on its
    /// properties. Its constructor owns the destination Limits and options.
    pub fn final_render_clone(&self) -> Result<GradientFinalClone<E>, PointwiseError<E>> {
        if self.kind() != ImageGeneratorKind::Gradient {
            return Err(PointwiseError::UnsupportedCapability(
                "Gradient final clone source",
            ));
        }
        let generator = self.copy_for_final_render(true)?;
        Ok(GradientFinalClone {
            generator,
            original: self.clone(),
        })
    }

    pub(in crate::pointwise) fn copy_for_final_render(
        &self,
        register: bool,
    ) -> Result<Self, PointwiseError<E>> {
        let kind = self.kind();
        if !matches!(
            kind,
            ImageGeneratorKind::Gradient
                | ImageGeneratorKind::ImportedImage
                | ImageGeneratorKind::Cells
                | ImageGeneratorKind::Noise
                | ImageGeneratorKind::WoodRings
                | ImageGeneratorKind::BricksBoards
        ) || guard(&self.state()).final_clone
            || !self.node().contents().is_empty()
        {
            return Err(PointwiseError::UnsupportedCapability(
                "image generator final clone source",
            ));
        }
        if matches!(
            kind,
            ImageGeneratorKind::Cells
                | ImageGeneratorKind::Noise
                | ImageGeneratorKind::WoodRings
                | ImageGeneratorKind::BricksBoards
        ) {
            let generator = self.copy_local()?;
            guard(&generator.state()).final_clone = true;
            if register {
                guard(&self.state()).final_cache.recipient = Some(generator.node.id);
            }
            return Ok(generator);
        }
        let generator = Self::with_kind(kind)?;
        guard(&generator.state()).final_clone = true;

        let (factor, pairs, choices, animations, wire) = {
            let original = self.state();
            let mut state = guard(&original);
            if register {
                state.final_cache.recipient = Some(generator.node.id);
            }
            (
                state.factor,
                state.pairs.clone(),
                if kind == ImageGeneratorKind::Gradient {
                    [state.selection.current, 0]
                } else {
                    [
                        state.choice(ForceSquareRatio).current,
                        state.choice(UpsizeSmoothing).current,
                    ]
                },
                state.animations.clone(),
                state.wire.clone(),
            )
        };
        guard(&generator.state()).factor = factor;
        for &property in kind.parameters() {
            let parameter = generator.parameter(property);
            let ix = property.ix();
            match property {
                Size | Location => {
                    let source = &pairs[ix];
                    parameter.set_axis(0, source.current[0])?;
                    parameter.set_axis(1, source.current[1])?;
                    let state = generator.state();
                    let mut state = guard(&state);
                    state.pairs[ix].normalized = source.normalized;
                    state.pairs[ix].scale = source.scale;
                }
                Image => {
                    // The final graph retains the original. Sharing the mn
                    // container need not attach the clone to the live tree.
                    generator.copy_imported_value_from(self)?;
                }
                Mode | ForceSquareRatio => parameter.set_index(choices[0])?,
                UpsizeSmoothing => parameter.set_index(choices[1])?,
                _ => unreachable!("properties of final-render generators"),
            }
            let metadata = wire[ix].source_clone()?;
            let animation = animations[ix]
                .as_ref()
                .map(crate::GradientAnimation::source_clone)
                .transpose()?;
            let state = generator.state();
            let mut state = guard(&state);
            state.wire[ix] = metadata;
            state.animations[ix] = animation;
        }
        Ok(generator)
    }
}

impl<E: 'static> GradientFinalClone<E> {
    /// Final render of a slotless Gradient with an explicit source cache
    /// request. A hit matches the original's recipient identity and geometry.
    pub fn render(
        &self,
        width: i32,
        height: i32,
        time: f64,
        execution: ProceduralExecution,
        use_final_cache: bool,
    ) -> Result<RenderedImage, PointwiseError<E>> {
        self.render_with_cancel(width, height, time, execution, use_final_cache, &|| false)
    }

    pub fn render_with_cancel(
        &self,
        width: i32,
        height: i32,
        time: f64,
        execution: ProceduralExecution,
        use_final_cache: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<RenderedImage, PointwiseError<E>> {
        check_render_cancelled(cancelled)?;
        if !self.generator.node().contents().is_empty() || self.generator.node().parent().is_some()
        {
            return Err(PointwiseError::UnsupportedCapability(
                "Gradient final clone graph scheduling",
            ));
        }
        if let Some(image) = self.generator.node().cached_rendered_output() {
            return Ok(image);
        }

        let recipient = self.generator.node.id;
        if use_final_cache {
            let original = self.original.state();
            if let Some(image) = guard(&original)
                .final_cache
                .read(recipient, ImageGeometry::new(width, height))
            {
                return Ok(RenderedImage::Still(image));
            }
        }

        let image = self
            .generator
            .render_gradient(width, height, time, execution, cancelled)
            .and_then(|image| {
                image.finish_graph_render()?;
                Ok(image)
            })
            .map_err(|error| match error {
                PointwiseError::Cancelled | PointwiseError::Image(ImageError::Allocation) => error,
                error if error.is_unsupported_capability() => error,
                error => PointwiseError::FinalRender(Box::new(error)),
            })?;
        check_render_cancelled(cancelled)?;
        if use_final_cache {
            let old = {
                let original = self.original.state();
                let mut original = guard(&original);
                original.final_cache.publish(recipient, &image)
            };
            drop(old);
        }
        Ok(RenderedImage::Still(image))
    }
}
