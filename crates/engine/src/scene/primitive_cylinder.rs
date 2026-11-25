//! Direct port of TexTree kw.mi; array access and allocation are fallible.
use super::*;
pub(super) fn cylinder(
    length: i32,
    rot: i32,
    cap: i32,
    taper: f64,
    caps: bool,
    effects: &mut SceneAllocationEffects<'_>,
) -> Result<MeshModel, SceneError> {
    let mut mesh = MeshBuild::new();
    let mut list: SourceList<MeshVector> = SourceList::new();
    let mut list2: SourceList<MeshUv> = SourceList::new();
    let mut list3: SourceList<MeshVector> = SourceList::new();
    let mut list4: SourceList<MeshVector> = SourceList::new();
    mesh.group(
        PolygonTopology::Quads,
        4_i32.wrapping_mul(length).wrapping_mul(rot),
        effects,
    )?;
    let mut index: i32 = 1_i32.wrapping_neg();
    if caps {
        if cap > 1 {
            mesh.group(
                PolygonTopology::Quads,
                8_i32.wrapping_mul(rot).wrapping_mul(cap.wrapping_sub(1)),
                effects,
            )?;
            index = 2;
        } else {
            index = 1;
        }
        mesh.group(PolygonTopology::Triangles, 6_i32.wrapping_mul(rot), effects)?;
    }
    let mut cursor: i32 = 0;
    let num: f64 = 360.0 / sd(rot);
    let num2: f64 = 1.0 / sd(length);
    let num3: f64 = taper / 100.0;
    let array: Indices = Indices::zeros(rot)?;
    let array2: Indices = Indices::zeros(rot)?;
    let array3: Indices = Indices::zeros(rot)?;
    let array4: Indices = Indices::zeros(rot.wrapping_add(1))?;
    let array5: Indices = Indices::zeros(rot.wrapping_add(1))?;
    let mut array6: Indices = Indices::zeros(4)?;
    let array7: Indices = Indices::zeros(4)?;
    let mut array8: Indices = Indices::zeros(4)?;
    let mut value: f64 = 0.5;
    let mut num4: i32 = 0;
    let mut num5: f64 = 0.0;
    while num4 <= rot {
        let sample: Sample = cylinder_side(-0.5, value, num5, taper, caps)?;
        if num4 < rot {
            list.push(sample.position)?;
            list3.push(sample.normal)?;
            list4.push(sample.tangent)?;
            array.put(num4, list.count().wrapping_sub(1))?;
            array2.put(num4, array.at(num4)?)?;
        }
        list2.push(sample.uv)?;
        array4.put(num4, list2.count().wrapping_sub(1))?;
        num4 = num4.wrapping_add(1);
        num5 += num;
    }
    let mut num6: i32 = 1;
    let mut num7: f64 = -0.5 + sd(num6) * num2;
    while num6 <= length {
        value = 0.5 * (1.0 - num3 * (num7 + 0.5));
        num4 = 0;
        num5 = 0.0;
        while num4 <= rot {
            let sample: Sample = cylinder_side(num7, value, num5, taper, caps)?;
            if num4 < rot {
                list.push(sample.position)?;
                array3.put(num4, list.count().wrapping_sub(1))?;
            }
            list2.push(sample.uv)?;
            array5.put(num4, list2.count().wrapping_sub(1))?;
            if num4 > 0 {
                array6.put(0, array2.at(num4.wrapping_sub(1))?)?;
                array7.put(0, array.at(num4.wrapping_sub(1))?)?;
                array7.put(3, array7.at(0)?)?;
                if num4 < rot {
                    array6.put(1, array2.at(num4)?)?;
                    array6.put(2, array3.at(num4)?)?;
                    array7.put(1, array.at(num4)?)?;
                    array7.put(2, array7.at(1)?)?;
                } else {
                    array6.put(1, array2.at(0)?)?;
                    array6.put(2, array3.at(0)?)?;
                    array7.put(1, array.at(0)?)?;
                    array7.put(2, array7.at(1)?)?;
                }
                array6.put(3, array3.at(num4.wrapping_sub(1))?)?;
                array8.put(0, array4.at(num4.wrapping_sub(1))?)?;
                array8.put(1, array4.at(num4)?)?;
                array8.put(2, array5.at(num4)?)?;
                array8.put(3, array5.at(num4.wrapping_sub(1))?)?;
                mesh.emit(0, &mut cursor, &[&array6, &array8, &array7, &array7])?;
            }
            num4 = num4.wrapping_add(1);
            num5 += num;
        }
        {
            num4 = 0;
            while num4 < rot {
                array2.put(num4, array3.at(num4)?)?;
                array4.put(num4, array5.at(num4)?)?;
                num4 = num4.wrapping_add(1);
            }
        }
        array4.put(rot, array5.at(rot)?)?;
        num6 = num6.wrapping_add(1);
        num7 += num2;
    }
    if caps {
        let array9: Indices = Indices::zeros(rot)?;
        let array10: Indices = Indices::zeros(rot)?;
        let array11: Indices = Indices::zeros(rot)?;
        num4 = 0;
        num5 = 0.0;
        let mut sample: Sample;
        while num4 < rot {
            sample = cylinder_cap(0.5, 0.5 * (1.0 - num3), num5)?;
            list2.push(sample.uv)?;
            array4.put(num4, list2.count().wrapping_sub(1))?;
            sample = cylinder_cap(-0.5, 0.5, num5)?;
            list2.push(sample.uv)?;
            array11.put(num4, list2.count().wrapping_sub(1))?;
            num4 = num4.wrapping_add(1);
            num5 += num;
        }
        list3.push(MeshVector::new(0_f32, 1_f32, 0_f32))?;
        let num8: i32 = list3.count().wrapping_sub(1);
        list3.push(MeshVector::new(0_f32, -1_f32, 0_f32))?;
        let num9: i32 = list3.count().wrapping_sub(1);
        let array12: Indices = Indices::zeros(4)?;
        list4.push(MeshVector::new(1_f32, 0_f32, 0_f32))?;
        let num10: i32 = list4.count().wrapping_sub(1);
        let array13: Indices = Indices::zeros(4)?;
        {
            let mut num11: i32 = 0;
            while num11 < 4 {
                array7.put(num11, num8)?;
                array12.put(num11, num9)?;
                array13.put(num11, num10)?;
                num11 = num11.wrapping_add(1);
            }
        }
        if cap > 1 {
            cursor = 0;
            let num12: f64 = 0.5 / sd(cap);
            num6 = 1;
            let mut num13: f64 = 0.5 - num12;
            while num6 < cap {
                num4 = 0;
                num5 = 0.0;
                while num4 <= rot {
                    sample = cylinder_cap(0.5, num13 * (1.0 - num3), num5)?;
                    if num4 < rot {
                        list.push(sample.position)?;
                        array3.put(num4, list.count().wrapping_sub(1))?;
                    }
                    list2.push(sample.uv)?;
                    array5.put(num4, list2.count().wrapping_sub(1))?;
                    if num4 > 0 {
                        array6.put(0, array2.at(num4.wrapping_sub(1))?)?;
                        if num4 < rot {
                            array6.put(1, array2.at(num4)?)?;
                            array6.put(2, array3.at(num4)?)?;
                            array8.put(1, array4.at(num4)?)?;
                            array8.put(2, array5.at(num4)?)?;
                        } else {
                            array6.put(1, array2.at(0)?)?;
                            array6.put(2, array3.at(0)?)?;
                            array8.put(1, array4.at(0)?)?;
                            array8.put(2, array5.at(0)?)?;
                        }
                        array6.put(3, array3.at(num4.wrapping_sub(1))?)?;
                        array8.put(0, array4.at(num4.wrapping_sub(1))?)?;
                        array8.put(3, array5.at(num4.wrapping_sub(1))?)?;
                        mesh.emit(1, &mut cursor, &[&array6, &array8, &array7, &array13])?;
                    }
                    sample = cylinder_cap(-0.5, num13, num5)?;
                    if num4 < rot {
                        list.push(sample.position)?;
                        array9.put(num4, list.count().wrapping_sub(1))?;
                        list2.push(sample.uv)?;
                        array10.put(num4, list2.count().wrapping_sub(1))?;
                    }
                    if num4 > 0 {
                        array6.put(1, array.at(num4.wrapping_sub(1))?)?;
                        if num4 < rot {
                            array6.put(0, array.at(num4)?)?;
                            array6.put(3, array9.at(num4)?)?;
                            array8.put(0, array11.at(num4)?)?;
                            array8.put(3, array10.at(num4)?)?;
                        } else {
                            array6.put(0, array.at(0)?)?;
                            array6.put(3, array9.at(0)?)?;
                            array8.put(0, array11.at(0)?)?;
                            array8.put(3, array10.at(0)?)?;
                        }
                        array6.put(2, array9.at(num4.wrapping_sub(1))?)?;
                        array8.put(1, array11.at(num4.wrapping_sub(1))?)?;
                        array8.put(2, array10.at(num4.wrapping_sub(1))?)?;
                        mesh.emit(1, &mut cursor, &[&array6, &array8, &array12, &array13])?;
                    }
                    num4 = num4.wrapping_add(1);
                    num5 += num;
                }
                {
                    num4 = 0;
                    while num4 < rot {
                        array2.put(num4, array3.at(num4)?)?;
                        array4.put(num4, array5.at(num4)?)?;
                        array.put(num4, array9.at(num4)?)?;
                        array11.put(num4, array10.at(num4)?)?;
                        num4 = num4.wrapping_add(1);
                    }
                }
                num6 = num6.wrapping_add(1);
                num13 -= num12;
            }
        }
        sample = cylinder_cap(0.5, 0.0, 0.0)?;
        list.push(sample.position)?;
        let num14: i32 = list.count().wrapping_sub(1);
        sample = cylinder_cap(-0.5, 0.0, 0.0)?;
        list.push(sample.position)?;
        let num15: i32 = list.count().wrapping_sub(1);
        cursor = 0;
        array6 = Indices::zeros(3)?;
        array8 = Indices::zeros(3)?;
        num4 = 1;
        num5 = num;
        while num4 <= rot {
            sample = cylinder_cap(0.5, 0.0, num5)?;
            list2.push(sample.uv)?;
            let num16: i32 = list2.count().wrapping_sub(1);
            sample = cylinder_cap(-0.5, 0.0, num5)?;
            list2.push(sample.uv)?;
            let num17: i32 = list2.count().wrapping_sub(1);
            array6.put(0, num14)?;
            array6.put(1, array2.at(num4.wrapping_sub(1))?)?;
            if num4 < rot {
                array6.put(2, array2.at(num4)?)?;
                array8.put(2, array4.at(num4)?)?;
            } else {
                array6.put(2, array2.at(0)?)?;
                array8.put(2, array4.at(0)?)?;
            }
            array8.put(0, num16)?;
            array8.put(1, array4.at(num4.wrapping_sub(1))?)?;
            mesh.emit(index, &mut cursor, &[&array6, &array8, &array7, &array13])?;
            array6.put(0, num15)?;
            array6.put(2, array.at(num4.wrapping_sub(1))?)?;
            if num4 < rot {
                array6.put(1, array.at(num4)?)?;
                array8.put(1, array11.at(num4)?)?;
            } else {
                array6.put(1, array.at(0)?)?;
                array8.put(1, array11.at(0)?)?;
            }
            array8.put(0, num17)?;
            array8.put(2, array11.at(num4.wrapping_sub(1))?)?;
            mesh.emit(index, &mut cursor, &[&array6, &array8, &array12, &array13])?;
            num4 = num4.wrapping_add(1);
            num5 += num;
        }
    }

    mesh.finish(list, list2, list3, list4, true, effects)
}
