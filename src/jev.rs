//! A small client for TypeSafe's System One API (docs.typesafe.ai/api).
//! There's no Rust SDK, so we build the multipart HTTP request ourselves.

use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use reqwest::blocking::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use serde_json::json;

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
/// Images and Box questions are experimental: every request that uses them needs this header.
const IMAGES_HEADER: &str = "TypeSafe-Danger-Zone-Images";
const MAX_ATTEMPTS: u32 = 6;

pub struct Client {
    http: reqwest::blocking::Client,
    api_key: String,
    model: String,
}

/// Jev's answers about one frame.
pub struct FrameAnswer {
    /// Probability (0 to 1) that the frame shows the reference product.
    pub probability: f64,
    /// Where the product would be. Only meaningful when `probability` is high.
    pub product_box: BoxAnswer,
}

/// A Box answer, in the same shape the API returns it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BoxAnswer {
    #[serde(rename = "box")]
    pub edges: Edges,
    pub confidence: f64,
}

/// The four edges of a box, as fractions (0 to 1) of the image's width and height,
/// measured from the top-left corner.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Edges {
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
}

// The parts of the API response we read. Each field name must match a key in the JSON.
#[derive(Deserialize)]
struct Response {
    answers: Answers,
}

#[derive(Deserialize)]
struct Answers {
    product_visible: NoulAnswer,
    product_box: BoxAnswer,
}

#[derive(Deserialize)]
struct NoulAnswer {
    noul: f64,
}

impl Client {
    /// Creates a client using the TYPESAFE_API_KEY environment variable.
    pub fn from_env(model: &str) -> Result<Self> {
        let api_key = std::env::var("TYPESAFE_API_KEY")
            .context("TYPESAFE_API_KEY is not set (export it, or put it in a .env file)")?;
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self {
            http,
            api_key,
            model: model.to_string(),
        })
    }

    /// Asks Jev whether `frame` shows the product in `reference`, and where it is.
    pub fn check_frame(
        &self,
        reference_jpeg: &[u8],
        frame: &Path,
        describe: Option<&str>,
    ) -> Result<FrameAnswer> {
        let frame_jpeg =
            std::fs::read(frame).with_context(|| format!("reading {}", frame.display()))?;
        let body = self.request_body(describe);

        let response: Response = self.send(&body, reference_jpeg, &frame_jpeg)?;
        Ok(FrameAnswer {
            probability: response.answers.product_visible.noul,
            product_box: response.answers.product_box,
        })
    }

    /// Builds the JSON part of the request. The images travel as separate file parts;
    /// `assets` maps a reference string to each part, and wherever that string appears
    /// in the state or questions, Jev sees the image instead.
    fn request_body(&self, describe: Option<&str>) -> serde_json::Value {
        // A fresh random reference per image, so it can't accidentally match other text.
        let reference_key = uuid::Uuid::new_v4().to_string();
        let frame_key = uuid::Uuid::new_v4().to_string();

        let mut state = json!({ "reference": reference_key, "frame": frame_key });
        let mut product = "the product pictured in `reference`".to_string();
        if let Some(description) = describe {
            state["reference_description"] = json!(description);
            product = "the product pictured in `reference` (described in `reference_description`)"
                .to_string();
        }

        json!({
            "model": self.model,
            "state": state,
            "assets": {
                reference_key: { "type": "image", "part": "reference" },
                frame_key: { "type": "image", "part": "frame" },
            },
            "questions": {
                // Both questions run in parallel against the same state. Code ignores
                // the box when the Noul says the product isn't there.
                "product_visible": {
                    "type": "noul",
                    "instructions": format!("Is {product} visible in `frame`?"),
                    "criteria": {
                        "true": "The same product as in `reference` is physically shown in `frame`: any copy of it, from any angle, even if small or partly hidden.",
                        "false": "The product is not in `frame`. Nothing like it is visible, a different product is shown (even a similar-looking one), or only something related to it appears, such as its packaging, artwork, a poster, or footage of it on a screen.",
                    },
                },
                "product_box": {
                    "type": "box",
                    "instructions": format!("{product}, as it appears in `frame`"),
                },
            },
        })
    }

    /// Sends a multipart request, retrying with backoff when the API is busy.
    fn send<T: serde::de::DeserializeOwned>(
        &self,
        body: &serde_json::Value,
        reference_jpeg: &[u8],
        frame_jpeg: &[u8],
    ) -> Result<T> {
        let mut wait = Duration::from_secs(1);
        for attempt in 1..=MAX_ATTEMPTS {
            // A multipart Form is used up when it's sent, so build a new one each attempt.
            // The JSON part must come first and be named "request".
            let form = Form::new()
                .part(
                    "request",
                    Part::text(body.to_string()).mime_str("application/json")?,
                )
                .part(
                    "reference",
                    Part::bytes(reference_jpeg.to_vec())
                        .file_name("reference.jpg")
                        .mime_str("image/jpeg")?,
                )
                .part(
                    "frame",
                    Part::bytes(frame_jpeg.to_vec())
                        .file_name("frame.jpg")
                        .mime_str("image/jpeg")?,
                );

            let result = self
                .http
                .post(ENDPOINT)
                .bearer_auth(&self.api_key)
                .header(IMAGES_HEADER, "true")
                .multipart(form)
                .send();

            let response = match result {
                Ok(response) => response,
                // Network trouble: retry unless this was the last attempt.
                Err(err) if attempt < MAX_ATTEMPTS => {
                    eprintln!("\nrequest failed ({err}), retrying in {}s", wait.as_secs());
                    sleep(wait);
                    wait *= 2;
                    continue;
                }
                Err(err) => return Err(err).context("calling the TypeSafe API"),
            };

            let status = response.status();
            if status.is_success() {
                return response.json().context("reading the TypeSafe API response");
            }
            // 429 = rate limited, 529 = overloaded, 5xx = server trouble: all worth retrying.
            let retryable = status == StatusCode::TOO_MANY_REQUESTS
                || status.as_u16() == 529
                || status.is_server_error();
            if retryable && attempt < MAX_ATTEMPTS {
                let delay = retry_after(&response).unwrap_or(wait);
                sleep(delay);
                wait *= 2;
                continue;
            }
            let text = response.text().unwrap_or_default();
            bail!("TypeSafe API returned {status}: {text}");
        }
        unreachable!("the last attempt always returns")
    }
}

/// Reads the `retry-after` header (in seconds), if the API sent one.
fn retry_after(response: &reqwest::blocking::Response) -> Option<Duration> {
    let seconds: u64 = response
        .headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds))
}
