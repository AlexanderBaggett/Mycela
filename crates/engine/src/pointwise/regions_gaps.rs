//! Periodic gap assignment from sj.g (0x060027c0–0x060027ca).
use super::{
    ImageError, Region, absolute_index,
    contour::{self, BoundaryPoint, Contour},
    reserved,
};
use std::f64::consts::{FRAC_PI_2, PI, SQRT_2, TAU};

pub(super) type Coord = [f64; 2];

#[derive(Clone, Copy)]
struct Site {
    point: BoundaryPoint,
    label: usize,
    copy: [i32; 2],
}

impl Site {
    fn near_tile(self, center: Coord, size: [i32; 2], limit: f64) -> Option<Self> {
        let (shift, distance) = wrapped_distance(center, self.point.position, size);
        (distance < limit).then(|| self.shifted(shift))
    }

    fn shifted(mut self, shift: Coord) -> Self {
        for (axis, shift) in shift.into_iter().enumerate() {
            self.copy[axis] += shift as i32;
            self.point.position[axis] += shift;
        }
        self
    }
}

#[derive(Clone, Copy)]
struct Segment {
    ends: [Coord; 2],
    labels: [usize; 2],
    delta: Coord,
    slope: Coord,
    intercept: f64,
}

impl Segment {
    fn new([start, end]: [Coord; 2], labels: [usize; 2]) -> Self {
        let delta = [end[0] - start[0], end[1] - start[1]];
        let inv = 1. / (delta[0] * delta[0] + delta[1] * delta[1]);
        let slope = [delta[0] * inv, delta[1] * inv];
        let intercept = (0. - start[0]) * slope[0] - start[1] * slope[1];
        Self {
            ends: [start, end],
            labels,
            delta,
            slope,
            intercept,
        }
    }

    fn distance(self, [x, y]: Coord) -> f64 {
        let t = (x * self.slope[0] + y * self.slope[1] + self.intercept).clamp(0., 1.);
        let closest = if t >= 1. {
            self.ends[1]
        } else if t <= 0. {
            self.ends[0]
        } else {
            [
                self.ends[0][0] + t * self.delta[0],
                self.ends[0][1] + t * self.delta[1],
            ]
        };
        squared(closest, [x, y])
    }

    fn near_tile(self, center: Coord, size: [i32; 2], limit: f64) -> Option<Self> {
        let shift = std::array::from_fn::<_, 2, _>(|axis| {
            let middle = (self.ends[0][axis] + self.ends[1][axis]) * 0.5;
            f64::from(near_copy(center[axis] - middle, size[axis]))
        });
        let local = [center[0] - shift[0], center[1] - shift[1]];
        (self.distance(local) < limit).then(|| self.shifted(shift))
    }

    fn shifted(self, shift: Coord) -> Self {
        if shift == [0., 0.] {
            self
        } else {
            Self::new(
                self.ends.map(|p| [p[0] + shift[0], p[1] + shift[1]]),
                self.labels,
            )
        }
    }
}

#[derive(Clone, Copy)]
enum Candidate {
    Point(Site),
    Segment(Segment),
}

impl Candidate {
    fn add_copies(
        self,
        candidates: &mut Vec<Self>,
        area: Tile,
        size: [i32; 2],
        center: Coord,
        limit: f64,
    ) -> Result<(), ImageError> {
        if size[0] == size[1] {
            let copy = match self {
                Self::Point(site) => site.near_tile(center, size, limit).map(Self::Point),
                Self::Segment(segment) => segment.near_tile(center, size, limit).map(Self::Segment),
            };
            if let Some(copy) = copy {
                push(candidates, copy)?;
            }
        } else {
            let points = match self {
                Self::Point(site) => [site.point.position; 2],
                Self::Segment(segment) => segment.ends,
            };
            for shift in area.periodic_shifts(points, size) {
                let copy = match self {
                    Self::Point(site) => Self::Point(site.shifted(shift)),
                    Self::Segment(segment) => Self::Segment(segment.shifted(shift)),
                };
                if copy.distance(center) <= limit {
                    push(candidates, copy)?;
                }
            }
        }
        Ok(())
    }

    fn distance(self, center: Coord) -> f64 {
        match self {
            Self::Point(site) => squared(center, site.point.position),
            Self::Segment(segment) => segment.distance(center),
        }
    }

    fn shares_label(self, other: Self) -> bool {
        let labels = |feature| match feature {
            Self::Point(site) => [site.label; 2],
            Self::Segment(segment) => segment.labels,
        };
        labels(self)
            .iter()
            .any(|label| labels(other).contains(label))
    }

    fn closest_position(self, pixel: Coord, shift: Coord) -> Coord {
        match self {
            Self::Point(site) => [
                site.point.position[0] + shift[0],
                site.point.position[1] + shift[1],
            ],
            Self::Segment(segment) => {
                let [a, b] = segment.ends.map(|p| [p[0] + shift[0], p[1] + shift[1]]);
                let [dx, dy] = [b[0] - a[0], b[1] - a[1]];
                let fraction =
                    ((pixel[0] - a[0]) * dx + (pixel[1] - a[1]) * dy) / (dx * dx + dy * dy);
                if fraction <= 0. {
                    a
                } else if fraction >= 1. {
                    b
                } else {
                    [a[0] + fraction * dx, a[1] + fraction * dy]
                }
            }
        }
    }
}

struct Bridge {
    segment: Segment,
    length: f64,
    direction: [f64; 2],
    keep: bool,
}

impl Bridge {
    fn new(segment: Segment, length: f64) -> Self {
        let mut angle = (segment.delta[0] / length).clamp(-1., 1.).acos();
        if segment.delta[1] < 0. {
            angle = TAU - angle;
        }
        Self {
            segment,
            length,
            direction: [angle, (angle + PI) % TAU],
            keep: true,
        }
    }

    fn angle_from(&self, label: usize) -> f64 {
        self.direction[usize::from(self.segment.labels[0] != label)]
    }

    fn obscured_by(&self, shorter: &Self, label: usize) -> bool {
        let mut angle = (self.angle_from(label) - shorter.angle_from(label)).abs();
        if angle > PI {
            angle = TAU - angle;
        }
        if angle > FRAC_PI_2 {
            return false;
        }
        let fourth = angle * angle;
        let fourth = fourth * fourth;
        let eighth = fourth * fourth;
        let factor = 1. + 0.35 * fourth + 0.0025 * (eighth * (eighth * angle));
        self.length > shorter.length * factor
    }
}

pub(super) fn squared(a: Coord, b: Coord) -> f64 {
    let [x, y] = [a[0] - b[0], a[1] - b[1]];
    x * x + y * y
}

pub(super) fn near_copy(delta: f64, period: i32) -> i32 {
    let half = f64::from(period) * 0.5;
    if delta < -half {
        -period
    } else if delta > half {
        period
    } else {
        0
    }
}

pub(super) fn push<T>(values: &mut Vec<T>, value: T) -> Result<(), ImageError> {
    values.try_reserve(1).map_err(|_| ImageError::Allocation)?;
    values.push(value);
    Ok(())
}

fn bridges(
    contours: &[Contour],
    labels: &[usize],
    edges: &[usize],
    size: [i32; 2],
) -> Result<Vec<Bridge>, ImageError> {
    let mut links = reserved(contours.len())?;
    for contour in contours {
        let mut group = reserved(contour.extrema.len())?;
        group.resize_with(contour.extrema.len(), Vec::new);
        links.push(group);
    }
    let mut bridges = Vec::new();
    let reach = f64::from(size[0]) * 0.1;
    for (region, contour) in contours.iter().enumerate() {
        for (index, &point) in contour.extrema.iter().enumerate() {
            let start = contour.points[point].position;
            let nearest = contours
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != region)
                .flat_map(|(_, other)| other.points.iter())
                .map(|point| wrapped_distance(start, point.position, size).1)
                .fold(f64::MAX, f64::min);
            let limit = (nearest * 7.).min(reach * reach);
            for (other, contour) in contours.iter().enumerate().skip(region + 1) {
                for (other_index, &point) in contour.extrema.iter().enumerate() {
                    let position = contour.points[point].position;
                    let (shift, distance) = wrapped_distance(start, position, size);
                    if distance <= 1. || distance >= limit {
                        continue;
                    }
                    let end = [position[0] + shift[0], position[1] + shift[1]];
                    let segment = Segment::new([start, end], [region + 1, other + 1]);
                    let length = distance.sqrt();
                    let bridge = bridges.len();
                    push(
                        &mut bridges,
                        Bridge {
                            keep: crosses_gap(segment, length, labels, edges, size),
                            ..Bridge::new(segment, length)
                        },
                    )?;
                    push(&mut links[region][index], bridge)?;
                    push(&mut links[other][other_index], bridge)?;
                }
            }
        }
    }
    for (region, links) in links.iter().enumerate() {
        for links in links.iter().filter(|links| !links.is_empty()) {
            prune_bridges(&mut bridges, links, region + 1)?;
        }
    }
    Ok(bridges)
}

fn wrapped_distance(start: Coord, end: Coord, size: [i32; 2]) -> (Coord, f64) {
    let diff = [start[0] - end[0], start[1] - end[1]];
    let shift = std::array::from_fn(|axis| {
        if size[0] == size[1] {
            f64::from(near_copy(diff[axis], size[axis]))
        } else {
            let period = f64::from(size[axis]);
            (diff[axis] / period).round_ties_even() * period
        }
    });
    let [x, y] = [diff[0] - shift[0], diff[1] - shift[1]];
    (shift, x * x + y * y)
}

fn crosses_gap(
    segment: Segment,
    length: f64,
    labels: &[usize],
    edges: &[usize],
    [width, height]: [i32; 2],
) -> bool {
    let mut interior = 0;
    let inv = 1. / length;
    for i in 1..length.ceil() as i32 {
        let fraction = f64::from(i) * inv;
        let x = (segment.ends[0][0] + fraction * segment.delta[0] + 0.5) as i32;
        let y = (segment.ends[0][1] + fraction * segment.delta[1] + 0.5) as i32;
        let index = absolute_index(x, y, width, height);
        let label = labels[index];
        if label != 0 {
            if interior >= 5 || !segment.labels.contains(&label) {
                return false;
            }
            interior += usize::from(edges[index] == 0);
        }
    }
    true
}

fn prune_bridges(bridges: &mut [Bridge], links: &[usize], label: usize) -> Result<(), ImageError> {
    // The original walks same-destination edges depth first, in link order.
    let mut states = reserved(links.len())?;
    states.extend(links.iter().map(|&edge| !bridges[edge].keep));
    let mut pending = Vec::new();
    while let Some((first, _)) = links
        .iter()
        .enumerate()
        .filter(|(index, _)| !states[*index])
        .min_by(|&(_, &a), &(_, &b)| bridges[a].length.total_cmp(&bridges[b].length))
    {
        states[first] = true;
        push(&mut pending, (links[first], 0))?;
        while let Some((base, cursor)) = pending.last_mut() {
            let Some(&edge) = links.get(*cursor) else {
                pending.pop();
                continue;
            };
            let position = *cursor;
            *cursor += 1;
            if states[position] {
                continue;
            }
            let base = &bridges[*base];
            let current = &bridges[edge];
            if current.segment.labels == base.segment.labels {
                states[position] = true;
                push(&mut pending, (edge, 0))?;
            } else if current.obscured_by(base, label) {
                states[position] = true;
                bridges[edge].keep = false;
            }
        }
    }
    Ok(())
}

pub(super) fn extend(
    labels: &mut [usize],
    regions: &mut [Region],
    size: [i32; 2],
    radial: bool,
) -> Result<Vec<Coord>, ImageError> {
    let [width, height] = size;
    let edges = contour::boundary(labels, size)?;
    let mut contours = reserved(regions.len())?;
    for (region, bounds) in regions.iter().enumerate() {
        contours.push(contour::trace(region + 1, bounds, labels, &edges, size)?);
    }
    let bridges = bridges(&contours, labels, &edges, size)?;
    let mut sites = reserved(contours.iter().map(|c| c.points.len()).sum())?;
    for (index, contour) in contours.into_iter().enumerate() {
        sites.extend(contour.points.into_iter().map(|point| Site {
            point,
            label: index + 1,
            copy: point.anchor,
        }));
    }
    let tile = tile_size(width);
    let mut radial = radial.then(|| RadialSites::new(size, tile)).transpose()?;
    let mut candidates = Vec::new();
    for y in (0..height).step_by(tile as usize) {
        for x in (0..width).step_by(tile as usize) {
            let area = Tile {
                first: [x, y],
                last: [
                    (x + tile - 1).min(width - 1),
                    (y + tile - 1).min(height - 1),
                ],
            };
            let has_gap = if let Some(radial) = &radial {
                (y - 1..=area.last[1] + 1).any(|sy| {
                    (x - 1..=area.last[0] + 1).any(|sx| {
                        let index = absolute_index(sx, sy, width, height);
                        labels[index] == 0 || radial.matches[index].is_some()
                    })
                })
            } else {
                (y..=area.last[1]).any(|row| {
                    labels[(row * width + x) as usize..=(row * width + area.last[0]) as usize]
                        .contains(&0)
                })
            };
            if !has_gap {
                continue;
            }
            let center = area.tile_center(size, tile);
            let nearest = sites
                .iter()
                .map(|site| squared(center, site.point.position))
                .fold(f64::MAX, f64::min);
            let limit = nearest.sqrt() + SQRT_2 * f64::from(tile);
            let limit = limit * limit;
            candidates.clear();
            for &site in &sites {
                Candidate::Point(site).add_copies(&mut candidates, area, size, center, limit)?;
            }
            for bridge in bridges.iter().filter(|bridge| bridge.keep) {
                Candidate::Segment(bridge.segment).add_copies(
                    &mut candidates,
                    area,
                    size,
                    center,
                    limit,
                )?;
            }
            let mut selected = reserved(candidates.len())?;
            selected.extend(0..candidates.len());
            area.fill(
                labels,
                regions,
                width,
                &candidates,
                &selected,
                radial.as_mut().map(|radial| radial.matches.as_mut_slice()),
            )?;
            if let Some(radial) = &mut radial {
                let slot = radial.tile_index([x, y]);
                radial.tiles[slot] = std::mem::take(&mut candidates);
            }
        }
    }
    for region in regions {
        normalize_span(&mut region.left, &mut region.right, width);
        normalize_span(&mut region.top, &mut region.bottom, height);
    }
    radial.map_or_else(|| Ok(Vec::new()), |radial| radial.boundaries(labels))
}

pub(super) fn tile_size(width: i32) -> i32 {
    (width / 10).max(20)
}

struct RadialSites {
    matches: Vec<Option<WinningFeatures>>,
    tiles: Vec<Vec<Candidate>>,
    size: [i32; 2],
    tile: i32,
    columns: i32,
}

impl RadialSites {
    fn new([width, height]: [i32; 2], tile: i32) -> Result<Self, ImageError> {
        let [columns, rows] = [width, height].map(|size| 1 + (size - 1) / tile);
        let length = (width * height) as usize;
        let tile_count = (columns * rows) as usize;
        let mut matches = reserved(length)?;
        matches.resize(length, None);
        let mut tiles = reserved(tile_count)?;
        tiles.resize_with(tile_count, Vec::new);
        Ok(Self {
            matches,
            tiles,
            size: [width, height],
            tile,
            columns,
        })
    }

    fn tile_index(&self, [x, y]: [i32; 2]) -> usize {
        (y / self.tile * self.columns + x / self.tile) as usize
    }

    fn features(&self, pixel: [i32; 2], index: usize) -> Result<[Candidate; 2], ImageError> {
        let sites = &self.tiles[self.tile_index(pixel)];
        if let Some(winner) = self.matches[index] {
            return Ok([sites[winner.feature], sites[winner.point]]);
        }
        let center = pixel.map(f64::from);
        let mut nearest = f64::MAX;
        let mut feature = None;
        for &candidate in sites {
            if matches!(candidate, Candidate::Point(_)) {
                let distance = candidate.distance(center);
                if distance < nearest {
                    nearest = distance;
                    feature = Some(candidate);
                }
            }
        }
        Ok([feature.ok_or(ImageError::Index)?; 2])
    }

    fn boundaries(self, labels: &[usize]) -> Result<Vec<Coord>, ImageError> {
        let [width, height] = self.size;
        let mut boundaries = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let pixel = [x, y];
                let index = (y * width + x) as usize;
                for axis in 0..2 {
                    let mut neighbor = pixel;
                    neighbor[axis] = (pixel[axis] + 1) % self.size[axis];
                    let next = (neighbor[1] * width + neighbor[0]) as usize;
                    if labels[index] == labels[next] {
                        continue;
                    }
                    let mut step = [0.; 2];
                    step[axis] = 1.;
                    let mut shift = [0.; 2];
                    if neighbor[axis] == 0 {
                        shift[axis] = f64::from(self.size[axis]);
                    }
                    let a = self.features(pixel, index)?;
                    let b = self.features(neighbor, next)?;
                    if let Some(point) = transition(a, b, pixel.map(f64::from), step, shift) {
                        push(&mut boundaries, point)?;
                    }
                }
            }
        }
        Ok(boundaries)
    }
}

fn transition(
    [mut first, first_endpoint]: [Candidate; 2],
    [mut second, second_endpoint]: [Candidate; 2],
    start: Coord,
    step: Coord,
    shift: Coord,
) -> Option<Coord> {
    // A segment shared with the other feature uses the single-label point
    // selected when the gap pixel was assigned.
    if first.shares_label(second) {
        first = first_endpoint;
        second = second_endpoint;
    }
    let a = first.closest_position(start, [0.; 2]);
    let end = [start[0] + step[0], start[1] + step[1]];
    let b = second.closest_position(end, shift);
    let [ax, ay] = [start[0] - a[0], start[1] - a[1]];
    let [bx, by] = [start[0] - b[0], start[1] - b[1]];
    let fraction = (bx * bx + by * by - ax * ax - ay * ay)
        / (2. * (step[0] * (ax - bx) + step[1] * (ay - by)));
    (fraction > -0.5 && fraction < 1.5)
        .then(|| [start[0] + step[0] * fraction, start[1] + step[1] * fraction])
}

#[derive(Clone, Copy)]
pub(super) struct Tile {
    pub(super) first: [i32; 2],
    pub(super) last: [i32; 2],
}

impl Tile {
    pub(super) fn periodic_shifts(
        self,
        [a, b]: [Coord; 2],
        size: [i32; 2],
    ) -> impl Iterator<Item = Coord> {
        // Keep every copy that can be nearest to a pixel anywhere in this tile.
        let period = size.map(f64::from);
        let [columns, rows] = std::array::from_fn(|axis| {
            let low = f64::from(self.first[axis]) - a[axis].max(b[axis]);
            let high = f64::from(self.last[axis]) - a[axis].min(b[axis]);
            let first = (low / period[axis] - 0.5).ceil() as i32;
            let last = (high / period[axis] + 0.5).floor() as i32;
            first..=last
        });
        rows.flat_map(move |y| {
            columns
                .clone()
                .map(move |x| [f64::from(x) * period[0], f64::from(y) * period[1]])
        })
    }

    pub(super) fn tile_center(self, [width, height]: [i32; 2], tile: i32) -> Coord {
        if width == height {
            let half_tile = f64::from(tile - 1) * 0.5;
            self.first.map(|edge| f64::from(edge) + half_tile)
        } else {
            self.center()
        }
    }

    pub(super) fn center(self) -> Coord {
        std::array::from_fn(|axis| {
            f64::from(self.first[axis]) + f64::from(self.last[axis] - self.first[axis]) * 0.5
        })
    }

    pub(super) fn padding(self) -> f64 {
        let half = std::array::from_fn(|axis| f64::from(self.last[axis] - self.first[axis]) * 0.5);
        squared(half, [0.; 2]).sqrt() * 2.
    }

    pub(super) fn is_leaf(self) -> bool {
        self.last[0] - self.first[0] < 10 || self.last[1] - self.first[1] < 10
    }

    pub(super) fn quadrants(self) -> [Self; 4] {
        let [left, top] = self.first;
        let [right, bottom] = self.last;
        let x = left + (right - left) / 2;
        let y = top + (bottom - top) / 2;
        [
            Self {
                first: [left, top],
                last: [x - 1, y - 1],
            },
            Self {
                first: [x, top],
                last: [right, y - 1],
            },
            Self {
                first: [left, y],
                last: [x - 1, bottom],
            },
            Self {
                first: [x, y],
                last: [right, bottom],
            },
        ]
    }

    fn fill(
        self,
        labels: &mut [usize],
        regions: &mut [Region],
        width: i32,
        candidates: &[Candidate],
        selected: &[usize],
        mut matches: Option<&mut [Option<WinningFeatures>]>,
    ) -> Result<(), ImageError> {
        if self.is_leaf() {
            for y in self.first[1]..=self.last[1] {
                for x in self.first[0]..=self.last[0] {
                    let index = (y * width + x) as usize;
                    if labels[index] == 0 {
                        let winner =
                            choose_site([f64::from(x), f64::from(y)], candidates, selected)
                                .ok_or(ImageError::Index)?;
                        let Candidate::Point(site) = candidates[winner.point] else {
                            return Err(ImageError::Index);
                        };
                        if let Some(matches) = &mut matches {
                            matches[index] = Some(winner);
                        }
                        labels[index] = site.label;
                        let region = &mut regions[site.label - 1];
                        extend_span(&mut region.left, &mut region.right, site, x, 0);
                        extend_span(&mut region.top, &mut region.bottom, site, y, 1);
                    }
                }
            }
            return Ok(());
        }
        for child in self.quadrants() {
            child.fill(
                labels,
                regions,
                width,
                candidates,
                &child.candidates(candidates, selected)?,
                matches.as_deref_mut(),
            )?;
        }
        Ok(())
    }

    fn candidates(
        self,
        candidates: &[Candidate],
        parent: &[usize],
    ) -> Result<Vec<usize>, ImageError> {
        let center = self.center();
        let nearest = parent
            .iter()
            .map(|&index| candidates[index].distance(center))
            .fold(f64::MAX, f64::min);
        let padding = self.padding();
        let radius = nearest.sqrt() + padding;
        let limit = radius * radius;
        let mut endpoint = -1.;
        let mut selected = Vec::new();
        for &index in parent {
            let candidate = candidates[index];
            if candidate.distance(center) <= limit {
                push(&mut selected, index)?;
                if let Candidate::Segment(segment) = candidate {
                    endpoint = squared(center, segment.ends[0])
                        .max(squared(center, segment.ends[1]))
                        .max(endpoint);
                }
            }
        }
        // An eligible bridge can need an endpoint label farther away than
        // the general nearest-site bound. Keep those point candidates too.
        if endpoint > nearest {
            let radius = endpoint.sqrt() + padding;
            let rest = radius * radius;
            for &index in parent {
                let candidate = candidates[index];
                if let Candidate::Point(_) = candidate {
                    let distance = candidate.distance(center);
                    if distance > limit && distance <= rest {
                        push(&mut selected, index)?;
                    }
                }
            }
        }
        Ok(selected)
    }
}

#[derive(Clone, Copy)]
struct WinningFeatures {
    feature: usize,
    point: usize,
}

fn choose_site(
    center: Coord,
    candidates: &[Candidate],
    selected: &[usize],
) -> Option<WinningFeatures> {
    let mut nearest = f64::MAX;
    let mut winner = None;
    for &index in selected {
        let distance = candidates[index].distance(center);
        if distance < nearest {
            nearest = distance;
            winner = Some(index);
        }
    }
    let feature = winner?;
    let point = match candidates[feature] {
        Candidate::Point(_) => feature,
        Candidate::Segment(segment) => {
            let mut closest = [(f64::MAX, None); 2];
            for &index in selected {
                let candidate = candidates[index];
                let Candidate::Point(site) = candidate else {
                    continue;
                };
                let Some(side) = segment.labels.iter().position(|&label| label == site.label)
                else {
                    continue;
                };
                let distance = candidate.distance(center);
                if distance < closest[side].0 {
                    closest[side] = (distance, Some(index));
                }
            }
            closest[usize::from(closest[0].0 >= closest[1].0)].1?
        }
    };
    Some(WinningFeatures { feature, point })
}

fn extend_span(first: &mut i32, last: &mut i32, site: Site, pixel: i32, axis: usize) {
    let relative = pixel - site.copy[axis];
    let unwrapped = site.point.anchor[axis] + relative;
    if relative > 0 {
        *last = (*last).max(unwrapped);
    } else {
        *first = (*first).min(unwrapped);
    }
}

fn normalize_span(first: &mut i32, last: &mut i32, period: i32) {
    let excess = *last - *first - (period - 1);
    if excess > 0 {
        *first += excess / 2;
        *last -= (excess + 1) / 2;
    }
    if *first < 0 {
        *first += period;
        *last += period;
    }
}
