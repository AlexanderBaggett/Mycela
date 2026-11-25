//! Existing RegionsMap reconstruction from sj.g(ai5, ArrayList, int[], ref int, float[]).
use super::{Region, absolute_index, reserved};
use crate::ImageError;

pub(super) fn detect(
    values: &[f32],
    width: i32,
    height: i32,
) -> Result<(Vec<usize>, Vec<Region>), ImageError> {
    let length = (width * height) as usize;
    let values = values.get(..length).ok_or(ImageError::Index)?;
    let mut map = RegionMap {
        values,
        labels: reserved(length)?,
        width,
        height,
    };
    map.labels.resize(length, 0);
    let mut regions = Vec::new();
    let mut whole_labels = Vec::new();
    for (index, &value) in values.iter().enumerate() {
        if map.labels[index] != 0 || crate::framework_cast::integer(value.into()) as f32 == value {
            continue;
        }
        let id = regions.len() + 1;
        map.labels[index] = id;
        let x = index as i32 % width;
        let y = index as i32 / width;
        let mut region = Region {
            left: x - 1,
            right: x + 1,
            top: y - 1,
            bottom: y + 1,
            area: 1,
            random: [0.; 2],
        };
        while map.grow(&mut region, id, value, true) {}
        map.normalize(&mut region);
        regions.try_reserve(1).map_err(|_| ImageError::Allocation)?;
        whole_labels
            .try_reserve(1)
            .map_err(|_| ImageError::Allocation)?;
        regions.push(region);
        whole_labels.push(Some(value.floor()));
    }
    if !regions.is_empty() {
        // One sweep per component, in seed order. A completed component
        // retires, while other components continue in their original order.
        while whole_labels.iter().any(Option::is_some) {
            for (index, (region, whole)) in regions.iter_mut().zip(&mut whole_labels).enumerate() {
                let Some(value) = *whole else { continue };
                if map.grow(region, index + 1, value, false) {
                    map.normalize(region);
                } else {
                    *whole = None;
                }
            }
        }
        map.fill_unassigned();
    }
    // A map without fractional seeds has no regions. Skip the original's
    // final sweep in that case because no pixel could ever be assigned.
    Ok((map.labels, regions))
}

struct RegionMap<'a> {
    values: &'a [f32],
    labels: Vec<usize>,
    width: i32,
    height: i32,
}

impl RegionMap<'_> {
    fn index(&self, x: i32, y: i32) -> usize {
        absolute_index(x, y, self.width, self.height)
    }

    fn normalize(&self, region: &mut Region) {
        let [left, right, top, bottom] = self.scan_limits(region);
        region.left = left;
        region.right = right;
        region.top = top;
        region.bottom = bottom;
    }

    fn scan_limits(&self, region: &Region) -> [i32; 4] {
        let offset_x = if region.left < 0 { self.width } else { 0 };
        let offset_y = if region.top < 0 { self.height } else { 0 };
        let left = region.left + offset_x;
        let top = region.top + offset_y;
        [
            left,
            (region.right + offset_x).min(left + self.width - 1),
            top,
            (region.bottom + offset_y).min(top + self.height - 1),
        ]
    }

    fn grow(&mut self, region: &mut Region, id: usize, wanted: f32, seed_pass: bool) -> bool {
        let [left, right, top, bottom] = self.scan_limits(region);
        let mut grew = false;
        let mut extended = [false; 4];
        for y in top..=bottom {
            for x in left..=right {
                let index = self.index(x, y);
                if self.labels[index] != 0 || self.values[index] != wanted {
                    continue;
                }
                let matches_neighbor = |nx, ny| {
                    let neighbor = self.index(nx, ny);
                    self.labels[neighbor] == id
                        && if seed_pass {
                            self.values[neighbor] == wanted
                        } else {
                            self.values[neighbor] >= wanted && self.values[neighbor] < wanted + 1.
                        }
                };
                let connected = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                    .into_iter()
                    .any(|(nx, ny)| matches_neighbor(nx, ny))
                    || (seed_pass
                        && [
                            (x - 1, y - 1),
                            (x + 1, y - 1),
                            (x - 1, y + 1),
                            (x + 1, y + 1),
                        ]
                        .into_iter()
                        .any(|(nx, ny)| matches_neighbor(nx, ny)));
                if !connected {
                    continue;
                }
                self.labels[index] = id;
                region.area += 1;
                grew = true;
                if region.right - region.left < self.width - 1 {
                    if x == left && !extended[0] {
                        region.left -= 1;
                        extended[0] = true;
                    } else if x == right && !extended[1] {
                        region.right += 1;
                        extended[1] = true;
                    }
                }
                if region.bottom - region.top < self.height - 1 {
                    if y == top && !extended[2] {
                        region.top -= 1;
                        extended[2] = true;
                    } else if y == bottom && !extended[3] {
                        region.bottom += 1;
                        extended[3] = true;
                    }
                }
            }
        }
        grew
    }

    fn fill_unassigned(&mut self) {
        let mut pending = true;
        while pending {
            pending = false;
            for y in 0..self.height {
                for x in 0..self.width {
                    let index = self.index(x, y);
                    if self.labels[index] != 0 {
                        continue;
                    }
                    let neighbor = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                        .into_iter()
                        .map(|(nx, ny)| self.labels[self.index(nx, ny)])
                        .find(|&id| id != 0);
                    if let Some(id) = neighbor {
                        self.labels[index] = id;
                    } else {
                        pending = true;
                    }
                }
            }
        }
    }
}
