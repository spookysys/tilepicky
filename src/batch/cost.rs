// SPDX-License-Identifier: GPL-3.0-only
//! Library cost planning. Preview reads image headers; it never uploads images.

use super::{Job, Kind};
use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::Path, sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc}, time::Duration};

const GOOGLE_PRICES: &str = "https://ai.google.dev/gemini-api/docs/pricing";
const ROUTER_PRICES: &str = "https://openrouter.ai/api/v1/models";
// Prices checked on 2026-09-29. Do not silently reuse the snapshot indefinitely.
const PRICE_EXPIRY_MS: u64 = 1798761600000; // 2027-01-01 UTC

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Rates {
    input: f64,
    output: f64,
    per_request: f64,
    basis: String,
    source: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Usage {
    pub requests: u64,
    pub input: u64,
    pub output: u64,
}

impl Usage {
    pub fn record(&mut self, reply: &Value) {
        let google = &reply["usageMetadata"];
        let counts = if google.is_object() {
            google["promptTokenCount"].as_u64().zip(google["candidatesTokenCount"].as_u64())
                .map(|(input, output)| (input, output.saturating_add(google["thoughtsTokenCount"].as_u64().unwrap_or(0))))
        } else {
            // OpenAI-compatible completion_tokens already includes reasoning_tokens.
            reply["usage"]["prompt_tokens"].as_u64().zip(reply["usage"]["completion_tokens"].as_u64())
        };
        if let Some((input, output)) = counts {
            self.requests = self.requests.saturating_add(1);
            self.input = self.input.saturating_add(input);
            self.output = self.output.saturating_add(output);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Estimate {
    count: usize,
    input: [u64; 2],
    output: [u64; 2],
    rates: Option<Rates>,
    price_note: String,
    token_note: String,
    failed_headers: usize,
    pub usage_basis: u64,
}

fn dollars(value: f64) -> String {
    if value == 0.0 { "$0.00".into() }
    else if value < 0.01 { "less than $0.01".into() }
    else { format!("${value:.2}") }
}

impl Estimate {
    fn cost(&self, i: usize) -> Option<f64> {
        let rates = self.rates.as_ref()?;
        Some((self.input[i] as f64 * rates.input + self.output[i] as f64 * rates.output) / 1_000_000.0
            + self.count as f64 * rates.per_request)
    }

    pub fn summary(&self) -> String {
        match (self.cost(0), self.cost(1)) {
            (Some(_), Some(high)) if high < 0.01 && high > 0.0 => "Estimated cost: less than $0.01 USD".into(),
            (Some(low), Some(high)) => {
                let a = dollars(low); let b = dollars(high);
                if a == b { format!("Estimated cost: about {a} USD") } else { format!("Estimated cost: {a} to {b} USD") }
            }
            _ => "Cost estimate unavailable: no verified price for this model.".into(),
        }
    }

    pub fn usage_summary(&self, usage: &Usage) -> Option<String> {
        let rates = self.rates.as_ref()?;
        let amount = (usage.input as f64 * rates.input + usage.output as f64 * rates.output) / 1_000_000.0
            + usage.requests as f64 * rates.per_request;
        Some(format!("Reported usage: {} USD at the estimated rates.", dollars(amount)))
    }

    pub fn ui(&self, ui: &mut egui::Ui) {
        ui.strong(self.summary());
        if self.price_note.starts_with("The latest alias") { ui.small("Price assumption: Gemini 3.8 Flash. The latest alias can change."); }
        ui.small("Planning estimate, not a spending limit. Retries, taxes, and provider fees can add to the cost.");
        egui::CollapsingHeader::new("Estimate details").show(ui, |ui| {
            ui.label(format!("Input: {} to {} tokens, including images and the prompt for every sheet.", self.input[0], self.input[1]));
            ui.label(format!("Output: {} to {} tokens, including reasoning.", self.output[0], self.output[1]));
            ui.label(&self.token_note);
            if self.usage_basis > 0 { ui.label(format!("Output range uses {} reported requests from the previous library job.", self.usage_basis)); }
            else { ui.label("Output assumes 256 to 1536 tokens per sheet for the label, tags, JSON, and reasoning. Actual use can exceed this range."); }
            if self.failed_headers > 0 {
                ui.label(format!("Could not read {} image headers. Their input estimates use a broad fallback range.", self.failed_headers));
            }
            if let Some(rates) = &self.rates {
                ui.label(&rates.basis);
                ui.label(format!("USD per million tokens: {:.4} input, {:.4} output.", rates.input, rates.output));
                if rates.per_request > 0.0 { ui.label(format!("Additional image/request price: ${:.6} per sheet.", rates.per_request)); }
                ui.hyperlink_to("Price source", &rates.source);
            }
            if !self.price_note.is_empty() { ui.label(&self.price_note); }
            ui.small("Token counts are local approximations. No sheets are uploaded to calculate this estimate.");
            ui.small(format!("Response limit: 4096 tokens per sheet, {} for this job before retries.", self.count as u64 * 4096));
        });
    }
}

pub(super) struct Preview {
    pub receiver: mpsc::Receiver<Estimate>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Preview { fn drop(&mut self) { self.cancel.store(true, Ordering::Relaxed); } }

impl Preview {
    pub fn start(job: Job, root: std::path::PathBuf, previous: Option<&Job>, ctx: egui::Context) -> Result<Self, String> {
        let history = previous.filter(|old| old.provider == job.provider && old.model == job.model && old.tag_list == job.tag_list
            && !job.model.ends_with("latest")).map(|old| old.usage.clone()).unwrap_or_default();
        let cancel = Arc::new(AtomicBool::new(false)); let stopped = cancel.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new().name("library-cost".into()).spawn(move || {
            let (rates, note) = prices(&job, super::now_ms());
            if let Some(estimate) = estimate(&job, &root, rates, note, &history, &stopped) {
                let _ = sender.send(estimate); ctx.request_repaint();
            }
        }).map_err(|_| "Could not start the cost estimate.".to_string())?;
        Ok(Self { receiver, cancel })
    }
}

fn google_rates(model: &str, now: u64) -> Option<Rates> {
    if now >= PRICE_EXPIRY_MS { return None; }
    let (input, output) = match model {
        "gemini-3.8-flash" | "gemini-3.7-flash" | "gemini-3.6-flash" | "gemini-flash-latest" => (0.375, 1.875),
        "gemini-2.5-flash" => (0.15, 1.25),
        "gemini-2.5-flash-lite" => (0.05, 0.20),
        _ => return None,
    };
    Some(Rates { input, output, per_request: 0.0,
        basis: format!("Google batch prices, checked 2026-09-29. Price basis: {}.",
            if model == "gemini-flash-latest" { "Gemini 3.8 Flash" } else { model }), source: GOOGLE_PRICES.into() })
}

fn prices(job: &Job, now: u64) -> (Option<Rates>, String) {
    if job.provider.kind == Kind::Gemini {
        // Custom endpoints do not necessarily bill at Google's prices.
        let official = super::endpoint(&job.provider).ok().and_then(|s| s.parse::<ureq::http::Uri>().ok())
            .is_some_and(|u| u.host() == Some("generativelanguage.googleapis.com"));
        if !official { return (None, "Custom endpoint: Google prices may not apply.".into()); }
        let rates = google_rates(&job.model, now);
        let note = if rates.is_none() { "The built-in price snapshot is expired or does not include this model." }
            else if job.model == "gemini-flash-latest" {
                "The latest alias can change. This estimate assumes Gemini 3.8 Flash prices; it does not verify the alias target."
            } else { "Google batch rates already include the batch discount." };
        return (rates, note.into());
    }
    if !job.provider.is_openrouter() { return (None, "This endpoint does not provide a supported price catalog.".into()); }
    let result = fetch_router_prices(&job.model).and_then(|v| router_rates(&v, &job.model));
    match result {
        Ok(rates) => (Some(rates), "OpenRouter catalog prices. The selected route, caching, or account fees can change the bill.".into()),
        Err(error) => (None, error),
    }
}

fn fetch_router_prices(model: &str) -> Result<Value, String> {
    let query: String = model.bytes().map(|b| format!("%{b:02X}")).collect();
    let url = format!("{ROUTER_PRICES}?q={query}");
    #[cfg(test)]
    let url = std::env::var("TILEPICKY_TEST_TRANSPORT").ok()
        .filter(|s| s.parse::<ureq::http::Uri>().is_ok_and(|u| u.host() == Some("127.0.0.1") && u.scheme_str() == Some("http")))
        .map_or(url.clone(), |base| format!("{base}/models?q={query}"));
    let client = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).max_redirects(0).build().new_agent();
    let mut response = client.get(&url).call().map_err(|_| "Could not retrieve OpenRouter prices. You can still start the job.".to_string())?;
    response.body_mut().with_config().limit(4 * 1024 * 1024).read_json()
        .map_err(|_| "OpenRouter returned an unreadable price catalog.".into())
}

fn router_rates(value: &Value, model: &str) -> Result<Rates, String> {
    let entry = value["data"].as_array().and_then(|a| a.iter().find(|m| m["id"] == model))
        .ok_or("The price catalog does not contain this model.")?;
    let price = &entry["pricing"];
    let number = |key: &str, optional: bool| -> Result<f64, String> {
        let n = if price[key].is_null() && optional { Some(0.0) }
            else { price[key].as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| price[key].as_f64()) };
        n.filter(|n| n.is_finite() && *n >= 0.0 && *n <= 1000.0).ok_or_else(|| format!("The catalog has no usable {key} price."))
    };
    Ok(Rates { input: number("prompt", false)? * 1_000_000.0, output: number("completion", false)? * 1_000_000.0,
        per_request: number("image", true)? + number("request", true)?,
        basis: format!("OpenRouter live catalog: {model}. Standard requests, with no batch discount."), source: ROUTER_PRICES.into() })
}

fn image_tokens(kind: Kind, model: &str, dimensions: Option<(u32, u32)>) -> [u64; 2] {
    if kind == Kind::Gemini && (model.starts_with("gemini-3.") || model == "gemini-flash-latest") { return [560, 1120]; }
    let Some((w, h)) = dimensions.filter(|(w, h)| *w > 0 && *h > 0) else { return [256, 8192]; };
    let scale = (2048.0 / w.max(h) as f64).min(1.0);
    let (w, h) = ((w as f64 * scale).max(1.0), (h as f64 * scale).max(1.0));
    if kind == Kind::Gemini {
        if w <= 384.0 && h <= 384.0 { return [258, 258]; }
        let crop = (w.min(h) / 1.5).floor().clamp(256.0, 768.0);
        let n = (w / crop).ceil() as u64 * (h / crop).ceil() as u64 * 258;
        return [n / 2, n * 2];
    }
    // The catalog has no universal image tokenizer. Use a broad planning range.
    let tiles = (w / 512.0).ceil() as u64 * (h / 512.0).ceil() as u64;
    [256, (tiles * 512).max(1024)]
}

fn estimate(job: &Job, root: &Path, rates: Option<Rates>, price_note: String, usage: &Usage, cancel: &AtomicBool) -> Option<Estimate> {
    let mut input = [0, 0]; let mut failed_headers = 0;
    // Include the actual schema and prompt. Use a tiny image only to build the request locally.
    let request = crate::labels::request(&job.model, &image::RgbaImage::new(1, 1), &job.tag_list, job.free_tags).ok()?;
    let schema = request["response_format"].to_string().chars().count() as u64;
    for sheet in &job.sheets {
        if cancel.load(Ordering::Relaxed) { return None; }
        let dimensions = image::image_dimensions(root.join(&sheet.rel)).ok();
        if dimensions.is_none() { failed_headers += 1; }
        let image = image_tokens(job.provider.kind, &job.model, dimensions);
        let prompt = job.prompt.clone().unwrap_or_else(|| crate::labels::prompt(&job.tag_list, job.free_tags));
        let prompt = if job.file_context { crate::labels::sheet_prompt(prompt, &sheet.rel) } else { prompt };
        let chars = prompt.iter().map(|s| s.chars().count() as u64).sum::<u64>() + schema;
        input[0] += image[0] + chars.div_ceil(4) + 32;
        input[1] += image[1] + chars.div_ceil(2) + 128;
    }
    let (per_output, usage_basis) = if usage.requests >= 5 {
        let average = usage.output / usage.requests;
        ([average / 2, average.saturating_mul(2).max(256)], usage.requests)
    } else { ([256, 1536], 0) };
    Some(Estimate { count: job.sheets.len(), input, output: per_output.map(|n| n * job.sheets.len() as u64), rates, price_note,
        token_note: if job.provider.kind == Kind::Gemini {
            "Image estimates use Google's image token guidance. Text and JSON use a range of 2 to 4 characters per token."
        } else {
            "Image token use varies by model. This broad range uses resized image dimensions and 2 to 4 characters per text token."
        }.into(), failed_headers, usage_basis })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog() -> Value {
        json!({"data":[{"id":"test/vision", "pricing":{"prompt":"0.000002", "completion":"0.000006", "image":"0.002", "request":"0.001"}}]})
    }

    #[test]
    fn catalog_prices_are_per_token_and_fixed_fees_are_per_request() {
        let rates = router_rates(&catalog(), "test/vision").unwrap();
        assert_eq!(rates.input, 2.0); assert_eq!(rates.output, 6.0); assert_eq!(rates.per_request, 0.003);
        let estimate = Estimate { count: 1000, input: [1_000_000; 2], output: [500_000; 2], rates: Some(rates),
            price_note: String::new(), token_note: String::new(), failed_headers: 0, usage_basis: 0 };
        assert_eq!(estimate.cost(0), Some(8.0));
        assert!(estimate.summary().contains("$8.00"));
    }

    #[test]
    fn unknown_invalid_and_missing_prices_do_not_become_free() {
        assert!(router_rates(&catalog(), "another/vision").is_err());
        for value in [Value::Null, json!("-1"), json!("NaN"), json!("inf"), json!("1e300")] {
            let mut catalog = catalog(); catalog["data"][0]["pricing"]["prompt"] = value;
            assert!(router_rates(&catalog, "test/vision").is_err());
        }
        let mut free = catalog();
        free["data"][0]["pricing"] = json!({"prompt":"0", "completion":"0"});
        assert_eq!(router_rates(&free, "test/vision").unwrap().input, 0.0);
    }

    #[test]
    fn google_uses_batch_rates_and_expires_the_snapshot() {
        let rates = google_rates("gemini-3.8-flash", PRICE_EXPIRY_MS - 1).unwrap();
        assert_eq!(rates.input, 0.375); assert_eq!(rates.output, 1.875);
        assert!(google_rates("gemini-3.8-flash", PRICE_EXPIRY_MS).is_none());
        assert!(google_rates("unknown-gemini", PRICE_EXPIRY_MS - 1).is_none());
        assert!(google_rates("gemini-flash-latest", PRICE_EXPIRY_MS - 1).unwrap().basis.contains("Gemini 3.8 Flash"));
    }

    #[test]
    fn reasoning_is_included_once_and_missing_usage_is_not_zero_usage() {
        let mut usage = Usage::default();
        usage.record(&json!({"usageMetadata":{"promptTokenCount":500, "candidatesTokenCount":100, "thoughtsTokenCount":300}}));
        usage.record(&json!({"usage":{"prompt_tokens":600, "completion_tokens":450,"completion_tokens_details":{"reasoning_tokens":350}}}));
        usage.record(&json!({"error":"No response"}));
        assert_eq!((usage.requests, usage.input, usage.output), (2, 1100, 850));
        let saved = serde_json::to_value(&usage).unwrap();
        assert_eq!(serde_json::from_value::<Usage>(saved).unwrap().output, 850);
    }

    #[test]
    fn image_estimates_follow_resizing_and_small_google_images() {
        assert_eq!(image_tokens(Kind::Gemini, "gemini-2.5-flash", Some((16, 16))), [258, 258]);
        assert_eq!(image_tokens(Kind::Gemini, "gemini-3.8-flash", Some((16, 16))), [560, 1120]);
        assert_eq!(image_tokens(Kind::OpenAi, "vision", Some((4096, 4096))),
            image_tokens(Kind::OpenAi, "vision", Some((2048, 2048))));
        let [low, high] = image_tokens(Kind::Gemini, "gemini-2.5-flash", Some((1, 2048)));
        assert!(low <= high && high < 100_000);
        assert_eq!(image_tokens(Kind::OpenAi, "vision", None), [256, 8192]);
    }

    #[test]
    fn small_paid_costs_do_not_display_as_free() {
        assert_eq!(dollars(0.0002), "less than $0.01");
        assert_eq!(dollars(0.0), "$0.00");
    }

    fn job(root: &Path, scope: super::super::Scope) -> Job {
        super::super::prepare(&crate::index::Index::scan(root, [16, 16]), super::super::tests::provider(Kind::Gemini),
            "gemini-2.5-flash".into(), scope).unwrap()
    }

    #[test]
    fn estimate_counts_only_the_proposed_sheets_and_preserves_unknown_images() {
        let f = crate::storage::tests::Folder::new();
        image::RgbaImage::new(16, 16).save(f.0.join("saved.png")).unwrap();
        image::RgbaImage::new(16, 16).save(f.0.join("new.png")).unwrap();
        let label = crate::labels::response(&crate::labels::tests::completion(crate::labels::tests::labeled("Tile")), &[])
            .unwrap().into_label("test", "test", &[]);
        crate::sidecar::store_labels(&f.0, [("saved.png", Some(label))]).unwrap();
        let stop = AtomicBool::new(false);
        let pending = job(&f.0, super::super::Scope::Unlabeled);
        let all = job(&f.0, super::super::Scope::All);
        let estimate_for = |j: &Job, usage: &Usage| estimate(j, &f.0, None, String::new(), usage, &stop).unwrap();
        let a = estimate_for(&pending, &Usage::default()); let b = estimate_for(&all, &Usage::default());
        assert_eq!(a.count, 1); assert_eq!(b.count, 2); assert_eq!(a.output[1] * 2, b.output[1]);
        std::fs::remove_file(f.0.join("new.png")).unwrap();
        let missing = estimate_for(&pending, &Usage::default());
        assert_eq!(missing.count, 1); assert_eq!(missing.failed_headers, 1);
        assert!(missing.input[1] > a.input[1]);
        let learned = estimate_for(&pending, &Usage { requests: 10, input: 10000, output: 10000 });
        assert_eq!(learned.usage_basis, 10); assert_eq!(learned.output, [500, 2000]);
        stop.store(true, Ordering::Relaxed);
        assert!(estimate(&all, &f.0, None, String::new(), &Usage::default(), &stop).is_none());
    }

    #[test]
    fn estimates_and_usage_survive_restart_and_old_jobs_still_load() {
        let f = crate::storage::tests::Folder::new();
        image::RgbaImage::new(16, 16).save(f.0.join("sheet.png")).unwrap();
        let mut job = job(&f.0, super::super::Scope::All);
        job.estimate = estimate(&job, &f.0, google_rates(&job.model, 0), String::new(), &Usage::default(), &AtomicBool::new(false));
        job.usage = Usage { requests: 2, input: 4000, output: 800 };
        job.save(&f.0).unwrap();
        let loaded = Job::load(&f.0).unwrap().unwrap();
        assert_eq!(loaded.estimate.unwrap().summary(), job.estimate.as_ref().unwrap().summary());
        assert_eq!(loaded.usage.output, 800);
        let mut old = serde_json::to_value(job).unwrap();
        old.as_object_mut().unwrap().remove("estimate"); old.as_object_mut().unwrap().remove("usage");
        let restored: Job = serde_json::from_value(old).unwrap();
        assert!(restored.estimate.is_none()); assert_eq!(restored.usage.requests, 0);
    }

    #[test]
    fn custom_google_endpoints_do_not_inherit_google_prices() {
        let f = crate::storage::tests::Folder::new();
        let mut job = job(&f.0, super::super::Scope::All);
        job.provider.url = "https://example.invalid/v1beta".into();
        assert!(prices(&job, 0).0.is_none());
    }

}
