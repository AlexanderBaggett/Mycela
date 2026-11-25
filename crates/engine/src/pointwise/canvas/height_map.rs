//! Combine bevel normals and reconstruct the Style Height Map, following jv.
use super::{area, raster};
use crate::pointwise::{SharedImageMap, environment_optics, image_resize};
use crate::{
    FloatBuffer, ImageError, ImageGeometry, PointwiseError, ProceduralExecution, SharedFloatStore,
};

pub(super) struct StyleHeightMap(Vec<[f32; 2]>);

impl StyleHeightMap {
    pub(super) fn new(length: usize) -> Result<Self, ImageError> {
        Ok(Self(raster::filled(length, [0.5; 2])?))
    }

    pub(super) fn blend<E>(
        &mut self,
        height: &SharedImageMap,
        depth: f64,
        coverage: impl Fn(usize) -> f32,
        opacity: f32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), PointwiseError<E>> {
        environment_optics::visit_height_normals(height, depth, cancelled, |index, x, y| {
            let weight = (1. - (1. - coverage(index))) * opacity;
            let remaining = 1. - weight;
            let [old_x, old_y] = self.0[index];
            self.0[index] = [
                x * weight + old_x * remaining,
                y * weight + old_y * remaining,
            ];
            Ok(())
        })
    }

    pub(super) fn attach<E>(
        self,
        target: &SharedImageMap,
        execution: ProceduralExecution,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), PointwiseError<E>> {
        let geometry = target.geometry();
        let size = geometry.width();
        let normals = SharedImageMap::new(geometry);
        let mut x = raster::filled(self.0.len(), 0.)?;
        let mut y = raster::filled(self.0.len(), 0.)?;
        let mut z = raster::filled(self.0.len(), 0.)?;
        for (index, [nx, ny]) in self.0.into_iter().enumerate() {
            x[index] = 2. * nx - 1.;
            y[index] = 2. * ny - 1.;
            let zz = 1. - x[index] * x[index] - y[index] * y[index];
            z[index] = if zz > 0. {
                f64::from(zz).sqrt() as f32
            } else {
                1.
            };
        }
        for (channel, plane) in [x, y, z].into_iter().enumerate() {
            normals.set_buffer(channel as i32, Some(&FloatBuffer::new(plane)))?;
        }
        let mut pyramid = Vec::new();
        let mut width = size;
        while width > 1 {
            pyramid.push(width);
            width = (f64::from(width) / 2.).round_ties_even() as i32;
        }
        let mut stage = area::as_image(vec![0.; 4], ImageGeometry::new(2, 2))?;
        for width in pyramid.into_iter().rev() {
            crate::pointwise::check_render_cancelled(cancelled)?;
            let normal = image_resize::resize(&normals, width, width, execution)?;
            let previous = image_resize::resize(&stage, width, width, execution)?;
            let mut result = previous.materialize(0)?.values();
            relax::<E>(&mut result, &normal, width, cancelled)?;
            stage = area::as_image(result, ImageGeometry::new(width, width))?;
        }
        let mut result = stage.materialize(0)?.values();
        let mut minimum = result[0];
        let mut maximum = result[0];
        for &value in &result {
            minimum = minimum.min(value);
            maximum = maximum.max(value);
        }
        let range = maximum - minimum;
        let gain = if range == 0. { 0. } else { 1. / range };
        for value in &mut result {
            *value = (*value - minimum) * gain;
        }
        target.add_named_store(
            Some("Height Map".encode_utf16().collect()),
            Some(SharedFloatStore::new(FloatBuffer::new(result))?),
        )?;
        Ok(())
    }
}

fn relax<E>(
    values: &mut [f32],
    normals: &SharedImageMap,
    size: i32,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PointwiseError<E>> {
    let x = normals.materialize(0)?.values();
    let y = normals.materialize(1)?.values();
    let z = normals.materialize(2)?.values();
    let step = 1. / size as f32;
    let iterations = (size as f32 * (4. / f64::from(size).powf(0.6)) as f32) as usize;
    let size = size as usize;
    // The original calling-thread worker reads prior updates in the same sweep.
    for _ in 0..iterations {
        for row in 0..size {
            crate::pointwise::check_render_cancelled(cancelled)?;
            let upper = ((row + size - 1) % size) * size;
            let lower = ((row + 1) % size) * size;
            let current = row * size;
            for column in 0..size {
                let up = upper + column;
                let down = lower + column;
                let left = current + (column + size - 1) % size;
                let right = current + (column + 1) % size;
                let from_up = values[up] + (y[up] / z[up]) * step;
                let from_down = values[down] + ((0. - y[down]) / z[down]) * step;
                let from_left = values[left] + ((0. - x[left]) / z[left]) * step;
                let from_right = values[right] + (x[right] / z[right]) * step;
                let average = (from_up + from_down + from_left + from_right) * 0.25;
                values[current + column] = values[current + column] * (1. - 0.9) + average * 0.9;
            }
        }
    }
    Ok(())
}
