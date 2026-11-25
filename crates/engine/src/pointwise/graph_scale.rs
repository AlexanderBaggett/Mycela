//! Render and authored graph scaling with zl.h / zl.g Reference routing.
use super::{PointwiseError, PointwiseKind, PointwiseNode};
use crate::pointwise::{parameter_ops, region::NodeId, template_lab, transform};
use crate::{GradientGeneratorParameterKind, PointwiseParameterValue};
use std::collections::{BTreeMap, BTreeSet};

const GROUP_INPUTS: usize = 20;
const ALL_INPUTS: u32 = (1 << GROUP_INPUTS) - 1;

fn slot_bit(index: usize) -> u32 {
    if index < GROUP_INPUTS { 1 << index } else { 0 }
}

#[derive(Default)]
struct ScaleRoutes {
    raw: BTreeMap<NodeId, u32>,
    reached: BTreeMap<(NodeId, u32), u32>,
    raw_active: BTreeSet<NodeId>,
    active: BTreeSet<NodeId>,
}

impl ScaleRoutes {
    // zl.m sees References through physical Slots. A nested Group's Contents
    // use its own numbering and do not contribute in the enclosing scope.
    fn raw_references<E: 'static>(
        &mut self,
        node: &PointwiseNode<E>,
    ) -> Result<u32, PointwiseError<E>> {
        if let Some(&refs) = self.raw.get(&node.id) {
            return Ok(refs);
        }
        if !self.raw_active.insert(node.id) {
            return Err(PointwiseError::UnsupportedCapability(
                "cyclic scale reference discovery",
            ));
        }
        let refs = if node.kind() == Some(PointwiseKind::Reference) {
            node.reference_slot_at(None)?.map_or(0, slot_bit)
        } else {
            let mut refs = 0;
            for child in node.slots().into_iter().filter_map(|slot| slot.node()) {
                refs |= self.raw_references(&child)?;
            }
            refs
        };
        self.raw_active.remove(&node.id);
        self.raw.insert(node.id, refs);
        Ok(refs)
    }

    fn reached_references<E: 'static>(
        &mut self,
        node: &PointwiseNode<E>,
        flags: u32,
    ) -> Result<u32, PointwiseError<E>> {
        let key = (node.id, flags);
        if let Some(&refs) = self.reached.get(&key) {
            return Ok(refs);
        }
        if !self.active.insert(node.id) {
            return Err(PointwiseError::UnsupportedCapability(
                "cyclic scale reachability",
            ));
        }
        let refs = if node.kind() == Some(PointwiseKind::Reference) {
            node.reference_slot_at(None)?.map_or(0, slot_bit)
        } else {
            let slots = inputs(node);
            let enabled = if node.kind().is_some_and(PointwiseKind::is_group) {
                self.group_flags(node.content(0)?.node(), &slots, flags)?
            } else {
                self.slot_flags(node, &slots, flags)?
            };
            let mut refs = 0;
            for (index, child) in slots.iter().enumerate() {
                if enabled & slot_bit(index) != 0
                    && let Some(child) = child
                {
                    refs |= self.reached_references(child, flags)?;
                }
            }
            refs
        };
        self.active.remove(&node.id);
        self.reached.insert(key, refs);
        Ok(refs)
    }

    fn slot_flags<E: 'static>(
        &mut self,
        node: &PointwiseNode<E>,
        slots: &[Option<PointwiseNode<E>>],
        inherited: u32,
    ) -> Result<u32, PointwiseError<E>> {
        let mut enabled = 0;
        for (index, child) in slots.iter().enumerate() {
            if let Some(child) = child
                && (!conditional_input(node, index)?
                    || (inherited != 0
                        && self.reached_references(child, inherited)? & inherited != 0))
            {
                enabled |= slot_bit(index);
            }
        }
        Ok(enabled)
    }

    fn group_flags<E: 'static>(
        &mut self,
        content: Option<PointwiseNode<E>>,
        slots: &[Option<PointwiseNode<E>>],
        inherited: u32,
    ) -> Result<u32, PointwiseError<E>> {
        let Some(content) = content else {
            return Ok(ALL_INPUTS);
        };
        // Unused and empty inputs start enabled. Once a Reference is reached
        // unconditionally, its bit can open another conditional route.
        let mut enabled = ALL_INPUTS & !self.raw_references(&content)?;
        for (index, child) in slots.iter().enumerate() {
            if child.is_none() {
                enabled |= slot_bit(index);
            }
        }
        loop {
            let expanded = enabled | self.reached_references(&content, enabled)?;
            if expanded == enabled {
                break;
            }
            enabled = expanded;
        }
        if inherited != 0 {
            for (index, child) in slots.iter().enumerate() {
                let bit = slot_bit(index);
                if enabled & bit == 0
                    && let Some(child) = child
                    && self.reached_references(child, inherited)? & inherited != 0
                {
                    enabled |= bit;
                }
            }
        }
        Ok(enabled)
    }
}

fn inputs<E>(node: &PointwiseNode<E>) -> Vec<Option<PointwiseNode<E>>> {
    node.slots().into_iter().map(|slot| slot.node()).collect()
}

fn conditional_input<E: 'static>(
    node: &PointwiseNode<E>,
    index: usize,
) -> Result<bool, PointwiseError<E>> {
    use PointwiseKind::*;
    Ok(match node.kind() {
        Some(Regions) if index == 1 => {
            let filter = node.filter().ok_or(PointwiseError::InvalidCast)?;
            crate::pointwise::regions::image_scale_is_conditional(&filter)?
        }
        Some(kind) if transform::owns(kind) => node
            .filter()
            .ok_or(PointwiseError::InvalidCast)?
            .transform_accepts_scale(index as i32)?,
        Some(ReflectEnvironment | RefractEnvironment | ShapeBlur) => index == 1,
        Some(Stereogram) => index == 0,
        Some(FullLight) => index == 2,
        _ => false,
    })
}

pub(in crate::pointwise) fn reaches_scaled_reference<E: 'static>(
    node: &PointwiseNode<E>,
    inputs: &[bool],
) -> Result<bool, PointwiseError<E>> {
    let flags = inputs.iter().enumerate().fold(
        0,
        |mask, (index, &set)| {
            if set { mask | slot_bit(index) } else { mask }
        },
    );
    Ok(flags != 0 && ScaleRoutes::default().reached_references(node, flags)? & flags != 0)
}

struct ScaleVisit<E> {
    node: PointwiseNode<E>,
    flags: u32,
    incoming: f64,
    factor: f64,
}

impl<E> ScaleVisit<E> {
    fn flags(&self) -> [bool; GROUP_INPUTS] {
        std::array::from_fn(|index| self.flags & slot_bit(index) != 0)
    }
}

fn plan<E: 'static>(
    root: &PointwiseNode<E>,
    scale: f64,
    authored: bool,
) -> Result<Vec<ScaleVisit<E>>, PointwiseError<E>> {
    enum Step<E> {
        Enter(PointwiseNode<E>, u32, f64),
        Leave(NodeId),
    }

    let mut routes = ScaleRoutes::default();
    let mut visits = Vec::new();
    let mut active = BTreeSet::new();
    let mut pending = vec![Step::Enter(root.clone(), 0, scale)];
    while let Some(step) = pending.pop() {
        let (node, flags, incoming) = match step {
            Step::Enter(node, flags, incoming) => (node, flags, incoming),
            Step::Leave(node) => {
                active.remove(&node);
                continue;
            }
        };
        if !active.insert(node.id) {
            return Err(PointwiseError::UnsupportedCapability(
                "cyclic graph scaling",
            ));
        }
        if node.is_clone_mode() {
            return Err(PointwiseError::UnsupportedCapability(
                "scale traversal on a graph clone",
            ));
        }
        let is_lab = matches!(node.kind(), Some(PointwiseKind::TemplateLab(_)));
        let factor = if is_lab && !authored {
            let lab = node.filter().ok_or(PointwiseError::InvalidCast)?;
            incoming * template_lab::scale_multiplier(lab.parameter(template_lab::SCALE)?.number())
        } else {
            incoming
        };
        let slots = inputs(&node);
        let content = if !(is_lab && authored) && node.kind().is_some_and(PointwiseKind::is_group) {
            node.content(0)?.node()
        } else {
            None
        };
        let enabled = if is_lab && authored {
            ALL_INPUTS
        } else if content.is_some() {
            routes.group_flags(content.clone(), &slots, flags)?
        } else {
            routes.slot_flags(&node, &slots, flags)?
        };
        pending.push(Step::Leave(node.id));
        for (index, child) in slots.into_iter().enumerate().rev() {
            if enabled & slot_bit(index) != 0
                && let Some(child) = child
            {
                pending.push(Step::Enter(child, flags, factor));
            }
        }
        if let Some(content) = content {
            pending.push(Step::Enter(content, enabled, factor));
        }
        visits.push(ScaleVisit {
            node,
            flags,
            incoming,
            factor,
        });
    }
    Ok(visits)
}

impl<E: 'static> PointwiseNode<E> {
    /// Set the retained render scale for this branch, including conditional
    /// traversal of inputs through Groups and References. Render dimensions
    /// and authored Size values are independent of this transient scale.
    pub fn set_graph_scale(&self, scale: f64) -> Result<(), PointwiseError<E>> {
        self.apply_render_changes(plan(self, scale, false)?, |visit| {
            let scale = visit.factor;
            if let Some(generator) = visit.node.image_generator_handle() {
                let changed = generator.factor() != scale;
                generator.set_local_node_scale(scale)?;
                Ok(changed)
            } else if let Some(filter) = visit.node.filter() {
                let changed = filter.factor() != scale;
                if matches!(filter.kind(), PointwiseKind::TemplateLab(_)) {
                    filter.set_lab_inherited_scale(visit.incoming);
                }
                if transform::owns(filter.kind()) {
                    filter.set_local_transform_scale(scale, Some(&visit.flags()))?;
                } else if filter.kind() == PointwiseKind::Polygon {
                    filter.set_local_polygon_scale(scale, Some(&visit.flags()))?;
                } else {
                    if matches!(
                        filter.kind(),
                        PointwiseKind::Smear | PointwiseKind::LegacyBricksBoards
                    ) {
                        filter
                            .parameter(0)?
                            .set_size_scale(scale, Some(&visit.flags()))?;
                    }
                    if filter.kind() == PointwiseKind::Canvas {
                        filter
                            .parameter(3)?
                            .set_size_scale(scale, Some(&visit.flags()))?;
                    }
                    filter.set_local_factor(scale);
                }
                Ok(changed)
            } else {
                Ok(false)
            }
        })
    }

    /// Apply a factor to the authored spatial properties reached through this
    /// branch. Calls the ordinary property setters in graph order and
    /// scales active Size curve bounds through the same owners.
    pub fn apply_graph_size_scale(&self, scale: f64) -> Result<(), PointwiseError<E>> {
        self.apply_render_changes(plan(self, scale, true)?, |visit| {
            if let Some(generator) = visit.node.image_generator_handle() {
                generator.apply_size_scale(scale)?;
                Ok(true)
            } else if let Some(filter) = visit.node.filter() {
                if matches!(filter.kind(), PointwiseKind::TemplateLab(_)) {
                    let control = filter.parameter(template_lab::SCALE)?;
                    let multiplier = template_lab::scale_multiplier(control.number()) * scale;
                    control.set_number(template_lab::scale_control_value(multiplier))?;
                } else if transform::owns(filter.kind()) {
                    filter.apply_transform_size_scale_in_graph(scale, &visit.flags())?;
                } else if filter.kind() == PointwiseKind::Polygon {
                    filter.apply_polygon_size_scale(scale)?;
                } else {
                    if filter.kind() == PointwiseKind::LegacyBricksBoards {
                        crate::pointwise::legacy_bricks::commit_size(&filter, scale)?;
                    }
                    if filter.kind() == PointwiseKind::Smear {
                        let size = filter.parameter(0)?;
                        let [x, y] = [size.extent(0)? * scale, size.extent(1)? * scale];
                        size.set_extent(0, x)?;
                        size.set_extent(1, y)?;
                    }
                    filter.commit_spatial_scale(scale)?;
                }
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }

    /// Apply an inherited Seed offset to each physical input and contents
    /// branch. Seed values on the nodes remain authored and serializable.
    pub fn set_graph_seed_modifier(&self, modifier: f64) -> Result<(), PointwiseError<E>> {
        enum Step<E> {
            Enter(PointwiseNode<E>, f64),
            Leave(NodeId),
        }
        let mut pending = vec![Step::Enter(self.clone(), modifier)];
        let mut active = BTreeSet::new();
        let mut visits = Vec::new();
        while let Some(step) = pending.pop() {
            let (node, incoming) = match step {
                Step::Enter(node, incoming) => (node, incoming),
                Step::Leave(id) => {
                    active.remove(&id);
                    continue;
                }
            };
            if !active.insert(node.id) {
                return Err(PointwiseError::UnsupportedCapability(
                    "cyclic graph seed propagation",
                ));
            }
            let modifier = if matches!(node.kind(), Some(PointwiseKind::TemplateLab(_))) {
                let lab = node.filter().ok_or(PointwiseError::InvalidCast)?;
                parameter_ops::seed_modifier(
                    lab.parameter(template_lab::SEED)?.number(),
                    incoming,
                    true,
                )
            } else {
                incoming
            };
            pending.push(Step::Leave(node.id));
            pending.extend(
                node.slots()
                    .into_iter()
                    .chain(node.contents())
                    .rev()
                    .filter_map(|slot| slot.node())
                    .map(|child| Step::Enter(child, modifier)),
            );
            visits.push((node, incoming, modifier));
        }
        self.apply_render_changes(visits, |(node, incoming, modifier)| {
            if let Some(generator) = node.image_generator_handle()
                && let Some(seed) = generator
                    .parameters()
                    .into_iter()
                    .find(|p| p.kind() == GradientGeneratorParameterKind::Seed)
            {
                let changed = seed.modifier()? != modifier;
                seed.set_modifier(modifier)?;
                return Ok(changed);
            }
            if let Some(filter) = node.filter() {
                if matches!(filter.kind(), PointwiseKind::TemplateLab(_)) {
                    filter.set_lab_inherited_seed(incoming);
                    return Ok(false);
                }
                if filter.kind() == PointwiseKind::WizardReference {
                    filter.set_lab_inherited_seed(incoming);
                    return Ok(false);
                }
                if let Some(seed) = filter.parameters().into_iter().find(|p| {
                    p.name().into_iter().eq("Seed".encode_utf16())
                        && matches!(p.raw_value(), PointwiseParameterValue::Number { .. })
                }) {
                    return seed.set_seed_modifier(modifier);
                }
            }
            Ok(false)
        })
    }

    fn apply_render_changes<T>(
        &self,
        visits: Vec<T>,
        mut apply: impl FnMut(T) -> Result<bool, PointwiseError<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let mut tree = self.clone();
        let mut ancestors = BTreeSet::from([tree.id]);
        while let Some(parent) = tree.parent() {
            if !ancestors.insert(parent.id) {
                return Err(PointwiseError::UnsupportedCapability(
                    "cyclic render change ancestry",
                ));
            }
            tree = parent;
        }
        let mut changed = false;
        let result: Result<(), PointwiseError<E>> = visits.into_iter().try_for_each(|visit| {
            changed |= apply(visit)?;
            Ok(())
        });
        if changed || result.is_err() {
            // An exterior input can feed a Reference after its own factor or
            // seed stays fixed. Clear the enclosing graph's cached images too.
            let cleared = tree.clear_mode_subtree(&mut BTreeSet::new());
            result?;
            cleared?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PointwiseFilter;

    fn filter(kind: PointwiseKind) -> PointwiseFilter {
        PointwiseFilter::new(kind).unwrap()
    }

    fn reference(number: f64) -> PointwiseFilter {
        let reference = filter(PointwiseKind::Reference);
        reference.parameter(0).unwrap().set_number(number).unwrap();
        reference
    }

    #[test]
    fn regions_scales_image_only_when_the_mode_allows_it() {
        let regions = filter(PointwiseKind::Regions);
        let root = regions.node();
        let pattern = crate::GradientGenerator::new().unwrap();
        let image = crate::GradientGenerator::new().unwrap();
        root.slot(0)
            .unwrap()
            .set_node(Some(&pattern.node()))
            .unwrap();
        root.slot(1).unwrap().set_node(Some(&image.node())).unwrap();
        let size = image.parameter(GradientGeneratorParameterKind::Size);
        for (mode, conditional) in [
            (0, false),
            (4, false),
            (5, true),
            (6, true),
            (7, true),
            (8, false),
        ] {
            // Reset with an unconditional route. An excluded input keeps the previous factor.
            regions.parameter(2).unwrap().set_index(4).unwrap();
            root.set_graph_scale(1.).unwrap();
            regions.parameter(2).unwrap().set_index(mode).unwrap();
            root.set_graph_scale(3.).unwrap();
            let expected = if conditional { 1. } else { 3. };
            assert_eq!(image.factor(), expected);
            assert_eq!(pattern.factor(), 3.);
            assert_eq!(size.pair().unwrap(), [1.; 2]);
            assert_eq!(image.normalized_size(), [expected; 2]);
        }
        regions.parameter(2).unwrap().set_index(5).unwrap();
        regions
            .parameter(2)
            .unwrap()
            .set_axis_animated(0, true)
            .unwrap();
        root.set_graph_scale(4.).unwrap();
        assert_eq!(image.factor(), 4.);
    }

    #[test]
    fn shared_reference_routes_scale_a_conditional_regions_input() {
        let group = filter(PointwiseKind::Group);
        group.parameter(0).unwrap().set_number(2.).unwrap();
        let root = group.node();
        let pattern = crate::GradientGenerator::new().unwrap();
        let image = crate::GradientGenerator::new().unwrap();
        root.slot(0)
            .unwrap()
            .set_node(Some(&pattern.node()))
            .unwrap();
        root.slot(1).unwrap().set_node(Some(&image.node())).unwrap();

        let regions = filter(PointwiseKind::Regions);
        regions.parameter(2).unwrap().set_index(5).unwrap();
        let pattern_ref = reference(1.);
        let image_ref = reference(2.);
        let region_node = regions.node();
        region_node
            .slot(0)
            .unwrap()
            .set_node(Some(&pattern_ref.node()))
            .unwrap();
        region_node
            .slot(1)
            .unwrap()
            .set_node(Some(&image_ref.node()))
            .unwrap();
        root.content(0)
            .unwrap()
            .set_node(Some(&region_node))
            .unwrap();
        let extra = crate::GradientGenerator::new().unwrap();
        group
            .append_group_content([0.; 2])
            .unwrap()
            .set_node(Some(&extra.node()))
            .unwrap();

        root.set_graph_scale(2.).unwrap();
        assert_eq!(pattern.factor(), 2.);
        assert_eq!(image.factor(), 1.);
        assert_eq!(pattern_ref.factor(), 2.);
        assert_eq!(image_ref.factor(), 1.);
        root.apply_graph_size_scale(2.).unwrap();
        let size = |generator: &crate::GradientGenerator| {
            generator
                .parameter(GradientGeneratorParameterKind::Size)
                .pair()
                .unwrap()
        };
        assert_eq!(size(&pattern), [2.; 2]);
        assert_eq!(size(&image), [1.; 2]);

        let combine = filter(PointwiseKind::Combine).node();
        root.content(0).unwrap().set_node(Some(&combine)).unwrap();
        combine
            .slot(0)
            .unwrap()
            .set_node(Some(&region_node))
            .unwrap();
        combine
            .slot(1)
            .unwrap()
            .set_node(Some(&reference(2.).node()))
            .unwrap();
        root.set_graph_scale(3.).unwrap();
        assert_eq!(pattern.factor(), 3.);
        assert_eq!(image.factor(), 3.);
        assert_eq!(image_ref.factor(), 3.);
        assert_eq!(extra.factor(), 1.);
        root.apply_graph_size_scale(3.).unwrap();
        assert_eq!(size(&pattern), [6.; 2]);
        assert_eq!(size(&image), [3.; 2]);
        assert_eq!(size(&extra), [1.; 2]);
        assert_eq!(image.factor(), 3.);
    }

    #[test]
    fn wizard_group_with_empty_contents_scales_its_exterior_inputs() {
        let group = filter(PointwiseKind::WizardGroup);
        group.parameter(0).unwrap().set_number(1.).unwrap();
        let graph = group.node();
        let source = crate::GradientGenerator::new().unwrap();
        graph
            .slot(0)
            .unwrap()
            .set_node(Some(&source.node()))
            .unwrap();
        graph.set_graph_scale(2.).unwrap();
        graph.apply_graph_size_scale(3.).unwrap();
        let size = source.parameter(GradientGeneratorParameterKind::Size);
        assert_eq!(size.pair().unwrap(), [3.; 2]);
        assert_eq!(source.normalized_size(), [6.; 2]);
    }

    #[test]
    fn authored_scale_retains_repeat_size_when_the_reference_is_shared() {
        let group = filter(PointwiseKind::Group);
        let graph = group.node();
        let source = crate::GradientGenerator::new().unwrap();
        graph
            .slot(0)
            .unwrap()
            .set_node(Some(&source.node()))
            .unwrap();
        let repeat = filter(PointwiseKind::Kaleidoscope);
        repeat.parameter(0).unwrap().set_pair([2., 3.]).unwrap();
        repeat
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&reference(1.).node()))
            .unwrap();
        graph
            .content(0)
            .unwrap()
            .set_node(Some(&repeat.node()))
            .unwrap();
        graph.apply_graph_size_scale(2.).unwrap();
        assert_eq!(repeat.parameter(0).unwrap().pair().unwrap(), [4., 6.]);

        let add = filter(PointwiseKind::Add).node();
        graph.content(0).unwrap().set_node(Some(&add)).unwrap();
        add.slot(0).unwrap().set_node(Some(&repeat.node())).unwrap();
        add.slot(1)
            .unwrap()
            .set_node(Some(&reference(1.).node()))
            .unwrap();
        graph.apply_graph_size_scale(2.).unwrap();
        assert_eq!(repeat.parameter(0).unwrap().pair().unwrap(), [4., 6.]);
        assert_eq!(
            source
                .parameter(GradientGeneratorParameterKind::Size)
                .pair()
                .unwrap(),
            [2.; 2]
        );
    }

    #[test]
    fn seed_modifier_reaches_slots_and_extra_contents_without_changing_authored_values() {
        let group = filter(PointwiseKind::Group);
        let graph = group.node();
        let noise = PointwiseNode::image_generator(crate::ImageGeneratorKind::Noise).unwrap();
        let noise_seed = noise
            .image_generator_handle()
            .unwrap()
            .parameter(GradientGeneratorParameterKind::Seed);
        noise_seed.set_number(999_999.).unwrap();
        graph.slot(0).unwrap().set_node(Some(&noise)).unwrap();

        let regions = filter(PointwiseKind::Regions);
        let regions_seed = regions.parameter(4).unwrap();
        regions_seed.set_number(999_999.).unwrap();
        graph
            .content(0)
            .unwrap()
            .set_node(Some(&regions.node()))
            .unwrap();
        let atmosphere = filter(PointwiseKind::Atmosphere);
        let atmosphere_seed = atmosphere.parameter(5).unwrap();
        atmosphere_seed.set_number(17.).unwrap();
        group
            .append_group_content([20., 30.])
            .unwrap()
            .set_node(Some(&atmosphere.node()))
            .unwrap();

        let save = |node: &PointwiseNode| {
            let mut writer = crate::BinaryWriter::default();
            node.save_binary(&mut writer).unwrap();
            writer.bytes().to_vec()
        };
        let authored = save(&graph);
        graph.set_graph_seed_modifier(2.).unwrap();
        assert_eq!(noise_seed.effective_seed_at(0.).unwrap(), 1.);
        assert_eq!(regions_seed.effective_seed_at(0.).unwrap(), 1.);
        assert_eq!(atmosphere_seed.effective_seed_at(0.).unwrap(), 19.);
        assert_eq!(save(&graph), authored);
        graph.set_graph_seed_modifier(-3.).unwrap();
        assert_eq!(noise_seed.effective_seed_at(0.).unwrap(), -2.);
        assert_eq!(regions_seed.effective_seed_at(0.).unwrap(), 1.);
        assert_eq!(atmosphere_seed.effective_seed_at(0.).unwrap(), 19.);
        graph.set_graph_seed_modifier(0.).unwrap();
        assert_eq!(noise_seed.effective_seed_at(0.).unwrap(), 999_999.);
        assert_eq!(regions_seed.effective_seed_at(0.).unwrap(), 999_999.);
        assert_eq!(atmosphere_seed.effective_seed_at(0.).unwrap(), 17.);

        atmosphere_seed.set_axis_animated(0, true).unwrap();
        let sample = atmosphere_seed.effective_seed_at(0.75).unwrap();
        graph.set_graph_seed_modifier(99.).unwrap();
        assert_eq!(atmosphere_seed.effective_seed_at(0.75).unwrap(), sample);
    }

    #[test]
    fn filter_seed_capture_preserves_modifier_and_fingerprints_changes() {
        let regions = filter(PointwiseKind::Regions);
        let node = regions.node();
        let capture = |node: &PointwiseNode| {
            crate::RenderCaptureSession::new()
                .capture_filter(&node.filter().unwrap())
                .unwrap()
        };
        let before = capture(&node);
        let seed = regions.parameter(4).unwrap();
        node.set_graph_seed_modifier(3.).unwrap();
        let after = capture(&node);
        assert!(!before.same_render_values(&after));
        assert_eq!(seed.number(), 0.);

        let copy = node.source_clone().unwrap();
        let seed = copy.filter().unwrap().parameter(4).unwrap();
        assert_eq!(seed.effective_seed_at(0.).unwrap(), 3.);
        assert_eq!(seed.number(), 0.);
    }

    #[test]
    fn authored_scale_visits_images_in_a_group_with_3d_contents() {
        let group = filter(PointwiseKind::Group).node();
        let noise = PointwiseNode::image_generator(crate::ImageGeneratorKind::Noise).unwrap();
        group.slot(0).unwrap().set_node(Some(&noise)).unwrap();
        let cube = crate::ScenePrimitiveNode::new(crate::ScenePrimitiveKind::Cube).unwrap();
        let transform = crate::SceneTransformNode::new().unwrap();
        transform
            .node()
            .slot(0)
            .unwrap()
            .set_node(Some(&cube.node()))
            .unwrap();
        group
            .content(0)
            .unwrap()
            .set_node(Some(&transform.node()))
            .unwrap();
        let size = cube.property(1).unwrap();
        size.set_vector3d([3., 4., 5.]).unwrap();
        let before = size.evaluated_value(0.).unwrap();

        group.apply_graph_size_scale(2.).unwrap();
        assert_eq!(
            noise
                .image_generator_handle()
                .unwrap()
                .parameter(GradientGeneratorParameterKind::Size)
                .pair()
                .unwrap(),
            [2.; 2]
        );
        assert_eq!(size.evaluated_value(0.).unwrap(), before);
    }
}
