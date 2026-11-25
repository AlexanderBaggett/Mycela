use crate::{
    AdjusterMetadataRecord, BinaryCursor, BinaryWriter, CurveRangeRecord, CurveRecord,
    CurveRecordError, GradientRecord, PropertyRecordError, RangeOptionsRecord, RangeRecordError,
    ValueAdjusterError,
};
use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

/// A shared mutable wire root. Cloning this handle preserves identity; it does
/// not implement the source value's Clone operation. No callbacks or access
/// guards are exposed while the private value lock is held.
pub struct SharedReference<T>(Arc<Mutex<T>>);
impl<T> fmt::Debug for SharedReference<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Formatting sinks may reenter this root. Never format T under its lock.
        write!(f, "SharedReference({:p})", Arc::as_ptr(&self.0))
    }
}
impl<T> Clone for SharedReference<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<T> SharedReference<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(value)))
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
#[derive(Clone, Debug)]
pub struct SharedCurveRecord(SharedReference<Arc<CurveRecord>>);
/// Compatibility wire-import seam backed by the same live Gradient authority.
#[derive(Clone, Debug)]
pub struct SharedGradientRecord(crate::SharedGradient);
impl SharedGradientRecord {
    /// Detached wire import, not an original Gradient constructor.
    pub fn new(record: GradientRecord) -> Self {
        Self(crate::SharedGradient::from_wire_record(record))
    }
    pub fn from_live(value: crate::SharedGradient) -> Self {
        Self(value)
    }
    pub fn live(&self) -> crate::SharedGradient {
        self.0.clone()
    }
    pub fn same_reference(&self, other: &Self) -> bool {
        self.0.same_reference(&other.0)
    }
    pub fn load(&self, reader: &mut BinaryCursor<'_>) -> Result<(), ReferenceAdjusterError> {
        self.0
            .load(reader)
            .map_err(ReferenceAdjusterError::LiveGradient)
    }
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), ReferenceAdjusterError> {
        self.0
            .save(writer)
            .map_err(ReferenceAdjusterError::LiveGradient)
    }
}
pub type SharedRangeOptionsRecord = SharedReference<RangeOptionsRecord>;
pub type SharedCurveRangeRecord = SharedReference<CurveRangeRecord>;

impl SharedReference<RangeOptionsRecord> {
    /// Actual Controls.ad4(decimal, decimal, double, bool, bool) constructor.
    /// Its final argument is g/use-range (flag1); h/display is independent.
    pub fn from_range_parameters(
        minimum: crate::DecimalBits,
        maximum: crate::DecimalBits,
        multiplier: f64,
        display: bool,
        use_range: bool,
    ) -> Self {
        Self::new(RangeOptionsRecord {
            minimum,
            maximum,
            decimal_places: 3,
            multiplier,
            flags: [false, use_range, display],
        })
    }

    /// Detach wire fields without changing this root or claiming source Clone.
    pub fn copy_wire_value(&self) -> RangeOptionsRecord {
        let value = self.0.lock().unwrap_or_else(|error| error.into_inner());
        RangeOptionsRecord {
            minimum: value.minimum,
            maximum: value.maximum,
            decimal_places: value.decimal_places,
            multiplier: value.multiplier,
            flags: value.flags,
        }
    }

    /// Mutate this retained child in source field order, including partial loads.
    pub fn load(&self, reader: &mut BinaryCursor<'_>) -> Result<(), RangeRecordError> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .load(reader)
    }

    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), RangeRecordError> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .save(writer)
    }
}

impl SharedReference<CurveRangeRecord> {
    pub fn save_nullable(&self, writer: Option<&mut BinaryWriter>) -> Result<(), RangeRecordError> {
        match writer {
            Some(w) => self.save(w),
            None => self.save(&mut BinaryWriter::null_receiver()),
        }
    }
    pub fn source_equals_object<E>(&self, other: crate::SourceObject<'_, E>) -> bool {
        match other {
            crate::SourceObject::CurveRange(other) => self
                .copy_wire_value()
                .source_equals(Some(&other.copy_wire_value())),
            _ => false,
        }
    }

    /// Construct the actual retained CurveRange from public bounds/wrap inputs.
    pub fn from_bounds(
        minimum: crate::DecimalBits,
        maximum: crate::DecimalBits,
        wrap_y: bool,
    ) -> Self {
        Self::new(CurveRangeRecord {
            minimum,
            maximum,
            wrap_y,
        })
    }

    /// Detach wire fields without changing this root or claiming source Clone.
    pub fn copy_wire_value(&self) -> CurveRangeRecord {
        let value = self.0.lock().unwrap_or_else(|error| error.into_inner());
        CurveRangeRecord {
            minimum: value.minimum,
            maximum: value.maximum,
            wrap_y: value.wrap_y,
        }
    }

    /// Mutate this retained child in source field order, including partial loads.
    pub fn load(&self, reader: &mut BinaryCursor<'_>) -> Result<(), RangeRecordError> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .load(reader)
    }

    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), RangeRecordError> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .save(writer)
    }
}

impl SharedCurveRecord {
    pub fn new(value: CurveRecord) -> Self {
        Self(SharedReference::new(Arc::new(value)))
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        self.0.same_reference(&other.0)
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Arc<CurveRecord>> {
        self.0.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn lock_order(&self) -> usize {
        Arc::as_ptr(&self.0.0) as usize
    }

    pub(crate) fn snapshot(&self) -> Arc<CurveRecord> {
        self.lock().clone()
    }

    pub(crate) fn detached_copy(&self) -> Self {
        Self(SharedReference::new(self.snapshot()))
    }

    pub fn load(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), ReferenceAdjusterError> {
        Arc::make_mut(&mut self.lock()).load(reader, version)?;
        Ok(())
    }

    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), ReferenceAdjusterError> {
        self.snapshot().save(writer)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceAdjusterKind {
    Gradient,
    Curve,
}
impl ReferenceAdjusterKind {
    pub const fn tag(self) -> i32 {
        match self {
            Self::Gradient => 7,
            Self::Curve => 8,
        }
    }
}
/// Typed nullable backing references; raw metadata tags do not change this kind.
#[derive(Debug)]
pub enum ReferenceAdjusterPayload {
    Gradient {
        current: Option<SharedGradientRecord>,
        previous: Option<SharedGradientRecord>,
    },
    Curve {
        current: Option<SharedCurveRecord>,
        previous: Option<SharedCurveRecord>,
    },
}
#[derive(Clone, Debug)]
pub enum ReferenceValueHandle {
    Gradient(SharedGradientRecord),
    Curve(SharedCurveRecord),
}
impl ReferenceValueHandle {
    pub fn same_reference(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Gradient(a), Self::Gradient(b)) => a.same_reference(b),
            (Self::Curve(a), Self::Curve(b)) => a.same_reference(b),
            _ => false,
        }
    }
    pub fn load(
        &self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), ReferenceAdjusterError> {
        match self {
            Self::Gradient(value) => value.load(reader),
            Self::Curve(value) => value.load(reader, version),
        }
    }
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), ReferenceAdjusterError> {
        match self {
            Self::Gradient(value) => value.save(writer),
            Self::Curve(value) => value.save(writer),
        }
    }
}

/// Detached reference-valued adjuster wire state. Root aliasing is preserved;
/// internal object graphs, setters, cloning, callbacks and evaluation remain
/// separate capabilities. Complete catalog dispatch is not supplied by this DTO.
#[derive(Debug)]
pub struct ReferenceAdjusterRecord {
    pub metadata: AdjusterMetadataRecord,
    pub payload: ReferenceAdjusterPayload,
}
impl ReferenceAdjusterRecord {
    pub fn new(kind: ReferenceAdjusterKind, name: Option<Vec<u16>>, group_id: [u8; 16]) -> Self {
        Self {
            metadata: AdjusterMetadataRecord {
                name,
                group_id,
                type_tag: kind.tag(),
                order: 0,
                loaded: false,
            },
            payload: match kind {
                ReferenceAdjusterKind::Gradient => ReferenceAdjusterPayload::Gradient {
                    current: None,
                    previous: None,
                },
                ReferenceAdjusterKind::Curve => ReferenceAdjusterPayload::Curve {
                    current: None,
                    previous: None,
                },
            },
        }
    }
    /// Source Value getter: a null current creates a default and rebinds both
    /// slots. A non-null current leaves a separate previous unchanged.
    pub fn value(&mut self) -> ReferenceValueHandle {
        match &mut self.payload {
            ReferenceAdjusterPayload::Gradient { current, previous } => {
                if let Some(value) = current {
                    return ReferenceValueHandle::Gradient(value.clone());
                }
                let value = SharedGradientRecord::from_live(crate::SharedGradient::new());
                *current = Some(value.clone());
                *previous = Some(value.clone());
                ReferenceValueHandle::Gradient(value)
            }
            ReferenceAdjusterPayload::Curve { current, previous } => {
                if let Some(value) = current {
                    return ReferenceValueHandle::Curve(value.clone());
                }
                let value = SharedCurveRecord::new(CurveRecord::new(0.0, 100.0, false));
                *current = Some(value.clone());
                *previous = Some(value.clone());
                ReferenceValueHandle::Curve(value)
            }
        }
    }
    pub fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        identity_consumed: bool,
        version: f64,
    ) -> Result<(), ReferenceAdjusterError> {
        self.metadata.load(reader, identity_consumed, version)?;
        let value = self.value();
        value.load(reader, version)?;
        // The value lock has been released before reference-slot mutation.
        match (&mut self.payload, value) {
            (
                ReferenceAdjusterPayload::Gradient { previous, .. },
                ReferenceValueHandle::Gradient(value),
            ) => *previous = Some(value),
            (
                ReferenceAdjusterPayload::Curve { previous, .. },
                ReferenceValueHandle::Curve(value),
            ) => *previous = Some(value),
            _ => unreachable!("value getter preserves concrete payload kind"),
        }
        Ok(())
    }
    pub fn save(&mut self, writer: &mut BinaryWriter) -> Result<(), ReferenceAdjusterError> {
        self.metadata.save(writer)?;
        self.value().save(writer)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceAdjusterError {
    Metadata(ValueAdjusterError),
    Curve(CurveRecordError),
    Gradient(PropertyRecordError),
    LiveGradient(crate::GradientError),
}
impl ReferenceAdjusterError {
    pub const fn source_class(self) -> &'static str {
        match self {
            Self::Metadata(e) => e.source_class(),
            Self::Curve(e) => e.source_class(),
            Self::Gradient(e) => e.source_class(),
            Self::LiveGradient(e) => e.source_class(),
        }
    }
}
impl From<ValueAdjusterError> for ReferenceAdjusterError {
    fn from(e: ValueAdjusterError) -> Self {
        Self::Metadata(e)
    }
}
impl From<CurveRecordError> for ReferenceAdjusterError {
    fn from(e: CurveRecordError) -> Self {
        Self::Curve(e)
    }
}
impl From<PropertyRecordError> for ReferenceAdjusterError {
    fn from(e: PropertyRecordError) -> Self {
        Self::Gradient(e)
    }
}
impl fmt::Display for ReferenceAdjusterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Metadata(e) => e.fmt(f),
            Self::Curve(e) => e.fmt(f),
            Self::Gradient(e) => e.fmt(f),
            Self::LiveGradient(e) => e.fmt(f),
        }
    }
}
impl Error for ReferenceAdjusterError {}

impl SharedReference<crate::MatrixWireRecord> {
    pub fn apply_matrix(&self, operation: crate::MatrixOperation) {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .apply(operation);
    }
    pub fn copy_matrix_value(&self) -> Self {
        let value = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .copy_value();
        Self::new(value)
    }
    pub fn float_rotate_matrix(&self, angle: f64) -> Self {
        let value = self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .float_rotate_at_offsets(angle);
        Self::new(value)
    }
    pub fn matrix_elements(&self) -> [f64; 6] {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .elements
    }
    pub fn save_matrix(&self, writer: &mut BinaryWriter) -> Result<(), crate::BinaryError> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .save(writer)
    }
}
