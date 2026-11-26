//! Final-quality animation frames streamed directly to disk.
use super::{Ticket, model};
use crate::{
    image_file::{Format, write_hdr, write_standard},
    scene_backend::{PreviewDevices, render_sequence_frame},
    solid_color::{
        export_settings::checked_dimensions,
        map_preview::{self, PreviewSelection},
    },
};
use mycela_engine::{
    EnvironmentResourceLibrary, ImageGeometry, PointwiseRenderOptions, SharedImageMap,
};
use std::{
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Naming {
    Suffix,
    DottedSuffix,
    Extensionless,
    DottedExtensionless,
}

impl Naming {
    pub(super) const ALL: [Self; 4] = [
        Self::Suffix,
        Self::DottedSuffix,
        Self::Extensionless,
        Self::DottedExtensionless,
    ];

    pub(super) fn filename(
        self,
        stem: &str,
        index: u32,
        padding: usize,
        extension: &str,
    ) -> String {
        match self {
            Self::Suffix => format!("{stem}{index:0padding$}.{extension}"),
            Self::DottedSuffix => format!("{stem}.{index:0padding$}.{extension}"),
            Self::Extensionless => format!("{stem}{index:0padding$}"),
            Self::DottedExtensionless => format!("{stem}.{index:0padding$}"),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct SequenceSettings {
    pub(super) geometry: ImageGeometry,
    pub(super) antialias: bool,
    pub(super) color_only: bool,
    pub(super) effect_maps: bool,
    pub(super) frame_count: u32,
    pub(super) first: u32,
    pub(super) last: u32,
    pub(super) format: Format,
    pub(super) jpeg_quality: u8,
    pub(super) padding: usize,
    pub(super) start_number: u32,
    pub(super) naming: Naming,
}

impl SequenceSettings {
    pub(super) fn length(self) -> u32 {
        self.last - self.first + 1
    }

    fn sample_time(self, frame: u32) -> f64 {
        f64::from(frame - 1) / f64::from(self.frame_count)
    }

    fn supersampled_geometry(self) -> Result<ImageGeometry, String> {
        let (width, height) = (self.geometry.width(), self.geometry.height());
        if !self.antialias {
            return Ok(self.geometry);
        }
        // Follow the reference renderer's 2× samples with a 1.5× scale on
        // larger frames. Use a common factor to preserve the aspect ratio.
        let scale = |side: i32| {
            if width.max(height) <= 2048 {
                side.checked_mul(2)
            } else {
                side.checked_add(side / 2)
            }
            .ok_or("The supersampled dimensions are too large.")
        };
        let (width, height) = checked_dimensions(scale(width)?, scale(height)?)?;
        Ok(ImageGeometry::new(width, height))
    }

    pub(super) fn validate(self) -> Result<Self, String> {
        self.supersampled_geometry()?;
        Ok(self)
    }

    fn filename(self, stem: &str, offset: u32, extension: &str) -> String {
        let padding = self.padding.max(self.length().to_string().len());
        self.naming
            .filename(stem, self.start_number + offset, padding, extension)
    }
}

struct Destination {
    directory: PathBuf,
    stem: String,
    extension: String,
}

impl Destination {
    fn new(path: &Path, format: Format) -> Result<Self, String> {
        let expected = format.extension();
        let extension = match path.extension() {
            None => expected,
            Some(ext) => ext
                .to_str()
                .filter(|ext| {
                    ext.eq_ignore_ascii_case(expected)
                        || format == Format::Jpeg && ext.eq_ignore_ascii_case("jpeg")
                        || format == Format::Tiff && ext.eq_ignore_ascii_case("tif")
                })
                .ok_or_else(|| format!("Use a .{expected} file name for {}.", format.label()))?,
        };
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty() && !name.contains(['\r', '\n']))
            .ok_or("Choose a sequence name on one line.")?;
        let directory = path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        Ok(Self {
            directory: directory.into(),
            stem: stem.into(),
            extension: extension.into(),
        })
    }
}

struct Track {
    selection: PreviewSelection,
    stem: String,
    frames: Vec<String>,
}

impl Track {
    fn list_path(&self, destination: &Destination) -> PathBuf {
        destination.directory.join(format!("{}.ifl", self.stem))
    }

    fn check_available(
        &self,
        destination: &Destination,
        settings: SequenceSettings,
        active: &Ticket,
    ) -> Result<(), String> {
        let paths = std::iter::once(self.list_path(destination)).chain((0..settings.length()).map(
            |offset| {
                destination.directory.join(settings.filename(
                    &self.stem,
                    offset,
                    &destination.extension,
                ))
            },
        ));
        for path in paths.take_while(|_| active.is_current()) {
            match fs::symlink_metadata(&path) {
                Ok(_) => {
                    return Err(format!(
                        "{} already exists. Choose a different sequence name or folder.",
                        path.display()
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("Couldn’t check {} ({error}).", path.display())),
            }
        }
        Ok(())
    }
}

pub(super) struct SequenceReport {
    pub(super) completed: u32,
    pub(super) error: Option<String>,
}

pub(super) fn render(
    pending: model::PendingPreview,
    settings: SequenceSettings,
    path: &Path,
    active: &Ticket,
    stop: &Ticket,
    library: &EnvironmentResourceLibrary,
    progress: impl Fn(u32),
) -> SequenceReport {
    let mut completed = 0;
    let run = || {
        let destination = Destination::new(path, settings.format)?;
        let color_stem = if settings.color_only {
            format!("{} - Color Map", destination.stem)
        } else {
            destination.stem.clone()
        };
        let mut tracks = vec![Track {
            selection: PreviewSelection::BaseColor,
            stem: color_stem,
            frames: Vec::new(),
        }];
        tracks[0].check_available(&destination, settings, active)?;
        let mut renderer = pending
            .materialize()?
            .prepare_sequence(
                settings.supersampled_geometry()?,
                PointwiseRenderOptions::new(settings.color_only, settings.effect_maps),
            )
            .map_err(model::failure)?;
        let mut devices = PreviewDevices::default();
        let stopped = || !active.is_current() || !stop.is_current();
        let result = (|| -> Result<(), String> {
            for frame in settings.first..=settings.last {
                if stopped() {
                    break;
                }
                let image = match render_sequence_frame(
                    &mut renderer,
                    settings.sample_time(frame),
                    settings.geometry,
                    &mut devices,
                    &stopped,
                    library,
                ) {
                    Ok(image) => image,
                    Err(error) if error.is_cancelled() => break,
                    Err(error) => return Err(format!("Frame {frame} could not render ({error}).")),
                };
                let offset = frame - settings.first;
                if !write_frame(&image, &destination, &mut tracks, settings, offset, active)
                    .map_err(|error| format!("Frame {frame} could not be saved. {error}"))?
                {
                    break;
                }
                completed += 1;
                progress(completed);
            }
            Ok(())
        })();
        let lists = write_lists(&destination, &tracks, active);
        result.and(lists)
    };
    let error = run().err();
    SequenceReport { completed, error }
}

fn safe_map_name(name: &[u16]) -> String {
    let sanitized = String::from_utf16_lossy(name)
        .chars()
        .map(|ch| {
            if ch.is_control() || "/\\:*?\"<>|".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>();
    let name = sanitized.trim();
    if name.is_empty() {
        "Unnamed map".into()
    } else {
        name.into()
    }
}

fn write_frame(
    image: &SharedImageMap,
    destination: &Destination,
    tracks: &mut Vec<Track>,
    settings: SequenceSettings,
    offset: u32,
    active: &Ticket,
) -> Result<bool, String> {
    let selections = if settings.effect_maps {
        map_preview::choices(image)
    } else {
        vec![PreviewSelection::BaseColor]
    };
    let mut indices = Vec::with_capacity(selections.len());
    for selection in selections {
        let index =
            if let Some(index) = tracks.iter().position(|track| track.selection == selection) {
                index
            } else {
                let map = safe_map_name(selection.name().expect("named map"));
                let base = format!("{} - {map}", destination.stem);
                let mut stem = base.clone();
                let mut suffix = 2;
                while tracks
                    .iter()
                    .any(|track| track.stem.eq_ignore_ascii_case(&stem))
                {
                    stem = format!("{base} {suffix}");
                    suffix += 1;
                }
                let track = Track {
                    selection,
                    stem,
                    frames: Vec::new(),
                };
                track.check_available(destination, settings, active)?;
                tracks.push(track);
                tracks.len() - 1
            };
        indices.push(index);
    }
    for index in indices {
        let track = &mut tracks[index];
        let name = settings.filename(&track.stem, offset, &destination.extension);
        let path = destination.directory.join(&name);
        let written = model::write_new_file(path, active, |file| {
            let projected;
            let image = match &track.selection {
                PreviewSelection::BaseColor => image,
                selection => {
                    let mut map =
                        map_preview::project_image(image, selection).map_err(model::failure)?;
                    // Effect maps are opaque grayscale data images.
                    map.set_constant(3, 0.).map_err(model::failure)?;
                    projected = SharedImageMap::from_image(map);
                    &projected
                }
            };
            let mut writer = BufWriter::new(file);
            match settings.format {
                Format::Pfm | Format::Hdr => {
                    write_hdr(image, settings.format, &mut writer, &|| {
                        !active.is_current()
                    })?;
                }
                format => write_standard(image, format, &mut writer, settings.jpeg_quality)?,
            }
            writer.flush().map_err(model::failure)
        })?;
        if written.is_none() {
            return Ok(false);
        }
        track.frames.push(name);
    }
    Ok(true)
}

fn write_lists(destination: &Destination, tracks: &[Track], active: &Ticket) -> Result<(), String> {
    let mut result = Ok(());
    for track in tracks.iter().filter(|track| !track.frames.is_empty()) {
        let saved = model::write_new_file(track.list_path(destination), active, |file| {
            let mut writer = BufWriter::new(file);
            writer
                .write_all(track.frames.join("\r\n").as_bytes())
                .map_err(model::failure)?;
            writer.flush().map_err(model::failure)
        });
        result = result.and(saved.map(|_| ()));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ForegroundError;
    use mycela_engine::{
        AnalyticCurveKind, CurveLayerHandle, PointwiseFilter, PointwiseKind, PointwiseNode,
        SolidColorProperty,
    };
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_DIRECTORY: AtomicU32 = AtomicU32::new(0);

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mycela-sequence-test-{}-{serial}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn config() -> SequenceSettings {
        SequenceSettings {
            geometry: ImageGeometry::new(7, 5),
            antialias: false,
            color_only: false,
            effect_maps: false,
            frame_count: 5,
            first: 2,
            last: 4,
            format: Format::Png,
            jpeg_quality: 90,
            padding: 3,
            start_number: 0,
            naming: Naming::Suffix,
        }
    }

    fn animated_alpha() -> PointwiseNode<ForegroundError> {
        let node = PointwiseNode::solid_color().unwrap();
        node.set_solid_color([0.5, 0.25, 0.75]).unwrap();
        let property = SolidColorProperty::Transparency;
        let animation = node.solid_animation(property).unwrap();
        animation.ensure_curve(0).unwrap();
        let curve = animation.curves().unwrap().value(0).unwrap().unwrap();
        curve.set_minimum(0.);
        curve.set_maximum(1.);
        curve.append(&[Some(CurveLayerHandle::new(AnalyticCurveKind::Linear))]);
        animation.flags().unwrap().set(0, true).unwrap();
        node.set_solid_animation(property, Some(animation)).unwrap();
        node
    }

    fn pixel(path: &Path) -> [u8; 4] {
        let image = image::open(path).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (7, 5));
        image.get_pixel(3, 2).0
    }

    #[test]
    fn sequence_partial_range_samples_the_full_period_and_writes_ordered_ifl() {
        let dir = Directory::new();
        let node = animated_alpha();
        let settings = config();
        assert_eq!(
            (settings.first..=settings.last)
                .map(|frame| settings.sample_time(frame))
                .collect::<Vec<_>>(),
            [0.2, 0.4, 0.6],
        );
        let library = EnvironmentResourceLibrary::default();
        let report = render(
            model::freeze(&node, 0.).unwrap(),
            settings,
            &dir.0.join("ramp.png"),
            &Ticket::new(),
            &Ticket::new(),
            &library,
            |_| {},
        );
        assert_eq!(report.error, None);
        assert_eq!(report.completed, 3);
        assert_eq!(
            fs::read(dir.0.join("ramp.ifl")).unwrap(),
            b"ramp000.png\r\nramp001.png\r\nramp002.png",
        );
        for (frame, name) in (2..=4).zip(["ramp000.png", "ramp001.png", "ramp002.png"]) {
            let expected = crate::scene_backend::render_export(
                &model::freeze(&node, settings.sample_time(frame))
                    .unwrap()
                    .materialize()
                    .unwrap(),
                settings.geometry,
                &|| false,
                PointwiseRenderOptions::new(false, false),
                None,
                None,
                Some(&library),
            )
            .unwrap();
            let bitmap =
                map_preview::project_bitmap(&expected, &PreviewSelection::BaseColor).unwrap();
            let rgba = crate::solid_color::preview_export::PreviewPixels::capture_rendered(&bitmap)
                .unwrap()
                .into_rgba();
            assert_eq!(&pixel(&dir.0.join(name)), &rgba[..4]);
        }
        assert_ne!(
            pixel(&dir.0.join("ramp000.png")),
            pixel(&dir.0.join("ramp002.png"))
        );
    }

    #[test]
    fn sequence_stop_keeps_completed_antialiased_color_and_maps_and_checks_collisions() {
        let dir = Directory::new();
        let node = animated_alpha();
        let store = PointwiseFilter::new(PointwiseKind::StoreMap).unwrap();
        store
            .parameter(0)
            .unwrap()
            .set_text(Some(&"Height".encode_utf16().collect::<Vec<_>>()))
            .unwrap();
        store.node().slot(0).unwrap().set_node(Some(&node)).unwrap();
        let settings = SequenceSettings {
            antialias: true,
            effect_maps: true,
            color_only: true,
            naming: Naming::DottedSuffix,
            ..config()
        };
        let path = dir.0.join("tile.png");
        let library = EnvironmentResourceLibrary::default();
        let stop = Ticket::new();
        let report = render(
            model::freeze(&store.node(), 0.).unwrap(),
            settings,
            &path,
            &Ticket::new(),
            &stop,
            &library,
            |_| stop.retire(),
        );
        assert_eq!(report.error, None);
        assert_eq!(report.completed, 1);
        let saved = dir.0.join("tile - Color Map.000.png");
        let effect = pixel(&dir.0.join("tile - Height.000.png"));
        assert_eq!(effect[0], effect[1]);
        assert_eq!(effect[1], effect[2]);
        assert_eq!(effect[3], 255);
        assert_ne!(pixel(&saved)[3], 255);
        assert_eq!(
            fs::read(dir.0.join("tile - Height.ifl")).unwrap(),
            b"tile - Height.000.png"
        );
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 4);
        let prior = fs::read(&saved).unwrap();
        let report = render(
            model::freeze(&store.node(), 0.).unwrap(),
            settings,
            &path,
            &Ticket::new(),
            &Ticket::new(),
            &library,
            |_| panic!("existing sequence"),
        );
        assert_eq!(report.completed, 0);
        assert!(report.error.unwrap().contains("already exists"));
        assert_eq!(fs::read(saved).unwrap(), prior);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 4);
    }
}
