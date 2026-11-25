//! gg.g / atw: actual silent raw fields, readonly constructor os and shared aoi stream.
use super::*;
use crate::{HistoryDirection, HistoryError, PointwiseNode, history::HistoryGraphEdit};

type RegionFields = (i32, f32, f32, f32);
pub(super) struct RegionState<E> {
    fields: Mutex<RegionFields>,
    owner: Mutex<Option<Arc<MatrixBinding<E>>>>,
    constructor_pending: Mutex<bool>,
}
impl<E> RegionState<E> {
    pub(super) fn copied(fields: (i32, f32, f32, f32)) -> Self {
        Self {
            fields: Mutex::new(fields),
            owner: Mutex::new(None),
            constructor_pending: Mutex::new(false),
        }
    }
}
/// Real atw object. Raw field setters are silent; aoi copy/notify uses its existing multicast authority.
pub struct PointwiseRegion<E = ()> {
    object: PointwiseCustomObject<E>,
}
impl<E> Clone for PointwiseRegion<E> {
    fn clone(&self) -> Self {
        Self {
            object: self.object.clone(),
        }
    }
}
impl<E> fmt::Debug for PointwiseRegion<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PointwiseRegion({:?})", self.object)
    }
}
impl<E> PointwiseCustomObject<E> {
    /// Actual atw(null) defaults, with a null readonly constructor owner.
    pub fn new_region() -> Self {
        Self::from_payload(Payload::Region(RegionState::copied((0, 0.1, 0.1, 0.8))))
    }
    pub(in crate::pointwise) fn new_region_for_property() -> Self {
        let state = RegionState::copied((0, 0.1, 0.1, 0.8));
        *guard(&state.constructor_pending) = true;
        Self::from_payload(Payload::Region(state))
    }
    pub fn region(&self) -> Result<PointwiseRegion<E>, PointwiseError<E>> {
        if matches!(&self.body.payload, Payload::Region(_)) {
            Ok(PointwiseRegion {
                object: self.clone(),
            })
        } else {
            Err(PointwiseError::InvalidCast)
        }
    }
    pub(in crate::pointwise) fn captured_region(fields: (i32, f32, f32, f32)) -> Self {
        Self::from_payload(Payload::Region(RegionState::copied(fields)))
    }
    pub(in crate::pointwise) fn region_binding(&self) -> Option<Arc<MatrixBinding<E>>> {
        match &self.body.payload {
            Payload::Region(r) => guard(&r.owner).clone(),
            _ => None,
        }
    }
    pub(in crate::pointwise) fn initialize_region_owner(&self, binding: &Arc<MatrixBinding<E>>) {
        if let Payload::Region(r) = &self.body.payload {
            let mut pending = guard(&r.constructor_pending);
            if *pending {
                *guard(&r.owner) = Some(Arc::new(MatrixBinding {
                    id: NEXT.fetch_add(1, Ordering::Relaxed),
                    node: binding.node.clone(),
                    parameter: binding.parameter,
                }));
                *pending = false;
            }
        }
    }
    pub(in crate::pointwise) fn install_region_owner(
        &self,
        binding: Option<Arc<MatrixBinding<E>>>,
    ) {
        if let Payload::Region(r) = &self.body.payload {
            *guard(&r.owner) = binding;
        }
    }
}
impl<E> PointwiseRegion<E> {
    fn state(&self) -> &RegionState<E> {
        match &self.object.body.payload {
            Payload::Region(r) => r,
            _ => unreachable!(),
        }
    }
    pub fn fields(&self) -> (i32, f32, f32, f32) {
        *guard(&self.state().fields)
    }
    pub fn mode(&self) -> i32 {
        self.fields().0
    }
    pub fn x(&self) -> f32 {
        self.fields().1
    }
    pub fn y(&self) -> f32 {
        self.fields().2
    }
    pub fn size(&self) -> f32 {
        self.fields().3
    }
    pub fn set_mode(&self, value: i32) {
        guard(&self.state().fields).0 = value;
    }
    pub fn set_x(&self, value: f32) {
        guard(&self.state().fields).1 = value;
    }
    pub fn set_y(&self, value: f32) {
        guard(&self.state().fields).2 = value;
    }
    pub fn set_size(&self, value: f32) {
        guard(&self.state().fields).3 = value;
    }
    pub fn aspect(&self) -> f32 {
        Self::aspect_for_mode(self.mode())
    }
    pub fn aspect_for_mode(mode: i32) -> f32 {
        if mode < 2 {
            1.
        } else {
            let aspect = (1.0 / libm::sin(std::f64::consts::PI / 3.)) as f32;
            match mode {
                2 => aspect,
                3 => aspect / 2.,
                _ => 0.1,
            }
        }
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.object.same_reference(&other.object)
    }
    pub fn source_equals(&self, other: &Self) -> bool {
        let a = self.fields();
        let b = other.fields();
        a.0 == b.0
            && (f64::from(a.1) - f64::from(b.1)).abs() < 1E-5
            && (f64::from(a.2) - f64::from(b.2)).abs() < 1E-5
            && (f64::from(a.3) - f64::from(b.3)).abs() < 1E-5
    }
    pub fn custom_object(&self) -> PointwiseCustomObject<E> {
        self.object.clone()
    }
    pub(in crate::pointwise) fn capture_owner(&self) -> Option<(PointwiseNode<E>, usize)> {
        let binding = self.object.region_binding()?;
        let core = binding.node.upgrade()?;
        Some((PointwiseNode::from_core(&core)?, binding.parameter))
    }
    pub fn constructor_owner(&self) -> Result<PointwiseParameter<E>, PointwiseError<E>> {
        let (node, parameter) = self.capture_owner().ok_or(PointwiseError::NullReference)?;
        node.filter()
            .ok_or(PointwiseError::InvalidCast)?
            .parameter(parameter as i32)
    }
    pub fn copy_fields_from(&self, source: &Self) {
        let a = source.fields();
        self.set_mode(a.0);
        self.set_x(a.1);
        self.set_y(a.2);
        self.set_size(a.3);
    }
    pub fn save(&self, writer: &mut crate::BinaryWriter) -> Result<(), crate::BinaryError> {
        writer.write_i32(self.mode())?;
        writer.write_f32(self.x())?;
        writer.write_f32(self.y())?;
        writer.write_f32(self.size())
    }
    pub fn load(
        &self,
        reader: &mut crate::BinaryCursor<'_>,
        _encoding: f64,
    ) -> Result<(), crate::BinaryError> {
        self.set_mode(reader.read_i32()?);
        self.set_x(reader.read_f32()?);
        self.set_y(reader.read_f32()?);
        self.set_size(reader.read_f32()?);
        Ok(())
    }
    pub fn notify(&self) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.object.notify()
    }
    pub fn copy_and_notify(&self, source: Option<&Self>) -> Result<(), PointwiseError<E>>
    where
        E: 'static,
    {
        self.object.copy_and_notify(source.map(|s| &s.object))
    }
}
impl<E: 'static> PointwiseParameter<E> {
    pub fn region(&self) -> Result<PointwiseRegion<E>, PointwiseError<E>> {
        self.custom_object()?
            .ok_or(PointwiseError::NullReference)?
            .region()
    }
    pub fn previous_region(&self) -> Result<PointwiseRegion<E>, PointwiseError<E>> {
        self.previous_custom_object()?
            .ok_or(PointwiseError::NullReference)?
            .region()
    }

    /// Publish one Region edit, retaining the original property object through history.
    pub fn edit_region(
        &self,
        region: &PointwiseRegion<E>,
        before: RegionFields,
        after: RegionFields,
    ) -> Result<(), PointwiseError<E>> {
        if ![after.1, after.2, after.3].into_iter().all(f32::is_finite)
            || !self.region()?.same_reference(region)
        {
            return Err(ImageError::Argument.into());
        }
        if !apply_region(region, before, after)? {
            return Ok(());
        }
        if let Some(slot) = self
            .owner()
            .node()
            .texture_node_logic()
            .and_then(|logic| logic.slot())
            && let Some(history) = slot.history()
        {
            let sequence = history.edit_sequence();
            if let Err(error) = history.record_graph_edit(
                slot.history_target(),
                self.name(),
                Box::new(RegionEdit {
                    property: self.downgrade(),
                    object: region.object.raw(),
                    before,
                    after,
                }),
            ) {
                if history.edit_sequence() == sequence {
                    let _ = apply_region(region, after, before);
                }
                return Err(PointwiseError::History(Box::new(error)));
            }
        }
        Ok(())
    }
}

fn apply_region<E: 'static>(
    region: &PointwiseRegion<E>,
    before: RegionFields,
    after: RegionFields,
) -> Result<bool, PointwiseError<E>> {
    let mut fields = guard(&region.state().fields);
    if region_field_bits(*fields) != region_field_bits(before) {
        return Err(ImageError::Argument.into());
    }
    if region_field_bits(before) == region_field_bits(after) {
        return Ok(false);
    }
    *fields = after;
    drop(fields);
    if let Err(error) = region.notify() {
        let mut fields = guard(&region.state().fields);
        if region_field_bits(*fields) == region_field_bits(after) {
            *fields = before;
            drop(fields);
            let _ = region.notify();
        }
        return Err(error);
    }
    Ok(true)
}

fn region_field_bits(fields: RegionFields) -> (i32, u32, u32, u32) {
    (
        fields.0,
        fields.1.to_bits(),
        fields.2.to_bits(),
        fields.3.to_bits(),
    )
}

struct RegionEdit<E> {
    property: super::super::super::WeakPointwiseParameter<E>,
    object: PointwiseCustomObject<E>,
    before: RegionFields,
    after: RegionFields,
}

impl<E: 'static> HistoryGraphEdit<E> for RegionEdit<E> {
    fn apply(&self, direction: HistoryDirection) -> Result<(), HistoryError<E>> {
        let property = self.property.upgrade().ok_or(HistoryError::NullTarget)?;
        let region = property
            .region()
            .map_err(|error| HistoryError::Property(Box::new(error)))?;
        if !region.object.same_reference(&self.object) {
            return Err(HistoryError::NullTarget);
        }
        let (before, after) = match direction {
            HistoryDirection::Undo => (self.after, self.before),
            HistoryDirection::Redo => (self.before, self.after),
        };
        apply_region(&region, before, after)
            .map(|_| ())
            .map_err(|error| HistoryError::Property(Box::new(error)))
    }
}
