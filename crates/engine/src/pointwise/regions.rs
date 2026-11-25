//! Source sj region detection and fill. Geometry is expressed on the render grid.
use super::{PointwiseChannelGuarantees, PointwiseError, PointwiseFilter, SharedImageMap};
use crate::{FloatBuffer, ImageError, ImageGeometry, SharedFloatStore};
#[path = "regions_contour.rs"]
mod contour;
#[path = "regions_from_map.rs"]
mod from_map;
#[path = "regions_gaps.rs"]
mod gaps;
#[path = "regions_orientation.rs"]
mod orientation;
#[path = "regions_radial.rs"]
mod radial;

pub(super) const FILL_MODES: [&str; 9] = [
    "Color Randomly",
    "Color Radially",
    "Color by Size",
    "Color by Orientation",
    "Image Color",
    "Centered Image",
    "Scaled Image",
    "Best Fit Image",
    "Scrambled Image",
];

fn choice_is_animated<E: 'static>(
    choice: &super::PointwiseParameter<E>,
) -> Result<bool, PointwiseError<E>> {
    Ok(
        super::parameter_ops::is_animated(|| choice.retained_animation())?
            || choice.has_animated_binding()?,
    )
}

pub(super) fn image_scale_is_conditional<E: 'static>(
    filter: &PointwiseFilter<E>,
) -> Result<bool, PointwiseError<E>> {
    let choice = filter.parameter(2)?;
    if choice_is_animated(&choice)? {
        return Ok(false);
    }
    let text = choice.text()?.ok_or(PointwiseError::NullReference)?;
    Ok(FILL_MODES[5..8]
        .iter()
        .any(|mode| text.iter().copied().eq(mode.encode_utf16())))
}

/// sj.a7/a6 and de, before zl.k checks for a matching constant slot.
pub(super) fn guarantee_basis<E: 'static>(
    filter: &PointwiseFilter<E>,
    [pattern, image]: [PointwiseChannelGuarantees; 2],
) -> Result<PointwiseChannelGuarantees, PointwiseError<E>> {
    let choice = filter.parameter(2)?;
    let animated = choice_is_animated(&choice)?;
    let text = if animated {
        Vec::new()
    } else {
        choice.text()?.ok_or(PointwiseError::NullReference)?
    };
    let suffix = [
        b'I' as u16,
        b'm' as u16,
        b'a' as u16,
        b'g' as u16,
        b'e' as u16,
    ];
    let uses_image_channels = animated || text.windows(suffix.len()).any(|part| part == suffix);
    let uses_image_scene = animated || text.ends_with(&suffix);
    let constants = if uses_image_channels {
        image
            .constants()
            .map(|value| value.filter(|&sample| sample == 0.).map(|_| 0.))
    } else {
        [None, Some(0.), Some(0.), Some(0.)]
    };
    Ok(PointwiseChannelGuarantees::new(
        constants,
        [false; 4],
        pattern.has_scene() || (uses_image_scene && image.has_scene()),
        None,
    ))
}

pub(super) fn retains_gap_map<E: 'static>(
    filter: &PointwiseFilter<E>,
) -> Result<bool, PointwiseError<E>> {
    let choice = filter.parameter(1)?;
    Ok(choice_is_animated(&choice)?
        || choice
            .text()?
            .is_some_and(|text| text.into_iter().eq("Yes".encode_utf16())))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fill {
    Random,
    Radial,
    Size,
    Orientation,
    ImageColor,
    Centered,
    Scaled,
    BestFit,
    Scrambled,
}

impl Fill {
    const ALL: [Self; 9] = [
        Self::Random,
        Self::Radial,
        Self::Size,
        Self::Orientation,
        Self::ImageColor,
        Self::Centered,
        Self::Scaled,
        Self::BestFit,
        Self::Scrambled,
    ];

    fn samples_image(self) -> bool {
        matches!(
            self,
            Self::Centered | Self::Scaled | Self::BestFit | Self::Scrambled
        )
    }
}

struct Region {
    left: i32,
    right: i32,
    top: i32,
    bottom: i32,
    area: usize,
    random: [f32; 2],
}

fn reserved<T>(length: usize) -> Result<Vec<T>, ImageError> {
    let mut data = Vec::new();
    data.try_reserve_exact(length)
        .map_err(|_| ImageError::Allocation)?;
    Ok(data)
}

fn absolute_index(x: i32, y: i32, width: i32, height: i32) -> usize {
    (y.rem_euclid(height) * width + x.rem_euclid(width)) as usize
}

fn detect(
    red: &[f32],
    threshold: f32,
    width: i32,
    height: i32,
) -> Result<(Vec<usize>, Vec<Region>), ImageError> {
    let length = (width * height) as usize;
    if red.len() < length {
        return Err(ImageError::Index);
    }
    let mut labels = reserved(length)?;
    labels.resize(length, 0);
    let mut regions = Vec::new();
    let mut pending = reserved(2048)?;
    for start in 0..length {
        let selected = red[start] >= threshold;
        if labels[start] != 0 || !selected {
            continue;
        }
        let label = regions.len() + 1;
        let x = start as i32 % width;
        let y = start as i32 / width;
        let mut region = Region {
            left: x,
            right: x,
            top: y,
            bottom: y,
            area: 0,
            random: [0.; 2],
        };
        pending.push((x, y));
        while let Some((x, y)) = pending.pop() {
            let index = absolute_index(x, y, width, height);
            if labels[index] != 0 {
                continue;
            }
            labels[index] = label;
            pending.try_reserve(4).map_err(|_| ImageError::Allocation)?;
            // The source pushes left, up, right, down. The LIFO order affects
            // the wrapped bounds used for color and image sampling.
            for (nx, ny) in [(x - 1, y), (x, y - 1), (x + 1, y), (x, y + 1)] {
                let neighbor = absolute_index(nx, ny, width, height);
                if labels[neighbor] == 0 && red[neighbor] >= threshold {
                    pending.push((nx, ny));
                }
            }
            if region.right - region.left < width - 1 {
                if x < region.left {
                    region.left -= 1;
                } else if x > region.right {
                    region.right += 1;
                }
            }
            if region.bottom - region.top < height - 1 {
                if y < region.top {
                    region.top -= 1;
                } else if y > region.bottom {
                    region.bottom += 1;
                }
            }
            region.area += 1;
        }
        if region.left < 0 {
            region.left += width;
            region.right += width;
        }
        if region.top < 0 {
            region.top += height;
            region.bottom += height;
        }
        regions.try_reserve(1).map_err(|_| ImageError::Allocation)?;
        regions.push(region);
    }
    Ok((labels, regions))
}

fn randomize<E>(
    regions: &mut [Region],
    width: i32,
    height: i32,
    seed: i32,
) -> Result<(), PointwiseError<E>> {
    let mut ratio = 0.;
    let mut count = 0_f64;
    for region in regions.iter().filter(|region| region.area > 1) {
        let extent = (region.right - region.left).max(region.bottom - region.top);
        ratio += f64::from(extent) / (region.area as f64).sqrt();
        count += 1.;
    }
    ratio /= count;
    let ratio = if ratio < 2.25 {
        1.
    } else {
        (ratio / 1.5).round_ties_even() * 1.5
    };
    let side = ((count.sqrt() * (ratio * 1.5)) as i32).max(4);
    let length = side.checked_mul(side).ok_or(ImageError::Overflow)? as usize;
    let mut first = reserved(length)?;
    let mut second = reserved(length)?;
    let mut random = crate::seeded_random::SeededRandom::from_seed(seed)
        .map_err(|_| PointwiseError::Arithmetic)?;
    for _ in 0..length {
        first.push(random.sample() as f32);
        second.push(random.sample() as f32);
    }
    let first = FloatBuffer::new(first);
    let second = FloatBuffer::new(second);
    for region in regions {
        let x = (region.left + region.right) as f32 * 0.5 / width as f32 * side as f32;
        let y = (region.top + region.bottom) as f32 * 0.5 / height as f32 * side as f32;
        region.random = [
            super::transform::sample(x, y, &first, side, side)?,
            super::transform::sample(x, y, &second, side, side)?,
        ];
    }
    Ok(())
}

fn color_fill(
    output: &SharedImageMap,
    image: &SharedImageMap,
    fill: Fill,
    labels: &[usize],
    regions: &[Region],
    orientations: &[orientation::Orientation],
    [width, height]: [i32; 2],
) -> Result<(), ImageError> {
    let max_size = regions.iter().map(|region| region.area).max().unwrap_or(1) as f32;
    let channel_count = if fill == Fill::ImageColor {
        image.channel_count()?.max(1)
    } else {
        1
    };
    for channel in 0..channel_count {
        let source = (fill == Fill::ImageColor)
            .then(|| image.channel(channel as i32, false))
            .transpose()?;
        let mut colors = reserved(regions.len() + 1)?;
        colors.push(0.);
        for (index, region) in regions.iter().enumerate() {
            colors.push(match fill {
                Fill::Random => region.random[0] * 0.99 + 0.01,
                Fill::Size => region.area as f32 / max_size,
                Fill::Orientation => orientations[index].color(),
                _ => {
                    let source = source.as_ref().ok_or(ImageError::NullReference)?;
                    let index = if source.len() == 1 {
                        0
                    } else {
                        absolute_index(
                            (region.left + region.right) / 2,
                            (region.top + region.bottom) / 2,
                            width,
                            height,
                        )
                    };
                    source.read(index)?
                }
            });
        }
        let mut samples = reserved(labels.len())?;
        samples.extend(labels.iter().map(|id| colors[*id]));
        output.set_buffer(channel as i32, Some(&FloatBuffer::new(samples)))?;
    }
    Ok(())
}

fn image_fill<E>(
    output: &SharedImageMap,
    image: &SharedImageMap,
    fill: Fill,
    labels: &[usize],
    regions: &[Region],
    orientations: &[orientation::Orientation],
    [width, height]: [i32; 2],
) -> Result<(), PointwiseError<E>> {
    let mut dense = Vec::new();
    for channel in 0..image.all_channel_count() {
        let source = image.channel_with_effects(channel, false)?;
        if source.len() == 1 {
            output.copy_indexed_constant(channel, source.read(0)?, image)?;
        } else {
            dense.push((source, output.allocate_indexed(channel, image)?));
        }
    }
    for (label, region) in regions.iter().enumerate() {
        let rotation = orientations
            .get(label)
            .map(|orientation| orientation.fit(fill, [width, height], region.random));
        let translated = rotation.is_none() && matches!(fill, Fill::Centered | Fill::Scrambled);
        let mut offset_x = (f64::from(width - region.left - region.right) * 0.5) as i32;
        let mut offset_y = (f64::from(height - region.top - region.bottom) * 0.5) as i32;
        if fill == Fill::Scrambled {
            offset_x += (region.random[0] * width as f32) as i32;
            offset_y += (region.random[1] * height as f32) as i32;
        }
        let mut left = region.left as f32;
        let mut top = region.top as f32;
        let mut span_x = (region.right - region.left).max(1) as f32;
        let mut span_y = (region.bottom - region.top).max(1) as f32;
        if fill == Fill::Scaled {
            let square = span_x.max(span_y);
            left -= (square - span_x) * 0.5;
            top -= (square - span_y) * 0.5;
            span_x = square;
            span_y = square;
        }
        let scale_x = 1. / (span_x / (width - 1) as f32);
        let scale_y = 1. / (span_y / (height - 1) as f32);
        for y in region.top..=region.bottom {
            for x in region.left..=region.right {
                let index = absolute_index(x, y, width, height);
                if labels[index] != label + 1 {
                    continue;
                }
                let [u, v] = if let Some(rotation) = &rotation {
                    rotation.coordinates([x, y])
                } else {
                    [(x as f32 - left) * scale_x, (y as f32 - top) * scale_y]
                };
                for (source, destination) in &dense {
                    let sample = if translated {
                        source.read(absolute_index(x + offset_x, y + offset_y, width, height))?
                    } else {
                        super::transform::sample(u, v, source, width, height)?
                    };
                    destination.write(index, sample)?;
                }
            }
        }
    }
    Ok(())
}

pub(super) fn retained<E: 'static>(
    node: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let width = geometry.width();
    let height = geometry.height();
    if width <= 0 || height <= 0 || width.checked_mul(height).is_none() {
        return Err(ImageError::Overflow.into());
    }
    let pattern = inputs
        .first()
        .ok_or(PointwiseError::Index)?
        .clone()
        .unwrap_or_else(|| SharedImageMap::new(geometry));
    let image = inputs
        .get(1)
        .ok_or(PointwiseError::Index)?
        .clone()
        .unwrap_or_else(|| SharedImageMap::new(geometry));
    if pattern.geometry() != geometry || image.geometry() != geometry {
        return Err(PointwiseError::UnsupportedCapability(
            "Regions with mismatched image geometry",
        ));
    }
    let selected_text = |index| {
        let property = node.parameter(index)?;
        let choice = property.effective_index_at(time)?;
        property
            .options()?
            .entries()
            .get(choice as usize)
            .cloned()
            .flatten()
            .ok_or(PointwiseError::Index)
    };
    let remove_gaps = selected_text(1)?.iter().copied().eq("Yes".encode_utf16());
    let mode = selected_text(2)?;
    let fill = FILL_MODES
        .iter()
        .position(|name| mode.iter().copied().eq(name.encode_utf16()))
        .map(|index| Fill::ALL[index])
        .ok_or(PointwiseError::UnsupportedCapability(
            "unknown Regions fill mode",
        ))?;
    let out = SharedImageMap::new(geometry);
    out.merge_effects(&[Some(pattern.clone())])?;
    let threshold = node.parameter(0)?.effective_value_at(time)? as f32;
    let red = pattern.materialize(0)?;
    let existing = if remove_gaps && fill != Fill::Radial {
        pattern.region_store()
    } else {
        None
    };
    let samples = if let Some(store) = &existing {
        store.data(true)?.values()
    } else {
        red.values()
    };
    let (mut labels, mut regions) = if existing.is_some() {
        from_map::detect(&samples, width, height)?
    } else {
        detect(&samples, threshold, width, height)?
    };
    if remove_gaps && regions.len() <= 1 {
        if matches!(fill, Fill::Centered | Fill::Scaled | Fill::BestFit) {
            return Ok(image);
        }
        if fill == Fill::Radial || regions.is_empty() {
            return Ok(out);
        }
    }
    let size = [width, height];
    let mut rebuilt_boundary = Vec::new();
    if let Some(store) = existing {
        out.set_region_store(Some(store));
    } else if remove_gaps {
        rebuilt_boundary = gaps::extend(&mut labels, &mut regions, size, fill == Fill::Radial)?;
        let mut values = reserved(labels.len())?;
        values.extend(
            labels
                .iter()
                .zip(&samples)
                .map(|(id, sample)| *id as f32 + if *sample >= threshold { 0.5 } else { 0. }),
        );
        out.set_region_store(Some(SharedFloatStore::new(FloatBuffer::new(values))?));
    }
    let rotate = selected_text(3)?.iter().copied().eq("Yes".encode_utf16());
    if fill == Fill::Radial {
        let boundary = remove_gaps.then_some(rebuilt_boundary.as_slice());
        out.set_buffer(0, Some(&radial::render(&labels, &regions, size, boundary)?))?;
        return Ok(out);
    }
    if matches!(fill, Fill::Random | Fill::Scrambled) {
        let seed = super::transform::cast(node.parameter(4)?.effective_seed_at(time)?);
        randomize(&mut regions, width, height, seed)?;
    }
    let orientations = if fill == Fill::Orientation || (rotate && fill.samples_image()) {
        orientation::measure(&regions, &labels, size)?
    } else {
        Vec::new()
    };
    if regions.is_empty() {
        return Ok(out);
    }
    if fill.samples_image() {
        image_fill(&out, &image, fill, &labels, &regions, &orientations, size)?;
    } else {
        color_fill(&out, &image, fill, &labels, &regions, &orientations, size)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PointwiseKind;

    #[test]
    fn regions_guarantees_follow_fill_text_and_the_two_input_roles() {
        let filter = PointwiseFilter::<()>::new(PointwiseKind::Regions).unwrap();
        let node = filter.node();
        let choice = filter.parameter(2).unwrap();
        let pattern = PointwiseChannelGuarantees::new([None; 4], [true; 4], false, None);
        let image = PointwiseChannelGuarantees::new(
            [Some(0.), Some(0.5), None, Some(0.)],
            [true; 4],
            true,
            None,
        );
        let channels = || {
            node.calculate_image_guarantees_for_inputs(&[Some(pattern), Some(image)])
                .unwrap()
        };
        choice.set_index(2).unwrap();
        assert_eq!(channels().constants(), [None, Some(0.), Some(0.), Some(0.)]);
        assert!(!channels().has_scene());
        choice.set_index(4).unwrap();
        assert_eq!(channels().constants(), [Some(0.), None, None, Some(0.)]);
        assert_eq!(channels().in_range(), [false; 4]);
        assert!(!channels().has_scene());
        choice.set_index(5).unwrap();
        assert_eq!(channels().constants(), [Some(0.), None, None, Some(0.)]);
        assert!(channels().has_scene());
        let zeros = node
            .calculate_image_guarantees_for_inputs(&[Some(pattern), None])
            .unwrap();
        assert_eq!(zeros.constants(), [Some(0.); 4]);
        assert_eq!(zeros.passthrough_slot(), Some(1));

        choice.set_index(2).unwrap();
        choice.set_axis_animated(0, true).unwrap();
        assert_eq!(channels().constants(), [Some(0.), None, None, Some(0.)]);
        assert!(channels().has_scene());
        choice.set_axis_animated(0, false).unwrap();
        node.prepare_image_guarantees(&mut |_| Ok(())).unwrap();
        assert_eq!(
            node.prepared_image_guarantees().unwrap().constants(),
            [None, Some(0.), Some(0.), Some(0.)]
        );
    }

    #[test]
    fn regions_radial_preserves_threshold_boundaries_and_normalizes_each_shape() {
        let size = [32, 32];
        let pattern = SharedImageMap::new(ImageGeometry::new(size[0], size[1]));
        let index = |x, y| absolute_index(x, y, size[0], size[1]);
        let mut mask = vec![0.; (size[0] * size[1]) as usize];
        for (left, top, side) in [(3, 4, 9), (18, 6, 5)] {
            for y in top..top + side {
                for x in left..left + side {
                    mask[index(x, y)] = 1.;
                }
            }
        }
        mask[index(28, 18)] = 1.;
        pattern
            .set_buffer(0, Some(&FloatBuffer::new(mask)))
            .unwrap();
        let node = PointwiseFilter::<()>::new(PointwiseKind::Regions).unwrap();
        node.parameter(1).unwrap().set_index(1).unwrap();
        node.parameter(2).unwrap().set_index(1).unwrap();
        let image = node
            .render_supplied(&[Some(pattern), None], size[0], size[1])
            .unwrap()
            .unwrap();
        let pixels = image.materialize(0).unwrap().values();
        assert_eq!(pixels[index(3, 8)], 0.);
        assert_eq!(pixels[index(28, 18)], 0.);
        assert_eq!(pixels[index(26, 4)], 0.);
        for (x, y) in [(7, 8), (20, 8)] {
            assert!((pixels[index(x, y)] - 1.).abs() < 0.00001);
            assert!(pixels[index(x - 1, y)] > 0.);
            assert!(pixels[index(x - 1, y)] < pixels[index(x, y)]);
        }
    }

    #[test]
    fn regions_radial_rebuilds_the_periodic_bisectors_and_map() {
        let node = PointwiseFilter::<()>::new(PointwiseKind::Regions).unwrap();
        node.parameter(1).unwrap().set_index(0).unwrap();
        node.parameter(2).unwrap().set_index(1).unwrap();
        for ([width, height], axis) in [([32, 32], 0), ([32, 70], 0), ([70, 32], 1)] {
            let geometry = ImageGeometry::new(width, height);
            let pattern = SharedImageMap::new(geometry);
            let mask: Vec<_> = (0..geometry.area())
                .map(|pixel| {
                    let coordinates = [pixel % width, pixel / width];
                    if matches!(coordinates[axis], 8 | 24) {
                        1.
                    } else {
                        0.
                    }
                })
                .collect();
            pattern
                .set_buffer(0, Some(&FloatBuffer::new(mask.clone())))
                .unwrap();
            let original = SharedFloatStore::new(FloatBuffer::new(mask)).unwrap();
            pattern.set_region_store(Some(original.clone()));
            let image = node
                .render_supplied(&[Some(pattern), None], width, height)
                .unwrap()
                .unwrap();
            assert_eq!(image.geometry(), geometry);
            let regions = image.region_store().unwrap();
            assert!(!regions.same_store(&original));
            let labels = regions.data(true).unwrap().values();
            let pixels = image.materialize(0).unwrap().values();
            for pixel in 0..geometry.area() {
                let coordinates = [pixel % width, pixel / width];
                let coordinate = coordinates[axis];
                let expected_label = if (0..=16).contains(&coordinate) {
                    1.
                } else {
                    2.
                };
                let actual_label = labels[pixel as usize];
                if matches!(coordinate, 0 | 16) {
                    assert!(matches!(actual_label, 1. | 2.));
                } else {
                    let part = if matches!(coordinate, 8 | 24) {
                        0.5
                    } else {
                        0.
                    };
                    assert_eq!(actual_label, expected_label + part);
                }
                let distance = coordinate.min((coordinate - 16).abs()).min(32 - coordinate);
                let expected = distance as f32 / 8.;
                assert!(
                    (pixels[pixel as usize] - expected).abs() < 0.00001,
                    "{width}×{height}, {coordinates:?}: {} vs {expected}",
                    pixels[pixel as usize]
                );
            }
        }
    }

    #[test]
    fn regions_measure_orientation_and_rotate_all_image_fills() {
        let geometry = ImageGeometry::new(16, 16);
        let pattern = SharedImageMap::new(geometry);
        let mut mask = vec![0.; 16 * 16];
        let index = |x: usize, y: usize| y * 16 + x;
        for i in 0..5 {
            mask[index(i + 5, 2)] = 1.;
            mask[index(2, i + 5)] = 1.;
        }
        pattern
            .set_buffer(0, Some(&FloatBuffer::new(mask)))
            .unwrap();
        let image = SharedImageMap::new(geometry);
        let ramp = (0..16 * 16).map(|i| (i % 16) as f32).collect();
        image.set_buffer(0, Some(&FloatBuffer::new(ramp))).unwrap();
        image.set_constant(1, 0.25).unwrap();
        let node = PointwiseFilter::<()>::new(PointwiseKind::Regions).unwrap();
        node.parameter(1).unwrap().set_index(1).unwrap();
        let sources = [Some(pattern), Some(image)];
        let render = |mode, rotate| {
            node.parameter(2).unwrap().set_index(mode).unwrap();
            node.parameter(3)
                .unwrap()
                .set_index(if rotate { 0 } else { 1 })
                .unwrap();
            node.render_supplied(&sources, 16, 16).unwrap().unwrap()
        };
        let orientation = render(3, false).materialize(0).unwrap().values();
        assert_eq!(orientation[index(7, 2)], 0.01);
        assert_eq!(orientation[index(2, 7)], 0.505);
        assert_eq!(orientation[index(7, 7)], 0.);
        let near = |samples: [f32; 3], expected: [f32; 3]| {
            for (sample, expected) in samples.into_iter().zip(expected) {
                assert!(
                    (sample - expected).abs() < 0.00001,
                    "{sample} vs {expected}"
                );
            }
        };
        for (mode, expected) in [(5, [10., 8., 6.]), (6, [15., 7.5, 0.]), (7, [15., 7.5, 0.])] {
            let image = render(mode, true);
            let pixels = image.materialize(0).unwrap().values();
            near(
                [
                    pixels[index(2, 5)],
                    pixels[index(2, 7)],
                    pixels[index(2, 9)],
                ],
                expected,
            );
            assert_eq!(pixels[index(7, 7)], 0.);
            assert_eq!(image.channel(1, false).unwrap().read(0).unwrap(), 0.25);
            assert_ne!(pixels, render(mode, false).materialize(0).unwrap().values());
        }
        let scramble = || render(8, true).materialize(0).unwrap().values();
        let first = scramble();
        assert_eq!(first, scramble());
        let seed = node.parameter(4).unwrap();
        seed.set_number(1.).unwrap();
        let second = scramble();
        assert_ne!(first, second);
        seed.set_number(0.).unwrap();
        assert_eq!(first, scramble());
        node.node().set_graph_seed_modifier(1.).unwrap();
        assert_eq!(second, scramble());

        let snapshot = crate::RenderCaptureSession::new()
            .capture_filter(&node)
            .unwrap();
        node.node().set_graph_seed_modifier(0.).unwrap();
        let frozen_pixels = snapshot
            .render_supplied_with_options(
                &sources,
                16,
                16,
                crate::PointwiseRenderOptions::new(false, false),
            )
            .unwrap()
            .unwrap()
            .materialize(0)
            .unwrap()
            .values();
        assert_eq!(second, frozen_pixels);
        assert_eq!(first, scramble());
    }

    #[test]
    fn regions_reconstructs_wrapped_seeds_and_retains_the_original_store() {
        let geometry = ImageGeometry::new(5, 3);
        let pattern = SharedImageMap::new(geometry);
        let mut values = vec![0.; 15];
        values[0] = 1.5;
        values[4] = 1.5;
        values[5] = 1.;
        values[12] = 2.5;
        values[13] = 2.;
        let store = SharedFloatStore::new(FloatBuffer::new(values)).unwrap();
        pattern.set_region_store(Some(store.clone()));
        let filter = PointwiseFilter::<()>::new(PointwiseKind::Regions).unwrap();
        filter.parameter(2).unwrap().set_index(2).unwrap();
        let result = filter
            .render_supplied(&[Some(pattern), None], 5, 3)
            .unwrap()
            .unwrap();
        assert!(result.region_store().unwrap().same_store(&store));
        let red = result.materialize(0).unwrap().values();
        assert_eq!([red[0], red[4], red[5]], [1.; 3]);
        assert_eq!([red[12], red[13]], [2. / 3.; 2]);
        assert!(!red.contains(&0.));

        let empty = SharedImageMap::new(geometry);
        empty.set_region_store(Some(
            SharedFloatStore::new(FloatBuffer::new(vec![1.; 15])).unwrap(),
        ));
        let result = filter
            .render_supplied(&[Some(empty), None], 5, 3)
            .unwrap()
            .unwrap();
        assert_eq!(result.channel(0, false).unwrap().read(0).unwrap(), 0.);
    }

    #[test]
    fn regions_find_wrapped_components_and_fill_from_image() {
        let geometry = ImageGeometry::new(7, 7);
        let pattern = SharedImageMap::new(geometry);
        let mut mask = vec![0.; 49];
        mask[0] = 0.3;
        mask[6] = 0.8;
        mask[16] = 1.;
        pattern
            .set_buffer(0, Some(&FloatBuffer::new(mask)))
            .unwrap();
        let image = SharedImageMap::new(geometry);
        image
            .set_buffer(
                0,
                Some(&FloatBuffer::new((0..49).map(|i| i as f32).collect())),
            )
            .unwrap();
        let node = PointwiseFilter::<()>::new(PointwiseKind::Regions).unwrap();
        node.parameter(1).unwrap().set_index(1).unwrap();
        node.parameter(3).unwrap().set_index(1).unwrap();
        let sources = [Some(pattern), Some(image.clone())];
        let render = |mode| {
            node.parameter(2).unwrap().set_index(mode).unwrap();
            node.render_supplied(&sources, 7, 7)
                .unwrap()
                .unwrap()
                .materialize(0)
                .unwrap()
                .values()
        };
        let sizes = render(2);
        assert_eq!([sizes[0], sizes[6], sizes[16], sizes[1]], [1., 1., 0.5, 0.]);
        let colors = render(4);
        assert_eq!(
            [colors[0], colors[6], colors[16], colors[1]],
            [6., 6., 16., 0.]
        );
        let centered = render(5);
        assert_eq!([centered[0], centered[6], centered[1]], [25., 24., 0.]);
        let random = render(0);
        assert_eq!(random[0], random[6]);
        assert_eq!(random, render(0));
        node.parameter(4).unwrap().set_number(1.).unwrap();
        assert_ne!(random, render(0));
        node.parameter(1).unwrap().set_index(0).unwrap();
        node.parameter(2).unwrap().set_index(5).unwrap();
        let filled = node.render_supplied(&sources, 7, 7).unwrap().unwrap();
        let map = filled.region_store().unwrap().data(true).unwrap().values();
        assert_eq!([map[0], map[6], map[16]], [1.5, 1.5, 2.5]);
        assert_eq!(map[5], 1.);
        assert!(map.iter().all(|&label| label >= 1.));
        let uniform = SharedImageMap::new(geometry);
        uniform.set_constant(0, 1.).unwrap();
        let forwarded = node
            .render_supplied(&[Some(uniform), Some(image.clone())], 7, 7)
            .unwrap()
            .unwrap();
        assert!(forwarded.same_reference(&image));
    }
}
