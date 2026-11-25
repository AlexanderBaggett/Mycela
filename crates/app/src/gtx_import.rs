//! Immutable import provenance; the live node remains the current value owner.
use crate::{
    ForegroundError, LIVE_PREVIEW_SIZE,
    graph_document::LiveDocument,
    solid_color::map_preview::{self, PreparedPreview, PreviewSelection},
};
use mycela_engine::{
    Gtx375GraphDocument, Gtx375WireDocument, GtxBranch, GtxPresetError, GtxReadError,
    SharedImageMap, SharedSolidColorNode, SolidColorWireRecord, UnsupportedGtx,
};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct GtxImport {
    path: PathBuf,
    document: Gtx375WireDocument,
    original: Box<[u8]>,
}
pub(crate) struct PreparedGtx {
    pub(crate) import: GtxImport,
    pub(crate) graph: LiveDocument,
    pub(crate) node: SharedSolidColorNode<ForegroundError>,
    pub(crate) preview: PreparedPreview,
}
impl GtxImport {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn document(&self) -> &Gtx375WireDocument {
        &self.document
    }
    pub(crate) fn original(&self) -> &[u8] {
        &self.original
    }
    pub(crate) fn name(&self) -> String {
        self.path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }
}
impl PreparedGtx {
    pub(crate) fn read(path: PathBuf) -> Result<Self, String> {
        if !path
            .metadata()
            .map_err(|e| format!("Couldn’t open GTX: {e}"))?
            .is_file()
        {
            return Err("Choose a regular GTX file.".into());
        }
        let mut file = File::open(&path).map_err(|e| format!("Couldn’t open GTX: {e}"))?;
        let size = file
            .metadata()
            .map_err(|e| format!("Couldn’t read GTX: {e}"))?
            .len();
        if size > MAX_FILE_BYTES {
            return Err("GTX files larger than 32 MiB are not supported yet.".into());
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Couldn’t read GTX: {e}"))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err("GTX files larger than 32 MiB are not supported yet.".into());
        }
        Self::decode(path, bytes)
    }
    fn decode(path: PathBuf, bytes: Vec<u8>) -> Result<Self, String> {
        let document = Gtx375WireDocument::load(&bytes).map_err(|error| {
            let reason = match error.kind() {
                GtxReadError::Unsupported(UnsupportedGtx::Version(_)) => {
                    "This GTX version is outside the supported range of 3.02 to 3.83."
                }
                GtxReadError::Unsupported(UnsupportedGtx::Node(_)) => {
                    "This GTX contains texture nodes that are not supported yet."
                }
                GtxReadError::Unsupported(UnsupportedGtx::CatalogType { .. }) => {
                    "This GTX contains document controls that are not supported yet."
                }
                GtxReadError::Preset(GtxPresetError::UnsupportedType { .. }) => {
                    "This GTX contains presets that are not supported yet."
                }
                GtxReadError::Resource => "This GTX exceeds the supported memory limits.",
                _ => "This GTX is incomplete, damaged, or contains unsupported values.",
            };
            format!("Couldn’t open GTX. {reason}")
        })?;
        if document.consumed() as usize != bytes.len() {
            return Err(
                "This GTX contains trailing content that this editor cannot preserve yet.".into(),
            );
        }
        if document.header().filter_inputs() != 0 {
            return Err(
                "This GTX requires filter inputs that this editor does not support yet.".into(),
            );
        }
        if document.output_order() != [0, 1, 2] {
            return Err(
                "This GTX uses an output channel order that this editor does not support yet."
                    .into(),
            );
        }
        let [GtxBranch::SolidColor(node)] = document.branches() else {
            return Err(
                "This editor currently opens GTX files with one Solid Color branch.".into(),
            );
        };
        eligible(node)?;
        // The wire record keeps the stored values for provenance. The graph
        // reader builds the editable node and reapplies linked Lab controls.
        let (graph, live) =
            Gtx375GraphDocument::load_standalone_solid_color(&bytes).map_err(|e| {
                format!(
                    "Couldn’t load Solid Color at byte {}: {}",
                    e.position(),
                    e.kind()
                )
            })?;
        if graph.consumed() != document.consumed() {
            return Err("Couldn’t decode the complete Solid Color GTX.".into());
        }
        let image = live
            .render(LIVE_PREVIEW_SIZE, LIVE_PREVIEW_SIZE, 0.)
            .map(SharedImageMap::from_image)
            .map_err(|e| format!("Couldn’t render the imported Solid Color: {e}"))?;
        let preview = map_preview::render(image, None, &[], &PreviewSelection::BaseColor, 0.)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            import: GtxImport {
                path,
                document,
                original: bytes.into_boxed_slice(),
            },
            graph: Arc::new(Mutex::new(graph)),
            node: live,
            preview,
        })
    }
}

fn eligible(node: &SolidColorWireRecord) -> Result<(), String> {
    for (name, metadata) in [
        ("Color", &node.color_metadata),
        ("Transparency", &node.transparency_metadata),
    ] {
        // os.aj checks bg >= 0; negative stored binding indices do not
        // resolve an ancestor. Neither unbound render getter applies m_as.
        if metadata.binding.index >= 0 {
            return Err(format!(
                "{name} is bound to document controls, which this editor cannot evaluate yet."
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_binding_that_needs_an_ancestor_is_rejected() {
        let mut node = SolidColorWireRecord::default();
        node.color_metadata.binding.index = 0;
        assert!(eligible(&node).unwrap_err().contains("Color is bound"));
        node.color_metadata.binding.index = -1;
        assert!(eligible(&node).is_ok());
    }
    #[test]
    fn solid_import_retains_adjustments_catalog_and_previews() {
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../genetica/Presets/Texture/Pattern/Solid Color.gtx"
        ));
        let prepared = PreparedGtx::decode("Solid Color.gtx".into(), bytes.to_vec()).unwrap();
        assert_eq!(prepared.node.snapshot().color, [1.; 3]);
        assert_eq!(prepared.import.original(), bytes);
        let GtxBranch::SolidColor(node) = &prepared.import.document().branches()[0] else {
            panic!("real decoded node")
        };
        assert!(
            node.color_metadata
                .adjustment
                .as_ref()
                .unwrap()
                .components()[0]
                .active
        );
        assert_eq!(prepared.import.document().catalog().unwrap().len(), 2);
        assert_eq!(
            prepared.import.document().header().previews()[0].len(),
            1111
        );
        let mut trailing = bytes.to_vec();
        trailing.push(0);
        assert!(
            PreparedGtx::decode("trailing.gtx".into(), trailing)
                .err()
                .unwrap()
                .contains("trailing")
        );
    }
}
