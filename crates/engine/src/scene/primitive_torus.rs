//! Direct port of TexTree afu.mi; array access and allocation are fallible.
use super::*;
pub(super) fn torus(
    ring: i32,
    pipe: i32,
    radius: f64,
    effects: &mut SceneAllocationEffects<'_>,
) -> Result<MeshModel, SceneError> {
    let mut mesh = MeshBuild::new();
    let mut list: SourceList<MeshVector> = SourceList::new();
    let mut list2: SourceList<MeshUv> = SourceList::new();
    let mut list3: SourceList<MeshVector> = SourceList::new();
    let mut list4: SourceList<MeshVector> = SourceList::new();
    let value: i32 = 4_i32.wrapping_mul(ring).wrapping_mul(pipe);
    mesh.group(PolygonTopology::Quads, value, effects)?;
    let mut cursor: i32 = 0;
    let num: f64 = 360.0 / sd(ring);
    let num2: f64 = 360.0 / sd(pipe);
    let array: Indices = Indices::zeros(pipe)?;
    let array2: Indices = Indices::zeros(pipe)?;
    let array3: Indices = Indices::zeros(pipe)?;
    let array4: Indices = Indices::zeros(pipe.wrapping_add(1))?;
    let array5: Indices = Indices::zeros(pipe.wrapping_add(1))?;
    let array6: Indices = Indices::zeros(pipe.wrapping_add(1))?;
    {
        let mut num3: i32 = 0;
        while num3 < pipe {
            let num4: f64 = sd(num3) * num2;
            array2.put(
                num3,
                torus_add(&mut list, &mut list3, &mut list4, 0.0, num4, radius)?,
            )?;
            array.put(num3, array2.at(num3)?)?;
            array5.put(num3, torus_uv_add(&mut list2, 0.0, num4)?)?;
            array4.put(num3, array5.at(num3)?)?;
            num3 = num3.wrapping_add(1);
        }
    }
    array5.put(pipe, torus_uv_add(&mut list2, 0.0, 360.0)?)?;
    array4.put(pipe, array5.at(pipe)?)?;
    let mut array7: Indices;
    let mut normal: Indices;
    {
        let mut num5: i32 = 1;
        while num5 < ring {
            let num6: f64 = sd(num5) * num;
            {
                let mut num7: i32 = 0;
                while num7 < pipe {
                    let num8: f64 = sd(num7) * num2;
                    array3.put(
                        num7,
                        torus_add(&mut list, &mut list3, &mut list4, num6, num8, radius)?,
                    )?;
                    array6.put(num7, torus_uv_add(&mut list2, num6, num8)?)?;
                    if num7 > 0 {
                        array7 = Indices::values(vec![
                            array2.at(num7)?,
                            array3.at(num7)?,
                            array3.at(num7.wrapping_sub(1))?,
                            array2.at(num7.wrapping_sub(1))?,
                        ]);
                        normal = Indices::values(vec![
                            array5.at(num7)?,
                            array6.at(num7)?,
                            array6.at(num7.wrapping_sub(1))?,
                            array5.at(num7.wrapping_sub(1))?,
                        ]);
                        mesh.emit(0, &mut cursor, &[&array7, &normal, &array7, &array7])?;
                    }
                    num7 = num7.wrapping_add(1);
                }
            }
            array6.put(pipe, torus_uv_add(&mut list2, num6, 360.0)?)?;
            array7 = Indices::values(vec![
                array2.at(0)?,
                array3.at(0)?,
                array3.at(pipe.wrapping_sub(1))?,
                array2.at(pipe.wrapping_sub(1))?,
            ]);
            normal = Indices::values(vec![
                array5.at(pipe)?,
                array6.at(pipe)?,
                array6.at(pipe.wrapping_sub(1))?,
                array5.at(pipe.wrapping_sub(1))?,
            ]);
            mesh.emit(0, &mut cursor, &[&array7, &normal, &array7, &array7])?;
            {
                let mut num9: i32 = 0;
                while num9 < pipe {
                    array2.put(num9, array3.at(num9)?)?;
                    array5.put(num9, array6.at(num9)?)?;
                    num9 = num9.wrapping_add(1);
                }
            }
            array5.put(pipe, array6.at(pipe)?)?;
            num5 = num5.wrapping_add(1);
        }
    }
    array6.put(0, torus_uv_add(&mut list2, 360.0, 0.0)?)?;
    {
        let mut num10: i32 = 1;
        while num10 < pipe {
            let uv_indices: f64 = sd(num10) * num2;
            array6.put(num10, torus_uv_add(&mut list2, 360.0, uv_indices)?)?;
            array7 = Indices::values(vec![
                array2.at(num10)?,
                array.at(num10)?,
                array.at(num10.wrapping_sub(1))?,
                array2.at(num10.wrapping_sub(1))?,
            ]);
            normal = Indices::values(vec![
                array5.at(num10)?,
                array6.at(num10)?,
                array6.at(num10.wrapping_sub(1))?,
                array5.at(num10.wrapping_sub(1))?,
            ]);
            mesh.emit(0, &mut cursor, &[&array7, &normal, &array7, &array7])?;
            num10 = num10.wrapping_add(1);
        }
    }
    array6.put(pipe, torus_uv_add(&mut list2, 360.0, 360.0)?)?;
    array7 = Indices::values(vec![
        array2.at(0)?,
        array.at(0)?,
        array.at(pipe.wrapping_sub(1))?,
        array2.at(pipe.wrapping_sub(1))?,
    ]);
    normal = Indices::values(vec![
        array5.at(pipe)?,
        array6.at(pipe)?,
        array6.at(pipe.wrapping_sub(1))?,
        array5.at(pipe.wrapping_sub(1))?,
    ]);
    mesh.emit(0, &mut cursor, &[&array7, &normal, &array7, &array7])?;

    mesh.finish(list, list2, list3, list4, true, effects)
}
