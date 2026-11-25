//! Genetica binary file framing and retained metadata.
//!
//! The historical wire-document API owns detached Solid Color records. The
//! graph-document API owns actual live node roots and uses recursive node IO.
//! Unknown payloads have no length and stop at their real boundary. Attached
//! document history and source preview regeneration remain separate.
use crate::{
    AdjusterMetadataRecord, BinaryCursor, BinaryError, BinaryWriter, ContributorRecord,
    ContributorRecordError, ReferenceAdjusterError, ReferenceAdjusterKind, ReferenceAdjusterRecord,
    SolidColorRecordError, SolidColorWireRecord, ValueAdjusterError, ValueAdjusterKind,
    ValueAdjusterRecord,
};
use std::{error::Error, fmt};

mod catalog;
mod graph;
pub(crate) use graph::{
    AffectedProperties, BranchOrderChange, DocumentPreviewSamples, LabLayoutEdit, PresetInputMove,
    PreviewSampleChange,
};
pub use graph::{
    ColorAdjusterLink, CurveAdjuster, CurveAdjusterLink, EnvironmentAdjuster,
    EnvironmentAdjusterLink, EnvironmentColorLink, EnvironmentMapTarget, EnvironmentNumericMapping,
    FilterPreviewSample, GradientAdjusterLink, Gtx375GraphDocument, GtxGraphError,
    GtxGraphLoadFailure, GtxIncompatibility, LabAdjusterMetadata, LabColorAdjuster, LabColorValue,
    LabNumericAdjuster, LabNumericAxis, LabNumericLink, LabNumericMapping, LabNumericRange,
    LabNumericValue, LabPreset, LabPresetOperation, LabPresetSection, LabScalarDefinition,
    OverviewPresetMode, StyleSynthesisNumberMapping, StyleSynthesisNumberTarget, SynthesisAdjuster,
    SynthesisAdjusterLink, SynthesisDraftPreviewTarget, SynthesisPairTarget,
};
mod presets;
pub use catalog::{EnvironmentCatalogValue, GtxCatalogRecord, NumericCatalogValue};
pub use presets::{
    GtxPresetCollection, GtxPresetError, GtxPresetGroup, GtxPresetKind, GtxPresetRecord,
    GtxPresetValue,
};

const MAGIC: &str = "Genetica]%82?/{=";
const VERSION: f64 = 3.75;
const LATEST_READ_VERSION: f64 = 3.83;
const PROJECT_MAGIC: &[u8; 8] = b"MYCELA\0\x1a";
const PROJECT_VERSION: i32 = 3;

/// File front matter. Encoded previews remain bytes, not decoded graph output.
#[derive(Debug)]
pub struct Gtx375Header {
    source_version: f64,
    filter_inputs: i32,
    previews: Vec<Vec<u8>>,
    contributors: crate::DocumentContributors,
}
impl Gtx375Header {
    /// ael.a's binary header/preview/credit path with nodes/previews/credits on
    /// and a nonempty path. The caller owns the reader and its decoder state.
    /// File length is the actual complete input length, not remaining bytes.
    pub fn read(reader: &mut BinaryCursor<'_>, file_length: usize) -> Result<Self, GtxReadError> {
        let marker = reader.read_char()?;
        let identity = if marker == MAGIC.len() as u16 {
            reader.read_chars(i32::from(marker))?
        } else {
            Vec::new()
        };
        if identity != MAGIC.encode_utf16().collect::<Vec<_>>() {
            return Err(GtxReadError::Unsupported(UnsupportedGtx::Format(identity)));
        }
        let version = reader.read_f64()?;
        // Earlier files use the XML reader. The original binary reader's
        // version ceiling is the 3.83 value in TexTree's zl.h.
        if !(3.02..=LATEST_READ_VERSION).contains(&version) {
            return Err(GtxReadError::Unsupported(UnsupportedGtx::Version(
                version.to_bits(),
            )));
        }
        Self::read_body(reader, file_length, version)
    }

    /// Read the metadata and encoded previews at the start of a Mycela project.
    pub fn read_project(
        reader: &mut BinaryCursor<'_>,
        file_length: usize,
    ) -> Result<Self, GtxReadError> {
        if reader.read_bytes(PROJECT_MAGIC.len() as i32)?.as_slice() != PROJECT_MAGIC {
            return Err(GtxReadError::ProjectSignature);
        }
        let version = reader.read_i32()?;
        if !(1..=PROJECT_VERSION).contains(&version) {
            return Err(GtxReadError::ProjectVersion(version));
        }
        Self::read_body(reader, file_length, VERSION)
    }

    fn read_body(
        reader: &mut BinaryCursor<'_>,
        file_length: usize,
        version: f64,
    ) -> Result<Self, GtxReadError> {
        let filter_inputs = reader.read_i32()?;
        let count = if version < 3.04 { 1 } else { reader.read_u8()? };
        let mut previews = Vec::new();
        reserve(&mut previews, usize::from(count))?;
        for _ in 0..count {
            let length = reader.read_i32()?;
            // Original named-file guard compares against the whole stream.
            if length >= 0 && length as usize > file_length {
                return Err(GtxReadError::PreviewExceedsFile);
            }
            // ReadBytes can short-read. The following ordinary read then fails
            // at its actual position; no invented exact-length preview read.
            previews.push(reader.read_bytes(length)?);
        }
        let count = reader.read_i16()?;
        if count < 0 {
            return Err(GtxReadError::Unsupported(UnsupportedGtx::Count {
                section: "contributors",
                count: i32::from(count),
            }));
        }
        let mut contributors = Vec::new();
        reserve(&mut contributors, count as usize)?;
        for _ in 0..count {
            contributors.push(ContributorRecord::load(reader).map_err(GtxReadError::Contributor)?);
        }
        Ok(Self {
            source_version: version,
            filter_inputs,
            previews,
            contributors: crate::DocumentContributors::from_records(contributors),
        })
    }

    /// Version read from the input. Save emits Mycela's 3.75 GTX layout.
    pub fn source_version(&self) -> f64 {
        self.source_version
    }
    pub fn filter_inputs(&self) -> i32 {
        self.filter_inputs
    }
    pub fn previews(&self) -> &[Vec<u8>] {
        &self.previews
    }
    /// Snapshot getter values; actual contributor and profile identities are
    /// retained by `contributor_owner`, which also owns the saved values.
    pub fn contributors(&self) -> Vec<ContributorRecord> {
        self.contributors.records()
    }
    pub fn contributor_owner(&self) -> crate::DocumentContributors {
        self.contributors.clone()
    }

    /// Serialize retained front matter; does not regenerate/dispose GDI previews.
    pub fn save(&self, writer: &mut BinaryWriter) -> Result<(), GtxReadError> {
        self.save_with_filter_inputs(writer, self.filter_inputs)
    }

    fn save_with_filter_inputs(
        &self,
        writer: &mut BinaryWriter,
        filter_inputs: i32,
    ) -> Result<(), GtxReadError> {
        writer.write_utf16(&MAGIC.encode_utf16().collect::<Vec<_>>())?;
        writer.write_f64(VERSION)?;
        self.save_body(writer, filter_inputs)
    }

    fn save_body(&self, writer: &mut BinaryWriter, filter_inputs: i32) -> Result<(), GtxReadError> {
        writer.write_i32(filter_inputs)?;
        writer.write_u8(u8::try_from(self.previews.len()).map_err(|_| GtxReadError::Resource)?)?;
        for preview in &self.previews {
            writer.write_i32(count_i32(preview.len())?)?;
            writer.write_bytes(preview)?;
        }
        let contributors = self.contributors.entries();
        writer.write_i16(i16::try_from(contributors.len()).map_err(|_| GtxReadError::Resource)?)?;
        for contributor in contributors {
            contributor
                .record()
                .save(writer)
                .map_err(GtxReadError::Contributor)?;
        }
        Ok(())
    }
}

/// Recognized branch wire payloads. Null is an actual serialized branch value.
#[derive(Debug)]
pub enum GtxBranch {
    Null,
    SolidColor(Box<SolidColorWireRecord>),
}

/// A decoded known subset of the 3.75 container, not a live document graph.
#[derive(Debug)]
pub struct Gtx375WireDocument {
    header: Gtx375Header,
    presets: GtxPresetCollection,
    // ael's zero catalog count returns null, not a newly allocated empty array.
    catalog: Option<Vec<GtxCatalogRecord>>,
    positions: Vec<[f32; 2]>,
    branches: Vec<GtxBranch>,
    output_order: [i32; 3],
    consumed: i32,
}
impl Gtx375WireDocument {
    pub fn load(input: &[u8]) -> Result<Self, GtxLoadFailure> {
        let mut reader = BinaryCursor::new(input).map_err(|error| GtxLoadFailure {
            position: 0,
            kind: GtxReadError::Binary(error),
        })?;
        let result = Self::read(&mut reader, input.len());
        let position = reader.position().unwrap_or(0);
        reader.close();
        result
            .map(|mut document| {
                document.consumed = position;
                document
            })
            .map_err(|kind| GtxLoadFailure { position, kind })
    }

    fn read(reader: &mut BinaryCursor<'_>, file_length: usize) -> Result<Self, GtxReadError> {
        let FrontMatter {
            header,
            presets,
            catalog,
            positions,
            count,
        } = read_front_matter(reader, file_length)?;
        let mut branches = Vec::new();
        reserve(&mut branches, count as usize)?;
        for _ in 0..count {
            let identity = reader.read_string()?;
            if identity == "null".encode_utf16().collect::<Vec<_>>() {
                branches.push(GtxBranch::Null);
            } else if identity == "Solid Color".encode_utf16().collect::<Vec<_>>() {
                let mut node = SolidColorWireRecord::default();
                node.load_body(reader, header.source_version)
                    .map_err(GtxReadError::SolidColor)?;
                branches.push(GtxBranch::SolidColor(Box::new(node)));
            } else {
                return Err(GtxReadError::Unsupported(UnsupportedGtx::Node(identity)));
            }
        }
        let output_order = read_output_order(reader, header.source_version)?;
        Ok(Self {
            header,
            presets,
            catalog,
            positions,
            branches,
            output_order,
            consumed: 0,
        })
    }

    pub fn header(&self) -> &Gtx375Header {
        &self.header
    }
    pub fn presets(&self) -> &GtxPresetCollection {
        &self.presets
    }
    pub fn presets_mut(&mut self) -> &mut GtxPresetCollection {
        &mut self.presets
    }
    pub fn catalog(&self) -> Option<&[GtxCatalogRecord]> {
        self.catalog.as_deref()
    }
    /// Mutable access retains each reference-adjuster root; no detached copies.
    pub fn catalog_mut(&mut self) -> Option<&mut [GtxCatalogRecord]> {
        self.catalog.as_deref_mut()
    }
    pub fn positions(&self) -> &[[f32; 2]] {
        &self.positions
    }
    pub fn branches(&self) -> &[GtxBranch] {
        &self.branches
    }
    pub fn output_order(&self) -> [i32; 3] {
        self.output_order
    }
    /// Source does not demand EOF after output order; trailing bytes are unparsed.
    pub fn consumed(&self) -> i32 {
        self.consumed
    }

    /// Serialize only decoded fields. No unknown/trailing bytes are replayed.
    /// Node Save may perform its already characterized lazy wire allocations.
    /// This is not rt's progress/JPEG construction/resource-disposal workflow.
    pub fn save(&mut self, writer: &mut BinaryWriter) -> Result<(), GtxReadError> {
        self.header.save(writer)?;
        write_front_matter(
            &mut self.presets,
            self.catalog.iter_mut().flatten().collect(),
            &self.positions,
            self.branches.len(),
            writer,
        )?;
        for branch in &mut self.branches {
            match branch {
                GtxBranch::Null => {
                    writer.write_utf16(&"null".encode_utf16().collect::<Vec<_>>())?
                }
                GtxBranch::SolidColor(node) => node.save(writer).map_err(|error| {
                    GtxReadError::SolidColor(SolidColorRecordError::Property(error))
                })?,
            }
        }
        for word in self.output_order {
            writer.write_i32(word)?;
        }
        Ok(())
    }
}

/// Shared binary framing for detached records and retained node roots.
struct FrontMatter {
    header: Gtx375Header,
    presets: GtxPresetCollection,
    catalog: Option<Vec<GtxCatalogRecord>>,
    positions: Vec<[f32; 2]>,
    count: i32,
}
fn read_front_matter(
    reader: &mut BinaryCursor<'_>,
    file_length: usize,
) -> Result<FrontMatter, GtxReadError> {
    let header = Gtx375Header::read(reader, file_length)?;
    read_front_matter_body(reader, file_length, header)
}

fn read_front_matter_body(
    reader: &mut BinaryCursor<'_>,
    file_length: usize,
    header: Gtx375Header,
) -> Result<FrontMatter, GtxReadError> {
    let version = header.source_version;
    let mut presets = GtxPresetCollection::default();
    let catalog = if version >= 3.72 {
        presets
            .load(reader, version)
            .map_err(GtxReadError::Preset)?;
        let count = reader.read_i32()?;
        if count == 0 {
            None
        } else {
            bound_count(reader, file_length, count, 25, "catalog")?;
            let mut values = Vec::new();
            reserve(&mut values, count as usize)?;
            for _ in 0..count {
                values.push(GtxCatalogRecord::read(reader, version)?);
            }
            Some(values)
        }
    } else {
        None
    };
    let count = if version > 3.5 { reader.read_i32()? } else { 1 };
    if count < 1 {
        return Err(GtxReadError::Unsupported(UnsupportedGtx::Count {
            section: "branches",
            count,
        }));
    }
    // Versions 3.51–3.52 have multiple branches but no stored positions.
    // The source does not create a position list for them; the current editor
    // requires one, so do not fabricate a layout.
    if version < 3.53 && count > 1 {
        return Err(GtxReadError::Unsupported(UnsupportedGtx::Count {
            section: "legacy branch positions",
            count,
        }));
    }
    // Reserve no more position/branch entries than the remaining input
    // can minimally encode. This is an explicit memory policy, not a claim
    // about the CLR's allocation error on malformed gigantic counts.
    bound_count(reader, file_length, count - 1, 8, "branch positions")?;
    let mut positions = Vec::new();
    reserve(&mut positions, (count - 1) as usize)?;
    if version >= 3.53 {
        for _ in 0..count - 1 {
            positions.push([reader.read_f32()?, reader.read_f32()?]);
        }
    }
    bound_count(reader, file_length, count, 1, "branches")?;
    Ok(FrontMatter {
        header,
        presets,
        catalog,
        positions,
        count,
    })
}
fn read_output_order(
    reader: &mut BinaryCursor<'_>,
    version: f64,
) -> Result<[i32; 3], GtxReadError> {
    if version < 3.75 {
        Ok([0, 1, 2])
    } else {
        Ok([reader.read_i32()?, reader.read_i32()?, reader.read_i32()?])
    }
}
fn write_front_matter(
    presets: &mut GtxPresetCollection,
    catalog: Vec<&mut GtxCatalogRecord>,
    positions: &[[f32; 2]],
    branch_count: usize,
    writer: &mut BinaryWriter,
) -> Result<(), GtxReadError> {
    presets.save(writer).map_err(GtxReadError::Preset)?;
    writer.write_i32(count_i32(catalog.len())?)?;
    for value in catalog {
        value.save(writer)?;
    }
    writer.write_i32(count_i32(branch_count)?)?;
    for position in positions {
        writer.write_f32(position[0])?;
        writer.write_f32(position[1])?;
    }
    Ok(())
}

fn reserve<T>(values: &mut Vec<T>, count: usize) -> Result<(), GtxReadError> {
    values
        .try_reserve_exact(count)
        .map_err(|_| GtxReadError::Resource)
}
fn count_i32(count: usize) -> Result<i32, GtxReadError> {
    i32::try_from(count).map_err(|_| GtxReadError::Resource)
}
fn bound_count(
    reader: &BinaryCursor<'_>,
    file_length: usize,
    count: i32,
    minimum: usize,
    section: &'static str,
) -> Result<(), GtxReadError> {
    if count < 0 {
        return Err(GtxReadError::Unsupported(UnsupportedGtx::Count {
            section,
            count,
        }));
    }
    let position = usize::try_from(reader.position()?).map_err(|_| GtxReadError::Resource)?;
    if count as usize > file_length.saturating_sub(position) / minimum {
        return Err(GtxReadError::Resource);
    }
    Ok(())
}

#[derive(Debug)]
pub enum UnsupportedGtx {
    Format(Vec<u16>),
    Version(u64),
    PresetGroups(i32),
    CatalogType { tag: i32, name: Vec<u16> },
    Node(Vec<u16>),
    Count { section: &'static str, count: i32 },
}
#[derive(Debug)]
pub enum GtxReadError {
    Binary(BinaryError),
    Contributor(ContributorRecordError),
    Catalog(ValueAdjusterError),
    ReferenceCatalog(ReferenceAdjusterError),
    Environment(crate::EnvironmentWireError),
    Synthesis(Box<crate::NodeWireError>),
    Preset(GtxPresetError),
    SolidColor(SolidColorRecordError),
    PreviewExceedsFile,
    ProjectSignature,
    ProjectVersion(i32),
    TrailingProjectData,
    LegacyXml(String),
    Unsupported(UnsupportedGtx),
    Resource,
}
impl From<BinaryError> for GtxReadError {
    fn from(value: BinaryError) -> Self {
        Self::Binary(value)
    }
}
impl fmt::Display for GtxReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProjectSignature => f.write_str("Choose a Mycela project file."),
            Self::ProjectVersion(version) => {
                write!(
                    f,
                    "This Mycela project uses unsupported format version {version}."
                )
            }
            Self::TrailingProjectData => f.write_str("This Mycela project contains trailing data."),
            _ => write!(f, "GTX wire: {self:?}"),
        }
    }
}
impl Error for GtxReadError {}

#[derive(Debug)]
pub struct GtxLoadFailure {
    position: i32,
    kind: GtxReadError,
}
impl GtxLoadFailure {
    pub fn position(&self) -> i32 {
        self.position
    }
    pub fn kind(&self) -> &GtxReadError {
        &self.kind
    }
}
impl fmt::Display for GtxLoadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}", self.kind, self.position)
    }
}
impl Error for GtxLoadFailure {}

#[cfg(test)]
mod tests;
