use super::*;
use crate::{
    CurveRecord, EnvironmentRecord, EnvironmentWireError, ReferenceValueHandle, SharedCurveRecord,
    SharedGradient, SharedGradientRecord,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Concrete im preset factory classes, independent of an existing receiver's
/// mutable raw Type word. No catalog accuracy, Limits or options are on this wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GtxPresetKind {
    Number,
    Dual,
    Selection,
    Color,
    Gradient,
    Curve,
    Environment,
    Synthesis,
    Vector,
}
impl GtxPresetKind {
    pub const fn tag(self) -> i32 {
        match self {
            Self::Number => 0,
            Self::Dual => 1,
            Self::Selection => 4,
            Self::Color => 6,
            Self::Gradient => 7,
            Self::Curve => 8,
            Self::Environment => 9,
            Self::Synthesis => 10,
            Self::Vector => 14,
        }
    }
    pub(super) fn from_tag(tag: i32) -> Option<Self> {
        match tag {
            0 => Some(Self::Number),
            1 => Some(Self::Dual),
            4 => Some(Self::Selection),
            6 => Some(Self::Color),
            7 => Some(Self::Gradient),
            8 => Some(Self::Curve),
            9 => Some(Self::Environment),
            10 => Some(Self::Synthesis),
            14 => Some(Self::Vector),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum GtxPresetValue {
    Number(f64),
    Dual([f64; 2]),
    Color([f64; 3]),
    Vector([f64; 3]),
    Reference(ReferenceValueHandle),
    Environment(Box<EnvironmentRecord>),
    Synthesis(crate::SynthesisRecord),
}

/// Owned preset wire receiver. Reference values retain the actual shared leaf
/// root. This is not the live manager's preset-object/Clone/randomization API.
#[derive(Clone, Debug)]
pub struct GtxPresetRecord {
    name: Option<Vec<u16>>,
    group_id: [u8; 16],
    type_tag: i32,
    kind: GtxPresetKind,
    hidden: bool,
    value: GtxPresetValue,
}
impl GtxPresetRecord {
    fn copy_with<E>(
        &self,
        copy_reference: &mut impl FnMut(&ReferenceValueHandle) -> Result<ReferenceValueHandle, E>,
    ) -> Result<Self, E> {
        let value = match &self.value {
            GtxPresetValue::Number(value) => GtxPresetValue::Number(*value),
            GtxPresetValue::Dual(value) => GtxPresetValue::Dual(*value),
            GtxPresetValue::Color(value) => GtxPresetValue::Color(*value),
            GtxPresetValue::Vector(value) => GtxPresetValue::Vector(*value),
            GtxPresetValue::Reference(value) => GtxPresetValue::Reference(copy_reference(value)?),
            GtxPresetValue::Environment(value) => GtxPresetValue::Environment(value.clone()),
            GtxPresetValue::Synthesis(value) => GtxPresetValue::Synthesis(value.clone()),
        };
        Ok(Self {
            name: self.name.clone(),
            group_id: self.group_id,
            type_tag: self.type_tag,
            kind: self.kind,
            hidden: self.hidden,
            value,
        })
    }
    /// im.a(string, Guid, o1), whose valid type set differs from cp.a: tag12
    /// has no preset constructor. Valid complex types stop at a target boundary;
    /// other raw tags follow im's actual ArgumentException path. A null name is
    /// retained by a supported constructor and fails only if Save writes it.
    pub fn from_type_tag(
        tag: i32,
        name: Option<Vec<u16>>,
        group_id: [u8; 16],
    ) -> Result<Self, GtxPresetError> {
        if let Some(kind) = GtxPresetKind::from_tag(tag) {
            return Ok(Self::new(kind, name, group_id));
        }
        match tag {
            13 | 15 => Err(GtxPresetError::UnsupportedType { tag, name }),
            _ => Err(GtxPresetError::InvalidType { tag }),
        }
    }

    pub fn new(kind: GtxPresetKind, name: Option<Vec<u16>>, group_id: [u8; 16]) -> Self {
        let value = match kind {
            GtxPresetKind::Number | GtxPresetKind::Selection => GtxPresetValue::Number(-1.0),
            GtxPresetKind::Dual => GtxPresetValue::Dual([0.0; 2]),
            GtxPresetKind::Color => GtxPresetValue::Color([0.0; 3]),
            GtxPresetKind::Vector => GtxPresetValue::Vector([0.0; 3]),
            GtxPresetKind::Gradient => GtxPresetValue::Reference(ReferenceValueHandle::Gradient(
                SharedGradientRecord::from_live(SharedGradient::new()),
            )),
            GtxPresetKind::Curve => GtxPresetValue::Reference(ReferenceValueHandle::Curve(
                SharedCurveRecord::new(CurveRecord::new(0.0, 100.0, false)),
            )),
            GtxPresetKind::Synthesis => {
                GtxPresetValue::Synthesis(crate::SynthesisRecord::default())
            }
            GtxPresetKind::Environment => {
                GtxPresetValue::Environment(Box::<EnvironmentRecord>::default())
            }
        };
        Self {
            name,
            group_id,
            type_tag: kind.tag(),
            kind,
            hidden: false,
            value,
        }
    }

    /// u9's factory header is Name, Type, GUID, unlike cp's Name, GUID, Type.
    /// An unported factory stops after the complete header, before its payload.
    pub fn read(reader: &mut BinaryCursor<'_>, version: f64) -> Result<Self, GtxPresetError> {
        let name = reader.read_string()?;
        let tag = reader.read_i32()?;
        let group_id = read_guid(reader)?;
        let mut value = Self::from_type_tag(tag, Some(name), group_id)?;
        value.load(reader, true, version)?;
        Ok(value)
    }

    /// An existing receiver keeps its concrete class and Hide state. Metadata
    /// commits sequentially. Reference loads mutate the same retained object.
    pub fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        identity_consumed: bool,
        version: f64,
    ) -> Result<(), GtxPresetError> {
        if !identity_consumed {
            self.name = Some(reader.read_string()?);
            self.type_tag = reader.read_i32()?;
            self.group_id = read_guid(reader)?;
        }
        match &mut self.value {
            GtxPresetValue::Number(value) => *value = reader.read_f64()?,
            GtxPresetValue::Dual(value) => *value = [reader.read_f64()?, reader.read_f64()?],
            GtxPresetValue::Color(value) => {
                // dq mutates its rr field channel by channel, unlike mx/uo.
                for channel in value {
                    *channel = reader.read_f64()?;
                }
            }
            GtxPresetValue::Vector(value) => {
                // AdjusterPreset<ae4>.fn loads a boxed value and discards it;
                // uo then reads another triplet and commits only when complete.
                for _ in 0..3 {
                    reader.read_f64()?;
                }
                *value = [reader.read_f64()?, reader.read_f64()?, reader.read_f64()?];
            }
            GtxPresetValue::Reference(value) => value
                .load(reader, version)
                .map_err(GtxPresetError::Reference)?,
            GtxPresetValue::Synthesis(value) => {
                *value = crate::SynthesisRecord::read(reader, version)
                    .map_err(|error| GtxPresetError::Synthesis(Box::new(error)))?;
            }
            GtxPresetValue::Environment(value) => value
                .load(reader, version)
                .map_err(GtxPresetError::Environment)?,
        }
        Ok(())
    }

    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), GtxPresetError> {
        writer.write_optional_utf16(self.name.as_deref())?;
        writer.write_i32(self.type_tag)?;
        writer.write_bytes(&self.group_id)?;
        match &self.value {
            GtxPresetValue::Number(value) => writer.write_f64(*value)?,
            GtxPresetValue::Dual(value) => {
                for v in value {
                    writer.write_f64(*v)?;
                }
            }
            GtxPresetValue::Color(value) => {
                for v in value {
                    writer.write_f64(*v)?;
                }
            }
            GtxPresetValue::Vector(value) => {
                for _ in 0..2 {
                    for v in value {
                        writer.write_f64(*v)?;
                    }
                }
            }
            GtxPresetValue::Reference(value) => {
                value.save(writer).map_err(GtxPresetError::Reference)?
            }
            GtxPresetValue::Synthesis(value) => value
                .write(writer)
                .map_err(|error| GtxPresetError::Synthesis(Box::new(error)))?,
            GtxPresetValue::Environment(value) => {
                value.save(writer).map_err(GtxPresetError::Environment)?
            }
        }
        Ok(())
    }
    pub fn name(&self) -> Option<&[u16]> {
        self.name.as_deref()
    }
    pub fn group_id(&self) -> [u8; 16] {
        self.group_id
    }
    pub fn type_tag(&self) -> i32 {
        self.type_tag
    }
    pub fn kind(&self) -> GtxPresetKind {
        self.kind
    }
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }
    pub fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
    }
    pub fn set_group_id(&mut self, id: [u8; 16]) {
        self.group_id = id;
    }
    pub fn value(&self) -> &GtxPresetValue {
        &self.value
    }
    pub(super) fn with_value(mut self, value: GtxPresetValue) -> Self {
        debug_assert_eq!(
            std::mem::discriminant(&self.value),
            std::mem::discriminant(&value)
        );
        self.value = value;
        self
    }
    pub fn as_environment_mut(&mut self) -> Option<&mut EnvironmentRecord> {
        match &mut self.value {
            GtxPresetValue::Environment(value) => Some(value),
            _ => None,
        }
    }
}

/// u9's retained owned wire fields. Load clears presets only after Name/GUID;
/// an incomplete child is never appended. Caller-held leaf roots stay alive.
#[derive(Debug)]
pub struct GtxPresetGroup {
    name: Option<Vec<u16>>,
    group_id: [u8; 16],
    hidden: bool,
    present: Arc<AtomicBool>,
    presets: Vec<GtxPresetRecord>,
}
impl Default for GtxPresetGroup {
    fn default() -> Self {
        Self {
            name: Some("Preset".encode_utf16().collect()),
            group_id: [0; 16],
            hidden: false,
            present: Arc::new(AtomicBool::new(true)),
            presets: Vec::new(),
        }
    }
}
impl GtxPresetGroup {
    pub(super) fn refreshed(&self, presets: Vec<GtxPresetRecord>) -> Self {
        Self {
            name: self.name.clone(),
            group_id: self.group_id,
            hidden: presets.iter().all(GtxPresetRecord::is_hidden),
            present: Arc::new(AtomicBool::new(false)),
            presets,
        }
    }

    pub(super) fn lab_snapshot(
        name: Vec<u16>,
        group_id: [u8; 16],
        presets: Vec<GtxPresetRecord>,
    ) -> Self {
        Self {
            name: Some(name),
            group_id,
            hidden: false,
            present: Arc::new(AtomicBool::new(false)),
            presets,
        }
    }

    pub(super) fn presence(&self) -> &Arc<AtomicBool> {
        &self.present
    }

    fn copy_with<E>(
        &self,
        copy_reference: &mut impl FnMut(&ReferenceValueHandle) -> Result<ReferenceValueHandle, E>,
    ) -> Result<Self, E> {
        Ok(Self {
            name: self.name.clone(),
            group_id: self.group_id,
            hidden: self.hidden,
            present: Arc::new(AtomicBool::new(true)),
            presets: self
                .presets
                .iter()
                .map(|p| p.copy_with(copy_reference))
                .collect::<Result<_, _>>()?,
        })
    }
    pub fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), GtxPresetError> {
        self.name = Some(reader.read_string()?);
        self.group_id = read_guid(reader)?;
        self.presets.clear();
        let count = reader.read_i32()?;
        // Negative source loop counts mean no iterations, not a count error.
        for _ in 0..count {
            let preset = GtxPresetRecord::read(reader, version)?;
            self.presets
                .try_reserve(1)
                .map_err(|_| GtxPresetError::Resource)?;
            self.presets.push(preset);
        }
        Ok(())
    }
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), GtxPresetError> {
        writer.write_optional_utf16(self.name.as_deref())?;
        writer.write_bytes(&self.group_id)?;
        let count = self.presets.iter().filter(|v| !v.is_hidden()).count();
        writer.write_i32(i32::try_from(count).map_err(|_| GtxPresetError::Resource)?)?;
        for value in &self.presets {
            if !value.is_hidden() {
                value.save(writer)?;
            }
        }
        Ok(())
    }
    pub fn name(&self) -> Option<&[u16]> {
        self.name.as_deref()
    }
    pub fn set_name(&mut self, name: Option<Vec<u16>>) {
        self.name = name;
    }
    pub fn group_id(&self) -> [u8; 16] {
        self.group_id
    }
    pub fn is_hidden(&self) -> bool {
        self.hidden || !self.present.load(Ordering::Acquire)
    }
    pub fn presets(&self) -> &[GtxPresetRecord] {
        &self.presets
    }
    pub fn presets_mut(&mut self) -> &mut [GtxPresetRecord] {
        &mut self.presets
    }
    /// Actual u9.GroupID setter only retags entries matching the old group ID.
    pub fn set_group_id(&mut self, id: [u8; 16]) {
        for preset in &mut self.presets {
            if preset.group_id == self.group_id {
                preset.set_group_id(id);
            }
        }
        self.group_id = id;
    }
}

/// sp's serialized groups. Load publishes a default group before reading it;
/// errors therefore retain that partially loaded final group.
#[derive(Debug, Default)]
pub struct GtxPresetCollection {
    groups: Vec<GtxPresetGroup>,
}
impl GtxPresetCollection {
    pub(super) fn stage_lab_snapshot(
        &mut self,
        snapshot: GtxPresetGroup,
        replacing: Option<&Arc<AtomicBool>>,
    ) {
        self.groups.retain(|group| {
            group.present.load(Ordering::Acquire) || Arc::strong_count(&group.present) > 1
        });
        let index = replacing
            .and_then(|old| {
                self.groups
                    .iter()
                    .position(|group| Arc::ptr_eq(group.presence(), old))
            })
            .unwrap_or(self.groups.len());
        self.groups.insert(index, snapshot);
    }

    /// Source sp copy retains only visible groups, while each retained group
    /// copies all of its presets, including hidden ones.
    pub fn source_copy(&self) -> Result<Self, crate::GradientError> {
        self.copy_with(true, &mut |value| {
            Ok(match value {
                ReferenceValueHandle::Gradient(gradient) => ReferenceValueHandle::Gradient(
                    SharedGradientRecord::from_live(gradient.live().source_clone()?),
                ),
                ReferenceValueHandle::Curve(curve) => {
                    ReferenceValueHandle::Curve(curve.detached_copy())
                }
            })
        })
    }
    pub(crate) fn copy_with<E>(
        &self,
        visible_only: bool,
        copy_reference: &mut impl FnMut(&ReferenceValueHandle) -> Result<ReferenceValueHandle, E>,
    ) -> Result<Self, E> {
        Ok(Self {
            groups: self
                .groups
                .iter()
                .filter(|group| {
                    group.present.load(Ordering::Acquire) && (!visible_only || !group.hidden)
                })
                .map(|group| group.copy_with(copy_reference))
                .collect::<Result<_, _>>()?,
        })
    }
    pub fn load(
        &mut self,
        reader: &mut BinaryCursor<'_>,
        version: f64,
    ) -> Result<(), GtxPresetError> {
        self.groups.clear();
        let count = reader.read_i32()?;
        for _ in 0..count {
            self.groups
                .try_reserve(1)
                .map_err(|_| GtxPresetError::Resource)?;
            self.groups.push(GtxPresetGroup::default());
            self.groups
                .last_mut()
                .expect("published group")
                .load(reader, version)?;
        }
        Ok(())
    }
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), GtxPresetError> {
        let count = self.groups.iter().filter(|v| !v.is_hidden()).count();
        writer.write_i32(i32::try_from(count).map_err(|_| GtxPresetError::Resource)?)?;
        for group in &self.groups {
            if !group.is_hidden() {
                group.save(writer)?;
            }
        }
        Ok(())
    }
    pub fn groups(&self) -> &[GtxPresetGroup] {
        &self.groups
    }
    pub fn groups_mut(&mut self) -> &mut [GtxPresetGroup] {
        &mut self.groups
    }
}

fn read_guid(reader: &mut BinaryCursor<'_>) -> Result<[u8; 16], GtxPresetError> {
    reader
        .read_bytes(16)?
        .try_into()
        .map_err(|_| GtxPresetError::Guid)
}
#[derive(Debug)]
pub enum GtxPresetError {
    Binary(BinaryError),
    Guid,
    Reference(ReferenceAdjusterError),
    Environment(EnvironmentWireError),
    Synthesis(Box<crate::NodeWireError>),
    UnsupportedType { tag: i32, name: Option<Vec<u16>> },
    InvalidType { tag: i32 },
    Resource,
}
impl GtxPresetError {
    /// Unsupported valid source payloads are target boundaries, not invented
    /// source exceptions. Resource exhaustion also has a separate qualification.
    pub fn source_class(&self) -> Option<&'static str> {
        match self {
            Self::Binary(e) => Some(e.source_class()),
            Self::Guid => Some("ArgumentException"),
            Self::Reference(e) => Some(e.source_class()),
            Self::InvalidType { .. } => Some("ArgumentException"),
            Self::Environment(_)
            | Self::Synthesis(_)
            | Self::UnsupportedType { .. }
            | Self::Resource => None,
        }
    }
}
impl From<BinaryError> for GtxPresetError {
    fn from(e: BinaryError) -> Self {
        Self::Binary(e)
    }
}
impl fmt::Display for GtxPresetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GTX preset: {self:?}")
    }
}
impl Error for GtxPresetError {}

#[cfg(test)]
mod copy_laws {
    use super::*;
    #[test]
    fn source_copy_omits_hidden_groups_but_keeps_hidden_presets() {
        let mut hidden_preset =
            GtxPresetRecord::new(GtxPresetKind::Number, Some(vec![65]), [1; 16]);
        hidden_preset.set_hidden(true);
        let group = GtxPresetGroup {
            name: Some(vec![66]),
            group_id: [1; 16],
            hidden: false,
            present: Arc::new(AtomicBool::new(true)),
            presets: vec![hidden_preset],
        };
        let hidden_group = GtxPresetGroup {
            name: Some(vec![67]),
            group_id: [2; 16],
            hidden: true,
            present: Arc::new(AtomicBool::new(true)),
            presets: vec![],
        };
        let original = GtxPresetCollection {
            groups: vec![group, hidden_group],
        };
        let copy = original.source_copy().unwrap();
        assert_eq!(copy.groups.len(), 1);
        assert_eq!(copy.groups[0].presets.len(), 1);
        assert!(copy.groups[0].presets[0].is_hidden());
        let snapshot = original
            .copy_with(false, &mut |value| Ok::<_, ()>(value.clone()))
            .unwrap();
        assert_eq!(snapshot.groups.len(), 2);
    }
}
