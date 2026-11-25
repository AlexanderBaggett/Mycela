//! Direct port of TexTree arh.mi; array access and allocation are fallible.
use super::*;
pub(super) fn cube(
    xs: i32,
    ys: i32,
    zs: i32,
    effects: &mut SceneAllocationEffects<'_>,
) -> Result<MeshModel, SceneError> {
    let mut mesh = MeshBuild::new();
    let mut list: SourceList<MeshVector> = SourceList::new();
    let mut list2: SourceList<MeshUv> = SourceList::new();
    let mut list3: SourceList<MeshVector> = SourceList::new();
    let mut list4: SourceList<MeshVector> = SourceList::new();
    list3.push(MeshVector::new(-1_f32, 0_f32, 0_f32))?;
    list3.push(MeshVector::new(0_f32, 0_f32, -1_f32))?;
    list3.push(MeshVector::new(0_f32, 0_f32, 1_f32))?;
    list3.push(MeshVector::new(0_f32, 1_f32, 0_f32))?;
    list3.push(MeshVector::new(0_f32, -1_f32, 0_f32))?;
    list3.push(MeshVector::new(1_f32, 0_f32, 0_f32))?;
    list4.push(MeshVector::new(0_f32, 0_f32, -1_f32))?;
    list4.push(MeshVector::new(1_f32, 0_f32, 0_f32))?;
    list4.push(MeshVector::new(-1_f32, 0_f32, 0_f32))?;
    list4.push(MeshVector::new(1_f32, 0_f32, 0_f32))?;
    list4.push(MeshVector::new(1_f32, 0_f32, 0_f32))?;
    list4.push(MeshVector::new(0_f32, 0_f32, 1_f32))?;
    let value: i32 = 8_i32.wrapping_mul(
        xs.wrapping_mul(ys)
            .wrapping_add(xs.wrapping_mul(zs))
            .wrapping_add(ys.wrapping_mul(zs)),
    );
    mesh.group(PolygonTopology::Quads, value, effects)?;
    let mut cursor: i32 = 0;
    let num: f32 = 1_f32 / sf(xs);
    let num2: f32 = 1_f32 / sf(ys);
    let num3: f32 = 1_f32 / sf(zs);
    let num4: i32 = ys.wrapping_add(zs).wrapping_mul(2);
    let array: Indices = Indices::zeros(num4)?;
    let array2: Indices = Indices::zeros(num4)?;
    let num5: i32 = ys.wrapping_add(zs).wrapping_mul(2).wrapping_add(4);
    let array3: Indices = Indices::zeros(num5)?;
    let array4: Indices = Indices::zeros(num5)?;
    let array5: Indices = Indices::zeros(ys.wrapping_add(1))?;
    let array6: Indices = Indices::zeros(ys.wrapping_add(1))?;
    let array7: Indices = Indices::zeros(ys.wrapping_add(1))?;
    let array8: Indices = Indices::zeros(ys.wrapping_add(1))?;
    let num6: f32 = 47_f32 / 150_f32;
    let abk2: MeshUv = MeshUv::new(0.01_f32, 0.01_f32);
    let abk3: MeshUv = MeshUv::new(0.01_f32, 0.34333333_f32);
    let abk4: MeshUv = MeshUv::new(0.01_f32, 0.6766667_f32);
    let abk5: MeshUv = MeshUv::new(0.34333333_f32, 0.6766667_f32);
    let abk6: MeshUv = MeshUv::new(0.34333333_f32, 0.34333333_f32);
    let abk7: MeshUv = MeshUv::new(0.34333333_f32, 0.01_f32);
    let num7: f32 = num6 / sf(xs);
    let num8: f32 = num6 / sf(ys);
    let num9: f32 = num6 / sf(zs);
    {
        let mut num10: i32 = 0;
        while num10 <= ys {
            list.push(MeshVector::new(0_f32, sf(num10) * num2, 0_f32))?;
            array.put(num10, list.count().wrapping_sub(1))?;
            list.push(MeshVector::new(0_f32, sf(num10) * num2, 1_f32))?;
            array.put(
                num4.wrapping_sub(zs).wrapping_sub(num10),
                list.count().wrapping_sub(1),
            )?;
            array5.put(num10, array.at(num10)?)?;
            list2.push(MeshUv::new(
                abk2.u(),
                abk2.v() + sf(ys.wrapping_sub(num10)) * num8,
            ))?;
            array3.put(num10, list2.count().wrapping_sub(1))?;
            list2.push(MeshUv::new(
                abk3.u() + sf(xs) * num7,
                abk3.v() + sf(ys.wrapping_sub(num10)) * num8,
            ))?;
            array3.put(
                num5.wrapping_sub(zs).wrapping_sub(num10).wrapping_sub(2),
                list2.count().wrapping_sub(1),
            )?;
            list2.push(MeshUv::new(
                abk6.u() + sf(zs) * num9,
                abk6.v() + sf(ys.wrapping_sub(num10)) * num8,
            ))?;
            array7.put(num10, list2.count().wrapping_sub(1))?;
            num10 = num10.wrapping_add(1);
        }
    }
    {
        let mut num11: i32 = 0;
        while num11 <= zs {
            if num11 > 0 && num11 < zs {
                list.push(MeshVector::new(0_f32, 1_f32, sf(num11) * num3))?;
                array.put(num11.wrapping_add(ys), list.count().wrapping_sub(1))?;
                list.push(MeshVector::new(0_f32, 0_f32, sf(num11) * num3))?;
                array.put(num4.wrapping_sub(num11), list.count().wrapping_sub(1))?;
            }
            list2.push(MeshUv::new(
                abk4.u(),
                abk4.v() + sf(zs.wrapping_sub(num11)) * num9,
            ))?;
            array3.put(
                num11.wrapping_add(ys).wrapping_add(1),
                list2.count().wrapping_sub(1),
            )?;
            list2.push(MeshUv::new(abk5.u(), abk5.v() + sf(num11) * num9))?;
            array3.put(
                num5.wrapping_sub(num11).wrapping_sub(1),
                list2.count().wrapping_sub(1),
            )?;
            num11 = num11.wrapping_add(1);
        }
    }
    let array9: Indices = Indices::zeros(4)?;
    let array10: Indices = array9.clone();
    {
        let mut num12: i32 = 1;
        while num12 <= zs {
            array6.put(0, array.at(num4.wrapping_sub(num12))?)?;
            list2.push(MeshUv::new(
                abk6.u() + sf(zs.wrapping_sub(num12)) * num9,
                abk6.v() + sf(ys) * num8,
            ))?;
            array8.put(0, list2.count().wrapping_sub(1))?;
            {
                let mut num13: i32 = 1;
                while num13 <= ys {
                    list2.push(MeshUv::new(
                        abk6.u() + sf(zs.wrapping_sub(num12)) * num9,
                        abk6.v() + sf(ys.wrapping_sub(num13)) * num8,
                    ))?;
                    array8.put(num13, list2.count().wrapping_sub(1))?;
                    if num12 < zs && num13 < ys {
                        list.push(MeshVector::new(0_f32, sf(num13) * num2, sf(num12) * num3))?;
                        array6.put(num13, list.count().wrapping_sub(1))?;
                    } else if num12 == zs {
                        array6.put(num13, array.at(num4.wrapping_sub(zs).wrapping_sub(num13))?)?;
                    } else {
                        array6.put(num13, array.at(num12.wrapping_add(ys))?)?;
                    }
                    let normal: Indices = Indices::values(vec![
                        array5.at(num13)?,
                        array6.at(num13)?,
                        array6.at(num13.wrapping_sub(1))?,
                        array5.at(num13.wrapping_sub(1))?,
                    ]);
                    let uv_indices: Indices = Indices::values(vec![
                        array7.at(num13)?,
                        array8.at(num13)?,
                        array8.at(num13.wrapping_sub(1))?,
                        array7.at(num13.wrapping_sub(1))?,
                    ]);
                    mesh.emit(0, &mut cursor, &[&normal, &uv_indices, &array10, &array10])?;
                    num13 = num13.wrapping_add(1);
                }
            }
            {
                let mut num14: i32 = 0;
                while num14 <= ys {
                    array5.put(num14, array6.at(num14)?)?;
                    array7.put(num14, array8.at(num14)?)?;
                    num14 = num14.wrapping_add(1);
                }
            }
            num12 = num12.wrapping_add(1);
        }
    }
    let array11: Indices = Indices::values(vec![1, 1, 1, 1]);
    let array12: Indices = Indices::values(vec![2, 2, 2, 2]);
    let array13: Indices = Indices::values(vec![3, 3, 3, 3]);
    let array14: Indices = Indices::values(vec![4, 4, 4, 4]);
    let array15: Indices = Indices::values(vec![5, 5, 5, 5]);
    {
        let mut num15: i32 = 1;
        while num15 <= xs {
            {
                let mut num16: i32 = 0;
                while num16 <= ys {
                    list.push(MeshVector::new(sf(num15) * num, sf(num16) * num2, 0_f32))?;
                    array2.put(num16, list.count().wrapping_sub(1))?;
                    list.push(MeshVector::new(sf(num15) * num, sf(num16) * num2, 1_f32))?;
                    array2.put(
                        num4.wrapping_sub(zs).wrapping_sub(num16),
                        list.count().wrapping_sub(1),
                    )?;
                    list2.push(MeshUv::new(
                        abk2.u() + sf(num15) * num7,
                        abk2.v() + sf(ys.wrapping_sub(num16)) * num8,
                    ))?;
                    array4.put(num16, list2.count().wrapping_sub(1))?;
                    list2.push(MeshUv::new(
                        abk3.u() + sf(xs.wrapping_sub(num15)) * num7,
                        abk3.v() + sf(ys.wrapping_sub(num16)) * num8,
                    ))?;
                    array4.put(
                        num5.wrapping_sub(zs).wrapping_sub(num16).wrapping_sub(2),
                        list2.count().wrapping_sub(1),
                    )?;
                    if num16 > 0 {
                        let mut normal: Indices = Indices::values(vec![
                            array.at(num16.wrapping_sub(1))?,
                            array2.at(num16.wrapping_sub(1))?,
                            array2.at(num16)?,
                            array.at(num16)?,
                        ]);
                        let mut uv_indices: Indices = Indices::values(vec![
                            array3.at(num16.wrapping_sub(1))?,
                            array4.at(num16.wrapping_sub(1))?,
                            array4.at(num16)?,
                            array3.at(num16)?,
                        ]);
                        mesh.emit(0, &mut cursor, &[&normal, &uv_indices, &array11, &array11])?;
                        normal = Indices::values(vec![
                            array.at(num4.wrapping_sub(zs).wrapping_sub(num16))?,
                            array2.at(num4.wrapping_sub(zs).wrapping_sub(num16))?,
                            array2.at(num4.wrapping_sub(zs).wrapping_sub(num16).wrapping_add(1))?,
                            array.at(num4.wrapping_sub(zs).wrapping_sub(num16).wrapping_add(1))?,
                        ]);
                        uv_indices = Indices::values(vec![
                            array3.at(num5.wrapping_sub(zs).wrapping_sub(num16).wrapping_sub(2))?,
                            array4.at(num5.wrapping_sub(zs).wrapping_sub(num16).wrapping_sub(2))?,
                            array4.at(num5.wrapping_sub(zs).wrapping_sub(num16).wrapping_sub(1))?,
                            array3.at(num5.wrapping_sub(zs).wrapping_sub(num16).wrapping_sub(1))?,
                        ]);
                        mesh.emit(0, &mut cursor, &[&normal, &uv_indices, &array12, &array12])?;
                    }
                    if num15 == xs {
                        array5.put(num16, array2.at(num16)?)?;
                        list2.push(MeshUv::new(
                            abk7.u(),
                            abk7.v() + sf(ys.wrapping_sub(num16)) * num8,
                        ))?;
                        array7.put(num16, list2.count().wrapping_sub(1))?;
                    }
                    num16 = num16.wrapping_add(1);
                }
            }
            {
                let mut num17: i32 = 0;
                while num17 <= zs {
                    list2.push(MeshUv::new(
                        abk4.u() + sf(num15) * num7,
                        abk4.v() + sf(zs.wrapping_sub(num17)) * num9,
                    ))?;
                    array4.put(
                        num17.wrapping_add(ys).wrapping_add(1),
                        list2.count().wrapping_sub(1),
                    )?;
                    list2.push(MeshUv::new(
                        abk5.u() + sf(num15) * num7,
                        abk5.v() + sf(num17) * num9,
                    ))?;
                    array4.put(
                        num5.wrapping_sub(num17).wrapping_sub(1),
                        list2.count().wrapping_sub(1),
                    )?;
                    if num17 > 0 {
                        if num17 < zs {
                            list.push(MeshVector::new(sf(num15) * num, 1_f32, sf(num17) * num3))?;
                            array2.put(num17.wrapping_add(ys), list.count().wrapping_sub(1))?;
                            list.push(MeshVector::new(sf(num15) * num, 0_f32, sf(num17) * num3))?;
                            array2.put(num4.wrapping_sub(num17), list.count().wrapping_sub(1))?;
                        }
                        let mut normal: Indices = Indices::values(vec![
                            array.at(num17.wrapping_add(ys).wrapping_sub(1))?,
                            array2.at(num17.wrapping_add(ys).wrapping_sub(1))?,
                            array2.at(num17.wrapping_add(ys))?,
                            array.at(num17.wrapping_add(ys))?,
                        ]);
                        let mut uv_indices: Indices = Indices::values(vec![
                            array3.at(num17.wrapping_add(ys))?,
                            array4.at(num17.wrapping_add(ys))?,
                            array4.at(num17.wrapping_add(ys).wrapping_add(1))?,
                            array3.at(num17.wrapping_add(ys).wrapping_add(1))?,
                        ]);
                        mesh.emit(0, &mut cursor, &[&normal, &uv_indices, &array13, &array13])?;
                        let num18: i32 = if num17 > 1 {
                            num4.wrapping_sub(num17).wrapping_add(1)
                        } else {
                            0
                        };
                        normal = Indices::values(vec![
                            array.at(num4.wrapping_sub(num17))?,
                            array2.at(num4.wrapping_sub(num17))?,
                            array2.at(num18)?,
                            array.at(num18)?,
                        ]);
                        uv_indices = Indices::values(vec![
                            array3.at(num5.wrapping_sub(num17).wrapping_sub(1))?,
                            array4.at(num5.wrapping_sub(num17).wrapping_sub(1))?,
                            array4.at(num5.wrapping_sub(num17))?,
                            array3.at(num5.wrapping_sub(num17))?,
                        ]);
                        mesh.emit(0, &mut cursor, &[&normal, &uv_indices, &array14, &array14])?;
                    }
                    num17 = num17.wrapping_add(1);
                }
            }
            {
                let mut num19: i32 = 0;
                while num19 < num4 {
                    array.put(num19, array2.at(num19)?)?;
                    num19 = num19.wrapping_add(1);
                }
            }
            {
                let mut num20: i32 = 0;
                while num20 < num5 {
                    array3.put(num20, array4.at(num20)?)?;
                    num20 = num20.wrapping_add(1);
                }
            }
            if num15 != xs {
                num15 = num15.wrapping_add(1);
                continue;
            }
            {
                let mut num21: i32 = 1;
                while num21 <= zs {
                    array6.put(0, array.at(num4.wrapping_sub(num21))?)?;
                    list2.push(MeshUv::new(
                        abk7.u() + sf(num21) * num9,
                        abk7.v() + sf(ys) * num8,
                    ))?;
                    array8.put(0, list2.count().wrapping_sub(1))?;
                    {
                        let mut num22: i32 = 1;
                        while num22 <= ys {
                            list2.push(MeshUv::new(
                                abk7.u() + sf(num21) * num9,
                                abk7.v() + sf(ys.wrapping_sub(num22)) * num8,
                            ))?;
                            array8.put(num22, list2.count().wrapping_sub(1))?;
                            if num21 < zs && num22 < ys {
                                list.push(MeshVector::new(
                                    sf(num15) * num,
                                    sf(num22) * num2,
                                    sf(num21) * num3,
                                ))?;
                                array6.put(num22, list.count().wrapping_sub(1))?;
                            } else if num21 == zs {
                                array6.put(
                                    num22,
                                    array.at(num4.wrapping_sub(zs).wrapping_sub(num22))?,
                                )?;
                            } else {
                                array6.put(num22, array.at(num21.wrapping_add(ys))?)?;
                            }
                            let normal: Indices = Indices::values(vec![
                                array5.at(num22.wrapping_sub(1))?,
                                array6.at(num22.wrapping_sub(1))?,
                                array6.at(num22)?,
                                array5.at(num22)?,
                            ]);
                            let uv_indices: Indices = Indices::values(vec![
                                array7.at(num22.wrapping_sub(1))?,
                                array8.at(num22.wrapping_sub(1))?,
                                array8.at(num22)?,
                                array7.at(num22)?,
                            ]);
                            mesh.emit(0, &mut cursor, &[&normal, &uv_indices, &array15, &array15])?;
                            num22 = num22.wrapping_add(1);
                        }
                    }
                    {
                        let mut num23: i32 = 0;
                        while num23 <= ys {
                            array5.put(num23, array6.at(num23)?)?;
                            array7.put(num23, array8.at(num23)?)?;
                            num23 = num23.wrapping_add(1);
                        }
                    }
                    num21 = num21.wrapping_add(1);
                }
            }
            num15 = num15.wrapping_add(1);
        }
    }

    mesh.finish(list, list2, list3, list4, false, effects)
}
