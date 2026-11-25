//! Direct port of TexTree sk.mi; array access and allocation are fallible.
use super::*;
pub(super) fn sphere(
    latitude: i32,
    longitude: i32,
    effects: &mut SceneAllocationEffects<'_>,
) -> Result<MeshModel, SceneError> {
    let mut mesh = MeshBuild::new();
    let mut list: SourceList<MeshVector> = SourceList::new();
    let mut list2: SourceList<MeshUv> = SourceList::new();
    let mut list3: SourceList<MeshVector> = SourceList::new();
    let mut list4: SourceList<MeshVector> = SourceList::new();
    mesh.group(
        PolygonTopology::Triangles,
        6_i32.wrapping_mul(longitude),
        effects,
    )?;
    if latitude > 2 {
        mesh.group(
            PolygonTopology::Quads,
            4_i32
                .wrapping_mul(latitude.wrapping_sub(2))
                .wrapping_mul(longitude),
            effects,
        )?;
    }
    let mut cursor: i32 = 0;
    let num: f64 = 360.0 / sd(longitude);
    let num2: f64 = 180.0 / sd(latitude);
    let array: Indices = Indices::zeros(longitude)?;
    let array2: Indices = Indices::zeros(longitude)?;
    let array3: Indices = Indices::zeros(longitude)?;
    let array4: Indices = Indices::zeros(longitude.wrapping_add(1))?;
    let array5: Indices = Indices::zeros(longitude.wrapping_add(1))?;
    let array6: Indices = Indices::zeros(longitude.wrapping_add(1))?;
    let mut num3: i32 = 0;
    let mut num4: f64 = 0.0;
    let mut sample: Sample;
    while num3 <= longitude {
        sample = sphere_sample(num2, num4)?;
        if num3 < longitude {
            list.push(sample.position)?;
            list3.push(sample.normal)?;
            list4.push(sample.tangent)?;
            array.put(num3, list.count().wrapping_sub(1))?;
            array2.put(num3, array.at(num3)?)?;
            if latitude == 2 {
                array3.put(num3, array2.at(num3)?)?;
            }
        }
        list2.push(sample.uv)?;
        array4.put(num3, list2.count().wrapping_sub(1))?;
        array5.put(num3, array4.at(num3)?)?;
        if latitude == 2 {
            array6.put(num3, array5.at(num3)?)?;
        }
        num3 = num3.wrapping_add(1);
        num4 += num;
    }
    let mut array7: Indices = Indices::zeros(4)?;
    let mut array8: Indices = Indices::zeros(4)?;
    let mut num5: i32 = 2;
    let mut num6: f64 = sd(num5) * num2;
    while num5 < latitude {
        num3 = 0;
        num4 = 0.0;
        while num3 <= longitude {
            sample = sphere_sample(num6, num4)?;
            if num3 < longitude {
                list.push(sample.position)?;
                list3.push(sample.normal)?;
                list4.push(sample.tangent)?;
                array3.put(num3, list.count().wrapping_sub(1))?;
            }
            list2.push(sample.uv)?;
            array6.put(num3, list2.count().wrapping_sub(1))?;
            if num3 > 0 {
                if num3 < longitude {
                    array7.put(1, array2.at(num3)?)?;
                    array7.put(0, array3.at(num3)?)?;
                } else {
                    array7.put(1, array2.at(0)?)?;
                    array7.put(0, array3.at(0)?)?;
                }
                array7.put(3, array3.at(num3.wrapping_sub(1))?)?;
                array7.put(2, array2.at(num3.wrapping_sub(1))?)?;
                array8.put(1, array5.at(num3)?)?;
                array8.put(0, array6.at(num3)?)?;
                array8.put(3, array6.at(num3.wrapping_sub(1))?)?;
                array8.put(2, array5.at(num3.wrapping_sub(1))?)?;
                mesh.emit(1, &mut cursor, &[&array7, &array8, &array7, &array7])?;
            }
            num3 = num3.wrapping_add(1);
            num4 += num;
        }
        {
            num3 = 0;
            while num3 < longitude {
                array2.put(num3, array3.at(num3)?)?;
                array5.put(num3, array6.at(num3)?)?;
                num3 = num3.wrapping_add(1);
            }
        }
        array5.put(longitude, array6.at(longitude)?)?;
        num5 = num5.wrapping_add(1);
        num6 += num2;
    }
    sample = sphere_sample(0.0, 0.0)?;
    list.push(sample.position)?;
    list3.push(sample.normal)?;
    let num7: i32 = list.count().wrapping_sub(1);
    sample = sphere_sample(180.0, 0.0)?;
    list.push(sample.position)?;
    list3.push(sample.normal)?;
    let num8: i32 = list.count().wrapping_sub(1);
    cursor = 0;
    array7 = Indices::zeros(3)?;
    array8 = Indices::zeros(3)?;
    let array9: Indices = Indices::zeros(3)?;
    num3 = 0;
    num4 = 0.0;
    while num3 <= longitude {
        sample = sphere_sample(0.0, num4)?;
        list2.push(sample.uv)?;
        let num9: i32 = list2.count().wrapping_sub(1);
        sample = sphere_sample(180.0, num4)?;
        list2.push(sample.uv)?;
        let num10: i32 = list2.count().wrapping_sub(1);
        if num3 > 0 {
            array7.put(0, num7)?;
            array7.put(1, array.at(num3.wrapping_sub(1))?)?;
            array9.put(1, array.at(num3.wrapping_sub(1))?)?;
            if num3 < longitude {
                array7.put(2, array.at(num3)?)?;
                array9.put(2, array.at(num3)?)?;
            } else {
                array7.put(2, array.at(0)?)?;
                array9.put(2, array.at(0)?)?;
            }
            let mut item: MeshVector =
                MeshVector::add(list4.at(array9.at(1)?)?, list4.at(array9.at(2)?)?);
            item.normalize();
            list4.push(item)?;
            array9.put(0, list4.count().wrapping_sub(1))?;
            array8.put(0, num9)?;
            array8.put(1, array4.at(num3.wrapping_sub(1))?)?;
            array8.put(2, array4.at(num3)?)?;
            mesh.emit(0, &mut cursor, &[&array7, &array8, &array7, &array9])?;
            array7.put(0, num8)?;
            array7.put(2, array3.at(num3.wrapping_sub(1))?)?;
            array9.put(2, array3.at(num3.wrapping_sub(1))?)?;
            if num3 < longitude {
                array7.put(1, array3.at(num3)?)?;
                array9.put(1, array3.at(num3)?)?;
            } else {
                array7.put(1, array3.at(0)?)?;
                array9.put(1, array3.at(0)?)?;
            }
            item = MeshVector::add(list4.at(array9.at(1)?)?, list4.at(array9.at(2)?)?);
            item.normalize();
            list4.push(item)?;
            array9.put(0, list4.count().wrapping_sub(1))?;
            array8.put(0, num10)?;
            array8.put(2, array6.at(num3.wrapping_sub(1))?)?;
            array8.put(1, array6.at(num3)?)?;
            mesh.emit(0, &mut cursor, &[&array7, &array8, &array7, &array9])?;
        }
        num3 = num3.wrapping_add(1);
        num4 += num;
    }

    mesh.finish(list, list2, list3, list4, true, effects)
}
