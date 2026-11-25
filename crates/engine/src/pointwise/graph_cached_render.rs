//! Original, non-clone graph Preview/Floating (zl.o and zl.q).
use super::{
    Payload, PointwiseKind, PointwiseNode, PointwiseRenderOptions, PointwiseRenderOutcome,
    RenderedImage, group_skips_color, guard, merge_group_color_and_channels, validate_group_render,
    validate_reference_render,
};
use crate::{
    ImageError, ImageGeometry, PointwiseError, PointwiseRenderEffects, ProceduralExecution,
    SharedImageMap,
};
use std::{collections::BTreeSet, sync::Arc};

#[path = "graph_timed_scene.rs"]
mod scene;
use scene::PreparedScenes;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointwiseCachedRenderMode {
    Preview,
    Floating,
}

impl PointwiseCachedRenderMode {
    pub(in crate::pointwise) fn index(self) -> usize {
        match self {
            Self::Preview => 0,
            Self::Floating => 1,
        }
    }
}

#[derive(Clone, Copy)]
struct Request {
    mode: PointwiseCachedRenderMode,
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
}

enum Operation {
    Ordinary,
    Scene,
    UvTexture,
    Group {
        skip_color: bool,
        contents_count: usize,
    },
}

enum Frame<E> {
    Enter(PointwiseNode<E>, f64, bool),
    GeometryOnly(PointwiseNode<E>, f64),
    GroupContents(PointwiseNode<E>, bool, usize, f64, bool),
    Leave(PointwiseNode<E>, Operation, usize, f64, bool),
    ReferenceSample(ReferenceRender<E>),
    ReferenceResult(ReferenceRender<E>, f64, bool),
    WarmSceneReference(PointwiseNode<E>, RenderedImage, bool),
    Missing,
}

struct ReferenceRender<E> {
    node: PointwiseNode<E>,
    time: f64,
    frames: Option<Vec<SharedImageMap>>,
    scenes: Option<Vec<Option<crate::Scene>>>,
}

struct Output {
    image: RenderedImage,
    animated_preview: bool,
    scenes: Option<PreparedScenes>,
    disconnected: bool,
}

impl Output {
    fn image_only(image: RenderedImage, animated_preview: bool) -> Self {
        Self {
            image,
            animated_preview,
            scenes: None,
            disconnected: false,
        }
    }
}

impl Request {
    fn blank(&self) -> RenderedImage {
        RenderedImage::Still(SharedImageMap::new(self.geometry))
    }

    fn group<E>(
        &self,
        inputs: &[Output],
        contents_count: usize,
        skip_color: bool,
    ) -> Result<RenderedImage, PointwiseError<E>> {
        let contents = &inputs[0].image;
        if !skip_color {
            // zl.u / hj.k8 forward the actual Contents image (or sequence).
            return Ok(contents.clone());
        }
        let first = inputs
            .get(contents_count)
            .ok_or(PointwiseError::Index)?
            .image
            .frame_at_time(self.time)
            .ok_or(PointwiseError::ArgumentIndex)?;
        if !self.options.is_channel_rendering() {
            return Ok(RenderedImage::Still(first));
        }
        // ImageMap.n2 reads the source's base named-channel field. It
        // is empty on the animation container, even if its frames have maps.
        let effect_map = contents.still_image();
        let out = merge_group_color_and_channels(self.geometry, &first, effect_map.as_ref())?;
        Ok(RenderedImage::Still(out))
    }

    fn frame<E: 'static>(
        &self,
        node: &PointwiseNode<E>,
        operation: &Operation,
        inputs: &[Output],
        time: f64,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        if matches!(operation, Operation::Scene) {
            let scene = self.scene_frame(node, inputs, time, effects)?;
            return effects.rasterize_scene(scene.as_ref(), self.geometry);
        }
        if matches!(operation, Operation::UvTexture) {
            let (model, texture) = self.uv_texture_inputs(inputs, time)?;
            return if node.uv_bakes_map(time)? {
                effects.bake_uv_texture(node, time, model.as_ref(), texture.as_ref(), self.geometry)
            } else {
                let scene = effects.texture_uv_scene(
                    node,
                    time,
                    model,
                    texture.as_ref(),
                    self.geometry,
                    self.options,
                )?;
                effects.rasterize_node_scene(node, time, scene.as_ref(), self.geometry)
            };
        }
        // zl.q uses Math.Round to even, and List indexing reports an invalid
        // sample rather than wrapping the time or borrowing an adjacent frame.
        let uses_fallbacks = matches!(
            node.kind(),
            Some(
                PointwiseKind::OklabColor
                    | PointwiseKind::Shader
                    | PointwiseKind::Style
                    | PointwiseKind::Synthesis
            )
        );
        let inputs = inputs
            .iter()
            .map(|input| {
                if uses_fallbacks && input.disconnected {
                    return Ok(None);
                }
                input
                    .image
                    .frame_at_time(time)
                    .map(Some)
                    .ok_or(PointwiseError::ArgumentIndex)
            })
            .collect::<Result<Vec<_>, _>>()?;
        node.validate_supplied_graph_frame()?;
        node.render_supplied_graph_frame(
            &inputs,
            self.geometry.width(),
            self.geometry.height(),
            Some((time, self.execution)),
            self.options,
            effects,
        )?
        .ok_or(PointwiseError::NullReference)
    }

    fn reference_frame<E>(
        &self,
        target: &RenderedImage,
        time: f64,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let image = target
            .frame_at_time(time)
            .ok_or(PointwiseError::ArgumentIndex)?;
        if image.geometry().width() != self.geometry.width() {
            super::super::image_resize::resize(
                &image,
                self.geometry.width(),
                self.geometry.height(),
                self.execution,
            )
        } else {
            Ok(image)
        }
    }

    fn seal_image<E>(
        &self,
        image: &RenderedImage,
        effects: &PointwiseRenderEffects<'_, E>,
    ) -> Result<(), PointwiseError<E>> {
        for frame in image.frames() {
            super::super::check_render_cancelled(effects.cancel_check())?;
            frame.finish_graph_render()?;
        }
        Ok(())
    }

    fn image_output<E>(
        &self,
        image: RenderedImage,
        animated_preview: bool,
        need_scene: bool,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<Output, PointwiseError<E>> {
        self.seal_image(&image, effects)?;
        let scenes = need_scene
            .then(|| self.prepare_image_scenes(&image, animated_preview, effects))
            .transpose()?;
        Ok(Output {
            image,
            animated_preview,
            scenes,
            disconnected: false,
        })
    }

    fn finish<E: 'static>(
        &self,
        node: &PointwiseNode<E>,
        result: Result<Output, PointwiseError<E>>,
        animated_preview: bool,
        effects: &PointwiseRenderEffects<'_, E>,
        caught: &mut Vec<PointwiseError<E>>,
    ) -> Result<Output, PointwiseError<E>> {
        match result {
            Ok(output) => {
                super::super::check_render_cancelled(effects.cancel_check())?;
                if let Some(gradient) = node.gradient_generator_handle() {
                    gradient.publish_cached_image(
                        self.mode,
                        &output.image,
                        effects.cancel_check(),
                    )?;
                } else {
                    node.publish_timed_output(self.mode, self.options, &output.image)?;
                }
                Ok(output)
            }
            Err(
                error @ (PointwiseError::Cancelled
                | PointwiseError::Image(ImageError::Allocation)
                | PointwiseError::Scene(crate::SceneError::Allocation)),
            ) => Err(error),
            Err(error) if error.is_unsupported_capability() => Err(error),
            Err(error) => {
                caught.push(error);
                Ok(Output::image_only(self.blank(), animated_preview))
            }
        }
    }
}

impl<E: 'static> PointwiseNode<E> {
    /// The live node's Preview or Floating cache. Preview publication also
    /// sets Rendered Image. Floating has an independent cache.
    pub fn cached_graph_output(&self, mode: PointwiseCachedRenderMode) -> Option<RenderedImage> {
        self.timed_output(mode, None)
    }

    fn timed_output(
        &self,
        mode: PointwiseCachedRenderMode,
        options: Option<PointwiseRenderOptions>,
    ) -> Option<RenderedImage> {
        if let Some(gradient) = self.gradient_generator_handle() {
            return gradient.cached_render_output(mode);
        }
        guard(&self.core().mode_events).timed_images[mode.index()]
            .as_ref()
            .filter(|(saved, _)| options.is_none_or(|requested| requested == *saved))
            .map(|(_, image)| image.clone())
    }

    /// Render an image graph in the original Preview or Floating context.
    pub fn render_cached_graph(
        &self,
        mode: PointwiseCachedRenderMode,
        width: i32,
        height: i32,
        time: f64,
        execution: ProceduralExecution,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>> {
        let mut capacity = |_| {
            Err(PointwiseError::UnsupportedCapability(
                "Combine ambient capacity predicate",
            ))
        };
        let mut preflight = |_, _| {
            Err(PointwiseError::UnsupportedCapability(
                "image graph memory preflight context",
            ))
        };
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        self.render_cached_graph_with_effects(
            mode,
            ImageGeometry::new(width, height),
            time,
            execution,
            PointwiseRenderOptions::new(false, false),
            &mut effects,
        )
    }

    /// Render timed graphs with caller-owned allocation, scene rendering, and
    /// cancellation services. Geometry and material sequences are evaluated
    /// within this request and the image result is retained on each node.
    pub fn render_cached_graph_with_effects(
        &self,
        mode: PointwiseCachedRenderMode,
        geometry: ImageGeometry,
        time: f64,
        execution: ProceduralExecution,
        options: PointwiseRenderOptions,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>> {
        let request = Request {
            mode,
            geometry,
            time,
            execution,
            options,
        };
        let mut pending = vec![Frame::Enter(self.clone(), time, false)];
        let mut active = BTreeSet::new();
        let mut outputs = Vec::new();
        let mut caught = Vec::new();
        while let Some(frame) = pending.pop() {
            super::super::check_render_cancelled(effects.cancel_check())?;
            match frame {
                Frame::Enter(node, time, need_scene) => {
                    if !active.insert(node.id) {
                        return Err(PointwiseError::UnsupportedCapability("cyclic timed graph"));
                    }
                    node.validate_timed_node()?;
                    if let Some(gradient) = node.gradient_generator_handle() {
                        let result = gradient.prepare_cached_image(
                            mode,
                            geometry.width(),
                            geometry.height(),
                            time,
                            execution,
                            effects.cancel_check(),
                        )?;
                        let image = result.image.ok_or(PointwiseError::NullReference)?;
                        let animated_preview = request.mode == PointwiseCachedRenderMode::Preview
                            && (matches!(&image, RenderedImage::Animated(_))
                                || gradient.animated_preview_needed()?);
                        if result.caught.is_empty() {
                            let request = Request { time, ..request };
                            let scenes = need_scene
                                .then(|| {
                                    request.prepare_image_scenes(&image, animated_preview, effects)
                                })
                                .transpose();
                            outputs.push(request.finish(
                                &node,
                                scenes.map(|scenes| Output {
                                    image,
                                    animated_preview,
                                    scenes,
                                    disconnected: false,
                                }),
                                animated_preview,
                                effects,
                                &mut caught,
                            )?);
                        } else {
                            caught.extend(result.caught);
                            outputs.push(Output::image_only(image, animated_preview));
                        }
                        active.remove(&node.id);
                        continue;
                    }
                    if node.kind() == Some(PointwiseKind::Reference) {
                        validate_reference_render(&node)?;
                        let target = node.reference_target()?;
                        let warm = target
                            .as_ref()
                            .and_then(|target| target.timed_output(mode, Some(options)));
                        let animated_preview = mode == PointwiseCachedRenderMode::Preview
                            && (matches!(&warm, Some(RenderedImage::Animated(_)))
                                || node.has_animated_preview()?);
                        if let Some(image) =
                            warm.or_else(|| target.is_none().then(|| request.blank()))
                        {
                            if need_scene
                                && let Some(target) = target
                                && target.has_scene_output()?
                            {
                                pending.push(Frame::WarmSceneReference(
                                    node,
                                    image,
                                    animated_preview,
                                ));
                                pending.push(Frame::Enter(target, time, true));
                                continue;
                            }
                            // zl.o reads the target mode cache before invoking ark.k8.
                            // This path forwards the whole retained image unchanged.
                            let request = Request { time, ..request };
                            let result =
                                request.image_output(image, animated_preview, need_scene, effects);
                            outputs.push(request.finish(
                                &node,
                                result,
                                animated_preview,
                                effects,
                                &mut caught,
                            )?);
                            active.remove(&node.id);
                        } else {
                            pending.push(Frame::ReferenceSample(ReferenceRender {
                                node,
                                time,
                                frames: animated_preview.then(|| {
                                    Vec::with_capacity(RenderedImage::PREVIEW_FRAME_COUNT)
                                }),
                                scenes: need_scene.then(Vec::new),
                            }));
                        }
                        continue;
                    }
                    if node.kind().is_some_and(PointwiseKind::is_group) {
                        validate_group_render(&node)?;
                        let skip_color = group_skips_color(&node, options)?;
                        let slots = node.slots();
                        pending.push(Frame::GroupContents(
                            node,
                            skip_color,
                            slots.len(),
                            time,
                            need_scene,
                        ));
                        // zl.o schedules all exterior inputs before Contents.
                        for slot in slots.into_iter().rev() {
                            pending
                                .push(slot.node().map_or(Frame::Missing, |node| {
                                    Frame::Enter(node, time, false)
                                }));
                        }
                        continue;
                    }
                    if !node.contents().is_empty() && node.kind() != Some(PointwiseKind::Canvas) {
                        return Err(PointwiseError::UnsupportedCapability(
                            "timed image Contents context",
                        ));
                    }
                    let slots = node.slots();
                    let is_scene = node.is_scene_node();
                    let scene_slots = super::super::render::is_scene_modifier(&node);
                    let uv_texture = node.uv_texture_output().is_some();
                    let uv_geometry_only = node.uv_uses_geometry_only();
                    let operation = if uv_texture {
                        Operation::UvTexture
                    } else if is_scene {
                        Operation::Scene
                    } else {
                        Operation::Ordinary
                    };
                    pending.push(Frame::Leave(
                        node.clone(),
                        operation,
                        slots.len(),
                        time,
                        need_scene,
                    ));
                    // zl.o pushes slots from last to first.
                    for (index, slot) in slots.into_iter().enumerate().rev() {
                        let child = node.uses_image_slot(index).then(|| slot.node()).flatten();
                        pending.push(child.map_or(Frame::Missing, |node| {
                            if uv_geometry_only {
                                Frame::GeometryOnly(node, time)
                            } else {
                                Frame::Enter(node, time, scene_slots || (uv_texture && index == 0))
                            }
                        }));
                    }
                }
                Frame::GeometryOnly(node, time) => {
                    let request = Request { time, ..request };
                    let animated_preview = mode == PointwiseCachedRenderMode::Preview
                        && node.has_animated_preview()?;
                    let scenes = request.prepare_scenes(animated_preview, |time| {
                        effects.geometry_scene(node.clone(), time, PointwiseNode::reference_target)
                    })?;
                    outputs.push(Output {
                        image: request.blank(),
                        animated_preview,
                        scenes: Some(scenes),
                        disconnected: false,
                    });
                }
                Frame::GroupContents(node, skip_color, slot_count, time, need_scene) => {
                    let contents = node.contents();
                    let contents_count = contents.len();
                    if contents_count == 0 {
                        return Err(PointwiseError::Index);
                    }
                    pending.push(Frame::Leave(
                        node,
                        Operation::Group {
                            skip_color,
                            contents_count,
                        },
                        slot_count + contents_count,
                        time,
                        need_scene,
                    ));
                    // Unlike the external slots, Contents are pushed in order.
                    for content in contents {
                        pending.push(
                            content.node().map_or(Frame::Missing, |node| {
                                Frame::Enter(node, time, need_scene)
                            }),
                        );
                    }
                }
                Frame::Leave(node, operation, count, time, need_scene) => {
                    let mut inputs = outputs.split_off(outputs.len() - count);
                    if let Operation::Group { contents_count, .. } = &operation {
                        inputs.rotate_right(*contents_count);
                        inputs[..*contents_count].reverse();
                    }
                    let result = node.render_timed_image(
                        &Request { time, ..request },
                        operation,
                        &inputs,
                        need_scene,
                        effects,
                    );
                    outputs.push(request.finish(
                        &node,
                        result,
                        inputs.iter().any(|input| input.animated_preview),
                        effects,
                        &mut caught,
                    )?);
                    active.remove(&node.id);
                }
                Frame::ReferenceSample(reference) => {
                    let animated_preview = reference.frames.is_some();
                    let time = reference.frames.as_ref().map_or(reference.time, |frames| {
                        frames.len() as f64 / RenderedImage::PREVIEW_FRAME_COUNT as f64
                    });
                    let target = (|| {
                        if animated_preview {
                            reference
                                .node
                                .filter()
                                .ok_or(PointwiseError::NullReference)?
                                .parameter(0)?
                                .refresh_binding_for_preview(time)?;
                        }
                        reference.node.reference_target()
                    })();
                    match target {
                        Ok(target) => {
                            let target_scene = match &target {
                                Some(target) if reference.scenes.is_some() => {
                                    target.has_scene_output()?
                                }
                                _ => false,
                            };
                            pending.push(Frame::ReferenceResult(reference, time, target_scene));
                            pending.push(target.map_or(Frame::Missing, |node| {
                                Frame::Enter(node, time, target_scene)
                            }));
                        }
                        Err(error) => {
                            outputs.push(request.finish(
                                &reference.node,
                                Err(error),
                                animated_preview,
                                effects,
                                &mut caught,
                            )?);
                            active.remove(&reference.node.id);
                        }
                    }
                }
                Frame::ReferenceResult(mut reference, time, target_scene) => {
                    let target = outputs.pop().ok_or(PointwiseError::InvalidOperation)?;
                    let animated_preview = reference.frames.is_some();
                    let sample = (|| {
                        let image = request.reference_frame::<E>(&target.image, time)?;
                        image.finish_graph_render()?;
                        if let Some(scenes) = &mut reference.scenes {
                            let scene = if target_scene {
                                target.scene_at(time)?
                            } else {
                                Some(effects.image_as_scene(&image)?)
                            };
                            scenes.push(scene);
                        }
                        Ok::<_, PointwiseError<E>>(image)
                    })();
                    let image = match sample {
                        Ok(image) => {
                            if let Some(frames) = &mut reference.frames {
                                frames.push(image);
                                if frames.len() < RenderedImage::PREVIEW_FRAME_COUNT {
                                    pending.push(Frame::ReferenceSample(reference));
                                    continue;
                                }
                                let frames = std::mem::take(frames)
                                    .try_into()
                                    .map_err(|_| PointwiseError::InvalidOperation)?;
                                Ok(RenderedImage::Animated(Arc::new(frames)))
                            } else {
                                Ok(RenderedImage::Still(image))
                            }
                        }
                        Err(error) => Err(error),
                    };
                    outputs.push(request.finish(
                        &reference.node,
                        image.map(|image| Output {
                            image,
                            animated_preview,
                            scenes: reference.scenes.map(PreparedScenes::new),
                            disconnected: false,
                        }),
                        animated_preview,
                        effects,
                        &mut caught,
                    )?);
                    active.remove(&reference.node.id);
                }
                Frame::WarmSceneReference(node, image, animated_preview) => {
                    let target = outputs.pop().ok_or(PointwiseError::InvalidOperation)?;
                    outputs.push(request.finish(
                        &node,
                        Ok(Output {
                            image,
                            animated_preview,
                            scenes: target.scenes,
                            disconnected: false,
                        }),
                        animated_preview,
                        effects,
                        &mut caught,
                    )?);
                    active.remove(&node.id);
                }
                Frame::Missing => outputs.push(Output {
                    disconnected: true,
                    ..Output::image_only(request.blank(), false)
                }),
            }
        }
        Ok(PointwiseRenderOutcome {
            image: Some(outputs.pop().ok_or(PointwiseError::InvalidOperation)?.image),
            caught,
        })
    }

    fn validate_timed_node(&self) -> Result<(), PointwiseError<E>> {
        if self.is_clone_mode() {
            return Err(PointwiseError::UnsupportedCapability(
                "clone-mode timed graph",
            ));
        }
        Ok(())
    }

    fn render_timed_image(
        &self,
        request: &Request,
        operation: Operation,
        inputs: &[Output],
        need_scene: bool,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<Output, PointwiseError<E>> {
        let cached = self.timed_output(request.mode, Some(request.options));
        let uses_fallbacks = self.kind() == Some(PointwiseKind::OklabColor);
        let animated_preview = request.mode == PointwiseCachedRenderMode::Preview
            && (inputs.iter().any(|input| input.animated_preview)
                || matches!(&cached, Some(RenderedImage::Animated(_)))
                || self.local_animated_preview_needed()?);
        let image = if let Some(cached) = cached {
            cached
        } else if let Operation::Group {
            skip_color,
            contents_count,
        } = operation
        {
            request.group(inputs, contents_count, skip_color)?
        } else if animated_preview {
            let mut frames = Vec::with_capacity(RenderedImage::PREVIEW_FRAME_COUNT);
            for index in 0..RenderedImage::PREVIEW_FRAME_COUNT {
                super::super::check_render_cancelled(effects.cancel_check())?;
                let time = index as f64 / RenderedImage::PREVIEW_FRAME_COUNT as f64;
                if let Some(filter) = self.filter() {
                    for (index, property) in filter.parameters().into_iter().enumerate() {
                        if uses_fallbacks
                            && inputs.get(index).is_some_and(|input| !input.disconnected)
                        {
                            continue;
                        }
                        property.refresh_binding_for_preview(time)?;
                    }
                }
                if matches!(&self.core().payload, Payload::Solid { .. }) {
                    self.refresh_solid_bindings_for_preview(time)?;
                }
                frames.push(request.frame(self, &operation, inputs, time, effects)?);
            }
            let frames = frames
                .try_into()
                .map_err(|_| PointwiseError::InvalidOperation)?;
            RenderedImage::Animated(Arc::new(frames))
        } else {
            RenderedImage::Still(request.frame(self, &operation, inputs, request.time, effects)?)
        };
        request.seal_image(&image, effects)?;
        let scenes = if need_scene {
            Some(match operation {
                Operation::Scene => request.prepare_scenes(animated_preview, |time| {
                    request.scene_frame(self, inputs, time, effects)
                })?,
                Operation::UvTexture => request.prepare_scenes(animated_preview, |time| {
                    if self.uv_bakes_map(time)? {
                        let map = image
                            .frame_at_time(time)
                            .ok_or(PointwiseError::ArgumentIndex)?;
                        effects.image_as_scene(&map).map(Some)
                    } else {
                        let (model, texture) = request.uv_texture_inputs(inputs, time)?;
                        effects.texture_uv_scene(
                            self,
                            time,
                            model,
                            texture.as_ref(),
                            request.geometry,
                            request.options,
                        )
                    }
                })?,
                Operation::Group { .. } => match &inputs[0].scenes {
                    // zl.o forwards the first Contents entry's scenes, even if
                    // the Group's selected image came from an outside slot.
                    Some(scenes) => scenes.clone(),
                    None => request.prepare_image_scenes(&image, false, effects)?,
                },
                Operation::Ordinary => {
                    request.prepare_image_scenes(&image, animated_preview, effects)?
                }
            })
        } else {
            None
        };
        Ok(Output {
            image,
            animated_preview,
            scenes,
            disconnected: false,
        })
    }

    /// Whether this graph references animated controls or a retained sequence.
    pub fn has_animated_preview(&self) -> Result<bool, PointwiseError<E>> {
        let mut pending = vec![self.clone()];
        let mut visited = BTreeSet::new();
        while let Some(node) = pending.pop() {
            if !visited.insert(node.id) {
                continue;
            }
            if node.local_animated_preview_needed()? {
                return Ok(true);
            }
            if node.kind() == Some(PointwiseKind::Reference) {
                pending.extend(node.reference_target()?);
            }
            pending.extend(
                node.slots()
                    .into_iter()
                    .chain(node.contents())
                    .filter_map(|slot| slot.node()),
            );
        }
        Ok(false)
    }

    fn publish_timed_output(
        &self,
        mode: PointwiseCachedRenderMode,
        options: PointwiseRenderOptions,
        image: &RenderedImage,
    ) -> Result<(), PointwiseError<E>> {
        let old = guard(&self.core().mode_events).timed_images[mode.index()]
            .replace((options, image.clone()));
        drop(old);
        if mode == PointwiseCachedRenderMode::Preview {
            // After publication the source callback may inspect or replace it.
            // A thrown observer propagates rather than producing an empty map.
            self.set_rendered_output(Some(image.clone()))?;
        }
        Ok(())
    }

    fn local_animated_preview_needed(&self) -> Result<bool, PointwiseError<E>> {
        if self
            .shader_program()
            .is_some_and(|shader| shader.is_time_dependent())
        {
            return Ok(true);
        }
        if matches!(
            self.cached_rendered_output(),
            Some(RenderedImage::Animated(_))
        ) {
            return Ok(true);
        }
        if let Some(generator) = self.image_generator_handle() {
            return generator.animated_preview_needed();
        }
        if let Payload::Solid { model, .. } = &self.core().payload {
            return Ok(self.solid_property_is_animated(&model.color_property())?
                || self.solid_property_is_animated(&model.transparency_property())?);
        }
        if matches!(&self.core().payload, Payload::Invert { .. }) {
            for property in self
                .invert_properties()
                .map_err(super::super::invert_node::graph_error)?
            {
                if property.is_animated()? {
                    return Ok(true);
                }
            }
        }
        if let Some(filter) = self.filter() {
            for (index, property) in filter.parameters().into_iter().enumerate() {
                if filter.kind() == PointwiseKind::OklabColor
                    && self.slot(index as i32)?.node().is_some()
                {
                    continue;
                }
                if super::super::parameter_ops::is_animated(|| property.retained_animation())?
                    || property.has_animated_binding()?
                {
                    return Ok(true);
                }
            }
        }
        self.scene_properties_are_animated()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AnalyticCurveKind, CurveLayerHandle, GradientGenerator,
        GradientGeneratorParameterKind::Location, PointwiseFilter,
    };
    use PointwiseCachedRenderMode::{Floating, Preview};
    use ProceduralExecution::CallingThread;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    fn animated_gradient() -> GradientGenerator<&'static str> {
        let gradient = GradientGenerator::new().unwrap();
        let animation = gradient.parameter(Location).animation().unwrap();
        animation.ensure_curve(0).unwrap();
        let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
        curve.set_minimum(0.);
        curve.set_maximum(1.);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        animation.flags().unwrap().set(0, true).unwrap();
        gradient
    }

    #[test]
    fn gradient_material_reaches_nested_scene_and_ordinary_image_consumer() {
        struct Backend {
            mixed: Vec<(f32, f64)>,
            calls: usize,
        }
        impl crate::SceneImageRenderer<&'static str> for Backend {
            fn preflight_array(
                &mut self,
                _: crate::SceneAllocationRequest,
            ) -> Result<(), crate::SceneError> {
                Ok(())
            }

            fn material_texture_limit(&mut self) -> Result<u32, PointwiseError<&'static str>> {
                Ok(4096)
            }

            fn render_scene_image(
                &mut self,
                scene: &crate::Scene,
                width: i32,
                height: i32,
            ) -> Result<SharedImageMap, PointwiseError<&'static str>> {
                self.calls += 1;
                let models = scene.models();
                let mut value = 0.;
                if let Some(map) = models[0].material().map() {
                    assert!(map.is_permanently_sealed());
                    value = map.channel(0, false)?.read(0)?;
                }
                if models.len() == 2 {
                    let x = models[0]
                        .try_positions()
                        .unwrap()
                        .iter()
                        .map(|position| f64::from(position.values()[0]))
                        .fold(f64::INFINITY, f64::min);
                    self.mixed.push((value, x));
                }
                let image = SharedImageMap::new(ImageGeometry::new(width, height));
                image.set_constant(0, value)?;
                Ok(image)
            }
        }

        let gradient = animated_gradient();
        let wizard = PointwiseFilter::new(PointwiseKind::WizardGroup).unwrap();
        wizard.parameter(0).unwrap().set_number(1.).unwrap();
        wizard
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let reference = PointwiseFilter::new(PointwiseKind::Reference)
            .unwrap()
            .node();
        wizard
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&reference))
            .unwrap();

        let transform = crate::SceneTransformNode::new().unwrap();
        let move_property = transform.property(1).unwrap();
        let animation = move_property.animation().unwrap().unwrap();
        animation.ensure_curve(0).unwrap();
        let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
        curve.set_minimum(0.);
        curve.set_maximum(2.);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        animation.flags().unwrap().set(0, true).unwrap();
        transform
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&wizard.node()))
            .unwrap();
        transform.node().convert_to_filter(false).unwrap();

        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap().node();
        let scene_reference = PointwiseFilter::new(PointwiseKind::Reference)
            .unwrap()
            .node();
        group
            .slot(0)
            .unwrap()
            .set_node(Some(&transform.node()))
            .unwrap();
        group
            .content(0)
            .unwrap()
            .set_node(Some(&scene_reference))
            .unwrap();
        let cube = crate::ScenePrimitiveNode::new(crate::ScenePrimitiveKind::Cube).unwrap();
        let combine = crate::SceneCombineNode::new().node();
        combine.slot(0).unwrap().set_node(Some(&group)).unwrap();
        combine
            .slot(1)
            .unwrap()
            .set_node(Some(&cube.node()))
            .unwrap();
        combine.convert_to_filter(false).unwrap();
        let root = PointwiseNode::invert();
        root.slot(0).unwrap().set_node(Some(&combine)).unwrap();

        let geometry = ImageGeometry::new(4, 2);
        let mut backend = Backend {
            mixed: Vec::new(),
            calls: 0,
        };
        let run = |mode, time, backend: &mut Backend| {
            let mut capacity = |_| unreachable!();
            let mut preflight = |_, _| unreachable!();
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
                .with_scene_renderer(backend);
            let rendered = root
                .render_cached_graph_with_effects(
                    mode,
                    geometry,
                    time,
                    CallingThread,
                    PointwiseRenderOptions::new(false, false),
                    &mut effects,
                )
                .unwrap();
            assert!(rendered.caught_errors().is_empty());
            rendered.output().unwrap()
        };
        let preview = run(Preview, 0.75, &mut backend);
        assert_eq!(backend.mixed.len(), 32);
        for (index, (frame, &(material, x))) in
            preview.frames().iter().zip(&backend.mixed).enumerate()
        {
            let t = index as f32 / 32.;
            assert_eq!(material, t);
            assert!((x - (-0.5 + 2. * f64::from(t))).abs() < 1e-6);
            assert_eq!(frame.materialize(0).unwrap().read(0).unwrap(), 1. - t);
        }
        assert!(
            reference
                .cached_graph_output(Preview)
                .unwrap()
                .same_reference(&gradient.cached_render_output(Preview).unwrap())
        );
        let calls = backend.calls;
        assert!(run(Preview, 0., &mut backend).same_reference(&preview));
        assert_eq!(backend.calls, calls);

        let floating = run(Floating, 0.25, &mut backend);
        assert_eq!(backend.mixed.last(), Some(&(0.25, 0.)));
        assert_eq!(
            floating
                .still_image()
                .unwrap()
                .materialize(0)
                .unwrap()
                .read(0)
                .unwrap(),
            0.75
        );
        assert!(preview.same_reference(&root.cached_rendered_output().unwrap()));
        let calls = backend.calls;
        // The final no-op filter must forward geometry as well as the image.
        let passthrough = PointwiseFilter::new(PointwiseKind::ChangeBci)
            .unwrap()
            .node();
        combine.slot(0).unwrap().set_node(None).unwrap();
        passthrough.slot(0).unwrap().set_node(Some(&group)).unwrap();
        combine
            .slot(0)
            .unwrap()
            .set_node(Some(&passthrough))
            .unwrap();
        combine.convert_to_filter(false).unwrap();
        root.set_should_final_cache(true);
        transform.node().set_should_final_cache(true);
        let final_graph = root.final_graph_clone().unwrap();
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut final_render = |backend: &mut Backend| {
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
                .with_scene_renderer(backend);
            final_graph
                .render_with_effects(
                    geometry,
                    0.25,
                    CallingThread,
                    PointwiseRenderOptions::new(false, false),
                    true,
                    &mut effects,
                )
                .unwrap()
                .image()
                .unwrap()
        };
        let final_image = final_render(&mut backend);
        assert_eq!(backend.calls, calls + 1);
        assert!(transform.node().cached_final_image().is_none());
        assert_eq!(backend.mixed.last(), Some(&(0.25, 0.)));
        assert_eq!(
            final_image.channel(0, false).unwrap().read(0).unwrap(),
            0.75
        );
        assert!(final_render(&mut backend).same_reference(&final_image));
        assert_eq!(backend.calls, calls + 1);
        gradient.parameter(Location).set_axis(1, 0.5).unwrap();
        assert!(root.cached_graph_output(Preview).is_none());
        assert!(root.cached_graph_output(Floating).is_none());
        assert!(root.cached_final_image().is_none());
    }

    #[test]
    fn scene_filter_mode_uses_gradient_and_retained_default_model() {
        struct Materials(Vec<f32>);
        impl crate::SceneImageRenderer<&'static str> for Materials {
            fn preflight_array(
                &mut self,
                _: crate::SceneAllocationRequest,
            ) -> Result<(), crate::SceneError> {
                Ok(())
            }

            fn material_texture_limit(&mut self) -> Result<u32, PointwiseError<&'static str>> {
                Ok(4096)
            }

            fn render_scene_image(
                &mut self,
                scene: &crate::Scene,
                _: i32,
                _: i32,
            ) -> Result<SharedImageMap, PointwiseError<&'static str>> {
                let models = scene.models();
                assert_eq!(models.len(), 2);
                let default = models[1].material().map().unwrap();
                assert!(default.is_permanently_sealed());
                assert_eq!(default.channel(0, false)?.values(), self.0);
                assert!(
                    default
                        .channel(3, false)?
                        .values()
                        .iter()
                        .all(|value| *value == 1.)
                );
                Ok(models[0].material().map().unwrap())
            }
        }

        let gradient = animated_gradient();
        let combine = crate::SceneCombineNode::new().node();
        combine
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        combine.convert_to_filter(false).unwrap();
        let default = combine.slot(1).unwrap().node().unwrap();
        assert!(combine.is_filter());
        assert!(default.is_default_input());
        let geometry = ImageGeometry::new(16, 8);
        let options = PointwiseRenderOptions::new(false, true);
        let time = 0.375;
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let expected = default
            .render_original_with_effects(16, 8, options, time, CallingThread, &mut effects)
            .unwrap();
        assert!(expected.caught_errors().is_empty());
        let expected = expected
            .image()
            .unwrap()
            .channel(0, false)
            .unwrap()
            .values();
        assert!(expected.iter().all(|value| *value == 0.));
        let mut backend = Materials(expected);
        let mut effects = effects.with_scene_renderer(&mut backend);
        let red = |image: &SharedImageMap| image.channel(0, false).unwrap().read(0).unwrap();

        let original = combine
            .render_original_with_effects(16, 8, options, time, CallingThread, &mut effects)
            .unwrap();
        assert!(original.caught_errors().is_empty());
        assert_eq!(red(&original.image().unwrap()), time as f32);
        for mode in [Preview, Floating] {
            let timed = combine
                .render_cached_graph_with_effects(
                    mode,
                    geometry,
                    time,
                    CallingThread,
                    options,
                    &mut effects,
                )
                .unwrap();
            assert!(timed.caught_errors().is_empty());
            let output = timed.output().unwrap();
            assert_eq!(red(&output.frame_at_time(time).unwrap()), time as f32);
            if mode == Preview {
                assert_eq!(output.frames().len(), 32);
                for (index, frame) in output.frames().iter().enumerate() {
                    assert_eq!(red(frame), index as f32 / 32.);
                }
            }
        }

        default.set_should_final_cache(true);
        let final_graph = combine.final_graph_clone().unwrap();
        let final_result = final_graph
            .render_with_effects(geometry, time, CallingThread, options, true, &mut effects)
            .unwrap();
        assert_eq!(red(&final_result.image().unwrap()), time as f32);
        assert!(
            default
                .cached_final_image()
                .unwrap()
                .is_permanently_sealed()
        );

        let snapshot = crate::RenderCaptureSession::new()
            .capture_node(&combine, time)
            .unwrap();
        let empty = crate::PointwiseImportedImage::new().custom_object();
        default
            .imported_image_handle()
            .unwrap()
            .parameter(crate::ImageGeneratorParameterKind::Image)
            .set_custom_value(Some(&empty), false)
            .unwrap();
        assert!(default.cached_final_image().is_none());
        let exported = snapshot
            .render_export_with_effects(geometry, CallingThread, options, &mut effects)
            .unwrap();
        assert!(exported.caught_errors().is_empty());
        assert_eq!(red(&exported.image().unwrap()), time as f32);
    }

    #[test]
    fn color_only_group_keeps_its_exterior_image_and_its_contents_scene() {
        struct Material;
        impl crate::SceneImageRenderer<&'static str> for Material {
            fn preflight_array(
                &mut self,
                _: crate::SceneAllocationRequest,
            ) -> Result<(), crate::SceneError> {
                Ok(())
            }

            fn material_texture_limit(&mut self) -> Result<u32, PointwiseError<&'static str>> {
                Ok(4096)
            }

            fn render_scene_image(
                &mut self,
                scene: &crate::Scene,
                width: i32,
                height: i32,
            ) -> Result<SharedImageMap, PointwiseError<&'static str>> {
                let map = scene.models()[0].material().map().unwrap();
                assert!(map.is_permanently_sealed());
                let raster = SharedImageMap::new(ImageGeometry::new(width, height));
                raster.set_constant(0, map.channel(0, false)?.read(0)?)?;
                Ok(raster)
            }
        }
        let exterior = GradientGenerator::new().unwrap();
        exterior.parameter(Location).set_axis(0, 0.125).unwrap();
        let content = animated_gradient();
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        group.parameter(3).unwrap().set_index(0).unwrap();
        let group = group.node();
        group
            .slot(0)
            .unwrap()
            .set_node(Some(&exterior.node()))
            .unwrap();
        group
            .content(0)
            .unwrap()
            .set_node(Some(&content.node()))
            .unwrap();
        let enclosing_group = PointwiseFilter::new(PointwiseKind::Group).unwrap().node();
        enclosing_group
            .content(0)
            .unwrap()
            .set_node(Some(&group))
            .unwrap();
        let scene = crate::SceneClearLightsNode::new().node();
        scene
            .slot(0)
            .unwrap()
            .set_node(Some(&enclosing_group))
            .unwrap();
        let geometry = ImageGeometry::new(7, 3);
        let mut backend = Material;
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_scene_renderer(&mut backend);
        let red = |image: &SharedImageMap| image.channel(0, false).unwrap().read(0).unwrap();
        for channels in [false, true] {
            let options = PointwiseRenderOptions::new(true, channels);
            for (node, expected) in [(&enclosing_group, 0.125), (&scene, 0.5)] {
                let original = node
                    .render_original_with_effects(7, 3, options, 0.5, CallingThread, &mut effects)
                    .unwrap();
                assert!(original.caught_errors().is_empty());
                assert_eq!(red(&original.image().unwrap()), expected);
            }
        }
        let mut timed = |node: &PointwiseNode<&'static str>, mode, options| {
            let outcome = node
                .render_cached_graph_with_effects(
                    mode,
                    geometry,
                    0.5,
                    CallingThread,
                    options,
                    &mut effects,
                )
                .unwrap();
            assert!(
                outcome.caught_errors().is_empty(),
                "{:?}",
                outcome.caught_errors()
            );
            outcome.output().unwrap()
        };
        let full = timed(&group, Preview, PointwiseRenderOptions::new(false, false));
        assert_eq!(full.frames().len(), 32);
        assert_eq!(red(&full.frames()[16]), 0.5);
        for channels in [false, true] {
            let options = PointwiseRenderOptions::new(true, channels);
            let image = timed(&group, Preview, options).still_image().unwrap();
            assert_eq!(red(&image), 0.125);
            let rendered = timed(&scene, Preview, options);
            assert_eq!(rendered.frames().len(), 32);
            for (i, frame) in rendered.frames().iter().enumerate() {
                assert_eq!(red(frame), i as f32 / 32.);
            }
            let floating = timed(&scene, Floating, options).still_image().unwrap();
            assert_eq!(red(&floating), 0.5);
        }
        let full_again = timed(&group, Preview, PointwiseRenderOptions::new(false, false));
        assert_eq!(red(&full_again.frames()[16]), 0.5);
        for channels in [false, true] {
            let options = PointwiseRenderOptions::new(true, channels);
            let final_image =
                |node: &PointwiseNode<&'static str>,
                 effects: &mut PointwiseRenderEffects<'_, &'static str>| {
                    node.final_graph_clone()
                        .unwrap()
                        .render_with_effects(geometry, 0.5, CallingThread, options, false, effects)
                        .unwrap()
                        .image()
                        .unwrap()
                };
            assert_eq!(red(&final_image(&group, &mut effects)), 0.125);
            assert_eq!(red(&final_image(&scene, &mut effects)), 0.5);
        }
    }

    #[test]
    fn failed_material_allocation_does_not_publish_the_gradient_image() {
        struct Backend;
        impl crate::SceneImageRenderer<&'static str> for Backend {
            fn preflight_array(
                &mut self,
                _: crate::SceneAllocationRequest,
            ) -> Result<(), crate::SceneError> {
                Err(crate::SceneError::Allocation)
            }

            fn render_scene_image(
                &mut self,
                _: &crate::Scene,
                _: i32,
                _: i32,
            ) -> Result<SharedImageMap, PointwiseError<&'static str>> {
                unreachable!()
            }
        }
        let gradient = animated_gradient();
        let root = crate::SceneClearLightsNode::new().node();
        root.slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut backend = Backend;
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_scene_renderer(&mut backend);
        assert!(matches!(
            root.render_cached_graph_with_effects(
                Preview,
                ImageGeometry::new(4, 2),
                0.,
                CallingThread,
                PointwiseRenderOptions::new(false, false),
                &mut effects,
            ),
            Err(PointwiseError::Scene(crate::SceneError::Allocation))
        ));
        assert!(gradient.cached_render_output(Preview).is_none());
        assert!(root.cached_graph_output(Preview).is_none());
    }

    #[test]
    fn animated_gradient_feeds_a_timed_graph_through_real_slots_and_group_contents() {
        let gradient = animated_gradient();
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap().node();
        group
            .content(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let invert = PointwiseNode::invert();
        invert.slot(0).unwrap().set_node(Some(&group)).unwrap();
        let root = PointwiseNode::<&'static str>::invert();
        root.slot(0).unwrap().set_node(Some(&invert)).unwrap();

        let events = Arc::new(Mutex::new(Vec::new()));
        for (node, name) in [
            (gradient.node(), "gradient"),
            (group.clone(), "group"),
            (invert.clone(), "invert"),
            (root.clone(), "root"),
        ] {
            let events = events.clone();
            node.add_rendered_image_change_handler(crate::PointwiseNodeChangeHandler::new(
                move |node, _| {
                    assert!(matches!(
                        node.cached_rendered_output(),
                        Some(RenderedImage::Animated(_))
                    ));
                    events.lock().unwrap().push(name);
                    Ok(())
                },
            ));
        }
        let preview = root
            .render_cached_graph(Preview, 4, 2, 0.75, CallingThread)
            .unwrap();
        assert!(preview.caught_errors().is_empty());
        let preview = preview.output().unwrap();
        assert_eq!(preview.frames().len(), 32);
        let source = gradient.cached_render_output(Preview).unwrap();
        assert!(source.same_reference(&group.cached_graph_output(Preview).unwrap()));
        assert!(preview.same_reference(&root.cached_rendered_output().unwrap()));
        for (index, frame) in preview.frames().iter().enumerate() {
            assert!(frame.is_permanently_sealed());
            assert_eq!(
                frame.materialize(0).unwrap().read(0).unwrap(),
                index as f32 / 32.
            );
        }
        let cached = root
            .render_cached_graph(Preview, 3, 3, 0., CallingThread)
            .unwrap()
            .output()
            .unwrap();
        assert!(preview.same_reference(&cached));
        assert_eq!(
            *events.lock().unwrap(),
            [
                "gradient", "group", "invert", "root", "gradient", "group", "invert", "root"
            ]
        );

        let floated = root
            .render_cached_graph(Floating, 4, 2, 0.25, CallingThread)
            .unwrap()
            .image()
            .unwrap();
        assert_eq!(floated.materialize(0).unwrap().read(0).unwrap(), 0.25);
        assert!(preview.same_reference(&root.cached_rendered_output().unwrap()));
        assert_eq!(events.lock().unwrap().len(), 8);
        let copy = root.source_clone().unwrap();
        assert!(preview.same_reference(&copy.cached_rendered_output().unwrap()));
        assert!(preview.same_reference(&copy.cached_graph_output(Preview).unwrap()));
        assert!(copy.cached_graph_output(Floating).is_none());

        gradient.parameter(Location).set_axis(1, 0.25).unwrap();
        for node in [gradient.node(), group, invert, root] {
            assert!(node.cached_rendered_output().is_none());
            assert!(node.cached_graph_output(Preview).is_none());
            assert!(node.cached_graph_output(Floating).is_none());
        }
    }

    #[test]
    fn wizard_control_drives_gradient_in_live_and_captured_image_graphs() {
        let wizard = PointwiseFilter::<&'static str>::new(PointwiseKind::WizardGroup).unwrap();
        let mut spec = crate::VariableSpec::new(crate::VARIABLE_NUMERIC);
        spec.set_name(Some(&"Offset".encode_utf16().collect::<Vec<_>>()));
        wizard.parameter(3).unwrap().set_variable(spec).unwrap();
        let control = wizard.wizard_dynamic_properties().unwrap().remove(0);
        control.set_axis_animated(0, true).unwrap();
        let curve = control.animation_curve(0).unwrap();
        curve.set_minimum(0.);
        curve.set_maximum(100.);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);

        let gradient = GradientGenerator::new().unwrap();
        let location = gradient.parameter(Location);
        location.set_binding_index(0);
        location.set_binding_name(Some(&"W0>0;100>1".encode_utf16().collect::<Vec<_>>()));
        let group = wizard.node();
        group
            .content(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();

        let snapshot = crate::RenderCaptureSession::new()
            .capture_node(&group, 0.375)
            .unwrap();
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let result = snapshot
            .render_with_execution_and_effects(
                4,
                2,
                CallingThread,
                PointwiseRenderOptions::new(false, false),
                &mut effects,
            )
            .unwrap();
        assert!(result.caught_errors().is_empty());
        let first_pixel = |image: &SharedImageMap| image.materialize(0).unwrap().read(0).unwrap();
        assert_eq!(first_pixel(&result.image().unwrap()), 0.375);

        let preview = group
            .render_cached_graph(Preview, 4, 2, 0., CallingThread)
            .unwrap();
        assert!(preview.caught_errors().is_empty());
        let preview = preview.output().unwrap();
        assert!(preview.same_reference(&gradient.cached_render_output(Preview).unwrap()));
        assert!(preview.same_reference(&group.cached_rendered_output().unwrap()));
        assert_eq!(preview.frames().len(), 32);
        for (i, frame) in preview.frames().iter().enumerate() {
            assert_eq!(first_pixel(frame), i as f32 / 32.);
        }
        let warm = group
            .render_cached_graph(Preview, 8, 2, 0.5, CallingThread)
            .unwrap()
            .output()
            .unwrap();
        assert!(warm.same_reference(&preview));
        let floating = group
            .render_cached_graph(Floating, 4, 2, 0.625, CallingThread)
            .unwrap();
        assert!(floating.caught_errors().is_empty());
        assert_eq!(first_pixel(&floating.image().unwrap()), 0.625);

        control.set_axis_animated(0, false).unwrap();
        assert!(gradient.cached_render_output(Preview).is_none());
        assert!(group.cached_graph_output(Preview).is_none());
        assert!(group.cached_graph_output(Floating).is_none());
    }

    #[test]
    fn timed_wizard_color_only_samples_its_exterior_input() {
        let wizard = PointwiseFilter::<&'static str>::new(PointwiseKind::WizardGroup).unwrap();
        let group = wizard.node();
        wizard.parameter(23).unwrap().set_index(0).unwrap();
        let gradient = animated_gradient();
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let options = PointwiseRenderOptions::new(true, false);
        let geometry = ImageGeometry::new(4, 2);
        let missing = group
            .render_cached_graph_with_effects(
                Preview,
                geometry,
                0.5,
                CallingThread,
                options,
                &mut effects,
            )
            .unwrap();
        assert!(missing.caught_errors().is_empty());
        assert_eq!(
            missing
                .image()
                .unwrap()
                .materialize(0)
                .unwrap()
                .read(0)
                .unwrap(),
            0.
        );
        wizard.parameter(0).unwrap().set_number(1.).unwrap();
        group
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let preview = group
            .render_cached_graph_with_effects(
                Preview,
                geometry,
                0.5,
                CallingThread,
                options,
                &mut effects,
            )
            .unwrap();
        assert!(preview.caught_errors().is_empty());
        let input = gradient.cached_render_output(Preview).unwrap();
        assert!(preview.image().unwrap().same_reference(&input.frames()[16]));
    }

    #[test]
    fn timed_ordinary_inputs_round_to_even_and_reject_the_end_of_the_sequence() {
        let gradient = animated_gradient();
        let input = gradient
            .render_cached(Preview, 4, 2, 0., CallingThread)
            .unwrap()
            .output()
            .unwrap();
        let root = PointwiseNode::<&'static str>::invert();
        let request = Request {
            mode: Preview,
            geometry: ImageGeometry::new(4, 2),
            time: 0.,
            execution: CallingThread,
            options: PointwiseRenderOptions::new(false, false),
        };
        let inputs = [Output::image_only(input, true)];
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        for (time, index) in [(1.5 / 32., 2), (2.5 / 32., 2), (3.5 / 32., 4)] {
            let output = request
                .frame(&root, &Operation::Ordinary, &inputs, time, &mut effects)
                .unwrap();
            assert_eq!(
                output.materialize(0).unwrap().read(0).unwrap(),
                1. - index as f32 / 32.
            );
        }
        assert!(matches!(
            request.frame(&root, &Operation::Ordinary, &inputs, 1., &mut effects),
            Err(PointwiseError::ArgumentIndex)
        ));
    }

    #[test]
    fn timed_group_prepares_slots_before_reversing_contents_and_forwarding_a_warm_reference() {
        let gradient = animated_gradient();
        let spare = GradientGenerator::new().unwrap();
        let extra = GradientGenerator::new().unwrap();
        let reference = PointwiseFilter::new(PointwiseKind::Reference)
            .unwrap()
            .node();
        let invert = PointwiseNode::invert();
        invert.slot(0).unwrap().set_node(Some(&reference)).unwrap();
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        group.parameter(0).unwrap().set_number(2.).unwrap();
        let group = group.node();
        group
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        group
            .slot(1)
            .unwrap()
            .set_node(Some(&spare.node()))
            .unwrap();
        group.content(0).unwrap().set_node(Some(&invert)).unwrap();
        group
            .append_content(Vec::new())
            .set_node(Some(&extra.node()))
            .unwrap();

        let events = Arc::new(Mutex::new(Vec::new()));
        for (node, name) in [
            (gradient.node(), "slot 1"),
            (spare.node(), "slot 2"),
            (extra.node(), "contents 2"),
            (reference.clone(), "reference"),
            (invert.clone(), "invert"),
            (group.clone(), "group"),
        ] {
            let events = events.clone();
            node.add_rendered_image_change_handler(crate::PointwiseNodeChangeHandler::new(
                move |_, _| {
                    events.lock().unwrap().push(name);
                    Ok(())
                },
            ));
        }
        let first = group
            .render_cached_graph(Preview, 4, 2, 0.25, CallingThread)
            .unwrap();
        assert!(first.caught_errors().is_empty());
        let first = first.output().unwrap();
        let gradient_preview = gradient.cached_render_output(Preview).unwrap();
        let warm_ref = reference.cached_graph_output(Preview).unwrap();
        assert!(warm_ref.same_reference(&gradient_preview));
        assert!(first.same_reference(&invert.cached_graph_output(Preview).unwrap()));
        assert_eq!(
            *events.lock().unwrap(),
            [
                "slot 1",
                "slot 2",
                "contents 2",
                "reference",
                "invert",
                "group"
            ]
        );
        let warm_ref = reference
            .render_cached_graph(Preview, 8, 3, 0.75, CallingThread)
            .unwrap()
            .output()
            .unwrap();
        assert!(warm_ref.same_reference(&gradient_preview));
        assert_eq!(warm_ref.frames()[0].geometry().width(), 4);
        assert_eq!(*events.lock().unwrap().last().unwrap(), "reference");

        let floated = group
            .render_cached_graph(Floating, 4, 2, 0.25, CallingThread)
            .unwrap()
            .image()
            .unwrap();
        assert_eq!(floated.materialize(0).unwrap().read(0).unwrap(), 0.75);
        let floating_ref = reference
            .render_cached_graph(Floating, 8, 4, 0.75, CallingThread)
            .unwrap()
            .image()
            .unwrap();
        assert!(floating_ref.same_reference(&gradient.cached_render_image(Floating).unwrap()));
        assert!(warm_ref.same_reference(&reference.cached_rendered_output().unwrap()));

        gradient
            .parameter(crate::GradientGeneratorParameterKind::Size)
            .set_axis(0, 2.)
            .unwrap();
        for affected in [&reference, &invert, &group] {
            assert!(affected.cached_graph_output(Preview).is_none());
            assert!(affected.cached_graph_output(Floating).is_none());
            assert!(affected.cached_rendered_output().is_none());
        }
        assert!(spare.cached_render_output(Preview).is_some());
        assert!(extra.cached_render_output(Preview).is_some());
        let changed = group
            .render_cached_graph(Preview, 4, 2, 0.25, CallingThread)
            .unwrap()
            .output()
            .unwrap();
        assert_ne!(
            first.frames()[0].materialize(0).unwrap().read(1).unwrap(),
            changed.frames()[0].materialize(0).unwrap().read(1).unwrap(),
        );
    }

    #[test]
    fn cold_reference_samples_and_resizes_target_frames_before_publishing_its_own_sequence() {
        let gradient = animated_gradient();
        let target = PointwiseFilter::new(PointwiseKind::Group).unwrap().node();
        target
            .content(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let outer = PointwiseFilter::new(PointwiseKind::Group).unwrap().node();
        let reference = PointwiseFilter::new(PointwiseKind::Reference)
            .unwrap()
            .node();
        outer.slot(0).unwrap().set_node(Some(&target)).unwrap();
        outer
            .content(0)
            .unwrap()
            .set_node(Some(&reference))
            .unwrap();
        let original = gradient
            .render_cached(Preview, 2, 2, 0., CallingThread)
            .unwrap()
            .output()
            .unwrap();
        assert!(target.cached_graph_output(Preview).is_none());

        let cold = reference
            .render_cached_graph(Preview, 4, 3, 0.25, CallingThread)
            .unwrap();
        assert!(cold.caught_errors().is_empty());
        let cold = cold.output().unwrap();
        assert!(!cold.same_reference(&original));
        assert_eq!(cold.frames().len(), 32);
        for index in [0, 7, 16, 31] {
            let expected = super::super::super::image_resize::resize::<&'static str>(
                &original.frames()[index],
                4,
                3,
                CallingThread,
            )
            .unwrap();
            let actual = &cold.frames()[index];
            assert_eq!(actual.geometry(), ImageGeometry::new(4, 3));
            assert_eq!(
                actual.materialize(0).unwrap().values(),
                expected.materialize(0).unwrap().values()
            );
        }
        assert!(original.same_reference(&target.cached_graph_output(Preview).unwrap()));
        let warm = reference
            .render_cached_graph(Preview, 4, 3, 0.25, CallingThread)
            .unwrap()
            .output()
            .unwrap();
        assert!(original.same_reference(&warm));
    }

    #[test]
    fn cold_reference_refreshes_its_variable_bound_slot_per_preview_frame() {
        let map = PointwiseFilter::<()>::new(PointwiseKind::MapVariable).unwrap();
        map.parameter(0).unwrap().set_number(2.).unwrap();
        let variable = map.parameter(3).unwrap();
        variable.set_axis_animated(0, true).unwrap();
        let curve = variable.animation_curve(0).unwrap();
        curve.set_minimum(0.);
        curve.set_maximum(100.);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        for (index, location) in [(0, 0.25), (1, 0.75)] {
            let gradient = GradientGenerator::new().unwrap();
            gradient.parameter(Location).set_axis(0, location).unwrap();
            map.node()
                .slot(index)
                .unwrap()
                .set_node(Some(&gradient.node()))
                .unwrap();
        }
        let reference = PointwiseFilter::new(PointwiseKind::Reference).unwrap();
        let selector = reference.parameter(0).unwrap();
        selector.set_binding_index(0);
        selector.set_binding_name(Some(&"0>1;100>2".encode_utf16().collect::<Vec<_>>()));
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&reference.node()))
            .unwrap();
        let rendered = reference
            .node()
            .render_cached_graph(Preview, 4, 2, 0., CallingThread)
            .unwrap();
        assert!(rendered.caught_errors().is_empty());
        let rendered = rendered.output().unwrap();
        assert_eq!(rendered.frames().len(), 32);
        for index in [0, 15, 16, 31] {
            assert_eq!(
                rendered.frames()[index]
                    .materialize(0)
                    .unwrap()
                    .read(0)
                    .unwrap(),
                if index < 16 { 0.25 } else { 0.75 },
            );
        }
        assert_eq!(selector.number(), 2.);
    }

    #[test]
    fn cold_reference_passes_the_frame_time_into_a_nested_color_only_group() {
        let gradient = animated_gradient();
        let target = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        target.parameter(3).unwrap().set_index(0).unwrap();
        target
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let outer = PointwiseFilter::new(PointwiseKind::Group).unwrap().node();
        let reference = PointwiseFilter::new(PointwiseKind::Reference)
            .unwrap()
            .node();
        outer
            .slot(0)
            .unwrap()
            .set_node(Some(&target.node()))
            .unwrap();
        outer
            .content(0)
            .unwrap()
            .set_node(Some(&reference))
            .unwrap();

        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let rendered = reference
            .render_cached_graph_with_effects(
                Preview,
                ImageGeometry::new(4, 2),
                0.5,
                CallingThread,
                PointwiseRenderOptions::new(true, false),
                &mut effects,
            )
            .unwrap()
            .output()
            .unwrap();
        // The Group's first request is at frame zero. Its resulting still
        // cache is reused for the remaining Reference frames.
        let source = gradient.cached_render_output(Preview).unwrap();
        assert!(
            rendered
                .frames()
                .iter()
                .all(|frame| frame.same_reference(&source.frames()[0]))
        );
    }

    #[test]
    fn timed_filter_refreshes_bound_raw_values_before_sampling_gradient_input() {
        let gradient = animated_gradient();
        let map = PointwiseFilter::new(PointwiseKind::MapVariable).unwrap();
        let variable = map.parameter(3).unwrap();
        variable.set_axis_animated(0, true).unwrap();
        let value = variable.animation_curve(0).unwrap();
        value.set_minimum(0.);
        value.set_maximum(100.);
        value.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);

        let brightness = PointwiseFilter::new(PointwiseKind::ChangeBci).unwrap();
        let adjustment = brightness.parameter(1).unwrap();
        adjustment.set_binding_index(0);
        adjustment.set_binding_name(Some(&"0>0;100>64".encode_utf16().collect::<Vec<_>>()));
        brightness
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&brightness.node()))
            .unwrap();

        let notified = Arc::new(AtomicBool::new(false));
        let changed = notified.clone();
        brightness
            .node()
            .add_change_handler(crate::PointwiseNodeChangeHandler::new(move |_, _| {
                changed.store(true, Ordering::Release);
                Ok(())
            }));
        let rendered = map
            .node()
            .render_cached_graph(Preview, 4, 2, 0., CallingThread)
            .unwrap();
        assert!(rendered.caught_errors().is_empty());
        let rendered = rendered.output().unwrap();
        assert!(rendered.same_reference(&brightness.node().cached_graph_output(Preview).unwrap()));
        assert_eq!(adjustment.number(), 62.);
        assert_eq!(adjustment.previous_number(), 62.);
        assert!(!notified.load(Ordering::Acquire));

        let source = gradient.cached_render_output(Preview).unwrap();
        let control = PointwiseFilter::<&'static str>::new(PointwiseKind::ChangeBci).unwrap();
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        for index in [0, 9, 16, 31] {
            control
                .parameter(1)
                .unwrap()
                .set_number(2. * index as f64)
                .unwrap();
            let frame = control
                .render_supplied_at_time_with_effects(
                    &[Some(source.frames()[index].clone())],
                    4,
                    2,
                    index as f64 / 32.,
                    CallingThread,
                    PointwiseRenderOptions::new(false, false),
                    &mut effects,
                )
                .unwrap()
                .unwrap();
            assert_eq!(
                rendered.frames()[index].materialize(0).unwrap().values(),
                frame.materialize(0).unwrap().values(),
            );
        }
    }

    #[test]
    fn timed_graph_cancellation_keeps_the_published_child_and_stops_before_the_parent() {
        let gradient = animated_gradient();
        let root = PointwiseNode::invert();
        root.slot(0)
            .unwrap()
            .set_node(Some(&gradient.node()))
            .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        gradient
            .node()
            .add_rendered_image_change_handler(crate::PointwiseNodeChangeHandler::new(
                move |_, _| {
                    cancel.store(true, Ordering::Release);
                    Ok(())
                },
            ));
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let stop = || cancelled.load(Ordering::Acquire);
        let mut effects =
            PointwiseRenderEffects::new(&mut capacity, &mut preflight).with_cancel_check(&stop);
        let result = root.render_cached_graph_with_effects(
            Preview,
            ImageGeometry::new(4, 2),
            0.,
            CallingThread,
            PointwiseRenderOptions::new(false, false),
            &mut effects,
        );
        assert!(matches!(result, Err(PointwiseError::Cancelled)));
        assert!(gradient.cached_render_output(Preview).is_some());
        assert!(root.cached_rendered_output().is_none());
        assert!(root.cached_graph_output(Preview).is_none());
    }
}
