//! Direct port of TexTree jg.mi; array access and allocation are fallible.
use super::*;
pub(super) fn plane(
    xs: i32,
    ys: i32,
    two: bool,
    effects: &mut SceneAllocationEffects<'_>,
) -> Result<MeshModel, SceneError> {
    let mut mesh = MeshBuild::new();
    let mut list: SourceList<MeshVector> = SourceList::new();
    let mut list2: SourceList<MeshUv> = SourceList::new();
    let vector3f: MeshVector = MeshVector::new(0_f32, 1_f32, 0_f32);
    let vector3f2: MeshVector = MeshVector::new(0_f32, -1_f32, 0_f32);
    let vector3f3: MeshVector = MeshVector::new(1_f32, 0_f32, 0_f32);
    let array: Indices = Indices::zeros(4)?;
    let array2: Indices = array.clone();
    let value: Indices = Indices::values(vec![1, 1, 1, 1]);
    let mut num: i32 = 4_i32.wrapping_mul(xs.wrapping_mul(ys));
    let num2: f32;
    if two {
        num = num.wrapping_mul(2);
        num2 = 5E-05_f32;
    } else {
        num2 = 0_f32;
    }
    mesh.group(PolygonTopology::Quads, num, effects)?;
    let mut cursor: i32 = 0;
    let num3: f32 = 1_f32 / sf(xs);
    let num4: f32 = 1_f32 / sf(ys);
    let num5: i32 = 1_i32.wrapping_add(ys);
    let num6: f32 = 0.98_f32;
    let num7: f32 = num6 / sf(xs);
    let num8: f32 = num6 / sf(ys);
    {
        let mut num9: i32 = 0;
        while num9 <= ys {
            list.push(MeshVector::new(0_f32, num2, sf(num9) * num4))?;
            if two {
                list.push(MeshVector::new(0_f32, 0_f32 - num2, sf(num9) * num4))?;
            }
            list2.push(MeshUv::new(0.01_f32, 1_f32 - (0.01_f32 + sf(num9) * num8)))?;
            num9 = num9.wrapping_add(1);
        }
    }
    {
        let mut num10: i32 = 1;
        while num10 <= xs {
            {
                let mut num11: i32 = 0;
                while num11 <= ys {
                    list2.push(MeshUv::new(
                        0.01_f32 + sf(num10) * num7,
                        1_f32 - (0.01_f32 + sf(num11) * num8),
                    ))?;
                    list.push(MeshVector::new(sf(num10) * num3, num2, sf(num11) * num4))?;
                    if two {
                        list.push(MeshVector::new(
                            sf(num10) * num3,
                            0_f32 - num2,
                            sf(num11) * num4,
                        ))?;
                    }
                    if num11 <= 0 {
                        num11 = num11.wrapping_add(1);
                        continue;
                    }
                    let array3: Indices = Indices::values(vec![
                        num10
                            .wrapping_sub(1)
                            .wrapping_mul(num5)
                            .wrapping_add(num11)
                            .wrapping_sub(1),
                        num10.wrapping_mul(num5).wrapping_add(num11).wrapping_sub(1),
                        num10.wrapping_mul(num5).wrapping_add(num11),
                        num10.wrapping_sub(1).wrapping_mul(num5).wrapping_add(num11),
                    ]);
                    if two {
                        let array4: Indices = Indices::zeros(4)?;
                        let array5: Indices = Indices::zeros(4)?;
                        let array6: Indices = Indices::zeros(4)?;
                        {
                            let mut num12: i32 = 0;
                            while num12 < 4 {
                                array4.put(num12, array3.at(num12)?.wrapping_mul(2))?;
                                array6.put(num12, array3.at(REVERSE.at(num12)?)?)?;
                                array5.put(
                                    num12,
                                    array6.at(num12)?.wrapping_mul(2).wrapping_add(1),
                                )?;
                                num12 = num12.wrapping_add(1);
                            }
                        }
                        mesh.emit(0, &mut cursor, &[&array4, &array3, &array2, &array2])?;
                        mesh.emit(0, &mut cursor, &[&array5, &array6, &value, &array2])?;
                    } else {
                        mesh.emit(0, &mut cursor, &[&array3, &array3, &array2, &array2])?;
                    }
                    num11 = num11.wrapping_add(1);
                }
            }
            num10 = num10.wrapping_add(1);
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
