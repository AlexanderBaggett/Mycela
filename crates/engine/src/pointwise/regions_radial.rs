//! Regions contour distances and per-label normalization from sj.g (0x060027cd).
use super::{
    FloatBuffer, ImageError, Region, absolute_index, contour,
    gaps::{Coord, Tile, near_copy, push, squared, tile_size},
    reserved,
};
use std::f64::consts::SQRT_2;

pub(super) fn render(
    labels: &[usize],
    regions: &[Region],
    size: [i32; 2],
    rebuilt: Option<&[Coord]>,
) -> Result<FloatBuffer, ImageError> {
    let mut output = reserved(labels.len())?;
    output.resize(labels.len(), 0.);
    let mut maxima = reserved(regions.len() + 1)?;
    maxima.resize(regions.len() + 1, 0.);
    let mut pass = Distances {
        labels,
        size,
        output,
        maxima,
    };
    if let Some(points) = rebuilt {
        if !points.is_empty() {
            pass.scan(
                Tile {
                    first: [0, 0],
                    last: size.map(|period| period - 1),
                },
                points,
                PeriodicCopies::Nearest,
                None,
            )?;
        }
    } else {
        let edges = contour::boundary(labels, size)?;
        let mut points = Vec::new();
        for (index, region) in regions.iter().enumerate() {
            let label = index + 1;
            let contour = contour::trace(label, region, labels, &edges, size)?;
            if contour.points.len() < 2 {
                continue;
            }
            points.clear();
            points
                .try_reserve(contour.points.len())
                .map_err(|_| ImageError::Allocation)?;
            points.extend(contour.points.into_iter().map(|point| point.position));
            let area = Tile {
                first: [region.left, region.top],
                last: [region.right, region.bottom],
            };
            let wrap =
                std::array::from_fn(|axis| area.last[axis] - area.first[axis] + 1 >= size[axis]);
            pass.scan(
                area,
                &points,
                PeriodicCopies::FullSpan(wrap),
                Some((label, &edges)),
            )?;
        }
    }
    for max in &mut pass.maxima {
        if *max > 0. {
            *max = 1. / *max;
        }
    }
    for (pixel, &label) in pass.output.iter_mut().zip(labels) {
        *pixel *= pass.maxima[label];
    }
    Ok(FloatBuffer::new(pass.output))
}

#[derive(Clone, Copy)]
enum PeriodicCopies {
    Nearest,
    FullSpan([bool; 2]),
}

struct Distances<'a> {
    labels: &'a [usize],
    size: [i32; 2],
    output: Vec<f32>,
    maxima: Vec<f32>,
}

impl Distances<'_> {
    fn scan(
        &mut self,
        area: Tile,
        points: &[Coord],
        periodic: PeriodicCopies,
        retained: Option<(usize, &[usize])>,
    ) -> Result<(), ImageError> {
        let tile = tile_size(self.size[0]);
        let mut candidates = Vec::new();
        for y in (area.first[1]..=area.last[1]).step_by(tile as usize) {
            for x in (area.first[0]..=area.last[0]).step_by(tile as usize) {
                let block = Tile {
                    first: [x, y],
                    last: [
                        (x + tile - 1).min(area.last[0]),
                        (y + tile - 1).min(area.last[1]),
                    ],
                };
                let center = block.tile_center(self.size, tile);
                let reach = nearest(center, points).sqrt() + SQRT_2 * f64::from(tile);
                candidates.clear();
                for &point in points {
                    self.add_copies(
                        &mut candidates,
                        point,
                        block,
                        center,
                        reach * reach,
                        periodic,
                    )?;
                }
                self.fill(block, &candidates, retained)?;
            }
        }
        Ok(())
    }

    fn add_copies(
        &self,
        candidates: &mut Vec<Coord>,
        point: Coord,
        block: Tile,
        center: Coord,
        limit: f64,
        periodic: PeriodicCopies,
    ) -> Result<(), ImageError> {
        match periodic {
            PeriodicCopies::Nearest if self.size[0] != self.size[1] => {
                for shift in block.periodic_shifts([point; 2], self.size) {
                    let copy = [point[0] + shift[0], point[1] + shift[1]];
                    if squared(center, copy) <= limit {
                        push(candidates, copy)?;
                    }
                }
            }
            PeriodicCopies::Nearest => {
                let shift: Coord = std::array::from_fn(|axis| {
                    f64::from(near_copy(center[axis] - point[axis], self.size[axis]))
                });
                let [x, y] = [
                    center[0] - point[0] - shift[0],
                    center[1] - point[1] - shift[1],
                ];
                if x * x + y * y < limit {
                    push(candidates, [point[0] + shift[0], point[1] + shift[1]])?;
                }
            }
            PeriodicCopies::FullSpan([wrap_x, wrap_y]) => {
                let [dx, dy] = [
                    if wrap_x { self.size[0] } else { 0 },
                    if wrap_y { self.size[1] } else { 0 },
                ];
                let offsets_x = [-dx, 0, dx];
                let offsets_y = [-dy, 0, dy];
                let offsets_x = if wrap_x {
                    &offsets_x[..]
                } else {
                    &offsets_x[1..2]
                };
                let offsets_y = if wrap_y {
                    &offsets_y[..]
                } else {
                    &offsets_y[1..2]
                };
                for &dy in offsets_y {
                    for &dx in offsets_x {
                        let copy = [point[0] + f64::from(dx), point[1] + f64::from(dy)];
                        if squared(center, copy) < limit {
                            push(candidates, copy)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn fill(
        &mut self,
        area: Tile,
        points: &[Coord],
        retained: Option<(usize, &[usize])>,
    ) -> Result<(), ImageError> {
        if area.is_leaf() {
            for y in area.first[1]..=area.last[1] {
                for x in area.first[0]..=area.last[0] {
                    let index = absolute_index(x, y, self.size[0], self.size[1]);
                    let id = self.labels[index];
                    if let Some((label, edges)) = retained
                        && (id != label || edges[index] == label)
                    {
                        continue;
                    }
                    let distance = nearest([f64::from(x), f64::from(y)], points).sqrt() as f32;
                    self.output[index] = distance;
                    self.maxima[id] = self.maxima[id].max(distance);
                }
            }
            return Ok(());
        }
        let mut selected = Vec::new();
        for child in area.quadrants() {
            let center = child.center();
            let reach = nearest(center, points).sqrt() + child.padding();
            selected.clear();
            for &point in points {
                if squared(center, point) <= reach * reach {
                    push(&mut selected, point)?;
                }
            }
            self.fill(child, &selected, retained)?;
        }
        Ok(())
    }
}

fn nearest(pixel: Coord, points: &[Coord]) -> f64 {
    points
        .iter()
        .map(|&point| squared(pixel, point))
        .fold(f64::MAX, f64::min)
}
