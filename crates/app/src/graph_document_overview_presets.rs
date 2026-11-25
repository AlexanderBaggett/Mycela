//! Discover the Texture and Filter libraries without loading their graphs.
use super::{DocumentFormat, Ticket, model};
use gpui_kit::RenderImage;
use mycela_engine::{BinaryCursor, Gtx375Header};
use std::{
    collections::HashSet,
    env,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

const PRESET_HEADER_LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Kind {
    Texture,
    Filter,
}

impl Kind {
    fn directory(self) -> &'static str {
        match self {
            Self::Texture => "Texture",
            Self::Filter => "Filter",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Preset {
    path: PathBuf,
    format: DocumentFormat,
    name: String,
    categories: Vec<String>,
    search_text: String,
}

impl Preset {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn categories(&self) -> &[String] {
        &self.categories
    }

    pub(super) fn matches(&self, query: &str) -> bool {
        query
            .to_lowercase()
            .split_whitespace()
            .all(|term| self.search_text.contains(term))
    }

    pub(super) fn read(&self, ticket: &Ticket) -> Result<Option<model::Document>, String> {
        model::read_document(&self.path, self.format, ticket)
    }

    pub(super) fn import_warning(&self, document: &model::Document) -> Option<String> {
        if self.format == DocumentFormat::Gtx {
            model::gtx_import_warning(document)
        } else {
            None
        }
    }

    pub(super) fn thumbnail(&self) -> Result<Option<Arc<RenderImage>>, String> {
        let header = header(&self.path, self.format)?;
        header
            .previews()
            .iter()
            .find(|preview| !preview.is_empty())
            .map(|preview| model::decode_preset_thumbnail(preview.clone(), Some(96)))
            .transpose()
    }
}

#[derive(Default)]
pub(super) struct Catalog {
    textures: Vec<Preset>,
    filters: Vec<Preset>,
}

impl Catalog {
    pub(super) fn entries(&self, kind: Kind) -> &[Preset] {
        match kind {
            Kind::Texture => &self.textures,
            Kind::Filter => &self.filters,
        }
    }

    pub(super) fn discover(ticket: &Ticket) -> Result<Option<Self>, String> {
        Self::discover_with_user(None, ticket)
    }

    pub(super) fn discover_with_user(
        user: Option<&Path>,
        ticket: &Ticket,
    ) -> Result<Option<Self>, String> {
        let factory = configured_path("MYCELA_OVERVIEW_FACTORY_PRESETS").or_else(|| {
            env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("Presets")))
                .filter(|candidate| candidate.is_dir())
        });
        let user = user.map(Path::to_owned).or_else(|| {
            configured_path("MYCELA_OVERVIEW_PRESETS").or_else(|| {
                dirs::data_dir()
                    .map(|directory| directory.join("Mycela/Presets"))
                    .filter(|candidate| candidate.is_dir())
            })
        });
        Self::read(factory.as_deref(), user.as_deref(), ticket)
    }

    pub(super) fn read(
        factory: Option<&Path>,
        user: Option<&Path>,
        ticket: &Ticket,
    ) -> Result<Option<Self>, String> {
        let mut catalog = Self::default();
        let mut opened = HashSet::new();
        let mut seen = HashSet::new();
        for (root, factory) in factory
            .map(|path| (path, true))
            .into_iter()
            .chain(user.map(|path| (path, false)))
        {
            if !ticket.is_current() {
                return Ok(None);
            }
            let mut root = root.canonicalize().map_err(|error| {
                format!(
                    "Couldn’t open the preset folder {}. {error}",
                    root.display()
                )
            })?;
            let presets = root.join("Presets");
            if !typed_folders(&root).any(|(_, path)| path.is_dir())
                && typed_folders(&presets).any(|(_, path)| path.is_dir())
            {
                root = presets.canonicalize().map_err(model::failure)?;
            }
            if !opened.insert(root.clone()) {
                continue;
            }
            let mut typed = false;
            for (kind, folder) in typed_folders(&root) {
                match fs::symlink_metadata(&folder) {
                    Ok(metadata) if metadata.file_type().is_dir() => {
                        typed = true;
                        catalog.scan(&folder, Some(kind), &mut seen, ticket)?;
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(model::failure(error)),
                }
            }
            if !typed {
                if factory {
                    return Err(format!(
                        "Choose a Genetica preset folder with Texture or Filter folders. Current folder: {}",
                        root.display()
                    ));
                }
                catalog.scan(&root, None, &mut seen, ticket)?;
            }
        }
        if !ticket.is_current() {
            return Ok(None);
        }
        for entries in [&mut catalog.textures, &mut catalog.filters] {
            entries.sort_by_cached_key(|preset| {
                let categories = preset
                    .categories
                    .iter()
                    .enumerate()
                    .map(|(depth, category)| {
                        let rank = match (depth, category.as_str()) {
                            (0, "Miscellaneous") => 1,
                            (0, "Legacy Presets") => 2,
                            _ => 0,
                        };
                        (rank, category.to_lowercase())
                    })
                    .collect::<Vec<_>>();
                (categories, preset.name.to_lowercase(), preset.path.clone())
            });
        }
        Ok(Some(catalog))
    }

    fn scan(
        &mut self,
        root: &Path,
        kind: Option<Kind>,
        seen: &mut HashSet<PathBuf>,
        ticket: &Ticket,
    ) -> Result<(), String> {
        let mut folders = vec![(root.to_owned(), Vec::<String>::new())];
        while let Some((folder, categories)) = folders.pop() {
            for entry in fs::read_dir(&folder).map_err(|error| {
                format!("Couldn’t browse presets in {}. {error}", folder.display())
            })? {
                if !ticket.is_current() {
                    return Ok(());
                }
                let entry = entry.map_err(model::failure)?;
                let file_type = entry.file_type().map_err(model::failure)?;
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let path = entry.path();
                if file_type.is_dir() {
                    let mut nested = categories.clone();
                    nested.push(entry.file_name().to_string_lossy().into_owned());
                    folders.push((path, nested));
                    continue;
                }
                if !file_type.is_file() {
                    continue;
                }
                let Some(format) = format(&path) else {
                    continue;
                };
                let Some(kind) = kind.or_else(|| classify(&path, format)) else {
                    continue;
                };
                let Some(name) = path.file_stem() else {
                    continue;
                };
                let name = name.to_string_lossy().into_owned();
                if !seen.insert(path.clone()) {
                    continue;
                }
                let search_text = format!("{} {}", categories.join(" "), name).to_lowercase();
                let preset = Preset {
                    path,
                    format,
                    name,
                    categories: categories.clone(),
                    search_text,
                };
                match kind {
                    Kind::Texture => self.textures.push(preset),
                    Kind::Filter => self.filters.push(preset),
                }
            }
        }
        Ok(())
    }
}

fn configured_path(variable: &str) -> Option<PathBuf> {
    env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn typed_folders(root: &Path) -> impl Iterator<Item = (Kind, PathBuf)> + '_ {
    [Kind::Texture, Kind::Filter]
        .into_iter()
        .map(|kind| (kind, root.join(kind.directory())))
}

fn format(path: &Path) -> Option<DocumentFormat> {
    let extension = path.extension()?;
    [DocumentFormat::Gtx, DocumentFormat::Mycela]
        .into_iter()
        .find(|format| extension.eq_ignore_ascii_case(format.extension()))
}

fn classify(path: &Path, format: DocumentFormat) -> Option<Kind> {
    // A folder without an explicit library type follows the original's GTX
    // filter input count. Multiple previews identify the animation library.
    let header = header(path, format).ok()?;
    if header.filter_inputs() > 0 {
        Some(Kind::Filter)
    } else {
        (header.previews().len() <= 1).then_some(Kind::Texture)
    }
}

fn header(path: &Path, format: DocumentFormat) -> Result<Gtx375Header, String> {
    if !fs::symlink_metadata(path)
        .map_err(model::failure)?
        .is_file()
    {
        return Err("Choose a regular preset file.".into());
    }
    let file = File::open(path).map_err(model::failure)?;
    if !file.metadata().map_err(model::failure)?.is_file() {
        return Err("Choose a regular preset file.".into());
    }
    let mut prefix = Vec::new();
    file.take(PRESET_HEADER_LIMIT)
        .read_to_end(&mut prefix)
        .map_err(model::failure)?;
    let mut reader = BinaryCursor::new(&prefix).map_err(model::failure)?;
    // Supply the bounded stream length to disallow oversized preview allocations.
    match format {
        DocumentFormat::Gtx => Gtx375Header::read(&mut reader, prefix.len()),
        DocumentFormat::Mycela => Gtx375Header::read_project(&mut reader, prefix.len()),
    }
    .map_err(model::failure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycela_engine::BinaryWriter;

    fn stock_folder() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../genetica/Presets")
    }

    #[test]
    fn catalog_preserves_installed_categories_and_reads_actual_presets() {
        let ticket = Ticket::new();
        let stock = stock_folder();
        let catalog = Catalog::read(Some(&stock), Some(&stock), &ticket)
            .unwrap()
            .unwrap();
        let matches = |kind: Kind, query: &str| {
            catalog
                .entries(kind)
                .iter()
                .filter(|entry| entry.matches(query))
                .collect::<Vec<_>>()
        };
        let textures = matches(Kind::Texture, "MISCELLANEOUS Leather");
        let leather = textures
            .into_iter()
            .find(|entry| entry.categories() == ["Miscellaneous"])
            .unwrap();
        assert_eq!(leather.name(), "Leather");
        assert!(
            leather
                .path()
                .ends_with("Texture/Miscellaneous/Leather.gtx")
        );
        assert!(leather.thumbnail().unwrap().is_some());
        assert!(leather.read(&ticket).unwrap().unwrap().branches()[0].is_some());

        let filters = matches(Kind::Filter, "cracked photo emboss");
        let [cracked_photo] = filters.as_slice() else {
            panic!("Expected exactly one Cracked Photo grouped by its native categories.");
        };
        assert_eq!(
            cracked_photo.categories(),
            ["Texture Effect", "Print & Emboss"]
        );
        assert!(cracked_photo.thumbnail().unwrap().is_some());
        let emboss = matches(Kind::Filter, "texture effect emboss")
            .into_iter()
            .find(|entry| entry.name() == "Emboss")
            .unwrap();
        let document = emboss.read(&ticket).unwrap().unwrap();
        assert!(document.header().filter_inputs() > 0);

        ticket.retire();
        assert!(
            Catalog::read(Some(&stock), None, &ticket)
                .unwrap()
                .is_none()
        );
        assert!(emboss.read(&ticket).unwrap().is_none());
    }

    #[test]
    fn ordinary_user_folders_classify_native_projects_and_gtx_filters() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../port/.scratch/mycela-overview-catalog")
            .join(std::process::id().to_string());
        let category = folder.join("Custom");
        fs::create_dir_all(&category).unwrap();

        let node = model::Node::solid_color().unwrap();
        let mut document = model::Document::new(vec![Some(node)]).unwrap();
        let mut bytes = BinaryWriter::default();
        document.save_project(&mut bytes).unwrap();
        fs::write(category.join("My Texture.MYCELA"), bytes.bytes()).unwrap();
        fs::copy(
            stock_folder().join("Filter/Texture Effect/Print & Emboss/Cracked Photo.gtx"),
            category.join("My Filter.GTX"),
        )
        .unwrap();
        fs::write(category.join("Not a preset.gtx"), b"invalid header").unwrap();

        let ticket = Ticket::new();
        let catalog = Catalog::read(None, Some(&folder), &ticket)
            .unwrap()
            .unwrap();
        let [texture] = catalog.entries(Kind::Texture) else {
            panic!("Expected one native texture.");
        };
        let [filter] = catalog.entries(Kind::Filter) else {
            panic!("Expected one GTX filter.");
        };
        assert_eq!(texture.name(), "My Texture");
        assert_eq!(filter.name(), "My Filter");
        assert_eq!(texture.categories(), ["Custom"]);
        assert!(texture.thumbnail().unwrap().is_none());
        assert!(texture.read(&ticket).unwrap().unwrap().branches()[0].is_some());
    }
}
