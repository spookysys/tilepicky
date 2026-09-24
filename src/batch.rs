// SPDX-License-Identifier: GPL-3.0-only
//! One library batch: prepared locally, then sent through the provider's batch API.
//! Each sheet is one request, the same one that Label with AI sends.

use crate::{ai::{Kind, Provider}, index::Index, labels, sidecar::{Label, Status}};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::{BTreeMap, BTreeSet}, path::{Path, PathBuf}, time::Duration};

const MAX_BYTES: usize = 18_000_000;

#[derive(Clone, Serialize, Deserialize)]
pub struct Sheet {
    pub rel: String,
    /// The fingerprint at preparation. A sheet that changes before it is
    /// sent is not sent.
    pub identity: String,
    pub label: Option<Label>,
    pub error: String,
    /// The label is in the book.
    pub imported: bool,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub enum Remote { Ready, Submitting, Waiting(String), Done }

/// The sheets that go to the provider in one batch.
#[derive(Clone, Serialize, Deserialize)]
pub struct Group {
    pub sheets: Vec<usize>,
    pub remote: Remote,
}

fn key(sheet: usize) -> String { format!("sheet-{sheet}") }

#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    pub provider: Provider,
    pub model: String,
    pub sheets: Vec<Sheet>,
    pub groups: Vec<Group>,
    pub started: bool,
    pub skipped: usize,
    pub excluded: Vec<String>,
    pub bytes: usize,
}

impl Job {
    pub fn done(&self) -> bool { self.groups.iter().all(|g| g.remote == Remote::Done) }
    pub fn uncertain(&self) -> bool { self.groups.iter().any(|g| g.remote == Remote::Submitting) }
    pub fn summary(&self) -> String {
        let completed = self.sheets.iter().filter(|s| s.label.as_ref().is_some_and(|l| l.status == Status::Labeled)).count();
        let failed = self.sheets.iter().filter(|s| !s.error.is_empty()).count();
        format!("{completed}/{} sheets complete; {failed} need attention", self.sheets.len())
    }
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        crate::storage::write(&dir.join("state.json"), self)
    }
    pub fn load(dir: &Path) -> Result<Option<Self>, String> {
        match std::fs::read(dir.join("state.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|_| "The saved batch state is unreadable.".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// The folder of one library's batch journal, in the configuration folder.
pub fn directory(root: &Path) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let hash = Sha256::digest(root.as_os_str().as_encoded_bytes());
    Ok(crate::settings::dir().ok_or("No configuration directory is available.")?.join("batches").join(format!("{hash:x}")))
}

/// The request body for one sheet, in the format of the provider.
fn body(job: &Job, img: &image::RgbaImage) -> Result<Value, String> {
    let body = labels::request(&job.model, img)?;
    Ok(if job.provider.kind == Kind::Gemini { gemini_request(&body) } else { body })
}

/// Reads the sheets and sizes the requests. Nothing goes to the provider,
/// and nothing is written to the book.
pub fn prepare(root: &Path, dir: &Path, provider: Provider, model: String) -> Result<Job, String> {
    endpoint(&provider)?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    let index = Index::scan(root, [16, 16]);
    if let Some(error) = index.error { return Err(error); }
    let mut job = Job { provider, model: model.trim_end_matches(":batch").into(), sheets: vec![], groups: vec![],
        started: false, skipped: 0, excluded: vec![], bytes: 0 };
    let (mut group, mut group_bytes) = (Vec::new(), 0);
    for entry in index.entries {
        let result = (|| {
            let img = image::open(root.join(&entry.rel)).map_err(|e| format!("Could not decode image: {e}"))?.to_rgba8();
            let identity = labels::identity(&img);
            if entry.side.label.as_ref().is_some_and(|l| l.identity == identity && l.status == Status::Labeled) {
                job.skipped += 1;
                return Ok(());
            }
            let size = serde_json::to_vec(&body(&job, &img)?).unwrap().len() + 512;
            if size > MAX_BYTES { return Err("The image request is too large.".into()); }
            if !group.is_empty() && (group_bytes + size > MAX_BYTES || group.len() == 100) {
                job.groups.push(Group { sheets: std::mem::take(&mut group), remote: Remote::Ready });
                group_bytes = 0;
            }
            group.push(job.sheets.len());
            group_bytes += size;
            job.bytes += size;
            job.sheets.push(Sheet { rel: entry.rel.clone(), identity, label: None, error: String::new(), imported: true });
            Ok::<_, String>(())
        })();
        if let Err(error) = result { job.excluded.push(format!("{}: {error}", entry.rel)); }
    }
    if !group.is_empty() { job.groups.push(Group { sheets: group, remote: Remote::Ready }); }
    Ok(job)
}

fn gemini_request(chat: &Value) -> Value {
    let parts: Vec<_> = chat["messages"][1]["content"].as_array().unwrap().iter().map(|part| {
        if part["type"] == "text" { json!({"text":part["text"]}) } else {
            let data = part["image_url"]["url"].as_str().unwrap().strip_prefix("data:image/png;base64,").unwrap();
            json!({"inlineData":{"mimeType":"image/png", "data":data}})
        }
    }).collect();
    json!({"systemInstruction":{"parts":[{"text":chat["messages"][0]["content"]}]},
        "contents":[{"role":"user", "parts":parts}], "generationConfig":{"maxOutputTokens":4096,
            "responseMimeType":"application/json", "responseJsonSchema":chat["response_format"]["json_schema"]["schema"]}})
}

pub fn endpoint(provider: &Provider) -> Result<String, String> {
    let url = labels::checked_url(&provider.url)?;
    let uri: ureq::http::Uri = url.parse().map_err(|_| "Invalid batch endpoint URL.")?;
    match provider.kind {
        Kind::Gemini => Ok(url),
        Kind::OpenAi if uri.host() == Some("openrouter.ai") => Ok("https://openrouter.ai/api/beta".into()),
        _ => Err("Library batches support Google Gemini and OpenRouter. Select one in Settings.".into()),
    }
}

fn submit_body(job: &Job, requests: &[(String, Value)]) -> Value {
    match job.provider.kind {
        Kind::Gemini => json!({"batch":{"displayName":"Tilepicky library labels", "inputConfig":{"requests":{"requests":
            requests.iter().map(|(key, body)| json!({"metadata":{"key":key}, "request":body})).collect::<Vec<_>>()}}}}),
        Kind::OpenAi => json!({"endpoint":"/v1/chat/completions", "model":job.model,
            "requests":requests.iter().map(|(key, body)| json!({"custom_id":key, "body":body})).collect::<Vec<_>>()}),
    }
}

fn remote_id(kind: Kind, response: &Value) -> Result<String, String> {
    let id = response[if kind == Kind::Gemini { "name" } else { "id" }].as_str().ok_or("The batch ID is missing.")?;
    validate_id(kind, id)?;
    Ok(id.into())
}

pub fn validate_id(kind: Kind, id: &str) -> Result<(), String> {
    let tail = if kind == Kind::Gemini { id.strip_prefix("batches/").ok_or("Expected a batches/... ID.")? } else { id };
    if tail.is_empty() || !tail.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)) {
        return Err("Invalid batch ID.".into());
    }
    Ok(())
}

pub struct Transport { client: ureq::Agent, base: String, key: String, kind: Kind }
impl Transport {
    pub fn new(provider: &Provider, key: String) -> Result<Self, String> {
        let client = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(60)))
            .max_redirects(0).http_status_as_error(false).build().new_agent();
        Ok(Self { client, base: endpoint(provider)?, key, kind: provider.kind })
    }
    pub fn send(&self, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let (header, key) = if self.kind == Kind::Gemini { ("x-goog-api-key", self.key.clone()) }
            else { ("Authorization", format!("Bearer {}", self.key)) };
        let url = format!("{}/{path}", self.base);
        let response = match body {
            Some(body) => self.client.post(&url).header(header, &key).send_json(body),
            None => self.client.get(&url).header(header, &key).call(),
        };
        let mut response = response.map_err(|_| "The batch endpoint could not be reached. Check the saved batch before retrying submission.")?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            if body.is_some() && matches!(status, 400 | 401 | 403 | 404 | 413 | 422 | 429) {
                return Ok(json!({"submission_rejected":status}));
            }
            return Err(format!("Batch endpoint returned HTTP {status}."));
        }
        response.body_mut().with_config().limit(32_000_000).read_json().map_err(|_| "Invalid or oversized batch response.".into())
    }
}

fn outputs(kind: Kind, value: &Value) -> Result<Option<Vec<(String, Value)>>, String> {
    let values = if kind == Kind::Gemini {
        if value["done"] != true { return Ok(None); }
        if !value["error"].is_null() { return Err("The provider batch failed, expired, or was cancelled.".into()); }
        value.pointer("/response/inlinedResponses/inlinedResponses").and_then(Value::as_array)
    } else {
        match value["status"].as_str() {
            Some("failed" | "cancelled" | "expired") => return Err("The provider batch failed, expired, or was cancelled.".into()),
            Some("completed") => {},
            Some("validating" | "in_progress" | "finalizing" | "queued") => return Ok(None),
            _ => return Err("The provider returned an unknown batch status.".into()),
        }
        value["results"].as_array()
    }.ok_or("The completed batch has no inline results.")?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in values {
        let key = if kind == Kind::Gemini { &item["metadata"]["key"] } else { &item["custom_id"] };
        let key = key.as_str().ok_or("A batch result has no request ID.")?;
        if !seen.insert(key.to_string()) { return Err("Duplicate batch request ID.".into()); }
        let response = if !item["error"].is_null() { Value::Null } else if kind == Kind::Gemini {
            if item["response"]["candidates"].as_array().is_none_or(|a| a.len() != 1)
                || !item["response"]["promptFeedback"]["blockReason"].is_null() {
                out.push((key.into(), Value::Null));
                continue;
            }
            let candidate = &item["response"]["candidates"][0];
            let parts = candidate["content"]["parts"].as_array();
            let text = parts.map(|p| p.iter().filter(|p| p["thought"] != true).filter_map(|p| p["text"].as_str()).collect::<String>());
            json!({"choices":[{"finish_reason": if candidate["finishReason"] == "STOP" {"stop"} else {"invalid"},
                "message":{"content":text}}]})
        } else if item["response"]["status_code"].as_u64().is_some_and(|s| s != 200) { Value::Null }
        else { item["response"]["body"].clone() };
        out.push((key.into(), response));
    }
    Ok(Some(out))
}

fn accept(job: &mut Job, group: usize, values: Vec<(String, Value)>) -> Result<(), String> {
    let mut values: BTreeMap<_, _> = values.into_iter().collect();
    if values.keys().any(|k| !job.groups[group].sheets.iter().any(|&i| key(i) == *k)) {
        return Err("The batch returned an unknown request ID.".into());
    }
    let (provider, model) = (job.provider.name.clone(), job.model.clone());
    for &i in &job.groups[group].sheets {
        let sheet = &mut job.sheets[i];
        let result = values.remove(&key(i)).ok_or("No result returned for this request.".into()).and_then(|v| labels::response(&v));
        match result {
            Ok(reply) => {
                let label = reply.into_label(&provider, &model, &sheet.identity);
                if label.status == Status::Unlabelable { sheet.error = "The model could not label the sheet.".into(); }
                sheet.label = Some(label);
                sheet.imported = false;
            }
            Err(error) => sheet.error = error,
        }
    }
    Ok(())
}

/// Advance one network operation. Persist submission intent before the POST.
/// An interrupted POST stays uncertain and is never silently submitted twice.
pub fn advance(mut job: Job, root: &Path, dir: &Path, mut send: impl FnMut(&str, Option<&Value>) -> Result<Value, String>) -> Result<Job, String> {
    if !job.started { return Err("Confirm Start batch before sending requests.".into()); }
    if job.uncertain() { return Err("Submission outcome is unknown. Attach its provider batch ID before continuing.".into()); }
    let next = job.groups.iter().position(|g| g.remote == Remote::Ready)
        .or_else(|| job.groups.iter().position(|g| matches!(g.remote, Remote::Waiting(_))));
    let Some(i) = next else { return Ok(job) };
    match job.groups[i].remote.clone() {
        Remote::Ready => {
            let mut requests = Vec::new();
            for &s in &job.groups[i].sheets.clone() {
                let sheet = &mut job.sheets[s];
                let img = image::open(root.join(&sheet.rel)).map_err(|e| e.to_string())?.to_rgba8();
                if labels::identity(&img) == sheet.identity { requests.push((key(s), body(&job, &img)?)); }
                else { sheet.error = "The image changed after the batch was prepared.".into(); }
            }
            if requests.is_empty() {
                job.groups[i].remote = Remote::Done;
                job.save(dir)?;
                return Ok(job);
            }
            let path = match job.provider.kind {
                Kind::Gemini => format!("models/{}:batchGenerateContent", job.model),
                Kind::OpenAi => "batches".into(),
            };
            let body = submit_body(&job, &requests);
            job.groups[i].remote = Remote::Submitting;
            job.save(dir)?;
            let response = send(&path, Some(&body))?;
            if let Some(status) = response["submission_rejected"].as_u64() {
                for &s in &job.groups[i].sheets { job.sheets[s].error = format!("Batch submission rejected (HTTP {status})."); }
                job.groups[i].remote = Remote::Done;
            } else { job.groups[i].remote = Remote::Waiting(remote_id(job.provider.kind, &response)?); }
        }
        Remote::Waiting(id) => {
            validate_id(job.provider.kind, &id)?;
            let path = if job.provider.kind == Kind::Gemini { id } else { format!("batches/{id}") };
            let response = send(&path, None)?;
            match outputs(job.provider.kind, &response) {
                Ok(None) => { job.groups.rotate_left(i + 1); job.save(dir)?; return Ok(job); }
                Ok(Some(values)) => accept(&mut job, i, values)?,
                Err(error) => for &s in &job.groups[i].sheets { job.sheets[s].error = error.clone(); },
            }
            job.groups[i].remote = Remote::Done;
        }
        _ => {},
    }
    job.save(dir)?;
    Ok(job)
}

/// UI state only. The journal contains no keys and no UI or thread state.
#[derive(Default)]
pub struct Panel {
    pub root: PathBuf,
    pub dir: PathBuf,
    pub job: Option<Job>,
    task: Option<std::sync::mpsc::Receiver<Result<Job, String>>>,
    pub review: bool,
    pub error: String,
    paused: bool,
    next_check: Option<std::time::Instant>,
    attach_id: String,
    retry_import: bool,
}

impl Panel {
    pub fn pause(&mut self, error: String) { self.error = error; self.paused = true; }

    pub fn busy(&self) -> bool { self.task.is_some() || self.review || self.job.as_ref().is_some_and(|j| j.started && !j.done()) }

    fn spawn(&mut self, ctx: &eframe::egui::Context, work: impl FnOnce() -> Result<Job, String> + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        match std::thread::Builder::new().name("library batch".into()).spawn(move || { let _ = tx.send(work()); ctx.request_repaint(); }) {
            Ok(_) => { self.task = Some(rx); self.error.clear(); }
            Err(e) => self.error = format!("Could not start batch worker: {e}"),
        }
    }

    pub fn tick(&mut self, ctx: &eframe::egui::Context, root: &Path, keys: &crate::ai::Keys) -> bool {
        let mut changed = std::mem::take(&mut self.retry_import);
        if !self.busy() && self.root != root && !root.as_os_str().is_empty() {
            self.root = root.into();
            match directory(root).and_then(|dir| { let job = Job::load(&dir)?; Ok((dir, job)) }) {
                Ok((dir, job)) => { self.dir = dir; self.job = job; self.paused = false; self.error.clear(); changed = true; }
                Err(e) => { self.job = None; self.error = e; self.paused = true; }
            }
        }
        if let Some(rx) = &self.task {
            use std::sync::mpsc::TryRecvError;
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Disconnected) => Some(Err("The batch worker stopped unexpectedly.".into())),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.task = None;
                match result {
                    Ok(job) => {
                        self.review = !job.started;
                        self.next_check = Some(std::time::Instant::now() + Duration::from_secs(
                            if job.groups.iter().all(|g| g.remote == Remote::Done) || job.groups.iter().any(|g| g.remote == Remote::Ready) { 0 } else { 30 }));
                        self.job = Some(job);
                        changed = true;
                    }
                    Err(error) => {
                        if let Ok(Some(job)) = Job::load(&self.dir) { self.job = Some(job); }
                        self.error = error;
                        self.paused = true;
                    }
                }
            }
        }
        if changed { ctx.request_repaint(); return true; }
        if self.task.is_none() && !self.paused && let Some(job) = &self.job && job.started && !job.done() && !job.uncertain() {
            let now = std::time::Instant::now();
            if self.next_check.is_none_or(|next| next <= now) {
                match job.provider.key(keys).ok_or("The batch provider key is missing in Settings.")
                    .and_then(|key| Transport::new(&job.provider, key).map_err(|_| "Invalid batch provider endpoint.")) {
                    Ok(transport) => {
                        let job = job.clone();
                        let (root, dir) = (self.root.clone(), self.dir.clone());
                        self.spawn(ctx, move || advance(job, &root, &dir, |path, body| transport.send(path, body)));
                    }
                    Err(e) => { self.error = e.into(); self.paused = true; }
                }
            } else if let Some(next) = self.next_check { ctx.request_repaint_after(next.saturating_duration_since(now)); }
        }
        changed
    }

    pub fn ui(&mut self, ui: &mut eframe::egui::Ui, root: &Path, ai: &crate::ai::Ai, keys: &crate::ai::Keys, single_busy: bool) {
        use eframe::egui;
        ui.separator();
        ui.strong("Library batch");
        let configured = ai.chosen(crate::ai::Mode::Batch);
        let ready = configured.is_some_and(|(p, _)| endpoint(p).is_ok() && p.key_source(keys) != crate::ai::KeySource::None);
        if let Some(job) = &self.job && job.started {
            ui.label(self.root.display().to_string());
            ui.label(job.summary());
            egui::CollapsingHeader::new("Details").show(ui, |ui| {
                for s in &job.sheets { if !s.error.is_empty() { ui.label(format!("{}: {}", s.rel, s.error)); } }
                for group in &job.groups { if let Remote::Waiting(id) = &group.remote { ui.monospace(id); } }
            });
        }
        if self.task.is_some() { ui.horizontal(|ui| { ui.spinner(); ui.label("Preparing or checking the batch..."); }); }
        if !self.error.is_empty() { ui.colored_label(egui::Color32::LIGHT_RED, &self.error); }
        if !ready { ui.weak("Set a Google or OpenRouter batch model and key in Settings."); }
        let button = ui.add_enabled(ready && !single_busy && !self.busy() && !root.as_os_str().is_empty(),
            egui::Button::new("Label entire library with AI..."));
        crate::stop(&button);
        if button.clicked() {
            let (provider, model) = configured.unwrap();
            let (provider, model, root) = (provider.clone(), model.id.clone(), root.to_path_buf());
            match directory(&root) {
                Ok(dir) => {
                    self.root = root.clone(); self.dir = dir.clone(); self.paused = false;
                    self.spawn(ui.ctx(), move || prepare(&root, &dir, provider, model));
                }
                Err(e) => self.error = e,
            }
        }
        if self.paused && self.job.as_ref().is_some_and(|j| j.started && !j.uncertain())
            && crate::stopped(ui.button("Resume batch / retry save")).clicked() {
            self.paused = false; self.next_check = None; self.error.clear(); self.retry_import = true;
        }
        if let Some(job) = &mut self.job && job.uncertain() {
            ui.weak("Submission was interrupted. Check the provider's batch list and attach its ID. Nothing will be submitted again automatically.");
            crate::stopped(ui.text_edit_singleline(&mut self.attach_id));
            if crate::stopped(ui.button("Attach batch ID")).clicked() {
                match validate_id(job.provider.kind, self.attach_id.trim()) {
                    Ok(()) => {
                        if let Some(group) = job.groups.iter_mut().find(|g| g.remote == Remote::Submitting) {
                            group.remote = Remote::Waiting(self.attach_id.trim().into());
                        }
                        match job.save(&self.dir) {
                            Ok(()) => { self.paused = false; self.next_check = None; self.error.clear(); }
                            Err(e) => self.error = e,
                        }
                    }
                    Err(e) => self.error = e,
                }
            }
        }
    }

    pub fn confirmation(&mut self, ctx: &eframe::egui::Context) {
        if !self.review { return; }
        let Some(job) = &mut self.job else { return; };
        let mut start = false;
        let mut cancel = false;
        eframe::egui::Modal::new(eframe::egui::Id::new("library batch confirmation")).show(ctx, |ui| {
            ui.set_width(430.0);
            ui.heading("Label this entire library?");
            ui.label(self.root.display().to_string());
            ui.weak(format!("{} / {} (from Settings)", job.provider.name, job.model));
            ui.label(format!("{} sheets to label, one request each; {} already current; {} excluded", job.sheets.len(), job.skipped, job.excluded.len()));
            ui.label(format!("About {:.1} MB of request data; at most {} output tokens requested", job.bytes as f64 / 1_000_000.0, job.sheets.len() * 4096));
            ui.weak("Price estimate unavailable. Image token charges and model output vary; these counts are not a price quote.");
            ui.weak("No images have been sent. You can close the app and resume later.");
            eframe::egui::CollapsingHeader::new("Excluded files").show(ui, |ui| {
                eframe::egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| { for error in &job.excluded { ui.label(error); } });
            });
            ui.horizontal(|ui| {
                start = ui.add_enabled(!job.sheets.is_empty(), eframe::egui::Button::new("Start batch")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if start {
            job.started = true;
            match job.save(&self.dir) {
                Ok(()) => { self.review = false; self.next_check = None; self.paused = false; }
                Err(e) => { job.started = false; self.error = e; }
            }
        }
        if cancel { self.review = false; self.job = None; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::tests::{completion, labeled};
    use image::{Rgba, RgbaImage};

    struct Files { base: PathBuf, root: PathBuf, spool: PathBuf }
    impl Files {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let base = std::env::temp_dir().join(format!("tilepicky-batch-{}-{stamp}", std::process::id()));
            let (root, spool) = (base.join("library"), base.join("spool"));
            std::fs::create_dir_all(root.join("folder")).unwrap();
            RgbaImage::from_pixel(8, 4, Rgba([80, 90, 100, 255])).save(root.join("folder/sheet.png")).unwrap();
            Self { base, root, spool }
        }
        fn prepare(&self) -> Job { prepare(&self.root, &self.spool, provider(Kind::OpenAi), "test:batch".into()).unwrap() }
        fn advance(&self, job: Job, send: impl FnMut(&str, Option<&Value>) -> Result<Value, String>) -> Result<Job, String> {
            advance(job, &self.root, &self.spool, send)
        }
    }
    impl Drop for Files { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.base); } }
    fn provider(kind: Kind) -> Provider {
        Provider { name: "test".into(), kind, key_env: vec![], url: match kind {
            Kind::Gemini => "https://generativelanguage.googleapis.com/v1beta".into(),
            Kind::OpenAi => "https://openrouter.ai/api/v1".into(),
        } }
    }
    fn result(job: &Job, caption: &str) -> Value {
        json!({"status":"completed", "results":job.groups[0].sheets.iter().rev().map(|&i| {
            json!({"custom_id":key(i), "response":{"status_code":200, "body":completion(labeled(caption))}})
        }).collect::<Vec<_>>()})
    }

    #[test]
    fn animated_gifs_use_only_the_first_frame() {
        let files = Files::new();
        let first = RgbaImage::from_pixel(8, 4, Rgba([80, 90, 100, 255]));
        let second = RgbaImage::from_pixel(8, 4, Rgba([200, 10, 20, 255]));
        let path = files.root.join("animated.gif");
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(std::fs::File::create(&path).unwrap());
            encoder.encode_frame(image::Frame::new(first.clone())).unwrap();
            encoder.encode_frame(image::Frame::new(second)).unwrap();
        }
        let job = files.prepare();
        assert_eq!(job.sheets.len(), 2);
        assert!(job.excluded.is_empty());
        let i = job.sheets.iter().position(|s| s.rel == "animated.gif").unwrap();
        assert_eq!(job.sheets[i].identity, labels::identity(&first));
        let ctx = eframe::egui::Context::default();
        let sheet = crate::sheet::Sheet::open(&ctx, &files.root, "animated.gif", [4, 4], Default::default()).unwrap();
        assert_eq!(sheet.label_input().identity, job.sheets[i].identity);
    }

    #[test]
    fn new_library_prepares_locally_and_requires_confirmation() {
        let files = Files::new();
        std::fs::write(files.root.join("broken.png"), b"not an image").unwrap();
        let job = files.prepare();
        assert_eq!(job.sheets.len(), 1);
        assert_eq!(job.excluded.len(), 1);
        assert_eq!(job.sheets[0].rel, "folder/sheet.png");
        assert!(job.bytes > 0);
        assert!(!files.root.join(crate::sidecar::BOOK).exists());
        assert!(!files.spool.join("state.json").exists());
        assert!(files.advance(job, |_, _| panic!("must not send before confirmation")).is_err());
    }

    #[test]
    fn a_batch_submits_resumes_by_id_and_skips_current_labels() {
        let files = Files::new();
        let mut job = files.prepare();
        job.started = true;
        job = files.advance(job, |path, request| {
            assert_eq!(path, "batches");
            assert!(Job::load(&files.spool).unwrap().unwrap().uncertain());
            let request = request.unwrap();
            assert_eq!(request["model"], "test");
            assert_eq!(request["endpoint"], "/v1/chat/completions");
            Ok(json!({"id":"batch-sheet"}))
        }).unwrap();
        let reply = result(&job, "Forest assets");
        let restored = Job::load(&files.spool).unwrap().unwrap();
        job = files.advance(restored, |path, request| {
            assert_eq!(path, "batches/batch-sheet");
            assert!(request.is_none());
            Ok(reply.clone())
        }).unwrap();
        assert!(job.done());
        let label = job.sheets[0].label.clone().unwrap();
        assert_eq!(label.caption, "Forest assets");
        assert_eq!(label.identity, job.sheets[0].identity);
        assert!(!job.sheets[0].imported);
        crate::sidecar::store_label(&files.root, &job.sheets[0].rel, Some(label.clone())).unwrap();
        let again = files.prepare();
        assert_eq!(again.skipped, 1);
        assert!(again.sheets.is_empty());
        let mut image = image::open(files.root.join("folder/sheet.png")).unwrap().to_rgba8();
        image.put_pixel(0, 0, Rgba([255; 4]));
        image.save(files.root.join("folder/sheet.png")).unwrap();
        assert_eq!(files.prepare().sheets.len(), 1);
    }

    #[test]
    fn a_sheet_that_changed_after_preparation_is_not_sent() {
        let files = Files::new();
        let mut job = files.prepare();
        job.started = true;
        RgbaImage::new(8, 4).save(files.root.join("folder/sheet.png")).unwrap();
        let job = files.advance(job, |_, _| panic!("nothing left to send")).unwrap();
        assert!(job.done());
        assert!(job.sheets[0].error.contains("changed"));
    }

    #[test]
    fn interrupted_submission_is_not_sent_twice() {
        let files = Files::new();
        let mut job = files.prepare();
        job.started = true;
        assert!(files.advance(job, |_, _| Err("timeout".into())).is_err());
        let job = Job::load(&files.spool).unwrap().unwrap();
        assert!(job.uncertain());
        assert!(files.advance(job, |_, _| panic!("duplicate paid submission")).is_err());
    }

    #[test]
    fn request_ids_ignore_order_and_missing_results_stay_missing() {
        let files = Files::new();
        let mut job = files.prepare();
        job.sheets.push(job.sheets[0].clone());
        job.groups[0].sheets = vec![0, 1];
        accept(&mut job, 0, vec![(key(1), completion(labeled("Second")))]).unwrap();
        assert!(job.sheets[0].label.is_none());
        assert!(!job.sheets[0].error.is_empty());
        assert_eq!(job.sheets[1].label.as_ref().unwrap().caption, "Second");
        assert!(accept(&mut job, 0, vec![("unknown".into(), completion(labeled("Wrong")))]).is_err());
    }

    #[test]
    fn gemini_request_and_response_use_the_same_structured_label() {
        let request = labels::request("test", &RgbaImage::new(2, 2)).unwrap();
        let gemini = gemini_request(&request);
        assert_eq!(gemini["generationConfig"]["responseMimeType"], "application/json");
        assert!(gemini["generationConfig"]["responseJsonSchema"].is_object());
        assert!(gemini.to_string().contains("inlineData"));
        let response = json!({"done":true, "response":{"inlinedResponses":{"inlinedResponses":[{
            "metadata":{"key":"sheet-0"}, "response":{"candidates":[{"finishReason":"STOP", "content":{"parts":[{
                "text":completion(labeled("Tree"))["choices"][0]["message"]["content"]
            }]}}]}
        }]}}});
        let out = outputs(Kind::Gemini, &response).unwrap().unwrap();
        assert_eq!(out[0].0, "sheet-0");
        assert_eq!(labels::response(&out[0].1).unwrap().into_label("", "", "").caption, "Tree");
        let mut bad = response.clone();
        bad["response"]["inlinedResponses"]["inlinedResponses"][0]["response"]["candidates"][0]["finishReason"] = json!("SAFETY");
        let out = outputs(Kind::Gemini, &bad).unwrap().unwrap();
        assert!(labels::response(&out[0].1).is_err());
        assert!(outputs(Kind::Gemini, &json!({"done":false})).unwrap().is_none());
        assert!(outputs(Kind::OpenAi, &json!({"status":"completed", "results":[{"custom_id":"a"},{"custom_id":"a"}]})).is_err());
    }

    #[test]
    fn rejected_submissions_finish_without_ambiguous_retries() {
        let files = Files::new();
        let mut job = files.prepare();
        job.started = true;
        job = files.advance(job, |_, _| Ok(json!({"submission_rejected":401}))).unwrap();
        assert!(!job.uncertain());
        assert!(job.sheets[0].error.contains("401"));
        assert!(job.sheets[0].label.is_none());
        assert!(job.done());
    }

    #[test]
    fn all_ready_groups_submit_before_polling_and_polls_rotate() {
        let files = Files::new();
        let mut job = files.prepare();
        job.started = true;
        job.groups.push(job.groups[0].clone());
        job.groups[0].remote = Remote::Waiting("first".into());
        job = files.advance(job, |path, body| {
            assert_eq!(path, "batches");
            assert!(body.is_some());
            Ok(json!({"id":"second"}))
        }).unwrap();
        job = files.advance(job, |path, body| {
            assert_eq!(path, "batches/first");
            assert!(body.is_none());
            Ok(json!({"status":"in_progress"}))
        }).unwrap();
        assert!(matches!(&job.groups[0].remote, Remote::Waiting(id) if id == "second"));
        job.sheets[0].imported = true;
        job.save(&files.spool).unwrap();
        assert!(Job::load(&files.spool).unwrap().unwrap().sheets[0].imported);
    }

    #[test]
    fn provider_and_resource_paths_do_not_redirect_keys() {
        assert_eq!(endpoint(&provider(Kind::OpenAi)).unwrap(), "https://openrouter.ai/api/beta");
        assert!(validate_id(Kind::Gemini, "batches/test_1").is_ok());
        for id in ["", "../files", "abc?key=other", "https://other.test", "abc/def"] { assert!(validate_id(Kind::OpenAi, id).is_err()); }
        let mut p = provider(Kind::Gemini);
        p.url = "https://user:password@example.test".into();
        assert!(endpoint(&p).is_err());
    }
}
