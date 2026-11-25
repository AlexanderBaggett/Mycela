//! Legacy GTX stores one preview and a sized XML payload after its binary magic.
use super::*;
use roxmltree::Document;
use std::borrow::Cow;

/// Only consumes the prefix when it identifies a wrapped XML document.
pub(super) fn read_version(reader: &mut BinaryCursor<'_>) -> Result<Option<f64>, BinaryError> {
    let start = reader.position()?;
    let candidate = (|| {
        let marker = reader.read_char().ok()?;
        if marker != MAGIC.len() as u16
            || !reader
                .read_chars(i32::from(marker))
                .ok()?
                .into_iter()
                .eq(MAGIC.encode_utf16())
        {
            return None;
        }
        let version = reader.read_f64().ok()?;
        (1.0..3.02).contains(&version).then_some(version)
    })();
    if candidate.is_none() {
        reader.seek(i64::from(start))?;
    }
    Ok(candidate)
}

fn invalid(message: impl Into<String>) -> GtxReadError {
    GtxReadError::LegacyXml(message.into())
}

fn decode(bytes: &[u8]) -> Result<Cow<'_, str>, GtxReadError> {
    let little_endian = if bytes.starts_with(&[0xff, 0xfe]) {
        Some(true)
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        Some(false)
    } else {
        None
    };
    if let Some(little_endian) = little_endian {
        let units = bytes[2..].chunks_exact(2);
        if !units.remainder().is_empty() {
            return Err(invalid("Incomplete UTF-16 unit"));
        }
        let units: Vec<_> = units
            .map(|unit| {
                let pair = [unit[0], unit[1]];
                if little_endian {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            })
            .collect();
        return String::from_utf16(&units)
            .map(Cow::Owned)
            .map_err(|error| invalid(error.to_string()));
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    std::str::from_utf8(bytes)
        .map(Cow::Borrowed)
        .map_err(|error| invalid(error.to_string()))
}

fn read_sized(
    reader: &mut BinaryCursor<'_>,
    file_length: usize,
    preview: bool,
) -> Result<Vec<u8>, GtxReadError> {
    let length = reader.read_i32()?;
    if length >= 0 && length as usize > file_length {
        return Err(if preview {
            GtxReadError::PreviewExceedsFile
        } else {
            invalid("XML payload exceeds file")
        });
    }
    let bytes = reader.read_bytes(length)?;
    if bytes.len() != length as usize {
        return Err(invalid("Truncated legacy GTX body"));
    }
    Ok(bytes)
}

fn contributors(
    root: roxmltree::Node<'_, '_>,
) -> Result<crate::DocumentContributors, GtxReadError> {
    let mut records = Vec::new();
    for group in root
        .children()
        .filter(|element| element.has_tag_name("Contribs"))
    {
        for entry in group
            .children()
            .filter(|element| element.has_tag_name("Contrib"))
        {
            let field = |name| {
                entry
                    .attribute(name)
                    .ok_or_else(|| invalid(format!("Missing contributor {name}")))
            };
            let text = |name| field(name).map(|value| value.encode_utf16().collect());
            let ticks = field("date")?
                .trim()
                .parse()
                .map_err(|_| invalid("Invalid contributor date"))?;
            records.push(ContributorRecord {
                name: text("name")?,
                website: text("website")?,
                email: text("email")?,
                ticks: crate::ContributorTicks::new(ticks).map_err(GtxReadError::Contributor)?,
                comment: field("comments")?.trim().encode_utf16().collect(),
            });
        }
    }
    Ok(crate::DocumentContributors::from_records(records))
}

pub(super) fn read<E: 'static>(
    reader: &mut BinaryCursor<'_>,
    file_length: usize,
    version: f64,
    context: &mut crate::pointwise::NodeWireReadContext<'_, E>,
) -> Result<Gtx375GraphDocument<E>, GtxGraphError<E>> {
    let filter_inputs = reader.read_i32()?;
    let preview = read_sized(reader, file_length, true)?;
    let payload = read_sized(reader, file_length, false)?;
    let xml = decode(&payload)?;
    let document = Document::parse(&xml).map_err(|error| invalid(error.to_string()))?;
    let root = document.root_element();
    if !root.has_tag_name("GeneticaTexture") {
        return Err(invalid("Expected a GeneticaTexture element").into());
    }
    let contributors = contributors(root)?;
    let mut principal = None;
    if let Some(element) = root.children().find(|element| element.has_tag_name("Node")) {
        principal = crate::pointwise::load_node_xml(element, version, context)
            .map_err(GtxGraphError::Node)?;
    }
    if let Some(node) = &principal {
        migrate_legacy_seeds(node).map_err(GtxGraphError::Node)?;
    }
    prepare_filter_document(principal.as_ref(), filter_inputs)?;
    let mut graph = Gtx375GraphDocument::new(vec![principal])?;
    graph.header = Gtx375Header {
        source_version: version,
        filter_inputs,
        previews: vec![preview],
        contributors,
    };
    graph.consumed = reader.position()?;
    Ok(graph)
}
