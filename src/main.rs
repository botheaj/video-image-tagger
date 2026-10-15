//! image-reader: find a product from a reference photo in a video, frame by frame,
//! using TypeSafe's Jev model.
//!
//!     ./image-reader reference/starfox.png https://www.youtube.com/shorts/K1WHleqWkJw

// Each `mod` line pulls in a file from src/ (src/video.rs, src/images.rs, ...).
mod images;
mod jev;
mod report;
mod video;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Parser;
use rayon::prelude::*;

/// Command-line arguments. `clap` reads the doc comments below to build `--help`.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Find the product in a reference image within a video, using TypeSafe Jev"
)]
struct Args {
    /// Image of the product to look for (PNG, JPEG, WebP, GIF or BMP)
    reference: PathBuf,

    /// Video file path, or a URL (YouTube or anything else yt-dlp supports)
    video: String,

    /// Frames per second to sample from the video
    #[arg(long, default_value_t = 2.0)]
    fps: f64,

    /// Match probability at or above which a frame counts as a match
    #[arg(long, default_value_t = 0.5)]
    threshold: f64,

    /// Optional words describing the product, e.g. "Star Fox SNES cartridge"
    #[arg(long)]
    describe: Option<String>,

    /// Jev model to use. Box questions need Jev 1.14 or later.
    #[arg(long, default_value = "jev-1.14.0")]
    model: String,

    /// How many frames to send to the API at the same time
    #[arg(long, default_value_t = 4)]
    concurrency: usize,

    /// Refuse to run if the video would produce more frames than this (a cost guard)
    #[arg(long, default_value_t = 500)]
    max_frames: usize,

    /// Folder for outlined and cropped images of matching frames
    #[arg(long, default_value = "output")]
    out: PathBuf,

    /// Print results as JSON instead of a table
    #[arg(long)]
    json: bool,
}

/// What we learned about one sampled frame.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FrameResult {
    pub index: usize,
    pub seconds: f64,
    /// Probability (0 to 1) that the frame shows the reference product.
    pub probability: f64,
    pub is_match: bool,
    /// Where the product is, only kept for matching frames.
    pub product_box: Option<jev::BoxAnswer>,
    /// Paths of the outlined and cropped images, for matching frames.
    pub outlined: Option<PathBuf>,
    pub crop: Option<PathBuf>,
}

fn main() -> ExitCode {
    // Exit codes: 0 = match found, 1 = no match, 2 = something went wrong.
    match run() {
        Ok(true) => ExitCode::from(0),
        Ok(false) => ExitCode::from(1),
        Err(err) => {
            // `{:#}` prints the error together with every `.context(...)` added on the way up.
            eprintln!("error: {err:#}");
            ExitCode::from(2)
        }
    }
}

/// Returns Ok(true) when the product was found in the video.
fn run() -> Result<bool> {
    let args = Args::parse();
    // Load TYPESAFE_API_KEY from a .env file if there is one; ignore it if not.
    dotenvy::dotenv().ok();

    if args.fps <= 0.0 {
        bail!("--fps must be greater than 0");
    }
    let client = jev::Client::from_env(&args.model)?;
    let reference = images::prepare_reference(&args.reference)?;

    // The temporary folder (and everything in it) is deleted when `work` goes out of scope.
    let work = tempfile::tempdir().context("creating a temporary folder")?;
    let video_path = video::fetch(&args.video, work.path())?;
    let frames = video::extract_frames(&video_path, args.fps, work.path(), args.max_frames)?;
    eprintln!(
        "Checking {} frames at {} fps with {}...",
        frames.len(),
        args.fps,
        args.model
    );

    // Ask Jev about every frame, `concurrency` at a time. Collecting into a single
    // `Result` stops at the first error (such as a bad API key) instead of sending every frame.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(args.concurrency.max(1))
        .build()?;
    let answers: Vec<jev::FrameAnswer> = pool.install(|| {
        frames
            .par_iter()
            .map(|frame| {
                let answer = client
                    .check_frame(&reference, &frame.path, args.describe.as_deref())
                    .with_context(|| format!("checking frame at {:.1}s", frame.seconds));
                eprint!(".");
                answer
            })
            .collect::<Result<_>>()
    })?;
    eprintln!();

    // Keep each video's images in its own folder, e.g. output/K1WHleqWkJw/.
    let out_dir = args.out.join(run_label(&args.video));
    let mut results = Vec::with_capacity(frames.len());
    for (frame, answer) in frames.iter().zip(answers) {
        let is_match = answer.probability >= args.threshold;
        let mut result = FrameResult {
            index: frame.index,
            seconds: frame.seconds,
            probability: answer.probability,
            is_match,
            product_box: None,
            outlined: None,
            crop: None,
        };
        // A Box answer is always a box, even when the product isn't there,
        // so we only keep it for frames the match question says contain the product.
        if is_match {
            let (outlined, crop) =
                images::save_box_images(&frame.path, &answer.product_box, &out_dir, frame)?;
            result.product_box = Some(answer.product_box);
            result.outlined = Some(outlined);
            result.crop = Some(crop);
        }
        results.push(result);
    }

    let found = results.iter().any(|r| r.is_match);
    if found {
        eprintln!(
            "Saved outlined and cropped matches to {}",
            out_dir.display()
        );
    }
    if args.json {
        report::print_json(
            &args.video,
            &args.reference,
            &args.model,
            args.threshold,
            &results,
        )?;
    } else {
        report::print_table(&results, args.threshold);
    }
    Ok(found)
}

/// A short, filesystem-safe name for a video: the file name without its extension,
/// the last part of a URL path, or a YouTube `v=` id.
fn run_label(video: &str) -> String {
    let without_query = video.split(['?', '#']).next().unwrap_or(video);
    let youtube_id = video
        .split(['?', '&'])
        .find_map(|part| part.strip_prefix("v="));
    let last_segment = without_query
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    let name = youtube_id.unwrap_or_else(|| last_segment.split('.').next().unwrap_or(""));
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "video".to_string()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::run_label;

    #[test]
    fn labels() {
        assert_eq!(
            run_label("https://www.youtube.com/shorts/K1WHleqWkJw"),
            "K1WHleqWkJw"
        );
        assert_eq!(
            run_label("https://www.youtube.com/watch?v=abc123&t=5"),
            "abc123"
        );
        assert_eq!(run_label("clips/my video.mp4"), "my_video");
    }
}
