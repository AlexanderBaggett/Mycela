use super::*;
use crate::{AnimationCurves, AnimationFlags};
pub(crate) struct AdjustmentCaptureParts<E> {
    pub(crate) count: i32,
    pub(crate) property_type: i32,
    pub(crate) mode: i32,
    pub(crate) guid: [u8; 16],
    pub(crate) bounds: Vec<[f64; 2]>,
    pub(crate) names: Option<SharedSelectionOptions>,
    pub(crate) flags: Option<AnimationFlags>,
    pub(crate) curves: Option<AnimationCurves<E>>,
}
impl<E> Clone for AdjustmentCaptureParts<E> {
    fn clone(&self) -> Self {
        Self {
            count: self.count,
            property_type: self.property_type,
            mode: self.mode,
            guid: self.guid,
            bounds: self.bounds.clone(),
            names: self.names.clone(),
            flags: self.flags.clone(),
            curves: self.curves.clone(),
        }
    }
}
/// Actual retained akf leaves; the detached AdjustmentRecord is not backing state.
pub struct SharedPropertyAdjustment<E = ()>(Arc<Mutex<AdjustmentCaptureParts<E>>>);
impl<E> Clone for SharedPropertyAdjustment<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<E> fmt::Debug for SharedPropertyAdjustment<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SharedPropertyAdjustment({:p})", Arc::as_ptr(&self.0))
    }
}
impl<E> SharedPropertyAdjustment<E> {
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn property_type(&self) -> i32 {
        guard(&self.0).property_type
    }
    pub fn mode(&self) -> i32 {
        guard(&self.0).mode
    }
    pub fn set_mode(&self, value: i32) {
        guard(&self.0).mode = value;
    }
    pub fn guid(&self) -> [u8; 16] {
        guard(&self.0).guid
    }
    pub(in crate::pointwise) fn set_guid(&self, guid: [u8; 16]) {
        guard(&self.0).guid = guid;
    }
    pub fn count(&self) -> i32 {
        guard(&self.0).count
    }
    pub(in crate::pointwise) fn set_property_type(&self, value: i32) {
        guard(&self.0).property_type = value;
    }
    pub fn is_active(&self) -> Result<bool, PointwiseError<E>> {
        let (count, flags) = {
            let s = guard(&self.0);
            (s.count, s.flags.clone())
        };
        let Some(flags) = flags else {
            return Ok(false);
        };
        let mut active = false;
        for i in 0..count {
            active |= flags.value(i).map_err(|_| PointwiseError::Index)?;
        }
        Ok(active)
    }
    pub fn flags(&self) -> AnimationFlags {
        let mut s = guard(&self.0);
        let n = s.count as usize;
        s.flags
            .get_or_insert_with(|| AnimationFlags::from_values(vec![false; n]))
            .clone()
    }
    pub fn names(&self) -> SharedSelectionOptions {
        let mut s = guard(&self.0);
        let n = s.count as usize;
        s.names
            .get_or_insert_with(|| SharedSelectionOptions::new(vec![Some(Vec::new()); n]))
            .clone()
    }
    pub(crate) fn capture_parts(&self) -> AdjustmentCaptureParts<E> {
        guard(&self.0).clone()
    }
    pub(crate) fn from_capture_parts(parts: AdjustmentCaptureParts<E>) -> Self {
        Self(Arc::new(Mutex::new(parts)))
    }
}
impl<E: 'static> SharedPropertyAdjustment<E> {
    pub(in crate::pointwise) fn new(
        property_type: i32,
        count: i32,
        limits: Option<&SharedLimits>,
    ) -> Result<Self, LivePropertyWireError<E>> {
        if count < 0 {
            return Err(LivePropertyWireError::Property(ImageError::Overflow.into()));
        }
        let mut bound = [0.; 2];
        if let Some(l) = limits {
            let l = l.snapshot();
            bound = if let Some(c) = l.curve_range {
                let c = c.copy_wire_value();
                [c.minimum.to_framework_f64(), c.maximum.to_framework_f64()]
            } else if let Some(r) = l.use_range {
                let r = r.copy_wire_value();
                if r.flags[1] {
                    [r.minimum.to_framework_f64(), r.maximum.to_framework_f64()]
                } else {
                    [l.minimum.to_framework_f64(), l.maximum.to_framework_f64()]
                }
            } else {
                return Err(PointwiseError::NullReference.into());
            };
            if bound[0] < -1000. {
                bound[0] = -1000.;
            }
            if bound[1] > 1000. {
                bound[1] = 1000.;
            }
        }
        let mut bounds = Vec::new();
        bounds
            .try_reserve_exact(count as usize)
            .map_err(|_| LivePropertyWireError::Property(ImageError::Allocation.into()))?;
        bounds.resize(count as usize, bound);
        Ok(Self::from_capture_parts(AdjustmentCaptureParts {
            count,
            property_type,
            mode: 1,
            guid: [0; 16],
            bounds,
            names: None,
            flags: None,
            curves: None,
        }))
    }
    pub fn curve(&self, index: i32) -> Result<SharedCurve<E>, PointwiseError<E>> {
        let count = self.count();
        if index < 0 || index >= count {
            return self.default_curve(index);
        }
        let slots = {
            let mut s = guard(&self.0);
            let n = s.count as usize;
            s.curves
                .get_or_insert_with(|| AnimationCurves::from_values(vec![None; n]))
                .clone()
        };
        if let Some(c) = slots.value(index)? {
            return Ok(c);
        }
        let curve = self.default_curve(index)?;
        slots.set(index, Some(curve.clone()))?;
        Ok(curve)
    }
    pub(crate) fn default_curve(&self, index: i32) -> Result<SharedCurve<E>, PointwiseError<E>> {
        let bounds = {
            let s = guard(&self.0);
            *s.bounds
                .get(if s.bounds.len() <= 1 {
                    0
                } else {
                    index as usize
                })
                .ok_or(PointwiseError::Index)?
        };
        let c = SharedCurve::new(bounds[0] as f32, bounds[1] as f32, false);
        c.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        Ok(c)
    }
    pub(in crate::pointwise) fn set_link_name(
        &self,
        axis: i32,
        name: Option<&[u16]>,
    ) -> Result<(), PointwiseError<E>> {
        self.names()
            .set_entry(axis, Some(name.unwrap_or_default().to_vec()))
            .map_err(|_| PointwiseError::Index)?;
        self.flags()
            .set(axis, name.is_some())
            .map_err(|_| PointwiseError::Index)?;
        if name.is_some() {
            self.curve(axis)?;
        } else if let Some(curves) = self.capture_parts().curves {
            curves.set(axis, None)?;
        }
        Ok(())
    }
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), LivePropertyWireError<E>> {
        writer.write_i32(self.property_type())?;
        writer.write_i32(self.mode())?;
        writer.write_bytes(&self.guid())?;
        for i in 0..self.count() {
            let names = self.names().entries();
            let name = names.get(i as usize).ok_or(PointwiseError::Index)?;
            writer.write_optional_utf16(name.as_deref())?;
            writer.write_bool(self.flags().value(i).map_err(|_| PointwiseError::Index)?)?;
            self.curve(i)?.save(writer)?;
        }
        Ok(())
    }
    pub fn load(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), LivePropertyWireError<E>> {
        guard(&self.0).property_type = reader.read_i32()?;
        guard(&self.0).mode = reader.read_i32()?;
        let bytes = reader.read_bytes(16)?;
        let guid = bytes
            .try_into()
            .map_err(|_| LivePropertyWireError::Payload(PropertyRecordError::GuidLength))?;
        guard(&self.0).guid = guid;
        for i in 0..self.count() {
            let names = self.names();
            let name = reader.read_string()?;
            names
                .set_entry(i, Some(name))
                .map_err(|_| PointwiseError::Index)?;
            let flags = self.flags();
            let flag = reader.read_bool()?;
            flags.set(i, flag).map_err(|_| PointwiseError::Index)?;
            self.curve(i)?.load(reader, version)?;
        }
        Ok(())
    }
    pub(crate) fn source_clone(&self) -> Result<Self, PointwiseError<E>> {
        let source = self.capture_parts();
        let mut p = AdjustmentCaptureParts {
            count: source.count,
            property_type: source.property_type,
            mode: source.mode,
            guid: source.guid,
            bounds: vec![[0.; 2]; source.count as usize],
            names: None,
            flags: None,
            curves: None,
        };
        for i in 0..source.count {
            if let Some(flags) = &source.flags {
                let target = p.flags.get_or_insert_with(|| {
                    AnimationFlags::from_values(vec![false; source.count as usize])
                });
                target
                    .set(i, flags.value(i).map_err(|_| PointwiseError::Index)?)
                    .map_err(|_| PointwiseError::Index)?;
            }
            if let Some(names) = &source.names {
                let target = p.names.get_or_insert_with(|| {
                    SharedSelectionOptions::new(vec![Some(Vec::new()); source.count as usize])
                });
                let value = names
                    .entries()
                    .get(i as usize)
                    .cloned()
                    .ok_or(PointwiseError::Index)?;
                target
                    .set_entry(i, value)
                    .map_err(|_| PointwiseError::Index)?;
            }
            p.bounds[i as usize] = *source.bounds.get(i as usize).ok_or(PointwiseError::Index)?;
            // akf.h(int) checks the existing Curve slot, NOT its active flag.
            if let Some(curves) = &source.curves
                && let Some(curve) = curves.value(i)?
            {
                let copy = curve.source_clone()?;
                p.curves
                    .get_or_insert_with(|| {
                        AnimationCurves::from_values(vec![None; source.count as usize])
                    })
                    .set(i, Some(copy))?;
            }
        }
        Ok(Self::from_capture_parts(p))
    }
}
