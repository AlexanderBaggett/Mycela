use crate::reference_adjuster_record::{SharedCurveRangeRecord, SharedRangeOptionsRecord};
use crate::{
    BinaryCursor, BinaryWriter, CurveRangeRecord, DecimalArithmeticError, DecimalBits,
    LimitsRecord, RangeOptionsRecord, RangeRecordError,
};
use std::{
    fmt,
    sync::{Arc, Mutex},
};

/// Scalar backing values and retained nullable child roots of one Limits object.
/// Cloning this snapshot copies scalar bits and shares child handles, not values.
#[derive(Clone, Debug)]
pub struct LimitsValues {
    pub decimal_places: i32,
    pub increment: DecimalBits,
    pub big_increment: DecimalBits,
    pub minimum: DecimalBits,
    pub maximum: DecimalBits,
    pub used: bool,
    pub use_range: Option<SharedRangeOptionsRecord>,
    pub curve_range: Option<SharedCurveRangeRecord>,
}

impl Default for LimitsValues {
    fn default() -> Self {
        let record = LimitsRecord::default();
        Self {
            decimal_places: record.decimal_places,
            increment: record.increment,
            big_increment: record.big_increment,
            minimum: record.minimum,
            maximum: record.maximum,
            used: record.used,
            use_range: record.use_range.map(SharedRangeOptionsRecord::new),
            curve_range: record.curve_range.map(SharedCurveRangeRecord::new),
        }
    }
}

/// Source constructor step: derive BigIncrement with exact 10m multiplication,
/// or retain an explicitly supplied value without performing arithmetic.
#[derive(Clone, Copy, Debug)]
pub enum LimitsStep {
    Derived(DecimalBits),
    Explicit {
        increment: DecimalBits,
        big_increment: DecimalBits,
    },
}

/// Compatibility arithmetic for the derived BigIncrement constructor step.
/// This selects literal 10m FIRST only, not a general Decimal multiply profile.
/// Existing constructors retain the separately characterized Mono behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitsArithmeticProfile {
    MonoCheckedTenFirst,
    /// The pinned Framework64/CLR/OLEAUT32 image, not every Windows runtime.
    Framework64OleAut32TenFirst,
}

/// Resolved constructor arguments across the source's Limits overloads.
/// Optional children are caller-owned handles; a default child is allocated only
/// after derived arithmetic succeeds. This is distinct from raw backing import.
#[derive(Clone, Debug)]
pub struct LimitsDefinition {
    pub decimal_places: i32,
    pub step: LimitsStep,
    pub minimum: DecimalBits,
    pub maximum: DecimalBits,
    pub used: bool,
    pub use_range: Option<SharedRangeOptionsRecord>,
    pub curve_range: Option<SharedCurveRangeRecord>,
}

impl Default for LimitsDefinition {
    fn default() -> Self {
        let record = LimitsRecord::default();
        Self {
            decimal_places: record.decimal_places,
            step: LimitsStep::Derived(record.increment),
            minimum: record.minimum,
            maximum: record.maximum,
            used: record.used,
            use_range: None,
            curve_range: None,
        }
    }
}

/// The source's five direct scalar property setters; they do not adjust children.
#[derive(Clone, Copy, Debug)]
pub enum LimitsEdit {
    DecimalPlaces(i32),
    Increment(DecimalBits),
    BigIncrement(DecimalBits),
    Minimum(DecimalBits),
    Maximum(DecimalBits),
}

/// A retained Limits parent with shared child roots and concrete wire operations.
/// Handle Clone shares this parent; clone_source creates a fresh source parent.
/// Validation, live properties, custom subclasses, source GC and concurrency
/// semantics remain separate capabilities.
pub struct SharedLimits(Arc<Mutex<LimitsValues>>);

impl Clone for SharedLimits {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl fmt::Debug for SharedLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Formatting sinks can reenter: no protected value or guard escapes.
        write!(f, "SharedLimits({:p})", Arc::as_ptr(&self.0))
    }
}

impl Default for SharedLimits {
    fn default() -> Self {
        Self::new(LimitsValues::default())
    }
}

impl SharedLimits {
    /// Apply the Mono-qualified constructor step before parent/default-child
    /// allocation, preserving the existing caller and corpus contract.
    pub fn from_definition(definition: LimitsDefinition) -> Result<Self, DecimalArithmeticError> {
        Self::from_definition_with_profile(definition, LimitsArithmeticProfile::MonoCheckedTenFirst)
    }

    /// Apply the selected literal-ten-FIRST constructor arithmetic before any
    /// parent/default-child allocation. Explicit BigIncrement bypasses arithmetic
    /// in both profiles; the profile is not retained as parallel object state.
    pub fn from_definition_with_profile(
        definition: LimitsDefinition,
        profile: LimitsArithmeticProfile,
    ) -> Result<Self, DecimalArithmeticError> {
        let (increment, big_increment) = match definition.step {
            LimitsStep::Derived(increment) => {
                let big_increment = match profile {
                    LimitsArithmeticProfile::MonoCheckedTenFirst => increment.multiply_by_ten()?,
                    LimitsArithmeticProfile::Framework64OleAut32TenFirst => {
                        increment.framework_ten_times()
                    }
                };
                (increment, big_increment)
            }
            LimitsStep::Explicit {
                increment,
                big_increment,
            } => (increment, big_increment),
        };
        Ok(Self::new(LimitsValues {
            decimal_places: definition.decimal_places,
            increment,
            big_increment,
            minimum: definition.minimum,
            maximum: definition.maximum,
            used: definition.used,
            use_range: definition.use_range,
            curve_range: definition.curve_range,
        }))
    }

    /// Retain exact already-resolved backing state, including a null UseRange.
    /// This import boundary does not invoke the source master constructor.
    pub fn from_backing(values: LimitsValues) -> Self {
        Self(Arc::new(Mutex::new(values)))
    }

    /// Source full master constructor: only null UseRange is normalized.
    /// Explicit BigIncrement and all supplied scalar bits/child roots are retained.
    pub fn new(mut values: LimitsValues) -> Self {
        if values.use_range.is_none() {
            values.use_range = Some(SharedRangeOptionsRecord::new(RangeOptionsRecord::new(
                values.minimum,
                values.maximum,
            )));
        }
        Self::from_backing(values)
    }

    pub fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Copy scalar bits and retain child identities; no child lock is held.
    pub fn snapshot(&self) -> LimitsValues {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Explicit detached wire projection, not source Clone or an atomic capture
    /// of concurrent parent/child changes.
    pub fn copy_wire_value(&self) -> LimitsRecord {
        let values = self.snapshot();
        LimitsRecord {
            decimal_places: values.decimal_places,
            increment: values.increment,
            big_increment: values.big_increment,
            minimum: values.minimum,
            maximum: values.maximum,
            used: values.used,
            use_range: values
                .use_range
                .as_ref()
                .map(|range| range.copy_wire_value()),
            curve_range: values
                .curve_range
                .as_ref()
                .map(|range| range.copy_wire_value()),
        }
    }

    /// Source Clone forwards child roots through a fresh master constructor.
    /// Thus a null backing UseRange becomes a new default-bounds child.
    pub fn clone_source(&self) -> Self {
        Self::new(self.snapshot())
    }

    pub fn edit(&self, edit: LimitsEdit) {
        let mut values = self.0.lock().unwrap_or_else(|error| error.into_inner());
        match edit {
            LimitsEdit::DecimalPlaces(value) => values.decimal_places = value,
            LimitsEdit::Increment(value) => values.increment = value,
            LimitsEdit::BigIncrement(value) => values.big_increment = value,
            LimitsEdit::Minimum(value) => values.minimum = value,
            LimitsEdit::Maximum(value) => values.maximum = value,
        }
    }

    /// Load in source commit order. BigIncrement is not a wire field. True child
    /// presence replaces the root before its first read; false presence retains it.
    pub fn load(&self, reader: &mut BinaryCursor<'_>) -> Result<(), RangeRecordError> {
        self.edit(LimitsEdit::DecimalPlaces(reader.read_i32()?));
        self.edit(LimitsEdit::Increment(reader.read_decimal()?));
        self.edit(LimitsEdit::Minimum(reader.read_decimal()?));
        self.edit(LimitsEdit::Maximum(reader.read_decimal()?));
        let used = reader.read_bool()?;
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .used = used;
        if reader.read_bool()? {
            let range = SharedRangeOptionsRecord::new(RangeOptionsRecord::default());
            let retired = {
                let mut values = self.0.lock().unwrap_or_else(|error| error.into_inner());
                values.use_range.replace(range.clone())
            };
            drop(retired);
            range.load(reader)?;
        }
        if reader.read_bool()? {
            let range = SharedCurveRangeRecord::new(CurveRangeRecord::default());
            let retired = {
                let mut values = self.0.lock().unwrap_or_else(|error| error.into_inner());
                values.curve_range.replace(range.clone())
            };
            drop(retired);
            range.load(reader)?;
        }
        Ok(())
    }

    /// Save concrete retained children outside the parent guard. A null backing
    /// UseRange fails after its presence byte, preserving the source partial write.
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), RangeRecordError> {
        let values = self.snapshot();
        writer.write_i32(values.decimal_places)?;
        writer.write_decimal(values.increment)?;
        writer.write_decimal(values.minimum)?;
        writer.write_decimal(values.maximum)?;
        writer.write_bool(values.used)?;
        writer.write_bool(values.use_range.is_some())?;
        values
            .use_range
            .as_ref()
            .ok_or(RangeRecordError::MissingUseRange)?
            .save(writer)?;
        writer.write_bool(values.curve_range.is_some())?;
        if let Some(range) = &values.curve_range {
            range.save(writer)?;
        }
        Ok(())
    }

    /// Reuse the ordered source value comparison on explicit detached snapshots.
    /// Parent/child identity is not equality; no Eq/PartialEq/Hash is claimed.
    pub fn source_equals(&self, other: Option<&Self>) -> bool {
        let Some(other) = other else {
            return false;
        };
        let left = self.copy_wire_value();
        let right = other.copy_wire_value();
        left.source_equals(Some(&right))
    }
    pub fn save_nullable(&self, writer: Option<&mut BinaryWriter>) -> Result<(), RangeRecordError> {
        match writer {
            Some(w) => self.save(w),
            None => self.save(&mut BinaryWriter::null_receiver()),
        }
    }
    pub fn load_nullable(
        &self,
        reader: Option<&mut BinaryCursor<'_>>,
    ) -> Result<(), RangeRecordError> {
        match reader {
            Some(r) => self.load(r),
            None => self.load(&mut BinaryCursor::null_receiver()),
        }
    }
    pub fn source_equals_object<E>(&self, other: crate::SourceObject<'_, E>) -> bool {
        match other {
            crate::SourceObject::Limits(other) => self.source_equals(Some(other)),
            _ => false,
        }
    }
}
