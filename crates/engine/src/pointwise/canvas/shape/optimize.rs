//! Refit Canvas curves with the same cubic fitter used by Freehand.
use super::*;

impl CanvasPath {
    pub(in crate::pointwise::canvas) fn segment_count(&self) -> usize {
        self.contours.iter().map(Contour::segment_count).sum()
    }

    pub(in crate::pointwise::canvas) fn optimized(&self, strength: f32) -> Option<Self> {
        let original_count = self.segment_count();
        if original_count <= 1 || !(0. ..=100.).contains(&strength) {
            return None;
        }
        let threshold = f64::from(strength).powi(4) / 1_000_000.;
        let mut contours = Vec::new();
        let mut reduced = false;
        for mut group in self.separate_curves() {
            let segment_count = group.segment_count();
            if segment_count == 1 {
                continue;
            }
            for contour in &mut group.contours {
                contour.prepare_curve_refit();
            }
            let CanvasShape::Shape(shape) = CanvasShape::from_canvas_path::<()>(group).ok()? else {
                unreachable!()
            };
            let Shape { base, details, .. } = *shape;
            let ShapeDetails::Path(mut group) = details else {
                unreachable!()
            };
            let shift = Self::tile_shift(&base);
            let closed = !group.is_open();
            let mut samples = Vec::new();
            for contour in &mut group.contours {
                contour.restore_collapsed_tangents();
                contour.append_refit_samples(shift, &mut samples);
            }
            if let Some(points) = freehand::fit_to_error(&samples, threshold) {
                reduced |= points.len() < 4 * segment_count;
                contours.push(Contour { closed, points });
            }
        }
        contours.sort_by_key(|contour| contour.closed);
        let candidate = Self {
            width: self.width,
            contours,
        };
        (reduced && candidate.segment_count() < original_count && candidate.valid())
            .then_some(candidate)
    }
}

impl Contour {
    fn segment_count(&self) -> usize {
        self.points.len().saturating_sub(usize::from(!self.closed))
    }

    fn append_refit_samples(&self, shift: [f64; 2], samples: &mut Vec<[f32; 2]>) {
        let mut segment = Vec::new();
        for (points, straight) in self.segment_controls(shift, true) {
            let points = points.map(|point| point.map(quantize_point));
            segment.clear();
            if straight {
                segment.push(points[0]);
            } else {
                subdivide(&mut segment, points, 0, OutlineSampling::Scanline);
            }
            segment.push(points[3]);
            samples.extend(segment.iter().map(|point| point.map(|value| value as f32)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(record: &str) -> CanvasPath {
        let contours = record
            .split('|')
            .map(|contour| {
                let (kind, values) = contour.split_once(':').unwrap();
                let points = values
                    .split(';')
                    .map(|vertex| {
                        let (coordinates, mode) = vertex.rsplit_once(',').unwrap();
                        let coordinates = coordinates
                            .split(',')
                            .map(|value| value.parse::<f32>().unwrap())
                            .collect::<Vec<_>>()
                            .try_into()
                            .unwrap();
                        CanvasPathPoint {
                            coordinates,
                            mode: mode.parse().unwrap(),
                            flags: [false; 3],
                        }
                    })
                    .collect();
                Contour {
                    closed: kind == "c",
                    points,
                }
            })
            .collect();
        CanvasPath {
            width: 8.5,
            contours,
        }
    }

    #[test]
    fn canvas_optimize_matches_original_curve_refits() {
        let capture =
            include_str!("../../../../../../../port/evidence/canvas-optimize-original.tsv");
        for line in capture.lines().skip(2) {
            let [name, strength, input, expected]: [&str; 4] =
                line.split('\t').collect::<Vec<_>>().try_into().unwrap();
            let actual = path(input).optimized(strength.parse().unwrap());
            if expected == "-" {
                assert!(actual.is_none(), "{name}: {actual:?}");
                continue;
            }
            let actual = actual.unwrap_or_else(|| panic!("{name}: missing refit"));
            let expected = path(expected);
            assert_eq!(actual.stroke_width(), expected.stroke_width(), "{name}");
            assert_eq!(actual.contours.len(), expected.contours.len(), "{name}");
            for (a, b) in actual.contours.iter().zip(expected.contours) {
                assert_eq!(a.closed, b.closed, "{name}");
                assert_eq!(a.points.len(), b.points.len(), "{name}");
                for (a, b) in a.points.iter().zip(b.points) {
                    assert_eq!(a.kind(), b.kind(), "{name}");
                    for (left, right) in a.coordinates.into_iter().zip(b.coordinates) {
                        assert!((left - right).abs() <= 0.00015, "{name}: {left} != {right}");
                    }
                }
            }
        }
    }
}
