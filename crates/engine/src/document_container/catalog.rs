use super::*;
use crate::{EnvironmentRecord, ValueAdjusterPayload};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone)]
pub struct EnvironmentCatalogValue(Arc<Mutex<Arc<EnvironmentRecord>>>);

impl EnvironmentCatalogValue {
    pub fn new(map: EnvironmentRecord) -> Self {
        Self::from_snapshot(Arc::new(map))
    }

    pub(super) fn from_snapshot(map: Arc<EnvironmentRecord>) -> Self {
        Self(Arc::new(Mutex::new(map)))
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, Arc<EnvironmentRecord>> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn value(&self) -> Arc<EnvironmentRecord> {
        self.lock().clone()
    }

    pub(super) fn same_reference(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(super) fn lock_order(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
}

/// Shared numeric authority for a catalog entry attached to a graph document.
#[derive(Clone, Debug)]
pub struct NumericCatalogValue(Arc<Mutex<ValueAdjusterPayload>>);

impl NumericCatalogValue {
    pub(super) fn new(payload: ValueAdjusterPayload) -> Self {
        Self(Arc::new(Mutex::new(payload)))
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, ValueAdjusterPayload> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(super) fn lock_order(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }

    /// Read the preset value using the original type's representation.
    pub fn preset_value(&self, kind: GtxPresetKind) -> Option<GtxPresetValue> {
        match (&*self.lock(), kind) {
            (ValueAdjusterPayload::Number(number), GtxPresetKind::Number) => {
                Some(GtxPresetValue::Number(number.value))
            }
            (ValueAdjusterPayload::Number(number), GtxPresetKind::Selection) => {
                Some(GtxPresetValue::Number(f64::from(number.selected)))
            }
            (ValueAdjusterPayload::Dual(dual), GtxPresetKind::Dual) => {
                Some(GtxPresetValue::Dual(dual.value))
            }
            _ => None,
        }
    }
}

/// Removing an entry from the live catalog leaves its wire value available for
/// history travel. The held marker controls whether the document saves it.
#[derive(Default)]
pub(super) struct GraphCatalog(Vec<(GtxCatalogRecord, Arc<AtomicBool>)>);

impl GraphCatalog {
    pub(super) fn into_active(self) -> impl Iterator<Item = GtxCatalogRecord> {
        self.0
            .into_iter()
            .filter_map(|(record, active)| active.load(Ordering::Acquire).then_some(record))
    }

    pub(super) fn new(records: Vec<GtxCatalogRecord>) -> Self {
        Self(
            records
                .into_iter()
                .map(|record| (record.into_live(), Arc::new(AtomicBool::new(true))))
                .collect(),
        )
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = (&GtxCatalogRecord, &Arc<AtomicBool>)> {
        self.0
            .iter()
            .filter(|(_, active)| active.load(Ordering::Acquire))
            .map(|(record, active)| (record, active))
    }

    pub(super) fn active_mut(&mut self) -> Vec<&mut GtxCatalogRecord> {
        self.0
            .iter_mut()
            .filter(|(_, active)| active.load(Ordering::Acquire))
            .map(|(record, _)| record)
            .collect()
    }

    pub(super) fn stage(&mut self, entry: GtxCatalogRecord) -> Arc<AtomicBool> {
        self.0
            .retain(|(_, active)| active.load(Ordering::Acquire) || Arc::strong_count(active) > 1);
        let active = Arc::new(AtomicBool::new(false));
        self.0.push((entry.into_live(), active.clone()));
        active
    }

    pub(super) fn stage_replacement(
        &mut self,
        entry: GtxCatalogRecord,
        replaced: &Arc<AtomicBool>,
    ) -> Arc<AtomicBool> {
        self.0
            .retain(|(_, active)| active.load(Ordering::Acquire) || Arc::strong_count(active) > 1);
        let position = self
            .0
            .iter()
            .position(|(_, active)| Arc::ptr_eq(active, replaced))
            .expect("replacement for a retained catalog entry");
        let active = Arc::new(AtomicBool::new(false));
        self.0
            .insert(position + 1, (entry.into_live(), active.clone()));
        active
    }
}

impl fmt::Debug for EnvironmentCatalogValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EnvironmentCatalogValue({:p})", Arc::as_ptr(&self.0))
    }
}

/// One decoded cp catalog entry. Reference variants retain the existing shared
/// Curve/Gradient roots; this enum does not import a second writable wire copy.
#[derive(Debug)]
pub enum GtxCatalogRecord {
    Value(Box<ValueAdjusterRecord>),
    Reference(Box<ReferenceAdjusterRecord>),
    Numeric {
        metadata: AdjusterMetadataRecord,
        value: NumericCatalogValue,
    },
    Environment {
        metadata: AdjusterMetadataRecord,
        value: EnvironmentCatalogValue,
    },
    Synthesis {
        metadata: AdjusterMetadataRecord,
        value: crate::SynthesisCatalogValue,
    },
}

impl GtxCatalogRecord {
    pub(super) fn color_or_gradient(
        &self,
    ) -> Option<crate::environment_record::EnvironmentLabReference> {
        use crate::environment_record::EnvironmentLabReference;
        match (self.metadata().type_tag, self) {
            (6, Self::Value(record)) => match record.payload {
                ValueAdjusterPayload::Color { value, .. } => {
                    Some(EnvironmentLabReference::Color(value))
                }
                _ => None,
            },
            (7, Self::Reference(record)) => match &record.payload {
                crate::ReferenceAdjusterPayload::Gradient {
                    current: Some(value),
                    ..
                } => Some(EnvironmentLabReference::Gradient(value.clone())),
                _ => None,
            },
            _ => None,
        }
    }

    fn into_live(self) -> Self {
        match self {
            Self::Value(record)
                if matches!(
                    record.payload,
                    ValueAdjusterPayload::Number(_) | ValueAdjusterPayload::Dual(_)
                ) =>
            {
                Self::Numeric {
                    metadata: record.metadata,
                    value: NumericCatalogValue::new(record.payload),
                }
            }
            _ => self,
        }
    }

    /// cp.a(BinaryReader, version): consume Name/GUID/type, construct the exact
    /// concrete receiver, load its body, then publish loaded=true on success.
    /// Unsupported tags stop after the type word, before any payload byte.
    /// On failure the caller's reader retains the actual partial cursor.
    pub fn read(reader: &mut BinaryCursor<'_>, version: f64) -> Result<Self, GtxReadError> {
        let name = reader.read_string()?;
        let guid = reader.read_bytes(16)?;
        let guid = guid
            .try_into()
            .map_err(|_| GtxReadError::Catalog(ValueAdjusterError::Guid))?;
        let tag = reader.read_i32()?;
        if matches!(tag, 9 | 10) {
            let mut metadata = AdjusterMetadataRecord {
                name: Some(name),
                group_id: guid,
                type_tag: tag,
                order: 0,
                loaded: false,
            };
            metadata
                .load(reader, true, version)
                .map_err(GtxReadError::Catalog)?;
            if tag == 10 {
                let value = crate::SynthesisRecord::read(reader, version)
                    .map_err(|error| GtxReadError::Synthesis(Box::new(error)))?;
                metadata.loaded = true;
                return Ok(Self::Synthesis {
                    metadata,
                    value: crate::SynthesisCatalogValue::new(value),
                });
            }
            let value = crate::EnvironmentRecord::read(reader, version)
                .map_err(GtxReadError::Environment)?;
            metadata.loaded = true;
            return Ok(Self::Environment {
                metadata,
                value: EnvironmentCatalogValue::new(value),
            });
        }
        let value_kind = match tag {
            0 => Some(ValueAdjusterKind::Number),
            1 => Some(ValueAdjusterKind::Dual),
            4 => Some(ValueAdjusterKind::Selection),
            6 => Some(ValueAdjusterKind::Color),
            14 => Some(ValueAdjusterKind::Vector),
            _ => None,
        };
        if let Some(kind) = value_kind {
            let mut value = ValueAdjusterRecord::new(kind, Some(name), guid);
            value
                .load(reader, true, version)
                .map_err(GtxReadError::Catalog)?;
            value.metadata.loaded = true;
            return Ok(Self::Value(Box::new(value)));
        }
        let kind = match tag {
            7 => ReferenceAdjusterKind::Gradient,
            8 => ReferenceAdjusterKind::Curve,
            _ => {
                return Err(GtxReadError::Unsupported(UnsupportedGtx::CatalogType {
                    tag,
                    name,
                }));
            }
        };
        let mut value = ReferenceAdjusterRecord::new(kind, Some(name), guid);
        value
            .load(reader, true, version)
            .map_err(GtxReadError::ReferenceCatalog)?;
        value.metadata.loaded = true;
        Ok(Self::Reference(Box::new(value)))
    }

    pub fn metadata(&self) -> &AdjusterMetadataRecord {
        match self {
            Self::Value(value) => &value.metadata,
            Self::Reference(value) => &value.metadata,
            Self::Numeric { metadata, .. }
            | Self::Environment { metadata, .. }
            | Self::Synthesis { metadata, .. } => metadata,
        }
    }

    pub fn as_value(&self) -> Option<&ValueAdjusterRecord> {
        match self {
            Self::Value(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_value_mut(&mut self) -> Option<&mut ValueAdjusterRecord> {
        match self {
            Self::Value(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_reference(&self) -> Option<&ReferenceAdjusterRecord> {
        match self {
            Self::Reference(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_reference_mut(&mut self) -> Option<&mut ReferenceAdjusterRecord> {
        match self {
            Self::Reference(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_environment(&self) -> Option<Arc<EnvironmentRecord>> {
        match self {
            Self::Environment { value, .. } => Some(value.value()),
            _ => None,
        }
    }

    pub fn as_synthesis(&self) -> Option<crate::SynthesisRecord> {
        match self {
            Self::Synthesis { value, .. } => Some(value.record()),
            _ => None,
        }
    }

    /// Replace a detached catalog value. Live graph edits should go through
    /// the document so connected properties and history are updated together.
    pub fn replace_environment(&mut self, map: EnvironmentRecord) -> bool {
        let Self::Environment { value, .. } = self else {
            return false;
        };
        *value.lock() = Arc::new(map);
        true
    }

    /// Save the retained concrete receiver. Editing its raw header tag does not
    /// select another payload class. Reference lazy-save effects remain owned
    /// by ReferenceAdjusterRecord and are visible through caller-held handles.
    pub fn save(&mut self, writer: &mut BinaryWriter) -> Result<(), GtxReadError> {
        match self {
            Self::Value(value) => value.save(writer).map_err(GtxReadError::Catalog),
            Self::Reference(value) => value.save(writer).map_err(GtxReadError::ReferenceCatalog),
            Self::Numeric { metadata, value } => {
                metadata.save(writer).map_err(GtxReadError::Catalog)?;
                value.lock().save(writer).map_err(GtxReadError::Catalog)
            }
            Self::Synthesis { metadata, value } => {
                metadata.save(writer).map_err(GtxReadError::Catalog)?;
                value
                    .record()
                    .write(writer)
                    .map_err(|error| GtxReadError::Synthesis(Box::new(error)))
            }
            Self::Environment { metadata, value } => {
                metadata.save(writer).map_err(GtxReadError::Catalog)?;
                value
                    .value()
                    .save(writer)
                    .map_err(GtxReadError::Environment)
            }
        }
    }
}
