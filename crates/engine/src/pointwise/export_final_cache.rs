//! Still export cache ownership across detached document render jobs.
use super::PointwiseNodeRenderSnapshot;
use crate::pointwise::{
    check_render_cancelled,
    graph::{FinalExportCacheTree, StillExportCachePolicy},
    guard,
};
use crate::{
    ImageGeometry, PointwiseError, PointwiseRenderEffects, PointwiseRenderOptions,
    PointwiseRenderOutcome, ProceduralExecution,
};
use std::sync::{Arc, Mutex};

/// Retains final cache owners for the latest eligible still export of a document.
/// Its mutex covers preparation and recipient registration. Rendering uses the
/// private final clone after that guard has been released.
pub struct PointwiseFinalExportCache<E = ()> {
    current: Arc<Mutex<Option<CacheOwner<E>>>>,
}

struct CacheOwner<E> {
    snapshot: PointwiseNodeRenderSnapshot<E>,
    tree: FinalExportCacheTree<E>,
    policy: StillExportCachePolicy,
}

impl<E> Default for PointwiseFinalExportCache<E> {
    fn default() -> Self {
        Self {
            current: Arc::new(Mutex::new(None)),
        }
    }
}

impl<E> Clone for PointwiseFinalExportCache<E> {
    fn clone(&self) -> Self {
        Self {
            current: self.current.clone(),
        }
    }
}

impl<E: 'static> PointwiseFinalExportCache<E> {
    /// Render a frozen graph using the document's fixed export camera.
    /// Matching frozen dependencies can inherit intermediate images from the
    /// previous export of the same enclosing tree.
    pub fn render_snapshot_with_effects(
        &self,
        snapshot: &PointwiseNodeRenderSnapshot<E>,
        geometry: ImageGeometry,
        execution: ProceduralExecution,
        options: PointwiseRenderOptions,
        effects: &mut PointwiseRenderEffects<'_, E>,
    ) -> Result<PointwiseRenderOutcome<E>, PointwiseError<E>> {
        let prepared = {
            let mut current = guard(&self.current);
            // A cancelled job may have been queued while another export was
            // preparing. It must not replace that job's cache recipient.
            check_render_cancelled(effects.cancel_check())?;
            let previous = current.take();
            match snapshot.node.clone_supported_final_graph()? {
                Some(graph) => {
                    let tree = graph.export_cache_tree(
                        &snapshot.origins,
                        snapshot.time,
                        effects.cancel_check(),
                    )?;
                    if effects.has_environment_library() && tree.uses_environment_library() {
                        // The files can change while the frozen GTX operands stay equal.
                        drop(current);
                        return graph.render_with_effects(
                            geometry,
                            snapshot.time,
                            execution,
                            options,
                            false,
                            effects,
                        );
                    }
                    let mut policy = StillExportCachePolicy::default();
                    if let Some(old) = previous.filter(|old| tree.same_root(&old.tree)) {
                        let same_settings = old.policy.matches(geometry, options)
                            && old.snapshot.time.to_bits() == snapshot.time.to_bits();
                        let same_selection = old.snapshot.origins[&old.snapshot.node.id]
                            == snapshot.origins[&snapshot.node.id];
                        let same_context =
                            same_selection && old.snapshot.same_render_values(snapshot);
                        tree.inherit(
                            &old.tree,
                            same_settings,
                            same_context,
                            effects.cancel_check(),
                        )?;
                        policy = old.policy;
                    }
                    let use_cache = graph.prepare_still_export_caches(
                        geometry,
                        options,
                        &mut policy,
                        effects.cancel_check(),
                    )?;
                    if use_cache {
                        *current = Some(CacheOwner {
                            snapshot: snapshot.clone(),
                            tree,
                            policy,
                        });
                    }
                    Some((graph, use_cache))
                }
                None => None,
            }
        };
        if let Some((graph, use_cache)) = prepared {
            let rendered = graph.render_with_effects(
                geometry,
                snapshot.time,
                execution,
                options,
                use_cache,
                effects,
            );
            if use_cache {
                graph.retire_destructive_caches();
            }
            rendered
        } else {
            snapshot.render_with_execution_and_effects(
                geometry.width(),
                geometry.height(),
                execution,
                options,
                effects,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AnalyticCurveKind, CurveLayerHandle, GradientGenerator,
        GradientGeneratorParameterKind::Location, PointwiseFilter, PointwiseKind, PointwiseNode,
        RenderCaptureSession, SceneCombineNode, SharedImageMap,
    };

    fn render(
        cache: &PointwiseFinalExportCache,
        snapshot: &PointwiseNodeRenderSnapshot,
        size: ImageGeometry,
        cancelled: bool,
    ) -> Result<SharedImageMap, PointwiseError> {
        let mut capacity = |_| unreachable!();
        let mut preflight = |_, _| unreachable!();
        let check = || cancelled;
        let mut effects =
            PointwiseRenderEffects::new(&mut capacity, &mut preflight).with_cancel_check(&check);
        cache
            .render_snapshot_with_effects(
                snapshot,
                size,
                ProceduralExecution::CallingThread,
                PointwiseRenderOptions::new(false, true),
                &mut effects,
            )
            .map(|outcome| outcome.image().unwrap())
    }

    fn invert_chain(mut node: PointwiseNode, length: usize) -> Vec<PointwiseNode> {
        let mut chain = vec![node.clone()];
        for _ in 0..length {
            let invert = PointwiseNode::invert();
            invert.slot(0).unwrap().set_node(Some(&node)).unwrap();
            node = invert;
            chain.push(node.clone());
        }
        chain
    }

    #[test]
    fn duplicate_exports_reuse_the_frozen_graph_at_the_same_geometry_and_time() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        gradient.parameter(Location).set_axis(0, 0.25).unwrap();
        let chain = invert_chain(gradient.node(), 9);
        // The selected node is one of the two marked intermediate caches.
        let selected = &chain[6];
        let capture = || {
            RenderCaptureSession::new()
                .capture_node(selected, 0.)
                .unwrap()
        };
        let cache = PointwiseFinalExportCache::default();
        let run = |snapshot: &PointwiseNodeRenderSnapshot, size, cancelled| {
            render(&cache, snapshot, size, cancelled)
        };
        let geometry = ImageGeometry::new(8, 3);
        let old = capture();
        let first = run(&old, geometry, false).unwrap();
        assert!(
            run(&capture(), geometry, false)
                .unwrap()
                .same_reference(&first)
        );
        assert!(matches!(
            run(&capture(), ImageGeometry::new(8, 4), true),
            Err(PointwiseError::Cancelled)
        ));
        assert!(
            run(&capture(), geometry, false)
                .unwrap()
                .same_reference(&first)
        );
        let resized = run(&capture(), ImageGeometry::new(8, 4), false).unwrap();
        assert!(!resized.same_reference(&first));
        assert_eq!(resized.geometry(), ImageGeometry::new(8, 4));
        let at_another_time = RenderCaptureSession::new()
            .capture_node(selected, 0.5)
            .unwrap();
        let other = run(&at_another_time, ImageGeometry::new(8, 4), false).unwrap();
        assert!(!other.same_reference(&resized));

        gradient.parameter(Location).set_axis(0, 0.5).unwrap();
        let current = run(&capture(), geometry, false).unwrap();
        assert_eq!(current.channel(0, false).unwrap().read(0).unwrap(), 0.5);
        let frozen = run(&old, geometry, false).unwrap();
        assert_eq!(frozen.channel(0, false).unwrap().read(0).unwrap(), 0.25);
        assert!(gradient.cached_final_image().is_none());
    }

    #[test]
    fn selection_changes_and_sibling_edits_keep_unaffected_intermediate_images() {
        let left = GradientGenerator::<()>::new().unwrap();
        let right = GradientGenerator::<()>::new().unwrap();
        let left_nodes = invert_chain(left.node(), 6);
        let right_nodes = invert_chain(right.node(), 6);
        let a = &left_nodes[6];
        let b = &right_nodes[6];
        let root = SceneCombineNode::new().node();
        root.slot(0).unwrap().set_node(Some(a)).unwrap();
        root.slot(1).unwrap().set_node(Some(b)).unwrap();
        let cache = PointwiseFinalExportCache::default();
        let run = |selected: &PointwiseNode| {
            let snapshot = RenderCaptureSession::new()
                .capture_node(selected, 0.)
                .unwrap();
            render(&cache, &snapshot, ImageGeometry::new(8, 3), false).unwrap()
        };
        let left_image = run(a);
        let right_image = run(b);
        assert!(run(a).same_reference(&left_image));

        right.parameter(Location).set_axis(0, 0.375).unwrap();
        assert!(run(a).same_reference(&left_image));
        let edited_right = run(b);
        assert!(!edited_right.same_reference(&right_image));
        assert_eq!(
            edited_right.channel(0, false).unwrap().read(0).unwrap(),
            0.375
        );
        assert!(run(a).same_reference(&left_image));
    }

    #[test]
    fn group_reference_invalidates_its_consumers_when_its_input_changes() {
        let selected_gradient = GradientGenerator::<()>::new().unwrap();
        let other_gradient = GradientGenerator::new().unwrap();
        let group = PointwiseFilter::new(PointwiseKind::Group).unwrap();
        group.parameter(0).unwrap().set_number(2.).unwrap();
        let group = group.node();
        group
            .slot(0)
            .unwrap()
            .set_node(Some(&selected_gradient.node()))
            .unwrap();
        group
            .slot(1)
            .unwrap()
            .set_node(Some(&other_gradient.node()))
            .unwrap();
        let reference = PointwiseFilter::new(PointwiseKind::Reference).unwrap();
        let nodes = invert_chain(reference.node(), 9);
        group.content(0).unwrap().set_node(nodes.last()).unwrap();
        let cache = PointwiseFinalExportCache::default();
        let run = |selected: &PointwiseNode| {
            let snapshot = RenderCaptureSession::new()
                .capture_node(selected, 0.)
                .unwrap();
            render(&cache, &snapshot, ImageGeometry::new(8, 3), false).unwrap()
        };
        let first = run(&nodes[6]);
        other_gradient
            .parameter(Location)
            .set_axis(0, 0.625)
            .unwrap();
        assert!(run(&nodes[6]).same_reference(&first));
        selected_gradient
            .parameter(Location)
            .set_axis(0, 0.375)
            .unwrap();
        let changed = run(&nodes[6]);
        assert!(!changed.same_reference(&first));
        assert_eq!(changed.channel(0, false).unwrap().read(0).unwrap(), 0.375);
        reference.parameter(0).unwrap().set_number(2.).unwrap();
        let rerouted = run(&nodes[6]);
        assert!(!rerouted.same_reference(&changed));
        assert_eq!(rerouted.channel(0, false).unwrap().read(0).unwrap(), 0.625);

        let selector = reference.parameter(0).unwrap();
        selector.set_number(1.).unwrap();
        selector.set_axis_animated(0, true).unwrap();
        let curve = selector.animation_curve(0).unwrap();
        curve.set_minimum(2.);
        curve.set_maximum(2.);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        let grouped = run(&group);
        assert_eq!(grouped.channel(0, false).unwrap().read(0).unwrap(), 0.625);
        let direct = run(&nodes[6]);
        assert_eq!(direct.channel(0, false).unwrap().read(0).unwrap(), 0.375);
        assert!(run(&nodes[6]).same_reference(&direct));
    }

    #[test]
    fn cached_gradient_rereads_its_variable_owner_after_a_curve_edit() {
        let gradient = GradientGenerator::<()>::new().unwrap();
        let location = gradient.parameter(Location);
        location.set_binding_index(0);
        location.set_binding_name(Some(&"X".encode_utf16().collect::<Vec<_>>()));
        let nodes = invert_chain(gradient.node(), 9);
        let map = PointwiseFilter::new(PointwiseKind::MapVariable).unwrap();
        map.node()
            .content(0)
            .unwrap()
            .set_node(nodes.last())
            .unwrap();
        let scalar = map.parameter(3).unwrap();
        scalar.set_axis_animated(0, true).unwrap();
        let curve = scalar.animation_curve(0).unwrap();
        curve.set_minimum(0.25);
        curve.set_maximum(0.25);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        let cache = PointwiseFinalExportCache::default();
        let run = || {
            let snapshot = RenderCaptureSession::new()
                .capture_node(&nodes[6], 0.5)
                .unwrap();
            render(&cache, &snapshot, ImageGeometry::new(8, 3), false).unwrap()
        };
        let first = run();
        assert_eq!(first.channel(0, false).unwrap().read(0).unwrap(), 0.25);
        assert!(run().same_reference(&first));
        curve.set_minimum(0.5);
        curve.set_maximum(0.5);
        let edited = run();
        assert!(!edited.same_reference(&first));
        assert_eq!(edited.channel(0, false).unwrap().read(0).unwrap(), 0.5);
    }
}
