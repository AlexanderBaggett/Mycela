use super::math::{hsb_values, rgb_values};
use super::numeric::{add, div, hue_remainder, mul, sub};
use super::{
    ArithmeticOperation as Op, ArithmeticWorker, PointwiseError, PointwiseFilter, PointwiseKind,
    SharedImageMap, max32, max64, min32, min64,
};
use crate::{EnvironmentResourceLibrary, FloatBuffer, ImageError, ImageGeometry, SharedGradient};

/// Borrowed actual source memory effects. Never retained in model/capture data.
/// Combine asks a Boolean quality4 predicate; blur/Noise require the separate
/// void preflight (blur quality2). Caller releases policy locks before return.
pub struct PointwiseRenderEffects<'a, E> {
    capacity: &'a mut dyn FnMut(i32) -> Result<bool, PointwiseError<E>>,
    required: &'a mut dyn FnMut(i32, i32) -> Result<(), PointwiseError<E>>,
    scene_renderer: Option<&'a mut dyn super::SceneImageRenderer<E>>,
    cancelled: &'a dyn Fn() -> bool,
    environment_library: Option<&'a EnvironmentResourceLibrary>,
}
impl<'a, E> PointwiseRenderEffects<'a, E> {
    pub fn new(
        capacity: &'a mut dyn FnMut(i32) -> Result<bool, PointwiseError<E>>,
        preflight: &'a mut dyn FnMut(i32, i32) -> Result<(), PointwiseError<E>>,
    ) -> Self {
        Self {
            capacity,
            required: preflight,
            scene_renderer: None,
            cancelled: &|| false,
            environment_library: None,
        }
    }
    /// Borrow the active job's cancellation check for this graph evaluation.
    pub fn with_cancel_check(mut self, cancelled: &'a dyn Fn() -> bool) -> Self {
        self.cancelled = cancelled;
        self
    }
    pub fn with_environment_library(
        mut self,
        library: Option<&'a EnvironmentResourceLibrary>,
    ) -> Self {
        self.environment_library = library;
        self
    }

    pub(super) fn has_environment_library(&self) -> bool {
        self.environment_library
            .is_some_and(|library| library.root().is_some())
    }

    pub(super) fn environment_library(&self) -> Option<&'a EnvironmentResourceLibrary> {
        self.environment_library
    }

    pub(super) fn cancel_check(&self) -> &'a dyn Fn() -> bool {
        self.cancelled
    }
    /// Attach this execution's real scene backend. The backend is borrowed only
    /// for rendering; it never enters the retained graph or a captured frame.
    pub fn with_scene_renderer(
        mut self,
        renderer: &'a mut dyn super::SceneImageRenderer<E>,
    ) -> Self {
        self.scene_renderer = Some(renderer);
        self
    }
    pub(super) fn render_scene_node(
        &mut self,
        node: &super::PointwiseNode<E>,
        width: i32,
        height: i32,
        time: f64,
        execution: crate::ProceduralExecution,
        options: super::PointwiseRenderOptions,
    ) -> Result<SharedImageMap, PointwiseError<E>>
    where
        E: 'static,
    {
        if contains_uv_texture(node)? {
            let graph = node.clone_supported_final_graph()?.ok_or(
                PointwiseError::UnsupportedCapability("mixed UV texture graph"),
            )?;
            let result = graph.render_with_effects(
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                false,
                self,
            )?;
            return result.image().ok_or(PointwiseError::NullReference);
        }
        if contains_image_scene_input(node)? {
            return self
                .render_scene_tree_with_images(node, width, height, time, execution, options);
        }
        let scene =
            self.with_scene_allocation(|allocation| node.render_scene_optional(time, allocation))?;
        // Geometry/allocation callbacks and the subsequent transform reads have
        // completed. Neither a property guard nor a mesh guard spans the backend.
        self.rasterize_scene(scene.as_ref(), ImageGeometry::new(width, height))
    }

    fn with_scene_allocation<T>(
        &mut self,
        f: impl FnOnce(&mut crate::SceneAllocationEffects<'_>) -> Result<T, PointwiseError<E>>,
    ) -> Result<T, PointwiseError<E>> {
        let mut preflight = |request| match self.scene_renderer.as_deref_mut() {
            Some(renderer) => renderer.preflight_array(request),
            None => Err(crate::SceneError::MissingRenderer),
        };
        f(&mut crate::SceneAllocationEffects::new(&mut preflight))
    }

    pub(super) fn rasterize_scene(
        &mut self,
        scene: Option<&crate::Scene>,
        geometry: ImageGeometry,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        match scene {
            Some(scene) => self
                .scene_renderer
                .as_deref_mut()
                .ok_or(PointwiseError::Scene(crate::SceneError::MissingRenderer))?
                .render_scene_image(scene, geometry.width(), geometry.height()),
            None => Ok(SharedImageMap::new(geometry)),
        }
    }

    pub(super) fn rasterize_node_scene(
        &mut self,
        node: &super::PointwiseNode<E>,
        time: f64,
        scene: Option<&crate::Scene>,
        geometry: ImageGeometry,
    ) -> Result<SharedImageMap, PointwiseError<E>>
    where
        E: 'static,
    {
        let Some(scene) = scene else {
            return Ok(SharedImageMap::new(geometry));
        };
        let renderer = self
            .scene_renderer
            .as_deref_mut()
            .ok_or(PointwiseError::Scene(crate::SceneError::MissingRenderer))?;
        if let Some(light) = node.uv_light_at(time)? {
            renderer.render_uv_light_image(scene, geometry, light)
        } else if node.is_uv_vertex_map() {
            renderer.render_uv_vertex_map_image(scene, geometry)
        } else if node.is_uv_environment_light() {
            renderer.render_uv_material_image(scene, geometry)
        } else {
            renderer.render_scene_image(scene, geometry.width(), geometry.height())
        }
    }

    pub(super) fn generate_scene(
        &mut self,
        node: &super::PointwiseNode<E>,
        time: f64,
    ) -> Result<Option<crate::Scene>, PointwiseError<E>>
    where
        E: 'static,
    {
        self.with_scene_allocation(|allocation| {
            if let Some(primitive) = node.scene_primitive() {
                primitive.render_scene(time, allocation).map(Some)
            } else {
                node.render_imported_scene(time, allocation)
            }
        })
    }

    pub(super) fn image_as_scene(
        &mut self,
        image: &SharedImageMap,
    ) -> Result<crate::Scene, PointwiseError<E>> {
        let scene = self.untextured_plane()?;
        let limit = self
            .scene_renderer
            .as_deref_mut()
            .ok_or(PointwiseError::Scene(crate::SceneError::MissingRenderer))?
            .material_texture_limit()?;
        scene
            .attach_fresh_plane_image(image, limit)
            .map_err(PointwiseError::Scene)?;
        Ok(scene)
    }

    fn untextured_plane(&mut self) -> Result<crate::Scene, PointwiseError<E>> {
        self.with_scene_allocation(|allocation| {
            crate::generate_primitive(
                crate::ScenePrimitiveParameters::plane(1, 1, true),
                allocation,
            )
            .map_err(PointwiseError::Scene)
        })
    }

    /// The geometry-only Model demand follows a 3D output without evaluating
    /// its image or material branches. Other images supply a plain plane.
    pub(super) fn geometry_scene(
        &mut self,
        root: super::PointwiseNode<E>,
        time: f64,
        reference: impl Fn(
            &super::PointwiseNode<E>,
        ) -> Result<Option<super::PointwiseNode<E>>, PointwiseError<E>>,
    ) -> Result<Option<crate::Scene>, PointwiseError<E>>
    where
        E: 'static,
    {
        use super::region::NodeId;
        use super::{PointwiseKind, PointwiseNode};
        enum Frame<E> {
            Enter(PointwiseNode<E>),
            Forward(NodeId),
            Modify(PointwiseNode<E>, usize),
            Empty,
        }
        let mut pending = vec![Frame::Enter(root)];
        let mut active = std::collections::BTreeSet::new();
        let mut scenes = Vec::new();
        while let Some(frame) = pending.pop() {
            super::check_render_cancelled(self.cancelled)?;
            match frame {
                Frame::Enter(node) => {
                    if !active.insert(node.id) {
                        return Err(PointwiseError::UnsupportedCapability(
                            "cyclic geometry scheduling",
                        ));
                    }
                    if node.is_scene_generator() {
                        scenes.push(self.generate_scene(&node, time)?);
                    } else if is_scene_modifier(&node) {
                        let slots = node.slots();
                        pending.push(Frame::Modify(node, slots.len()));
                        pending.extend(
                            slots
                                .into_iter()
                                .rev()
                                .map(|slot| slot.node().map_or(Frame::Empty, Frame::Enter)),
                        );
                        continue;
                    } else if node.uv_texture_output().is_some() && !node.uv_bakes_map(time)? {
                        pending.push(Frame::Forward(node.id));
                        pending.push(node.slot(0)?.node().map_or(Frame::Empty, Frame::Enter));
                        continue;
                    } else {
                        let target = match node.kind() {
                            Some(PointwiseKind::Reference) => reference(&node)?,
                            Some(kind) if kind.is_group() => node.content(0)?.node(),
                            _ => None,
                        };
                        if let Some(target) = target {
                            pending.push(Frame::Forward(node.id));
                            pending.push(Frame::Enter(target));
                            continue;
                        }
                        scenes.push(Some(self.untextured_plane()?));
                    }
                    active.remove(&node.id);
                }
                Frame::Modify(node, count) => {
                    let children = scenes.split_off(scenes.len() - count);
                    scenes.push(self.modify_prepared_scene(
                        &node,
                        children.into_iter().map(Ok),
                        time,
                    )?);
                    active.remove(&node.id);
                }
                Frame::Forward(id) => {
                    active.remove(&id);
                }
                Frame::Empty => scenes.push(None),
            }
        }
        Ok(scenes.pop().flatten())
    }

    pub(super) fn bake_uv_texture(
        &self,
        node: &super::PointwiseNode<E>,
        time: f64,
        scene: Option<&crate::Scene>,
        texture: Option<&SharedImageMap>,
        geometry: ImageGeometry,
    ) -> Result<SharedImageMap, PointwiseError<E>>
    where
        E: 'static,
    {
        let Some(image) = texture.filter(|_| !node.uv_bakes_mesh_material()) else {
            let models = scene
                .map(crate::Scene::try_models_snapshot)
                .transpose()
                .map_err(PointwiseError::Scene)?
                .unwrap_or_default();
            return self.bake_uv_mesh(node, time, &models, texture, geometry);
        };
        let Some(projection) = node.uv_projection_at(time)? else {
            return crate::scene::uv_apply::bake(scene, image, geometry, self.cancel_check());
        };
        let Some(scene) = scene else {
            return Ok(SharedImageMap::new(geometry));
        };
        let models = scene.try_models_snapshot().map_err(PointwiseError::Scene)?;
        let Some(model) = models.first() else {
            return Ok(SharedImageMap::new(geometry));
        };
        crate::scene::uv_project::bake(model, image, geometry, &projection, self.cancel_check())
    }

    fn bake_uv_mesh(
        &self,
        node: &super::PointwiseNode<E>,
        time: f64,
        models: &[crate::MeshModel],
        image: Option<&SharedImageMap>,
        geometry: ImageGeometry,
    ) -> Result<SharedImageMap, PointwiseError<E>>
    where
        E: 'static,
    {
        let cancelled = self.cancel_check();
        if let Some(settings) = node.uv_occlusion_at(time)? {
            crate::scene::uv_occlusion::bake(models, geometry, settings, image, cancelled)
        } else if node.is_uv_environment_light() {
            crate::scene::uv_environment_light::bake(models, image, geometry, cancelled)
        } else if let Some(light) = node.uv_light_at(time)? {
            crate::scene::uv_light::bake(models, geometry, light, cancelled)
        } else if node.is_uv_thickness() {
            crate::scene::uv_thickness::bake(models, geometry, cancelled)
        } else {
            crate::scene::uv_curvature::bake(models, geometry, cancelled)
        }
    }

    pub(super) fn texture_uv_scene(
        &mut self,
        node: &super::PointwiseNode<E>,
        time: f64,
        model: Option<crate::Scene>,
        image: Option<&SharedImageMap>,
        geometry: ImageGeometry,
        options: super::PointwiseRenderOptions,
    ) -> Result<Option<crate::Scene>, PointwiseError<E>>
    where
        E: 'static,
    {
        // y7's scene output leaves the Model as geometry. Its lighting shader
        // is applied by wy only when a caller asks for the camera image.
        if node.is_uv_light() {
            return Ok(model);
        }
        let Some(model) = model else {
            return Ok(None);
        };
        let scene = self.with_scene_allocation(|allocation| {
            model
                .source_copy_with_effects(allocation)
                .map_err(PointwiseError::Scene)
        })?;
        let limit = self
            .scene_renderer
            .as_deref_mut()
            .ok_or(PointwiseError::Scene(crate::SceneError::MissingRenderer))?
            .material_texture_limit()?;
        let Some(texture) = image.filter(|_| !node.uv_bakes_mesh_material()) else {
            let geometry = if node.is_uv_environment_light() {
                crate::scene::uv_environment_light::material_geometry(geometry, limit)?
            } else {
                geometry
            };
            for model in scene.try_models_snapshot().map_err(PointwiseError::Scene)? {
                let map =
                    self.bake_uv_mesh(node, time, std::slice::from_ref(&model), image, geometry)?;
                model
                    .material()
                    .replace_color_map(&map, limit)
                    .map_err(PointwiseError::Scene)?;
            }
            return Ok(Some(scene));
        };
        let image = if options.is_color_only() {
            let copy = texture.shallow_copy()?;
            copy.clear_named_channels();
            copy
        } else {
            texture.clone()
        };
        if let Some(projection) = node.uv_projection_at(time)? {
            for model in scene.try_models_snapshot().map_err(PointwiseError::Scene)? {
                let map = crate::scene::uv_project::bake(
                    &model,
                    &image,
                    image.geometry(),
                    &projection,
                    self.cancel_check(),
                )?;
                model
                    .material()
                    .replace_color_map(&map, limit)
                    .map_err(PointwiseError::Scene)?;
            }
        } else {
            scene
                .replace_material_maps(&image, limit)
                .map_err(PointwiseError::Scene)?;
        }
        Ok(Some(scene))
    }

    pub(super) fn modify_prepared_scene(
        &mut self,
        node: &super::PointwiseNode<E>,
        children: impl Iterator<Item = Result<Option<crate::Scene>, PointwiseError<E>>>,
        time: f64,
    ) -> Result<Option<crate::Scene>, PointwiseError<E>>
    where
        E: 'static,
    {
        self.with_scene_allocation(|allocation| {
            // aqr.g samples and source-copies each slot before reading the next.
            let mut copies = children.map(|child| {
                child?
                    .map(|scene| scene.source_copy_with_effects(allocation))
                    .transpose()
                    .map_err(PointwiseError::Scene)
            });
            let first = copies.next().transpose()?.flatten();
            if node.scene_combine_3d().is_some() {
                let second = copies.next().transpose()?.flatten();
                return crate::combine_3d(first, second).map_err(PointwiseError::Scene);
            }
            if let Some(scene) = &first {
                if let Some(transform) = node.scene_transform_3d() {
                    transform.apply(scene, time)?;
                } else {
                    scene.clear_lights().map_err(PointwiseError::Scene)?;
                }
            }
            Ok(first)
        })
    }
    // The source zl.o schedules scene children in slot order before invoking
    // aqr's scene operation. Every aqr.g then copies the scheduled child.
    // This bounded route uses the existing node, image and scene owners; it
    // does not synthesize a second graph or treat an image as untextured.
    fn render_scene_tree_with_images(
        &mut self,
        root: &super::PointwiseNode<E>,
        width: i32,
        height: i32,
        time: f64,
        execution: crate::ProceduralExecution,
        options: super::PointwiseRenderOptions,
    ) -> Result<SharedImageMap, PointwiseError<E>>
    where
        E: 'static,
    {
        validate_image_scene_tree(root)?;
        enum Frame<E> {
            Enter(super::PointwiseNode<E>),
            Leave(super::PointwiseNode<E>, usize),
            Forward(super::PointwiseNode<E>),
            Empty,
        }
        let mut frames = vec![Frame::Enter(root.clone())];
        let mut active = std::collections::BTreeSet::new();
        let mut results: Vec<Option<crate::Scene>> = Vec::new();
        while let Some(frame) = frames.pop() {
            super::check_render_cancelled(self.cancel_check())?;
            match frame {
                Frame::Empty => results.push(None),
                Frame::Enter(node) => {
                    if active.contains(&node.id) {
                        return Err(PointwiseError::UnsupportedCapability(
                            "cyclic scene graph scheduling",
                        ));
                    }
                    let forward = original_scene_input(&node)?;
                    if forward.is_none()
                        && let Some(image) = original_cached_material(&node)?
                    {
                        results.push(Some(self.image_as_scene(&image)?));
                        continue;
                    }
                    // zl.o pushes one frame per slot occurrence. A completed
                    // cold child does not publish an Original image cache.
                    if let Some(target) = forward {
                        match node.kind() {
                            Some(kind) if kind.is_group() => {
                                super::graph::validate_group_render(&node)?
                            }
                            Some(PointwiseKind::Reference) => {
                                super::graph::validate_reference_render(&node)?
                            }
                            _ => unreachable!(),
                        }
                        active.insert(node.id);
                        frames.push(Frame::Forward(node));
                        frames.push(Frame::Enter(target));
                        continue;
                    }
                    if node.is_clone_mode()
                        || (!node.contents().is_empty()
                            && node.kind() != Some(PointwiseKind::Canvas)
                            && !node.kind().is_some_and(PointwiseKind::is_group))
                    {
                        return Err(PointwiseError::UnsupportedCapability(
                            "scene image scheduling context",
                        ));
                    }
                    if node.is_scene_generator() {
                        results.push(self.generate_scene(&node, time)?);
                    } else if is_scene_modifier(&node) {
                        active.insert(node.id);
                        let slots = node.slots();
                        frames.push(Frame::Leave(node, slots.len()));
                        for slot in slots.into_iter().rev() {
                            frames.push(match slot.node() {
                                Some(child) => Frame::Enter(child),
                                None => Frame::Empty,
                            });
                        }
                    } else {
                        let outcome = node.render_original_with_effects(
                            width, height, options, time, execution, self,
                        )?;
                        if !outcome.caught_errors().is_empty() {
                            return Err(PointwiseError::UnsupportedCapability(
                                "scene image child caught exception scheduling",
                            ));
                        }
                        let image = outcome.image().ok_or(PointwiseError::NullReference)?;
                        results.push(Some(self.image_as_scene(&image)?));
                    }
                }
                Frame::Forward(node) => {
                    active.remove(&node.id);
                }
                Frame::Leave(node, count) => {
                    active.remove(&node.id);
                    let children = results.split_off(results.len() - count);
                    results.push(self.modify_prepared_scene(
                        &node,
                        children.into_iter().map(Ok),
                        time,
                    )?);
                }
            }
        }
        self.rasterize_scene(
            results.pop().flatten().as_ref(),
            ImageGeometry::new(width, height),
        )
    }
    pub(super) fn preflight(&mut self, width: i32, quality: i32) -> Result<(), PointwiseError<E>> {
        (self.required)(width, quality)
    }
}
pub(super) fn is_scene_modifier<E: 'static>(node: &super::PointwiseNode<E>) -> bool {
    node.scene_clear_lights().is_some()
        || node.scene_transform_3d().is_some()
        || node.scene_combine_3d().is_some()
}

// A Model dependency requests the first contents scene from zl.o even when
// the Group's image comes from an exterior slot. A plain image in contents
// supplies its own textured plane. References still use their scene target
// or materialize a plane from the Reference image.
fn original_scene_input<E: 'static>(
    node: &super::PointwiseNode<E>,
) -> Result<Option<super::PointwiseNode<E>>, PointwiseError<E>> {
    if node.kind().is_some_and(PointwiseKind::is_group) && node.cached_image().is_none() {
        Ok(node.content(0)?.node())
    } else {
        node.scene_forward_target()
    }
}

// zl.o uses a warm Still RenderedImageMap as the plane material. An ordinary
// Reference inherits the saved target's cache until its selector is animated.
fn original_cached_material<E: 'static>(
    node: &super::PointwiseNode<E>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    if node.is_scene_node() || node.is_clone_mode() {
        return Ok(None);
    }
    let mut source = node.clone();
    let mut visited = std::collections::BTreeSet::new();
    while source.kind() == Some(PointwiseKind::Reference) {
        if !visited.insert(source.id) {
            return Err(PointwiseError::UnsupportedCapability(
                "cyclic scene graph scheduling",
            ));
        }
        super::graph::validate_reference_render(&source)?;
        let selector = source
            .filter()
            .ok_or(PointwiseError::InvalidCast)?
            .parameter(0)?;
        if super::parameter_ops::is_animated(|| selector.retained_animation())?
            || selector.has_animated_binding()?
        {
            break;
        }
        let Some(target) = source.reference_target()? else {
            break;
        };
        source = target;
    }
    Ok(source.cached_image())
}

fn contains_uv_texture<E: 'static>(
    root: &super::PointwiseNode<E>,
) -> Result<bool, PointwiseError<E>> {
    let mut pending = vec![root.clone()];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(node) = pending.pop() {
        if !seen.insert(node.id) {
            continue;
        }
        if node.uv_texture_output().is_some() {
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

// Check only the scene-valued branches; image subtrees are validated separately.
fn contains_image_scene_input<E: 'static>(
    root: &super::PointwiseNode<E>,
) -> Result<bool, PointwiseError<E>> {
    use std::collections::BTreeSet;
    let mut stack = vec![root.clone()];
    let mut seen = BTreeSet::new();
    while let Some(node) = stack.pop() {
        if !seen.insert(node.id) {
            continue;
        }
        if let Some(target) = original_scene_input(&node)? {
            stack.push(target);
        } else if node.is_scene_generator() {
            continue;
        } else if is_scene_modifier(&node) {
            for slot in node.slots() {
                if let Some(child) = slot.node() {
                    stack.push(child);
                }
            }
        } else {
            return Ok(true);
        }
    }
    Ok(false)
}

// Validate each role a node serves before performing any rendering or scene
// allocation. The actual render steps stay per occurrence, as in zl.o.
fn validate_image_scene_tree<E: 'static>(
    root: &super::PointwiseNode<E>,
) -> Result<(), PointwiseError<E>> {
    use std::collections::BTreeSet;
    enum Frame<E> {
        Enter(super::PointwiseNode<E>, bool),
        Leave(super::region::NodeId),
    }
    let mut frames = vec![Frame::Enter(root.clone(), true)];
    let mut active = BTreeSet::new();
    let mut checked = BTreeSet::new();
    while let Some(frame) = frames.pop() {
        match frame {
            Frame::Leave(id) => {
                active.remove(&id);
            }
            Frame::Enter(node, scene) => {
                if active.contains(&node.id) {
                    return Err(PointwiseError::UnsupportedCapability(
                        "cyclic scene graph scheduling",
                    ));
                }
                if !checked.insert((node.id, scene)) {
                    continue;
                }
                let forward = if scene {
                    original_scene_input(&node)?
                } else {
                    None
                };
                let scene_node =
                    node.is_scene_generator() || is_scene_modifier(&node) || forward.is_some();
                if (scene && !scene_node && original_cached_material(&node)?.is_some())
                    || (!scene && !node.is_clone_mode() && node.cached_image().is_some())
                {
                    continue;
                }
                active.insert(node.id);
                if let Some(target) = forward {
                    match node.kind() {
                        Some(kind) if kind.is_group() => {
                            super::graph::validate_group_render(&node)?
                        }
                        Some(PointwiseKind::Reference) => {
                            super::graph::validate_reference_render(&node)?
                        }
                        _ => unreachable!(),
                    }
                    frames.push(Frame::Leave(node.id));
                    frames.push(Frame::Enter(target, true));
                    continue;
                }
                let is_group = node.kind().is_some_and(PointwiseKind::is_group);
                if is_group {
                    super::graph::validate_group_render(&node)?;
                } else if node.kind() == Some(PointwiseKind::Reference) {
                    super::graph::validate_reference_render(&node)?;
                }
                if node.is_clone_mode()
                    || (!node.contents().is_empty()
                        && !is_group
                        && node.kind() != Some(PointwiseKind::Canvas))
                {
                    return Err(PointwiseError::UnsupportedCapability(
                        if scene && scene_node {
                            "scene image scheduling context"
                        } else {
                            "scene image child scheduling context"
                        },
                    ));
                }
                if !scene && scene_node {
                    return Err(PointwiseError::UnsupportedCapability(
                        "scene image child scheduling context",
                    ));
                }
                frames.push(Frame::Leave(node.id));
                if scene && scene_node && node.is_scene_generator() {
                    continue;
                }
                let next_scene = scene && scene_node;
                // Image-valued Groups evaluate their retained contents and
                // References follow their actual group input.
                if !next_scene {
                    if node.kind() == Some(PointwiseKind::Reference)
                        && let Some(target) = node.reference_target()?
                    {
                        frames.push(Frame::Enter(target, false));
                    }
                    for slot in node.contents().into_iter().rev() {
                        if let Some(child) = slot.node() {
                            frames.push(Frame::Enter(child, false));
                        }
                    }
                }
                for slot in node.slots().into_iter().rev() {
                    if let Some(child) = slot.node() {
                        frames.push(Frame::Enter(child, next_scene));
                    }
                }
            }
        }
    }
    Ok(())
}

fn get<E>(
    inputs: &[Option<SharedImageMap>],
    ix: usize,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    inputs.get(ix).cloned().ok_or(PointwiseError::Index)
}
fn required<E>(input: Option<SharedImageMap>) -> Result<SharedImageMap, PointwiseError<E>> {
    input.ok_or(PointwiseError::NullReference)
}
fn copy(source: &FloatBuffer, destination: &FloatBuffer, length: usize) -> Result<(), ImageError> {
    if source.len() < length || destination.len() < length {
        return Err(ImageError::Argument);
    }
    // Array.Copy handles overlap; these renderer copies have fresh destinations.
    let values = source.values();
    for (ix, value) in values.into_iter().take(length).enumerate() {
        destination.write(ix, value)?;
    }
    Ok(())
}
pub(super) fn factor(value: f64) -> f32 {
    let mut value = value;
    if value > 1000.0 {
        let delta = value - 1000.0;
        value += libm::pow(delta, delta / 1500.0 + 0.6) + delta;
    }
    (value / 100.0) as f32
}
fn native_log(a: f64, base: f64) -> f64 {
    if a.is_nan() {
        return a;
    }
    if base.is_nan() {
        return base;
    }
    if base == 1.0 || a != 1.0 && (base == 0.0 || base == f64::INFINITY) {
        // Exact invalid-base constant in Framework Math.Log0600103d CIL.
        return f64::from_bits(0xfff8_0000_0000_0000);
    }
    libm::log(a) / libm::log(base)
}
fn gamma(mid: f32, min: f32, max: f32) -> f32 {
    if mid == max {
        return 0.01;
    }
    if mid == min {
        return 9.99;
    }
    min64(
        max64(
            1.0 / native_log(0.5, f64::from(div(sub(mid, min), sub(max, min)))),
            0.01,
        ),
        9.99,
    ) as f32
}
fn level<E>(
    value: f32,
    lo: f32,
    scale: f32,
    exponent: f32,
    dense: bool,
) -> Result<f32, PointwiseError<E>> {
    let delta = sub(value, lo);
    let sign = if delta < 0.0 {
        -1.0
    } else if delta > 0.0 {
        1.0
    } else if delta == 0.0 {
        0.0
    } else {
        return Err(PointwiseError::Arithmetic);
    };
    let mut value =
        (sign * libm::pow(f64::from(mul(delta.abs(), scale)), f64::from(exponent))) as f32;
    if value == f32::INFINITY {
        value = f32::MAX;
    } else if value == f32::NEG_INFINITY {
        value = f32::MIN;
    } else if dense && value.is_nan() {
        return Err(PointwiseError::Exception);
    }
    Ok(value)
}
impl<E: 'static> PointwiseFilter<E> {
    /// Runs the actual source protected-k8 numeric seam over supplied retained
    /// maps. Upstream graph/cache/property animation is not emulated here.
    pub fn render_supplied(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        self.render_supplied_with_options(
            inputs,
            width,
            height,
            super::PointwiseRenderOptions::new(false, false),
        )
    }
    /// Supplied-image render with an actual lazy Combine memory predicate.
    /// The callback runs only at the source's dense width>=500 count decision,
    /// before area/base arithmetic, and is not retained by this model. It must
    /// release any capacity-state guard before returning; the kernel runs after
    /// the callback. No other node kind queries this provider.
    pub fn render_supplied_with_capacity_query(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
        options: super::PointwiseRenderOptions,
        query: &mut impl FnMut(i32) -> Result<bool, PointwiseError<E>>,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        if self.kind() == PointwiseKind::Combine {
            return super::combine::render_with_capacity_query(
                self,
                inputs,
                ImageGeometry::new(width, height),
                options,
                None,
                query,
                &|| false,
            );
        }
        self.render_supplied_with_options(inputs, width, height, options)
    }
    /// The protected-k8 caller's actual wo flags. Effect-map and Combine bodies
    /// read render_channels at their source-defined positions. No
    /// scene/clone/editor context is fabricated by this supplied-image call.
    pub fn render_supplied_with_options(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
        options: super::PointwiseRenderOptions,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        if matches!(
            self.kind(),
            PointwiseKind::Canvas
                | PointwiseKind::Style
                | PointwiseKind::Synthesis
                | PointwiseKind::PointLight
                | PointwiseKind::IlluminateG2
                | PointwiseKind::Shadow
                | PointwiseKind::ShadowG2
                | PointwiseKind::Atmosphere
                | PointwiseKind::Hair
                | PointwiseKind::LegacyBricksBoards
                | PointwiseKind::ConvertEnvironment
                | PointwiseKind::MakeEnvironment
                | PointwiseKind::RotateEnvironment
                | PointwiseKind::DiffuseEnvironment
                | PointwiseKind::ScatterEnvironment
                | PointwiseKind::ReflectEnvironment
                | PointwiseKind::RefractEnvironment
                | PointwiseKind::Reflect
                | PointwiseKind::Refract
                | PointwiseKind::Stereogram
                | PointwiseKind::Regions
                | PointwiseKind::Caustics
                | PointwiseKind::Transmit
                | PointwiseKind::FullLight
                | PointwiseKind::Polygon
                | PointwiseKind::WizardColor
                | PointwiseKind::WizardFiveColor
                | PointwiseKind::WizardSlideColor
                | PointwiseKind::WizardToneCurve
                | PointwiseKind::WizardKaleidoscope
        ) {
            return self.render_supplied_at_time(
                inputs,
                width,
                height,
                0.0,
                crate::ProceduralExecution::CallingThread,
                options,
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::SmartBlur
                | PointwiseKind::Smear
                | PointwiseKind::VariableBlur
                | PointwiseKind::ArtBlur
        ) {
            return self.render_supplied_at_time(
                inputs,
                width,
                height,
                0.,
                crate::ProceduralExecution::CallingThread,
                options,
            );
        }

        if super::transform::owns(self.kind()) {
            return super::transform::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                0.,
                crate::ProceduralExecution::CallingThread,
                options,
            );
        }
        if self.kind() == PointwiseKind::MatchColor {
            return super::match_color::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                0.,
                crate::ProceduralExecution::CallingThread,
                &|| false,
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::Exposure | PointwiseKind::Selector
        ) {
            return super::exposure_selector::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                0.,
                crate::ProceduralExecution::CallingThread,
                &|| false,
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::Frequency | PointwiseKind::Fractalize
        ) {
            return super::frequency_fractalize::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                0.,
                crate::ProceduralExecution::CallingThread,
                &|| false,
            );
        }

        if self.kind() == PointwiseKind::LayerMaps {
            return super::layer_maps::render(self, inputs, width, height, options, None);
        }
        if matches!(
            self.kind(),
            PointwiseKind::Equalize
                | PointwiseKind::GaussianBlur
                | PointwiseKind::Sharpen
                | PointwiseKind::MedianBlur
                | PointwiseKind::MotionBlur
                | PointwiseKind::ShapeBlur
        ) {
            let mut capacity = |_| {
                Err(PointwiseError::UnsupportedCapability(
                    "Combine ambient capacity predicate",
                ))
            };
            let mut required = |_, _| {
                Err(PointwiseError::UnsupportedCapability(
                    "blur required memory preflight",
                ))
            };
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut required);
            return (if self.kind() == PointwiseKind::Equalize {
                super::equalize::retained
            } else if self.kind() == PointwiseKind::ShapeBlur {
                super::shape::retained
            } else if self.kind() == PointwiseKind::MotionBlur {
                super::motion::retained
            } else if self.kind() == PointwiseKind::MedianBlur {
                super::median::retained
            } else {
                super::blur::retained
            })(
                self,
                inputs,
                ImageGeometry::new(width, height),
                0.0,
                crate::ProceduralExecution::CallingThread,
                &mut effects,
            );
        }
        match self.kind() {
            PointwiseKind::WizardGroup
            | PointwiseKind::Group
            | PointwiseKind::MapVariable
            | PointwiseKind::WizardReference
            | PointwiseKind::TemplateLab(_) => Err(PointwiseError::UnsupportedCapability(
                "Group contents and variable evaluation",
            )),
            PointwiseKind::Reference => Err(PointwiseError::UnsupportedCapability(
                "Reference group context",
            )),
            PointwiseKind::Pixelate | PointwiseKind::Matrix | PointwiseKind::WizardMatrix => self
                .render_spatial_at_time(
                    inputs,
                    width,
                    height,
                    0.0,
                    crate::ProceduralExecution::CallingThread,
                ),
            PointwiseKind::Rotate => super::rotate::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                0.0,
                crate::ProceduralExecution::CallingThread,
            ),
            PointwiseKind::Combine => {
                super::combine::render(self, inputs, width, height, options, None, None)
            }
            PointwiseKind::ToneCurve => {
                super::tone_curve::render(self, inputs, width, height, None)
            }
            PointwiseKind::ToneCurveG2 => {
                super::tone_curve::render_legacy(self, inputs, width, height, None)
            }
            PointwiseKind::OklabColor => super::oklab::render(
                self,
                inputs,
                ImageGeometry::new(width, height),
                0.0,
                &|| false,
            )
            .map(Some),
            PointwiseKind::Checkerboard => {
                super::checkerboard::render(self, ImageGeometry::new(width, height), 0.0, &|| false)
                    .map(Some)
            }
            PointwiseKind::ClearMaps
            | PointwiseKind::StoreMap
            | PointwiseKind::MergeMaps
            | PointwiseKind::FixMapContrast => {
                super::effect_maps::render(self, inputs, width, height, options, None)
            }
            PointwiseKind::Distort
            | PointwiseKind::FreeTransform
            | PointwiseKind::Kaleidoscope
            | PointwiseKind::MoveRepeat
            | PointwiseKind::Remap
            | PointwiseKind::RotateTile
            | PointwiseKind::MatchColor
            | PointwiseKind::ArtBlur
            | PointwiseKind::Exposure
            | PointwiseKind::Selector
            | PointwiseKind::Frequency
            | PointwiseKind::Fractalize
            | PointwiseKind::LayerMaps
            | PointwiseKind::Equalize
            | PointwiseKind::PointLight
            | PointwiseKind::IlluminateG2
            | PointwiseKind::Shadow
            | PointwiseKind::ShadowG2
            | PointwiseKind::Atmosphere
            | PointwiseKind::Hair
            | PointwiseKind::LegacyBricksBoards
            | PointwiseKind::Canvas
            | PointwiseKind::Shader
            | PointwiseKind::Style
            | PointwiseKind::Synthesis
            | PointwiseKind::ConvertEnvironment
            | PointwiseKind::MakeEnvironment
            | PointwiseKind::RotateEnvironment
            | PointwiseKind::DiffuseEnvironment
            | PointwiseKind::ScatterEnvironment
            | PointwiseKind::ReflectEnvironment
            | PointwiseKind::RefractEnvironment
            | PointwiseKind::Reflect
            | PointwiseKind::Refract
            | PointwiseKind::Stereogram
            | PointwiseKind::Regions
            | PointwiseKind::Caustics
            | PointwiseKind::Transmit
            | PointwiseKind::FullLight
            | PointwiseKind::Polygon
            | PointwiseKind::WizardColor
            | PointwiseKind::WizardFiveColor
            | PointwiseKind::WizardSlideColor
            | PointwiseKind::WizardToneCurve
            | PointwiseKind::WizardKaleidoscope
            | PointwiseKind::GaussianBlur
            | PointwiseKind::Sharpen
            | PointwiseKind::MedianBlur
            | PointwiseKind::MotionBlur
            | PointwiseKind::ShapeBlur
            | PointwiseKind::SmartBlur
            | PointwiseKind::Smear
            | PointwiseKind::VariableBlur => {
                unreachable!("dedicated supplied dispatch")
            }
            PointwiseKind::Add => {
                let a = get(inputs, 0)?;
                let b = get(inputs, 1)?;
                let a = required(a)?;
                if a.is_empty()? {
                    return Ok(b);
                }
                let b = required(b)?;
                if b.is_empty()? {
                    return Ok(Some(a));
                }
                let out = SharedImageMap::new(ImageGeometry::new(width, height));
                for ch in 0..4 {
                    let x = a.channel(ch, false)?;
                    let y = b.channel(ch, false)?;
                    match (x.len(), y.len()) {
                        (1, 1) => out.set_constant(ch, add(x.read(0)?, y.read(0)?))?,
                        (n, 1) if n > 1 => ArithmeticWorker::new(
                            Op::AddScalar,
                            Some(out.materialize(ch)?),
                            Some(x),
                            None,
                            y.read(0)?,
                        )
                        .run()?,
                        (1, n) if n > 1 => ArithmeticWorker::new(
                            Op::AddScalar,
                            Some(out.materialize(ch)?),
                            Some(y),
                            None,
                            x.read(0)?,
                        )
                        .run()?,
                        (n, m) if n > 1 && m > 1 => ArithmeticWorker::new(
                            Op::AddArrays,
                            Some(out.materialize(ch)?),
                            Some(x),
                            Some(y),
                            0.0,
                        )
                        .run()?,
                        _ => (),
                    }
                    // ajc.k8 seals every completed ordinary-channel branch.
                    out.seal_channel(ch)?;
                }
                out.set_region_store(a.region_store().or_else(|| b.region_store()));
                out.merge_effects(&[Some(a), Some(b)])?;
                Ok(Some(out))
            }
            PointwiseKind::Multiply => {
                let a = get(inputs, 0)?;
                let b = get(inputs, 1)?;
                let out = SharedImageMap::new(ImageGeometry::new(width, height));
                let a = required(a)?;
                // Math.Min(A.ng(), B.ng()) evaluates A before dereferencing B.
                let a_count = a.channel_count()?;
                let b = required(b)?;
                let count = a_count.min(b.channel_count()?);
                for ch in 0..count as i32 {
                    let x = a.channel(ch, false)?;
                    let y = b.channel(ch, false)?;
                    if (x.len() == 1 && x.read(0)? == 0.0) || (y.len() == 1 && y.read(0)? == 0.0) {
                        continue;
                    }
                    match (x.len(), y.len()) {
                        (1, 1) => out.set_constant(ch, mul(x.read(0)?, y.read(0)?))?,
                        (1, n) if n > 1 => ArithmeticWorker::new(
                            Op::MultiplyScalar,
                            Some(out.materialize(ch)?),
                            Some(y),
                            None,
                            x.read(0)?,
                        )
                        .run()?,
                        (n, 1) if n > 1 => ArithmeticWorker::new(
                            Op::MultiplyScalar,
                            Some(out.materialize(ch)?),
                            Some(x),
                            None,
                            y.read(0)?,
                        )
                        .run()?,
                        (n, m) if n > 1 && m > 1 => ArithmeticWorker::new(
                            Op::MultiplyArrays,
                            Some(out.materialize(ch)?),
                            Some(x),
                            Some(y),
                            0.0,
                        )
                        .run()?,
                        _ => (),
                    }
                    // ub.k8 skips sealing along with arithmetic for scalar zero.
                    out.seal_channel(ch)?;
                }
                out.set_region_store(a.region_store().or_else(|| b.region_store()));
                out.merge_effects(&[Some(a), Some(b)])?;
                Ok(Some(out))
            }
            PointwiseKind::FromHsb => {
                let a = get(inputs, 0)?;
                let b = get(inputs, 1)?;
                let c = get(inputs, 2)?;
                // Actual output allocation precedes every input read/materialize.
                let out = SharedImageMap::new(ImageGeometry::new(width, height));
                let a = required(a)?;
                let mut x = a.channel(0, false)?;
                let b = required(b)?;
                let mut y = b.channel(0, false)?;
                let c = required(c)?;
                let mut z = c.channel(0, false)?;
                if z.len() != 1 || z.read(0)? != 0.0 {
                    if x.len() == 1 && y.len() == 1 && z.len() == 1 {
                        let values = hsb_values(x.read(0)?, y.read(0)?, z.read(0)?);
                        for (ch, v) in values.into_iter().enumerate() {
                            out.set_constant(ch as i32, v)?;
                        }
                    } else {
                        x = a.materialize(0)?;
                        y = b.materialize(0)?;
                        z = c.materialize(0)?;
                        let dest = [
                            out.materialize(0)?,
                            out.materialize(1)?,
                            out.materialize(2)?,
                        ];
                        let mut ix = 0;
                        let count = width.wrapping_mul(height);
                        while ix < count {
                            let at = ix as usize;
                            let values = hsb_values(x.read(at)?, y.read(at)?, z.read(at)?);
                            for (ch, v) in values.into_iter().enumerate() {
                                dest[ch].write(at, v)?;
                            }
                            ix = ix.wrapping_add(1);
                        }
                    }
                }
                out.merge_effects(&[Some(a), Some(b), Some(c)])?;
                Ok(Some(out))
            }
            PointwiseKind::ChangeBci => self.render_bci(get(inputs, 0)?, width, height, None),
            PointwiseKind::ChangeHsb => self
                .render_hsb(get(inputs, 0)?, width, height, None)
                .map(Some),
            PointwiseKind::Colorize => self
                .render_colorize(get(inputs, 0)?, width, height)
                .map(Some),
            PointwiseKind::ColorizeG2 => super::colorize_g2::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                None,
                &|| false,
            ),
            PointwiseKind::Levels => self.render_levels(get(inputs, 0)?, width, height).map(Some),
            PointwiseKind::GetChannel => self
                .render_channel(get(inputs, 0)?, width, height, None)
                .map(Some),
        }
    }
    fn render_bci(
        &self,
        input: Option<SharedImageMap>,
        width: i32,
        height: i32,
        time: Option<f64>,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        let auto = self.parameter(0)?;
        let bright = self.parameter(1)?;
        let contrast = self.parameter(2)?;
        let intensity = self.parameter(3)?;
        let (auto_index, brightness, contrast_value, intensity_value, animated_or_bound) =
            match time {
                Some(time) => {
                    let brightness = bright.effective_value_at(time)?;
                    let contrast_value = contrast.effective_value_at(time)?;
                    let intensity_value = intensity.effective_value_at(time)?;
                    let mut animated_or_bound = false;
                    for property in [&auto, &bright, &contrast, &intensity] {
                        if super::parameter_ops::is_animated(|| property.retained_animation())?
                            || property.has_animated_binding()?
                            || property.binding_index() >= 0
                        {
                            animated_or_bound = true;
                            break;
                        }
                    }
                    (
                        auto.effective_index_at(time)?,
                        brightness,
                        contrast_value,
                        intensity_value,
                        animated_or_bound,
                    )
                }
                None => (
                    auto.index(),
                    bright.number(),
                    contrast.number(),
                    intensity.number(),
                    false,
                ),
            };
        let unchanged = brightness == 0.0 && contrast_value == 0.0 && intensity_value == 0.0;
        if auto_index == 1 && unchanged && !animated_or_bound {
            return Ok(input);
        }
        let out = SharedImageMap::new(ImageGeometry::new(width, height));
        let mut input = required(input)?;
        out.copy_auxiliary(&input)?;
        let entries = auto.options()?.entries();
        let text = entries
            .get(usize::try_from(auto_index).map_err(|_| PointwiseError::Index)?)
            .ok_or(PointwiseError::Index)?;
        if text
            .as_ref()
            .is_some_and(|text| text.iter().copied().eq("Yes".encode_utf16()))
        {
            let mut active = [false; 3];
            for (ch, on) in active.iter_mut().enumerate() {
                let a = input.channel(ch as i32, false)?;
                for value in a.values() {
                    if value != 0.0 {
                        *on = true;
                        break;
                    }
                }
            }
            let (mut lo, mut hi) = (1.0, 0.0);
            for (ch, on) in active.iter().enumerate() {
                if *on {
                    for value in input.channel(ch as i32, false)?.values() {
                        if value < lo {
                            lo = value;
                        }
                        if value > hi {
                            hi = value;
                        }
                    }
                }
            }
            if unchanged && lo == 0.0 && hi == 1.0 {
                return Ok(Some(input));
            }
            if lo < 0.0 {
                lo = 0.0;
            }
            if hi > 1.0 {
                hi = 1.0;
            }
            if hi != lo && sub(hi, lo) < 0.998 {
                let normalized = SharedImageMap::new(ImageGeometry::new(width, height));
                for ch in 3..input.channel_count()? as i32 {
                    normalized.set_buffer(ch, Some(&input.channel(ch, false)?))?;
                }
                let scale = div(1.0, sub(hi, lo));
                for (ch, on) in active.into_iter().enumerate() {
                    if on {
                        let source = input.channel(ch as i32, false)?;
                        let dest = normalized.destination(ch as i32, &source)?;
                        for ix in 0..source.len() {
                            dest.write(ix, mul(sub(source.read(ix)?, lo), scale))?;
                        }
                    }
                }
                input = normalized;
            }
        }
        let brightness = (brightness / 100.0) as f32;
        let contrast = factor(contrast_value);
        let intensity = factor(intensity_value);
        for ch in 0..3 {
            let source = input.channel(ch, false)?;
            let dest = out.destination(ch, &source)?;
            if brightness != 0.0 {
                ArithmeticWorker::new(
                    Op::AddScalar,
                    Some(dest.clone()),
                    Some(source),
                    None,
                    brightness,
                )
                .run()?;
            } else {
                copy(&source, &dest, source.len())?;
            }
            if contrast != 0.0 {
                for ix in 0..dest.len() {
                    let value = dest.read(ix)?;
                    dest.write(ix, add(value, mul(sub(value, 0.5), contrast)))?;
                }
            }
            if intensity != 0.0 {
                for ix in 0..dest.len() {
                    let value = dest.read(ix)?;
                    dest.write(ix, add(value, mul(value, intensity)))?;
                }
            }
            let first = max32(0.0, min32(1.0, dest.read(0)?));
            for ix in 0..dest.len() {
                let value = dest.read(ix)?;
                if value > 1.0 {
                    dest.write(ix, 1.0)?;
                } else if value < 0.0 {
                    dest.write(ix, 0.0)?;
                }
            }
            let mut constant = true;
            for ix in 0..dest.len() {
                if constant && dest.read(ix)? != first {
                    constant = false;
                }
            }
            if constant {
                out.set_constant(ch, first)?;
            }
        }
        out.set_buffer(3, Some(&input.channel(3, false)?))?;
        Ok(Some(out))
    }
    fn render_hsb(
        &self,
        input: Option<SharedImageMap>,
        width: i32,
        height: i32,
        time: Option<f64>,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let out = SharedImageMap::new(ImageGeometry::new(width, height));
        let input = required(input)?;
        let arrays = [
            input.channel(0, false)?,
            input.channel(1, false)?,
            input.channel(2, false)?,
        ];
        let count = arrays[0].len().max(arrays[1].len().max(arrays[2].len()));
        let mut current = [arrays[0].read(0)?, arrays[1].read(0)?, arrays[2].read(0)?];
        let dest = std::array::from_fn::<_, 3, _>(|_| FloatBuffer::new(vec![0.0; count]));
        let number = |index| {
            let property = self.parameter(index)?;
            match time {
                Some(time) => property.effective_value_at(time),
                None => Ok(property.number()),
            }
        };
        let mut hue = hue_remainder(div(number(0)? as f32, 360.0));
        if hue < 0.0 {
            hue = add(hue, 1.0);
        }
        let saturation = div(number(1)? as f32, 100.0);
        let brightness = div(number(2)? as f32, 100.0);
        for ix in 0..count {
            for ch in 0..3 {
                if arrays[ch].len() > 1 {
                    current[ch] = arrays[ch].read(ix)?;
                }
            }
            let mut hsb = if current == [0.0; 3] {
                // Black has no hue or saturation. The legacy RGB conversion's
                // epsilon fallback gives black a large negative hue that tints it
                // when Brightness is increased.
                [0.0; 3]
            } else {
                rgb_values(current[0], current[1], current[2], false)
            };
            if hue != 0.0 {
                hsb[0] = add(hsb[0], hue);
                if hsb[0] >= 1.0 {
                    hsb[0] = sub(hsb[0], 1.0);
                }
            }
            if current[0] != current[1] || current[0] != current[2] {
                if saturation <= 0.0 {
                    hsb[1] = mul(hsb[1], add(saturation, 1.0));
                } else {
                    hsb[1] = add(hsb[1], mul(saturation, sub(1.0, hsb[1])));
                }
            }
            if brightness <= 0.0 {
                hsb[2] = mul(hsb[2], add(brightness, 1.0));
            } else {
                hsb[2] = add(hsb[2], mul(brightness, sub(1.0, hsb[2])));
            }
            let rgb = hsb_values(hsb[0], hsb[1], hsb[2]);
            for ch in 0..3 {
                dest[ch].write(ix, rgb[ch])?;
            }
        }
        for (ch, a) in dest.iter().enumerate() {
            out.set_buffer(ch as i32, Some(a))?;
        }
        out.set_buffer(3, Some(&input.channel(3, false)?))?;
        out.copy_auxiliary(&input)?;
        Ok(out)
    }
    fn render_channel(
        &self,
        input: Option<SharedImageMap>,
        width: i32,
        height: i32,
        time: Option<f64>,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let out = SharedImageMap::new(ImageGeometry::new(width, height));
        let selection = self.parameter(0)?;
        let index = match time {
            Some(time) => selection.effective_index_at(time)?,
            None => selection.index(),
        };
        let entries = selection.options()?.entries();
        let selected = entries
            .get(usize::try_from(index).map_err(|_| PointwiseError::Index)?)
            .ok_or(PointwiseError::Index)?;
        let matches = |name: &str| {
            selected
                .as_ref()
                .is_some_and(|text| text.iter().copied().eq(name.encode_utf16()))
        };
        let input = required(input)?;
        if let Some(ch) = ["Red", "Green", "Blue", "Transparency"]
            .into_iter()
            .position(matches)
        {
            out.set_buffer(0, Some(&input.channel(ch as i32, false)?))?;
        } else {
            let arrays = [
                input.channel(0, false)?,
                input.channel(1, false)?,
                input.channel(2, false)?,
            ];
            let mut current = [arrays[0].read(0)?, arrays[1].read(0)?, arrays[2].read(0)?];
            let dest = if arrays.iter().any(|a| a.len() > 1) {
                out.materialize(0)?
            } else {
                out.channel(0, false)?
            };
            let channel = if matches("Saturation") {
                1
            } else if matches("Brightness") {
                2
            } else {
                0
            };
            for ix in 0..dest.len() {
                for ch in 0..3 {
                    if arrays[ch].len() > 1 {
                        current[ch] = arrays[ch].read(ix)?;
                    }
                }
                // Black has no hue. The legacy conversion's zero-brightness
                // fallback yields an out-of-range hue that corrupts downstream color.
                let value = if channel == 0 && current == [0.0; 3] {
                    0.0
                } else {
                    rgb_values(current[0], current[1], current[2], false)[channel]
                };
                dest.write(ix, value)?;
            }
        }
        out.copy_auxiliary(&input)?;
        Ok(out)
    }
    fn render_levels(
        &self,
        input: Option<SharedImageMap>,
        width: i32,
        height: i32,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let hist = self.parameter(0)?.histogram()?;
        let out = SharedImageMap::new(ImageGeometry::new(width, height));
        let input = required(input)?;
        for ch in 0..4 {
            let source = input.channel(ch, false)?;
            let dest = out.destination(ch, &source)?;
            if ch == 3 {
                copy(&source, &dest, dest.len())?;
            } else {
                let p = hist.projection(ch)?.ok_or(PointwiseError::NullReference)?;
                let lo = p.read(0)?;
                let mid = p.read(1)?;
                let hi = p.read(2)?;
                let gamma = gamma(mid, lo, hi);
                let scale = div(1.0, sub(hi, lo));
                let exponent = div(1.0, gamma);
                if source.len() == 1 {
                    out.set_constant(ch, level(source.read(0)?, lo, scale, exponent, false)?)?;
                } else {
                    let mut ix = 0;
                    let count = width.wrapping_mul(height);
                    while ix < count {
                        let at = ix as usize;
                        dest.write(at, level(source.read(at)?, lo, scale, exponent, true)?)?;
                        ix = ix.wrapping_add(1);
                    }
                }
            }
            // yd.k8 seals both transformed RGB and copied alpha stores.
            out.seal_channel(ch)?;
        }
        out.copy_auxiliary(&input)?;
        Ok(out)
    }
    fn render_colorize(
        &self,
        input: Option<SharedImageMap>,
        width: i32,
        height: i32,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let out = SharedImageMap::new(ImageGeometry::new(width, height));
        let input = required(input)?;
        let source = input.channel(0, false)?;
        let gradient = self.parameter(0)?.gradient()?;
        let channels = gradient.render_channels()?;
        for (ch, map) in channels.iter().enumerate() {
            let shared = if ch < 3 {
                matching_channel(&gradient, ch)?
            } else {
                None
            };
            if let Some(old) = shared {
                out.set_buffer(ch as i32, Some(&out.channel(old as i32, false)?))?;
            } else if map.len() > 1 {
                let dest = out.destination(ch as i32, &source)?;
                interpolate(&source, &dest, map).map_err(PointwiseError::Interpolation)?;
            } else {
                out.set_constant(ch as i32, *map.first().ok_or(PointwiseError::Index)? as f32)?;
            }
            // cl.k8 seals before the optional later Keep Transparency alias.
            out.seal_channel(ch as i32)?;
        }
        if out.channel_count()? <= 3
            && self.parameter(1)?.text()?.as_deref()
                == Some("Yes".encode_utf16().collect::<Vec<_>>().as_slice())
        {
            out.set_buffer(3, Some(&input.channel(3, false)?))?;
        }
        out.copy_auxiliary(&input)?;
        Ok(out)
    }
}
fn matching_channel<E>(
    gradient: &SharedGradient,
    ch: usize,
) -> Result<Option<usize>, PointwiseError<E>> {
    for prior in 0..ch {
        let mut same = true;
        for ix in 0..gradient.color_count()? {
            let color = gradient
                .color_point(ix)?
                .ok_or(PointwiseError::NullReference)?
                .color()
                .argb();
            if !(color[prior + 1] - color[ch + 1]).abs().lt(&1e-5) {
                same = false;
                break;
            }
        }
        if same {
            return Ok(Some(prior));
        }
    }
    Ok(None)
}

// cl.g wraps the actual worker exception; the stored typed cause survives.
fn interpolate(
    source: &FloatBuffer,
    destination: &FloatBuffer,
    map: &[f64],
) -> Result<(), ImageError> {
    for ix in 0..destination.len() {
        if let Some(sample) = colorize_table_sample(map, source.read(ix)?)? {
            destination.write(ix, sample)?;
        }
    }
    Ok(())
}

pub(super) fn colorize_table_sample(map: &[f64], value: f32) -> Result<Option<f32>, ImageError> {
    if map.len() == 1 || value <= 0.0 {
        return Ok(Some(*map.first().ok_or(ImageError::Index)? as f32));
    }
    if value >= 1.0 {
        return Ok(Some(*map.last().ok_or(ImageError::Index)? as f32));
    }
    let intervals = map.len().checked_sub(1).ok_or(ImageError::Index)?;
    let offset = f64::from(mul(value, intervals as f32));
    let Ok(index) = usize::try_from(crate::framework_cast::integer(offset)) else {
        return Ok(None);
    };
    let first = *map.get(index).ok_or(ImageError::Index)?;
    let second = *map.get(index + 1).ok_or(ImageError::Index)?;
    Ok(Some(
        (first + (offset - index as f64) * (second - first)) as f32,
    ))
}

impl<E: 'static> PointwiseFilter<E> {
    /// Supplied retained render with both distinct source memory operations.
    #[allow(clippy::too_many_arguments)]
    pub fn render_supplied_at_time_with_effects(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
        time: f64,
        execution: crate::ProceduralExecution,
        options: super::PointwiseRenderOptions,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        if self.kind() == PointwiseKind::Shader {
            let program = self
                .node()
                .shader_program()
                .ok_or(PointwiseError::InvalidOperation)?;
            if program.is_imported_pixel_script() {
                return Err(PointwiseError::Shader(
                    "Imported PixelScript uses C#. Open Edit shader and replace it with WGSL."
                        .into(),
                ));
            }
            return effects
                .scene_renderer
                .as_mut()
                .ok_or(PointwiseError::UnsupportedCapability("Shader renderer"))?
                .render_shader_image(&program, inputs, ImageGeometry::new(width, height), time)
                .map(Some);
        }
        if self.kind() == PointwiseKind::Canvas {
            return super::canvas::render(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                effects,
            )
            .map(Some);
        }
        if self.kind() == PointwiseKind::Synthesis {
            return super::canvas::render_synthesis(
                self,
                inputs.first().and_then(Option::as_ref),
                ImageGeometry::new(width, height),
                time,
                execution,
                options.synthesis_layer_for(self.node().id),
                effects,
            )
            .map(Some);
        }
        if self.kind() == PointwiseKind::Style {
            return super::canvas::render_style(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                effects,
            )
            .map(Some);
        }
        if self.kind() == PointwiseKind::LegacyBricksBoards {
            return super::legacy_bricks::render(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                effects.cancel_check(),
            )
            .map(Some);
        }
        if self.kind() == PointwiseKind::Hair {
            return super::hair::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                effects.cancel_check(),
            );
        }
        if self.kind() == PointwiseKind::OklabColor {
            return super::oklab::render(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                effects.cancel_check(),
            )
            .map(Some);
        }
        if self.kind() == PointwiseKind::Checkerboard {
            return super::checkerboard::render(
                self,
                ImageGeometry::new(width, height),
                time,
                effects.cancel_check(),
            )
            .map(Some);
        }
        if self.kind() == PointwiseKind::ColorizeG2 {
            return super::colorize_g2::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                Some(time),
                effects.cancel_check(),
            );
        }
        if self.kind() == PointwiseKind::Polygon {
            return super::polygon::retained(
                self,
                ImageGeometry::new(width, height),
                time,
                execution,
                effects.cancel_check(),
            )
            .map(Some);
        }
        if self.kind() == PointwiseKind::FullLight {
            return super::full_light::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                effects,
            );
        }
        if self.kind() == PointwiseKind::Transmit {
            return super::transmit::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                options,
                effects.cancel_check(),
            );
        }
        if self.kind() == PointwiseKind::Caustics {
            return super::caustics::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                options,
                effects.cancel_check(),
            );
        }
        if self.kind() == PointwiseKind::Stereogram {
            return super::stereogram::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                effects.cancel_check(),
            );
        }
        if matches!(self.kind(), PointwiseKind::Reflect | PointwiseKind::Refract) {
            return super::planar_optics::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                effects.cancel_check(),
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::ReflectEnvironment | PointwiseKind::RefractEnvironment
        ) {
            return super::environment_optics::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                effects.cancel_check(),
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::DiffuseEnvironment | PointwiseKind::ScatterEnvironment
        ) {
            return super::environment_blur::retained(self, inputs, time, effects.cancel_check());
        }
        if self.kind() == PointwiseKind::Atmosphere {
            return super::atmosphere::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                effects.cancel_check(),
            );
        }
        if self.kind() == PointwiseKind::MakeEnvironment {
            return super::make_environment::retained(
                self,
                width,
                height,
                time,
                execution,
                effects.environment_library,
                effects.cancel_check(),
            );
        }
        if self.kind() == PointwiseKind::ConvertEnvironment {
            return super::convert_environment::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                effects.cancel_check(),
            );
        }
        if self.kind() == PointwiseKind::RotateEnvironment {
            return super::rotate_environment::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                effects.cancel_check(),
            );
        }
        if self.kind() == PointwiseKind::Shadow {
            return super::shadow::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                effects.cancel_check(),
            );
        }
        if self.kind() == PointwiseKind::ShadowG2 {
            return super::shadow_g2::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                options,
                effects.cancel_check(),
            );
        }
        if self.kind() == PointwiseKind::PointLight {
            return super::point_light::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                effects,
            );
        }
        if self.kind() == PointwiseKind::IlluminateG2 {
            return super::illuminate_g2::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                options,
                effects.cancel_check(),
            );
        }
        if self.kind() == PointwiseKind::ArtBlur {
            return super::art_blur::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                effects,
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::SmartBlur | PointwiseKind::Smear | PointwiseKind::VariableBlur
        ) {
            return super::adaptive::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                effects,
            );
        }

        if matches!(
            self.kind(),
            PointwiseKind::Equalize
                | PointwiseKind::GaussianBlur
                | PointwiseKind::Sharpen
                | PointwiseKind::MedianBlur
                | PointwiseKind::MotionBlur
                | PointwiseKind::ShapeBlur
        ) {
            return (if self.kind() == PointwiseKind::Equalize {
                super::equalize::retained
            } else if self.kind() == PointwiseKind::ShapeBlur {
                super::shape::retained
            } else if self.kind() == PointwiseKind::MotionBlur {
                super::motion::retained
            } else if self.kind() == PointwiseKind::MedianBlur {
                super::median::retained
            } else {
                super::blur::retained
            })(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                effects,
            );
        }
        if self.kind() == PointwiseKind::Combine {
            let cancelled = effects.cancel_check();
            return super::combine::render_with_capacity_query(
                self,
                inputs,
                ImageGeometry::new(width, height),
                options,
                Some(time),
                effects.capacity,
                cancelled,
            );
        }
        if self.kind() == PointwiseKind::Fractalize {
            return super::frequency_fractalize::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                effects.cancel_check(),
            );
        }
        if self.kind() == PointwiseKind::MatchColor {
            return super::match_color::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                effects.cancel_check(),
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::Exposure | PointwiseKind::Selector
        ) {
            return super::exposure_selector::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                effects.cancel_check(),
            );
        }
        self.render_supplied_at_time(inputs, width, height, time, execution, options)
    }
    pub fn render_supplied_at_time(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
        time: f64,
        execution: crate::ProceduralExecution,
        options: super::PointwiseRenderOptions,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        if matches!(
            self.kind(),
            PointwiseKind::Canvas | PointwiseKind::Style | PointwiseKind::Synthesis
        ) {
            let mut capacity = |_| Ok(true);
            let mut preflight = |_, _| Ok(());
            return self.render_supplied_at_time_with_effects(
                inputs,
                width,
                height,
                time,
                execution,
                options,
                &mut PointwiseRenderEffects::new(&mut capacity, &mut preflight),
            );
        }
        if self.kind() == PointwiseKind::LegacyBricksBoards {
            return super::legacy_bricks::render(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                &|| false,
            )
            .map(Some);
        }
        if self.kind() == PointwiseKind::ColorizeG2 {
            return super::colorize_g2::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                Some(time),
                &|| false,
            );
        }
        if self.kind() == PointwiseKind::Hair {
            return super::hair::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                &|| false,
            );
        }
        if self.kind() == PointwiseKind::OklabColor {
            return super::oklab::render(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                &|| false,
            )
            .map(Some);
        }
        if self.kind() == PointwiseKind::Checkerboard {
            return super::checkerboard::render(
                self,
                ImageGeometry::new(width, height),
                time,
                &|| false,
            )
            .map(Some);
        }
        if self.kind() == PointwiseKind::Regions {
            return super::regions::retained(self, inputs, ImageGeometry::new(width, height), time)
                .map(Some);
        }
        if self.kind() == PointwiseKind::MakeEnvironment {
            return super::make_environment::retained(
                self,
                width,
                height,
                time,
                execution,
                None,
                &|| false,
            );
        }
        if self.kind() == PointwiseKind::Combine {
            return super::combine::render(self, inputs, width, height, options, None, Some(time));
        }
        if self.kind() == PointwiseKind::ToneCurve {
            return super::tone_curve::render(self, inputs, width, height, Some(time));
        }
        if self.kind() == PointwiseKind::ToneCurveG2 {
            return super::tone_curve::render_legacy(self, inputs, width, height, Some(time));
        }
        if self.kind() == PointwiseKind::LayerMaps {
            return super::layer_maps::render(self, inputs, width, height, options, Some(time));
        }
        if matches!(
            self.kind(),
            PointwiseKind::ClearMaps
                | PointwiseKind::StoreMap
                | PointwiseKind::MergeMaps
                | PointwiseKind::FixMapContrast
        ) {
            return super::effect_maps::render(self, inputs, width, height, options, Some(time));
        }
        if self.kind() == PointwiseKind::ChangeBci {
            return self.render_bci(get(inputs, 0)?, width, height, Some(time));
        }
        if self.kind() == PointwiseKind::ChangeHsb {
            return self
                .render_hsb(get(inputs, 0)?, width, height, Some(time))
                .map(Some);
        }
        if self.kind() == PointwiseKind::GetChannel {
            return self
                .render_channel(get(inputs, 0)?, width, height, Some(time))
                .map(Some);
        }
        if matches!(
            self.kind(),
            PointwiseKind::WizardFiveColor | PointwiseKind::WizardSlideColor
        ) {
            return super::wizard_color::palettes(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
            );
        }
        if self.kind() == PointwiseKind::WizardToneCurve {
            return super::tone_curve::render_wizard(self, inputs, width, height, time);
        }
        if self.kind() == PointwiseKind::WizardKaleidoscope {
            return super::transform::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
            );
        }
        if self.kind() == PointwiseKind::WizardColor {
            return super::wizard_color::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
            );
        }
        if self.kind() == PointwiseKind::Polygon {
            return super::polygon::retained(
                self,
                ImageGeometry::new(width, height),
                time,
                execution,
                &|| false,
            )
            .map(Some);
        }
        if self.kind() == PointwiseKind::FullLight {
            let mut capacity = |_| {
                Err(PointwiseError::UnsupportedCapability(
                    "Combine ambient capacity predicate",
                ))
            };
            let mut preflight = |_, _| {
                Err(PointwiseError::UnsupportedCapability(
                    "blur required memory preflight",
                ))
            };
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            return super::full_light::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                &mut effects,
            );
        }
        if self.kind() == PointwiseKind::Transmit {
            return super::transmit::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                options,
                &|| false,
            );
        }
        if self.kind() == PointwiseKind::Caustics {
            return super::caustics::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                options,
                &|| false,
            );
        }
        if self.kind() == PointwiseKind::Stereogram {
            return super::stereogram::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                &|| false,
            );
        }
        if matches!(self.kind(), PointwiseKind::Reflect | PointwiseKind::Refract) {
            return super::planar_optics::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                &|| false,
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::ReflectEnvironment | PointwiseKind::RefractEnvironment
        ) {
            return super::environment_optics::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                &|| false,
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::DiffuseEnvironment | PointwiseKind::ScatterEnvironment
        ) {
            return super::environment_blur::retained(self, inputs, time, &|| false);
        }
        if self.kind() == PointwiseKind::Atmosphere {
            return super::atmosphere::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                &|| false,
            );
        }
        if self.kind() == PointwiseKind::ConvertEnvironment {
            return super::convert_environment::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                &|| false,
            );
        }
        if self.kind() == PointwiseKind::RotateEnvironment {
            return super::rotate_environment::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                &|| false,
            );
        }
        if self.kind() == PointwiseKind::Shadow {
            return super::shadow::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                &|| false,
            );
        }
        if self.kind() == PointwiseKind::ShadowG2 {
            return super::shadow_g2::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                options,
                &|| false,
            );
        }
        if self.kind() == PointwiseKind::PointLight {
            let mut capacity = |_| {
                Err(PointwiseError::UnsupportedCapability(
                    "Combine ambient capacity predicate",
                ))
            };
            let mut preflight = |_, _| {
                Err(PointwiseError::UnsupportedCapability(
                    "blur required memory preflight",
                ))
            };
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            return super::point_light::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                &mut effects,
            );
        }
        if self.kind() == PointwiseKind::IlluminateG2 {
            return super::illuminate_g2::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                options,
                &|| false,
            );
        }
        if self.kind() == PointwiseKind::ArtBlur {
            let mut capacity = |_| {
                Err(PointwiseError::UnsupportedCapability(
                    "Combine ambient capacity predicate",
                ))
            };
            let mut required = |_, _| {
                Err(PointwiseError::UnsupportedCapability(
                    "blur required memory preflight",
                ))
            };
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut required);
            return super::art_blur::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                &mut effects,
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::SmartBlur | PointwiseKind::Smear | PointwiseKind::VariableBlur
        ) {
            let mut capacity = |_| {
                Err(PointwiseError::UnsupportedCapability(
                    "Combine ambient capacity predicate",
                ))
            };
            let mut preflight = |_, _| {
                Err(PointwiseError::UnsupportedCapability(
                    "blur required memory preflight",
                ))
            };
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight);
            return super::adaptive::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
                &mut effects,
            );
        }

        if matches!(
            self.kind(),
            PointwiseKind::Equalize
                | PointwiseKind::GaussianBlur
                | PointwiseKind::Sharpen
                | PointwiseKind::MedianBlur
                | PointwiseKind::MotionBlur
                | PointwiseKind::ShapeBlur
        ) {
            let mut capacity = |_| {
                Err(PointwiseError::UnsupportedCapability(
                    "Combine ambient capacity predicate",
                ))
            };
            let mut required = |_, _| {
                Err(PointwiseError::UnsupportedCapability(
                    "blur required memory preflight",
                ))
            };
            let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut required);
            return (if self.kind() == PointwiseKind::Equalize {
                super::equalize::retained
            } else if self.kind() == PointwiseKind::ShapeBlur {
                super::shape::retained
            } else if self.kind() == PointwiseKind::MotionBlur {
                super::motion::retained
            } else if self.kind() == PointwiseKind::MedianBlur {
                super::median::retained
            } else {
                super::blur::retained
            })(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                &mut effects,
            );
        }
        if super::transform::owns(self.kind()) {
            return super::transform::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                options,
            );
        }
        if self.kind() == PointwiseKind::MatchColor {
            return super::match_color::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                &|| false,
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::Exposure | PointwiseKind::Selector
        ) {
            return super::exposure_selector::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                &|| false,
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::Frequency | PointwiseKind::Fractalize
        ) {
            return super::frequency_fractalize::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
                &|| false,
            );
        }
        if self.kind() == PointwiseKind::Rotate {
            return super::rotate::retained(
                self,
                inputs,
                ImageGeometry::new(width, height),
                time,
                execution,
            );
        }
        if matches!(
            self.kind(),
            PointwiseKind::Pixelate | PointwiseKind::Matrix | PointwiseKind::WizardMatrix
        ) {
            self.render_spatial_at_time(inputs, width, height, time, execution)
        } else {
            self.render_supplied_with_options(inputs, width, height, options)
        }
    }
    // Keep source geometry, sample time, execution and capacity policy explicit.
    #[allow(clippy::too_many_arguments)]
    pub fn render_supplied_at_time_with_capacity_query(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
        time: f64,
        execution: crate::ProceduralExecution,
        options: super::PointwiseRenderOptions,
        query: &mut impl FnMut(i32) -> Result<bool, PointwiseError<E>>,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        if self.kind() == PointwiseKind::Combine {
            super::combine::render_with_capacity_query(
                self,
                inputs,
                ImageGeometry::new(width, height),
                options,
                Some(time),
                query,
                &|| false,
            )
        } else {
            self.render_supplied_at_time(inputs, width, height, time, execution, options)
        }
    }
    pub(super) fn render_spatial_at_time(
        &self,
        inputs: &[Option<SharedImageMap>],
        width: i32,
        height: i32,
        time: f64,
        execution: crate::ProceduralExecution,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        let geometry = ImageGeometry::new(width, height);
        if self.kind() == PointwiseKind::WizardMatrix {
            let iterations =
                crate::framework_cast::integer(self.parameter(1)?.effective_value_at(time)?);
            let input = inputs.first().ok_or(PointwiseError::Index)?;
            return super::spatial::retained_wizard_matrix(
                self,
                input.as_ref(),
                geometry,
                iterations,
                time,
                execution,
            );
        }
        if self.kind() == PointwiseKind::Matrix {
            let iterations =
                crate::framework_cast::integer(self.parameter(1)?.effective_value_at(time)?);
            let input = inputs.first().ok_or(PointwiseError::Index)?;
            return super::spatial::retained_matrix(
                self,
                input.as_ref(),
                geometry,
                iterations,
                execution,
            );
        }
        let input = inputs.first().ok_or(PointwiseError::Index)?;
        super::spatial::retained_pixelate(self, input.as_ref(), geometry, time, execution).map(Some)
    }
}
