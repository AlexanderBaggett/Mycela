//! Authoring commands for the ordered shape hierarchy within a Canvas layer.
use super::{
    CanvasDocument, CanvasLayerEditor, CanvasPath, CanvasPrimitive, CanvasShape, CanvasShapeBounds,
    ItemHeader, PointwiseError, PointwiseFilter,
    selection::{CanvasShapeEditor, visit_shapes},
    shape::{MAX_ITEM_DEPTH, ShapeGroup},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// An item in a layer's current shape tree. The identity survives edits and history replay.
#[derive(Clone)]
pub struct CanvasItem {
    identity: Arc<()>,
    name: String,
    kind: &'static str,
    depth: usize,
    group: bool,
    visible: bool,
    position: usize,
    sibling_count: usize,
}

impl CanvasItem {
    /// A process-local key, valid as long as an item handle is held.
    pub fn identity(&self) -> usize {
        Arc::as_ptr(&self.identity) as usize
    }

    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            self.kind
        } else {
            &self.name
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn is_group(&self) -> bool {
        self.group
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn can_move(&self, offset: isize) -> bool {
        self.position
            .checked_add_signed(offset)
            .is_some_and(|target| target < self.sibling_count && target != self.position)
    }
}

#[derive(Clone)]
pub enum CanvasItemPlacement {
    Layer,
    Into(CanvasItem),
    Before(CanvasItem),
    After(CanvasItem),
}

struct ItemMove {
    destination: usize,
    sources: Vec<(usize, Vec<usize>)>,
}

impl<E: 'static> CanvasLayerEditor<E> {
    /// The hierarchy in front-to-back order, with each group followed by its descendants.
    pub fn items(&self) -> Vec<CanvasItem> {
        let Ok((document, layer)) = self.current_layer() else {
            return Vec::new();
        };
        let mut items = Vec::new();
        collect_items(&document.layers[layer].shapes, 0, &mut items);
        items
    }

    /// Resolves an item to its editable leaves in layer order for geometry edits.
    pub fn item_shape_editors(&self, item: &CanvasItem) -> Vec<CanvasShapeEditor<E>> {
        let Ok((document, layer)) = self.current_layer() else {
            return Vec::new();
        };
        let root = &document.layers[layer].shapes;
        let Some(selected) = find_path(root, &item.identity).and_then(|path| {
            let (&index, parent) = path.split_last()?;
            items_at(root, parent)?.get(index)
        }) else {
            return Vec::new();
        };
        let mut shapes = Vec::new();
        visit_shapes(std::slice::from_ref(selected), true, &mut |shape, _| {
            shapes.push(CanvasShapeEditor::at_layer(self, shape, shapes.len() + 1));
        });
        shapes
    }

    pub(super) fn item_children(&self, item: &CanvasItem) -> Vec<CanvasItem> {
        let Ok((document, layer)) = self.current_layer() else {
            return Vec::new();
        };
        let root = &document.layers[layer].shapes;
        let Some(children) = find_path(root, &item.identity).and_then(|path| items_at(root, &path))
        else {
            return Vec::new();
        };
        let mut items = Vec::new();
        collect_items(children, item.depth + 1, &mut items);
        items.retain(|child| child.depth == item.depth + 1);
        items
    }

    pub fn can_group_items(&self, selection: &[CanvasItem], primary: &CanvasItem) -> bool {
        self.current_layer().is_ok_and(|(document, layer)| {
            group_placement(&document.layers[layer].shapes, selection, primary).is_some()
        })
    }

    /// Groups items from this layer at the frontmost selected sibling of the primary item.
    /// Selected descendants of a selected group travel with their ancestor.
    pub fn edit_group_items(
        &self,
        selection: &[CanvasItem],
        primary: &CanvasItem,
    ) -> Result<CanvasItem, PointwiseError<E>> {
        let (before, layer) = self.current_layer()?;
        let placement = group_placement(&before.layers[layer].shapes, selection, primary)
            .ok_or(PointwiseError::InvalidOperation)?;
        let mut after = (*before).clone();
        let root = &mut after.layers[layer].shapes;
        let mut children = Vec::with_capacity(placement.paths.len());
        for (path, order) in placement.paths.iter().rev() {
            let (&index, parent) = path.split_last().ok_or(PointwiseError::InvalidOperation)?;
            children.push((
                *order,
                items_at_mut(root, parent)
                    .ok_or(PointwiseError::InvalidOperation)?
                    .remove(index),
            ));
        }
        children.sort_unstable_by_key(|(order, _)| *order);
        let destination = match placement.parent {
            Some(parent) => {
                let path = find_path(root, &parent).ok_or(PointwiseError::InvalidOperation)?;
                items_at_mut(root, &path).ok_or(PointwiseError::InvalidOperation)?
            }
            None => root,
        };
        let identity = Arc::new(());
        destination.insert(
            placement.index,
            CanvasShape::Group(ShapeGroup {
                identity: identity.clone(),
                header: ItemHeader::new("Group"),
                children: children.into_iter().map(|(_, child)| child).collect(),
            }),
        );
        for layer in &mut after.layers {
            prune_small_groups(&mut layer.shapes);
        }
        let root = &after.layers[layer].shapes;
        if !fits_wire_depth(root, 0) {
            return Err(PointwiseError::Argument);
        }
        let mut items = Vec::new();
        collect_items(root, 0, &mut items);
        let group = items
            .into_iter()
            .find(|item| Arc::ptr_eq(&item.identity, &identity))
            .ok_or(PointwiseError::InvalidOperation)?;
        self.owner
            .commit_canvas_edit(before, after, "Group Canvas items")?;
        Ok(group)
    }

    pub fn edit_ungroup_item(&self, item: &CanvasItem) -> Result<(), PointwiseError<E>> {
        self.edit_item_siblings(&item.identity, "Ungroup Canvas items", |siblings, index| {
            if !matches!(siblings[index], CanvasShape::Group(_)) {
                return false;
            }
            let CanvasShape::Group(group) = siblings.remove(index) else {
                unreachable!();
            };
            siblings.splice(index..index, group.children);
            true
        })
    }

    pub fn edit_move_item(
        &self,
        item: &CanvasItem,
        offset: isize,
    ) -> Result<(), PointwiseError<E>> {
        self.edit_item_siblings(&item.identity, "Move Canvas item", |siblings, index| {
            let Some(target) = index
                .checked_add_signed(offset)
                .filter(|&target| target < siblings.len() && target != index)
            else {
                return false;
            };
            let item = siblings.remove(index);
            siblings.insert(target, item);
            true
        })
    }

    pub fn edit_item_visibility(
        &self,
        item: &CanvasItem,
        visible: bool,
    ) -> Result<(), PointwiseError<E>> {
        self.edit_item_siblings(
            &item.identity,
            "Change Canvas item visibility",
            |siblings, index| {
                let (current, header) = match &mut siblings[index] {
                    CanvasShape::Group(group) => {
                        (group.header.container_visible(), &mut group.header)
                    }
                    CanvasShape::Shape(shape) => (shape.header.visible(), &mut shape.header),
                    CanvasShape::Layer(_) => return false,
                };
                if current == visible {
                    return false;
                }
                header.set_visible(visible);
                true
            },
        )
    }

    pub fn edit_item_name(&self, item: &CanvasItem, name: &str) -> Result<bool, PointwiseError<E>> {
        let (before, layer) = self.current_layer()?;
        let root = &before.layers[layer].shapes;
        let path = find_path(root, &item.identity).ok_or(PointwiseError::InvalidOperation)?;
        let (&index, parent) = path.split_last().ok_or(PointwiseError::InvalidOperation)?;
        let header = match items_at(root, parent).and_then(|items| items.get(index)) {
            Some(CanvasShape::Group(group)) => &group.header,
            Some(CanvasShape::Shape(shape)) => &shape.header,
            _ => return Err(PointwiseError::InvalidOperation),
        };
        if header.name.iter().copied().eq(name.encode_utf16()) {
            return Ok(false);
        }
        let mut after = (*before).clone();
        let siblings = items_at_mut(&mut after.layers[layer].shapes, parent)
            .ok_or(PointwiseError::InvalidOperation)?;
        match &mut siblings[index] {
            CanvasShape::Group(group) => group.header.name = name.encode_utf16().collect(),
            CanvasShape::Shape(shape) => shape.header.name = name.encode_utf16().collect(),
            CanvasShape::Layer(_) => return Err(PointwiseError::InvalidOperation),
        }
        self.owner
            .commit_canvas_edit(before, after, "Rename Canvas item")?;
        Ok(true)
    }

    pub fn edit_insert_group_shape(
        &self,
        group: &CanvasItem,
        primitive: CanvasPrimitive,
        bounds: CanvasShapeBounds,
    ) -> Result<(), PointwiseError<E>> {
        self.insert_group_item(
            group,
            CanvasShape::primitive(primitive, bounds),
            primitive.name(),
        )
    }

    pub fn edit_insert_group_path(
        &self,
        group: &CanvasItem,
        path: CanvasPath,
    ) -> Result<(), PointwiseError<E>> {
        self.insert_group_item(group, CanvasShape::from_canvas_path(path)?, "Curve")
    }

    fn insert_group_item(
        &self,
        group: &CanvasItem,
        item: CanvasShape,
        label: &str,
    ) -> Result<(), PointwiseError<E>> {
        self.edit_item_siblings(
            &group.identity,
            &format!("Add {label}"),
            |siblings, index| {
                let CanvasShape::Group(group) = &mut siblings[index] else {
                    return false;
                };
                group.children.insert(0, item);
                true
            },
        )
    }

    pub fn item_outlines(&self, selection: &[CanvasItem]) -> Vec<Vec<[f64; 2]>> {
        if selection.is_empty() {
            return Vec::new();
        }
        let Ok((document, layer)) = self.current_layer() else {
            return Vec::new();
        };
        let layer = &document.layers[layer];
        let mut outlines = Vec::new();
        if layer.header.container_visible() {
            let selected = selection.iter().map(CanvasItem::identity).collect();
            append_selected_outlines::<E>(&layer.shapes, &selected, false, &mut outlines);
        }
        outlines
    }

    pub(super) fn edit_item_siblings(
        &self,
        identity: &Arc<()>,
        label: &str,
        edit: impl FnOnce(&mut Vec<CanvasShape>, usize) -> bool,
    ) -> Result<(), PointwiseError<E>> {
        let (before, layer) = self.current_layer()?;
        let mut after = (*before).clone();
        let root = &mut after.layers[layer].shapes;
        let path = find_path(root, identity).ok_or(PointwiseError::InvalidOperation)?;
        let (&index, parent) = path.split_last().ok_or(PointwiseError::InvalidOperation)?;
        let siblings = items_at_mut(root, parent).ok_or(PointwiseError::InvalidOperation)?;
        if !edit(siblings, index) {
            return Err(PointwiseError::InvalidOperation);
        }
        self.owner.commit_canvas_edit(before, after, label)
    }
}

impl<E: 'static> PointwiseFilter<E> {
    pub fn can_move_canvas_items(
        &self,
        selection: &[(CanvasLayerEditor<E>, CanvasItem)],
        layer: &CanvasLayerEditor<E>,
        placement: &CanvasItemPlacement,
    ) -> bool {
        let Some(before) = self.canvas_document() else {
            return false;
        };
        self.plan_item_move(&before, selection, layer, placement)
            .is_some_and(|plan| !plan.sources.is_empty())
    }

    /// Moves items in tree order, preserving their identities across layers.
    pub fn edit_move_canvas_items(
        &self,
        selection: &[(CanvasLayerEditor<E>, CanvasItem)],
        layer: &CanvasLayerEditor<E>,
        placement: &CanvasItemPlacement,
    ) -> Result<bool, PointwiseError<E>> {
        let before = self.canvas_document().ok_or(PointwiseError::InvalidCast)?;
        let ItemMove {
            destination,
            sources,
        } = self
            .plan_item_move(&before, selection, layer, placement)
            .ok_or(PointwiseError::InvalidOperation)?;
        if sources.is_empty() {
            return Ok(false);
        }
        let mut after = (*before).clone();
        let mut moved = Vec::with_capacity(sources.len());
        for (layer, path) in sources.iter().rev() {
            let (&index, parent) = path.split_last().ok_or(PointwiseError::InvalidOperation)?;
            let siblings = items_at_mut(&mut after.layers[*layer].shapes, parent)
                .ok_or(PointwiseError::InvalidOperation)?;
            moved.push(siblings.remove(index));
        }
        let root = &mut after.layers[destination].shapes;
        let (parent, index) =
            destination_position(root, placement).ok_or(PointwiseError::InvalidOperation)?;
        let siblings = items_at_mut(root, &parent).ok_or(PointwiseError::InvalidOperation)?;
        siblings.splice(index..index, moved.into_iter().rev());
        for layer in &mut after.layers {
            prune_small_groups(&mut layer.shapes);
            if !fits_wire_depth(&layer.shapes, 0) {
                return Err(PointwiseError::Argument);
            }
        }
        self.commit_canvas_edit(before, after, "Move Canvas items")?;
        Ok(true)
    }

    fn plan_item_move(
        &self,
        before: &CanvasDocument,
        selection: &[(CanvasLayerEditor<E>, CanvasItem)],
        layer: &CanvasLayerEditor<E>,
        placement: &CanvasItemPlacement,
    ) -> Option<ItemMove> {
        if selection.is_empty() || !self.node().same_reference(&layer.owner.node()) {
            return None;
        }
        let destination = layer.index_in(before)?;
        let root = &before.layers[destination].shapes;
        let (parent, index) = destination_position(root, placement)?;
        let anchor = match placement {
            CanvasItemPlacement::Layer => None,
            CanvasItemPlacement::Into(item)
            | CanvasItemPlacement::Before(item)
            | CanvasItemPlacement::After(item) => Some(find_path(root, &item.identity)?),
        };
        let mut by_layer: HashMap<usize, HashMap<usize, usize>> = HashMap::new();
        for (order, (layer, item)) in selection.iter().enumerate() {
            if !self.node().same_reference(&layer.owner.node()) {
                return None;
            }
            by_layer
                .entry(layer.index_in(before)?)
                .or_default()
                .insert(item.identity(), order);
        }
        let mut sources = Vec::new();
        for (layer, mut selected) in by_layer {
            let mut paths = Vec::new();
            collect_paths(
                &before.layers[layer].shapes,
                &mut selected,
                &mut Vec::new(),
                false,
                &mut paths,
            );
            if !selected.is_empty() {
                return None;
            }
            for (path, _) in paths {
                if layer == destination
                    && anchor
                        .as_ref()
                        .is_some_and(|anchor| anchor.starts_with(&path))
                {
                    return None;
                }
                sources.push((layer, path));
            }
        }
        sources.sort_unstable();
        let preceding = sources
            .iter()
            .filter(|(layer, path)| {
                *layer == destination
                    && path
                        .split_last()
                        .is_some_and(|(&slot, branch)| branch == parent && slot < index)
            })
            .count();
        let start = index - preceding;
        if sources.iter().enumerate().all(|(offset, (layer, path))| {
            *layer == destination
                && path
                    .split_last()
                    .is_some_and(|(&slot, branch)| branch == parent && slot == start + offset)
        }) {
            sources.clear();
        }
        Some(ItemMove {
            destination,
            sources,
        })
    }

    /// Deletes selected groups and individual items in one Canvas history change.
    pub fn edit_remove_canvas_items(
        &self,
        selection: &[(CanvasLayerEditor<E>, CanvasItem)],
    ) -> Result<(), PointwiseError<E>> {
        if selection.is_empty() {
            return Ok(());
        }
        let before = self.canvas_document().ok_or(PointwiseError::InvalidCast)?;
        let mut pending = HashSet::new();
        for (layer, item) in selection {
            if !self.node().same_reference(&layer.owner.node()) {
                return Err(PointwiseError::InvalidOperation);
            }
            let index = layer
                .index_in(&before)
                .ok_or(PointwiseError::InvalidOperation)?;
            if find_path(&before.layers[index].shapes, &item.identity).is_none() {
                return Err(PointwiseError::InvalidOperation);
            }
            pending.insert(item.identity());
        }
        self.remove_canvas_shapes(before, pending)
    }
}

impl CanvasShape {
    fn hierarchy_identity(&self) -> Option<&Arc<()>> {
        match self {
            Self::Group(group) => Some(&group.identity),
            Self::Shape(shape) => Some(&shape.identity),
            Self::Layer(_) => None,
        }
    }
}

fn collect_items(items: &[CanvasShape], depth: usize, result: &mut Vec<CanvasItem>) {
    for (position, item) in items.iter().enumerate() {
        let (identity, header, kind, children) = match item {
            CanvasShape::Group(group) => (
                &group.identity,
                &group.header,
                "Group",
                Some(&group.children),
            ),
            CanvasShape::Shape(shape) => {
                let kind = CanvasPrimitive::from_tag(shape.kind).map_or(
                    if shape.kind == 21 { "Text" } else { "Curve" },
                    CanvasPrimitive::name,
                );
                (&shape.identity, &shape.header, kind, None)
            }
            CanvasShape::Layer(_) => continue,
        };
        let name = String::from_utf16_lossy(&header.name);
        result.push(CanvasItem {
            identity: identity.clone(),
            name,
            kind,
            depth,
            group: children.is_some(),
            visible: if children.is_some() {
                header.container_visible()
            } else {
                header.visible()
            },
            position,
            sibling_count: items.len(),
        });
        if let Some(children) = children {
            collect_items(children, depth + 1, result);
        }
    }
}

fn destination_position(
    root: &[CanvasShape],
    placement: &CanvasItemPlacement,
) -> Option<(Vec<usize>, usize)> {
    match placement {
        CanvasItemPlacement::Layer => Some((Vec::new(), 0)),
        CanvasItemPlacement::Into(item) => {
            let path = find_path(root, &item.identity)?;
            items_at(root, &path)?;
            Some((path, 0))
        }
        CanvasItemPlacement::Before(item) | CanvasItemPlacement::After(item) => {
            let mut path = find_path(root, &item.identity)?;
            let anchor = path.pop()?;
            Some((
                path,
                anchor + usize::from(matches!(placement, CanvasItemPlacement::After(_))),
            ))
        }
    }
}

struct Placement {
    parent: Option<Arc<()>>,
    index: usize,
    paths: Vec<(Vec<usize>, usize)>,
}

fn group_placement(
    root: &[CanvasShape],
    items: &[CanvasItem],
    primary: &CanvasItem,
) -> Option<Placement> {
    let mut selected: HashMap<_, _> = items
        .iter()
        .enumerate()
        .map(|(order, item)| (item.identity(), order))
        .collect();
    if selected.len() < 2 || !selected.contains_key(&primary.identity()) {
        return None;
    }
    let mut paths = Vec::new();
    collect_paths(root, &mut selected, &mut Vec::new(), false, &mut paths);
    if paths.len() < 2 || !selected.is_empty() {
        return None;
    }
    let primary_path = find_path(root, &primary.identity)?;
    let (active, _) = paths
        .iter()
        .find(|(path, _)| primary_path.starts_with(path))?;
    let (_, parent_path) = active.split_last()?;
    let (parent, siblings) = if let Some((&parent_index, above)) = parent_path.split_last() {
        let siblings = items_at(root, above)?;
        let CanvasShape::Group(parent) = siblings.get(parent_index)? else {
            return None;
        };
        (Some(parent.identity.clone()), parent.children.as_slice())
    } else {
        (None, root)
    };
    let mut indices = paths.iter().filter_map(|(path, _)| {
        let (&index, candidate) = path.split_last()?;
        (parent_path == candidate).then_some(index)
    });
    let index = indices.next()?;
    if parent.is_some() && 1 + indices.count() == siblings.len() && siblings.len() == paths.len() {
        return None;
    }
    Some(Placement {
        parent,
        index,
        paths,
    })
}

fn collect_paths(
    items: &[CanvasShape],
    selection: &mut HashMap<usize, usize>,
    prefix: &mut Vec<usize>,
    ancestor_selected: bool,
    result: &mut Vec<(Vec<usize>, usize)>,
) {
    for (index, item) in items.iter().enumerate() {
        let Some(identity) = item.hierarchy_identity() else {
            continue;
        };
        prefix.push(index);
        let selected = selection.remove(&(Arc::as_ptr(identity) as usize));
        if let Some(order) = selected.filter(|_| !ancestor_selected) {
            result.push((prefix.clone(), order));
        }
        if let CanvasShape::Group(group) = item {
            collect_paths(
                &group.children,
                selection,
                prefix,
                ancestor_selected || selected.is_some(),
                result,
            );
        }
        prefix.pop();
    }
}

pub(super) fn find_path(items: &[CanvasShape], identity: &Arc<()>) -> Option<Vec<usize>> {
    fn search(items: &[CanvasShape], identity: &Arc<()>, path: &mut Vec<usize>) -> bool {
        for (index, item) in items.iter().enumerate() {
            path.push(index);
            if item
                .hierarchy_identity()
                .is_some_and(|current| Arc::ptr_eq(current, identity))
                || matches!(item, CanvasShape::Group(group) if search(&group.children, identity, path))
            {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = Vec::new();
    search(items, identity, &mut path).then_some(path)
}

pub(super) fn items_at<'a>(items: &'a [CanvasShape], path: &[usize]) -> Option<&'a [CanvasShape]> {
    let mut children = items;
    for &index in path {
        let CanvasShape::Group(group) = children.get(index)? else {
            return None;
        };
        children = &group.children;
    }
    Some(children)
}

pub(super) fn items_at_mut<'a>(
    items: &'a mut Vec<CanvasShape>,
    path: &[usize],
) -> Option<&'a mut Vec<CanvasShape>> {
    let mut children = items;
    for &index in path {
        let CanvasShape::Group(group) = children.get_mut(index)? else {
            return None;
        };
        children = &mut group.children;
    }
    Some(children)
}

pub(super) fn prune_small_groups(items: &mut Vec<CanvasShape>) {
    let mut index = 0;
    while let Some(item) = items.get_mut(index) {
        match item {
            CanvasShape::Group(group) => {
                prune_small_groups(&mut group.children);
                match group.children.len() {
                    0 => {
                        items.remove(index);
                        continue;
                    }
                    1 => *item = group.children.remove(0),
                    _ => {}
                }
            }
            CanvasShape::Layer(layer) => prune_small_groups(&mut layer.shapes),
            CanvasShape::Shape(_) => {}
        }
        index += 1;
    }
}

fn fits_wire_depth(items: &[CanvasShape], depth: usize) -> bool {
    depth < MAX_ITEM_DEPTH
        && items.iter().all(|item| match item {
            CanvasShape::Group(group) => fits_wire_depth(&group.children, depth + 1),
            CanvasShape::Layer(layer) => fits_wire_depth(&layer.shapes, depth + 1),
            CanvasShape::Shape(_) => true,
        })
}

fn append_selected_outlines<E>(
    items: &[CanvasShape],
    selection: &HashSet<usize>,
    ancestor_selected: bool,
    outlines: &mut Vec<Vec<[f64; 2]>>,
) {
    for item in items {
        let selected = ancestor_selected
            || item
                .hierarchy_identity()
                .is_some_and(|identity| selection.contains(&(Arc::as_ptr(identity) as usize)));
        match item {
            CanvasShape::Group(group) if group.header.container_visible() => {
                append_selected_outlines::<E>(&group.children, selection, selected, outlines);
            }
            CanvasShape::Shape(shape) if selected && shape.header.visible() && shape.editable() => {
                if let Ok(contours) = shape.guide_outline::<E>() {
                    outlines.extend(contours);
                }
            }
            _ => {}
        }
    }
}
