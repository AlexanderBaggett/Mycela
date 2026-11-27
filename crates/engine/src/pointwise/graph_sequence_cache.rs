//! Stationary outputs of one frozen animation graph. A node is stationary
//! only if its local controls and every dependency are stationary.
use super::{PointwiseKind, PointwiseNode};
use crate::{ImageGeometry, Scene, SharedImageMap, pointwise::region::NodeId};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Default)]
pub(in crate::pointwise) struct SequenceOutputCache {
    stationary: BTreeMap<NodeId, bool>,
    images: BTreeMap<NodeId, (NodeId, SharedImageMap)>,
    scenes: BTreeMap<NodeId, Option<Scene>>,
}

pub(super) struct SequenceFrameImages {
    input_owners: HashMap<usize, NodeId>,
    save: crate::image_map::ImageCapture,
    load: crate::image_map::ImageCapture,
}

impl SequenceFrameImages {
    pub(super) fn new() -> Self {
        Self {
            input_owners: HashMap::new(),
            save: crate::image_map::ImageCapture::sharing(),
            load: crate::image_map::ImageCapture::sharing(),
        }
    }
}

// The original evaluator has no FinalGraphExecution, but it can share the
// same stationary image results without changing its graph scheduling.
pub(in crate::pointwise) struct OriginalSequenceFrame<'a> {
    cache: &'a mut SequenceOutputCache,
    images: BTreeMap<NodeId, SharedImageMap>,
    frame: SequenceFrameImages,
}

impl<'a> OriginalSequenceFrame<'a> {
    pub(in crate::pointwise) fn new(cache: &'a mut SequenceOutputCache) -> Self {
        Self {
            cache,
            images: BTreeMap::new(),
            frame: SequenceFrameImages::new(),
        }
    }

    pub(in crate::pointwise) fn image(
        &mut self,
        id: NodeId,
        geometry: ImageGeometry,
    ) -> Option<SharedImageMap> {
        self.images
            .get(&id)
            .filter(|image| image.geometry() == geometry)
            .cloned()
            .or_else(|| {
                self.cache
                    .image(id, geometry, &mut self.images, &mut self.frame)
            })
    }

    pub(in crate::pointwise) fn record<E: 'static>(
        &mut self,
        node: &PointwiseNode<E>,
        image: &SharedImageMap,
    ) {
        self.cache.record_image(node, image, &mut self.frame);
        // Keep the image identity alive for this frame, including outputs
        // held only by the original evaluator's transient stack.
        if self.cache.images.contains_key(&node.id) {
            self.images.insert(node.id, image.clone());
        }
    }
}

impl SequenceOutputCache {
    fn is_stationary<E: 'static>(&mut self, root: &PointwiseNode<E>) -> bool {
        enum Visit<E> {
            Enter(PointwiseNode<E>),
            Leave(NodeId, Vec<NodeId>),
        }
        let mut pending = vec![Visit::Enter(root.clone())];
        let mut visiting = BTreeSet::new();
        while let Some(step) = pending.pop() {
            match step {
                Visit::Enter(node) => {
                    if self.stationary.contains_key(&node.id) {
                        continue;
                    }
                    if !visiting.insert(node.id)
                        || !node
                            .local_animation_needed(false)
                            .is_ok_and(|animated| !animated)
                    {
                        self.stationary.insert(node.id, false);
                        continue;
                    }
                    let reference = if node.kind() == Some(PointwiseKind::Reference) {
                        match node.reference_target() {
                            Ok(target) => target,
                            Err(_) => {
                                self.stationary.insert(node.id, false);
                                continue;
                            }
                        }
                    } else {
                        None
                    };
                    let inputs: Vec<_> = node
                        .slots()
                        .into_iter()
                        .chain(node.contents())
                        .filter_map(|slot| slot.node())
                        .chain(reference)
                        .collect();
                    pending.push(Visit::Leave(
                        node.id,
                        inputs.iter().map(|input| input.id).collect(),
                    ));
                    pending.extend(inputs.into_iter().map(Visit::Enter));
                }
                Visit::Leave(node, inputs) => {
                    visiting.remove(&node);
                    let stationary = self.stationary.get(&node) != Some(&false)
                        && inputs
                            .iter()
                            .all(|input| self.stationary.get(input) == Some(&true));
                    self.stationary.insert(node, stationary);
                }
            }
        }
        self.stationary.get(&root.id) == Some(&true)
    }

    pub(super) fn record_image<E: 'static>(
        &mut self,
        node: &PointwiseNode<E>,
        image: &SharedImageMap,
        frame: &mut SequenceFrameImages,
    ) {
        if self.images.contains_key(&node.id) || !self.is_stationary(node) {
            return;
        }
        // Preserve exact shared outputs (e.g. a Group and its contents) and
        // shared float or named stores without duplicating their pixels.
        let owner = *frame
            .input_owners
            .entry(image.storage_identity())
            .or_insert(node.id);
        let saved = if let Some((_, saved)) = self.images.get(&owner) {
            saved.clone()
        } else {
            SharedImageMap::from_image(frame.save.image(image.capture_fields()))
        };
        self.images.insert(node.id, (owner, saved));
    }

    pub(super) fn image(
        &self,
        node: NodeId,
        geometry: ImageGeometry,
        fresh: &mut BTreeMap<NodeId, SharedImageMap>,
        frame: &mut SequenceFrameImages,
    ) -> Option<SharedImageMap> {
        let (owner, original) = self.images.get(&node)?;
        if original.geometry() != geometry {
            return None;
        }
        let image = fresh
            .entry(*owner)
            .or_insert_with(|| {
                SharedImageMap::from_image(frame.load.image(original.capture_fields()))
            })
            .clone();
        frame.input_owners.insert(image.storage_identity(), *owner);
        Some(image)
    }

    pub(super) fn record_scene<E: 'static>(
        &mut self,
        node: &PointwiseNode<E>,
        scene: &Option<Scene>,
    ) {
        if !self.scenes.contains_key(&node.id) && self.is_stationary(node) {
            self.scenes.insert(node.id, scene.clone());
        }
    }

    pub(super) fn scene(&self, node: NodeId) -> Option<Option<Scene>> {
        self.scenes.get(&node).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PointwiseFilter, ScenePrimitiveKind, ScenePrimitiveNode};

    #[test]
    fn stationary_outputs_keep_named_and_region_maps_and_aliases_across_frames() {
        // Store Map wasn't covered by the old generator-and-blur cache.
        let first = PointwiseFilter::<()>::new(PointwiseKind::StoreMap)
            .unwrap()
            .node();
        let second = PointwiseFilter::<()>::new(PointwiseKind::Group)
            .unwrap()
            .node();
        second.content(0).unwrap().set_node(Some(&first)).unwrap();
        let geometry = ImageGeometry::new(3, 2);
        let raster = SharedImageMap::new(geometry);
        let region = crate::SharedFloatStore::new(crate::FloatBuffer::new(vec![1.; 6])).unwrap();
        raster.set_region_store(Some(region.clone()));
        let name: Vec<_> = "Height".encode_utf16().collect();
        raster
            .add_named_store(Some(name.clone()), Some(region))
            .unwrap();
        raster.set_constant(0, 0.5).unwrap();

        let mut cache = SequenceOutputCache::default();
        let mut previous = SequenceFrameImages::new();
        cache.record_image(&first, &raster, &mut previous);
        cache.record_image(&second, &raster, &mut previous);
        raster.set_constant(0, 0.75).unwrap();
        drop(previous);

        let mut images = BTreeMap::new();
        let mut frame = SequenceFrameImages::new();
        let forwarding = cache
            .image(second.id, geometry, &mut images, &mut frame)
            .unwrap();
        let source = cache
            .image(first.id, geometry, &mut images, &mut frame)
            .unwrap();
        assert!(forwarding.same_reference(&source));
        assert_eq!(source.channel(0, true).unwrap().read(0).unwrap(), 0.5);
        let named = source.named_channel(0).unwrap().unwrap();
        assert_eq!(named.name_utf16(), name);
        assert!(
            named
                .data()
                .unwrap()
                .same_store(&source.region_store().unwrap())
        );
        named
            .data()
            .unwrap()
            .data(false)
            .unwrap()
            .write(0, 42.)
            .unwrap();
        let independent = cache
            .image(
                first.id,
                geometry,
                &mut BTreeMap::new(),
                &mut SequenceFrameImages::new(),
            )
            .unwrap();
        assert_eq!(
            independent
                .region_store()
                .unwrap()
                .data(true)
                .unwrap()
                .read(0)
                .unwrap(),
            1.
        );
    }

    #[test]
    fn an_animated_numeric_control_qualifies_even_with_a_stationary_input_override() {
        let color = PointwiseFilter::<()>::new(PointwiseKind::OklabColor).unwrap();
        let signal = PointwiseNode::solid_color().unwrap();
        color
            .node()
            .slot(1)
            .unwrap()
            .set_node(Some(&signal))
            .unwrap();
        color
            .parameter(1)
            .unwrap()
            .set_axis_animated(0, true)
            .unwrap();
        let mut cache = SequenceOutputCache::default();
        assert!(cache.is_stationary(&signal));
        assert!(!cache.is_stationary(&color.node()));
    }

    #[test]
    fn original_evaluator_reuses_stationary_images_between_frames() {
        use crate::{
            GradientGenerator, ImageGeneratorKind, PointwiseRenderEffects, PointwiseRenderOptions,
            ProceduralExecution,
        };

        let noise = GradientGenerator::<()>::with_kind(ImageGeneratorKind::Noise)
            .unwrap()
            .node();
        let blur = PointwiseFilter::new(PointwiseKind::GaussianBlur).unwrap();
        blur.parameter(1).unwrap().set_number(0.1).unwrap();
        let node = blur.node();
        node.slot(0).unwrap().set_node(Some(&noise)).unwrap();
        let geometry = ImageGeometry::new(24, 18);
        let mut preflights = 0;
        let mut cache = SequenceOutputCache::default();
        let mut frames = Vec::new();
        let mut first_preflights = None;
        for time in [0.1, 0.9] {
            let image = node
                .render_original_sequence_frame(
                    geometry,
                    time,
                    ProceduralExecution::CallingThread,
                    PointwiseRenderOptions::new(false, false),
                    &mut cache,
                    &mut PointwiseRenderEffects::new(&mut |_| unreachable!(), &mut |_, _| {
                        preflights += 1;
                        Ok(())
                    }),
                )
                .unwrap()
                .image()
                .unwrap();
            frames.push(image.channel(0, true).unwrap().values());
            let expected = first_preflights.get_or_insert(preflights);
            assert!(*expected > 0);
            assert_eq!(preflights, *expected);
        }
        assert_eq!(frames[0], frames[1]);
    }

    #[test]
    fn stationary_primitive_scene_is_retained_for_scene_consumers() {
        let primitive = ScenePrimitiveNode::<()>::new(ScenePrimitiveKind::Plane).unwrap();
        let mut admit = |_| Ok(());
        let scene = primitive
            .render_scene(0., &mut crate::SceneAllocationEffects::new(&mut admit))
            .unwrap();
        let mut cache = SequenceOutputCache::default();
        cache.record_scene(&primitive.node(), &Some(scene.clone()));
        assert!(
            cache
                .scene(primitive.node().id)
                .unwrap()
                .unwrap()
                .same_reference(&scene)
        );
    }
}
