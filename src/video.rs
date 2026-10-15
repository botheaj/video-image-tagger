//! Getting a video (downloading it if it's a URL) and cutting it into still frames.
//! The heavy lifting is done by two external programs: yt-dlp and ffmpeg/ffprobe.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::images::fit_size;

/// One still image cut from the video.
pub struct Frame {
    /// 0-based position among the sampled frames.
    pub index: usize,
    /// Approximate time in the video, in seconds.
    pub seconds: f64,
    pub path: PathBuf,
}

/// Returns a local path to the video: the path itself, or a fresh download for a URL.
pub fn fetch(video: &str, work_dir: &Path) -> Result<PathBuf> {
    if !(video.starts_with("http://") || video.starts_with("https://")) {
        let path = PathBuf::from(video);
        if !path.is_file() {
            bail!("video file not found: {video}");
        }
        return Ok(path);
    }

    eprintln!("Downloading {video}...");
    let output = Command::new("yt-dlp")
        .args(["--no-playlist", "--no-warnings"])
        // Best video-only stream up to 1080p (we don't need audio), else the best combined one.
        .args(["-f", "bv*[height<=1080]/b"])
        .arg("-o")
        .arg(work_dir.join("video.%(ext)s"))
        // Print the final file path once the download is done, so we know the extension.
        .args(["--print", "after_move:filepath"])
        .arg(video)
        .output()
        .context("running yt-dlp (is it installed?)")?;
    if !output.status.success() {
        bail!(
            "yt-dlp failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        bail!("yt-dlp did not report a downloaded file");
    }
    Ok(PathBuf::from(path))
}

/// Samples `fps` frames per second from the video into JPEG files sized for the image API.
pub fn extract_frames(
    video: &Path,
    fps: f64,
    work_dir: &Path,
    max_frames: usize,
) -> Result<Vec<Frame>> {
    let info = probe(video)?;
    let expected = (info.duration * fps).ceil() as usize;
    if expected > max_frames {
        bail!(
            "this video is {:.0}s long, which at {fps} fps is about {expected} frames (one API request each). \
             Lower --fps or raise --max-frames.",
            info.duration
        );
    }

    let (width, height) = fit_size(info.width, info.height);
    let frames_dir = work_dir.join("frames");
    std::fs::create_dir_all(&frames_dir)?;

    eprintln!("Extracting about {expected} frames ({width}x{height})...");
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(video)
        .args(["-vf", &format!("fps={fps},scale={width}:{height}")])
        // JPEG quality: 2 is best, 31 is worst.
        .args(["-q:v", "3"])
        .arg(frames_dir.join("%05d.jpg"))
        .status()
        .context("running ffmpeg (is it installed?)")?;
    if !status.success() {
        bail!("ffmpeg could not extract frames from {}", video.display());
    }

    // ffmpeg names the files 00001.jpg, 00002.jpg, ... so sorting the names sorts by time.
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&frames_dir)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    paths.sort();
    if paths.is_empty() {
        bail!("ffmpeg produced no frames from {}", video.display());
    }

    let frames = paths
        .into_iter()
        .enumerate()
        .map(|(index, path)| Frame {
            index,
            seconds: index as f64 / fps,
            path,
        })
        .collect();
    Ok(frames)
}

/// The facts about a video we need: its display size and length.
struct VideoInfo {
    width: u32,
    height: u32,
    duration: f64,
}

/// Asks ffprobe for the video's size and duration.
fn probe(video: &Path) -> Result<VideoInfo> {
    // These structs mirror the parts of ffprobe's JSON output we care about;
    // serde fills them in and ignores every other field.
    #[derive(Deserialize)]
    struct Probe {
        streams: Vec<Stream>,
        format: Format,
    }
    #[derive(Deserialize)]
    struct Stream {
        width: u32,
        height: u32,
        #[serde(default)]
        side_data_list: Vec<SideData>,
    }
    #[derive(Deserialize)]
    struct SideData {
        rotation: Option<f64>,
    }
    #[derive(Deserialize)]
    struct Format {
        duration: String,
    }

    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-of", "json"])
        .args([
            "-show_entries",
            "stream=width,height:stream_side_data=rotation:format=duration",
        ])
        .arg(video)
        .output()
        .context("running ffprobe (is it installed?)")?;
    if !output.status.success() {
        bail!(
            "ffprobe could not read {}:\n{}",
            video.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let probe: Probe = serde_json::from_slice(&output.stdout).context("reading ffprobe output")?;
    let stream = probe
        .streams
        .into_iter()
        .next()
        .context("the file has no video stream")?;
    let duration: f64 = probe
        .format
        .duration
        .parse()
        .context("reading the video duration")?;

    // Phone videos are often stored sideways with a "rotate 90°" note. ffmpeg applies
    // that note when it decodes, so swap width and height to match what it will produce.
    let rotated = stream
        .side_data_list
        .iter()
        .filter_map(|s| s.rotation)
        .any(|r| (r.abs() as i64) % 180 == 90);
    let (width, height) = if rotated {
        (stream.height, stream.width)
    } else {
        (stream.width, stream.height)
    };
    Ok(VideoInfo {
        width,
        height,
        duration,
    })
}
