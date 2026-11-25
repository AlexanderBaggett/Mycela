//! `ath.k8`: density-driven atmosphere volume and light attenuation.
//! The renderer builds the volume and shadow grid once per call, then marches
//! a separate view ray per pixel.
use super::{PointwiseError, PointwiseFilter, SharedImageMap, check_render_cancelled};
use crate::{FloatBuffer, ImageError, ImageGeometry, seeded_random::SeededRandom};

struct Grid {
    xy: i32,
    z: i32,
    data: Vec<f32>,
}
impl Grid {
    fn zeros<E>(xy: i32, z: i32) -> Result<Self, PointwiseError<E>> {
        let n = xy
            .checked_mul(xy)
            .and_then(|n| n.checked_mul(z))
            .ok_or(ImageError::Overflow)?;
        let n = usize::try_from(n).map_err(|_| ImageError::Overflow)?;
        let mut data = Vec::new();
        data.try_reserve_exact(n)
            .map_err(|_| ImageError::Allocation)?;
        data.resize(n, 0.0);
        Ok(Self { xy, z, data })
    }
    fn index<E>(&self, x: i32, y: i32, z: i32) -> Result<usize, PointwiseError<E>> {
        if x < 0 || x >= self.xy || y < 0 || y >= self.xy || z < 0 || z >= self.z {
            return Err(ImageError::Index.into());
        }
        usize::try_from((x * self.xy + y) * self.z + z).map_err(|_| ImageError::Index.into())
    }
    fn get<E>(&self, x: i32, y: i32, z: i32) -> Result<f32, PointwiseError<E>> {
        Ok(self.data[self.index::<E>(x, y, z)?])
    }
    fn set<E>(&mut self, x: i32, y: i32, z: i32, value: f32) -> Result<(), PointwiseError<E>> {
        let i = self.index::<E>(x, y, z)?;
        self.data[i] = value;
        Ok(())
    }
    fn sample<E>(
        &self,
        mut x: f32,
        mut y: f32,
        z: f32,
        fallback: f32,
    ) -> Result<f32, PointwiseError<E>> {
        let xy = self.xy as f32;
        x *= xy;
        y *= xy;
        while x >= xy {
            x -= xy;
        }
        while y >= xy {
            y -= xy;
        }
        let z = z * self.z as f32;
        let xi = crate::framework_cast::integer(f64::from(x));
        let yi = crate::framework_cast::integer(f64::from(y));
        let zi = crate::framework_cast::integer(f64::from(z));
        let xn = if xi != self.xy - 1 { xi + 1 } else { 0 };
        let yn = if yi != self.xy - 1 { yi + 1 } else { 0 };
        let fy = y - yi as f32;
        let row = |ix, iz| -> Result<f32, PointwiseError<E>> {
            if iz >= 0 && iz < self.z {
                let a = self.get::<E>(ix, yi, iz)?;
                Ok(a + fy * (self.get::<E>(ix, yn, iz)? - a))
            } else {
                Ok(fallback)
            }
        };
        let upper_l = row(xi, zi + 1)?;
        let upper_r = row(xn, zi + 1)?;
        let lower_l = row(xi, zi)?;
        let lower_r = row(xn, zi)?;
        let fx = x - xi as f32;
        let upper = upper_l + fx * (upper_r - upper_l);
        let lower = lower_l + fx * (lower_r - lower_l);
        Ok(lower + (z - zi as f32) * (upper - lower))
    }
}
struct Volume {
    fine: Grid,
    coarse: Grid,
    centers: Vec<[f32; 4]>,
    table: Vec<f32>,
}
impl Volume {
    fn noise<E>(&self, grid: &Grid, x: f32, y: f32, z: f32) -> Result<f32, PointwiseError<E>> {
        Ok((grid.sample::<E>(x, y, z, 0.0)?
            + grid.sample::<E>(x * 2.0, y * 2.0, z * 2.0, 0.0)? * 0.5)
            * 0.5)
    }
    fn density<E>(
        &self,
        x: f32,
        y: f32,
        z: f32,
        acc: f32,
        scale: f32,
        thickness: f32,
    ) -> Result<f32, PointwiseError<E>> {
        let fine = self.noise::<E>(&self.fine, x, y, z)?;
        let at =
            crate::framework_cast::integer(f64::from(fine * 0.5 * (self.table.len() - 1) as f32));
        let mut value = *self
            .table
            .get(usize::try_from(at).map_err(|_| ImageError::Index)?)
            .ok_or(ImageError::Index)?
            - 0.5;
        value = value * (1.0 - acc) * scale;
        let ox = self.noise::<E>(&self.coarse, x, y, z)?;
        let oy = self.noise::<E>(&self.coarse, x + 0.4, y, z + 0.3)?;
        let oz = self.coarse.sample::<E>(x, y + 0.4, z + 0.6, 0.0)? * 0.5;
        for &[cx, cy, cz, r] in &self.centers {
            let bounds = r + 0.6;
            for ix in -1..=1 {
                let xx = cx + ix as f32;
                for iy in -1..=1 {
                    let yy = cy + iy as f32;
                    if x - 0.5 > xx - bounds
                        && x - 0.5 < xx + bounds
                        && y - 0.5 > yy - bounds
                        && y - 0.5 < yy + bounds
                        && z - 0.5 > cz - bounds
                        && z - 0.5 < cz + bounds
                    {
                        let a = -x + ox + xx;
                        let b = -y + oy + yy;
                        let c = -z + oz + cz;
                        let mut d = (1.0 - (a * a + b * b + c * c) / r) * scale * thickness;
                        if d > 0.0 {
                            let t = 1.0 - d;
                            d = 1.0 - t * t;
                            value += d * (1.0 - value);
                        }
                    }
                }
            }
        }
        if value < 0.0 {
            value = 0.0;
        }
        Ok(value)
    }
}
fn wrap(x: f32) -> f32 {
    let mut x = x % 1.0;
    if x < 0.0 {
        x += 1.0;
    }
    x
}

pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    check_render_cancelled(cancelled)?;
    let input = inputs
        .first()
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let input_plane = input.channel(0, true)?;
    let varying = input_plane.len() > 1;
    let out = SharedImageMap::new(geometry);
    if !varying && input_plane.read(0)? == 0.0 {
        out.set_constant(3, 1.0)?;
        return Ok(Some(out));
    }
    let w = geometry.width();
    let h = geometry.height();
    if w <= 0 || h <= 0 {
        return Err(PointwiseError::UnsupportedCapability(
            "Atmosphere nonpositive geometry",
        ));
    }
    let area = usize::try_from(w.checked_mul(h).ok_or(ImageError::Overflow)?)
        .map_err(|_| ImageError::Overflow)?;
    let pos = f
        .parameter(0)?
        .custom_object()?
        .ok_or(PointwiseError::NullReference)?;
    let position = pos.light_position()?;
    let mut lx = -(position[0] as f32);
    let mut ly = position[1] as f32;
    let mut lz = -1.0f32;
    let length = (lx * lx + ly * ly + lz * lz).sqrt();
    lx /= length;
    ly /= length;
    lz /= length;
    let sample_density = f.parameter(3)?.effective_value_at(time)?;
    let quality = ((0.875 + f64::from(w) / 2048.0) * sample_density / 10.0) as f32;
    let step = 0.02f32 / quality;
    if !step.is_finite() || step <= 0.0 {
        return Err(PointwiseError::UnsupportedCapability(
            "Atmosphere invalid sample step",
        ));
    }
    let seed = crate::framework_cast::integer(f.parameter(5)?.effective_seed_at(time)?);
    let mut rng = SeededRandom::from_seed(seed).map_err(|_| ImageError::Overflow)?;
    let scale = f.parameter(4)?.effective_value_at(time)? * f.factor();
    let inverse = if scale == 0.0 { 100000.0 } else { 1.0 / scale };
    let detail = (1.0 / inverse.clamp(1.0 / 3.0, 3.0)) as f32;
    let count = crate::framework_cast::integer(f64::from(detail) * 3.0).max(1);
    let mut centers = Vec::new();
    centers
        .try_reserve_exact(count as usize)
        .map_err(|_| ImageError::Allocation)?;
    for _ in 0..count {
        centers.push([
            (rng.sample() - 0.5) as f32,
            (rng.sample() - 0.5) as f32,
            (rng.sample() - 0.5) as f32,
            (rng.sample() * 0.3 + 0.1) as f32 / detail,
        ]);
    }
    let side = crate::framework_cast::integer(f64::from(64.0f32 * detail));
    let mut fine = Grid::zeros::<E>(side, side)?;
    for slab in fine.data.chunks_mut((side * side) as usize) {
        check_render_cancelled(cancelled)?;
        for n in slab {
            *n = rng.sample() as f32;
        }
    }
    let side2 = side / 4;
    let mut coarse = Grid::zeros::<E>(side2, side2)?;
    for n in &mut coarse.data {
        *n = rng.sample() as f32;
    }
    let table = (0..5000)
        .map(|i| ((i as f64 / 4999.0 * 2.0).powi(2)) as f32)
        .collect();
    let volume = Volume {
        fine,
        coarse,
        centers,
        table,
    };
    let thickness = f.parameter(1)?.effective_value_at(time)? as f32;
    let shadow_strength = f.parameter(2)?.effective_value_at(time)? as f32;
    let shadow = if shadow_strength > 0.0 {
        let mut shadow = Grid::zeros::<E>(48, 12)?;
        let sx = -lx / 24.0;
        let sy = -ly / 24.0;
        let sz = -lz / 24.0;
        let threshold = (1.0f32 / 48.0 * 2.0).max(sz.abs() * 3.0);
        for z in (0..12).rev() {
            let z0 = z as f32 / 12.0;
            for y in 0..48 {
                check_render_cancelled(cancelled)?;
                let yy = y as f32 / 48.0;
                for x in 0..48 {
                    let xx = x as f32 / 48.0;
                    let mut acc = 0.0;
                    let (mut px, mut py, mut pz) = (xx, yy, z0);
                    let mut iter = 0;
                    while pz < 1.0 && acc < 0.95 && iter < 20 {
                        iter += 1;
                        pz += sz;
                        if pz - z0 > threshold || z0 - pz > threshold {
                            acc += (1.0 - acc) * (1.0 - shadow.sample::<E>(px, py, pz, 1.0)?);
                            break;
                        }
                        px = wrap(px + sx);
                        py = wrap(py + sy);
                        let factor = if varying {
                            let ix = crate::framework_cast::integer(f64::from(px * w as f32)) % w;
                            let iy = crate::framework_cast::integer(f64::from(py * h as f32)) % h;
                            input_plane.read(
                                usize::try_from(ix + iy * w).map_err(|_| ImageError::Index)?,
                            )? * thickness
                        } else {
                            input_plane.read(0)? * thickness
                        };
                        acc = volume.density::<E>(px, py, pz, acc, shadow_strength, factor)?;
                    }
                    shadow.set::<E>(x, y, z, 1.0 - acc)?;
                }
            }
        }
        Some(shadow)
    } else {
        None
    };
    let mut light = Vec::new();
    let mut transparency = Vec::new();
    light
        .try_reserve_exact(area)
        .map_err(|_| ImageError::Allocation)?;
    transparency
        .try_reserve_exact(area)
        .map_err(|_| ImageError::Allocation)?;
    for y in 0..h {
        check_render_cancelled(cancelled)?;
        for x in 0..w {
            let at = usize::try_from(x + y * w).map_err(|_| ImageError::Index)?;
            let factor = input_plane.read(if varying { at } else { 0 })? * thickness;
            let px = x as f32 / w as f32;
            let py = y as f32 / h as f32;
            let mut z = 1.0;
            let mut acc = 0.0;
            let mut brightness = 0.0;
            loop {
                let d = volume.density::<E>(px, py, z, acc, step, factor)?;
                let mut lit = d;
                if let Some(grid) = &shadow {
                    lit *= grid.sample::<E>(px, py, z, 1.0)?;
                }
                brightness += lit;
                acc += (1.0 - acc) * d;
                z -= step;
                if !(z > 0.0 && acc <= 0.97) {
                    break;
                }
            }
            light.push(brightness);
            transparency.push(1.0 - acc);
        }
    }
    out.set_buffer(0, Some(&FloatBuffer::new(light)))?;
    out.set_buffer(3, Some(&FloatBuffer::new(transparency)))?;
    out.merge_effects(&[Some(input.clone())])?;
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PointwiseKind, PointwiseRenderEffects, PointwiseRenderOptions, ProceduralExecution,
        RenderCaptureSession,
    };
    use std::cell::Cell;

    #[test]
    fn rejected_light_position_edit_restores_both_coordinates() {
        let filter = PointwiseFilter::<&'static str>::new(PointwiseKind::Atmosphere).unwrap();
        let property = filter.parameter(0).unwrap();
        let object = property.custom_object().unwrap().unwrap();
        object.add_handler(crate::PointwiseMatrixHandler::new(|position, _| {
            if position.light_position().unwrap() == [1., 1.] {
                Ok(())
            } else {
                Err("rejected position")
            }
        }));
        assert!(matches!(
            property.edit_light_position(&object, [1., 1.], [2.5, -0.5]),
            Err(PointwiseError::Observer("rejected position"))
        ));
        assert_eq!(object.light_position().unwrap(), [1., 1.]);
    }
    fn input(v: f32) -> SharedImageMap {
        let image = SharedImageMap::new(ImageGeometry::new(4, 4));
        image.set_constant(0, v).unwrap();
        image
    }
    fn render(f: &PointwiseFilter<()>, image: &SharedImageMap) -> SharedImageMap {
        f.render_supplied_with_options(
            &[Some(image.clone())],
            4,
            4,
            PointwiseRenderOptions::new(false, false),
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn retained_factory_wire_capture_and_transparent_empty_density() {
        let node = crate::create_node_by_identity::<()>(Some(
            &"Atmosphere".encode_utf16().collect::<Vec<_>>(),
        ))
        .unwrap();
        let f = node.filter().unwrap();
        assert_eq!(f.kind(), PointwiseKind::Atmosphere);
        assert_eq!(
            String::from_utf16_lossy(&node.slot(0).unwrap().name()),
            "Density"
        );
        assert_eq!(
            f.parameters()
                .iter()
                .map(|p| String::from_utf16_lossy(&p.name()))
                .collect::<Vec<_>>(),
            [
                "Light Position",
                "Thickness Factor",
                "Shadow",
                "Sample Density",
                "Detail Scale",
                "Seed"
            ]
        );
        let out = render(&f, &input(0.0));
        assert_eq!(out.channel(3, true).unwrap().read(0).unwrap(), 1.0);
        assert_eq!(out.channel(0, true).unwrap().read(0).unwrap(), 0.0);
        f.parameter(5).unwrap().set_number(17.0).unwrap();
        let copy = f.source_clone().unwrap();
        assert_eq!(copy.parameter(5).unwrap().number(), 17.0);
        let before = RenderCaptureSession::new().capture_filter(&f).unwrap();
        f.parameter(5).unwrap().set_number(18.0).unwrap();
        let after = RenderCaptureSession::new().capture_filter(&f).unwrap();
        assert!(!before.same_render_values(&after));
        let mut writer = crate::BinaryWriter::default();
        node.save_binary(&mut writer).unwrap();
        let mut reader = crate::BinaryCursor::new(writer.bytes()).unwrap();
        let identity = reader.read_string().unwrap();
        let loaded = crate::load_node_binary::<()>(&mut reader, 3.75, Some(&identity)).unwrap();
        assert_eq!(
            loaded.filter().unwrap().parameter(5).unwrap().number(),
            18.0
        );
        let result = node.render_original(4, 4).unwrap();
        assert!(result.caught_errors().is_empty());
        let graph_image = result.image().unwrap();
        assert_eq!(graph_image.channel(3, true).unwrap().read(0).unwrap(), 1.0);
    }
    #[test]
    fn varying_density_and_detail_scale_follow_retained_controls() {
        let f = PointwiseFilter::<()>::new(PointwiseKind::Atmosphere).unwrap();
        f.parameter(2).unwrap().set_number(0.0).unwrap();
        let source = SharedImageMap::new(ImageGeometry::new(4, 4));
        source
            .set_buffer(
                0,
                Some(&FloatBuffer::new(vec![
                    0.0, 0.2, 0.6, 1.0, 0.0, 0.2, 0.6, 1.0, 0.0, 0.2, 0.6, 1.0, 0.0, 0.2, 0.6, 1.0,
                ])),
            )
            .unwrap();
        let initial = render(&f, &source);
        f.parameter(4).unwrap().set_number(2.0).unwrap();
        f.commit_spatial_scale(2.0).unwrap();
        assert_eq!(f.parameter(4).unwrap().number(), 4.0);
        let scaled = render(&f, &source);
        assert_ne!(
            initial.channel(3, true).unwrap().values(),
            scaled.channel(3, true).unwrap().values()
        );
    }
    #[test]
    fn unshadowed_constant_density_matches_genetica() {
        let f = PointwiseFilter::<()>::new(PointwiseKind::Atmosphere).unwrap();
        f.parameter(2).unwrap().set_number(0.0).unwrap();
        let out = render(&f, &input(0.8));

        // Genetica x64 final render, 4 × 4 with a constant Solid Color Density.
        let samples = [
            (0, 0x3ec5fcc7, 0x3f26a98b),
            (3, 0x3efe0bea, 0x3f148ef1),
            (7, 0x3f39e0ce, 0x3ede8638),
            (13, 0x3cb72cd8, 0x3f7a49ed),
            (15, 0x3e2b6e14, 0x3f565f80),
        ];
        let red = out.channel(0, true).unwrap();
        let transparency = out.channel(3, true).unwrap();
        for (index, expected_red, expected_transparency) in samples {
            assert_eq!(red.read(index).unwrap().to_bits(), expected_red);
            assert_eq!(
                transparency.read(index).unwrap().to_bits(),
                expected_transparency
            );
        }
    }

    #[test]
    fn seeded_volume_and_shadow_produce_deterministic_distinct_images() {
        let f = PointwiseFilter::<()>::new(PointwiseKind::Atmosphere).unwrap();
        f.parameter(2).unwrap().set_number(0.0).unwrap();
        f.parameter(3).unwrap().set_number(1.0).unwrap();
        let src = input(0.8);
        let first = render(&f, &src);
        let second = render(&f, &src);
        assert_eq!(
            first.channel(0, true).unwrap().values(),
            second.channel(0, true).unwrap().values()
        );
        assert_eq!(
            first.channel(3, true).unwrap().values(),
            second.channel(3, true).unwrap().values()
        );
        assert!(
            first
                .channel(3, true)
                .unwrap()
                .values()
                .iter()
                .any(|v| *v < 0.999)
        );
        f.parameter(5).unwrap().set_number(9.0).unwrap();
        let other = render(&f, &src);
        assert_ne!(
            first.channel(0, true).unwrap().values(),
            other.channel(0, true).unwrap().values()
        );
        f.parameter(2).unwrap().set_number(0.2).unwrap();
        let shaded = render(&f, &src);
        assert_ne!(
            shaded.channel(0, true).unwrap().values(),
            other.channel(0, true).unwrap().values()
        );
    }

    #[test]
    fn supplied_render_propagates_cancellation_during_volume_setup() {
        let f = PointwiseFilter::<()>::new(PointwiseKind::Atmosphere).unwrap();
        let checks = Cell::new(0);
        let cancelled = || {
            checks.set(checks.get() + 1);
            checks.get() >= 8
        };
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_cancel_check(&cancelled);
        let result = f.render_supplied_at_time_with_effects(
            &[Some(input(0.8))],
            4,
            4,
            0.0,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(false, true),
            &mut effects,
        );
        assert!(matches!(result, Err(PointwiseError::Cancelled)));
        assert_eq!(checks.get(), 8);
    }
}
