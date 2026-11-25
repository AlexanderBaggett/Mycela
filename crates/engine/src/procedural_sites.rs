//! Shared Cells/Wood site arena and nullable candidate grid.
//!
//! Source: TexTree avb (060029b8..bf) and ao4 (060029c0..c3).
//! Stable arena IDs stand for source object identity, including geometrically
//! identical but separately allocated wrapped sites. No distance sort replaces
//! the two source candidate-selection/deduplication passes.
use crate::framework_cast::integer;
use crate::procedural_lattice::{SeededLattice, coordinate_hash};
use crate::seeded_random::SeededRandom;
use crate::{ImageError, ImageGeometry};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SiteId(usize);

#[derive(Clone, Copy)]
enum AxisShift {
    Unchanged,
    Add(f32),
    Subtract(f32),
}
impl AxisShift {
    fn apply(self, value: f32) -> f32 {
        match self {
            Self::Unchanged => value,
            Self::Add(by) => value + by,
            Self::Subtract(by) => value - by,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Site {
    start: [f32; 3],
    delta: [f32; 3],
    projection: f32,
    inverse_length: f32,
    point: bool,
}
impl Site {
    fn new(start: [f32; 3], end: [f32; 3]) -> Self {
        let delta = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
        let projection = -start[0] * delta[0] - start[1] * delta[1] - start[2] * delta[2];
        let length_squared = delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2];
        Self {
            start,
            delta,
            projection,
            inverse_length: (1.0 / libm::sqrt(f64::from(length_squared))) as f32,
            point: delta[0] == 0.0 && delta[1] == 0.0,
        }
    }
    fn components(self, x: f32, y: f32) -> [f32; 3] {
        // This is the literal source projection, not a geometric correction.
        let t = (x * self.delta[0] + y * self.delta[1] + self.projection) * self.inverse_length;
        [
            self.start[0] + t - x,
            self.start[1] + t - y,
            self.start[2] + t,
        ]
    }
    pub(crate) fn squared_distance(self, x: f32, y: f32) -> f32 {
        if self.point {
            let a = x - self.start[0];
            let b = y - self.start[1];
            a * a + b * b
        } else {
            let [a, b, c] = self.components(x, y);
            a * a + b * b + c * c
        }
    }
    pub(crate) fn manhattan_distance(self, x: f32, y: f32) -> f32 {
        if self.point {
            branch_abs(x - self.start[0]) + branch_abs(y - self.start[1])
        } else {
            let [a, b, c] = self.components(x, y);
            branch_abs(a) + branch_abs(b) + branch_abs(c)
        }
    }
    pub(crate) fn maximum_distance(self, x: f32, y: f32) -> f32 {
        if self.point {
            let a = branch_abs(x - self.start[0]);
            let b = branch_abs(y - self.start[1]);
            if a > b { a } else { b }
        } else {
            let [a, b, c] = self.components(x, y).map(branch_abs);
            if a > b {
                if a > c { a } else { c }
            } else if b > c {
                b
            } else {
                c
            }
        }
    }
}
fn branch_abs(value: f32) -> f32 {
    if value < 0.0 { -value } else { value }
}
fn minimum(a: f64, b: f64) -> f64 {
    if a < b || a.is_nan() { a } else { b }
}
fn maximum(a: f64, b: f64) -> f64 {
    if a > b || a.is_nan() { a } else { b }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SiteGridOptions {
    pub size: [f64; 2],
    pub location: [f64; 2],
    pub density: f64,
    pub distribution: f64,
    pub seed: f64,
    pub irregularity: f64,
    pub cells: bool,
    pub alternate: bool,
}

pub(crate) struct SiteGrid {
    sites: Vec<Site>,
    candidates: Vec<Option<SiteId>>,
    dimensions: [i32; 3],
    pub span: [f32; 2],
}
impl SiteGrid {
    pub(crate) fn dimensions(&self) -> [i32; 3] {
        self.dimensions
    }
    fn offset(&self, x: i32, y: i32, rank: i32) -> Result<usize, ImageError> {
        if x < 0
            || x >= self.dimensions[0]
            || y < 0
            || y >= self.dimensions[1]
            || rank < 0
            || rank >= self.dimensions[2]
        {
            return Err(ImageError::Index);
        }
        Ok(
            (x as usize * self.dimensions[1] as usize + y as usize) * self.dimensions[2] as usize
                + rank as usize,
        )
    }
    pub(crate) fn candidate(&self, x: i32, y: i32, rank: i32) -> Result<Option<Site>, ImageError> {
        Ok(self.candidates[self.offset(x, y, rank)?].map(|id| self.sites[id.0]))
    }
}

fn allocated<T: Clone>(length: i32, value: T) -> Result<Vec<T>, ImageError> {
    let length = usize::try_from(length).map_err(|_| ImageError::Overflow)?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(length)
        .map_err(|_| ImageError::Allocation)?;
    result.resize(length, value);
    Ok(result)
}
fn empty_grid(dimensions: [i32; 3]) -> Result<Vec<Option<SiteId>>, ImageError> {
    let mut length = 1usize;
    for dimension in dimensions {
        let dimension = usize::try_from(dimension).map_err(|_| ImageError::Overflow)?;
        length = length
            .checked_mul(dimension)
            .ok_or(ImageError::Allocation)?;
    }
    let mut grid = Vec::new();
    grid.try_reserve_exact(length)
        .map_err(|_| ImageError::Allocation)?;
    grid.resize(length, None);
    Ok(grid)
}
fn append(sites: &mut Vec<Site>, start: [f32; 3], end: [f32; 3]) -> Result<(), ImageError> {
    sites.try_reserve(1).map_err(|_| ImageError::Allocation)?;
    sites.push(Site::new(start, end));
    Ok(())
}

pub use crate::procedural_schedule::ProceduralExecution as SiteGridExecution;
use crate::procedural_schedule::chunks;

pub(crate) fn build_grid<E>(
    options: SiteGridOptions,
    geometry: ImageGeometry,
    execution: SiteGridExecution,
) -> Result<Option<SiteGrid>, crate::PointwiseError<E>> {
    let density_count = integer(options.density);
    let size = options
        .size
        .map(|value| f64::from(integer(value).clamp(-100, 100)));
    let mut span = size.map(|value| (value * 10.0) as f32);
    let mut offset = [
        (options.location[0] * 10.0) as f32,
        -span[1] - (options.location[1] * 10.0) as f32,
    ];
    let end = [span[0] + offset[0], span[1] + offset[1]];
    let mut first = offset.map(|value| integer((f64::from(value) / 10.0).floor()));
    let mut last = end.map(|value| integer((f64::from(value) / 10.0).floor()));
    for axis in 0..2 {
        if last[axis] < first[axis] {
            std::mem::swap(&mut first[axis], &mut last[axis]);
            span[axis] = -span[axis];
            offset[axis] -= span[axis];
        }
    }
    let spread = if options.distribution == 0.0 {
        1e-6
    } else {
        options.distribution
    };
    let centered = 0.5 * (1.0 - spread);
    let line_scale = if options.cells { 0.0001f32 } else { 1.0 };
    let one = [
        first[0].wrapping_add(1) == last[0],
        first[1].wrapping_add(1) == last[1],
    ];
    let mut seed = options.seed;
    let lattice_seed = integer(seed).wrapping_add(i32::from(options.cells));
    let lattice = SeededLattice::new(lattice_seed)?;
    let mut sites = Vec::new();
    for cell_x in first[0]..last[0] {
        for cell_y in first[1]..last[1] {
            let mut random = if options.alternate {
                // Source loops for an infinite seed. Do not report a successful
                // finite texture for that nonterminating source request.
                if seed == f64::INFINITY {
                    return Err(crate::PointwiseError::UnsupportedCapability(
                        "infinite alternate site seed does not terminate",
                    ));
                }
                while seed > 10000.0 {
                    seed /= 4.942381635;
                }
                let seed_offset = if options.cells {
                    (seed + 10.0) as f32
                } else {
                    seed as f32
                };
                let hash =
                    coordinate_hash(cell_x as f32 + seed_offset / 1.2345988f32, cell_y as f32);
                Some(
                    SeededRandom::from_seed(integer(f64::from(100000f32 * hash)))
                        .map_err(|_| ImageError::Overflow)?,
                )
            } else {
                None
            };
            let mut sample_index = 0i32;
            for _ in 0..density_count {
                let mut sample = || {
                    if let Some(random) = random.as_mut() {
                        random.sample()
                    } else {
                        let value = f64::from(lattice.sample3(cell_x, cell_y, sample_index));
                        sample_index = sample_index.wrapping_add(1);
                        value
                    }
                };
                let x =
                    ((centered + sample() * spread + f64::from(cell_x)) * 10.0) as f32 - offset[0];
                let y =
                    ((centered + sample() * spread + f64::from(cell_y)) * 10.0) as f32 - offset[1];
                let jitter_x = ((sample() * 10.0 - 5.0) * options.irregularity) as f32 * line_scale;
                let jitter_y = ((sample() * 10.0 - 5.0) * options.irregularity) as f32 * line_scale;
                let start = [x + jitter_x, y + jitter_y, 5.0 * line_scale];
                let finish = [x - jitter_x, y - jitter_y, -5.0 * line_scale];
                append(&mut sites, start, finish)?;
                let left = cell_x <= first[0].wrapping_add(1);
                let right = cell_x >= last[0].wrapping_sub(2);
                let bottom = cell_y <= first[1].wrapping_add(1);
                let top = cell_y >= last[1].wrapping_sub(2);
                let mut shifted = |dx: AxisShift, dy: AxisShift| {
                    append(
                        &mut sites,
                        [dx.apply(start[0]), dy.apply(start[1]), start[2]],
                        [dx.apply(finish[0]), dy.apply(finish[1]), finish[2]],
                    )
                };
                use AxisShift::{Add, Subtract, Unchanged};
                // Keep literal append AND arithmetic order. An unchanged axis
                // is copied, not added to +0; negative shifts use subtraction.
                if left {
                    shifted(Add(span[0]), Unchanged)?;
                    if one[0] {
                        shifted(Add(span[0] * 2.0), Unchanged)?;
                    }
                }
                if right {
                    shifted(Subtract(span[0]), Unchanged)?;
                    if one[0] {
                        shifted(Subtract(span[0] * 2.0), Unchanged)?;
                    }
                }
                if bottom {
                    shifted(Unchanged, Add(span[1]))?;
                    if one[1] {
                        shifted(Unchanged, Add(span[1] * 2.0))?;
                    }
                }
                if top {
                    shifted(Unchanged, Subtract(span[1]))?;
                    if one[1] {
                        shifted(Unchanged, Subtract(span[1] * 2.0))?;
                    }
                }
                if left && bottom {
                    shifted(Add(span[0]), Add(span[1]))?;
                    if one[0] {
                        shifted(Add(span[0] * 2.0), Add(span[1]))?;
                    }
                    if one[1] {
                        shifted(Add(span[0]), Add(span[1] * 2.0))?;
                    }
                }
                if left && top {
                    shifted(Add(span[0]), Subtract(span[1]))?;
                    if one[0] {
                        shifted(Add(span[0] * 2.0), Subtract(span[1]))?;
                    }
                    if one[1] {
                        shifted(Add(span[0]), Subtract(span[1] * 2.0))?;
                    }
                }
                if right && bottom {
                    shifted(Subtract(span[0]), Add(span[1]))?;
                    if one[0] {
                        shifted(Subtract(span[0] * 2.0), Add(span[1]))?;
                    }
                    if one[1] {
                        shifted(Subtract(span[0]), Add(span[1] * 2.0))?;
                    }
                }
                if right && top {
                    shifted(Subtract(span[0]), Subtract(span[1]))?;
                    if one[0] {
                        shifted(Subtract(span[0] * 2.0), Subtract(span[1]))?;
                    }
                    if one[1] {
                        shifted(Subtract(span[0]), Subtract(span[1] * 2.0))?;
                    }
                }
            }
        }
    }
    if sites.is_empty() {
        return Ok(None);
    }
    let mut width = integer(size[0].abs());
    if width < 16 {
        width = 16.min(geometry.width());
    }
    let mut height = integer(size[1].abs());
    if height < 16 {
        height = 16.min(geometry.height());
    }
    let max_size = maximum(size[0].abs(), size[1].abs());
    let mut depth = 9.min(3i32.wrapping_add(integer(max_size)));
    depth = depth.wrapping_add(integer(max_size * 0.15));
    depth = depth.wrapping_add(integer(minimum(options.irregularity.abs(), 1.3) * 6.0));
    if options.density > 1.0 {
        let density = options.density * (1.0 + max_size * 0.03);
        depth = depth.wrapping_mul(integer(maximum(1.0, density * 0.05)));
        depth = depth.wrapping_add(integer(minimum(density, 10.0)));
        let mut expansion = (1.0 - options.distribution) * 0.3 + 1.0;
        expansion += minimum(0.3, density * 0.03);
        width = integer(f64::from(width) * expansion);
        height = integer(f64::from(height) * expansion);
        depth = integer(f64::from(depth) * expansion);
    }
    if depth > 100 {
        depth = 100;
    }
    let dimensions = [width, height, depth];
    if dimensions.iter().any(|dimension| *dimension < 0) {
        return Err(crate::PointwiseError::UnsupportedCapability(
            "negative site-grid rectangular allocation error is unqualified",
        ));
    }
    let candidates = empty_grid(dimensions)?;
    let mut grid = SiteGrid {
        sites,
        candidates,
        dimensions,
        span,
    };
    for (start, count) in chunks(width.wrapping_mul(height), execution)? {
        choose_candidates(&mut grid, start, count)?;
    }
    // The second rectangular allocation happens only after the first worker.
    let mut deduplicated = empty_grid(dimensions)?;
    for (start, count) in chunks(width.wrapping_mul(height), execution)? {
        deduplicate_candidates(&grid, &mut deduplicated, start, count)?;
    }
    grid.candidates = deduplicated;
    Ok(Some(grid))
}

fn choose_candidates(grid: &mut SiteGrid, start: i32, count: i32) -> Result<(), ImageError> {
    let [width, height, depth] = grid.dimensions;
    // One index array per actual bx invocation; do NOT clear it per cell.
    let mut selected = allocated(depth, 0usize)?;
    for cell in start..start.wrapping_add(count) {
        let x = cell % width;
        let y = cell / width;
        let center_x = (x as f32 + 0.5) / width as f32 * grid.span[0];
        let center_y = (y as f32 + 0.5) / height as f32 * grid.span[1];
        let mut distances = Vec::new();
        distances
            .try_reserve_exact(grid.sites.len())
            .map_err(|_| ImageError::Allocation)?;
        for site in &grid.sites {
            distances.push(site.squared_distance(center_x, center_y));
        }
        for rank in 0..selected.len() {
            let mut best = f32::MAX;
            for (index, distance) in distances.iter().copied().enumerate() {
                if distance.partial_cmp(&best) != Some(std::cmp::Ordering::Less) {
                    continue;
                }
                let mut used = false;
                for earlier in (0..rank).rev() {
                    if index == selected[earlier] {
                        used = true;
                        break;
                    }
                }
                if !used {
                    best = distance;
                    selected[rank] = index;
                }
            }
        }
        for (rank, index) in selected.iter().copied().enumerate() {
            let offset = grid.offset(x, y, rank as i32)?;
            grid.candidates[offset] = Some(SiteId(index));
        }
    }
    Ok(())
}
fn deduplicate_candidates(
    grid: &SiteGrid,
    destination: &mut [Option<SiteId>],
    start: i32,
    count: i32,
) -> Result<(), ImageError> {
    let [width, _, depth] = grid.dimensions;
    for cell in start..start.wrapping_add(count) {
        let x = cell % width;
        let y = cell / width;
        let mut cursor = 0;
        for rank in 0..depth {
            let source = grid.candidates[grid.offset(x, y, rank)?];
            if source.is_none() {
                continue;
            }
            let mut found = false;
            for previous in 0..depth {
                if destination[grid.offset(x, y, previous)?] == source {
                    found = true;
                    break;
                }
            }
            if !found {
                destination[grid.offset(x, y, cursor)?] = source;
                cursor += 1;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "procedural_sites_static_test.rs"]
mod static_test;
