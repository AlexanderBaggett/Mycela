//! Stable Canvas hierarchy selection and its editable shape projection.
use super::*;

pub(super) type SelectionItem = (Layer, CanvasItem);

#[derive(Clone, Default)]
pub(super) struct Selection {
    pub(super) items: Vec<SelectionItem>,
    pub(super) shapes: Vec<CanvasShape>,
}

impl Selection {
    pub(super) fn from_shapes(shapes: Vec<CanvasShape>) -> Self {
        let items = shapes
            .iter()
            .filter_map(|shape| branch(shape).pop())
            .collect();
        Self { items, shapes }
    }

    pub(super) fn from_items(
        items: Vec<SelectionItem>,
        available: &HashMap<usize, CanvasShape>,
    ) -> Self {
        let mut seen = HashSet::new();
        let mut shapes = Vec::new();
        for (layer, item) in &items {
            if item.is_group() {
                shapes.extend(
                    layer
                        .item_shape_editors(item)
                        .into_iter()
                        .filter_map(|shape| {
                            seen.insert(shape.identity())
                                .then(|| available.get(&shape.identity()).cloned())
                                .flatten()
                        }),
                );
            } else if seen.insert(item.identity())
                && let Some(shape) = available.get(&item.identity())
            {
                shapes.push(shape.clone());
            }
        }
        Self { items, shapes }
    }

    pub(super) fn active_shape(&self) -> Option<CanvasShape> {
        let (_, item) = self.items.last()?;
        self.shapes
            .iter()
            .find(|shape| shape.identity() == item.identity())
            .cloned()
    }

    pub(super) fn active_layer(&self) -> Option<Layer> {
        self.items.last().map(|(layer, _)| layer.clone())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(super) fn has_group(&self) -> bool {
        self.items.iter().any(|(_, item)| item.is_group())
    }

    pub(super) fn clicked(&self, path: &[SelectionItem], additive: bool) -> Vec<SelectionItem> {
        let Some(first) = path.first() else {
            return Vec::new();
        };
        if !additive {
            let index = path
                .iter()
                .position(|(_, candidate)| {
                    self.items
                        .iter()
                        .any(|(_, current)| current.identity() == candidate.identity())
                })
                .map_or(0, |index| (index + 1) % path.len());
            return vec![path[index].clone()];
        }
        let mut items = self.items.clone();
        if let Some(index) = items
            .iter()
            .position(|(_, item)| item.identity() == first.1.identity())
        {
            items.remove(index);
        } else {
            let descendants = subtree(&first.0.items(), first.1.identity())
                .iter()
                .map(CanvasItem::identity)
                .collect::<HashSet<_>>();
            items.retain(|(_, item)| !descendants.contains(&item.identity()));
            items.push(first.clone());
        }
        items
    }
}

pub(super) fn branch(shape: &CanvasShape) -> Vec<SelectionItem> {
    let layer = shape.layer();
    let mut path: Vec<CanvasItem> = Vec::new();
    for item in layer.items() {
        path.truncate(item.depth());
        let hit = item.identity() == shape.identity();
        path.push(item);
        if hit {
            return path.into_iter().map(|item| (layer.clone(), item)).collect();
        }
    }
    Vec::new()
}

pub(super) fn subtree(items: &[CanvasItem], identity: usize) -> &[CanvasItem] {
    let Some(start) = items.iter().position(|item| item.identity() == identity) else {
        return &[];
    };
    let depth = items[start].depth();
    let end = items[start + 1..]
        .iter()
        .position(|item| item.depth() <= depth)
        .map_or(items.len(), |next| start + 1 + next);
    &items[start..end]
}

pub(super) fn marquee_items(layers: Vec<Layer>, enclosed: &HashSet<usize>) -> Vec<SelectionItem> {
    let mut selected = Vec::new();
    for layer in layers {
        let items = layer.items();
        promote(&layer, &items, enclosed, &mut selected);
    }
    selected
}

fn promote(
    layer: &Layer,
    mut items: &[CanvasItem],
    enclosed: &HashSet<usize>,
    result: &mut Vec<SelectionItem>,
) -> bool {
    let mut all = !items.is_empty();
    while let Some(item) = items.first() {
        let count = items[1..]
            .iter()
            .position(|child| child.depth() <= item.depth())
            .map_or(items.len(), |following| following + 1);
        let (branch, rest) = items.split_at(count);
        let before = result.len();
        let selected = enclosed.contains(&item.identity())
            || (item.is_group()
                && item.is_visible()
                && promote(layer, &branch[1..], enclosed, result));
        if selected {
            result.truncate(before);
            result.push((layer.clone(), item.clone()));
        }
        all &= selected;
        items = rest;
    }
    all
}
