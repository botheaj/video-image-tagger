//! Printing results: a human-readable table, or JSON.

use std::path::Path;

use anyhow::Result;
use serde_json::json;

use crate::FrameResult;

/// Prints one row per frame with a bar showing the match probability, then a verdict.
pub fn print_table(results: &[FrameResult], threshold: f64) {
    println!("{:>7}  {:>5}  {:<20}  saved", "time", "P", "");
    for r in results {
        let filled = (r.probability * 20.0).round() as usize;
        let bar = format!("{}{}", "█".repeat(filled), "·".repeat(20 - filled));
        let mark = if r.is_match { "✔" } else { " " };
        let saved = r
            .outlined
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        println!(
            "{:>6.1}s  {:>5.2}  {bar} {mark} {saved}",
            r.seconds, r.probability
        );
    }
    println!();
    println!("{}", verdict(results, threshold));
}

/// One line summing up the run, e.g. "MATCH: 9 of 46 frames, first at 6.5s ...".
fn verdict(results: &[FrameResult], threshold: f64) -> String {
    // `max_by` with `total_cmp` finds the highest probability (f64 can't use plain `max`).
    let Some(best) = results
        .iter()
        .max_by(|a, b| a.probability.total_cmp(&b.probability))
    else {
        return "NO MATCH: no frames were checked".to_string();
    };
    let matches: Vec<&FrameResult> = results.iter().filter(|r| r.is_match).collect();
    match matches.first() {
        Some(first) => format!(
            "MATCH: product seen in {} of {} frames, first at {:.1}s, strongest {:.2} at {:.1}s",
            matches.len(),
            results.len(),
            first.seconds,
            best.probability,
            best.seconds
        ),
        None => format!(
            "NO MATCH: highest probability {:.2} at {:.1}s, below the {threshold:.2} threshold",
            best.probability, best.seconds
        ),
    }
}

pub fn print_json(
    video: &str,
    reference: &Path,
    model: &str,
    threshold: f64,
    results: &[FrameResult],
) -> Result<()> {
    let output = json!({
        "video": video,
        "reference": reference,
        "model": model,
        "threshold": threshold,
        "match": results.iter().any(|r| r.is_match),
        "verdict": verdict(results, threshold),
        "frames": results,
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
