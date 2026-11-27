use super::{PointwiseKind, PointwiseNode};
use crate::pointwise::region::NodeId;
use std::collections::{BTreeMap, BTreeSet};

/// Progress estimates the share of graph work completed, using the actual
/// evaluator's pending operations. A shortcut consumes its entire share.
/// Scheduling children splits the current share without moving backwards.
pub(super) struct GraphProgress {
    weights: BTreeMap<NodeId, u64>,
    pending: Vec<f64>,
    current: Option<(usize, f64)>,
    completed: f64,
}

impl GraphProgress {
    pub(super) fn new<E: 'static>(root: &PointwiseNode<E>) -> Self {
        let mut weights = BTreeMap::<NodeId, u64>::new();
        let mut visiting = BTreeSet::new();
        let mut pending = vec![(root.clone(), None::<Vec<NodeId>>)];
        while let Some((node, children)) = pending.pop() {
            if let Some(children) = children {
                let weight = children
                    .into_iter()
                    .fold(Self::kernel(&node), |sum: u64, id| {
                        sum.saturating_add(weights.get(&id).copied().unwrap_or(1))
                    });
                weights.insert(node.id, weight);
                visiting.remove(&node.id);
            } else if !weights.contains_key(&node.id) && visiting.insert(node.id) {
                let children: Vec<_> = node
                    .slots()
                    .into_iter()
                    .chain(node.contents())
                    .filter_map(|slot| slot.node())
                    .chain(
                        (node.kind() == Some(PointwiseKind::Reference))
                            .then(|| node.reference_target().ok().flatten())
                            .flatten(),
                    )
                    .collect();
                pending.push((
                    node,
                    Some(children.iter().map(|child| child.id).collect::<Vec<_>>()),
                ));
                pending.extend(children.into_iter().map(|child| (child, None)));
            }
        }
        Self {
            weights,
            pending: vec![1.],
            current: None,
            completed: 0.,
        }
    }

    pub(super) fn branch<E>(&self, node: &PointwiseNode<E>) -> u64 {
        self.weights.get(&node.id).copied().unwrap_or(1)
    }

    pub(super) fn kernel<E: 'static>(node: &PointwiseNode<E>) -> u64 {
        match node.kind() {
            Some(
                PointwiseKind::GaussianBlur
                | PointwiseKind::Frequency
                | PointwiseKind::Fractalize
                | PointwiseKind::Distort
                | PointwiseKind::Matrix
                | PointwiseKind::WizardMatrix
                | PointwiseKind::Canvas
                | PointwiseKind::Style
                | PointwiseKind::Synthesis,
            ) => 8,
            Some(kind) if kind.is_group() || kind == PointwiseKind::Reference => 1,
            _ if node.image_generator_handle().is_some() || node.is_scene_node() => 8,
            _ => 3,
        }
    }

    pub(super) fn resolve<T>(&mut self, stack: &[T], weight: impl Fn(&Self, &T) -> u64) {
        let Some((previous_len, budget)) = self.current.take() else {
            return;
        };
        let scheduled = &stack[previous_len..];
        if scheduled.is_empty() {
            self.completed += budget;
            return;
        }
        let weights: Vec<_> = scheduled
            .iter()
            .map(|step| weight(self, step) as f64)
            .collect();
        let total: f64 = weights.iter().sum();
        self.pending
            .extend(weights.into_iter().map(|weight| budget * weight / total));
    }

    pub(super) fn start(&mut self, remaining: usize) -> f32 {
        self.current = Some((remaining, self.pending.pop().expect("render work")));
        self.completed.min(0.999) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PointwiseRenderEffects, ProceduralExecution, RenderCaptureSession};

    #[test]
    fn render_reports_monotonic_progress_from_graph_evaluation() {
        let source = PointwiseNode::<()>::solid_color().unwrap();
        let invert = PointwiseNode::invert();
        invert.slot(0).unwrap().set_node(Some(&source)).unwrap();
        invert.set_invert_red_only().unwrap();
        let snapshot = RenderCaptureSession::new()
            .capture_node(&invert, 0.)
            .unwrap();
        let progress = std::cell::RefCell::new(Vec::new());
        let report = |fraction, node: &str| progress.borrow_mut().push((fraction, node.to_owned()));
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_graph_progress(Some(&report));
        let result = snapshot
            .render_with_execution_and_effects(
                8,
                8,
                ProceduralExecution::CallingThread,
                crate::PointwiseRenderOptions::new(false, true),
                &mut effects,
            )
            .unwrap();
        assert!(result.caught_errors().is_empty());
        let progress = progress.into_inner();
        assert_eq!(progress.first().unwrap().0, 0.);
        assert!(progress.last().unwrap().0 > 0.);
        assert!(progress.windows(2).all(|steps| steps[0].0 <= steps[1].0));
        assert!(progress.iter().any(|(_, node)| node == "Solid Color"));
    }
}
