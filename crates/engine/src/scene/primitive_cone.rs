//! Direct port of TexTree aa6.mi; array access and allocation are fallible.
use super::*;
pub(super) fn cone(
    length: i32,
    rot: i32,
    cap: i32,
    caps: bool,
    effects: &mut SceneAllocationEffects<'_>,
) -> Result<MeshModel, SceneError> {
    let taper = 100.;
    let mut mesh = MeshBuild::new();
    let mut list: SourceList<MeshVector> = SourceList::new();
    let mut list2: SourceList<MeshUv> = SourceList::new();
    let mut list3: SourceList<MeshVector> = SourceList::new();
    let mut list4: SourceList<MeshVector> = SourceList::new();
    let mut index: i32 = 0;
    let mut num: i32 = 0;
    if length > 1 {
        num = num.wrapping_add(4_i32.wrapping_mul(length.wrapping_sub(1)).wrapping_mul(rot));
    }
    if caps && cap > 1 {
        num = num.wrapping_add(4_i32.wrapping_mul(cap.wrapping_sub(1)).wrapping_mul(rot));
    }
    if num > 0 {
        mesh.group(PolygonTopology::Quads, num, effects)?;
        index = 1;
    }
    if caps {
        mesh.group(PolygonTopology::Triangles, 6_i32.wrapping_mul(rot), effects)?;
    } else {
        mesh.group(PolygonTopology::Triangles, 3_i32.wrapping_mul(rot), effects)?;
    }
    let mut cursor: i32 = 0;
    let num2: f64 = 360.0 / sd(rot);
    let num3: f64 = 1.0 / sd(length);
    let num4: f64 = taper / 100.0;
    let array: Indices = Indices::zeros(rot)?;
    let array2: Indices = Indices::zeros(rot)?;
    let array3: Indices = Indices::zeros(rot)?;
    let array4: Indices = Indices::zeros(rot.wrapping_add(1))?;
    let array5: Indices = Indices::zeros(rot.wrapping_add(1))?;
    let mut array6: Indices = Indices::zeros(4)?;
    let array7: Indices = Indices::zeros(4)?;
    let mut array8: Indices = Indices::zeros(4)?;
    let mut value: f64 = 0.5;
    let mut num5: i32 = 0;
    let mut num6: f64 = 0.0;
    let mut sample: Sample;
    while num5 <= rot {
        sample = cone_side(-0.5, value, num6, caps)?;
        if num5 < rot {
            list.push(sample.position)?;
            list3.push(sample.normal)?;
            list4.push(sample.tangent)?;
            array.put(num5, list.count().wrapping_sub(1))?;
            array2.put(num5, array.at(num5)?)?;
        }
        list2.push(sample.uv)?;
        array4.put(num5, list2.count().wrapping_sub(1))?;
        num5 = num5.wrapping_add(1);
        num6 += num2;
    }
    let mut num7: i32 = 1;
    let mut num8: f64 = -0.5 + sd(num7) * num3;
    while num7 < length {
        value = 0.5 * (1.0 - num4 * (num8 + 0.5));
        num5 = 0;
        num6 = 0.0;
        while num5 <= rot {
            sample = cone_side(num8, value, num6, caps)?;
            if num5 < rot {
                list.push(sample.position)?;
                array3.put(num5, list.count().wrapping_sub(1))?;
            }
            list2.push(sample.uv)?;
            array5.put(num5, list2.count().wrapping_sub(1))?;
            if num5 > 0 {
                array6.put(0, array2.at(num5.wrapping_sub(1))?)?;
                array7.put(0, array.at(num5.wrapping_sub(1))?)?;
                array7.put(3, array7.at(0)?)?;
                if num5 < rot {
                    array6.put(1, array2.at(num5)?)?;
                    array6.put(2, array3.at(num5)?)?;
                    array7.put(1, array.at(num5)?)?;
                    array7.put(2, array7.at(1)?)?;
                } else {
                    array6.put(1, array2.at(0)?)?;
                    array6.put(2, array3.at(0)?)?;
                    array7.put(1, array.at(0)?)?;
                    array7.put(2, array7.at(1)?)?;
                }
                array6.put(3, array3.at(num5.wrapping_sub(1))?)?;
                array8.put(0, array4.at(num5.wrapping_sub(1))?)?;
                array8.put(1, array4.at(num5)?)?;
                array8.put(2, array5.at(num5)?)?;
                array8.put(3, array5.at(num5.wrapping_sub(1))?)?;
                mesh.emit(0, &mut cursor, &[&array6, &array8, &array7, &array7])?;
            }
            num5 = num5.wrapping_add(1);
            num6 += num2;
        }
        {
            num5 = 0;
            while num5 < rot {
                array2.put(num5, array3.at(num5)?)?;
                array4.put(num5, array5.at(num5)?)?;
                num5 = num5.wrapping_add(1);
            }
        }
        array4.put(rot, array5.at(rot)?)?;
        num7 = num7.wrapping_add(1);
        num8 += num3;
    }
    sample = cone_side(0.5, 0.0, 0.0, caps)?;
    list.push(sample.position)?;
    let num9: i32 = list.count().wrapping_sub(1);
    let mut center_position: i32 = 0;
    array6 = Indices::zeros(3)?;
    array8 = Indices::zeros(3)?;
    let mut array9: Indices = Indices::zeros(3)?;
    num5 = 1;
    num6 = num2;
    while num5 <= rot {
        sample = cone_side(0.5, 0.0, num6, caps)?;
        list2.push(sample.uv)?;
        let num10: i32 = list2.count().wrapping_sub(1);
        array6.put(0, num9)?;
        array6.put(1, array2.at(num5.wrapping_sub(1))?)?;
        array9.put(1, num5.wrapping_sub(1))?;
        array7.put(1, num5.wrapping_sub(1))?;
        if num5 < rot {
            array6.put(2, array2.at(num5)?)?;
            array8.put(2, array4.at(num5)?)?;
            array9.put(2, num5)?;
            array7.put(2, num5)?;
        } else {
            array6.put(2, array2.at(0)?)?;
            array9.put(2, 0)?;
            array7.put(2, 0)?;
        }
        array8.put(0, num10)?;
        array8.put(1, array4.at(num5.wrapping_sub(1))?)?;
        array8.put(2, array4.at(num5)?)?;
        let mut normal: MeshVector =
            MeshVector::add(list4.at(array9.at(1)?)?, list4.at(array9.at(2)?)?);
        normal.normalize();
        let vector3f: MeshVector = list3.at(array7.at(2)?)?;
        normal = MeshVector::sub(normal, MeshVector::scale(normal.dot(vector3f), vector3f));
        normal.normalize();
        list4.push(normal)?;
        array9.put(0, list4.count().wrapping_sub(1))?;
        array7.put(0, array7.at(2)?)?;
        mesh.emit(
            index,
            &mut center_position,
            &[&array6, &array8, &array7, &array9],
        )?;
        num5 = num5.wrapping_add(1);
        num6 += num2;
    }
    if caps {
        let array10: Indices = Indices::zeros(rot)?;
        let array11: Indices = Indices::zeros(rot)?;
        let array12: Indices = Indices::zeros(rot)?;
        num5 = 0;
        num6 = 0.0;
        while num5 < rot {
            sample = cone_cap(-0.5, 0.5, num6)?;
            list2.push(sample.uv)?;
            array12.put(num5, list2.count().wrapping_sub(1))?;
            num5 = num5.wrapping_add(1);
            num6 += num2;
        }
        list3.push(MeshVector::new(0_f32, -1_f32, 0_f32))?;
        let num11: i32 = list3.count().wrapping_sub(1);
        let array13: Indices = Indices::zeros(4)?;
        list4.push(MeshVector::new(1_f32, 0_f32, 0_f32))?;
        let num12: i32 = list4.count().wrapping_sub(1);
        array9 = Indices::zeros(4)?;
        {
            let mut num13: i32 = 0;
            while num13 < 4 {
                array13.put(num13, num11)?;
                array9.put(num13, num12)?;
                num13 = num13.wrapping_add(1);
            }
        }
        if cap > 1 {
            array6 = Indices::zeros(4)?;
            array8 = Indices::zeros(4)?;
            let num14: f64 = 0.5 / sd(cap);
            num7 = 1;
            let mut num15: f64 = 0.5 - num14;
            while num7 < cap {
                num5 = 0;
                num6 = 0.0;
                while num5 <= rot {
                    sample = cone_cap(-0.5, num15, num6)?;
                    if num5 < rot {
                        list.push(sample.position)?;
                        array10.put(num5, list.count().wrapping_sub(1))?;
                        list2.push(sample.uv)?;
                        array11.put(num5, list2.count().wrapping_sub(1))?;
                    }
                    if num5 > 0 {
                        array6.put(0, array.at(num5.wrapping_sub(1))?)?;
                        if num5 < rot {
                            array6.put(1, array.at(num5)?)?;
                            array6.put(2, array10.at(num5)?)?;
                            array8.put(1, array12.at(num5)?)?;
                            array8.put(2, array11.at(num5)?)?;
                        } else {
                            array6.put(1, array.at(0)?)?;
                            array6.put(2, array10.at(0)?)?;
                            array8.put(1, array12.at(0)?)?;
                            array8.put(2, array11.at(0)?)?;
                        }
                        array6.put(3, array10.at(num5.wrapping_sub(1))?)?;
                        array8.put(0, array12.at(num5.wrapping_sub(1))?)?;
                        array8.put(3, array11.at(num5.wrapping_sub(1))?)?;
                        mesh.emit(0, &mut cursor, &[&array6, &array8, &array13, &array9])?;
                    }
                    num5 = num5.wrapping_add(1);
                    num6 += num2;
                }
                {
                    num5 = 0;
                    while num5 < rot {
                        array2.put(num5, array3.at(num5)?)?;
                        array4.put(num5, array5.at(num5)?)?;
                        array.put(num5, array10.at(num5)?)?;
                        array12.put(num5, array11.at(num5)?)?;
                        num5 = num5.wrapping_add(1);
                    }
                }
                num7 = num7.wrapping_add(1);
                num15 -= num14;
            }
        }
        sample = cone_cap(-0.5, 0.0, 0.0)?;
        list.push(sample.position)?;
        let num16: i32 = list.count().wrapping_sub(1);
        array6 = Indices::zeros(3)?;
        array8 = Indices::zeros(3)?;
        num5 = 1;
        num6 = num2;
        while num5 <= rot {
            sample = cone_cap(-0.5, 0.0, num6)?;
            list2.push(sample.uv)?;
            let num17: i32 = list2.count().wrapping_sub(1);
            array6.put(0, num16)?;
            array6.put(1, array.at(num5.wrapping_sub(1))?)?;
            if num5 < rot {
                array6.put(2, array.at(num5)?)?;
                array8.put(2, array12.at(num5)?)?;
            } else {
                array6.put(2, array.at(0)?)?;
                array8.put(2, array12.at(0)?)?;
            }
            array8.put(0, num17)?;
            array8.put(1, array12.at(num5.wrapping_sub(1))?)?;
            mesh.emit(
                index,
                &mut center_position,
                &[&array6, &array8, &array13, &array9],
            )?;
            num5 = num5.wrapping_add(1);
            num6 += num2;
        }
    }

    mesh.finish(list, list2, list3, list4, true, effects)
}
