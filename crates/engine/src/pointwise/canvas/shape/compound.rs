//! Curve-shape assembly and separation in authored Canvas coordinates.
use super::*;
use crate::pointwise::canvas::distance::FilledContour;

impl CanvasPath {
    pub(in crate::pointwise::canvas) fn merge_curves<'a>(
        paths: impl IntoIterator<Item = &'a Self>,
        width: f32,
    ) -> Self {
        let mut contours: Vec<_> = paths
            .into_iter()
            .flat_map(|path| path.contours.iter().cloned())
            .collect();
        // The original enumerates loose-end chains before closed loops.
        contours.sort_by_key(|contour| contour.closed);
        Self { width, contours }
    }

    pub(in crate::pointwise::canvas) fn separate_curves(&self) -> Vec<Self> {
        let mut remaining: Vec<_> = (0..self.contours.len()).collect();
        remaining.sort_by_key(|&index| self.contours[index].closed);
        let fills: Vec<_> = self.contours.iter().map(Contour::filled_outline).collect();
        let mut groups = Vec::new();
        while let Some(seed) = remaining.pop() {
            let mut members = vec![seed];
            if fills[seed].is_some() {
                loop {
                    let current: Vec<_> = members
                        .iter()
                        .filter_map(|&index| fills[index].as_ref())
                        .collect();
                    let mut added = Vec::new();
                    remaining.retain(|&index| {
                        let joins = fills[index]
                            .as_ref()
                            .is_some_and(|fill| fill.overlaps(&current));
                        if joins {
                            added.push(index);
                        }
                        !joins
                    });
                    if added.is_empty() {
                        break;
                    }
                    members.extend(added);
                }
            }
            groups.push(members);
        }
        // If every contour ended up in one component, the original releases
        // the individual contours so the command always separates the shape.
        if let [members] = groups.as_slice() {
            groups = members.iter().map(|&index| vec![index]).collect();
        }
        groups
            .into_iter()
            .map(|members| Self {
                width: self.width,
                contours: members
                    .into_iter()
                    .map(|index| self.contours[index].clone())
                    .collect(),
            })
            .collect()
    }
}

impl Contour {
    fn filled_outline(&self) -> Option<FilledContour> {
        if !self.closed || self.points.len() < 2 {
            return None;
        }
        let mut outline = Vec::new();
        for (points, straight) in self.segment_controls([0.; 2], true) {
            if straight {
                outline.push(points[0]);
            } else {
                subdivide(&mut outline, points, 0, OutlineSampling::Distance);
            }
        }
        FilledContour::new(&outline)
    }
}
