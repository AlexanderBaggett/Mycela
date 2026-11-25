//! The authored Canvas coordinate grid used by editor gestures.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CanvasGridKind {
    #[default]
    Regular,
    Isometric,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasSnapGrid {
    kind: CanvasGridKind,
    spacing: [f32; 2],
}

impl CanvasSnapGrid {
    pub fn new(kind: CanvasGridKind, density: [u16; 2]) -> Option<Self> {
        density
            .into_iter()
            .all(|count| (1..=200).contains(&count))
            .then(|| Self {
                kind,
                spacing: density.map(|count| 100. / f32::from(count)),
            })
    }

    pub(super) fn regular_spacing(spacing: f32) -> Option<Self> {
        (spacing.is_finite() && spacing > 0.).then_some(Self {
            kind: CanvasGridKind::Regular,
            spacing: [spacing; 2],
        })
    }

    pub fn snap(self, [x, y]: [f32; 2]) -> [f32; 2] {
        let [width, height] = self.spacing;
        if self.kind == CanvasGridKind::Regular {
            return [(x / width).round() * width, (y / height).round() * height];
        }
        let left = (x / width).floor() * width;
        let top = (y / height).floor() * height;
        let right = left + width;
        let bottom = top + height;
        let candidates = [
            [left, top],
            [right, top],
            [right, bottom],
            [left, bottom],
            [0.5 * (left + right), 0.5 * (top + bottom)],
        ];
        let distance = |[a, b]: [f32; 2]| (x - a) * (x - a) + (y - b) * (y - b);
        // Keep the first point in the original candidate order when distances tie.
        candidates
            .into_iter()
            .min_by(|&a, &b| distance(a).total_cmp(&distance(b)))
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_snap_grid_matches_original_at_boundaries_and_ties() {
        let point = |text: &str| {
            let (x, y) = text.split_once(',').unwrap();
            [x.parse::<f32>().unwrap(), y.parse::<f32>().unwrap()]
        };
        let evidence =
            include_str!("../../../../../../port/evidence/canvas-snap-grid-original.tsv");
        for row in evidence.lines().skip(2) {
            let [case, kind, x, y, input, expected]: [&str; 6] =
                row.split('\t').collect::<Vec<_>>().try_into().unwrap();
            let kind = if kind == "regular" {
                CanvasGridKind::Regular
            } else {
                CanvasGridKind::Isometric
            };
            let grid = CanvasSnapGrid::new(kind, [x.parse().unwrap(), y.parse().unwrap()]).unwrap();
            assert_eq!(grid.snap(point(input)), point(expected), "{case}");
        }
        assert!(CanvasSnapGrid::new(CanvasGridKind::Regular, [0, 20]).is_none());
        assert!(CanvasSnapGrid::new(CanvasGridKind::Regular, [20, 201]).is_none());
    }
}
