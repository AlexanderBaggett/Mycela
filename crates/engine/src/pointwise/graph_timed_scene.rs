//! Prepare scene inputs alongside timed image results (zl.o, zl.kv, aqr.g).
use super::{Output, PointwiseNode, RenderedImage, Request};
use crate::{PointwiseError, PointwiseRenderEffects, Scene};
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct PreparedScenes(Arc<[Option<Scene>]>);

impl PreparedScenes {
    pub(super) fn new(frames: Vec<Option<Scene>>) -> Self {
        Self(frames.into())
    }

    pub(super) fn sample<E>(&self, time: f64) -> Result<Option<Scene>, PointwiseError<E>> {
        let index = if self.0.len() == 1 {
            0
        } else {
            crate::framework_cast::integer((time * self.0.len() as f64).round_ties_even())
        };
        usize::try_from(index)
            .ok()
            .and_then(|index| self.0.get(index))
            .cloned()
            .ok_or(PointwiseError::ArgumentIndex)
    }
}

impl Output {
    pub(super) fn scene_at<E>(&self, time: f64) -> Result<Option<Scene>, PointwiseError<E>> {
        match &self.scenes {
            Some(scenes) => scenes.sample(time),
            None => Ok(None),
        }
    }
}

impl Request {
    pub(super) fn uv_texture_inputs<E>(
        &self,
        inputs: &[Output],
        time: f64,
    ) -> Result<(Option<Scene>, Option<crate::SharedImageMap>), PointwiseError<E>> {
        let (model, texture) = match inputs {
            [model] => (model, None),
            [model, texture] => (
                model,
                Some(
                    texture
                        .image
                        .frame_at_time(time)
                        .ok_or(PointwiseError::ArgumentIndex)?,
                ),
            ),
            _ => return Err(PointwiseError::Argument),
        };
        Ok((model.scene_at(time)?, texture))
    }

    pub(super) fn prepare_scenes<E>(
        &self,
        animated_preview: bool,
        mut build: impl FnMut(f64) -> Result<Option<Scene>, PointwiseError<E>>,
    ) -> Result<PreparedScenes, PointwiseError<E>> {
        let frames = if animated_preview {
            let count = RenderedImage::PREVIEW_FRAME_COUNT;
            (0..count)
                .map(|index| build(index as f64 / count as f64))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            vec![build(self.time)?]
        };
        Ok(PreparedScenes::new(frames))
    }

    pub(super) fn prepare_image_scenes<E>(
        &self,
        image: &RenderedImage,
        animated_preview: bool,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<PreparedScenes, PointwiseError<E>> {
        // The base kv wraps the image in a plane and uses the matching image
        // frame when the scheduler asks for a Preview scene sequence.
        self.prepare_scenes(animated_preview, |time| {
            crate::pointwise::check_render_cancelled(effects.cancel_check())?;
            let frame = image
                .frame_at_time(time)
                .ok_or(PointwiseError::ArgumentIndex)?;
            effects.image_as_scene(&frame).map(Some)
        })
    }

    pub(super) fn scene_frame<E: 'static>(
        &self,
        node: &PointwiseNode<E>,
        inputs: &[Output],
        time: f64,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<Option<Scene>, PointwiseError<E>> {
        crate::pointwise::check_render_cancelled(effects.cancel_check())?;
        if node.is_scene_generator() {
            effects.generate_scene(node, time)
        } else {
            effects.modify_prepared_scene(
                node,
                inputs.iter().map(|input| input.scene_at(time)),
                time,
            )
        }
    }
}

impl<E: 'static> PointwiseNode<E> {
    pub(super) fn has_scene_output(&self) -> Result<bool, PointwiseError<E>> {
        Ok(self.is_scene_node() || self.scene_forward_target()?.is_some())
    }

    pub(super) fn scene_properties_are_animated(&self) -> Result<bool, PointwiseError<E>> {
        let properties = match (self.scene_primitive(), self.scene_transform_3d()) {
            (Some(primitive), _) => primitive.properties(),
            (_, Some(transform)) => transform.properties(),
            _ => self
                .uv_texture_properties()
                .or_else(|| self.import_3d_properties())
                .unwrap_or_default(),
        };
        for property in properties {
            if property.is_animated()? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
