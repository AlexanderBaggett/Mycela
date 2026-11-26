use crate::{
    ForegroundError, LIVE_PREVIEW_SIZE,
    solid_color::map_preview::{self, PreviewSelection},
};
use gpui_kit::RenderImage;
use image::{ImageReader, Limits};
use mycela_engine::{
    BinaryWriter, Gtx375GraphDocument, ImageGeometry, NodeWireReadPolicy, PointwiseNode,
    PointwiseNodeRenderSnapshot, PreviewBitmap, RenderCaptureSession, RenderedImage,
    SharedImageMap,
};
use std::{
    fs::{File, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
pub(super) type Document = Gtx375GraphDocument<ForegroundError>;
pub(crate) type LiveDocument = Arc<Mutex<Document>>;
pub(crate) type Node = PointwiseNode<ForegroundError>;
const MAX_DOCUMENT_MIB: u64 = 256;
const MAX_FILE_BYTES: u64 = MAX_DOCUMENT_MIB * 1024 * 1024;
pub(super) const MAX_DEPTH: usize = 128;
const MAX_GRAPH_NODES: usize = 8192;
const MAX_BRANCHES: usize = 4096;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DocumentFormat {
    Mycela,
    Gtx,
}

impl DocumentFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mycela => "mycela",
            Self::Gtx => "gtx",
        }
    }

    pub fn validate_save_path(self, path: &Path) -> Result<(), String> {
        let extension = self.extension();
        if path
            .extension()
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            Ok(())
        } else {
            Err(format!("Choose a filename ending in .{extension}."))
        }
    }
}
pub(super) fn failure(value: impl std::fmt::Display) -> String {
    value.to_string()
}
pub(super) fn guard<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
#[derive(Clone)]
pub(super) struct Ticket {
    pub active: Arc<AtomicBool>,
}
impl Ticket {
    pub fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(true)),
        }
    }
    pub fn is_current(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    pub fn retire(&self) {
        self.active.store(false, Ordering::Release);
    }
}

pub(super) fn decode_preset_thumbnail(
    thumbnail: Vec<u8>,
    max_side: Option<u32>,
) -> Result<Arc<RenderImage>, String> {
    let mut reader = ImageReader::new(Cursor::new(thumbnail))
        .with_guessed_format()
        .map_err(failure)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(512);
    limits.max_image_height = Some(512);
    limits.max_alloc = Some(8 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(failure)?;
    let mut raster = match max_side {
        Some(size) => image.thumbnail(size, size).into_rgba8(),
        None => image.into_rgba8(),
    };
    for pixel in raster.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(raster)])))
}
#[derive(Clone)]
pub(super) struct Preview {
    pub bitmap: PreviewBitmap,
    pub raster: Arc<RenderImage>,
    pub selection: PreviewSelection,
    pub map_choices: Arc<[PreviewSelection]>,
}
impl Preview {
    pub fn base_color(bitmap: PreviewBitmap, raster: Arc<RenderImage>) -> Self {
        Self {
            bitmap,
            raster,
            selection: PreviewSelection::BaseColor,
            map_choices: Arc::from([]),
        }
    }

    pub(super) fn project(
        image: &SharedImageMap,
        requested: &PreviewSelection,
    ) -> Result<Self, String> {
        let choices = map_preview::choices(image);
        let selection = if choices.contains(requested) {
            requested.clone()
        } else {
            PreviewSelection::BaseColor
        };
        let (bitmap, raster) = map_preview::project(image, &selection).map_err(failure)?;
        Ok(Self {
            bitmap,
            raster,
            selection,
            map_choices: choices.into(),
        })
    }
}
pub(super) enum PreviewOutput {
    Still(Preview),
    Animation(Vec<Preview>),
}
pub(super) fn capture(node: &Node) -> Result<PointwiseNodeRenderSnapshot<ForegroundError>, String> {
    let mut session = RenderCaptureSession::new();
    let frame = session.capture_node(node, 0.).map_err(failure)?;
    drop(session);
    Ok(frame)
}
/// The pending frame owns only detached graph data and immutable spill leases.
pub(super) struct PendingPreview {
    frame: mycela_engine::PendingNodeRender<ForegroundError>,
    batch: mycela_engine::FrozenRenderBatch,
}
impl PendingPreview {
    pub fn values(&self) -> mycela_engine::FrozenRenderValues {
        self.frame.frozen_values().clone()
    }
    pub fn materialize(self) -> Result<PointwiseNodeRenderSnapshot<ForegroundError>, String> {
        let mut batch = self.batch.materialize().map_err(failure)?;
        let frame = self.frame.complete(&mut batch).map_err(failure)?;
        drop(batch);
        Ok(frame)
    }
}
pub(super) fn freeze(node: &Node, time: f64) -> Result<PendingPreview, String> {
    freeze_with_environment(node, time, None)
}

pub(super) type EnvironmentPreview = Arc<mycela_engine::EnvironmentRenderOverride<ForegroundError>>;

pub(super) fn freeze_with_environment(
    node: &Node,
    time: f64,
    environment: Option<&EnvironmentPreview>,
) -> Result<PendingPreview, String> {
    let session = environment.map_or_else(mycela_engine::RenderFreezeSession::new, |edit| {
        mycela_engine::RenderFreezeSession::with_environment_override(edit.clone())
    });
    freeze_with_session(node, time, session)
}

fn freeze_with_session(
    node: &Node,
    time: f64,
    mut session: mycela_engine::RenderFreezeSession<ForegroundError>,
) -> Result<PendingPreview, String> {
    let frame = session.freeze_node(node, time).map_err(failure)?;
    let batch = session.finish().map_err(failure)?;
    Ok(PendingPreview { frame, batch })
}
pub(super) fn render(
    frame: PointwiseNodeRenderSnapshot<ForegroundError>,
    ticket: &Ticket,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
) -> Result<Option<Preview>, String> {
    render_map(frame, ticket, library, &PreviewSelection::BaseColor)
}

pub(super) fn render_map(
    frame: PointwiseNodeRenderSnapshot<ForegroundError>,
    ticket: &Ticket,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
    selection: &PreviewSelection,
) -> Result<Option<Preview>, String> {
    render_still(frame, ticket, library, false, selection)
}

pub(super) fn render_canvas_tile(
    frame: PointwiseNodeRenderSnapshot<ForegroundError>,
    ticket: &Ticket,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
) -> Result<Option<Preview>, String> {
    render_still(frame, ticket, library, true, &PreviewSelection::BaseColor)
}

fn render_still(
    frame: PointwiseNodeRenderSnapshot<ForegroundError>,
    ticket: &Ticket,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
    canvas_tile: bool,
    selection: &PreviewSelection,
) -> Result<Option<Preview>, String> {
    if !ticket.is_current() {
        return Ok(None);
    }
    let cancelled = || !ticket.is_current();
    // Refraction Tiles renders differently on the ordinary and final graph
    // schedulers at the same dimensions. Use the PNG route for still previews.
    let geometry = ImageGeometry::new(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE);
    let rendered = if canvas_tile {
        crate::scene_backend::render_canvas_tile(&frame, geometry, &cancelled, library)
    } else {
        crate::scene_backend::render_export(
            &frame,
            geometry,
            &cancelled,
            crate::scene_backend::GRAPH_PNG_OPTIONS,
            None,
            None,
            library,
        )
    };
    let image = match rendered {
        Ok(image) => image,
        Err(crate::filter_pipeline::RenderError::Cancelled) => return Ok(None),
        Err(error) => return Err(format!("The selected graph could not render: {error}")),
    };
    if !ticket.is_current() {
        return Ok(None);
    }
    let result = Preview::project(&image, selection);
    image.dispose();
    let preview = result?;
    if !ticket.is_current() {
        return Ok(None);
    }
    Ok(Some(preview))
}

pub(super) fn render_animation(
    frame: PointwiseNodeRenderSnapshot<ForegroundError>,
    ticket: &Ticket,
    library: Option<&mycela_engine::EnvironmentResourceLibrary>,
    selection: &PreviewSelection,
) -> Result<Option<PreviewOutput>, String> {
    let frames = match crate::scene_backend::render_graph_playback(
        &frame,
        ImageGeometry::new(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE),
        &|| !ticket.is_current(),
        library,
    ) {
        Ok(RenderedImage::Animated(frames)) => frames,
        Ok(RenderedImage::Still(_)) => {
            return Err("The selected output has no animation.".into());
        }
        Err(crate::filter_pipeline::RenderError::Cancelled) => return Ok(None),
        Err(error) => return Err(format!("The animated preview could not render: {error}")),
    };
    let mut previews = Vec::with_capacity(frames.len());
    for image in frames.iter() {
        if !ticket.is_current() {
            return Ok(None);
        }
        previews.push(Preview::project(image, selection)?);
    }
    Ok(Some(PreviewOutput::Animation(previews)))
}
pub(super) struct Prepared {
    pub document: LiveDocument,
    pub path: PathBuf,
    pub format: DocumentFormat,
    pub preview: Option<Preview>,
    pub preview_error: Option<String>,
    pub import_warning: Option<String>,
}

/// Load a graph with the document picker’s size, format, and graph limits.
/// Preset application renders only after the loaded graph joins the document.
pub(super) fn read_document(
    path: &Path,
    format: DocumentFormat,
    ticket: &Ticket,
) -> Result<Option<Document>, String> {
    let Some(bytes) = read_document_bytes(path, format, ticket)? else {
        return Ok(None);
    };
    if !ticket.is_current() {
        return Ok(None);
    }
    let (_, document) = decode_document(&bytes)?;
    Ok(ticket.is_current().then_some(document))
}

fn read_document_bytes(
    path: &Path,
    format: DocumentFormat,
    ticket: &Ticket,
) -> Result<Option<Vec<u8>>, String> {
    if !ticket.is_current() {
        return Ok(None);
    }
    if !path
        .metadata()
        .map_err(|e| format!("Couldn’t open document: {e}"))?
        .is_file()
    {
        return Err("Choose a regular file.".into());
    }
    let mut file = File::open(path).map_err(|e| format!("Couldn’t open document: {e}"))?;
    let size = file.metadata().map_err(failure)?.len();
    if size > MAX_FILE_BYTES {
        return Err(format!(
            "Choose a document of {MAX_DOCUMENT_MIB} MiB or less."
        ));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size as usize)
        .map_err(|_| "Couldn’t allocate memory to read the document.".to_owned())?;
    Read::by_ref(&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Couldn’t read document: {e}"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "Choose a document of {MAX_DOCUMENT_MIB} MiB or less."
        ));
    }
    match (format, Document::is_mycela_project(&bytes)) {
        (DocumentFormat::Mycela, false) => {
            Err("Choose a Mycela project. Use Import Genetica GTX for GTX files.".into())
        }
        (DocumentFormat::Gtx, true) => Err("Use Open project to open a Mycela project.".into()),
        _ => Ok(Some(bytes)),
    }
}

fn decode_document(bytes: &[u8]) -> Result<(DocumentFormat, Document), String> {
    let policy = NodeWireReadPolicy::new(MAX_DEPTH, MAX_GRAPH_NODES);
    let (format, result) = if Document::is_mycela_project(bytes) {
        (
            DocumentFormat::Mycela,
            Document::load_project(bytes, policy),
        )
    } else {
        (DocumentFormat::Gtx, Document::load_bounded(bytes, policy))
    };
    let document = result.map_err(|e| {
        format!(
            "Couldn’t open document at byte {}: {}",
            e.position(),
            e.kind()
        )
    })?;
    if document.consumed() as usize != bytes.len() {
        return Err(
            "This document has trailing content that the editor cannot preserve yet.".into(),
        );
    }
    if document.branches().len() > MAX_BRANCHES {
        return Err(format!(
            "This editor supports at most {MAX_BRANCHES} document branches."
        ));
    }
    Ok((format, document))
}

pub(super) fn gtx_import_warning(document: &Document) -> Option<String> {
    let mut pending: Vec<_> = document.branches().iter().flatten().cloned().collect();
    let mut seen = std::collections::HashSet::new();
    let mut count = 0;
    while let Some(node) = pending.pop() {
        if !seen.insert(node.occurrence_id()) {
            continue;
        }
        if node
            .shader_program()
            .is_some_and(|program| program.is_imported_pixel_script())
        {
            count += 1;
        }
        pending.extend(
            node.slots()
                .into_iter()
                .chain(node.contents())
                .filter_map(|slot| slot.node()),
        );
    }
    (count > 0).then(|| {
        let nodes = if count == 1 { "node" } else { "nodes" };
        format!("Converted {count} PixelScript {nodes} to Shader. The original C# is preserved and must be replaced with WGSL before rendering.")
    })
}

impl Prepared {
    pub fn read(
        path: PathBuf,
        format: DocumentFormat,
        ticket: &Ticket,
        library: Option<&mycela_engine::EnvironmentResourceLibrary>,
    ) -> Result<Option<Self>, String> {
        let Some(bytes) = read_document_bytes(&path, format, ticket)? else {
            return Ok(None);
        };
        Self::decode(path, bytes, ticket, library)
    }
    pub fn decode(
        path: PathBuf,
        bytes: Vec<u8>,
        ticket: &Ticket,
        library: Option<&mycela_engine::EnvironmentResourceLibrary>,
    ) -> Result<Option<Self>, String> {
        if !ticket.is_current() {
            return Ok(None);
        }
        let (format, document) = decode_document(&bytes)?;
        drop(bytes);
        if !ticket.is_current() {
            return Ok(None);
        }
        let import_warning = if format == DocumentFormat::Gtx {
            gtx_import_warning(&document)
        } else {
            None
        };
        let result = document
            .branches()
            .iter()
            .flatten()
            .next()
            .map(|node| capture(node).and_then(|frame| render(frame, ticket, library)))
            .transpose();
        let (preview, preview_error) = match result {
            Ok(value) => (value.flatten(), None),
            Err(error) => (None, Some(error)),
        };
        if !ticket.is_current() {
            return Ok(None);
        }
        Ok(Some(Self {
            document: Arc::new(Mutex::new(document)),
            path,
            format,
            preview,
            preview_error,
            import_warning,
        }))
    }
}
// Same-directory create_new + rename preserves the previous destination on
// preparation/write failure. Platform replacement refusal is surfaced as error.
struct Temporary(Option<PathBuf>);
impl Temporary {
    fn create(destination: &Path) -> std::io::Result<(Self, File)> {
        if destination.file_name().is_none() {
            return Err(std::io::Error::other("Choose a file name"));
        }
        let directory = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        for _ in 0..32 {
            let name = format!(
                ".mycela-document-{}-{}.gtx.tmp",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            );
            let path = directory.join(name);
            if path.file_name() == destination.file_name() {
                continue;
            }
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((Self(Some(path)), file)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other("Document temporary names are in use"))
    }
    fn commit(&mut self, destination: &Path) -> std::io::Result<()> {
        std::fs::rename(
            self.0
                .as_ref()
                .ok_or_else(|| std::io::Error::other("Document already committed"))?,
            destination,
        )?;
        self.0 = None;
        Ok(())
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}
pub(super) fn save(
    document: &LiveDocument,
    path: PathBuf,
    format: DocumentFormat,
    ticket: &Ticket,
) -> Result<Option<PathBuf>, String> {
    if !ticket.is_current() {
        return Ok(None);
    }
    // Graph document edits are disabled for the live-owner serialization.
    let writer = serialize(document, format)?;
    save_bytes(writer.bytes(), path, ticket)
}

pub(super) fn serialize(
    document: &LiveDocument,
    format: DocumentFormat,
) -> Result<BinaryWriter, String> {
    let mut writer = BinaryWriter::default();
    let mut document = guard(document);
    match format {
        DocumentFormat::Mycela => document.save_project(&mut writer),
        DocumentFormat::Gtx => document.save_compatible_gtx(&mut writer),
    }
    .map_err(|e| format!("Couldn’t serialize document: {e}"))?;
    Ok(writer)
}

pub(super) fn duplicate(document: &LiveDocument) -> Result<LiveDocument, String> {
    let writer = serialize(document, DocumentFormat::Mycela)?;
    let policy = NodeWireReadPolicy::new(MAX_DEPTH, MAX_GRAPH_NODES);
    let copy = Document::load_project(writer.bytes(), policy).map_err(|error| {
        format!(
            "Couldn’t duplicate document at byte {}: {}",
            error.position(),
            error.kind()
        )
    })?;
    Ok(Arc::new(Mutex::new(copy)))
}

pub(super) fn save_bytes(
    bytes: &[u8],
    path: PathBuf,
    ticket: &Ticket,
) -> Result<Option<PathBuf>, String> {
    write_file(path, ticket, |file| file.write_all(bytes).map_err(failure))
}

pub(super) fn write_file(
    path: PathBuf,
    ticket: &Ticket,
    write: impl FnOnce(&mut File) -> Result<(), String>,
) -> Result<Option<PathBuf>, String> {
    write_file_if(path, ticket, write, |_| Ok(()))
}

pub(super) fn replace_bytes_if(
    bytes: &[u8],
    path: PathBuf,
    ticket: &Ticket,
    verify: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<Option<PathBuf>, String> {
    write_file_if(
        path,
        ticket,
        |file| file.write_all(bytes).map_err(failure),
        verify,
    )
}

fn write_file_if(
    path: PathBuf,
    ticket: &Ticket,
    write: impl FnOnce(&mut File) -> Result<(), String>,
    verify: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<Option<PathBuf>, String> {
    write_staged_file(path, ticket, write, |temp, path| {
        verify(path)?;
        temp.commit(path)
            .map_err(|e| format!("Couldn’t replace the file: {e}. Choose another path."))
    })
}

pub(super) fn save_new_bytes(
    bytes: &[u8],
    path: PathBuf,
    ticket: &Ticket,
) -> Result<Option<PathBuf>, String> {
    write_staged_file(
        path,
        ticket,
        |file| file.write_all(bytes).map_err(failure),
        |temp, path| {
            // Publishing a second name for the finished file preserves any
            // destination created during encoding. The temporary name is removed on drop.
            std::fs::hard_link(temp.0.as_ref().expect("staged file"), path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    "A preset with this name already exists in the category.".into()
                } else {
                    format!("Couldn’t publish the new preset: {error}")
                }
            })
        },
    )
}

fn write_staged_file(
    path: PathBuf,
    ticket: &Ticket,
    write: impl FnOnce(&mut File) -> Result<(), String>,
    commit: impl FnOnce(&mut Temporary, &Path) -> Result<(), String>,
) -> Result<Option<PathBuf>, String> {
    if !ticket.is_current() {
        return Ok(None);
    }
    let (mut temp, mut file) =
        Temporary::create(&path).map_err(|e| format!("Couldn’t create the file: {e}"))?;
    write(&mut file)?;
    file.sync_all()
        .map_err(|e| format!("Couldn’t write the file: {e}"))?;
    drop(file);
    // A publish already in progress can finish after retirement. Its completion
    // is ignored by the closed or replaced view.
    if !ticket.is_current() {
        return Ok(None);
    }
    commit(&mut temp, &path)?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom};

    #[test]
    fn nested_tone_curve_preview_uses_final_inherited_choices_at_512() {
        for (bytes, changes_with_time) in [
            (
                include_bytes!("../../../../port/evidence/tone-curve-map-variable.gtx").as_slice(),
                true,
            ),
            (
                include_bytes!("../../../../port/evidence/tone-curve-wizard-variable.gtx")
                    .as_slice(),
                false,
            ),
        ] {
            let document = Document::load_bounded(bytes, NodeWireReadPolicy::new(16, 128)).unwrap();
            let root = document.branch(0).flatten().unwrap();
            let group = root.content(0).unwrap().node().unwrap();
            let tone = group.content(0).unwrap().node().unwrap();
            let geometry = ImageGeometry::new(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE);
            let frames = [0.125, 0.875].map(|time| {
                let snapshot = freeze(&tone, time).unwrap().materialize().unwrap();
                let preview = render(snapshot.clone(), &Ticket::new(), None)
                    .unwrap()
                    .unwrap();
                let exported = crate::scene_backend::render_export(
                    &snapshot,
                    geometry,
                    &|| false,
                    crate::scene_backend::GRAPH_PNG_OPTIONS,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let expected =
                    map_preview::project_bitmap(&exported, &PreviewSelection::BaseColor).unwrap();
                let pixels = preview.bitmap.rows().unwrap();
                assert_eq!(pixels, expected.rows().unwrap());
                pixels
            });
            assert_eq!(frames[0] != frames[1], changes_with_time);

            let snapshot = freeze(&tone, 0.0).unwrap().materialize().unwrap();
            let Some(PreviewOutput::Animation(playback)) =
                render_animation(snapshot, &Ticket::new(), None, &PreviewSelection::BaseColor)
                    .unwrap()
            else {
                panic!("Expected an inherited animation sequence");
            };
            assert_eq!(playback.len(), RenderedImage::PREVIEW_FRAME_COUNT);
            for (index, still) in [4, 28].into_iter().zip(frames) {
                assert_eq!(playback[index].bitmap.rows().unwrap(), still);
            }
        }
    }

    #[test]
    fn pixel_script_import_warns_and_preserves_source_for_manual_migration() {
        let source = "// Original C#\r\nresult = new color(1, 0, 0, 1);";
        let mut gtx = BinaryWriter::default();
        gtx.write_utf16(&"Genetica]%82?/{=".encode_utf16().collect::<Vec<_>>())
            .unwrap();
        gtx.write_f64(3.83).unwrap();
        gtx.write_i32(0).unwrap(); // Preview flags
        gtx.write_u8(0).unwrap(); // Previews
        gtx.write_i16(0).unwrap(); // Catalog entries
        mycela_engine::GtxPresetCollection::default()
            .save(&mut gtx)
            .unwrap();
        gtx.write_i32(0).unwrap(); // Branch positions
        gtx.write_i32(1).unwrap(); // Branches
        gtx.write_utf16(&"PixelScript".encode_utf16().collect::<Vec<_>>())
            .unwrap();
        gtx.write_u8(1).unwrap(); // Script property only
        gtx.write_bool(false).unwrap(); // Animation
        gtx.write_bool(false).unwrap(); // Adjustment
        gtx.write_utf16(&source.encode_utf16().collect::<Vec<_>>())
            .unwrap();
        gtx.write_i32(0).unwrap(); // Custom properties
        gtx.write_u8(2).unwrap(); // Input slots
        for _ in 0..2 {
            gtx.write_utf16(&"null".encode_utf16().collect::<Vec<_>>())
                .unwrap();
        }
        for output in 0..3 {
            gtx.write_i32(output).unwrap();
        }

        let prepared = Prepared::decode(
            PathBuf::from("script.gtx"),
            gtx.bytes().to_vec(),
            &Ticket::new(),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(prepared.format, DocumentFormat::Gtx);
        let warning = prepared.import_warning.unwrap();
        assert!(warning.contains("Converted 1 PixelScript node to Shader"));
        assert!(warning.contains("must be replaced with WGSL before rendering"));
        assert!(prepared.preview.is_none());
        assert!(
            prepared
                .preview_error
                .unwrap()
                .contains("replace it with WGSL")
        );
        let node = guard(&prepared.document).branch(0).flatten().unwrap();
        assert_eq!(node.shader_program().unwrap().source(), source);
    }

    #[test]
    fn gtx_383_document_can_open_above_the_old_size_limit() {
        let preview_len = 33 * 1024 * 1024;
        let destination = std::env::temp_dir().join("large-graph.gtx");
        let (temporary, mut file) = Temporary::create(&destination).unwrap();
        let mut header = BinaryWriter::default();
        header
            .write_utf16(&"Genetica]%82?/{=".encode_utf16().collect::<Vec<_>>())
            .unwrap();
        header.write_f64(3.83).unwrap();
        header.write_i32(0).unwrap();
        header.write_u8(1).unwrap();
        header.write_i32(preview_len).unwrap();
        file.write_all(header.bytes()).unwrap();
        file.seek(SeekFrom::Current(i64::from(preview_len)))
            .unwrap();

        let mut body = BinaryWriter::default();
        body.write_i16(0).unwrap();
        mycela_engine::GtxPresetCollection::default()
            .save(&mut body)
            .unwrap();
        body.write_i32(0).unwrap();
        body.write_i32(1).unwrap();
        body.write_utf16(&"null".encode_utf16().collect::<Vec<_>>())
            .unwrap();
        for output in 0..3 {
            body.write_i32(output).unwrap();
        }
        file.write_all(body.bytes()).unwrap();
        drop(file);

        let prepared = Prepared::read(
            temporary.0.as_ref().unwrap().clone(),
            DocumentFormat::Gtx,
            &Ticket::new(),
            None,
        )
        .unwrap()
        .unwrap();
        let document = guard(&prepared.document);
        assert_eq!(document.header().source_version(), 3.83);
        assert_eq!(document.header().previews()[0].len(), preview_len as usize);
        assert_eq!(document.branches().len(), 1);
        assert!(document.branch(0).unwrap().is_none());
    }
}
