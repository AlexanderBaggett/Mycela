//! Source ps.k8: retained material, height, environment and reflectivity.
use super::numeric::{add, div, mul, sub};
use super::{
    PointwiseError, PointwiseFilter, PointwiseRenderEffects, PointwiseRenderOptions,
    SharedImageMap, check_render_cancelled,
};
use crate::{ImageGeometry, ProceduralExecution};

#[allow(clippy::too_many_arguments)]
pub(super) fn retained<E: 'static>(
    f: &PointwiseFilter<E>,
    inputs: &[Option<SharedImageMap>],
    geometry: ImageGeometry,
    time: f64,
    execution: ProceduralExecution,
    options: PointwiseRenderOptions,
    effects: &mut PointwiseRenderEffects<'_, E>,
) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
    check_render_cancelled(effects.cancel_check())?;
    let material = inputs
        .first()
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let height = inputs
        .get(1)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let environment = inputs
        .get(2)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let reflectivity = inputs
        .get(3)
        .ok_or(PointwiseError::Index)?
        .as_ref()
        .ok_or(PointwiseError::NullReference)?;
    let reflectivity_present = match options.supplied_slot_connected(3) {
        Some(connected) => connected,
        None => f.node().slot(3)?.node().is_some(),
    };
    let skip = f.parameter(9)?.effective_index_at(time)? == 0;
    let position = f.parameter(0)?.custom_object()?;
    if position.as_ref().is_some_and(|p| !p.is_light_position()) {
        return Err(PointwiseError::InvalidCast);
    }
    let mut height_factor = f.parameter(1)?.effective_value_at(time)? / f.factor();
    if skip && options.is_color_only() {
        height_factor = 0.0;
    }
    let environment_strength = div(f.parameter(7)?.effective_value_at(time)? as f32, 100.0);
    let attenuation = div(f.parameter(8)?.effective_value_at(time)? as f32, 100.0);
    let baseline = sub(1.0, attenuation);
    let reflected = if environment_strength > 0.0 && !environment.is_empty()? {
        let result = super::environment_optics::reflect_sample::<E>(
            environment,
            height,
            height_factor,
            None,
            effects.cancel_check(),
        )?;
        result.seal_permanently()?;
        Some(result)
    } else {
        None
    };
    let output = SharedImageMap::new(geometry);
    let direct = div(f.parameter(2)?.effective_value_at(time)? as f32, 100.0);
    let light = if direct != 0.0 || attenuation != 0.0 {
        let position = position.as_ref().ok_or(PointwiseError::NullReference)?;
        let x = position.light_position_x()?;
        let y = position.light_position_y()?;
        let shadow = super::point_light::max_clr(
            0.0,
            super::point_light::min_clr(
                100.0,
                f.parameter(3)?.effective_value_at(time)? / f.factor(),
            ),
        );
        let gi = f.parameter(4)?.effective_value_at(time)?;
        let scale0 =
            super::point_light::min_clr(1.0, f.parameter(1)?.effective_value_at(time)?.abs() * 5.0);
        let scale1 = super::point_light::min_clr(
            1.0,
            (f.parameter(1)?.effective_value_at(time)? / f.factor()).abs() * 5.0,
        );
        let raw_ao = if scale1 == 0.0 {
            0.0
        } else {
            f.parameter(5)?.effective_value_at(time)? * scale0 / scale1
        };
        let ao_limits = f.parameter(5)?.limits()?.snapshot();
        let ao = if raw_ao <= ao_limits.minimum.to_framework_f64() {
            ao_limits.minimum.to_framework_f64()
        } else if raw_ao >= ao_limits.maximum.to_framework_f64() {
            ao_limits.maximum.to_framework_f64()
        } else {
            raw_ao
        };
        let radius_property = f.parameter(6)?;
        let radius_raw = radius_property.effective_value_at(time)?;
        let limits = radius_property.limits()?.snapshot();
        let radius = if radius_raw <= limits.minimum.to_framework_f64() {
            limits.minimum.to_framework_f64()
        } else if radius_raw >= limits.maximum.to_framework_f64() {
            limits.maximum.to_framework_f64()
        } else {
            radius_raw
        };
        let map = super::point_light_image(
            Some(height),
            geometry,
            super::PointLightArguments {
                x,
                y,
                height_factor,
                shadow_strength: shadow,
                global_illumination: gi,
                ambient_occlusion: ao,
                occlusion_radius: radius,
                factor: f.factor(),
            },
            execution,
            effects,
        )?;
        Some(map.materialize(0)?)
    } else {
        None
    };
    let cancelled = effects.cancel_check();
    for ch in 0..4 {
        check_render_cancelled(cancelled)?;
        let source = material.channel(ch, false)?;
        if direct != 0.0 {
            if source.len() == 1 {
                let value = source.read(0)?;
                if value != 0.0 {
                    let dst = output.materialize(ch)?;
                    let light = light.as_ref().ok_or(PointwiseError::NullReference)?;
                    let scale = mul(value, direct);
                    for i in 0..light.len() {
                        if i.is_multiple_of(4096) {
                            check_render_cancelled(cancelled)?;
                        }
                        dst.write(i, mul(light.read(i)?, scale))?;
                    }
                }
            } else {
                let dst = output.materialize(ch)?;
                let light = light.as_ref().ok_or(PointwiseError::NullReference)?;
                for i in 0..source.len() {
                    if i.is_multiple_of(4096) {
                        check_render_cancelled(cancelled)?;
                    }
                    dst.write(i, mul(mul(source.read(i)?, light.read(i)?), direct))?;
                }
            }
        }
        if let Some(reflected) = &reflected {
            let reflectivity_array = reflectivity.channel(0, false)?;
            let first = if reflectivity_present {
                reflectivity_array.read(0)?
            } else {
                1.0
            };
            let varying = reflectivity_present && reflectivity_array.len() > 1;
            if ch < reflected.channel_count()? as i32 {
                let env = reflected.materialize(ch)?;
                let dst = output.materialize(ch)?;
                for i in 0..env.len() {
                    if i.is_multiple_of(4096) {
                        check_render_cancelled(cancelled)?;
                    }
                    let reflectivity = if varying {
                        reflectivity_array.read(i)?
                    } else {
                        first
                    };
                    let light_value = if attenuation == 0.0 {
                        1.0
                    } else {
                        add(
                            mul(
                                light
                                    .as_ref()
                                    .ok_or(PointwiseError::NullReference)?
                                    .read(i)?,
                                attenuation,
                            ),
                            baseline,
                        )
                    };
                    let value = mul(
                        mul(mul(env.read(i)?, environment_strength), reflectivity),
                        light_value,
                    );
                    if value > 0.0 {
                        dst.write(i, add(dst.read(i)?, value))?;
                    }
                }
            }
        }
        output.seal_channel(ch)?;
    }
    output.set_region_store(height.region_store());
    if options.is_channel_rendering() {
        output.merge_effects(&[
            Some(material.clone()),
            Some(height.clone()),
            Some(reflectivity.clone()),
        ])?;
    }
    Ok(Some(output))
}

pub(super) fn commit_scale<E: 'static>(
    f: &PointwiseFilter<E>,
    scale: f64,
) -> Result<(), PointwiseError<E>> {
    let height = f.parameter(1)?;
    let mut old = 1.0;
    let mut new = 1.0;
    if super::parameter_ops::is_animated(|| height.retained_animation())? {
        super::point_light::scale_point_light_curve(&height, scale)?;
    } else {
        old = super::point_light::min_clr(1.0, height.number().abs() * 5.0);
        height.set_number(height.effective_value_at(0.0)? / scale)?;
        new = super::point_light::min_clr(1.0, height.number().abs() * 5.0);
    }
    let ao = f.parameter(5)?;
    if old != new && new != 0.0 && !super::parameter_ops::is_animated(|| ao.retained_animation())? {
        ao.set_number(ao.number() * old / new)?;
    }
    let radius = f.parameter(6)?;
    if super::parameter_ops::is_animated(|| radius.retained_animation())? {
        super::point_light::scale_point_light_curve(&radius, scale)?;
    } else {
        radius.set_number(super::point_light::min_clr(radius.number() / scale, 100.0))?;
    }
    let shadow = f.parameter(3)?;
    if super::parameter_ops::is_animated(|| shadow.retained_animation())? {
        super::point_light::scale_point_light_curve(&shadow, scale)?;
    } else {
        shadow.set_number(super::point_light::max_clr(
            0.0,
            super::point_light::min_clr(100.0, shadow.effective_value_at(0.0)? / scale),
        ))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FloatBuffer, PointwiseKind, SharedFloatStore};
    fn f() -> PointwiseFilter<()> {
        PointwiseFilter::new(PointwiseKind::FullLight).unwrap()
    }
    fn image(color: [f32; 4]) -> SharedImageMap {
        let v = SharedImageMap::new(ImageGeometry::new(4, 4));
        for (ch, n) in color.into_iter().enumerate() {
            v.set_constant(ch as i32, n).unwrap();
        }
        v
    }
    fn render(
        f: &PointwiseFilter<()>,
        images: &[SharedImageMap; 4],
        channels: bool,
    ) -> SharedImageMap {
        let inputs = images.iter().cloned().map(Some).collect::<Vec<_>>();
        f.render_supplied_at_time(
            &inputs,
            4,
            4,
            0.0,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(false, channels),
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn full_light_factory_wire_capture_scale_and_four_slot_preparation() {
        let node = crate::create_node_by_identity::<()>(Some(
            &"Full Light".encode_utf16().collect::<Vec<_>>(),
        ))
        .unwrap();
        let f = node.filter().unwrap();
        assert_eq!(f.kind().input_count(), 4);
        assert_eq!(
            node.slots()
                .iter()
                .map(|s| String::from_utf16_lossy(&s.name()))
                .collect::<Vec<_>>(),
            ["Material", "Heightmap", "Environment", "Reflectivity"]
        );
        assert_eq!(f.parameter(2).unwrap().number(), 100.0);
        assert_eq!(f.parameter(7).unwrap().number(), 50.0);
        f.parameter(2).unwrap().set_number(70.0).unwrap();
        let mut writer = crate::BinaryWriter::default();
        node.save_binary(&mut writer).unwrap();
        let mut reader = crate::BinaryCursor::new(writer.bytes()).unwrap();
        let name = reader.read_string().unwrap();
        let copy = crate::load_node_binary::<()>(&mut reader, 3.75, Some(&name)).unwrap();
        assert_eq!(copy.filter().unwrap().parameter(2).unwrap().number(), 70.0);
        let clone = f.source_clone().unwrap();
        assert_eq!(clone.parameter(2).unwrap().number(), 70.0);
        f.commit_spatial_scale(2.0).unwrap();
        assert_eq!(f.parameter(1).unwrap().number(), 0.025);
        let g = node
            .calculate_image_guarantees_for_inputs(&[None; 4])
            .unwrap();
        assert_eq!(g.constants(), [None; 4]);
        node.prepare_image_guarantees(&mut |_| Ok(())).unwrap();
        assert_eq!(node.prepared_image_guarantees().unwrap(), g);
        assert!(node.render_original(4, 4).unwrap().image().is_some());
        assert!(node.final_graph_clone().is_ok());
    }
    #[test]
    fn material_direct_light_reuses_the_actual_point_light_result() {
        let f = f();
        let material = image([0.25, 0.5, 0.75, 0.2]);
        let height = image([0.0; 4]);
        let blank = image([0.0; 4]);
        let output = render(
            &f,
            &[material.clone(), height.clone(), blank.clone(), blank],
            false,
        );
        let p = f.parameter(0).unwrap().custom_object().unwrap().unwrap();
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects: PointwiseRenderEffects<'_, ()> =
            PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let light = super::super::point_light_image(
            Some(&height),
            ImageGeometry::new(4, 4),
            super::super::PointLightArguments {
                x: p.light_position_x().unwrap(),
                y: p.light_position_y().unwrap(),
                height_factor: 0.05,
                shadow_strength: 0.0,
                global_illumination: 50.0,
                ambient_occlusion: 0.0,
                occlusion_radius: 10.0,
                factor: 1.0,
            },
            ProceduralExecution::CallingThread,
            &mut effects,
        )
        .unwrap();
        let value = light.channel(0, true).unwrap().read(0).unwrap();
        for (ch, c) in [0.25, 0.5, 0.75, 0.2].into_iter().enumerate() {
            assert_eq!(
                output.channel(ch as i32, true).unwrap().read(0).unwrap(),
                c * value
            );
        }
    }
    #[test]
    fn captured_full_light_keeps_real_position_and_property_values() {
        let f = f();
        let images = [
            image([0.25, 0.5, 0.75, 0.0]),
            image([0.0; 4]),
            image([0.0; 4]),
            image([0.0; 4]),
        ];
        let expected = render(&f, &images, false)
            .channel(0, true)
            .unwrap()
            .read(0)
            .unwrap();
        let frame = crate::RenderCaptureSession::new()
            .capture_filter(&f)
            .unwrap();
        f.parameter(2).unwrap().set_number(0.0).unwrap();
        let inputs = images.into_iter().map(Some).collect::<Vec<_>>();
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects: PointwiseRenderEffects<'_, ()> =
            PointwiseRenderEffects::new(&mut capacity, &mut preflight);
        let output = frame
            .render_with_execution_and_effects(
                &inputs,
                4,
                4,
                ProceduralExecution::CallingThread,
                PointwiseRenderOptions::new(false, false),
                &mut effects,
            )
            .unwrap()
            .unwrap();
        assert_eq!(output.channel(0, true).unwrap().read(0).unwrap(), expected);
    }
    #[test]
    fn supplied_black_reflectivity_is_distinct_from_an_unconnected_route() {
        let f = f();
        f.parameter(2).unwrap().set_number(0.0).unwrap();
        f.parameter(8).unwrap().set_number(0.0).unwrap();
        let reflectivity = image([0.0; 4]);
        reflectivity
            .set_buffer(0, Some(&FloatBuffer::new(vec![0.; 16])))
            .unwrap();
        let inputs = [
            image([0.0; 4]),
            image([0.0; 4]),
            image([0.4, 0.6, 0.8, 0.0]),
            reflectivity,
        ]
        .into_iter()
        .map(Some)
        .collect::<Vec<_>>();
        let draw = |present| {
            f.render_supplied_at_time(
                &inputs,
                4,
                4,
                0.0,
                ProceduralExecution::CallingThread,
                PointwiseRenderOptions::new(false, false)
                    .with_supplied_slot_presence([true, true, true, present]),
            )
            .unwrap()
            .unwrap()
            .channel(0, true)
            .unwrap()
            .read(0)
            .unwrap()
        };
        assert_eq!(draw(false), 0.2);
        assert_eq!(draw(true), 0.0);
        assert!(f.node().slot(3).unwrap().node().is_none());
    }

    #[test]
    fn full_light_polls_cancellation_during_environment_sampling() {
        let f = f();
        let inputs = [
            image([0.; 4]),
            image([0.; 4]),
            image([0.4, 0.6, 0.8, 0.]),
            image([0.; 4]),
        ]
        .map(Some);
        let checks = std::cell::Cell::new(0);
        let cancel_third_check = || {
            checks.set(checks.get() + 1);
            checks.get() == 3
        };
        let mut capacity = |_| Ok(true);
        let mut preflight = |_, _| Ok(());
        let mut effects = PointwiseRenderEffects::new(&mut capacity, &mut preflight)
            .with_cancel_check(&cancel_third_check);
        let frame = f.render_supplied_at_time_with_effects(
            &inputs,
            4,
            4,
            0.,
            ProceduralExecution::CallingThread,
            PointwiseRenderOptions::new(false, false),
            &mut effects,
        );
        assert!(matches!(frame, Err(PointwiseError::Cancelled)));
    }
    #[test]
    fn connected_reflectivity_changes_environment_contribution_and_maps() {
        let f = f();
        f.parameter(2).unwrap().set_number(0.0).unwrap();
        f.parameter(8).unwrap().set_number(0.0).unwrap();
        let material = image([0.0; 4]);
        let height = image([0.0; 4]);
        let env = image([0.4, 0.6, 0.8, 0.0]);
        let reflection = image([0.25, 0.0, 0.0, 0.0]);
        let region = SharedFloatStore::new(FloatBuffer::new(vec![0.3; 16])).unwrap();
        height.set_region_store(Some(region.clone()));
        let unconnected = render(
            &f,
            &[
                material.clone(),
                height.clone(),
                env.clone(),
                reflection.clone(),
            ],
            false,
        );
        assert_eq!(unconnected.channel(0, true).unwrap().read(0).unwrap(), 0.2);
        let child = crate::PointwiseNode::<()>::solid_color().unwrap();
        f.node().slot(3).unwrap().set_node(Some(&child)).unwrap();
        let connected = render(&f, &[material, height, env, reflection], true);
        assert_eq!(connected.channel(0, true).unwrap().read(0).unwrap(), 0.05);
        assert!(connected.region_store().unwrap().same_store(&region));
    }
}
