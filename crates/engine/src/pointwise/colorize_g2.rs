//! aq8/aca lookup and cl's conditional migration of the deprecated gradient.
use super::{
    PointwiseChannelGuarantees, PointwiseCustomObject, PointwiseError, PointwiseFilter,
    PointwiseKind, PointwiseParameter, SharedImageMap, check_render_cancelled,
    property::WeakPointwiseParameter,
};
use crate::{
    BinaryCursor, BinaryError, BinaryWriter, HistoryDirection, HistoryError, ImageError,
    ImageGeometry, SharedGradient, framework_cast::integer, history::HistoryGraphEdit,
};

/// The retained Colorize G2 lookup. Each stop stores position, red, green,
/// blue, and transparency in that order.
#[derive(Clone, Debug)]
pub struct LegacyColorGradient {
    steps: i32,
    stops: Vec<[f64; 5]>,
}

impl Default for LegacyColorGradient {
    fn default() -> Self {
        Self {
            steps: 5000,
            stops: vec![[1.0, 1.0, 1.0, 1.0, 0.0]],
        }
    }
}

impl LegacyColorGradient {
    pub(in crate::pointwise) fn from_xml_values(steps: i32, stops: Vec<[f64; 5]>) -> Self {
        Self { steps, stops }
    }

    pub fn steps(&self) -> i32 {
        self.steps
    }

    pub fn stops(&self) -> &[[f64; 5]] {
        &self.stops
    }

    pub fn set_steps(&mut self, steps: i32) -> bool {
        if !(1..=10_000).contains(&steps) {
            return false;
        }
        self.steps = steps;
        true
    }

    pub fn set_stop_component(&mut self, index: usize, component: usize, value: f64) -> bool {
        let range = if component == 0 {
            0.0..=1.0
        } else {
            -100.0..=100.0
        };
        let Some(current) = self
            .stops
            .get_mut(index)
            .and_then(|stop| stop.get_mut(component))
        else {
            return false;
        };
        if !range.contains(&value) {
            return false;
        }
        *current = value;
        true
    }

    pub fn sort_stops(&mut self) {
        self.stops.sort_by(|left, right| {
            left.iter()
                .zip(right)
                .map(|(left, right)| {
                    left.partial_cmp(right)
                        .unwrap_or_else(|| left.total_cmp(right))
                })
                .find(|order| !order.is_eq())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    pub fn remove_stop(&mut self, index: usize) -> bool {
        if index >= self.stops.len() {
            return false;
        }
        self.stops.remove(index);
        true
    }

    pub fn add_sampled_stop(&mut self, position: f64, strip_width: usize) -> Option<usize> {
        if !(0.0..=1.0).contains(&position) || strip_width == 0 || self.steps <= 0 {
            return None;
        }
        let mut ordered = self.clone();
        ordered.sort_stops();
        let column = ((position * strip_width as f64) as usize).min(strip_width - 1);
        let [red, green, blue, transparency] = ordered
            .strip_sample(column, strip_width)
            .map(|value| crate::round_decimal_places(value, 3).ok());
        let index = self.stops.len();
        self.stops
            .push([position, red?, green?, blue?, transparency?]);
        Some(index)
    }

    /// The original editor strip interpolates in Double. The texture worker
    /// below has its own Single lookup and does not use this display path.
    pub fn strip_sample(&self, column: usize, width: usize) -> [f64; 4] {
        let count = (self.steps.max(0) as usize).min(width);
        if count == 0 || column >= width || self.stops.is_empty() {
            return [0.0; 4];
        }
        let index = if count == width {
            column
        } else {
            (column as f64 / width as f64 * count as f64) as usize
        };
        let position = index as f64 / count as f64;
        let next = self.stops.partition_point(|stop| stop[0] < position);
        std::array::from_fn(|channel| {
            let channel = channel + 1;
            if next == 0 {
                let first = self.stops[0];
                if first[0] == 0.0 {
                    first[channel]
                } else {
                    first[channel] * position / first[0]
                }
            } else if next == self.stops.len() {
                let last = self.stops[next - 1];
                last[channel] * (1.0 - (position - last[0]) / (1.0 - last[0]))
            } else {
                let left = self.stops[next - 1];
                let right = self.stops[next];
                let weight = (position - left[0]) / (right[0] - left[0]);
                right[channel] * weight + left[channel] * (1.0 - weight)
            }
        })
    }

    pub fn same_values(&self, other: &Self) -> bool {
        self.steps == other.steps
            && self.stops.len() == other.stops.len()
            && self
                .stops
                .iter()
                .flatten()
                .map(|value| value.to_bits())
                .eq(other.stops.iter().flatten().map(|value| value.to_bits()))
    }

    pub(super) fn read(&mut self, read: &mut BinaryCursor<'_>) -> Result<(), BinaryError> {
        self.steps = read.read_i32()?;
        let count = usize::try_from(read.read_i32()?).map_err(|_| BinaryError::NegativeLength)?;
        let remaining = (read.length()? - read.position()?).max(0) as usize;
        let mut stops = Vec::new();
        stops
            .try_reserve_exact(count.min(remaining / 40))
            .map_err(|_| BinaryError::Allocation)?;
        self.stops = stops;
        for _ in 0..count {
            let mut stop = [0.0; 5];
            for value in &mut stop {
                *value = read.read_f64()?;
            }
            self.stops.push(stop);
        }
        Ok(())
    }

    pub(super) fn write(
        &self,
        writer: &mut BinaryWriter,
        project: bool,
    ) -> Result<(), BinaryError> {
        writer.write_i32(self.steps)?;
        writer.write_i32(i32::try_from(self.stops.len()).map_err(|_| BinaryError::Count)?)?;
        for &[position, red, green, blue, alpha] in &self.stops {
            writer.write_f64(position)?;
            for value in [red, green, blue, alpha] {
                let value = if project {
                    value
                } else {
                    crate::round_decimal_places(value, 5)
                        .map_err(|_| BinaryError::DecimalEncoding)?
                };
                writer.write_f64(value)?;
            }
        }
        Ok(())
    }

    pub(super) fn render_values(&self, out: &mut Vec<u64>) {
        out.extend([self.steps as u64, self.stops.len() as u64]);
        out.extend(self.stops.iter().flatten().map(|value| value.to_bits()));
    }

    fn alias(&self, channel: usize) -> Option<usize> {
        (0..channel).find(|&previous| {
            self.stops
                .iter()
                .all(|stop| stop[previous + 1] == stop[channel + 1])
        })
    }

    fn table(&self, channel: usize, expanded: Option<usize>) -> Result<Vec<f32>, ImageError> {
        let mut stops = self.stops.iter();
        let Some(first) = stops.next() else {
            return Ok(vec![0.0; expanded.unwrap_or(1)]);
        };
        let end = self.stops.last().ok_or(ImageError::Index)?;
        let value_index = channel + 1;
        if expanded.is_none() {
            let value = first[value_index] as f32;
            if ((first[0] == 0.0 && end[0] == 1.0) || value == 0.0)
                && self
                    .stops
                    .iter()
                    .all(|stop| stop[value_index] == f64::from(value))
            {
                return Ok(vec![value]);
            }
        }
        let steps = usize::try_from(self.steps).map_err(|_| ImageError::Overflow)?;
        let count = expanded.map_or(steps, |size| steps.min(size));
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(count)
            .map_err(|_| ImageError::Allocation)?;
        let mut next = 0;
        for index in 0..count {
            let t = index as f32 / count as f32;
            while next < self.stops.len() && self.stops[next][0] < f64::from(t) {
                next += 1;
            }
            let value = if next == 0 {
                if first[0] == 0.0 {
                    first[value_index] as f32
                } else {
                    first[value_index] as f32 * t / first[0] as f32
                }
            } else if next == self.stops.len() {
                if t == 1.0 {
                    if end[0] == 1.0 {
                        end[value_index] as f32
                    } else {
                        0.0
                    }
                } else {
                    (end[value_index] * (1.0 - (f64::from(t) - end[0]) / (1.0 - end[0]))) as f32
                }
            } else {
                let before = self.stops[next - 1];
                let after = self.stops[next];
                let weight =
                    f64::from((t - before[0] as f32) / (after[0] as f32 - before[0] as f32));
                (after[value_index] * weight + before[value_index] * (1.0 - weight)) as f32
            };
            samples.push(value);
        }
        if let Some(size) = expanded
            && count < size
        {
            return (0..size)
                .map(|index| {
                    let at = (index as f32 / size as f32 * count as f32) as usize;
                    samples.get(at).copied().ok_or(ImageError::Index)
                })
                .collect();
        }
        Ok(samples)
    }
}

impl<E: 'static> PointwiseParameter<E> {
    pub fn legacy_color_gradient(&self) -> Result<LegacyColorGradient, PointwiseError<E>> {
        self.custom_object()?
            .ok_or(PointwiseError::NullReference)?
            .legacy_gradient()
    }

    /// Replace the retained gradient, recording one edit on its attached slot.
    pub fn edit_legacy_color_gradient(
        &self,
        expected: &LegacyColorGradient,
        next: LegacyColorGradient,
    ) -> Result<bool, PointwiseError<E>> {
        let edit = LegacyGradientEdit {
            parameter: self.downgrade(),
            before: expected.clone(),
            after: next,
        };
        if edit.before.same_values(&edit.after) {
            return Ok(false);
        }
        let map_error = |error| PointwiseError::History(Box::new(error));
        let slot = self
            .owner()
            .node()
            .texture_node_logic()
            .and_then(|logic| logic.slot());
        let history = slot
            .as_ref()
            .and_then(|slot| slot.history().map(|history| (slot, history)));
        edit.replace(self, HistoryDirection::Redo)
            .map_err(map_error)?;
        if let Some((slot, history)) = history {
            history
                .record_graph_edit(
                    slot.history_target(),
                    "Color Gradient".encode_utf16().collect(),
                    Box::new(edit),
                )
                .map_err(map_error)?;
        }
        Ok(true)
    }
}

struct LegacyGradientEdit<E> {
    parameter: WeakPointwiseParameter<E>,
    before: LegacyColorGradient,
    after: LegacyColorGradient,
}

impl<E: 'static> LegacyGradientEdit<E> {
    fn replace(
        &self,
        parameter: &PointwiseParameter<E>,
        direction: HistoryDirection,
    ) -> Result<(), HistoryError<E>> {
        let (before, after) = match direction {
            HistoryDirection::Undo => (&self.after, &self.before),
            HistoryDirection::Redo => (&self.before, &self.after),
        };
        let property_error = |error| HistoryError::Property(Box::new(error));
        if !parameter
            .legacy_color_gradient()
            .map_err(property_error)?
            .same_values(before)
        {
            return Err(HistoryError::InputChanged);
        }
        parameter
            .set_custom_object(Some(&PointwiseCustomObject::new_legacy_gradient(
                after.clone(),
            )))
            .map_err(property_error)
    }
}

impl<E: 'static> HistoryGraphEdit<E> for LegacyGradientEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        self.replace(
            &self.parameter.upgrade().ok_or(HistoryError::NullTarget)?,
            direction,
        )
    }
}

fn gradient<E: 'static>(
    filter: &PointwiseFilter<E>,
) -> Result<LegacyColorGradient, PointwiseError<E>> {
    filter.parameter(0)?.legacy_color_gradient()
}

pub(super) fn migrate<E: 'static>(
    legacy: &PointwiseFilter<E>,
) -> Result<Option<PointwiseFilter<E>>, PointwiseError<E>> {
    let ramp = gradient(legacy)?;
    if (21..1000).contains(&ramp.steps) {
        return Ok(None);
    }
    let filter = PointwiseFilter::new(PointwiseKind::Colorize)?;
    filter
        .parameter(1)?
        .set_index(legacy.parameter(1)?.index())?;
    let target = SharedGradient::new();
    if ramp.steps <= 20 {
        let channels = [
            ramp.table(0, Some(1000))?,
            ramp.table(1, Some(1000))?,
            ramp.table(2, Some(1000))?,
        ];
        let pixel = |index: usize| channels.each_ref().map(|channel| f64::from(channel[index]));
        let mut previous = pixel(0);
        for index in 1..1000 {
            let next = pixel(index);
            if previous != next {
                let [r, g, b] = previous;
                target.add_normalized_color((index - 1) as f64 / 1000.0, r, g, b)?;
                let [r, g, b] = next;
                target.add_normalized_color(index as f64 / 1000.0, r, g, b)?;
                previous = next;
            }
        }
        match ramp.table(3, None)?.as_slice() {
            &[constant] => {
                if constant != 0.0 {
                    target.add_normalized_alpha(0.0, 1.0 - f64::from(constant))?;
                }
            }
            samples if !samples.is_empty() => {
                let samples = ramp.table(3, Some(1000))?;
                let mut previous = samples[0];
                for (index, &sample) in samples.iter().enumerate().skip(1) {
                    if sample != previous {
                        target.add_normalized_alpha(
                            (index - 1) as f64 / 1000.0,
                            1.0 - f64::from(previous),
                        )?;
                        target
                            .add_normalized_alpha(index as f64 / 1000.0, 1.0 - f64::from(sample))?;
                        previous = sample;
                    }
                }
            }
            _ => (),
        }
    } else if let (Some(first), Some(last)) = (ramp.stops.first(), ramp.stops.last()) {
        let alpha = ramp.stops.iter().any(|stop| stop[4] != 0.0);
        for position in [
            if first[0] > 0.0 { Some(0.0) } else { None },
            if last[0] < 1.0 { Some(1.0) } else { None },
        ]
        .into_iter()
        .flatten()
        {
            target.add_normalized_color(position, 0.0, 0.0, 0.0)?;
            if alpha {
                target.add_normalized_alpha(position, 1.0)?;
            }
        }
        for &[position, red, green, blue, transparency] in &ramp.stops {
            target.add_normalized_color(position, red, green, blue)?;
            if alpha {
                target.add_normalized_alpha(position, 1.0 - transparency)?;
            }
        }
    }
    filter.parameter(0)?.set_gradient(Some(target))?;
    Ok(Some(filter))
}

pub(super) fn retained<E: 'static>(
    filter: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: Option<f64>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    let input = inputs
        .first()
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let source = input.channel(0, false)?;
    let ramp = gradient(filter)?;
    let output = SharedImageMap::new(geometry);
    for channel in 0..4 {
        check_render_cancelled(cancelled)?;
        if let Some(prior) = ramp.alias(channel) {
            output.set_buffer(channel as i32, Some(&output.channel(prior as i32, false)?))?;
            continue;
        }
        let table = ramp.table(channel, None)?;
        if table.len() <= 1 {
            output.set_constant(channel as i32, *table.first().ok_or(ImageError::Index)?)?;
            continue;
        }
        let destination = output.destination_count(channel as i32, source.len())?;
        for index in 0..destination.len() {
            if index.is_multiple_of(4096) {
                check_render_cancelled(cancelled)?;
            }
            let value = source.read(index)?;
            let lookup = if value >= 1.0 {
                table.len() - 1
            } else if value <= 0.0 {
                0
            } else {
                usize::try_from(integer(f64::from(value * table.len() as f32)))
                    .map_err(|_| ImageError::Index)?
            };
            destination.write(index, *table.get(lookup).ok_or(ImageError::Index)?)?;
        }
    }
    if output.channel_count()? <= 3 && super::effect_maps::choice_is_at(filter, 1, time, "Yes")? {
        output.set_buffer(3, Some(&input.channel(3, false)?))?;
    }
    output.copy_auxiliary(input)?;
    Ok(Some(output))
}

pub(super) fn guarantees<E: 'static>(
    filter: &PointwiseFilter<E>,
    input: PointwiseChannelGuarantees,
) -> Result<PointwiseChannelGuarantees, PointwiseError<E>> {
    let ramp = gradient(filter)?;
    let mut ranges = [false; 4];
    let mut constants = [None; 4];
    for channel in 0..4 {
        let table = ramp.table(channel, None)?;
        if channel == 3
            && table.as_slice() == [0.0]
            && super::effect_maps::choice_is_at(filter, 1, None, "Yes")?
        {
            constants[3] = input.constants()[3];
            ranges[3] = input.effective_range(3);
        } else {
            ranges[channel] = !ramp
                .stops
                .iter()
                .any(|stop| stop[channel + 1] < 0.0 || stop[channel + 1] > 1.0);
            constants[channel] = if let &[constant] = table.as_slice() {
                Some(constant)
            } else if let Some(red) = input.constants()[0] {
                let index = integer(f64::from(red.clamp(0.0, 1.0) * (table.len() as f32 - 1.0)));
                Some(
                    *table
                        .get(usize::try_from(index).map_err(|_| ImageError::Index)?)
                        .ok_or(ImageError::Index)?,
                )
            } else {
                None
            };
        }
    }
    Ok(PointwiseChannelGuarantees::new(
        constants,
        ranges,
        input.has_scene(),
        None,
    ))
}
