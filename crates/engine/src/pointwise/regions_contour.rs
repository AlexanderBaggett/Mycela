//! Periodic contour samples for sj.g (0x060027b2 through 0x060027b4).
use super::{ImageError, Region, absolute_index, reserved};

#[derive(Clone, Copy)]
pub(super) struct BoundaryPoint {
    // Anchor in the unwrapped component bounds, before smoothing.
    pub(super) anchor: [i32; 2],
    pub(super) position: [f64; 2],
    sharpness: f64,
    can_bridge: bool,
}

pub(super) struct Contour {
    pub(super) points: Vec<BoundaryPoint>,
    pub(super) extrema: Vec<usize>,
}

pub(super) fn boundary(
    labels: &[usize],
    [width, height]: [i32; 2],
) -> Result<Vec<usize>, ImageError> {
    let mut edges = reserved(labels.len())?;
    edges.resize(labels.len(), 0);
    for (index, &label) in labels.iter().enumerate() {
        let x = index as i32 % width;
        let y = index as i32 / width;
        for (nx, ny) in [(x + 1, y), (x, y + 1)] {
            let neighbor = absolute_index(nx, ny, width, height);
            if label != labels[neighbor] {
                edges[index] = label;
                edges[neighbor] = labels[neighbor];
            }
        }
    }
    Ok(edges)
}

pub(super) fn trace(
    label: usize,
    region: &Region,
    labels: &[usize],
    edges: &[usize],
    size: [i32; 2],
) -> Result<Contour, ImageError> {
    let origin = [region.left, region.top];
    let span = [
        region.right - region.left + 1,
        region.bottom - region.top + 1,
    ];
    let wrap = [span[0] >= size[0], span[1] >= size[1]];
    let length = (span[0] * span[1]) as usize;
    let mut grid = reserved(length)?;
    grid.resize(length, None);
    let mut points = Vec::new();
    let radius = ((region.area as f64).sqrt() / 40.) as i32;
    let radius = radius.max(3);
    let index = |x, y| absolute_index(x, y, size[0], size[1]);
    for y in 0..span[1] {
        for x in 0..span[0] {
            let [ax, ay] = [x + origin[0], y + origin[1]];
            if edges[index(ax, ay)] != label {
                continue;
            }
            let mut sum = [0i64; 2];
            let mut count = 0;
            for sy in ay - radius..=ay + radius {
                for sx in ax - radius..=ax + radius {
                    if edges[index(sx, sy)] == label {
                        sum[0] += i64::from(sx);
                        sum[1] += i64::from(sy);
                        count += 1;
                    }
                }
            }
            let ring = ring_features([ax, ay], label, labels, edges, size);
            let sharpness = if ring[0] == 0 {
                1.
            } else {
                (((ring[0] - 10) as f64 * (1. / 11.)) * 2.5 - 0.55).clamp(0., 1.)
            };
            grid[(x + y * span[0]) as usize] = Some(points.len());
            points.try_reserve(1).map_err(|_| ImageError::Allocation)?;
            points.push(BoundaryPoint {
                anchor: [ax, ay],
                position: [sum[0] as f64 / count as f64, sum[1] as f64 / count as f64],
                sharpness,
                can_bridge: ring[1] < 12,
            });
        }
    }
    let neighborhood = |point: BoundaryPoint, radius: i32| {
        let [x, y] = [point.anchor[0] - origin[0], point.anchor[1] - origin[1]];
        let limits = move |axis: usize, value: i32| {
            if wrap[axis] {
                value - radius..=value + radius
            } else {
                (value - radius).max(0)..=(value + radius).min(span[axis] - 1)
            }
        };
        limits(1, y).flat_map(move |ny| {
            let gy = ny.rem_euclid(span[1]);
            limits(0, x).map(move |nx| {
                let gx = nx.rem_euclid(span[0]);
                (grid_index(gx, gy, span[0]), [nx - gx, ny - gy])
            })
        })
    };
    // Both passes read the updated values of earlier points in scan order.
    for current in 0..points.len() {
        let mut sum = 0.;
        let mut count = 0;
        for (cell, _) in neighborhood(points[current], 3) {
            if let Some(neighbor) = grid[cell] {
                sum += points[neighbor].sharpness;
                count += 1;
            }
        }
        points[current].sharpness = sum / count as f64;
    }
    let mut extrema = Vec::new();
    for (current, &point) in points.iter().enumerate() {
        if point.sharpness <= 0.07 || !point.can_bridge {
            continue;
        }
        let mut maximum = true;
        let mut minimum = true;
        for (cell, shift) in neighborhood(point, 1) {
            let Some(neighbor) = grid[cell] else { continue };
            if neighbor == current && shift == [0, 0] {
                continue;
            }
            let neighbor = points[neighbor];
            if neighbor.can_bridge {
                maximum &= neighbor.sharpness <= point.sharpness;
                minimum &= !(neighbor.sharpness > 0.07 && neighbor.sharpness < point.sharpness);
            }
        }
        if maximum || minimum {
            extrema.try_reserve(1).map_err(|_| ImageError::Allocation)?;
            extrema.push(current);
        }
    }
    for current in 0..points.len() {
        let point = points[current];
        let mut sum = [0.; 2];
        let mut count = 0;
        for (cell, shift) in neighborhood(point, radius) {
            if let Some(neighbor) = grid[cell] {
                sum[0] += points[neighbor].position[0] + f64::from(shift[0]);
                sum[1] += points[neighbor].position[1] + f64::from(shift[1]);
                count += 1;
            }
        }
        points[current].position = std::array::from_fn(|axis| {
            let anchor = f64::from(point.anchor[axis]);
            anchor + (1. - point.sharpness) * (sum[axis] / count as f64 - anchor)
        });
    }
    Ok(Contour { points, extrema })
}

fn grid_index(x: i32, y: i32, width: i32) -> usize {
    (x + y * width) as usize
}

fn ring_features(
    [x, y]: [i32; 2],
    label: usize,
    labels: &[usize],
    edges: &[usize],
    [width, height]: [i32; 2],
) -> [i32; 2] {
    let sites: [usize; 24] = std::array::from_fn(|i| {
        let i = i as i32;
        let offset = i % 6;
        let (nx, ny) = match i / 6 {
            0 => (x - 3 + offset, y - 3),
            1 => (x + 3, y - 3 + offset),
            2 => (x + 3 - offset, y + 3),
            _ => (x - 3, y + 3 - offset),
        };
        absolute_index(nx, ny, width, height)
    });
    let Some(first) = sites.iter().position(|&site| edges[site] == label) else {
        return [0, 0];
    };
    let mut open = 0;
    let mut longest = 0;
    let mut inside = 0;
    for &site in sites.iter().cycle().skip(first + 1).take(23) {
        if edges[site] == label {
            longest = longest.max(open);
            open = 0;
            inside += 1;
        } else {
            open += 1;
            inside += i32::from(labels[site] == label);
        }
    }
    [longest.max(open), inside]
}
