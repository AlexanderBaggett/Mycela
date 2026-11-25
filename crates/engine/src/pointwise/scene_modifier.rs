//! Retained scene modifiers and direct Original scene evaluation. The timed
//! graph walker prepares scene slots and image materials before calling them.
use super::{PointwiseError, PointwiseNode, RenderCaptureSession};
use crate::{
    ModernNodeProperty, ModernNodeValue, Scene, SceneAllocationEffects, SceneRotationOrder,
    SceneTransformOrigin, SceneTransformParameters, SceneVector,
};
use std::fmt;
use std::sync::Arc;

pub(super) struct TransformState<E> {
    properties: Vec<Arc<super::scene_primitive::property::PropertyRoot<E>>>,
}
impl<E: 'static> TransformState<E> {
    fn new() -> Result<Arc<Self>, PointwiseError<E>> {
        use super::scene_primitive::{make_selected_property, make_transform_vector_property};
        Ok(Arc::new(Self {
            properties: vec![
                make_transform_vector_property("Scale", [1.; 3], false)?,
                make_transform_vector_property("Move", [0.; 3], false)?,
                make_transform_vector_property("Rotate", [0.; 3], true)?,
                make_selected_property("Origin", &["World", "All Models", "Each Model"], 1)?,
                make_selected_property("Rotate Order", &["Beginning", "End"], 1)?,
            ],
        }))
    }
    pub(super) fn bind(&self, node: &Arc<super::graph::NodeCore<E>>) {
        for property in &self.properties {
            property.bind(node);
        }
    }
    fn copied(
        &self,
        source_copy: bool,
        mut session: Option<&mut RenderCaptureSession<E>>,
    ) -> Result<Arc<Self>, PointwiseError<E>> {
        let properties = self
            .properties
            .iter()
            .map(|p| p.copied(source_copy, session.as_deref_mut()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Arc::new(Self { properties }))
    }
}

pub struct SceneTransformNode<E = ()> {
    node: PointwiseNode<E>,
    state: Arc<TransformState<E>>,
}
impl<E> Clone for SceneTransformNode<E> {
    fn clone(&self) -> Self {
        Self {
            node: self.node.clone(),
            state: self.state.clone(),
        }
    }
}
impl<E> fmt::Debug for SceneTransformNode<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SceneTransformNode({:?})", self.node)
    }
}
impl<E: 'static> SceneTransformNode<E> {
    pub fn new() -> Result<Self, PointwiseError<E>> {
        let state = TransformState::new()?;
        let node = PointwiseNode::publish_scene_transform(state.clone());
        Ok(Self { node, state })
    }
    pub fn node(&self) -> PointwiseNode<E> {
        self.node.clone()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.node.same_reference(&other.node)
    }
    pub fn properties(&self) -> Vec<ModernNodeProperty<E>> {
        self.state
            .properties
            .iter()
            .map(|p| ModernNodeProperty::from_root(self.node.clone(), p.clone()))
            .collect()
    }
    pub fn property(&self, index: i32) -> Result<ModernNodeProperty<E>, PointwiseError<E>> {
        self.state
            .properties
            .get(usize::try_from(index).map_err(|_| PointwiseError::Index)?)
            .cloned()
            .map(|p| ModernNodeProperty::from_root(self.node.clone(), p))
            .ok_or(PointwiseError::Index)
    }
    pub fn source_clone(&self) -> Result<Self, PointwiseError<E>> {
        let state = self.state.copied(true, None)?;
        let node = PointwiseNode::publish_scene_transform(state.clone());
        Ok(Self { node, state })
    }
    pub(super) fn capture_with(
        &self,
        session: &mut RenderCaptureSession<E>,
    ) -> Result<Self, PointwiseError<E>> {
        let state = self.state.copied(false, Some(session))?;
        let node = PointwiseNode::publish_scene_transform(state.clone());
        Ok(Self { node, state })
    }
    pub(super) fn raw_capture_values(
        &self,
        comparison: &mut super::property::capture::capture_values::Comparison<E>,
    ) -> Result<Vec<u64>, PointwiseError<E>> {
        super::scene_primitive::capture_values::properties(self.properties(), comparison)
    }
    pub(super) fn apply(&self, scene: &Scene, time: f64) -> Result<(), PointwiseError<E>> {
        let vector = |i| -> Result<SceneVector, PointwiseError<E>> {
            let ModernNodeValue::Vector3D(v) = self.property(i)?.evaluated_value(time)? else {
                return Err(PointwiseError::InvalidCast);
            };
            Ok(SceneVector::new(v[0], v[1], v[2]))
        };
        let scale = vector(0)?;
        let movement = vector(1)?;
        let rotation = vector(2)?;
        let ModernNodeValue::Int32(origin) = self.property(3)?.evaluated_value(time)? else {
            return Err(PointwiseError::InvalidCast);
        };
        let ModernNodeValue::Int32(order) = self.property(4)?.evaluated_value(time)? else {
            return Err(PointwiseError::InvalidCast);
        };
        let origin = if origin == 2 {
            SceneTransformOrigin::EachModel
        } else if origin == 0 {
            SceneTransformOrigin::World
        } else {
            SceneTransformOrigin::AllModels
        };
        let order = if order == 0 {
            SceneRotationOrder::Beginning
        } else {
            SceneRotationOrder::End
        };
        scene
            .apply_transform_3d(SceneTransformParameters::new(
                scale, movement, rotation, origin, order,
            ))
            .map_err(PointwiseError::Scene)
    }
}

/// Combine 3D is a retained two-slot scene node. Both scene children are
/// evaluated and source-copied in slot order before the first is combined.
pub struct SceneCombineNode<E = ()> {
    node: PointwiseNode<E>,
}
impl<E> Clone for SceneCombineNode<E> {
    fn clone(&self) -> Self {
        Self {
            node: self.node.clone(),
        }
    }
}
impl<E> fmt::Debug for SceneCombineNode<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SceneCombineNode({:?})", self.node)
    }
}
impl<E: 'static> SceneCombineNode<E> {
    pub fn new() -> Self {
        Self {
            node: PointwiseNode::publish_scene_combine(),
        }
    }
    pub fn node(&self) -> PointwiseNode<E> {
        self.node.clone()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.node.same_reference(&other.node)
    }
}
impl<E: 'static> Default for SceneCombineNode<E> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SceneClearLightsNode<E = ()> {
    node: PointwiseNode<E>,
}
impl<E> Clone for SceneClearLightsNode<E> {
    fn clone(&self) -> Self {
        Self {
            node: self.node.clone(),
        }
    }
}
impl<E> fmt::Debug for SceneClearLightsNode<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SceneClearLightsNode({:?})", self.node)
    }
}
impl<E: 'static> SceneClearLightsNode<E> {
    pub fn new() -> Self {
        Self {
            node: PointwiseNode::publish_scene_clear_lights(),
        }
    }
    pub fn node(&self) -> PointwiseNode<E> {
        self.node.clone()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.node.same_reference(&other.node)
    }
}
impl<E: 'static> Default for SceneClearLightsNode<E> {
    fn default() -> Self {
        Self::new()
    }
}
impl<E: 'static> PointwiseNode<E> {
    pub fn scene_transform_3d(&self) -> Option<SceneTransformNode<E>> {
        match &self.core().payload {
            super::graph::Payload::SceneTransform { state } => Some(SceneTransformNode {
                node: self.clone(),
                state: state.clone(),
            }),
            _ => None,
        }
    }
    pub fn scene_combine_3d(&self) -> Option<SceneCombineNode<E>> {
        matches!(&self.core().payload, super::graph::Payload::SceneCombine)
            .then(|| SceneCombineNode { node: self.clone() })
    }
    pub fn scene_clear_lights(&self) -> Option<SceneClearLightsNode<E>> {
        matches!(
            &self.core().payload,
            super::graph::Payload::SceneClearLights
        )
        .then(|| SceneClearLightsNode { node: self.clone() })
    }

    // hj.ku and ark.ku forward the scene classification of their actual
    // first contents or stored-slot target. Detect cycles before following it.
    pub(super) fn scene_forward_target(&self) -> Result<Option<Self>, PointwiseError<E>> {
        let first = match self.kind() {
            Some(kind) if kind.is_group() => self.content(0)?.node(),
            Some(super::PointwiseKind::Reference) => self.reference_target()?,
            _ => return Ok(None),
        };
        let mut current = first.clone();
        let mut seen = std::collections::BTreeSet::new();
        seen.insert(self.id);
        while let Some(node) = current {
            if !seen.insert(node.id) {
                return Err(PointwiseError::UnsupportedCapability(
                    "cyclic scene graph scheduling",
                ));
            }
            if node.is_scene_node() {
                // z8.ku delegates to k3. The supported scene types return
                // false from k3 independently of zl.IsFilter.
                return Ok(first);
            }
            current = match node.kind() {
                Some(kind) if kind.is_group() => node.content(0)?.node(),
                Some(super::PointwiseKind::Reference) => node.reference_target()?,
                _ => return Ok(None),
            };
        }
        Ok(None)
    }

    pub(super) fn render_scene_optional(
        &self,
        time: f64,
        effects: &mut SceneAllocationEffects<'_>,
    ) -> Result<Option<Scene>, PointwiseError<E>> {
        fn evaluate<E: 'static>(
            node: &PointwiseNode<E>,
            time: f64,
            effects: &mut SceneAllocationEffects<'_>,
            active: &mut Vec<PointwiseNode<E>>,
        ) -> Result<Option<Scene>, PointwiseError<E>> {
            if active.iter().any(|item| item.same_reference(node)) {
                return Err(PointwiseError::UnsupportedCapability(
                    "cyclic scene graph scheduling",
                ));
            }
            if let Some(target) = node.scene_forward_target()? {
                match node.kind() {
                    Some(kind) if kind.is_group() => super::graph::validate_group_render(node)?,
                    Some(super::PointwiseKind::Reference) => {
                        super::graph::validate_reference_render(node)?
                    }
                    _ => unreachable!(),
                }
                active.push(node.clone());
                let result = evaluate(&target, time, effects, active);
                active.pop();
                return result;
            }
            if !node.contents().is_empty() {
                return Err(PointwiseError::UnsupportedCapability(
                    "Contents scene evaluation context",
                ));
            }
            if !node.is_scene_node() && node.is_filter() {
                return Err(PointwiseError::UnsupportedCapability(
                    "filter-mode scene evaluation context",
                ));
            }
            active.push(node.clone());
            let result = if let Some(primitive) = node.scene_primitive() {
                primitive.render_scene(time, effects).map(Some)
            } else if node.is_scene_generator() {
                node.render_imported_scene(time, effects)
            } else if node.scene_combine_3d().is_some() {
                // aws.k5 invokes g(slot 0) then g(slot 1), each of which
                // source-copies the returned child before the next slot read.
                let first = match node.slot(0)?.node() {
                    None => None,
                    Some(child) => evaluate(&child, time, effects, active)?
                        .map(|scene| scene.source_copy_with_effects(effects))
                        .transpose()
                        .map_err(PointwiseError::Scene)?,
                };
                let second = match node.slot(1)?.node() {
                    None => None,
                    Some(child) => evaluate(&child, time, effects, active)?
                        .map(|scene| scene.source_copy_with_effects(effects))
                        .transpose()
                        .map_err(PointwiseError::Scene)?,
                };
                crate::combine_3d(first, second).map_err(PointwiseError::Scene)
            } else if node.scene_clear_lights().is_some() || node.scene_transform_3d().is_some() {
                match node.slot(0)?.node() {
                    None => Ok(None),
                    Some(child) => {
                        let scene = evaluate(&child, time, effects, active)?;
                        match scene {
                            None => Ok(None),
                            Some(scene) => {
                                // aqr.g copies a returned child scene before its
                                // parent effect; never mutate the child's roots.
                                let copy = scene
                                    .source_copy_with_effects(effects)
                                    .map_err(PointwiseError::Scene)?;
                                if let Some(transform) = node.scene_transform_3d() {
                                    transform.apply(&copy, time)?;
                                } else {
                                    copy.clear_lights().map_err(PointwiseError::Scene)?;
                                }
                                Ok(Some(copy))
                            }
                        }
                    }
                }
            } else {
                Err(PointwiseError::UnsupportedCapability(
                    "image input as scene fallback",
                ))
            };
            active.pop();
            result
        }
        evaluate(self, time, effects, &mut Vec::new())
    }

    /// Evaluate the actual scene-valued graph. A missing scene is represented by
    /// NullReference here; image rendering retains the source's empty-image path.
    pub fn render_scene(
        &self,
        time: f64,
        effects: &mut SceneAllocationEffects<'_>,
    ) -> Result<Scene, PointwiseError<E>> {
        self.render_scene_optional(time, effects)?
            .ok_or(PointwiseError::NullReference)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BinaryWriter, Gtx375GraphDocument, RenderCaptureSession, SceneAllocationRequest,
        ScenePrimitiveKind, ScenePrimitiveNode,
    };

    #[test]
    fn image_group_and_reference_become_scene_material_with_frozen_capture() {
        check_group_image_material(crate::PointwiseKind::Group);
    }

    #[test]
    fn wizard_image_and_reference_become_scene_material_with_frozen_capture() {
        check_group_image_material(crate::PointwiseKind::WizardGroup);
    }

    fn check_group_image_material(kind: crate::PointwiseKind) {
        struct Inspect {
            expected: f32,
            calls: usize,
        }
        impl crate::SceneImageRenderer<()> for Inspect {
            fn preflight_array(
                &mut self,
                _: SceneAllocationRequest,
            ) -> Result<(), crate::SceneError> {
                Ok(())
            }
            fn material_texture_limit(&mut self) -> Result<u32, PointwiseError<()>> {
                Ok(4096)
            }
            fn render_scene_image(
                &mut self,
                scene: &Scene,
                _: i32,
                _: i32,
            ) -> Result<crate::SharedImageMap, PointwiseError<()>> {
                self.calls += 1;
                let map = scene.models()[0].material().map().unwrap();
                assert!(
                    (map.channel(0, false).unwrap().read(0).unwrap() - self.expected).abs() < 1e-6
                );
                Err(PointwiseError::UnsupportedCapability("inspection backend"))
            }
        }
        let source = PointwiseNode::<()>::solid_color().unwrap();
        source.set_solid_color([0.2, 0.4, 0.6]).unwrap();
        let group = crate::PointwiseFilter::<()>::new(kind).unwrap();
        group.parameter(0).unwrap().set_number(1.).unwrap();
        group
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&source))
            .unwrap();
        let reference = crate::PointwiseFilter::<()>::new(crate::PointwiseKind::Reference).unwrap();
        let invert = PointwiseNode::<()>::invert();
        invert
            .slot(0)
            .unwrap()
            .set_node(Some(&reference.node()))
            .unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&invert))
            .unwrap();
        let clear = SceneClearLightsNode::<()>::new();
        clear
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&group.node()))
            .unwrap();
        let capture = RenderCaptureSession::new()
            .capture_node(&clear.node(), 0.)
            .unwrap();
        source.set_solid_color([0.7, 0.4, 0.6]).unwrap();
        let mut backend = Inspect {
            expected: 0.3,
            calls: 0,
        };
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_scene_renderer(&mut backend);
        let render = clear.node().render_original_with_effects(
            2,
            2,
            crate::PointwiseRenderOptions::new(false, false),
            0.,
            crate::ProceduralExecution::CallingThread,
            &mut effects,
        );
        assert!(matches!(
            render,
            Err(PointwiseError::UnsupportedCapability("inspection backend"))
        ));
        assert_eq!(backend.calls, 1);
        backend.expected = 0.8;
        let mut effects = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_scene_renderer(&mut backend);
        let frozen = capture.render_with_execution_and_effects(
            2,
            2,
            crate::ProceduralExecution::CallingThread,
            crate::PointwiseRenderOptions::new(false, false),
            &mut effects,
        );
        assert!(matches!(
            frozen,
            Err(PointwiseError::UnsupportedCapability("inspection backend"))
        ));
        assert_eq!(backend.calls, 2);
        group.parameter(1).unwrap().set_number(2.).unwrap();
        backend.expected = 0.3;
        let mut effects = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_scene_renderer(&mut backend);
        let repeated_value = clear.node().render_original_with_effects(
            2,
            2,
            crate::PointwiseRenderOptions::new(false, false),
            0.,
            crate::ProceduralExecution::CallingThread,
            &mut effects,
        );
        assert!(matches!(
            repeated_value,
            Err(PointwiseError::UnsupportedCapability("inspection backend"))
        ));
        assert_eq!(backend.calls, 3);
    }

    #[test]
    fn group_forwards_mixed_scene_with_real_image_material_to_backend() {
        struct Inspect {
            called: bool,
        }
        impl crate::SceneImageRenderer<()> for Inspect {
            fn preflight_array(
                &mut self,
                _: SceneAllocationRequest,
            ) -> Result<(), crate::SceneError> {
                Ok(())
            }
            fn material_texture_limit(&mut self) -> Result<u32, PointwiseError<()>> {
                Ok(4096)
            }
            fn render_scene_image(
                &mut self,
                scene: &Scene,
                _: i32,
                _: i32,
            ) -> Result<crate::SharedImageMap, PointwiseError<()>> {
                self.called = true;
                let map = scene.models()[0].material().map().unwrap();
                assert!((map.channel(0, false).unwrap().read(0).unwrap() - 0.25).abs() < 1e-6);
                Err(PointwiseError::UnsupportedCapability("inspection backend"))
            }
        }
        let image = PointwiseNode::<()>::solid_color().unwrap();
        image.set_solid_color([0.25, 0.5, 0.75]).unwrap();
        let inner = SceneClearLightsNode::<()>::new();
        inner
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&image))
            .unwrap();
        let group = crate::PointwiseFilter::<()>::new(crate::PointwiseKind::Group).unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&inner.node()))
            .unwrap();
        let outer = SceneClearLightsNode::<()>::new();
        outer
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&group.node()))
            .unwrap();
        // A warm screen raster belongs to the image path. The Model request
        // still needs the actual child geometry and its material.
        let raster = crate::SharedImageMap::new(crate::ImageGeometry::new(2, 2));
        raster.set_constant(0, 0.9).unwrap();
        inner.node().set_cached_image(Some(raster.clone())).unwrap();
        group.node().set_cached_image(Some(raster)).unwrap();
        let mut backend = Inspect { called: false };
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_scene_renderer(&mut backend);
        let result = outer.node().render_original_with_effects(
            2,
            2,
            crate::PointwiseRenderOptions::new(false, false),
            0.,
            crate::ProceduralExecution::CallingThread,
            &mut effects,
        );
        assert!(matches!(
            result,
            Err(PointwiseError::UnsupportedCapability("inspection backend"))
        ));
        assert!(backend.called);
    }

    #[test]
    fn map_variable_forwards_referenced_scene_and_capture() {
        let map = crate::PointwiseFilter::<()>::new(crate::PointwiseKind::MapVariable).unwrap();
        map.parameter(0).unwrap().set_number(1.).unwrap();
        let cube = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        map.node()
            .slot(0)
            .unwrap()
            .set_node(Some(&cube.node()))
            .unwrap();
        let reference = crate::PointwiseFilter::<()>::new(crate::PointwiseKind::Reference).unwrap();
        map.node()
            .content(0)
            .unwrap()
            .set_node(Some(&reference.node()))
            .unwrap();
        let clear = SceneClearLightsNode::<()>::new();
        clear
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&map.node()))
            .unwrap();
        let snapshot = RenderCaptureSession::new()
            .capture_node(&clear.node(), 0.)
            .unwrap();
        cube.property(0).unwrap().set_vector3d([2.; 3]).unwrap();
        let mut admit = |_: SceneAllocationRequest| Ok(());
        let captured = snapshot
            .render_scene_with_effects(&mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        assert!(
            captured.models()[0]
                .try_positions()
                .unwrap()
                .iter()
                .any(|p| p.values() == [0.5; 3])
        );
        let live = clear
            .node()
            .render_scene(0., &mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        assert!(
            live.models()[0]
                .try_positions()
                .unwrap()
                .iter()
                .any(|p| p.values() == [1.; 3])
        );
    }

    #[test]
    fn group_reference_forwards_scene_through_modifier_and_capture() {
        check_group_scene_forwarding(crate::PointwiseKind::Group);
    }

    #[test]
    fn wizard_reference_forwards_scene_through_modifier_and_capture() {
        check_group_scene_forwarding(crate::PointwiseKind::WizardGroup);
    }

    fn check_group_scene_forwarding(kind: crate::PointwiseKind) {
        let group = crate::PointwiseFilter::<()>::new(kind).unwrap();
        group.parameter(0).unwrap().set_number(1.).unwrap();
        let reference = crate::PointwiseFilter::<()>::new(crate::PointwiseKind::Reference).unwrap();
        let cube = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        cube.node().convert_to_filter(false).unwrap();
        group
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&cube.node()))
            .unwrap();
        group
            .node()
            .content(0)
            .unwrap()
            .set_node(Some(&reference.node()))
            .unwrap();
        let clear = SceneClearLightsNode::<()>::new();
        clear
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&group.node()))
            .unwrap();
        clear.node().convert_to_filter(false).unwrap();
        let captured = RenderCaptureSession::new()
            .capture_node(&clear.node(), 0.)
            .unwrap();
        cube.property(0).unwrap().set_vector3d([2.; 3]).unwrap();
        let mut admit = |_: SceneAllocationRequest| Ok(());
        let frozen = captured
            .render_scene_with_effects(&mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        assert!(
            frozen.models()[0]
                .try_positions()
                .unwrap()
                .iter()
                .any(|p| p.values() == [0.5; 3])
        );
        group
            .node()
            .set_cached_image(Some(crate::SharedImageMap::new(crate::ImageGeometry::new(
                2, 2,
            ))))
            .unwrap();
        let live = clear
            .node()
            .render_scene(0., &mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        assert!(
            live.models()[0]
                .try_positions()
                .unwrap()
                .iter()
                .any(|p| p.values() == [1.; 3])
        );
        assert_eq!(live.explicit_light_count(), 1);
        group.parameter(1).unwrap().set_number(2.).unwrap();
        let scene = clear
            .node()
            .render_scene(0., &mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        assert_eq!(scene.explicit_light_count(), 1);
    }

    #[test]
    fn clear_lights_scene_input_is_copied_and_capture_detaches_child() {
        let primitive = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        let clear = SceneClearLightsNode::<()>::new();
        clear
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&primitive.node()))
            .unwrap();
        let captured = RenderCaptureSession::new()
            .capture_node(&clear.node(), 0.)
            .unwrap();
        primitive
            .property(0)
            .unwrap()
            .set_vector3d([2.; 3])
            .unwrap();
        let mut admit = |_: SceneAllocationRequest| Ok(());
        let result = captured
            .render_scene_with_effects(&mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        assert_eq!(result.explicit_light_count(), 1);
        assert_eq!(result.lights()[0].values().ambient(), [1.; 3]);
        assert!(
            result.models()[0]
                .try_positions()
                .unwrap()
                .iter()
                .any(|p| p.values() == [0.5; 3])
        );
        let child = primitive
            .render_scene(0., &mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        assert_eq!(child.explicit_light_count(), 0);
        assert!(
            child.models()[0]
                .try_positions()
                .unwrap()
                .iter()
                .any(|p| p.values() == [1.; 3])
        );
    }

    #[test]
    fn clear_lights_factory_wire_and_source_copy_retain_model_occurrence() {
        let root = crate::create_node_by_identity::<()>(Some(
            &"Clear Lights".encode_utf16().collect::<Vec<_>>(),
        ))
        .unwrap();
        assert!(root.scene_clear_lights().is_some());
        let primitive = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        root.slot(0)
            .unwrap()
            .set_node(Some(&primitive.node()))
            .unwrap();
        let copy = root.source_clone().unwrap();
        assert!(!root.same_reference(&copy));
        assert!(
            !copy
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&primitive.node())
        );
        let mut document = Gtx375GraphDocument::new(vec![Some(root)]).unwrap();
        let mut writer = BinaryWriter::default();
        document.save(&mut writer).unwrap();
        let mut loaded = Gtx375GraphDocument::<()>::load(writer.bytes()).unwrap();
        let mut roundtrip = BinaryWriter::default();
        loaded.save(&mut roundtrip).unwrap();
        assert_eq!(writer.bytes(), roundtrip.bytes());
        assert!(
            loaded.branches()[0]
                .as_ref()
                .unwrap()
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .scene_primitive()
                .is_some()
        );
    }

    #[test]
    fn absent_and_unsupported_inputs_remain_distinct() {
        let root = SceneClearLightsNode::<()>::new().node();
        let mut admit = |_: SceneAllocationRequest| Ok(());
        let mut effects = SceneAllocationEffects::new(&mut admit);
        assert!(
            root.render_scene_optional(0., &mut effects)
                .unwrap()
                .is_none()
        );
        let image = PointwiseNode::<()>::solid_color().unwrap();
        root.slot(0).unwrap().set_node(Some(&image)).unwrap();
        assert!(matches!(
            root.render_scene_optional(0., &mut effects),
            Err(PointwiseError::UnsupportedCapability(_))
        ));
    }

    #[test]
    fn absent_scene_does_not_require_a_renderer_but_geometry_does() {
        let root = SceneClearLightsNode::<()>::new().node();
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        assert!(
            effects
                .render_scene_node(
                    &root,
                    4,
                    3,
                    0.,
                    crate::ProceduralExecution::CallingThread,
                    super::super::PointwiseRenderOptions::new(false, false)
                )
                .is_ok()
        );
        let primitive = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        root.slot(0)
            .unwrap()
            .set_node(Some(&primitive.node()))
            .unwrap();
        assert!(matches!(
            effects.render_scene_node(
                &root,
                4,
                3,
                0.,
                crate::ProceduralExecution::CallingThread,
                super::super::PointwiseRenderOptions::new(false, false)
            ),
            Err(PointwiseError::Scene(crate::SceneError::MissingRenderer))
        ));
    }

    #[test]
    fn transform_graph_uses_retained_properties_after_scene_copy_callbacks() {
        let primitive = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        let transform = SceneTransformNode::<()>::new().unwrap();
        transform
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&primitive.node()))
            .unwrap();
        assert_eq!(
            transform
                .property(3)
                .unwrap()
                .selected_options()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            transform
                .property(4)
                .unwrap()
                .selected_options()
                .unwrap()
                .len(),
            2
        );
        let movement = transform.property(1).unwrap();
        let mut calls = 0;
        let mut admit = |_: SceneAllocationRequest| {
            calls += 1;
            if calls == 1 {
                movement.set_vector3d([2., 0., 0.]).unwrap();
            }
            Ok(())
        };
        let scene = transform
            .node()
            .render_scene(0., &mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        assert!(
            scene.models()[0]
                .try_positions()
                .unwrap()
                .iter()
                .any(|p| p.values() == [2.5, 0.5, 0.5])
        );
        assert_eq!(
            primitive.property(1).unwrap().value(),
            ModernNodeValue::Vector3D([0.; 3])
        );
    }

    #[test]
    fn transform_factory_wire_capture_and_copy_preserve_selected_and_vector_roots() {
        let root = crate::create_node_by_identity::<()>(Some(
            &"Transform 3D".encode_utf16().collect::<Vec<_>>(),
        ))
        .unwrap();
        let transform = root.scene_transform_3d().unwrap();
        transform
            .property(0)
            .unwrap()
            .set_vector3d([2., 1., 1.])
            .unwrap();
        transform.property(3).unwrap().set_int32(2).unwrap();
        let primitive = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        root.slot(0)
            .unwrap()
            .set_node(Some(&primitive.node()))
            .unwrap();
        let frame = RenderCaptureSession::new().capture_node(&root, 0.).unwrap();
        let copy = root.source_clone().unwrap().scene_transform_3d().unwrap();
        assert!(
            !copy
                .property(3)
                .unwrap()
                .same_reference(&transform.property(3).unwrap())
        );
        assert_eq!(
            copy.property(3).unwrap().selected_options(),
            transform.property(3).unwrap().selected_options()
        );
        let mut document = Gtx375GraphDocument::new(vec![Some(root)]).unwrap();
        let mut writer = BinaryWriter::default();
        document.save(&mut writer).unwrap();
        let mut loaded = Gtx375GraphDocument::<()>::load(writer.bytes()).unwrap();
        let mut again = BinaryWriter::default();
        loaded.save(&mut again).unwrap();
        assert_eq!(writer.bytes(), again.bytes());
        transform
            .property(0)
            .unwrap()
            .set_vector3d([3., 1., 1.])
            .unwrap();
        let mut admit = |_: SceneAllocationRequest| Ok(());
        let scene = frame
            .render_scene_with_effects(&mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        assert!(
            scene.models()[0]
                .try_positions()
                .unwrap()
                .iter()
                .any(|p| p.values() == [1., 0.5, 0.5])
        );
    }

    #[test]
    fn combine_graph_preserves_slot_order_occurrences_and_empty_inputs() {
        let combine = SceneCombineNode::<()>::new();
        let root = combine.node();
        assert_eq!(root.slots().len(), 2);
        assert_eq!(
            String::from_utf16_lossy(&root.slot(0).unwrap().name()),
            "Model A"
        );
        assert_eq!(
            String::from_utf16_lossy(&root.slot(1).unwrap().name()),
            "Model B"
        );
        let mut admit = |_: SceneAllocationRequest| Ok(());
        assert!(
            root.render_scene_optional(0., &mut SceneAllocationEffects::new(&mut admit))
                .unwrap()
                .is_none()
        );
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut no_renderer = crate::PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        assert!(
            no_renderer
                .render_scene_node(
                    &root,
                    4,
                    3,
                    0.,
                    crate::ProceduralExecution::CallingThread,
                    super::super::PointwiseRenderOptions::new(false, false)
                )
                .is_ok()
        );
        let cube = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        root.slot(1).unwrap().set_node(Some(&cube.node())).unwrap();
        let only_second = root
            .render_scene(0., &mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        assert_eq!(only_second.models().len(), 1);
        root.slot(0).unwrap().set_node(Some(&cube.node())).unwrap();
        let both = root
            .render_scene(0., &mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        let models = both.models();
        assert_eq!(models.len(), 2);
        assert!(
            !models[0].same_reference(&models[1]),
            "each slot is source-copied independently"
        );
        assert_eq!(
            models[0].try_positions().unwrap(),
            models[1].try_positions().unwrap()
        );
        root.slot(1).unwrap().set_node(None).unwrap();
        let only_first = root
            .render_scene(0., &mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        assert_eq!(only_first.models().len(), 1);
    }

    #[test]
    fn combine_reads_second_slot_after_first_child_and_copy_work() {
        let root = SceneCombineNode::<()>::new().node();
        let first = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        let old = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        let replacement = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Sphere).unwrap();
        root.slot(0).unwrap().set_node(Some(&first.node())).unwrap();
        root.slot(1).unwrap().set_node(Some(&old.node())).unwrap();
        let slot = root.slot(1).unwrap();
        let mut changed = false;
        let mut admit = |_: SceneAllocationRequest| {
            if !changed {
                changed = true;
                slot.set_node(Some(&replacement.node())).unwrap();
            }
            Ok(())
        };
        let scene = root
            .render_scene(0., &mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        let models = scene.models();
        assert_eq!(models.len(), 2);
        assert_ne!(
            models[0].try_positions().unwrap().len(),
            models[1].try_positions().unwrap().len()
        );
        assert!(
            root.slot(1)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&replacement.node())
        );
    }

    #[test]
    fn combine_factory_wire_and_capture_keep_both_occurrences() {
        let root = crate::create_node_by_identity::<()>(Some(
            &"Combine 3D".encode_utf16().collect::<Vec<_>>(),
        ))
        .unwrap();
        assert!(root.scene_combine_3d().is_some());
        let cube = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Cube).unwrap();
        root.slot(0).unwrap().set_node(Some(&cube.node())).unwrap();
        root.slot(1).unwrap().set_node(Some(&cube.node())).unwrap();
        let capture = RenderCaptureSession::new().capture_node(&root, 0.).unwrap();
        let source_copy = root.source_clone().unwrap();
        assert!(
            !source_copy
                .slot(0)
                .unwrap()
                .node()
                .unwrap()
                .same_reference(&source_copy.slot(1).unwrap().node().unwrap())
        );
        let mut document = Gtx375GraphDocument::new(vec![Some(root.clone())]).unwrap();
        let mut writer = BinaryWriter::default();
        document.save(&mut writer).unwrap();
        let mut loaded = Gtx375GraphDocument::<()>::load(writer.bytes()).unwrap();
        let mut again = BinaryWriter::default();
        loaded.save(&mut again).unwrap();
        assert_eq!(writer.bytes(), again.bytes());
        let loaded = loaded.branches()[0].clone().unwrap();
        assert!(loaded.scene_combine_3d().is_some());
        assert!(loaded.slot(0).unwrap().node().is_some());
        assert!(loaded.slot(1).unwrap().node().is_some());
        root.slot(1).unwrap().set_node(None).unwrap();
        let mut admit = |_: SceneAllocationRequest| Ok(());
        let scene = capture
            .render_scene_with_effects(&mut SceneAllocationEffects::new(&mut admit))
            .unwrap();
        assert_eq!(scene.models().len(), 2);
    }

    #[test]
    fn combine_rejects_scene_cycles_and_image_fallback() {
        let root = SceneCombineNode::<()>::new().node();
        assert!(matches!(
            root.slot(0).unwrap().set_node(Some(&root)),
            Err(PointwiseError::UnsupportedCapability(_))
        ));
        assert!(root.slot(0).unwrap().node().unwrap().same_reference(&root));
        let mut admit = |_: SceneAllocationRequest| Ok(());
        let fresh = SceneCombineNode::<()>::new().node();
        fresh
            .slot(0)
            .unwrap()
            .set_node(Some(&PointwiseNode::<()>::solid_color().unwrap()))
            .unwrap();
        assert!(matches!(
            fresh.render_scene_optional(0., &mut SceneAllocationEffects::new(&mut admit)),
            Err(PointwiseError::UnsupportedCapability(_))
        ));
    }
}
