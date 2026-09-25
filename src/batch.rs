// SPDX-License-Identifier: GPL-3.0-only
//! One library batch: the sheets without a label, sent through the provider's batch API.
//! Each sheet is one request, the same one that Label with AI sends.
//!
//! The journal in the configuration folder holds the state, so that a batch
//! continues after a restart. It never holds a key. The state of a sheet:
//! not taken yet, then in a group that is submitted, waiting, and done.

use crate::{ai::{Kind, Provider}, index::Index, labels, sidecar::{Label, Status}};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::{Path, PathBuf}, sync::mpsc, time::{Duration, Instant}};

/// The limits of one provider batch.
const MAX_BYTES: usize = 18_000_000;
const MAX_REQUESTS: usize = 100;
/// How often the tool asks the provider about a submitted batch.
const POLL: Duration = Duration::from_secs(30);

#[derive(Clone, Serialize, Deserialize)]
pub struct Sheet {
    pub rel: String,
    /// The sheet is in a group, or it failed before it could join one.
    pub taken: bool,
    pub label: Option<Label>,
    pub error: String,
    /// The label is in the book.
    pub imported: bool,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub enum Remote {
    /// The request went out, and no reply confirmed it yet. The provider
    /// may have made the batch, so the tool does not send it again.
    Submitting,
    Waiting(String),
    Done,
}

/// The sheets that went to the provider in one batch.
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
    /// The sheets that had a label already.
    pub skipped: usize,
}

impl Job {
    fn untaken(&self) -> bool { self.sheets.iter().any(|s| !s.taken) }
    pub fn done(&self) -> bool { !self.untaken() && self.groups.iter().all(|g| g.remote == Remote::Done) }
    pub fn uncertain(&self) -> bool { self.groups.iter().any(|g| g.remote == Remote::Submitting) }
    fn progress(&self) -> String {
        let labeled = self.sheets.iter().filter(|s| s.label.as_ref().is_some_and(|l| l.status == Status::Labeled)).count();
        let failed = self.sheets.iter().filter(|s| !s.error.is_empty()).count();
        format!("Labeled: {labeled} of {}. Failed: {failed}.", self.sheets.len())
    }
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        crate::storage::write_private(&dir.join("state.json"), self)
    }
    fn load(dir: &Path) -> Result<Option<Self>, String> {
        crate::storage::read(&dir.join("state.json"))
    }
}

/// The folder of one library's batch journal, in the configuration folder.
fn directory(root: &Path) -> Result<PathBuf, String> {
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

/// Lists the sheets without a label. It reads no image and sends nothing.
pub fn prepare(index: &Index, provider: Provider, model: String) -> Result<Job, String> {
    endpoint(&provider)?;
    if let Some(error) = &index.error { return Err(error.clone()); }
    let (labeled, open): (Vec<_>, Vec<_>) = index.entries.iter().partition(|e| e.side.label.is_some());
    Ok(Job {
        provider, model: model.trim_end_matches(":batch").into(), groups: vec![], skipped: labeled.len(),
        sheets: open.iter().map(|e| Sheet { rel: e.rel.clone(), taken: false, label: None, error: String::new(), imported: false }).collect(),
    })
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

/// Why a request to the provider failed.
#[derive(Debug)]
pub enum Failure {
    /// The request did not leave this machine: no DNS answer, no connection.
    NotSent(String),
    /// The provider answered with this HTTP status.
    Status(u16),
    /// The request may have arrived: a timeout or a broken connection.
    Unknown(String),
}

impl Failure {
    fn message(&self) -> String {
        match self {
            Failure::NotSent(e) | Failure::Unknown(e) => e.clone(),
            Failure::Status(code) => format!("The batch endpoint returned HTTP {code}."),
        }
    }
}

pub struct Transport { client: ureq::Agent, base: String, key: String, kind: Kind }
impl Transport {
    pub fn new(provider: &Provider, key: String) -> Result<Self, String> {
        Ok(Self { client: labels::agent(), base: endpoint(provider)?, key, kind: provider.kind })
    }
    pub fn send(&self, path: &str, body: Option<&Value>) -> Result<Value, Failure> {
        use ureq::{Error, Timeout};
        let (header, key) = if self.kind == Kind::Gemini { ("x-goog-api-key", self.key.clone()) }
            else { ("Authorization", format!("Bearer {}", self.key)) };
        let url = format!("{}/{path}", self.base);
        let response = match body {
            Some(body) => self.client.post(&url).header(header, &key).send_json(body),
            None => self.client.get(&url).header(header, &key).call(),
        };
        let mut response = response.map_err(|e| match e {
            Error::HostNotFound | Error::ConnectionFailed | Error::Timeout(Timeout::Resolve | Timeout::Connect) =>
                Failure::NotSent("The batch endpoint could not be reached.".into()),
            _ => Failure::Unknown("The connection to the batch endpoint failed or timed out.".into()),
        })?;
        if !response.status().is_success() { return Err(Failure::Status(response.status().as_u16())); }
        response.body_mut().with_config().limit(32_000_000).read_json()
            .map_err(|_| Failure::Unknown("Invalid or oversized batch response.".into()))
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

fn accept(job: &mut Job, group: usize, values: Vec<(String, Value)>) {
    let mut values: std::collections::BTreeMap<_, _> = values.into_iter().collect();
    let (provider, model) = (job.provider.name.clone(), job.model.clone());
    for &i in &job.groups[group].sheets {
        let sheet = &mut job.sheets[i];
        match values.remove(&key(i)).ok_or("No result returned for this request.".into()).and_then(|v| labels::response(&v)) {
            Ok(reply) => {
                let label = reply.into_label(&provider, &model);
                if label.status == Status::Unlabelable { sheet.error = "The model could not label the sheet.".into(); }
                sheet.label = Some(label);
            }
            Err(error) => sheet.error = error,
        }
    }
}

type Send<'a> = &'a mut dyn FnMut(&str, Option<&Value>) -> Result<Value, Failure>;

/// Does one network operation: submit the next group of sheets, or ask about
/// a submitted one. The job is saved before it returns, with or without error.
pub fn advance(mut job: Job, root: &Path, dir: &Path, mut send: impl FnMut(&str, Option<&Value>) -> Result<Value, Failure>) -> (Job, Result<(), String>) {
    let result = if job.uncertain() {
        Err("The provider did not confirm the last submission.".into())
    } else if job.untaken() {
        submit(&mut job, root, dir, &mut send)
    } else {
        poll(&mut job, dir, &mut send)
    };
    (job, result)
}

/// Reads the next sheets, and submits them as one group. The group is saved
/// as `Submitting` before the request goes out, so that a crash cannot send
/// it twice.
fn submit(job: &mut Job, root: &Path, dir: &Path, send: Send) -> Result<(), String> {
    let (mut requests, mut bytes) = (Vec::new(), 0);
    for i in 0..job.sheets.len() {
        if job.sheets[i].taken { continue; }
        if requests.len() == MAX_REQUESTS { break; }
        let body = image::open(root.join(&job.sheets[i].rel)).map_err(|e| format!("Could not read the image: {e}"))
            .and_then(|img| body(job, &img.to_rgba8()));
        let size = body.as_ref().map_or(0, |b| serde_json::to_vec(b).unwrap().len() + 512);
        match body {
            Ok(_) if size > MAX_BYTES => job.sheets[i].error = "The image request is too large.".into(),
            Ok(_) if !requests.is_empty() && bytes + size > MAX_BYTES => break,
            Ok(body) => { bytes += size; requests.push((i, body)); continue; }
            Err(error) => job.sheets[i].error = error,
        }
        job.sheets[i].taken = true;
    }
    if requests.is_empty() { return job.save(dir); }
    let sheets: Vec<_> = requests.iter().map(|(i, _)| *i).collect();
    for &i in &sheets { job.sheets[i].taken = true; }
    job.groups.push(Group { sheets, remote: Remote::Submitting });
    job.save(dir)?;
    let path = match job.provider.kind {
        Kind::Gemini => format!("models/{}:batchGenerateContent", job.model),
        Kind::OpenAi => "batches".into(),
    };
    let requests: Vec<_> = requests.into_iter().map(|(i, body)| (key(i), body)).collect();
    let g = job.groups.len() - 1;
    let result = match send(&path, Some(&submit_body(job, &requests))) {
        Ok(response) => remote_id(job.provider.kind, &response).map(|id| job.groups[g].remote = Remote::Waiting(id)),
        // The provider made no batch: the sheets wait for the next try.
        Err(failure @ (Failure::NotSent(_) | Failure::Status(429 | 503))) => {
            job.send_again();
            Err(failure.message())
        }
        Err(Failure::Status(code)) if (400..500).contains(&code) => {
            for &i in &job.groups[g].sheets { job.sheets[i].error = format!("The provider refused the batch (HTTP {code})."); }
            job.groups[g].remote = Remote::Done;
            Ok(())
        }
        Err(failure) => Err(failure.message()),
    };
    job.save(dir).and(result)
}

fn poll(job: &mut Job, dir: &Path, send: Send) -> Result<(), String> {
    let Some(i) = job.groups.iter().position(|g| matches!(g.remote, Remote::Waiting(_))) else { return Ok(()) };
    let Remote::Waiting(id) = job.groups[i].remote.clone() else { unreachable!() };
    let kind = job.provider.kind;
    validate_id(kind, &id)?;
    let path = if kind == Kind::Gemini { id } else { format!("batches/{id}") };
    let response = send(&path, None).map_err(|f| f.message())?;
    match outputs(kind, &response) {
        // Ask about the other groups first, next time.
        Ok(None) => { job.groups.rotate_left(i + 1); return job.save(dir); }
        Ok(Some(values)) => accept(job, i, values),
        Err(error) => for &s in &job.groups[i].sheets { job.sheets[s].error = error.clone(); },
    }
    job.groups[i].remote = Remote::Done;
    job.save(dir)
}

impl Job {
    /// Forgets an unconfirmed submission. Its sheets go out again.
    fn send_again(&mut self) {
        for group in self.groups.iter().filter(|g| g.remote == Remote::Submitting) {
            for &i in &group.sheets { self.sheets[i].taken = false; }
        }
        self.groups.retain(|g| g.remote != Remote::Submitting);
    }
}

/// The part of the AI panel that runs library batches, and its state
/// between frames. A worker thread does each network operation.
#[derive(Default)]
pub struct Panel {
    /// The library that the batch belongs to.
    pub root: PathBuf,
    dir: PathBuf,
    /// The started batch, as the journal holds it.
    pub job: Option<Job>,
    /// A batch that waits for the user's yes. It is not in the journal yet.
    proposal: Option<Job>,
    task: Option<mpsc::Receiver<(Job, Result<(), String>)>>,
    error: String,
    /// Failures in a row, for the wait before the next try.
    failures: u32,
    next_check: Option<Instant>,
    attach_id: String,
    /// The user asked to try again now; the caller also imports again.
    retry: bool,
}

impl Panel {
    fn running(&self) -> bool { self.job.as_ref().is_some_and(|j| !j.done()) }

    pub fn open(&self) -> bool { self.proposal.is_some() }

    /// Records a failure. The next try waits longer after each one, up to 8 minutes.
    pub fn fail(&mut self, error: String) {
        self.error = error;
        self.failures += 1;
        self.next_check = Some(Instant::now() + POLL * 2u32.pow(self.failures.min(5) - 1));
    }

    /// Runs the batch forward. Returns true when the job changed, so that the
    /// caller imports the new labels.
    pub fn tick(&mut self, ctx: &eframe::egui::Context, root: &Path, keys: &crate::ai::Keys) -> bool {
        let mut changed = std::mem::take(&mut self.retry);
        if self.task.is_none() && !self.running() && self.root != root && !root.as_os_str().is_empty() {
            *self = Panel { root: root.into(), ..Panel::default() };
            match directory(root).and_then(|dir| Ok((Job::load(&dir)?, dir))) {
                Ok((job, dir)) => { self.job = job; self.dir = dir; changed = true; }
                Err(e) => self.error = e,
            }
        }
        if let Some(rx) = &self.task {
            let result = match rx.try_recv() {
                Ok((job, result)) => Some((Some(job), result)),
                Err(mpsc::TryRecvError::Disconnected) => Some((None, Err("The batch worker stopped unexpectedly.".into()))),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some((job, result)) = result {
                self.task = None;
                let soon = job.as_ref().is_some_and(Job::untaken);
                match job {
                    Some(job) if self.job.is_some() => self.job = Some(job),
                    // The user cancelled while the worker ran. The worker saved
                    // the journal once more, and it may have made a batch.
                    Some(job) => { self.forget(&job, keys); return true; }
                    None => {}
                }
                match result {
                    Ok(()) => {
                        self.error.clear();
                        self.failures = 0;
                        self.next_check = Some(Instant::now() + if soon { Duration::ZERO } else { POLL });
                    }
                    Err(e) => self.fail(e),
                }
                changed = true;
            }
        }
        let due = self.next_check.is_none_or(|t| t <= Instant::now());
        if self.task.is_none() && due && let Some(job) = &self.job && !job.done() && !job.uncertain() {
            match job.provider.key(keys).ok_or("The batch provider key is missing in Settings.".to_string())
                .and_then(|key| Transport::new(&job.provider, key)) {
                Ok(transport) => {
                    let (job, root, dir) = (job.clone(), self.root.clone(), self.dir.clone());
                    let (tx, rx) = mpsc::channel();
                    let ctx = ctx.clone();
                    self.task = Some(rx);
                    std::thread::spawn(move || {
                        let _ = tx.send(advance(job, &root, &dir, |path, body| transport.send(path, body)));
                        ctx.request_repaint();
                    });
                }
                Err(e) => self.fail(e),
            }
        }
        if let Some(t) = self.next_check { ctx.request_repaint_after(t.saturating_duration_since(Instant::now())); }
        changed
    }

    /// Writes the labels that arrived into the book, in one write, and returns them.
    pub fn import(&mut self) -> Vec<(String, Option<Label>)> {
        let Some(job) = &mut self.job else { return vec![] };
        let mut labels = Vec::new();
        for sheet in job.sheets.iter_mut().filter(|s| !s.imported && s.label.is_some()) {
            if self.root.join(&sheet.rel).is_file() { labels.push((sheet.rel.clone(), sheet.label.clone())); }
            else { (sheet.error, sheet.imported) = ("The file moved or was removed.".into(), true); }
        }
        if labels.is_empty() { return labels; }
        if let Err(e) = crate::sidecar::store_labels(&self.root, labels.iter().map(|(rel, label)| (rel.as_str(), label.clone()))) {
            self.fail(format!("Could not save the labels: {e}"));
            return vec![];
        }
        for sheet in job.sheets.iter_mut().filter(|s| s.label.is_some()) { sheet.imported = true; }
        if let Err(e) = job.save(&self.dir) { self.fail(e); }
        labels
    }

    /// Forgets the batch here, and asks the provider to cancel what it still runs.
    /// Labels already in the book stay.
    fn cancel(&mut self, keys: &crate::ai::Keys) {
        let Some(job) = self.job.take() else { return };
        // A running worker finishes first; `tick` forgets its result too.
        if self.task.is_none() { self.forget(&job, keys); }
        (self.error, self.failures, self.next_check) = (String::new(), 0, None);
    }

    fn forget(&self, job: &Job, keys: &crate::ai::Keys) {
        let _ = std::fs::remove_file(self.dir.join("state.json"));
        let ids: Vec<_> = job.groups.iter().filter_map(|g| if let Remote::Waiting(id) = &g.remote { Some(id.clone()) } else { None }).collect();
        if let Some(Ok(transport)) = job.provider.key(keys).map(|key| Transport::new(&job.provider, key)) {
            std::thread::spawn(move || for id in ids {
                let path = if transport.kind == Kind::Gemini { format!("{id}:cancel") } else { format!("batches/{id}/cancel") };
                let _ = transport.send(&path, Some(&json!({})));
            });
        }
    }

    /// What the batch does now, in one line.
    fn state(&self, job: &Job) -> String {
        let wait = self.next_check.map_or(0, |t| t.saturating_duration_since(Instant::now()).as_secs());
        if job.uncertain() {
            "The provider did not confirm the last submission.".into()
        } else if self.task.is_some() {
            if job.untaken() { "Sending sheets to the provider...".into() } else { "Asking the provider...".into() }
        } else if !self.error.is_empty() {
            format!("{} Next try in {wait} s.", self.error)
        } else if job.done() {
            "Done.".into()
        } else {
            format!("Waiting for the provider, which can take up to 24 hours. Next check in {wait} s.")
        }
    }

    pub fn ui(&mut self, ui: &mut eframe::egui::Ui, index: &Index, ai: &crate::ai::Ai, keys: &crate::ai::Keys) {
        use eframe::egui;
        ui.separator();
        ui.strong("Library batch");
        let configured = ai.chosen(crate::ai::Mode::Batch);
        let ready = configured.is_some_and(|(p, _)| endpoint(p).is_ok() && p.key_source(keys) != crate::ai::KeySource::None);
        if self.running() && self.root != index.root { ui.weak(format!("For {}", self.root.display())); }
        if let Some(job) = &self.job {
            ui.label(self.state(job));
            ui.label(job.progress());
            if self.running() { ui.ctx().request_repaint_after(Duration::from_secs(1)); }
            egui::CollapsingHeader::new("Failed sheets").show(ui, |ui| {
                for s in job.sheets.iter().filter(|s| !s.error.is_empty()) { ui.label(format!("{}: {}", s.rel, s.error)); }
            });
        } else if !self.error.is_empty() {
            ui.colored_label(egui::Color32::LIGHT_RED, &self.error);
        }
        if self.job.as_ref().is_some_and(Job::uncertain) {
            ui.weak("Find the batch in the provider's batch list and attach its ID. If there is none, send the sheets again.");
            crate::stopped(ui.text_edit_singleline(&mut self.attach_id));
            ui.horizontal(|ui| {
                let kind = self.job.as_ref().unwrap().provider.kind;
                if crate::stopped(ui.button("Attach batch ID")).clicked() {
                    match validate_id(kind, self.attach_id.trim()) {
                        Ok(()) => {
                            let job = self.job.as_mut().unwrap();
                            for g in job.groups.iter_mut().filter(|g| g.remote == Remote::Submitting) {
                                g.remote = Remote::Waiting(self.attach_id.trim().into());
                            }
                            self.retry = true;
                        }
                        Err(e) => self.error = e,
                    }
                }
                if crate::stopped(ui.button("Send again").on_hover_text("No batch was made. The provider may bill twice if one was.")).clicked() {
                    self.job.as_mut().unwrap().send_again();
                    self.retry = true;
                }
            });
            if self.retry && let Err(e) = self.job.as_ref().unwrap().save(&self.dir) { self.error = e; }
        }
        if self.running() {
            ui.horizontal(|ui| {
                if !self.error.is_empty() && crate::stopped(ui.button("Try again now")).clicked() { self.retry = true; }
                if crate::stopped(ui.button("Cancel batch").on_hover_text("Asks the provider to cancel. Labels already saved stay.")).clicked() {
                    self.cancel(keys);
                }
            });
        } else if self.job.is_some() && !self.error.is_empty() && crate::stopped(ui.button("Try again now")).clicked() {
            self.retry = true;
        }
        if self.retry { self.next_check = None; }
        if !ready { ui.weak("Set a Google or OpenRouter batch model and key in Settings."); }
        let button = ui.add_enabled(ready && !self.running() && self.task.is_none() && !index.root.as_os_str().is_empty(),
            egui::Button::new("Label entire library with AI..."));
        crate::stop(&button);
        if button.clicked() {
            let (provider, model) = configured.unwrap();
            // The panel follows the open library while no batch runs; see `tick`.
            match prepare(index, provider.clone(), model.id.clone()) {
                Ok(job) => self.proposal = Some(job),
                Err(e) => self.error = e,
            }
        }
    }

    /// The dialog that asks before a batch starts.
    pub fn confirmation(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui;
        let Some(job) = &self.proposal else { return };
        let (mut start, mut close) = (false, false);
        egui::Modal::new(egui::Id::new("library batch confirmation")).show(ctx, |ui| {
            ui.set_width(430.0);
            ui.heading("Label this entire library?");
            ui.label(self.root.display().to_string());
            ui.weak(format!("{} / {} (from Settings)", job.provider.name, job.model));
            if job.sheets.is_empty() {
                ui.label("Every sheet has a label already.");
            } else {
                ui.label(format!("Sheets to label: {}, one request each.", job.sheets.len()));
                ui.label(format!("Skipped because they have a label: {}.", job.skipped));
                ui.label(format!("At most {} output tokens.", job.sheets.len() * 4096));
                ui.weak("Price estimate unavailable. Image token charges and model output vary.");
                ui.weak("You can close the app while the batch runs; it continues when you open the library again.");
            }
            ui.horizontal(|ui| {
                start = !job.sheets.is_empty() && ui.button("Start batch").clicked();
                close = ui.button(if job.sheets.is_empty() { "Close" } else { "Cancel" }).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if start {
            let job = self.proposal.take().unwrap();
            match job.save(&self.dir) {
                Ok(()) => { self.job = Some(job); (self.error, self.failures, self.next_check) = (String::new(), 0, None); }
                Err(e) => self.error = e,
            }
        }
        if close { self.proposal = None; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::tests::{completion, labeled};
    use base64::{Engine, engine::general_purpose::STANDARD};
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
        fn prepare(&self) -> Job {
            prepare(&Index::scan(&self.root, [16, 16]), provider(Kind::OpenAi), "test:batch".into()).unwrap()
        }
        fn advance(&self, job: Job, send: impl FnMut(&str, Option<&Value>) -> Result<Value, Failure>) -> (Job, Result<(), String>) {
            advance(job, &self.root, &self.spool, send)
        }
        fn journal(&self) -> Job { Job::load(&self.spool).unwrap().unwrap() }
    }
    impl Drop for Files { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.base); } }
    fn provider(kind: Kind) -> Provider {
        Provider { name: "test".into(), kind, key_env: vec![], url: match kind {
            Kind::Gemini => "https://generativelanguage.googleapis.com/v1beta".into(),
            Kind::OpenAi => "https://openrouter.ai/api/v1".into(),
        } }
    }
    /// The provider's answer for a finished group, in reverse order.
    fn completed(job: &Job, group: usize, caption: &str) -> Value {
        json!({"status":"completed", "results":job.groups[group].sheets.iter().rev().map(|&i| {
            json!({"custom_id":key(i), "response":{"status_code":200, "body":completion(labeled(caption))}})
        }).collect::<Vec<_>>()})
    }
    fn id(value: &str) -> impl FnMut(&str, Option<&Value>) -> Result<Value, Failure> {
        let value = value.to_string();
        move |_, _| Ok(json!({"id":value}))
    }

    #[test]
    fn preparation_reads_no_image_and_skips_labeled_sheets() {
        let files = Files::new();
        std::fs::write(files.root.join("broken.png"), b"not an image").unwrap();
        RgbaImage::new(4, 4).save(files.root.join("done.png")).unwrap();
        let label = labels::tests::completion(labeled("Done"));
        let label = labels::response(&label).unwrap().into_label("test", "test");
        crate::sidecar::store_labels(&files.root, [("done.png", Some(label))]).unwrap();
        let job = files.prepare();
        assert_eq!(job.sheets.iter().map(|s| s.rel.as_str()).collect::<Vec<_>>(), ["broken.png", "folder/sheet.png"]);
        assert_eq!(job.skipped, 1);
        assert_eq!(job.model, "test");
        assert!(!files.spool.exists());
    }

    #[test]
    fn a_batch_submits_waits_and_saves_its_labels() {
        let files = Files::new();
        std::fs::write(files.root.join("broken.png"), b"not an image").unwrap();
        let (job, result) = files.advance(files.prepare(), |path, request| {
            assert_eq!(path, "batches");
            assert!(files.journal().uncertain(), "the journal says Submitting before the request goes out");
            let request = request.unwrap();
            assert_eq!(request["model"], "test");
            assert_eq!(request["requests"].as_array().unwrap().len(), 1);
            Ok(json!({"id":"batch-1"}))
        });
        result.unwrap();
        assert!(job.sheets[0].error.contains("Could not read"));
        assert!(matches!(&files.journal().groups[0].remote, Remote::Waiting(id) if id == "batch-1"));
        let (job, result) = files.advance(job, |path, request| {
            assert_eq!(path, "batches/batch-1");
            assert!(request.is_none());
            Ok(json!({"status":"in_progress"}))
        });
        result.unwrap();
        assert!(!job.done());
        let reply = completed(&job, 0, "Forest");
        let (job, result) = files.advance(job, |_, _| Ok(reply.clone()));
        result.unwrap();
        assert!(job.done());
        assert_eq!(job.sheets[1].label.as_ref().unwrap().caption, "Forest");
        assert!(files.journal().done());
    }

    #[test]
    fn animated_gifs_send_the_first_frame() {
        let files = Files::new();
        std::fs::remove_file(files.root.join("folder/sheet.png")).unwrap();
        let first = RgbaImage::from_pixel(8, 4, Rgba([80, 90, 100, 255]));
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(std::fs::File::create(files.root.join("animated.gif")).unwrap());
            encoder.encode_frame(image::Frame::new(first.clone())).unwrap();
            encoder.encode_frame(image::Frame::new(RgbaImage::from_pixel(8, 4, Rgba([200, 10, 20, 255])))).unwrap();
        }
        let (_, result) = files.advance(files.prepare(), |_, request| {
            let url = request.unwrap()["requests"][0]["body"]["messages"][1]["content"][1]["image_url"]["url"].as_str().unwrap().to_string();
            let png = STANDARD.decode(url.strip_prefix("data:image/png;base64,").unwrap()).unwrap();
            assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8(), first);
            Ok(json!({"id":"batch-1"}))
        });
        result.unwrap();
    }

    #[test]
    fn a_submission_that_never_left_goes_out_again() {
        let files = Files::new();
        for failure in [Failure::NotSent("offline".into()), Failure::Status(429)] {
            let mut failure = Some(failure);
            let (job, result) = files.advance(files.prepare(), |_, _| Err(failure.take().unwrap()));
            assert!(result.is_err());
            assert!(job.groups.is_empty() && !job.sheets[0].taken);
            assert!(!files.journal().uncertain());
            let (job, result) = files.advance(job, id("batch-2"));
            result.unwrap();
            assert!(matches!(&job.groups[0].remote, Remote::Waiting(_)));
        }
    }

    #[test]
    fn an_unconfirmed_submission_is_not_sent_twice() {
        let files = Files::new();
        let (_, result) = files.advance(files.prepare(), |_, _| Err(Failure::Unknown("timeout".into())));
        assert!(result.is_err());
        let mut job = files.journal();
        assert!(job.uncertain());
        let (_, result) = files.advance(job.clone(), |_, _| panic!("a paid submission must not go out twice"));
        assert!(result.is_err());
        job.send_again();
        let (job, result) = files.advance(job, id("batch-3"));
        result.unwrap();
        assert!(!job.uncertain());
    }

    #[test]
    fn a_refused_submission_fails_its_sheets() {
        let files = Files::new();
        let (job, result) = files.advance(files.prepare(), |_, _| Err(Failure::Status(401)));
        result.unwrap();
        assert!(job.sheets[0].error.contains("401"));
        assert!(job.done());
    }

    #[test]
    fn results_follow_their_ids_and_missing_results_fail() {
        let files = Files::new();
        let mut job = files.prepare();
        job.sheets.push(job.sheets[0].clone());
        job.groups.push(Group { sheets: vec![0, 1], remote: Remote::Waiting("x".into()) });
        accept(&mut job, 0, vec![(key(1), completion(labeled("Second"))), ("unknown".into(), Value::Null)]);
        assert!(job.sheets[0].label.is_none());
        assert!(!job.sheets[0].error.is_empty());
        assert_eq!(job.sheets[1].label.as_ref().unwrap().caption, "Second");
    }

    #[test]
    fn polls_take_turns() {
        let files = Files::new();
        let mut job = files.prepare();
        job.sheets[0].taken = true;
        job.groups = vec![Group { sheets: vec![0], remote: Remote::Waiting("first".into()) },
            Group { sheets: vec![0], remote: Remote::Waiting("second".into()) }];
        let (job, _) = files.advance(job, |path, _| { assert_eq!(path, "batches/first"); Ok(json!({"status":"in_progress"})) });
        let (_, _) = files.advance(job, |path, _| { assert_eq!(path, "batches/second"); Ok(json!({"status":"in_progress"})) });
    }

    #[test]
    fn failures_wait_longer_each_time() {
        let mut panel = Panel::default();
        let mut waits = Vec::new();
        for _ in 0..7 {
            panel.fail("offline".into());
            waits.push(panel.next_check.unwrap().saturating_duration_since(Instant::now()).as_secs() + 1);
        }
        assert_eq!(waits, [30, 60, 120, 240, 480, 480, 480]);
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
        assert_eq!(labels::response(&out[0].1).unwrap().into_label("", "").caption, "Tree");
        let mut bad = response.clone();
        bad["response"]["inlinedResponses"]["inlinedResponses"][0]["response"]["candidates"][0]["finishReason"] = json!("SAFETY");
        let out = outputs(Kind::Gemini, &bad).unwrap().unwrap();
        assert!(labels::response(&out[0].1).is_err());
        assert!(outputs(Kind::Gemini, &json!({"done":false})).unwrap().is_none());
        assert!(outputs(Kind::OpenAi, &json!({"status":"completed", "results":[{"custom_id":"a"},{"custom_id":"a"}]})).is_err());
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
