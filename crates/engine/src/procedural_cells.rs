//! Literal Cells and Wood Rings channel-zero kernels over effective parameters.
//! These values are an immutable render projection, not retained property state.
//! TexTree i1 060029cd/ce; av9 060029da/db. Source parity is not established by
//! the static laws in this module's test companion.
use crate::framework_cast::integer;
use crate::procedural_schedule::{chunks, for_each_dense_band};
use crate::procedural_sites::{Site, SiteGrid, SiteGridExecution, SiteGridOptions, build_grid};
use crate::{FloatBuffer, ImageError, ImageGeometry, PointwiseError, SharedImageMap};

const CELLS_TEXT: &[u16] = &[67, 101, 108, 108, 115];

/// Actual evaluated Size/Location/scalars and nullable UTF-16 mode text for one
/// Cells render. No index-to-mode fallback or text normalization is performed.
#[derive(Clone, Copy, Debug)]
pub struct CellsRenderOptions<'a> {
    size: [f64; 2],
    location: [f64; 2],
    density: f64,
    seed: f64,
    irregularity: f64,
    distribution: f64,
    mode_text: Option<&'a [u16]>,
    execution: SiteGridExecution,
}
impl<'a> CellsRenderOptions<'a> {
    pub const fn new(size: [f64; 2], location: [f64; 2], execution: SiteGridExecution) -> Self {
        Self {
            size,
            location,
            density: 1.0,
            seed: 0.0,
            irregularity: 0.0,
            distribution: 1.0,
            mode_text: Some(CELLS_TEXT),
            execution,
        }
    }
    pub const fn with_density(mut self, value: f64) -> Self {
        self.density = value;
        self
    }
    pub const fn with_seed(mut self, value: f64) -> Self {
        self.seed = value;
        self
    }
    pub const fn with_irregularity(mut self, value: f64) -> Self {
        self.irregularity = value;
        self
    }
    pub const fn with_distribution(mut self, value: f64) -> Self {
        self.distribution = value;
        self
    }
    pub const fn with_mode_text(mut self, value: Option<&'a [u16]>) -> Self {
        self.mode_text = value;
        self
    }
    pub const fn size(self) -> [f64; 2] {
        self.size
    }
    pub const fn location(self) -> [f64; 2] {
        self.location
    }
    pub const fn density(self) -> f64 {
        self.density
    }
    pub const fn seed(self) -> f64 {
        self.seed
    }
    pub const fn irregularity(self) -> f64 {
        self.irregularity
    }
    pub const fn distribution(self) -> f64 {
        self.distribution
    }
    pub const fn mode_text(self) -> Option<&'a [u16]> {
        self.mode_text
    }
    pub const fn execution(self) -> SiteGridExecution {
        self.execution
    }
}

/// Actual evaluated Wood Rings values. The original constructor sets Size X8
/// only; callers supply the inherited/evaluated Y rather than an invented pair.
#[derive(Clone, Copy, Debug)]
pub struct WoodRingsRenderOptions {
    size: [f64; 2],
    location: [f64; 2],
    density: f64,
    anomalies: f64,
    seed: f64,
    blend_width: f64,
    execution: SiteGridExecution,
}
impl WoodRingsRenderOptions {
    pub const fn new(size: [f64; 2], location: [f64; 2], execution: SiteGridExecution) -> Self {
        Self {
            size,
            location,
            density: 1.0,
            anomalies: 1.0,
            seed: 0.0,
            blend_width: 3.0,
            execution,
        }
    }
    pub const fn with_density(mut self, value: f64) -> Self {
        self.density = value;
        self
    }
    pub const fn with_anomalies(mut self, value: f64) -> Self {
        self.anomalies = value;
        self
    }
    pub const fn with_seed(mut self, value: f64) -> Self {
        self.seed = value;
        self
    }
    pub const fn with_blend_width(mut self, value: f64) -> Self {
        self.blend_width = value;
        self
    }
    pub const fn size(self) -> [f64; 2] {
        self.size
    }
    pub const fn location(self) -> [f64; 2] {
        self.location
    }
    pub const fn density(self) -> f64 {
        self.density
    }
    pub const fn anomalies(self) -> f64 {
        self.anomalies
    }
    pub const fn seed(self) -> f64 {
        self.seed
    }
    pub const fn blend_width(self) -> f64 {
        self.blend_width
    }
    pub const fn execution(self) -> SiteGridExecution {
        self.execution
    }
}

fn seed_route(seed: f64) -> (f64, bool) {
    if seed < 0.0 {
        (-(seed + 1.0), true)
    } else {
        (seed, false)
    }
}
fn text_is(text: Option<&[u16]>, literal: &str) -> bool {
    text.is_some_and(|text| text.iter().copied().eq(literal.encode_utf16()))
}
fn mode_selectors(text: Option<&[u16]>) -> (i32, i32) {
    if text_is(text, "Cells") {
        (1, 1)
    } else if text_is(text, "Bubbles") {
        (1, 2)
    } else if text_is(text, "Jagged Cells") {
        (2, 1)
    } else if text_is(text, "Circles") {
        (1, 3)
    } else if text_is(text, "Squares") {
        (3, 3)
    } else if text_is(text, "Square Bubbles") {
        (2, 2)
    } else if text_is(text, "Manhattan") {
        (3, 4)
    } else {
        (0, 0)
    }
}

pub fn render_cells<E>(
    options: &CellsRenderOptions<'_>,
    geometry: ImageGeometry,
) -> Result<SharedImageMap, PointwiseError<E>> {
    render_cells_with_inputs(geometry, || Ok(*options), &mut || {
        Ok(options.mode_text.map(<[u16]>::to_vec))
    })
}
/// Same body with retained getters at the source's actual access points.
pub(crate) fn render_cells_with_inputs<'a, E>(
    geometry: ImageGeometry,
    inputs: impl FnOnce() -> Result<CellsRenderOptions<'a>, PointwiseError<E>>,
    mode: &mut impl FnMut() -> Result<Option<Vec<u16>>, PointwiseError<E>>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let image = SharedImageMap::new(geometry);
    // Actual nn(0), before seed and grid work. Preserve the resulting plane
    // even for a null grid; no nw replacement or nl seal occurs in i1.k8.
    let output = image.materialize(0)?;
    let options = inputs()?;
    let (seed, alternate) = seed_route(options.seed);
    let Some(grid) = build_grid(
        SiteGridOptions {
            size: options.size,
            location: options.location,
            density: options.density,
            distribution: options.distribution,
            seed,
            irregularity: options.irregularity,
            cells: true,
            alternate,
        },
        geometry,
        options.execution,
    )?
    else {
        return Ok(image);
    };
    let normalizer = 1.0f32 / (30.0 * (1.0 + (options.density as f32 - 1.0) / 10.0));
    let mode_text = mode()?;
    let (metric, result) = mode_selectors(mode_text.as_deref());
    let scale = [
        grid.span[0] / geometry.width() as f32,
        grid.span[1] / geometry.height() as f32,
    ];
    // Retain the scheduler's validation before entering the independent raster pass.
    let schedule = chunks::<E>(geometry.area(), options.execution)?;
    if geometry.width() <= 0
        || geometry.height() <= 0
        || geometry
            .width()
            .checked_mul(geometry.height())
            .map(|area| area as usize)
            != Some(output.len())
    {
        for (start, count) in schedule {
            render_cells_chunk(
                &output, &grid, geometry, scale, metric, result, normalizer, start, count,
            )?;
        }
        return Ok(image);
    }
    let [grid_width, grid_height, _] = grid.dimensions();
    let width = geometry.width() as usize;
    let height = f64::from(geometry.height());
    let column = |x: usize| {
        let gx = integer(x as f64 / width as f64 * f64::from(grid_width));
        (gx, x as f32 * scale[0])
    };
    let mut columns = Vec::new();
    columns
        .try_reserve_exact(width)
        .map_err(|_| ImageError::Allocation)?;
    columns.extend((0..width).map(column));
    output.with_mut_slice(|pixels| {
        for_each_dense_band(pixels, width, options.execution, |start, band| {
            for (offset, row) in band.chunks_exact_mut(width).enumerate() {
                let y = start + offset;
                let grid_y = integer(y as f64 / height * f64::from(grid_height));
                let sample_y = y as f32 * scale[1];
                for (pixel, &(grid_x, sample_x)) in row.iter_mut().zip(&columns) {
                    *pixel = cell_value(
                        grid.cell_sites(grid_x, grid_y)?,
                        sample_x,
                        sample_y,
                        metric,
                        result,
                        normalizer,
                    );
                }
            }
            Ok(())
        })
    })?;
    Ok(image)
}

fn cell_value(
    sites: impl Iterator<Item = Site>,
    x: f32,
    y: f32,
    metric: i32,
    result: i32,
    normalizer: f32,
) -> f32 {
    let mut first = f32::MAX;
    let mut second = f32::MAX;
    for site in sites {
        let distance = match metric {
            1 => site.squared_distance(x, y),
            2 => site.manhattan_distance(x, y),
            _ => site.maximum_distance(x, y),
        };
        if distance < first {
            second = first;
            first = distance;
        } else if distance < second {
            second = distance;
        }
    }
    if metric == 1 {
        first = libm::sqrt(f64::from(first)) as f32;
        second = libm::sqrt(f64::from(second)) as f32;
    }
    match result {
        1 => 1.0 - first / second,
        2 => 1.0 - (first + second) * normalizer,
        3 => 1.0 - first * 0.07,
        _ => 1.0 - second * 0.07,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_cells_chunk<E>(
    output: &FloatBuffer,
    grid: &SiteGrid,
    geometry: ImageGeometry,
    scale: [f32; 2],
    metric: i32,
    result: i32,
    normalizer: f32,
    start: i32,
    count: i32,
) -> Result<(), PointwiseError<E>> {
    let [grid_width, grid_height, depth] = grid.dimensions();
    if geometry.width() == 0 {
        return Err(PointwiseError::DivideByZero);
    }
    let mut row = start / geometry.width();
    let mut x = start % geometry.width();
    let mut grid_y =
        integer(f64::from(row) / f64::from(geometry.height()) * f64::from(grid_height));
    let mut sample_y = row as f32 * scale[1];
    for index in start..start.wrapping_add(count) {
        if x >= geometry.width() {
            x = 0;
            row = row.wrapping_add(1);
            grid_y =
                integer(f64::from(row) / f64::from(geometry.height()) * f64::from(grid_height));
            sample_y = row as f32 * scale[1];
        }
        let sample_x = x as f32 * scale[0];
        let grid_x = integer(f64::from(x) / f64::from(geometry.width()) * f64::from(grid_width));
        let mut first = f32::MAX;
        let mut second = f32::MAX;
        for rank in 0..depth {
            let Some(site) = grid.candidate(grid_x, grid_y, rank)? else {
                break;
            };
            let distance = if metric == 1 {
                site.squared_distance(sample_x, sample_y)
            } else if metric == 2 {
                site.manhattan_distance(sample_x, sample_y)
            } else {
                site.maximum_distance(sample_x, sample_y)
            };
            if distance < first {
                second = first;
                first = distance;
            } else if distance < second {
                second = distance;
            }
        }
        if metric == 1 {
            first = libm::sqrt(f64::from(first)) as f32;
            second = libm::sqrt(f64::from(second)) as f32;
        }
        let value = if result == 1 {
            1.0 - first / second
        } else if result == 2 {
            1.0 - (first + second) * normalizer
        } else if result == 3 {
            1.0 - first * 0.07
        } else {
            1.0 - second * 0.07
        };
        output.write(index as usize, value)?;
        x = x.wrapping_add(1);
    }
    Ok(())
}

pub fn render_wood_rings<E>(
    options: &WoodRingsRenderOptions,
    geometry: ImageGeometry,
) -> Result<SharedImageMap, PointwiseError<E>> {
    render_wood_with_inputs(geometry, || Ok(*options))
}
pub(crate) fn render_wood_with_inputs<E>(
    geometry: ImageGeometry,
    inputs: impl FnOnce() -> Result<WoodRingsRenderOptions, PointwiseError<E>>,
) -> Result<SharedImageMap, PointwiseError<E>> {
    let image = SharedImageMap::new(geometry);
    let output = image.materialize(0)?;
    let options = inputs()?;
    let (seed, alternate) = seed_route(options.seed);
    let mut blend = options.blend_width as f32;
    if blend == 0.0 {
        blend = f32::from_bits(1);
    }
    let reciprocal = 1.0f32 / blend;
    let density = options.density as f32;
    let Some(grid) = build_grid(
        SiteGridOptions {
            size: options.size,
            location: options.location,
            density: options.anomalies,
            distribution: 1.0,
            seed,
            irregularity: 1.0,
            cells: false,
            alternate,
        },
        geometry,
        options.execution,
    )?
    else {
        return Ok(image);
    };
    let depth = grid.dimensions()[2];
    // The source really allocates this otherwise-unused array before scales.
    let mut scratch = Vec::<f32>::new();
    scratch
        .try_reserve_exact(depth as usize)
        .map_err(|_| crate::ImageError::Allocation)?;
    scratch.resize(depth as usize, 0.0);
    let scale = [
        grid.span[0] / geometry.width() as f32,
        grid.span[1] / geometry.height() as f32,
    ];
    for (start, count) in chunks(geometry.area(), options.execution)? {
        render_wood_chunk(
            &output,
            &grid,
            geometry,
            scale,
            reciprocal,
            density,
            scratch.len(),
            start,
            count,
        )?;
    }
    Ok(image)
}

fn ring_value(mut first: f32, second: f32, reciprocal: f32, density: f32) -> f32 {
    let mut blend = (second - first) * reciprocal + 0.5;
    if blend < 1.0 {
        blend = (1.0 - libm::cos(f64::from(blend) * std::f64::consts::PI) as f32) * 0.5;
        first = blend * first + (1.0 - blend) * second;
    }
    first *= density;
    first %= 1.0;
    if first < 0.0 {
        first += 1.0;
    }
    first
}
#[allow(clippy::too_many_arguments)]
fn render_wood_chunk<E>(
    output: &FloatBuffer,
    grid: &SiteGrid,
    geometry: ImageGeometry,
    scale: [f32; 2],
    reciprocal: f32,
    density: f32,
    depth: usize,
    start: i32,
    count: i32,
) -> Result<(), PointwiseError<E>> {
    let [grid_width, grid_height, _] = grid.dimensions();
    if geometry.width() == 0 {
        return Err(PointwiseError::DivideByZero);
    }
    let mut row = start / geometry.width();
    let mut x = start % geometry.width();
    let mut grid_y =
        integer(f64::from(row) / f64::from(geometry.height()) * f64::from(grid_height));
    for index in start..start.wrapping_add(count) {
        if x >= geometry.width() {
            x = 0;
            row = row.wrapping_add(1);
            grid_y =
                integer(f64::from(row) / f64::from(geometry.height()) * f64::from(grid_height));
        }
        let sample_y = row as f32 * scale[1];
        let sample_x = x as f32 * scale[0];
        let grid_x = integer(f64::from(x) / f64::from(geometry.width()) * f64::from(grid_width));
        let mut first = f32::MAX;
        let mut second = f32::MAX;
        for rank in 0..depth {
            let Some(site) = grid.candidate(grid_x, grid_y, rank as i32)? else {
                break;
            };
            let distance = site.squared_distance(sample_x, sample_y);
            if distance < first {
                second = first;
                first = distance;
            } else if distance < second {
                second = distance;
            }
        }
        first = libm::sqrt(f64::from(first)) as f32;
        second = libm::sqrt(f64::from(second)) as f32;
        output.write(
            index as usize,
            ring_value(first, second, reciprocal, density),
        )?;
        x = x.wrapping_add(1);
    }
    Ok(())
}

#[cfg(test)]
#[path = "procedural_cells_static_test.rs"]
mod static_test;
