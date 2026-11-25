//! Atomic merge, separation, and optimization commands for selected Curve shapes.
use super::*;
use crate::pointwise::canvas::hierarchy::{find_path, items_at, items_at_mut};

impl<E: 'static> CanvasShapeEditor<E> {
    pub fn is_curve(&self) -> bool {
        self.curve_contour_count().is_some()
    }

    pub fn can_separate_curve(&self) -> bool {
        self.curve_contour_count().is_some_and(|count| count > 1)
    }

    pub fn can_optimize_curve(&self) -> bool {
        let Some(document) = self.layer.owner.canvas_document() else {
            return false;
        };
        self.shape_in(&document)
            .and_then(Shape::path)
            .is_some_and(|path| path.segment_count() > 1)
    }

    fn curve_contour_count(&self) -> Option<usize> {
        let document = self.layer.owner.canvas_document()?;
        Some(self.shape_in(&document)?.path()?.contours().count())
    }

    /// Replaces the selected curve with separate shapes at its sibling position.
    pub fn edit_separate_curve(&self) -> Result<Vec<Self>, PointwiseError<E>> {
        let (before, layer) = self.layer.current_layer()?;
        let shape = self
            .shape_in(&before)
            .ok_or(PointwiseError::InvalidOperation)?;
        let curve = shape.path().ok_or(PointwiseError::InvalidOperation)?;
        if curve.contours().count() < 2 {
            return Err(PointwiseError::InvalidOperation);
        }
        let pieces = curve.separate_curves();
        let mut items = Vec::with_capacity(pieces.len());
        let mut editors = Vec::with_capacity(pieces.len());
        // In the original each piece is inserted at the same sibling index.
        for path in pieces.into_iter().rev() {
            let item = CanvasShape::from_canvas_path_with_mode(path, shape.combine_mode())?;
            editors.push(self.replacement_editor(&item));
            items.push(item);
        }

        let mut after = (*before).clone();
        let root = &mut after.layers[layer].shapes;
        let path = find_path(root, &self.identity).ok_or(PointwiseError::InvalidOperation)?;
        let (&index, parent) = path.split_last().ok_or(PointwiseError::InvalidOperation)?;
        let siblings = items_at_mut(root, parent).ok_or(PointwiseError::InvalidOperation)?;
        siblings.splice(index..=index, items);
        self.layer
            .owner
            .commit_canvas_edit(before, after, "Separate Canvas Curve")?;
        Ok(editors)
    }

    fn replacement_editor(&self, item: &CanvasShape) -> Self {
        let CanvasShape::Shape(shape) = item else {
            unreachable!()
        };
        Self {
            identity: shape.identity.clone(),
            layer: self.layer.clone(),
            label: format!("{} / Curve", self.layer.label()),
        }
    }
}

impl<E: 'static> PointwiseFilter<E> {
    /// Replaces each successfully refitted curve at its original sibling index.
    pub fn edit_optimize_canvas_curves(
        &self,
        selection: &[CanvasShapeEditor<E>],
        strength: f32,
    ) -> Result<Vec<CanvasShapeEditor<E>>, PointwiseError<E>> {
        if !(0. ..=100.).contains(&strength) {
            return Err(PointwiseError::Argument);
        }
        let before = self.canvas_document().ok_or(PointwiseError::InvalidCast)?;
        let mut after = None;
        let mut edited = Vec::new();
        let mut seen = HashSet::new();
        for editor in selection {
            if !self.node().same_reference(&editor.layer.owner.node()) {
                return Err(PointwiseError::InvalidOperation);
            }
            if !seen.insert(editor.identity()) {
                continue;
            }
            let shape = editor
                .shape_in(&before)
                .ok_or(PointwiseError::InvalidOperation)?;
            let Some(path) = shape.path().and_then(|path| path.optimized(strength)) else {
                continue;
            };
            let layer = editor
                .layer
                .index_in(&before)
                .ok_or(PointwiseError::InvalidOperation)?;
            let document = after.get_or_insert_with(|| (*before).clone());
            let root = &mut document.layers[layer].shapes;
            let location =
                find_path(root, &editor.identity).ok_or(PointwiseError::InvalidOperation)?;
            let (&index, parent) = location
                .split_last()
                .ok_or(PointwiseError::InvalidOperation)?;
            let siblings = items_at_mut(root, parent).ok_or(PointwiseError::InvalidOperation)?;
            let item = CanvasShape::from_canvas_path_with_mode(path, shape.combine_mode())?;
            edited.push(editor.replacement_editor(&item));
            siblings[index] = item;
        }
        if let Some(after) = after {
            self.commit_canvas_edit(before, after, "Optimize Canvas Curves")?;
        }
        Ok(edited)
    }

    /// Merges the selected Curve shapes at the active curve's saved sibling index.
    pub fn edit_merge_canvas_curves(
        &self,
        selection: &[CanvasShapeEditor<E>],
        active: &CanvasShapeEditor<E>,
    ) -> Result<CanvasShapeEditor<E>, PointwiseError<E>> {
        if !self.node().same_reference(&active.layer.owner.node()) {
            return Err(PointwiseError::InvalidOperation);
        }
        let (before, layer) = active.layer.current_layer()?;
        let Some(active_path) = active.shape_in(&before).and_then(Shape::path) else {
            return Err(PointwiseError::InvalidOperation);
        };

        let mut curves = Vec::new();
        let mut selected = HashSet::new();
        let mut combine = 6;
        for editor in selection {
            if !self.node().same_reference(&editor.layer.owner.node()) {
                return Err(PointwiseError::InvalidOperation);
            }
            let shape = editor
                .shape_in(&before)
                .ok_or(PointwiseError::InvalidOperation)?;
            if let Some(path) = shape.path()
                && selected.insert(editor.identity())
            {
                curves.push(path);
                combine = shape.combine_mode();
            }
        }
        if curves.len() < 2 || !selected.contains(&active.identity()) {
            return Err(PointwiseError::InvalidOperation);
        }
        let result = CanvasPath::merge_curves(curves, active_path.stroke_width());
        let item = CanvasShape::from_canvas_path_with_mode(result, combine)?;
        let merged = active.replacement_editor(&item);
        let root = &before.layers[layer].shapes;
        let location = find_path(root, &active.identity).ok_or(PointwiseError::InvalidOperation)?;
        let (&index, parent) = location
            .split_last()
            .ok_or(PointwiseError::InvalidOperation)?;
        let parent = if let Some((&parent_index, above)) = parent.split_last() {
            let Some(CanvasShape::Group(group)) =
                items_at(root, above).and_then(|items| items.get(parent_index))
            else {
                return Err(PointwiseError::InvalidOperation);
            };
            Some(group.identity.clone())
        } else {
            None
        };

        let mut after = (*before).clone();
        for layer in &mut after.layers {
            remove_shapes(&mut layer.shapes, &mut selected);
        }
        if !selected.is_empty() {
            return Err(PointwiseError::InvalidOperation);
        }
        let root = &mut after.layers[layer].shapes;
        let siblings = if let Some(parent) = parent {
            let path = find_path(root, &parent).ok_or(PointwiseError::InvalidOperation)?;
            items_at_mut(root, &path).ok_or(PointwiseError::InvalidOperation)?
        } else {
            root
        };
        siblings.insert(index.min(siblings.len()), item);
        self.commit_canvas_edit(before, after, "Merge Canvas Curves")?;
        Ok(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CanvasPathHandle, CanvasPathPoint, PointwiseKind};

    fn path(recipe: &str) -> CanvasPath {
        let fields: Vec<_> = recipe.split(',').collect();
        let [x, y, w, h]: [f32; 4] = fields[1..]
            .iter()
            .map(|value| value.parse().unwrap())
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        if fields[0] == "circle" {
            let k = 0.552_284_8 * w;
            let points = [
                ([x - w, y], [x - w, y - k]),
                ([x, y - w], [x + k, y - w]),
                ([x + w, y], [x + w, y + k]),
                ([x, y + w], [x - k, y + w]),
            ]
            .into_iter()
            .map(|(at, outgoing)| {
                let mut point = CanvasPathPoint::smooth(at, outgoing);
                point.move_handle(CanvasPathHandle::Outgoing, outgoing, true);
                point
            })
            .collect();
            return CanvasPath::from_points(points, true).unwrap();
        }
        let corners = if fields[0] == "line" {
            vec![[x, y], [w, h]]
        } else {
            vec![[x, y], [w, y], [w, h], [x, h]]
        };
        let closed = corners.len() == 4;
        let mut path = CanvasPath::from_points(
            corners.into_iter().map(CanvasPathPoint::corner).collect(),
            closed,
        )
        .unwrap();
        *path.point_mut(0, 0).unwrap() = CanvasPathPoint::corner([x, y]);
        path
    }

    fn signature(path: &CanvasPath) -> String {
        path.contours()
            .map(|(closed, points)| {
                let [x, y] = points[0].position();
                format!("{},{x},{y}", if closed { "closed" } else { "open" })
            })
            .collect::<Vec<_>>()
            .join(";")
    }

    #[test]
    fn canvas_compound_curve_commands_match_original() {
        let evidence =
            include_str!("../../../../../../../port/evidence/canvas-compound-original.tsv");
        for row in evidence.lines().skip(2) {
            let [case, recipes, merged_shape, siblings, width, combine]: [&str; 6] =
                row.split('\t').collect::<Vec<_>>().try_into().unwrap();
            let filter = PointwiseFilter::<()>::new(PointwiseKind::Canvas).unwrap();
            let layer = filter.edit_add_canvas_layer().unwrap();
            let recipes: Vec<_> = recipes.split(';').collect();
            for (index, recipe) in recipes.iter().enumerate().rev() {
                let path = CanvasPath::merge_curves([&path(recipe)], (index + 2) as f32);
                let mode = if index == recipes.len() - 1 { 2 } else { 6 };
                layer
                    .insert_item(
                        CanvasShape::from_canvas_path_with_mode::<()>(path, mode).unwrap(),
                        "Curve",
                    )
                    .unwrap();
            }
            let selection = filter.canvas_shape_editors();
            let editor = filter
                .edit_merge_canvas_curves(&selection, &selection[0])
                .unwrap();
            assert_eq!(filter.canvas_shape_editors().len(), 1, "{case}");
            assert!(
                selection.iter().all(|source| source.path().is_none()),
                "{case}"
            );
            let path = editor.path().unwrap();
            let width = width.parse::<f32>().unwrap();
            let combine = combine.parse::<i32>().unwrap();
            assert_eq!(signature(&path), merged_shape, "{case}");
            assert_eq!(path.stroke_width(), width, "{case}");
            let pieces = editor.edit_separate_curve().unwrap();
            let order: Vec<_> = filter
                .canvas_shape_editors()
                .iter()
                .map(CanvasShapeEditor::identity)
                .collect();
            assert_eq!(
                order,
                pieces
                    .iter()
                    .map(CanvasShapeEditor::identity)
                    .collect::<Vec<_>>(),
                "{case}"
            );
            let document = filter.canvas_document().unwrap();
            for piece in &pieces {
                assert_eq!(
                    piece.shape_in(&document).unwrap().combine_mode(),
                    combine,
                    "{case}"
                );
                assert_eq!(piece.path().unwrap().stroke_width(), width, "{case}");
            }
            let signature = pieces
                .iter()
                .map(|piece| signature(&piece.path().unwrap()))
                .collect::<Vec<_>>()
                .join("|");
            assert_eq!(signature, siblings, "{case}");
            assert!(editor.path().is_none(), "{case}");
        }
    }
}
