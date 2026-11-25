//! uq.k8 over evaluated values. Density, border and shifts are the outputs of
//! uq's actual four-out resolver; this leaf neither invents Factor nor replaces
//! its retained animation/ancestor/scale evaluation with a numeric override.
use crate::framework_cast::integer;
use crate::procedural_schedule::for_each_chunk;
use crate::seeded_random::SeededRandom;
use crate::{
    FloatBuffer, ImageError, ImageGeometry, PointwiseError, ProceduralExecution, SharedImageMap,
};
const HORIZONTAL: &[u16] = &[72, 111, 114, 105, 122, 111, 110, 116, 97, 108];

/// Immutable evaluated operands, not a second writable retained property model.
/// Size is normalized/effective, Location raw/active. The density pair, border
/// and shifts MUST already include the actual four-output Factor resolution.
#[derive(Clone, Copy, Debug)]
pub struct BricksRenderOptions<'a> {
    size: [f64; 2],
    location: [f64; 2],
    density: [f64; 2],
    border: f64,
    shifts: f64,
    width_variation: f64,
    slide_variation: f64,
    orientation: Option<&'a [u16]>,
    execution: ProceduralExecution,
}
impl<'a> BricksRenderOptions<'a> {
    pub const fn new(
        size: [f64; 2],
        location: [f64; 2],
        density: [f64; 2],
        border: f64,
        shifts: f64,
        execution: ProceduralExecution,
    ) -> Self {
        Self {
            size,
            location,
            density,
            border,
            shifts,
            width_variation: 0.0,
            slide_variation: 0.0,
            orientation: Some(HORIZONTAL),
            execution,
        }
    }
    pub const fn with_variations(mut self, width: f64, slide: f64) -> Self {
        self.width_variation = width;
        self.slide_variation = slide;
        self
    }
    pub const fn with_orientation_text(mut self, text: Option<&'a [u16]>) -> Self {
        self.orientation = text;
        self
    }
    pub const fn size(&self) -> [f64; 2] {
        self.size
    }
    pub const fn location(&self) -> [f64; 2] {
        self.location
    }
    pub const fn resolved_density(&self) -> [f64; 2] {
        self.density
    }
    pub const fn resolved_border(&self) -> f64 {
        self.border
    }
    pub const fn resolved_shifts(&self) -> f64 {
        self.shifts
    }
    pub const fn width_variation(&self) -> f64 {
        self.width_variation
    }
    pub const fn slide_variation(&self) -> f64 {
        self.slide_variation
    }
    pub const fn orientation_text(&self) -> Option<&'a [u16]> {
        self.orientation
    }
    pub const fn execution(&self) -> ProceduralExecution {
        self.execution
    }
}
/// Narrow retained-adapter seam with the exact source read boundaries. It does
/// not manufacture an owner or Factor provider; a future model must resolve real
/// Density.u/x, Border.o, Shifts.o in the first operation's actual order.
pub(crate) trait BricksInputSource<E> {
    fn resolved_values(&mut self) -> Result<[f64; 4], PointwiseError<E>>;
    fn is_horizontal(&mut self) -> Result<bool, PointwiseError<E>>;
    fn size_axis(&mut self, axis: usize) -> Result<f64, PointwiseError<E>>;
    fn location_axis(&mut self, axis: usize) -> Result<f64, PointwiseError<E>>;
    fn width_variation(&mut self) -> Result<f64, PointwiseError<E>>;
    fn slide_variation(&mut self) -> Result<f64, PointwiseError<E>>;
}
struct Evaluated<'a, 'b>(&'a BricksRenderOptions<'b>);
impl<E> BricksInputSource<E> for Evaluated<'_, '_> {
    fn resolved_values(&mut self) -> Result<[f64; 4], PointwiseError<E>> {
        Ok([
            self.0.density[0],
            self.0.density[1],
            self.0.border,
            self.0.shifts,
        ])
    }
    fn is_horizontal(&mut self) -> Result<bool, PointwiseError<E>> {
        Ok(self.0.orientation == Some(HORIZONTAL))
    }
    fn size_axis(&mut self, axis: usize) -> Result<f64, PointwiseError<E>> {
        Ok(self.0.size[axis])
    }
    fn location_axis(&mut self, axis: usize) -> Result<f64, PointwiseError<E>> {
        Ok(self.0.location[axis])
    }
    fn width_variation(&mut self) -> Result<f64, PointwiseError<E>> {
        Ok(self.0.width_variation)
    }
    fn slide_variation(&mut self) -> Result<f64, PointwiseError<E>> {
        Ok(self.0.slide_variation)
    }
}
#[derive(Clone, Copy, Debug)]
struct Rectangle {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}
struct Row {
    rectangles: Vec<Rectangle>,
}
fn append(
    rectangles: &mut Vec<Rectangle>,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
) -> Result<(), ImageError> {
    rectangles
        .try_reserve(1)
        .map_err(|_| ImageError::Allocation)?;
    rectangles.push(Rectangle {
        left,
        top,
        right,
        bottom,
    });
    Ok(())
}
#[allow(clippy::too_many_arguments)] // Exact uq+b constructor operands, plus caller scheduler profile.
fn build_row<E>(
    index: i32,
    columns: i32,
    width: f64,
    height: f64,
    offset_x: f64,
    offset_y: f64,
    border: f64,
    width_variation: f64,
    slide_variation: f64,
    size_x: f64,
    size_y: f64,
    execution: ProceduralExecution,
    legacy_width_batches: bool,
) -> Result<Row, PointwiseError<E>> {
    let mut rectangles = Vec::new();
    let gap = width * border;
    let gap_x = gap / size_x;
    let gap_y = gap / size_y;
    let usable_width = (width - gap_x * f64::from(columns)) / f64::from(columns);
    let usable_height = height - gap_y;
    // Actual ble.un: nonpositive OR unordered exits before either RNG/array.
    if usable_width.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
        || usable_height.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
    {
        return Ok(Row { rectangles });
    }
    let mut left = offset_x + gap_x / 2.0;
    let top = offset_y + gap_y / 2.0;
    let bottom = top + usable_height;
    if slide_variation != 0.0 {
        let mut random = SeededRandom::from_seed(0).map_err(|_| ImageError::Overflow)?;
        for _ in 0..index.wrapping_add(5) {
            let _ = random.sample();
        }
        left += (random.sample() - 0.5) * slide_variation * usable_width;
    }
    if width_variation == 0.0 {
        for _ in 0..columns {
            append(&mut rectangles, left, top, left + usable_width, bottom)?;
            left += usable_width + gap_x;
        }
        return Ok(Row { rectangles });
    }
    let length = usize::try_from(columns).map_err(|_| ImageError::Overflow)?;
    let mut widths = Vec::new();
    widths
        .try_reserve_exact(length)
        .map_err(|_| ImageError::Allocation)?;
    widths.resize(length, 0.0);
    let mut sample_widths = |start: i32, count: i32| -> Result<(), PointwiseError<E>> {
        let mut random = SeededRandom::from_seed(index).map_err(|_| ImageError::Overflow)?;
        for value in &mut widths[start as usize..start.wrapping_add(count) as usize] {
            *value = random.sample() * width_variation + 1.0;
        }
        Ok(())
    };
    if legacy_width_batches {
        // awu starts the same row seed at the beginning of each worker range.
        for_each_chunk(columns, execution, sample_widths)?;
    } else {
        sample_widths(0, columns)?;
    }
    let mut sum = 0.0;
    for value in &widths {
        sum += *value;
    }
    for_each_chunk::<E>(columns, execution, |start, count| {
        for i in start..start.wrapping_add(count) {
            let value = widths
                .get_mut(usize::try_from(i).map_err(|_| ImageError::Index)?)
                .ok_or(ImageError::Index)?;
            // Actual destination address/old value first; preserve grouping.
            let old = *value;
            *value = old * ((usable_width * f64::from(columns)) / sum);
        }
        Ok(())
    })?;
    for value in widths {
        append(&mut rectangles, left, top, left + value, bottom)?;
        left += value + gap_x;
    }
    Ok(Row { rectangles })
}
fn remainder<E>(value: i32, divisor: i32) -> Result<i32, PointwiseError<E>> {
    let result = value.checked_rem(divisor).ok_or(if divisor == 0 {
        PointwiseError::DivideByZero
    } else {
        PointwiseError::Image(ImageError::Overflow)
    })?;
    Ok(result)
}
/// Split X before Y and pass the source's interpolation ratios to each span.
fn visit_wrapped<E>(
    [left, top, right, bottom]: [i32; 4],
    geometry: ImageGeometry,
    ratios: [f32; 4],
    paint: &mut impl FnMut([i32; 4], [f32; 4]) -> Result<(), PointwiseError<E>>,
) -> Result<(), PointwiseError<E>> {
    let (width, height) = (geometry.width(), geometry.height());
    if left > right {
        let span = width.wrapping_sub(left).wrapping_add(right) as f32;
        visit_wrapped(
            [left, top, width, bottom],
            geometry,
            [0.0, width.wrapping_sub(left) as f32 / span, 0.0, 1.0],
            paint,
        )?;
        return visit_wrapped(
            [0, top, right, bottom],
            geometry,
            [(span - right as f32) / span, 1.0, 0.0, 1.0],
            paint,
        );
    }
    if top > bottom {
        let span = height.wrapping_sub(top).wrapping_add(bottom) as f32;
        visit_wrapped(
            [left, top, right, height],
            geometry,
            [0.0, 1.0, 0.0, height.wrapping_sub(top) as f32 / span],
            paint,
        )?;
        return visit_wrapped(
            [left, 0, right, bottom],
            geometry,
            [0.0, 1.0, (span - bottom as f32) / span, 1.0],
            paint,
        );
    }
    paint([left, top, right, bottom], ratios)
}
fn write_span<E>(
    buffer: &FloatBuffer,
    [left, top, right, bottom]: [i32; 4],
    width: i32,
    value: impl Fn(i32, i32) -> f32,
) -> Result<(), PointwiseError<E>> {
    for row in top..bottom {
        let offset = row.wrapping_mul(width);
        for column in left..right {
            let index = column.wrapping_add(offset);
            buffer.write(
                usize::try_from(index).map_err(|_| ImageError::Index)?,
                value(column, row),
            )?;
        }
    }
    Ok(())
}

/// Actual evaluated uq.k8 path. Other channels keep ImageMap defaults; channel0
/// remains scalar/unsealed when the first row is absent/empty. A successful draw
/// seals channel0 only after ALL rectangle fills. No RGB replication/AA substitute.
pub fn render_bricks<E>(
    options: &BricksRenderOptions<'_>,
    geometry: ImageGeometry,
) -> Result<SharedImageMap, PointwiseError<E>> {
    render_bricks_with_inputs(geometry, options.execution, &mut Evaluated(options))
}
pub(crate) fn render_bricks_with_inputs<E>(
    geometry: ImageGeometry,
    execution: ProceduralExecution,
    inputs: &mut impl BricksInputSource<E>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let image = SharedImageMap::new(geometry);
    let values = inputs.resolved_values()?;
    let layout = BrickLayout::build(geometry, execution, inputs, values, false, &|| false)?;
    if layout.has_rectangles() {
        let buffer = image.materialize(0)?;
        layout.draw(geometry, inputs, &|| false, |bounds| {
            visit_wrapped(
                wrap_bounds(bounds, geometry)?,
                geometry,
                [0.0, 1.0, 0.0, 1.0],
                &mut |span, _| write_span(&buffer, span, geometry.width(), |_, _| 1.0),
            )
        })?;
        image.seal_channel(0)?;
    }
    Ok(image)
}

struct BrickLayout {
    rows: Vec<Option<Row>>,
    signs: [f64; 2],
}

impl BrickLayout {
    fn build<E>(
        geometry: ImageGeometry,
        execution: ProceduralExecution,
        inputs: &mut impl BricksInputSource<E>,
        [density_x, density_y, border, shifts]: [f64; 4],
        legacy_width_batches: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, PointwiseError<E>> {
        let (first_size, second_size, location_x, location_y, shift) = if inputs.is_horizontal()? {
            let x = inputs.size_axis(0)?;
            let y = inputs.size_axis(1)?;
            let px = inputs.location_axis(0)?;
            let py = inputs.location_axis(1)?;
            let shift = shifts * inputs.size_axis(1)?;
            (x, y, px, py, shift)
        } else {
            let y = inputs.size_axis(1)?;
            let x = inputs.size_axis(0)?;
            let py = -inputs.location_axis(1)?;
            let px = -inputs.location_axis(0)?;
            let shift = shifts * inputs.size_axis(0)?;
            (y, x, py, px, shift)
        };
        let abs_x = first_size.abs();
        let abs_y = second_size.abs();
        let columns_double = density_y * abs_x;
        let rows_double = density_x * abs_y;
        let rows_count = integer(rows_double.ceil());
        let columns_count = integer(columns_double);
        let row_height = f64::from(geometry.height()) / rows_double;
        let row_shift =
            f64::from(geometry.width()) * shift / f64::from(rows_count) / f64::from(columns_count);
        let row_width =
            integer(f64::from(geometry.width()) / columns_double * f64::from(columns_count));
        let offset_x = location_x * f64::from(geometry.width()) / abs_x;
        // This repeats actual SizeY even in the vertical layout branch.
        let offset_y =
            (location_y + inputs.size_axis(1)? - 1.0) * f64::from(geometry.height()) / abs_y;
        let length = usize::try_from(rows_count).map_err(|_| ImageError::Overflow)?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(length)
            .map_err(|_| ImageError::Allocation)?;
        rows.resize_with(length, || None);
        let mut row = 0i32;
        while f64::from(row) < rows_double {
            if cancelled() {
                return Err(PointwiseError::Cancelled);
            }
            let width_variation = inputs.width_variation()?;
            let slide_variation = inputs.slide_variation()?;
            let value = build_row::<E>(
                row,
                columns_count,
                f64::from(row_width),
                row_height,
                row_shift * f64::from(row) - offset_x,
                row_height * f64::from(row) + offset_y,
                border,
                width_variation,
                slide_variation,
                abs_x,
                abs_y,
                execution,
                legacy_width_batches,
            )?;
            *rows
                .get_mut(usize::try_from(row).map_err(|_| ImageError::Index)?)
                .ok_or(ImageError::Index)? = Some(value);
            row = row.wrapping_add(1);
        }
        Ok(Self {
            rows,
            signs: [first_size, second_size],
        })
    }

    fn has_rectangles(&self) -> bool {
        self.rows
            .first()
            .and_then(Option::as_ref)
            .is_some_and(|row| !row.rectangles.is_empty())
    }

    fn draw<E>(
        self,
        geometry: ImageGeometry,
        inputs: &mut impl BricksInputSource<E>,
        cancelled: &dyn Fn() -> bool,
        mut paint: impl FnMut([i32; 4]) -> Result<(), PointwiseError<E>>,
    ) -> Result<(), PointwiseError<E>> {
        let [first_size, second_size] = self.signs;
        for row in self.rows {
            if cancelled() {
                return Err(PointwiseError::Cancelled);
            }
            let row = row.ok_or(ImageError::NullReference)?;
            for rectangle in row.rectangles {
                let horizontal = inputs.is_horizontal()?;
                let (mut left, mut top, mut right, mut bottom) = if horizontal {
                    (
                        integer(rectangle.left + 0.5),
                        integer(rectangle.top + 0.5),
                        integer(rectangle.right + 0.5),
                        integer(rectangle.bottom + 0.5),
                    )
                } else {
                    (
                        integer(rectangle.top + 0.5),
                        integer(rectangle.left + 0.5),
                        integer(rectangle.bottom + 0.5),
                        integer(rectangle.right + 0.5),
                    )
                };
                if first_size < 0.0 {
                    std::mem::swap(&mut left, &mut right);
                    left = left.wrapping_neg();
                    right = right.wrapping_neg();
                }
                if second_size < 0.0 {
                    std::mem::swap(&mut top, &mut bottom);
                    top = top.wrapping_neg();
                    bottom = bottom.wrapping_neg();
                }
                if geometry.width() < 0 || geometry.height() < 0 {
                    return Err(PointwiseError::UnsupportedCapability(
                        "Bricks recursive fill with nonpositive geometry is uncharacterized",
                    ));
                }
                paint([left, top, right, bottom])?;
            }
        }
        Ok(())
    }
}

fn wrap_bounds<E>(
    mut bounds: [i32; 4],
    geometry: ImageGeometry,
) -> Result<[i32; 4], PointwiseError<E>> {
    for (index, edge) in bounds.iter_mut().enumerate() {
        let size = if index % 2 == 0 {
            geometry.width()
        } else {
            geometry.height()
        };
        *edge = remainder(*edge, size)?;
        if *edge < 0 {
            *edge = edge.wrapping_add(size);
        }
    }
    Ok(bounds)
}

#[derive(Clone, Copy)]
pub(crate) enum LegacyBrickFill {
    Plain,
    Color,
    HorizontalGradient,
    VerticalGradient,
}

pub(crate) fn render_legacy_bricks<E>(
    geometry: ImageGeometry,
    execution: ProceduralExecution,
    inputs: &mut impl BricksInputSource<E>,
    fill: LegacyBrickFill,
    color: &SharedImageMap,
    cancelled: &dyn Fn() -> bool,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let image = SharedImageMap::new(geometry);
    let values = inputs.resolved_values()?;
    let mut planes = Vec::new();
    if let LegacyBrickFill::Color = fill {
        let count = color.channel_count()?;
        planes
            .try_reserve_exact(count)
            .map_err(|_| ImageError::Allocation)?;
        for channel in 0..count as i32 {
            planes.push((color.channel(channel, false)?, image.materialize(channel)?));
        }
    }
    let layout = BrickLayout::build(geometry, execution, inputs, values, true, cancelled)?;
    if !layout.has_rectangles() {
        return Ok(image);
    }
    let red = image.materialize(0)?;
    layout.draw(
        geometry,
        inputs,
        cancelled,
        |bounds @ [left, top, right, bottom]| {
            // Color samples the unwrapped center and only then wraps the rectangle.
            let center = if matches!(fill, LegacyBrickFill::Color) {
                let x = left.wrapping_add(right) / 2;
                let y = top.wrapping_add(bottom) / 2;
                let [x, y, ..] = wrap_bounds([x, y, x, y], geometry)?;
                Some((x + y * geometry.width()) as usize)
            } else {
                None
            };
            visit_wrapped(
                wrap_bounds(bounds, geometry)?,
                geometry,
                [0.0, 1.0, 0.0, 1.0],
                &mut |span @ [left, top, right, bottom], [x_start, x_end, y_start, y_end]| {
                    match fill {
                        LegacyBrickFill::Plain => {
                            write_span(&red, span, geometry.width(), |_, _| 1.0)?;
                        }
                        LegacyBrickFill::Color => {
                            let center = center.ok_or(PointwiseError::NullReference)?;
                            for (source, output) in &planes {
                                let value =
                                    source.read(if source.len() == 1 { 0 } else { center })?;
                                write_span(output, span, geometry.width(), |_, _| value)?;
                            }
                        }
                        LegacyBrickFill::HorizontalGradient => {
                            let extent =
                                right.wrapping_sub(left) as f32 / ((1.0 - x_start) * x_end);
                            let offset = extent * x_start;
                            write_span(&red, span, geometry.width(), |x, _| {
                                ((x - left) as f32 + offset) / (extent * 0.99) + 0.01
                            })?;
                        }
                        LegacyBrickFill::VerticalGradient => {
                            let extent =
                                bottom.wrapping_sub(top) as f32 / ((1.0 - y_start) * y_end);
                            let offset = extent * y_start;
                            write_span(&red, span, geometry.width(), |_, y| {
                                ((y - top) as f32 + offset) / (extent * 0.99) + 0.01
                            })?;
                        }
                    }
                    Ok(())
                },
            )
        },
    )?;
    Ok(image)
}

#[cfg(test)]
#[path = "procedural_bricks_test.rs"]
mod tests;
