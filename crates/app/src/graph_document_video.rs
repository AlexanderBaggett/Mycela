//! The video export boundary. Keep intermediate frames and FFmpeg's output in
//! a private directory on the destination filesystem until encoding succeeds.
use super::{
    SequenceUpdate, Ticket, model,
    output::{self, Naming, SequenceSettings},
};
use crate::image_file::Format;
use mycela_engine::{EnvironmentResourceLibrary, ImageGeometry};
use std::{
    fmt,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Codec {
    H264,
    Hevc,
    Vp9,
    Av1,
    Ffv1,
}

impl Codec {
    const ALL: [Self; 5] = [Self::H264, Self::Hevc, Self::Vp9, Self::Av1, Self::Ffv1];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264 · MP4",
            Self::Hevc => "H.265 (HEVC) · MP4",
            Self::Vp9 => "VP9 · WebM",
            Self::Av1 => "AV1 · WebM",
            Self::Ffv1 => "FFV1 (lossless, with alpha) · MKV",
        }
    }

    pub(super) fn extension(self) -> &'static str {
        match self {
            Self::H264 | Self::Hevc => "mp4",
            Self::Vp9 | Self::Av1 => "webm",
            Self::Ffv1 => "mkv",
        }
    }

    fn encoder(self) -> &'static str {
        match self {
            Self::H264 => "libx264",
            Self::Hevc => "libx265",
            Self::Vp9 => "libvpx-vp9",
            Self::Av1 => "libaom-av1",
            Self::Ffv1 => "ffv1",
        }
    }

    fn options(self) -> &'static [&'static str] {
        match self {
            Self::H264 => &[
                "-pix_fmt",
                "yuv420p",
                "-preset",
                "medium",
                "-crf",
                "18",
                "-movflags",
                "+faststart",
            ],
            Self::Hevc => &[
                "-pix_fmt",
                "yuv420p",
                "-preset",
                "medium",
                "-crf",
                "20",
                "-tag:v",
                "hvc1",
                "-movflags",
                "+faststart",
            ],
            Self::Vp9 => &[
                "-pix_fmt",
                "yuv420p",
                "-deadline",
                "good",
                "-cpu-used",
                "4",
                "-crf",
                "24",
                "-b:v",
                "0",
            ],
            Self::Av1 => &[
                "-pix_fmt",
                "yuv420p",
                "-cpu-used",
                "6",
                "-crf",
                "32",
                "-b:v",
                "0",
            ],
            Self::Ffv1 => &["-pix_fmt", "bgra", "-level", "3"],
        }
    }

    pub(super) fn check_geometry(self, geometry: ImageGeometry) -> Result<(), String> {
        if matches!(self, Self::H264 | Self::Hevc)
            && (geometry.width() % 2 != 0 || geometry.height() % 2 != 0)
        {
            return Err("H.264 and H.265 require even width and height. Adjust the resolution or choose another codec.".into());
        }
        if self == Self::Hevc && (geometry.width() < 16 || geometry.height() < 16) {
            return Err("H.265 requires width and height of at least 16 pixels. Adjust the resolution or choose another codec.".into());
        }
        Ok(())
    }
}

pub(super) fn available_codecs() -> Result<Vec<Codec>, String> {
    let result = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .map_err(ffmpeg_error)?;
    if !result.status.success() {
        return Err(
            "FFmpeg could not list its video encoders. Check your FFmpeg installation.".into(),
        );
    }
    let encoders = String::from_utf8_lossy(&result.stdout);
    let supported: Vec<_> = Codec::ALL
        .into_iter()
        .filter(|codec| {
            encoders.lines().any(|line| {
                let mut columns = line.split_whitespace();
                columns.next().is_some_and(|flags| flags.starts_with('V'))
                    && columns.next() == Some(codec.encoder())
            })
        })
        .collect();
    if supported.is_empty() {
        return Err("This FFmpeg installation has no supported video encoders.".into());
    }
    Ok(supported)
}

fn ffmpeg_error(error: io::Error) -> String {
    if error.kind() == io::ErrorKind::NotFound {
        "Install FFmpeg and make sure ffmpeg is on your PATH to export video.".into()
    } else {
        format!("Couldn’t run FFmpeg ({error}).")
    }
}

#[derive(Clone, Copy)]
pub(super) struct FrameRate {
    numerator: u32,
    denominator: u32,
}

impl FrameRate {
    pub(super) const DEFAULT: Self = Self {
        numerator: 30,
        denominator: 1,
    };

    pub(super) fn parse(value: &str) -> Option<Self> {
        let integer = |value: &str| {
            (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| value.parse::<u32>().ok())
                .flatten()
        };
        let value = value.trim();
        let (numerator, denominator) = if let Some((top, bottom)) = value.split_once('/') {
            (integer(top.trim())?, integer(bottom.trim())?)
        } else if let Some((whole, decimals)) = value.split_once('.') {
            if decimals.len() > 3 {
                return None;
            }
            let scale = 10u32.pow(decimals.len() as u32);
            (
                integer(whole)?
                    .checked_mul(scale)?
                    .checked_add(integer(decimals)?)?,
                scale,
            )
        } else {
            (integer(value)?, 1)
        };
        if denominator == 0 || numerator > i32::MAX as u32 || denominator > i32::MAX as u32 {
            return None;
        }
        let rate = f64::from(numerator) / f64::from(denominator);
        (1. ..=240.).contains(&rate).then_some(Self {
            numerator,
            denominator,
        })
    }

    pub(super) fn seconds(self, frames: u32) -> f64 {
        f64::from(frames) * f64::from(self.denominator) / f64::from(self.numerator)
    }
}

impl fmt::Display for FrameRate {
    fn fmt(&self, form: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.denominator == 1 {
            write!(form, "{}", self.numerator)
        } else {
            write!(form, "{}/{}", self.numerator, self.denominator)
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct VideoSettings {
    pub(super) frames: SequenceSettings,
    pub(super) codec: Codec,
    pub(super) rate: FrameRate,
}

pub(super) fn render(
    pending: model::PendingPreview,
    settings: VideoSettings,
    path: PathBuf,
    active: &Ticket,
    stop: &Ticket,
    library: &EnvironmentResourceLibrary,
    progress: impl Fn(SequenceUpdate) + Sync,
) -> Result<Option<PathBuf>, String> {
    let running = || active.is_current() && stop.is_current();
    let path = destination(path, settings.codec)?;
    if !running() {
        return Ok(None);
    }
    let workspace = tempfile::Builder::new()
        .prefix(".mycela-video-")
        .tempdir_in(
            path.parent()
                .filter(|folder| !folder.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
        .map_err(|error| format!("Couldn’t create the temporary frame folder ({error})."))?;
    let sequence = SequenceSettings {
        format: Format::Png,
        effect_maps: false,
        padding: 1,
        start_number: 0,
        naming: Naming::Suffix,
        ..settings.frames
    };
    let report = output::render(
        pending,
        sequence,
        &workspace.path().join("frame.png"),
        active,
        stop,
        library,
        |count| progress(SequenceUpdate::VideoFrames(count)),
    );
    if !running() {
        return Ok(None);
    }
    if let Some(error) = report.error {
        return Err(error);
    }
    if report.completed != sequence.length() {
        return Err("The render ended before all video frames were available.".into());
    }

    progress(SequenceUpdate::VideoEncoding(0));
    if !running() {
        return Ok(None);
    }
    let temp_video = format!("encoded.{}", settings.codec.extension());
    if !encode(workspace.path(), &temp_video, settings, &running, |count| {
        progress(SequenceUpdate::VideoEncoding(count));
    })? {
        return Ok(None);
    }
    let staged = workspace.path().join(temp_video);
    File::options()
        .write(true)
        .open(&staged)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("Couldn’t save the encoded video ({error})."))?;
    if !running() {
        return Ok(None);
    }
    model::publish_new_file(&staged, &path)
        .map_err(|error| format!("Couldn’t save {} ({error}).", path.display()))?;
    Ok(Some(path))
}

fn destination(mut path: PathBuf, codec: Codec) -> Result<PathBuf, String> {
    let expected = codec.extension();
    if path.file_name().is_none() {
        return Err("Choose a video file name.".into());
    }
    if path.extension().is_none() {
        path.set_extension(expected);
    } else if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(expected))
    {
        return Err(format!(
            "Use a .{expected} file name for {}.",
            codec.label()
        ));
    }
    match fs::symlink_metadata(&path) {
        Ok(_) => Err(format!(
            "{} already exists. Choose a different name.",
            path.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path),
        Err(error) => Err(format!("Couldn’t check {} ({error}).", path.display())),
    }
}

fn encode(
    workspace: &Path,
    output_name: &str,
    settings: VideoSettings,
    running: &(impl Fn() -> bool + Sync),
    progress: impl Fn(u32) + Sync,
) -> Result<bool, String> {
    let total = settings.frames.length();
    let padding = total.to_string().len();
    // The image renderer adds this suffix for its Color only output.
    let stem = if settings.frames.color_only {
        "frame - Color Map"
    } else {
        "frame"
    };
    let stderr_path = workspace.join("ffmpeg.log");
    let stderr = File::create(&stderr_path).map_err(model::failure)?;
    let mut child = Command::new("ffmpeg")
        .current_dir(workspace)
        .args([
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-nostats",
            "-stats_period",
            "0.2",
            "-n",
        ])
        .args([
            "-progress",
            "pipe:1",
            "-f",
            "image2",
            "-framerate",
            &settings.rate.to_string(),
        ])
        .args([
            "-start_number",
            "0",
            "-start_number_range",
            "1",
            "-i",
            &format!("{stem}%0{padding}d.png"),
        ])
        .args([
            "-frames:v",
            &total.to_string(),
            "-an",
            "-c:v",
            settings.codec.encoder(),
        ])
        .args(settings.codec.options())
        .arg(output_name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(ffmpeg_error)?;
    let stdout = child.stdout.take().expect("piped FFmpeg progress");
    let mut encoded_frames = 0;
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(frame) = line
                    .strip_prefix("frame=")
                    .and_then(|v| v.trim().parse::<u32>().ok())
                {
                    encoded_frames = frame;
                    progress(frame.min(total));
                }
            }
        });
        loop {
            if !running() {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
            match child.try_wait() {
                Ok(Some(status)) => return Ok(Some(status)),
                Ok(None) => std::thread::sleep(Duration::from_millis(40)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ffmpeg_error(error));
                }
            }
        }
    })?;
    let Some(status) = result else {
        return Ok(false);
    };
    if !status.success() {
        let details = (|| -> io::Result<String> {
            let mut error = File::open(stderr_path)?;
            let start = error.metadata()?.len().saturating_sub(2048);
            error.seek(SeekFrom::Start(start))?;
            let mut bytes = Vec::new();
            error.take(2048).read_to_end(&mut bytes)?;
            Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
        })()
        .unwrap_or_default();
        return Err(if details.is_empty() {
            format!("FFmpeg could not encode the video ({status}).")
        } else {
            format!("FFmpeg could not encode the video. {details}")
        });
    }
    if encoded_frames != total {
        return Err(format!(
            "FFmpeg encoded {encoded_frames} of {total} requested frames."
        ));
    }
    Ok(running())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ForegroundError;
    use mycela_engine::PointwiseNode;

    fn settings(codec: Codec) -> VideoSettings {
        VideoSettings {
            frames: SequenceSettings {
                geometry: ImageGeometry::new(64, 48),
                antialias: false,
                color_only: true,
                effect_maps: false,
                frame_count: 5,
                first: 2,
                last: 4,
                format: Format::Png,
                jpeg_quality: 90,
                padding: 3,
                start_number: 1,
                naming: Naming::Suffix,
            },
            codec,
            rate: FrameRate::parse("30000/1001").unwrap(),
        }
    }

    #[test]
    fn video_rate_supports_decimals_and_broadcast_fractions() {
        let rate = FrameRate::parse("30000/1001").unwrap();
        assert_eq!(rate.to_string(), "30000/1001");
        assert_eq!(rate.seconds(3), 0.1001);
        assert_eq!(FrameRate::parse("29.97").unwrap().seconds(2997), 100.);
        for invalid in [
            "",
            "0",
            "241",
            "30/0",
            "2/3",
            "29.97001",
            "30.1/1",
            "1e2",
            "4294967295/1",
        ] {
            assert!(FrameRate::parse(invalid).is_none(), "{invalid}");
        }
        assert!(
            Codec::H264
                .check_geometry(ImageGeometry::new(63, 48))
                .is_err()
        );
        assert!(
            Codec::Ffv1
                .check_geometry(ImageGeometry::new(63, 48))
                .is_ok()
        );
    }

    #[test]
    fn video_stages_then_encodes_all_available_codecs_at_the_requested_rate() {
        let Ok(codecs) = available_codecs() else {
            return;
        };
        if !Command::new("ffprobe")
            .arg("-version")
            .output()
            .is_ok_and(|result| result.status.success())
        {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let node = PointwiseNode::<ForegroundError>::solid_color().unwrap();
        node.set_solid_color([0.25, 0.5, 0.75]).unwrap();
        node.set_solid_transparency(0.375).unwrap();
        let library = EnvironmentResourceLibrary::default();
        for codec in codecs {
            let settings = settings(codec);
            let destination = workspace.path().join(format!(
                "sequence {}.{}",
                codec.encoder(),
                codec.extension()
            ));
            let updates = std::sync::Mutex::new(Vec::new());
            let rendered = render(
                model::freeze(&node, 0.).unwrap(),
                settings,
                destination.clone(),
                &Ticket::new(),
                &Ticket::new(),
                &library,
                |event| {
                    updates.lock().unwrap().push(event);
                },
            )
            .unwrap_or_else(|error| panic!("{}: {error}", codec.label()));
            assert_eq!(rendered, Some(destination.clone()));
            let updates = updates.into_inner().unwrap();
            assert!(matches!(
                updates.as_slice(),
                [
                    SequenceUpdate::VideoFrames(1),
                    SequenceUpdate::VideoFrames(2),
                    SequenceUpdate::VideoFrames(3),
                    SequenceUpdate::VideoEncoding(0),
                    ..
                ]
            ));
            assert!(matches!(
                updates.last(),
                Some(SequenceUpdate::VideoEncoding(3))
            ));
            let probe = Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-count_frames",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=width,height,r_frame_rate,nb_read_frames",
                    "-of",
                    "default=nw=1",
                ])
                .arg(&destination)
                .output()
                .unwrap();
            assert!(probe.status.success(), "{:?}", probe.stderr);
            let output = String::from_utf8(probe.stdout).unwrap();
            for expected in [
                "width=64",
                "height=48",
                "nb_read_frames=3",
                "r_frame_rate=30000/1001",
            ] {
                assert!(
                    output.lines().any(|line| line == expected),
                    "{}: {output}",
                    codec.label()
                );
            }
            if codec == Codec::Ffv1 {
                let pixels = Command::new("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(&destination)
                    .args(["-frames:v", "1", "-pix_fmt", "rgba", "-f", "rawvideo", "-"])
                    .output()
                    .unwrap();
                assert!(pixels.status.success());
                assert_eq!(&pixels.stdout[..4], &[64, 128, 191, 159]);
            }
        }
        assert!(
            fs::read_dir(workspace.path())
                .unwrap()
                .all(|entry| entry.unwrap().path().is_file())
        );
    }

    #[test]
    fn video_cancel_cleans_frames_and_collision_preserves_existing_output() {
        let workspace = tempfile::tempdir().unwrap();
        let node = PointwiseNode::<ForegroundError>::solid_color().unwrap();
        let path = workspace.path().join("cancel.mkv");
        let settings = settings(Codec::Ffv1);
        let stop = Ticket::new();
        let create = |stop: &Ticket| {
            render(
                model::freeze(&node, 0.).unwrap(),
                settings,
                path.clone(),
                &Ticket::new(),
                stop,
                &EnvironmentResourceLibrary::default(),
                |event| {
                    if matches!(event, SequenceUpdate::VideoEncoding(0)) {
                        stop.retire();
                    }
                },
            )
        };
        assert_eq!(create(&stop), Ok(None));
        assert_eq!(fs::read_dir(workspace.path()).unwrap().count(), 0);
        fs::write(&path, b"existing video").unwrap();
        assert!(
            create(&Ticket::new())
                .unwrap_err()
                .contains("already exists")
        );
        assert_eq!(fs::read(&path).unwrap(), b"existing video");
        assert_eq!(fs::read_dir(workspace.path()).unwrap().count(), 1);
    }
}
