//! Source Levels input histogram, sampled at the caller's preview resolution.
use super::{LevelsChannel, LevelsPoints, MINIMUM_RANGE, POINT_LIMIT};
use crate::{ImageError, SharedImageMap};

const BIN_COUNT: usize = 200;

pub struct LevelsInputHistogram {
    bounds: [f32; 2],
    bins: [[f32; BIN_COUNT]; 4],
    auto: [Option<LevelsPoints>; 4],
}

impl LevelsInputHistogram {
    pub fn from_image(image: &SharedImageMap) -> Result<Self, ImageError> {
        let red = image.materialize(0)?.values();
        let green = image.materialize(1)?.values();
        let blue = image.materialize(2)?.values();
        let rgb = red
            .iter()
            .zip(&green)
            .zip(&blue)
            .map(|((&r, &g), &b)| r / 3.0 + g / 3.0 + b / 3.0)
            .collect::<Vec<_>>();
        let planes = [rgb, red, green, blue];
        let [low, high] = planes[1..]
            .iter()
            .flatten()
            .copied()
            .filter(|value| value.is_finite())
            .fold([0.0_f32, 1.0_f32], |[low, high], value| {
                [
                    low.min(value.clamp(-POINT_LIMIT, POINT_LIMIT)),
                    high.max(value.clamp(-POINT_LIMIT, POINT_LIMIT)),
                ]
            });
        let bounds = [low, high];
        let step = (high - low) / (BIN_COUNT - 1) as f32;
        let bins = planes.each_ref().map(|plane| bin(plane, low, step));
        let mut auto = [None; 4];
        for (target, plane) in auto[1..].iter_mut().zip(&planes[1..]) {
            *target = auto_channel(plane, bounds);
        }
        auto[0] = auto[1..]
            .iter()
            .flatten()
            .map(|points| points.0)
            .reduce(|[black, _, white], [next_black, _, next_white]| {
                [black.min(next_black), 0.0, white.max(next_white)]
            })
            .map(|[black, _, white]| centered_points(black, white));
        Ok(Self { bounds, bins, auto })
    }

    pub fn bounds(&self) -> [f32; 2] {
        self.bounds
    }

    pub fn bins(&self, channel: LevelsChannel) -> &[f32] {
        &self.bins[channel.index()]
    }

    pub fn auto_points(&self, channel: LevelsChannel) -> Option<LevelsPoints> {
        self.auto[channel.index()]
    }
}

// avq gives the adjacent bins complementary fractional contributions. The
// scan uses the same order and Single increments as the original widget.
fn bin(plane: &[f32], low: f32, step: f32) -> [f32; BIN_COUNT] {
    let mut histogram = [0.0; BIN_COUNT];
    let mut left = low;
    let mut right = left + step;
    for bin in 0..BIN_COUNT {
        for &value in plane {
            if left <= value && value <= right {
                if bin == BIN_COUNT - 1 {
                    histogram[bin] += 1.0;
                } else {
                    histogram[bin] += (right - value) / step;
                    histogram[bin + 1] += (value - left) / step;
                }
            }
        }
        left += step;
        right += step;
    }
    histogram
}

fn auto_channel(plane: &[f32], bounds: [f32; 2]) -> Option<LevelsPoints> {
    let [low, high] = plane
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .fold([f32::MAX, f32::MIN], |[low, high], value| {
            [low.min(value), high.max(value)]
        });
    if low > high {
        return None;
    }
    // qo.r5 clips the selected channel's range to the chart. Auto then
    // scans bins generated on that channel's range, not the display bins.
    let low = low.max(bounds[0]);
    let high = high.min(bounds[1]);
    if low > high {
        return Some(LevelsPoints::DEFAULT);
    }
    let step = (high - low) / (BIN_COUNT - 1) as f32;
    let bins = bin(plane, low, if step == 0.0 { 1.0 } else { step });
    let [black, white] = occupied_range(&bins, low, step).unwrap_or([0.0, 1.0]);
    Some(if black == white {
        LevelsPoints([black, black + 0.005, white + MINIMUM_RANGE])
    } else {
        centered_points(black, white)
    })
}

fn occupied_range(bins: &[f32; BIN_COUNT], low: f32, step: f32) -> Option<[f32; 2]> {
    let mut position = low;
    let mut first = None;
    let mut last = low;
    for &count in bins {
        if count > 0.0 {
            first.get_or_insert(position);
            last = position;
        }
        position += step;
    }
    first.map(|black| [black, last])
}

fn centered_points(black: f32, white: f32) -> LevelsPoints {
    let midpoint = ((f64::from(black) + f64::from(white)) / 2.0) as f32;
    LevelsPoints([black, midpoint, white])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FloatBuffer, ImageGeometry};

    #[test]
    fn levels_input_samples_materialized_rgb_and_keeps_hdr_range() {
        let image = SharedImageMap::new(ImageGeometry::new(2, 1));
        image
            .set_buffer(0, Some(&FloatBuffer::new(vec![-0.5, 0.75])))
            .unwrap();
        image.set_constant(1, 0.5).unwrap();
        image
            .set_buffer(2, Some(&FloatBuffer::new(vec![0.125, 1.5])))
            .unwrap();
        let histogram = LevelsInputHistogram::from_image(&image).unwrap();
        assert_eq!(histogram.bounds(), [-0.5, 1.5]);
        let red = histogram.auto_points(LevelsChannel::Red).unwrap();
        let blue = histogram.auto_points(LevelsChannel::Blue).unwrap();
        let rgb = histogram.auto_points(LevelsChannel::Rgb).unwrap();
        assert_eq!(rgb.black_point(), red.black_point());
        assert_eq!(rgb.white_point(), blue.white_point());
        assert!(histogram.bins(LevelsChannel::Rgb)[0] == 0.0);
        assert!(histogram.bins(LevelsChannel::Red)[0] > 0.0);
        assert!(
            histogram
                .bins(LevelsChannel::Green)
                .iter()
                .filter(|&&count| count > 0.0)
                .count()
                <= 2
        );
        let fitted = histogram.auto_points(LevelsChannel::Green).unwrap();
        assert!(fitted.black_point() <= 0.5 && fitted.white_point() >= 0.5);
    }

    #[test]
    fn levels_auto_matches_original_per_channel_histogram_and_applied_points() {
        const OBSERVATIONS: &str =
            include_str!("../../../../../port/evidence/levels-auto-reference.tsv");
        let cases = [
            ("flat", [[0.25; 4], [0.5; 4], [0.75; 4]]),
            (
                "dense",
                [
                    [-0.5, 0.25, 0.75, 1.0],
                    [0.2, 0.4, 0.6, 0.8],
                    [1.875, 1.25, 0.25, -0.25],
                ],
            ),
            (
                "narrow",
                [
                    [0.25, 0.25025, 0.2525, 0.254],
                    [0.125, 0.7, 0.75, 1.0],
                    [0.345, 0.3, 0.25, 0.2],
                ],
            ),
            (
                "clipped",
                [
                    [-12.0, 0.25, 0.5, 15.0],
                    [-12.0, -12.1, -12.25, -12.125],
                    [12.0, 12.1, 12.25, 12.125],
                ],
            ),
        ];
        for (case, planes) in cases {
            let image = SharedImageMap::new(ImageGeometry::new(4, 1));
            for (index, plane) in planes.into_iter().enumerate() {
                image
                    .set_buffer(index as i32, Some(&FloatBuffer::new(plane.into())))
                    .unwrap();
            }
            let histogram = LevelsInputHistogram::from_image(&image).unwrap();
            let observations = OBSERVATIONS
                .lines()
                .skip(1)
                .filter(|line| line.starts_with(case))
                .collect::<Vec<_>>();
            assert_eq!(observations.len(), 4);
            for (channel, observation) in LevelsChannel::ALL.into_iter().zip(observations) {
                let columns = observation.split('\t').collect::<Vec<_>>();
                let expected = |offset: usize| -> [u32; 3] {
                    std::array::from_fn(|i| columns[offset + i].parse::<f32>().unwrap().to_bits())
                };
                let configured = histogram.auto_points(channel).unwrap();
                assert_eq!(
                    configured.0.map(f32::to_bits),
                    expected(4),
                    "{case} {channel:?} suggestion"
                );
                let applied = LevelsPoints::DEFAULT
                    .edited(super::super::LevelsControlEdit::Auto(configured))
                    .unwrap();
                assert_eq!(
                    applied.0.map(f32::to_bits),
                    expected(7),
                    "{case} {channel:?} applied"
                );
            }
        }
    }
}
