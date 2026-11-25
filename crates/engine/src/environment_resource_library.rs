//! Read-only local Resource Library lookup and size selection (wg, ara, nj).
use super::EnvironmentImage;
use crate::pointwise::{PointwiseError, image_resize};
use crate::{ImageGeometry, ProceduralExecution, SharedImageMap};
use std::{
    collections::VecDeque,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

const PREVIEW_WIDTH: i32 = 512;
const TIERS: usize = 6;
const CACHE_LIMIT: usize = 20;

/// A caller-selected directory. Library configuration and decode caches stay
/// outside GTX values and can be shared by detached render requests.
#[derive(Clone, Debug, Default)]
pub struct EnvironmentResourceLibrary(Option<Arc<Directory>>);

#[derive(Debug)]
struct Directory {
    root: PathBuf,
    images: Mutex<VecDeque<CachedImage>>,
}

#[derive(Debug)]
struct CachedImage {
    path: PathBuf,
    modified: Option<SystemTime>,
    length: u64,
    image: Arc<EnvironmentImage>,
}

impl EnvironmentResourceLibrary {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let root = path.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Choose a library folder",
            ));
        }
        Ok(Self(Some(Arc::new(Directory {
            root,
            images: Mutex::new(VecDeque::new()),
        }))))
    }

    pub fn root(&self) -> Option<&Path> {
        self.0.as_ref().map(|directory| directory.root.as_path())
    }

    pub(super) fn resolve<E>(
        &self,
        path: &[u16],
        extension: &[u16],
        size: [i32; 2],
        width: i32,
        execution: ProceduralExecution,
    ) -> Result<Option<SharedImageMap>, PointwiseError<E>> {
        let Some(directory) = &self.0 else {
            return Ok(None);
        };
        let Some(asset) = Asset::new(path, extension, size) else {
            return Ok(None);
        };
        let Some(paths) = directory.variants(&asset) else {
            return Ok(None);
        };
        let Some((source, synthesized, selected_width)) = asset.choose(&paths, width) else {
            return Ok(None);
        };
        let Some(image) = directory.load(source, &asset.extension) else {
            return Ok(None);
        };
        let Ok(decoded) = image.decoded() else {
            return Ok(None);
        };
        let mut image = decoded.clone();
        if let Some(dimensions) = synthesized {
            image = asset.resize::<E>(&image, dimensions, execution)?;
        }
        if width < selected_width {
            let actual = image.geometry();
            let height = proportional_height(actual.width(), actual.height(), width);
            let Some(height) = height else {
                return Ok(None);
            };
            image = asset.resize::<E>(&image, ImageGeometry::new(width, height), execution)?;
        }
        Ok(Some(image))
    }

    /// The photo editor opens the highest locally available tier, including a
    /// preview-only asset. Its choice does not depend on the output resolution.
    pub(super) fn editor_source(
        &self,
        path: &[u16],
        extension: &[u16],
        size: [i32; 2],
    ) -> Option<SharedImageMap> {
        let directory = self.0.as_ref()?;
        let asset = Asset::new(path, extension, size)?;
        let paths = directory.variants(&asset)?;
        let highest = Asset::highest_available_tier(&paths)?;
        let image = directory.load(paths[highest].as_deref()?, &asset.extension)?;
        image.decoded().ok().cloned()
    }
}

struct Asset {
    components: Vec<String>,
    extension: String,
    size: [i32; 2],
    environment: bool,
}

impl Asset {
    fn new(path: &[u16], extension: &[u16], size: [i32; 2]) -> Option<Self> {
        let path = String::from_utf16(path).ok()?;
        let extension = String::from_utf16(extension).ok()?;
        if !matches!(
            extension.to_ascii_lowercase().as_str(),
            ".jpg" | ".jpeg" | ".hdr"
        ) || size.iter().any(|&side| side <= 0)
        {
            return None;
        }
        let components: Vec<_> = path
            .split(['\\', '/'])
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if components.is_empty()
            || components
                .iter()
                .any(|part| matches!(part.as_str(), "." | "..") || part.contains([':', '\0']))
        {
            return None;
        }
        let environment = !components[0].eq_ignore_ascii_case("Texture Photo");
        Some(Self {
            components,
            extension,
            size,
            environment,
        })
    }

    fn dimensions(&self, tier: usize) -> Option<ImageGeometry> {
        let [width, height] = self.size;
        if tier == 0 {
            return Some(ImageGeometry::new(
                PREVIEW_WIDTH,
                proportional_height(width, height, PREVIEW_WIDTH)?,
            ));
        }
        let [width, height] = [width >> (tier - 1), height >> (tier - 1)];
        (width > 0 && height > 0).then(|| ImageGeometry::new(width, height))
    }

    fn requested_tier(&self, width: i32) -> usize {
        if self.size[0] < PREVIEW_WIDTH || width <= PREVIEW_WIDTH {
            return 0;
        }
        let mut tier = 1;
        let mut next = self.size[0] / 2;
        while next >= width {
            tier += 1;
            next /= 2;
        }
        tier
    }

    fn choose<'a>(
        &self,
        paths: &'a [Option<PathBuf>; TIERS],
        width: i32,
    ) -> Option<(&'a Path, Option<ImageGeometry>, i32)> {
        let highest = Self::highest_available_tier(paths)?;
        let highest_width = self.dimensions(highest)?.width();
        if highest_width < width {
            return (highest != 0)
                .then(|| (paths[highest].as_deref().unwrap(), None, highest_width));
        }
        let mut tier = self.requested_tier(width);
        let dimensions = self.dimensions(tier)?;
        if paths.get(tier).is_none_or(Option::is_none)
            && highest > 0
            && (tier > highest || tier == 0)
        {
            if tier == 0 || dimensions.width() >= PREVIEW_WIDTH * 3 / 2 {
                return Some((
                    paths[highest].as_deref()?,
                    Some(dimensions),
                    dimensions.width(),
                ));
            }
            tier = highest;
        }
        let nominal_width = self.dimensions(tier)?.width();
        if tier > 1
            && nominal_width > width
            && f64::from(nominal_width - width) / f64::from(width) < 0.25
            && paths.get(tier - 1).is_some_and(Option::is_some)
        {
            tier -= 1;
        }
        Some((paths.get(tier)?.as_deref()?, None, nominal_width))
    }

    fn highest_available_tier(paths: &[Option<PathBuf>; TIERS]) -> Option<usize> {
        (1..TIERS).chain([0]).find(|&tier| paths[tier].is_some())
    }

    fn resize<E>(
        &self,
        image: &SharedImageMap,
        geometry: ImageGeometry,
        execution: ProceduralExecution,
    ) -> Result<SharedImageMap, PointwiseError<E>> {
        let resize = if self.environment {
            image_resize::resize_clamped
        } else {
            image_resize::resize
        };
        resize(image, geometry.width(), geometry.height(), execution)
    }
}

fn proportional_height(width: i32, height: i32, requested: i32) -> Option<i32> {
    let height = i32::try_from(i64::from(requested) * i64::from(height) / i64::from(width)).ok()?;
    (height > 0).then_some(height)
}

impl Directory {
    fn variants(&self, asset: &Asset) -> Option<[Option<PathBuf>; TIERS]> {
        let (stem, parents) = asset.components.split_last()?;
        let mut directory = self.root.clone();
        for component in parents {
            directory = self.child(&directory, component)?;
            if !directory.is_dir() {
                return None;
            }
        }
        Some(std::array::from_fn(|tier| {
            let size = asset.dimensions(tier)?;
            let suffix = if tier == 0 {
                "[Preview]".to_owned()
            } else {
                format!("[Res{tier}-{}x{}]", size.width(), size.height())
            };
            self.child(&directory, &format!("{stem}{suffix}{}", asset.extension))
                .filter(|path| path.is_file())
        }))
    }

    fn child(&self, parent: &Path, name: &str) -> Option<PathBuf> {
        let exact = parent.join(name);
        let canonical = match exact.canonicalize() {
            Ok(path) => path,
            Err(_) => {
                let mut matches = fs::read_dir(parent).ok()?.flatten().filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
                });
                let candidate = matches.next()?.path();
                if matches.next().is_some() {
                    return None;
                }
                candidate.canonicalize().ok()?
            }
        };
        canonical.starts_with(&self.root).then_some(canonical)
    }

    fn load(&self, path: &Path, extension: &str) -> Option<Arc<EnvironmentImage>> {
        let metadata = path.metadata().ok()?;
        let modified = metadata.modified().ok();
        let length = metadata.len();
        {
            let mut images = self.images.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(index) = images.iter().position(|entry| entry.path == path) {
                let entry = images.remove(index)?;
                if entry.modified == modified && entry.length == length {
                    let image = entry.image.clone();
                    images.push_back(entry);
                    return Some(image);
                }
            }
        }
        let bytes = fs::read(path).ok()?;
        let image = Arc::new(EnvironmentImage::from_file_bytes(bytes, &extension[1..]).ok()?);
        // Decoding and filesystem access take place outside the cache lock.
        let mut images = self.images.lock().unwrap_or_else(|e| e.into_inner());
        images.retain(|entry| entry.path != path);
        if images.len() == CACHE_LIMIT {
            images.pop_front();
        }
        images.push_back(CachedImage {
            path: path.to_owned(),
            modified,
            length,
            image: image.clone(),
        });
        Some(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(size: [i32; 2]) -> Asset {
        Asset::new(
            &r"\Environment Map\Outdoor\Fixture"
                .encode_utf16()
                .collect::<Vec<_>>(),
            &".HDR".encode_utf16().collect::<Vec<_>>(),
            size,
        )
        .unwrap()
    }

    #[test]
    fn resource_tier_selection_uses_native_limits_preview_and_nearby_higher_tiers() {
        let asset = asset([4096, 2048]);
        let mut files = std::array::from_fn(|_| None);
        files[0] = Some("preview".into());
        assert_eq!(Asset::highest_available_tier(&files), Some(0));
        assert_eq!(
            asset.choose(&files, 512),
            Some((Path::new("preview"), None, 512))
        );
        assert!(asset.choose(&files, 513).is_none());

        files[0] = None;
        files[1] = Some("full".into());
        files[2] = Some("half".into());
        files[3] = Some("quarter".into());
        assert_eq!(Asset::highest_available_tier(&files), Some(1));
        assert_eq!(
            asset.choose(&files, 512),
            Some((Path::new("full"), Some(ImageGeometry::new(512, 256)), 512))
        );
        assert_eq!(
            asset.choose(&files, 769),
            Some((Path::new("quarter"), None, 1024))
        );
        assert_eq!(
            asset.choose(&files, 900),
            Some((Path::new("half"), None, 1024))
        );
        assert_eq!(
            asset.choose(&files, 5000),
            Some((Path::new("full"), None, 4096))
        );
    }

    #[test]
    fn malformed_library_references_cannot_select_files_outside_the_chosen_directory() {
        for path in [r"C:\Photos\Image", r"\Environment Map\..\outside", ".", ""] {
            assert!(
                Asset::new(
                    &path.encode_utf16().collect::<Vec<_>>(),
                    &".jpg".encode_utf16().collect::<Vec<_>>(),
                    [2048, 1024],
                )
                .is_none(),
                "{path}"
            );
        }
    }
}
