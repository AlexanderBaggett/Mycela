//! Static adjacent source-body laws, separate from executed source goldens.
use super::*;
use crate::{FloatBuffer, ImageGeometry};
use std::sync::{Arc, Mutex};
#[test]
fn returned_input_and_live_aliases() {
    let empty = SharedImageMap::new(ImageGeometry::new(1, 1));
    let input = SharedImageMap::new(ImageGeometry::new(1, 1));
    input.set_constant(0, 0.5).unwrap();
    let f = PointwiseFilter::<()>::new(PointwiseKind::Add).unwrap();
    let output = f
        .render_supplied(&[Some(empty), Some(input.clone())], 1, 1)
        .unwrap()
        .unwrap();
    assert!(input.same_reference(&output));
    output.set_constant(0, 0.25).unwrap();
    assert_eq!(input.channel(0, false).unwrap().read(0).unwrap(), 0.25);
}
#[test]
fn same_map_hsb_roles_do_not_retain_mutable_guards() {
    let input = SharedImageMap::new(ImageGeometry::new(2, 1));
    input
        .set_buffer(0, Some(&FloatBuffer::new(vec![0.25, 0.75])))
        .unwrap();
    let f = PointwiseFilter::<()>::new(PointwiseKind::FromHsb).unwrap();
    let result = f
        .render_supplied(
            &[Some(input.clone()), Some(input.clone()), Some(input)],
            2,
            1,
        )
        .unwrap()
        .unwrap();
    assert_eq!(result.channel(0, false).unwrap().len(), 2);
}
#[test]
fn invalid_histogram_index_precedes_disabled_guard() {
    let h = Histogram::new();
    let arrays = h.arrays();
    h.set_disabled(true);
    h.set_channel(4);
    assert_eq!(h.set_component(0, 1.0), Err(crate::ImageError::Index));
    h.set_channel(0);
    assert_eq!(h.selected_channel(), 4);
    arrays[0].write(1, 0.25).unwrap();
    assert_eq!(h.projection(0).unwrap().unwrap().read(0).unwrap(), 0.25);
}
#[test]
fn retained_parameter_sponsors_owner_and_options_clone() {
    let node = PointwiseFilter::<()>::new(PointwiseKind::GetChannel).unwrap();
    let p = node.parameter(0).unwrap();
    let old = p.options().unwrap();
    let clone = node.source_clone().unwrap();
    assert!(old.same_reference(&clone.parameter(0).unwrap().options().unwrap()));
    drop(node);
    old.set_entry(0, Some(vec![0xd800])).unwrap();
    assert_eq!(p.text().unwrap(), Some(vec![0xd800]));
}
#[test]
fn equal_options_keep_old_array() {
    let node = PointwiseFilter::<()>::new(PointwiseKind::GetChannel).unwrap();
    let p = node.parameter(0).unwrap();
    let old = p.options().unwrap();
    let equal = crate::SharedSelectionOptions::new(old.entries());
    p.set_options(Some(equal)).unwrap();
    assert!(old.same_reference(&p.options().unwrap()));
}
#[test]
fn actual_cache_callback_reentry_and_typed_error() {
    let node = PointwiseFilter::<u8>::new(PointwiseKind::Add).unwrap();
    let image = SharedImageMap::new(ImageGeometry::new(1, 1));
    let records = Arc::new(Mutex::new(Vec::new()));
    let trace = records.clone();
    let expected = image.clone();
    let listener = PointwiseCacheHandler::new(move |sender: &PointwiseFilter<u8>| {
        let current = sender.cached_image().unwrap();
        trace
            .lock()
            .unwrap()
            .push(current.same_reference(&expected));
        Err(7)
    });
    node.add_cache_handler(listener.clone());
    assert_eq!(
        node.set_cached_image(Some(image)),
        Err(PointwiseError::Observer(7))
    );
    assert_eq!(*records.lock().unwrap(), vec![true]);
    node.remove_cache_handler(&listener);
    node.set_cached_image(None).unwrap();
}
#[test]
fn worker_partial_failure_and_aliases() {
    let a = FloatBuffer::new(vec![1.0, 2.0, 3.0]);
    let out = FloatBuffer::new(vec![0.0; 2]);
    let worker = ArithmeticWorker::new(
        ArithmeticOperation::AddScalar,
        Some(out.clone()),
        Some(a.clone()),
        None,
        0.5,
    );
    assert_eq!(worker.run(), Err(crate::ImageError::Index));
    assert_eq!(out.values(), vec![1.5, 2.5]);
    let alias = ArithmeticWorker::new(
        ArithmeticOperation::MultiplyArrays,
        Some(a.clone()),
        Some(a.clone()),
        Some(a.clone()),
        0.0,
    );
    alias.run().unwrap();
    assert_eq!(a.values(), vec![1.0, 4.0, 9.0]);
}
#[test]
fn short_hsb_destination_preserves_first_writes() {
    let out = FloatBuffer::new(vec![7.0; 2]);
    assert_eq!(
        rgb_to_hsb(Some(&out), 0.0, 0.0, 0.0),
        Err(crate::ImageError::Index)
    );
    assert_eq!(out.values(), vec![7.0; 2]);
    assert_eq!(
        hsb_to_rgb(Some(&out), 0.0, 1.0, 1.0),
        Err(crate::ImageError::Index)
    );
    assert_eq!(out.values(), vec![1.0, 0.0]);
}
#[test]
fn null_worker_source_precedes_negative_index() {
    let worker = ArithmeticWorker::new(ArithmeticOperation::MultiplyScalar, None, None, None, 1.0);
    assert_eq!(
        worker.run_range(-1, 1),
        Err(crate::ImageError::NullReference)
    );
}
#[test]
fn bci_bypass_can_return_missing_input_and_bad_choice_precedes_input() {
    let bci = PointwiseFilter::<()>::new(PointwiseKind::ChangeBci).unwrap();
    assert!(bci.render_supplied(&[None], 1, 1).unwrap().is_none());
    let channel = PointwiseFilter::<()>::new(PointwiseKind::GetChannel).unwrap();
    channel.parameter(0).unwrap().set_index(99).unwrap();
    assert!(matches!(
        channel.render_supplied(&[None], 1, 1),
        Err(PointwiseError::Index)
    ));
}

#[test]
fn colorize_wraps_actual_interpolation_worker_cause() {
    let node = PointwiseFilter::<()>::new(PointwiseKind::Colorize).unwrap();
    let input = SharedImageMap::new(ImageGeometry::new(2, 1));
    input
        .set_buffer(0, Some(&FloatBuffer::new(vec![0.25, 0.75])))
        .unwrap();
    assert!(matches!(
        node.render_supplied(&[Some(input)], 4, 1),
        Err(PointwiseError::Interpolation(crate::ImageError::Index))
    ));
}

#[test]
fn graph_parent_move_detaches_first_occurrence_and_keeps_stale_edges() {
    use super::PointwiseNode;
    let a = PointwiseFilter::<()>::new(PointwiseKind::Add)
        .unwrap()
        .node();
    let b = PointwiseFilter::<()>::new(PointwiseKind::Multiply)
        .unwrap()
        .node();
    let child = PointwiseNode::solid_color().unwrap();
    a.slot(0).unwrap().set_node(Some(&child)).unwrap();
    a.slot(1).unwrap().set_node(Some(&child)).unwrap();
    b.slot(0).unwrap().set_node(Some(&child)).unwrap();
    assert!(a.slot(0).unwrap().node().is_none());
    assert!(a.slot(1).unwrap().node().unwrap().same_reference(&child));
    assert!(child.parent().unwrap().same_reference(&b));
    a.slot(1).unwrap().set_node(None).unwrap();
    assert!(child.parent().unwrap().same_reference(&b));
}

#[test]
fn graph_clone_splits_occurrences_but_retains_actual_cached_image() {
    use super::PointwiseNode;
    let node = PointwiseFilter::<()>::new(PointwiseKind::Add)
        .unwrap()
        .node();
    let child = PointwiseNode::solid_color().unwrap();
    let cache = SharedImageMap::new(ImageGeometry::new(2, 1));
    cache.set_constant(0, 0.25).unwrap();
    child.set_cached_image(Some(cache.clone())).unwrap();
    for slot in node.slots() {
        slot.set_node(Some(&child)).unwrap();
    }
    let clone = node.source_clone().unwrap();
    let a = clone.slot(0).unwrap().node().unwrap();
    let b = clone.slot(1).unwrap().node().unwrap();
    assert!(!a.same_reference(&b) && !a.same_reference(&child));
    assert!(a.parent().unwrap().same_reference(&clone));
    assert!(b.parent().unwrap().same_reference(&clone));
    assert!(a.cached_image().unwrap().same_reference(&cache));
    assert!(b.cached_image().unwrap().same_reference(&cache));
    cache.channel(0, false).unwrap().write(0, 0.75).unwrap();
    assert_eq!(
        a.cached_image()
            .unwrap()
            .channel(0, false)
            .unwrap()
            .read(0)
            .unwrap(),
        0.75
    );
}

#[test]
fn graph_callback_reentry_observes_committed_cache_without_owner_guard() {
    let filter = PointwiseFilter::<()>::new(PointwiseKind::Add).unwrap();
    let child = super::PointwiseNode::solid_color().unwrap();
    filter
        .node()
        .slot(0)
        .unwrap()
        .set_node(Some(&child))
        .unwrap();
    filter.add_cache_handler(super::PointwiseCacheHandler::new(|sender| {
        assert!(sender.cached_image().is_some());
        sender.node().slot(0).unwrap().set_node(None).unwrap();
        assert!(sender.cached_image().is_none());
        Ok(())
    }));
    filter
        .set_cached_image(Some(SharedImageMap::new(ImageGeometry::new(1, 1))))
        .unwrap();
    assert!(filter.cached_image().is_none());
    assert!(child.parent().unwrap().same_reference(&filter.node()));
}

#[test]
fn graph_retained_property_and_slot_keep_connected_owner_alive() {
    let filter = PointwiseFilter::<()>::new(PointwiseKind::ChangeHsb).unwrap();
    let property = filter.parameter(0).unwrap();
    let slot = filter.node().slot(0).unwrap();
    let child = super::PointwiseNode::solid_color().unwrap();
    slot.set_node(Some(&child)).unwrap();
    drop(filter);
    drop(child);
    property.set_number(30.0).unwrap();
    assert!(
        slot.node()
            .unwrap()
            .parent()
            .unwrap()
            .same_reference(&property.owner().node())
    );
    let result = property.owner().node().render_original(2, 1).unwrap();
    assert!(result.caught_errors().is_empty());
    assert!(result.image().unwrap().is_permanently_sealed());
}

#[test]
fn graph_public_render_retains_cause_and_returns_real_empty_map() {
    let filter = PointwiseFilter::<()>::new(PointwiseKind::GetChannel).unwrap();
    filter.parameter(0).unwrap().set_index(99).unwrap();
    let result = filter.node().render_original(2, 1).unwrap();
    assert_eq!(result.caught_errors().len(), 1);
    assert_eq!(
        result.caught_errors()[0].source_class(),
        "IndexOutOfRangeException"
    );
    let image = result.image().unwrap();
    assert!(image.is_empty().unwrap());
    assert!(!image.is_permanently_sealed());
    assert!(filter.cached_image().is_none());
}

// Source-body publication/error-order laws; no invented original golden values.
#[test]
fn arithmetic_outputs_seal_completed_dense_channels_without_replacing_input_stores() {
    for kind in [PointwiseKind::Add, PointwiseKind::Multiply] {
        let a = SharedImageMap::new(ImageGeometry::new(2, 1));
        let b = SharedImageMap::new(ImageGeometry::new(2, 1));
        let plane = FloatBuffer::new(vec![0.25, 0.75]);
        a.set_buffer(0, Some(&plane)).unwrap();
        b.set_constant(0, 0.5).unwrap();
        let held = a.channel_observation().planes().unwrap()[0]
            .clone()
            .unwrap();
        let f = PointwiseFilter::<()>::new(kind).unwrap();
        let out = f
            .render_supplied(&[Some(a.clone()), Some(b)], 2, 1)
            .unwrap()
            .unwrap();
        let store = out.channel_observation().planes().unwrap()[0]
            .clone()
            .unwrap();
        assert!(store.is_sealed());
        assert!(store.is_temporarily_sealed());
        assert!(!held.same_store(&store));
        assert!(held.data(true).unwrap().shares_storage(&plane));
        let readonly = out.channel(0, true).unwrap();
        assert!(store.is_sealed());
        let writable = out.channel(0, false).unwrap();
        assert!(readonly.shares_storage(&writable));
        assert!(!store.is_sealed());
        assert!(!held.is_sealed());
    }
}

#[test]
fn multiply_zero_shortcut_leaves_unmaterialized_output_while_other_channel_seals() {
    let a = SharedImageMap::new(ImageGeometry::new(2, 1));
    let b = SharedImageMap::new(ImageGeometry::new(2, 1));
    a.set_constant(0, 0.0).unwrap();
    a.set_buffer(1, Some(&FloatBuffer::new(vec![0.25, 0.75])))
        .unwrap();
    b.set_buffer(0, Some(&FloatBuffer::new(vec![0.5, 0.75])))
        .unwrap();
    b.set_constant(1, 0.5).unwrap();
    let f = PointwiseFilter::<()>::new(PointwiseKind::Multiply).unwrap();
    let out = f
        .render_supplied(&[Some(a), Some(b)], 2, 1)
        .unwrap()
        .unwrap();
    let observed = out.channel_observation();
    assert!(observed.planes().unwrap()[0].is_none());
    assert!(observed.planes().unwrap()[1].as_ref().unwrap().is_sealed());
    assert_eq!(out.channel(0, true).unwrap().read(0).unwrap(), 0.0);
}

#[test]
fn multiply_reads_first_channel_count_before_dereferencing_second_input() {
    let a = SharedImageMap::new(ImageGeometry::new(2, 1));
    a.dispose();
    let f = PointwiseFilter::<()>::new(PointwiseKind::Multiply).unwrap();
    assert!(matches!(
        f.render_supplied(&[Some(a), None], 2, 1),
        Err(PointwiseError::Image(ImageError::Disposed))
    ));
    let live = SharedImageMap::new(ImageGeometry::new(2, 1));
    assert!(matches!(
        f.render_supplied(&[Some(live), None], 2, 1),
        Err(PointwiseError::NullReference)
    ));
}

#[test]
fn levels_seals_copied_alpha_and_transformed_channels() {
    let input = SharedImageMap::new(ImageGeometry::new(2, 1));
    for channel in 0..4 {
        input
            .set_buffer(channel, Some(&FloatBuffer::new(vec![0.25, 0.75])))
            .unwrap();
    }
    let f = PointwiseFilter::<()>::new(PointwiseKind::Levels).unwrap();
    let output = f
        .render_supplied(&[Some(input.clone())], 2, 1)
        .unwrap()
        .unwrap();
    for store in output.channel_observation().planes().unwrap() {
        assert!(store.as_ref().unwrap().is_sealed());
        assert!(store.as_ref().unwrap().is_temporarily_sealed());
    }
    for store in input.channel_observation().planes().unwrap() {
        assert!(!store.as_ref().unwrap().is_sealed());
    }
}

#[test]
fn colorize_seals_rgb_before_later_unsealed_keep_transparency_alias() {
    let input = SharedImageMap::new(ImageGeometry::new(2, 1));
    input
        .set_buffer(0, Some(&FloatBuffer::new(vec![0.25, 0.75])))
        .unwrap();
    let alpha = FloatBuffer::new(vec![0.2, 0.4]);
    input.set_buffer(3, Some(&alpha)).unwrap();
    let f = PointwiseFilter::<()>::new(PointwiseKind::Colorize).unwrap();
    let g = f.parameter(0).unwrap().gradient().unwrap();
    g.set_color_at(0, crate::RgbColor::new(0.0, 0.2, 0.4, 1.0))
        .unwrap();
    g.set_color_at(1, crate::RgbColor::new(1.0, 0.7, 0.8, 1.0))
        .unwrap();
    let out = f.render_supplied(&[Some(input)], 2, 1).unwrap().unwrap();
    let observed = out.channel_observation();
    for channel in 0..3 {
        assert!(
            observed.planes().unwrap()[channel]
                .as_ref()
                .unwrap()
                .is_sealed()
        );
    }
    let kept = observed.planes().unwrap()[3].as_ref().unwrap();
    assert!(!kept.is_sealed());
    assert!(kept.data(true).unwrap().shares_storage(&alpha));
}

#[test]
fn invert_seals_transformed_and_ignored_arrays_without_sealing_input_store() {
    let mut input = crate::ImageMap::new(ImageGeometry::new(2, 1));
    for channel in 0..4 {
        input
            .set_buffer(channel, &FloatBuffer::new(vec![0.25, 0.75]))
            .unwrap();
    }
    let original = input.channel_observation();
    let output = crate::StoredInvert::default()
        .render(&mut input, 2, 1)
        .unwrap();
    let observed = output.channel_observation();
    for channel in 0..4 {
        let held = original.planes().unwrap()[channel].as_ref().unwrap();
        let store = observed.planes().unwrap()[channel].as_ref().unwrap();
        assert!(store.is_sealed());
        assert!(store.is_temporarily_sealed());
        assert!(!held.is_sealed());
        assert!(!held.same_store(store));
        assert_eq!(
            held.data(true)
                .unwrap()
                .shares_storage(&store.data(true).unwrap()),
            channel == 3
        );
    }
}
