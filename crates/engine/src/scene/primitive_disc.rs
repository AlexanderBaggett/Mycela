//! Direct port of TexTree avd.mi; array access and allocation are fallible.
use super::*;
pub(super) fn disc(
    circular: i32,
    radial: i32,
    inner: f64,
    two: bool,
    effects: &mut SceneAllocationEffects<'_>,
) -> Result<MeshModel, SceneError> {
    let mut mesh = MeshBuild::new();
    let mut list: SourceList<MeshVector> = SourceList::new();
    let mut list2: SourceList<MeshUv> = SourceList::new();
    let vector3f: MeshVector = MeshVector::new(0_f32, 1_f32, 0_f32);
    let vector3f2: MeshVector = MeshVector::new(0_f32, -1_f32, 0_f32);
    let vector3f3: MeshVector = MeshVector::new(1_f32, 0_f32, 0_f32);
    let mut num: i32;
    let mut num2: i32;
    if inner > 0.0 {
        num = 4_i32.wrapping_mul(circular).wrapping_mul(radial);
        num2 = 0;
    } else {
        num = 4_i32
            .wrapping_mul(circular.wrapping_sub(1))
            .wrapping_mul(radial);
        num2 = 3_i32.wrapping_mul(radial);
    }
    if two {
        num = num.wrapping_mul(2);
        num2 = num2.wrapping_mul(2);
    }
    if num > 0 {
        mesh.group(PolygonTopology::Quads, num, effects)?;
    }
    if num2 > 0 {
        mesh.group(PolygonTopology::Triangles, num2, effects)?;
    }
    let mut cursor: i32 = 0;
    let num3: f64 = 360.0 / sd(radial);
    let num4: f64 = (0.5 - inner) / sd(circular);
    let array: Indices = Indices::zeros(radial)?;
    let array2: Indices = Indices::zeros(radial)?;
    let array3: Indices = Indices::zeros(radial)?;
    let array4: Indices = Indices::zeros(radial)?;
    {
        let mut num5: i32 = 0;
        while num5 < radial {
            let value: f64 = sd(num5) * num3;
            let (disc_position, disc_uv) = disc_add(&mut list, &mut list2, 0.5, value, two)?;
            array.put(num5, disc_position)?;
            array3.put(num5, disc_uv)?;
            num5 = num5.wrapping_add(1);
        }
    }
    let array5: Indices = Indices::zeros(4)?;
    let array6: Indices = array5.clone();
    let normal: Indices = Indices::values(vec![1, 1, 1, 1]);
    let num6: i32 = if num2 > 0 {
        circular
    } else {
        circular.wrapping_add(1)
    };
    {
        let mut num7: i32 = 1;
        while num7 < num6 {
            let uv_indices: f64 = 0.5 - sd(num7) * num4;
            let mut value6: Indices;
            let mut value5: Indices;
            {
                let mut num8: i32 = 0;
                while num8 < radial {
                    let value4: f64 = sd(num8) * num3;
                    let (disc_position, disc_uv) =
                        disc_add(&mut list, &mut list2, uv_indices, value4, two)?;
                    array2.put(num8, disc_position)?;
                    array4.put(num8, disc_uv)?;
                    if num8 > 0 {
                        value5 = Indices::values(vec![
                            array3.at(num8.wrapping_sub(1))?,
                            array3.at(num8)?,
                            array4.at(num8)?,
                            array4.at(num8.wrapping_sub(1))?,
                        ]);
                        value6 = Indices::values(vec![
                            array.at(num8.wrapping_sub(1))?,
                            array.at(num8)?,
                            array2.at(num8)?,
                            array2.at(num8.wrapping_sub(1))?,
                        ]);
                        mesh.emit(0, &mut cursor, &[&value6, &value5, &array6, &array6])?;
                        if two {
                            value6 = Indices::values(vec![
                                array.at(num8)?.wrapping_add(1),
                                array.at(num8.wrapping_sub(1))?.wrapping_add(1),
                                array2.at(num8.wrapping_sub(1))?.wrapping_add(1),
                                array2.at(num8)?.wrapping_add(1),
                            ]);
                            value5 = Indices::values(vec![
                                array3.at(num8)?,
                                array3.at(num8.wrapping_sub(1))?,
                                array4.at(num8.wrapping_sub(1))?,
                                array4.at(num8)?,
                            ]);
                            mesh.emit(0, &mut cursor, &[&value6, &value5, &normal, &array6])?;
                        }
                    }
                    num8 = num8.wrapping_add(1);
                }
            }
            value5 = Indices::values(vec![
                array3.at(radial.wrapping_sub(1))?,
                array3.at(0)?,
                array4.at(0)?,
                array4.at(radial.wrapping_sub(1))?,
            ]);
            value6 = Indices::values(vec![
                array.at(radial.wrapping_sub(1))?,
                array.at(0)?,
                array2.at(0)?,
                array2.at(radial.wrapping_sub(1))?,
            ]);
            mesh.emit(0, &mut cursor, &[&value6, &value5, &array6, &array6])?;
            if two {
                value6 = Indices::values(vec![
                    array.at(0)?.wrapping_add(1),
                    array.at(radial.wrapping_sub(1))?.wrapping_add(1),
                    array2.at(radial.wrapping_sub(1))?.wrapping_add(1),
                    array2.at(0)?.wrapping_add(1),
                ]);
                value5 = Indices::values(vec![
                    array3.at(0)?,
                    array3.at(radial.wrapping_sub(1))?,
                    array4.at(radial.wrapping_sub(1))?,
                    array4.at(0)?,
                ]);
                mesh.emit(0, &mut cursor, &[&value6, &value5, &normal, &array6])?;
            }
            {
                let mut num9: i32 = 0;
                while num9 < radial {
                    array.put(num9, array2.at(num9)?)?;
                    array3.put(num9, array4.at(num9)?)?;
                    num9 = num9.wrapping_add(1);
                }
            }
            num7 = num7.wrapping_add(1);
        }
    }
    if num2 > 0 {
        cursor = 0;
        let (disc_position, disc_uv) = disc_add(&mut list, &mut list2, 0.0, 0.0, two)?;
        let center_position = disc_position;
        let center_uv = disc_uv;
        let mut value6: Indices;
        let mut value5: Indices;
        {
            let mut num10: i32 = 1;
            while num10 < radial {
                value6 = Indices::values(vec![
                    array.at(num10.wrapping_sub(1))?,
                    array.at(num10)?,
                    center_position,
                ]);
                value5 = Indices::values(vec![
                    array3.at(num10.wrapping_sub(1))?,
                    array3.at(num10)?,
                    center_uv,
                ]);
                mesh.emit(
                    mesh.group_count().wrapping_sub(1),
                    &mut cursor,
                    &[&value6, &value5, &array6, &array6],
                )?;
                if two {
                    value6 = Indices::values(vec![
                        array.at(num10)?.wrapping_add(1),
                        array.at(num10.wrapping_sub(1))?.wrapping_add(1),
                        center_position.wrapping_add(1),
                    ]);
                    value5 = Indices::values(vec![
                        array3.at(num10)?,
                        array3.at(num10.wrapping_sub(1))?,
                        center_uv,
                    ]);
                    mesh.emit(
                        mesh.group_count().wrapping_sub(1),
                        &mut cursor,
                        &[&value6, &value5, &normal, &array6],
                    )?;
                }
                num10 = num10.wrapping_add(1);
            }
        }
        value6 = Indices::values(vec![
            array.at(radial.wrapping_sub(1))?,
            array.at(0)?,
            center_position,
        ]);
        value5 = Indices::values(vec![
            array3.at(radial.wrapping_sub(1))?,
            array3.at(0)?,
            center_uv,
        ]);
        mesh.emit(
            mesh.group_count().wrapping_sub(1),
            &mut cursor,
            &[&value6, &value5, &array6, &array6],
        )?;
        if two {
            value6 = Indices::values(vec![
                array.at(0)?.wrapping_add(1),
                array.at(radial.wrapping_sub(1))?.wrapping_add(1),
                center_position.wrapping_add(1),
            ]);
            value5 = Indices::values(vec![
                array3.at(0)?,
                array3.at(radial.wrapping_sub(1))?,
                center_uv,
            ]);
            mesh.emit(
                mesh.group_count().wrapping_sub(1),
                &mut cursor,
                &[&value6, &value5, &normal, &array6],
            )?;
        }
    }
    let list3 = SourceList::from_values(if two {
        vec![vector3f, vector3f2]
    } else {
        vec![vector3f]
    });
    let list4 = SourceList::from_values(vec![vector3f3]);
    mesh.finish(list, list2, list3, list4, false, effects)
}
