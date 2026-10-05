// SPDX-License-Identifier: GPL-3.0-only
//! One library job: selected sheets, each sent as the request that
//! Label with AI sends. Google's Gemini API takes them as a batch. An
//! OpenAI-style endpoint takes them as an OpenRouter batch when the provider
//! has object storage; without it, several ordinary requests run at once.
//!
//! The library book holds the job state, so that a batch
//! continues after a restart. It never holds a key. The state of a sheet:
//! not taken yet, then in a group that is submitted, waiting, and done.

use crate::{ai::{Kind, Provider}, index::Index, labels, sidecar::{Label, Status}};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
mod cost;
mod openrouter;
mod runner;
mod store;

use std::{collections::{BTreeSet, BTreeMap}, path::{Path, PathBuf}, sync::mpsc, time::{Duration, Instant}};

/// The limits of one provider batch.
const MAX_BYTES: usize = 18_000_000;
const MAX_REQUESTS: usize = 100;
/// How often the tool asks the provider about a submitted batch.
const POLL: Duration = Duration::from_secs(30);

/// A loaded job that predates per-model concurrency keeps the default.
fn default_concurrency() -> u32 { crate::ai::DEFAULT_CONCURRENCY }

#[derive(Clone, Serialize, Deserialize)]
pub struct Sheet {
    pub rel: String,
    /// The sheet is in a group, or it failed before it could join one.
    pub taken: bool,
    pub label: Option<Label>,
    pub error: String,
    /// The label is in the book.
    pub imported: bool,
    /// Requests for this sheet that may have arrived but gave no answer the
    /// tool could read. See `one`.
    #[serde(default)]
    pub unknown: u32,
    /// A transient failure of this sheet's own request waits until this time
    /// before it goes out again.
    #[serde(default)]
    retry_ms: u64,
    /// How many transient failures this sheet had, for the wait it grows.
    #[serde(default)]
    attempts: u32,
    #[serde(default)]
    cancelled: bool,
    #[serde(default)]
    guard: Option<InputGuard>,
    /// The object key of this sheet in the batch bucket, while its batch runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stored: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct InputGuard { hash: String, label: Option<Label> }

#[derive(Clone, Default, Serialize, Deserialize)]
struct Tracking {
    #[serde(default)]
    id: String,
    #[serde(default)]
    error: String,
    state: String,
    checked_ms: u64,
    cancel_sent: bool,
    recoveries: u32,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
enum Mode { #[default] Running, Paused, Cancelling }

#[derive(Clone, Default, Serialize, Deserialize)]
struct Issue { message: String, attempts: u32, retry_ms: u64 }

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// How many requests of one sheet may end without a readable answer. Each
/// may have been billed, so the sheet fails after this many.
const UNKNOWN_TRIES: u32 = 3;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
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
    #[serde(default)]
    recovery: Option<Recovery>,
    #[serde(default)]
    tracking: Tracking,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Recovery {
    reference: String,
    #[serde(default)]
    page: String,
    #[serde(default)]
    matches: Vec<String>,
}

fn key(sheet: usize) -> String { format!("sheet-{sheet}") }

/// A job journal from before the count existed reads as the default.
fn default_free_tags() -> usize { crate::sidecar::FREE_TAGS }

#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    #[serde(default)]
    log_id: String,
    pub provider: Provider,
    pub model: String,
    /// How many ordinary requests the job keeps in flight. Only an
    /// OpenAI-style provider reads it; a Gemini job submits batches.
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,
    pub sheets: Vec<Sheet>,
    pub groups: Vec<Group>,
    /// The sheets that had a label already.
    pub skipped: usize,
    /// Existing labels that this job will replace.
    #[serde(default)]
    pub replacing: usize,
    /// The library's tag list when the job started. Every request of the job
    /// asks for it, and every label records it.
    #[serde(default)]
    pub tag_list: Vec<String>,
    /// The free-tag count when the job started. Every request of the job asks
    /// for it.
    #[serde(default = "default_free_tags")]
    pub free_tags: usize,
    #[serde(default)]
    prompt: Option<[String; 2]>,
    /// Older jobs keep their original requests without file context.
    #[serde(default)]
    file_context: bool,
    /// Alternate submissions and polling so early results can arrive before the whole library is sent.
    #[serde(default)]
    poll_next: bool,
    /// Give queued sheets a turn after each recovery request.
    #[serde(default)]
    submit_next: bool,
    #[serde(default)]
    mode: Mode,
    #[serde(default)]
    control_revision: u64,
    #[serde(default)]
    issues: BTreeMap<String, Issue>,
    #[serde(default)]
    last_response_ms: u64,
    #[serde(default)]
    poll_ms: u64,
    #[serde(default)]
    recovery_ms: u64,
    #[serde(default)]
    estimate: Option<cost::Estimate>,
    #[serde(default)]
    usage: cost::Usage,
    /// The object storage for this run. It holds the secret, so it is never
    /// saved with the job.
    #[serde(skip)]
    objects: Option<std::sync::Arc<dyn openrouter::Objects>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation { Label, Submit, Poll, Recover, Cancel }

impl Job {
    fn operation(&self) -> Operation {
        let waiting = self.groups.iter().any(|g| matches!(g.remote, Remote::Waiting(_)));
        if self.provider.kind == Kind::OpenAi && self.provider.store.is_none() { Operation::Label }
        else if waiting && (self.poll_next || (!self.untaken() && !self.uncertain())) { Operation::Poll }
        else if self.uncertain() && (!self.submit_next || !self.untaken()) { Operation::Recover }
        else if self.untaken() { Operation::Submit }
        else { Operation::Poll }
    }

    fn untaken(&self) -> bool { self.sheets.iter().any(|s| !s.taken) }
    fn remote_done(&self) -> bool { !self.untaken() && self.groups.iter().all(|g| g.remote == Remote::Done) }
    fn pending_save(&self) -> bool { self.sheets.iter().any(|s| s.label.is_some() && !s.imported) }
    pub fn done(&self) -> bool { self.remote_done() && !self.pending_save() }
    fn saved(&self) -> usize {
        self.sheets.iter().filter(|s| s.imported && s.label.as_ref().is_some_and(|l| l.status == Status::Labeled)).count()
    }
    fn outcomes(&self) -> [usize; 4] {
        let mut counts = [0; 4];
        for sheet in &self.sheets {
            if sheet.imported && let Some(label) = &sheet.label {
                counts[if label.status == Status::Labeled { 0 } else { 2 }] += 1;
            } else if sheet.cancelled { counts[3] += 1; }
            else if sheet.label.is_none() && !sheet.error.is_empty() { counts[1] += 1; }
        }
        counts
    }

    /// A Gemini batch went out and no reply confirmed it. An OpenAI-style job
    /// is never so: its groups are released; see `release_groups`.
    pub fn uncertain(&self) -> bool {
        self.provider.kind == Kind::Gemini && self.groups.iter().any(|g| g.remote == Remote::Submitting)
    }
    fn submission_counts(&self) -> (usize, usize, usize) {
        let queued = self.sheets.iter().filter(|s| !s.taken).count();
        let waiting = self.groups.iter().filter(|g| matches!(g.remote, Remote::Waiting(_))).map(|g| g.sheets.len()).sum();
        let uncertain = self.groups.iter().filter(|g| g.remote == Remote::Submitting).map(|g| g.sheets.len()).sum();
        (queued, waiting, uncertain)
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        store::save(dir, self)
    }
    fn load(dir: &Path) -> Result<Option<Self>, String> {
        store::load(dir)
    }
}

/// The folder of one library's batch journal, in the configuration folder.
fn legacy_directory(root: &Path) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let hash = Sha256::digest(root.as_os_str().as_encoded_bytes());
    Ok(crate::settings::dir().ok_or("No configuration directory is available.")?.join("batches").join(format!("{hash:x}")))
}

/// The request body for one sheet in a Gemini batch.
fn body(job: &Job, img: &image::RgbaImage, rel: &str) -> Result<Value, String> {
    Ok(crate::gemini::request(&chat_request(job, img, rel)?))
}

fn chat_request(job: &Job, img: &image::RgbaImage, rel: &str) -> Result<Value, String> {
    let mut request = labels::request(&job.model, img, &job.tag_list, job.free_tags)?;
    let texts = job.prompt.clone().unwrap_or_else(|| labels::prompt(&job.tag_list, job.free_tags));
    let [system, user] = if job.file_context { labels::sheet_prompt(texts, rel) } else { texts };
    request["messages"][0]["content"] = json!(system);
    request["messages"][1]["content"][0]["text"] = json!(user);
    Ok(request)
}

/// The request for one sheet. `existing` is the label the book held when the
/// request was built; it guards the save of the reply. The caller reads the
/// book once for many sheets instead of once for each.
fn image_request(job: &mut Job, root: &Path, i: usize, existing: Option<Label>) -> Result<Value, String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(root.join(&job.sheets[i].rel)).map_err(|e| format!("Could not read the image: {e}"))?;
    let image = image::load_from_memory(&bytes).map_err(|e| format!("Could not read the image: {e}"))?;
    job.sheets[i].guard = Some(InputGuard { hash: format!("{:x}", Sha256::digest(&bytes)), label: existing });
    if job.provider.kind == Kind::Gemini { body(job, &image.to_rgba8(), &job.sheets[i].rel) }
    else { Ok(job.provider.route(chat_request(job, &image.to_rgba8(), &job.sheets[i].rel)?)) }
}

fn save_labels(job: &Job, root: &Path) -> (runner::SaveReport, Vec<(String, Option<Label>)>) {
    use sha2::{Digest, Sha256};
    let mut report = runner::SaveReport { saved: vec![], rejected: vec![], error: None };
    let mut labels = Vec::new();
    let saved = crate::sidecar::update_book(root, |book| {
    for (i, sheet) in job.sheets.iter().enumerate().filter(|(_, s)| !s.imported && s.label.is_some()) {
        let current = book.sheets.get(&sheet.rel).and_then(|s| s.label.as_ref());
        if current == sheet.label.as_ref() { report.saved.push(i); continue; }
        let checked = std::fs::read(root.join(&sheet.rel)).map_err(|_| "The image moved or was removed.".to_string()).and_then(|bytes| {
            if let Some(guard) = &sheet.guard {
                if format!("{:x}", Sha256::digest(&bytes)) != guard.hash { return Err("The image changed after submission.".into()); }
                if current != guard.label.as_ref() { return Err("The label changed after submission. The newer label was kept.".into()); }
            } else if current.is_some() {
                return Err("An existing label was kept because this older job has no saved label revision.".into());
            }
            Ok(())
        });
        match checked {
            Ok(()) => {
                if sheet.label.as_ref().is_some_and(|l| l.status == Status::Unlabelable)
                    && current.is_some_and(|l| l.status == Status::Labeled) { report.saved.push(i); continue; }
                book.sheets.entry(sheet.rel.clone()).or_default().label = sheet.label.clone();
                labels.push((sheet.rel.clone(), sheet.label.clone())); report.saved.push(i);
            }
            Err(error) => report.rejected.push((i, error)),
        }
    }
        Ok(())
    });
    if let Err(error) = saved {
        report.saved.clear(); report.error = Some(format!("Could not save labels: {error}")); labels.clear();
    }
    if !labels.is_empty() { crate::ai_log::event("batch_saved", json!({"provider":job.provider.name, "labels":labels})); }
    (report, labels)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Scope { Unlabeled, All }

/// Lists the requested sheets. It reads no image and sends nothing.
pub fn prepare(index: &Index, provider: Provider, model: String, scope: Scope) -> Result<Job, String> {
    prepare_of(index, provider, model, scope, None)
}

/// Lists the requested sheets, kept to `only` when it is given: the paths of
/// the chosen files, below the library root. It reads no image and sends
/// nothing.
pub fn prepare_of(index: &Index, provider: Provider, model: String, scope: Scope, only: Option<&BTreeSet<String>>) -> Result<Job, String> {
    endpoint(&provider)?;
    if let Some(error) = &index.error { return Err(error.clone()); }
    let keep = |e: &crate::index::Entry| only.is_none_or(|set| set.contains(&e.rel));
    let labeled = index.entries.iter().filter(|e| keep(e) && e.side.label.is_some()).count();
    let all = scope == Scope::All;
    let open = index.entries.iter().filter(|e| keep(e) && (all || e.side.label.is_none()));
    Ok(Job {
        log_id: crate::ai_log::id(),
        provider, model: model.trim_end_matches(":batch").into(), concurrency: crate::ai::DEFAULT_CONCURRENCY,
        groups: vec![], skipped: if all { 0 } else { labeled },
        replacing: if all { labeled } else { 0 }, tag_list: index.tag_list.clone(), free_tags: index.free_tags,
        prompt: Some(labels::prompt(&index.tag_list, index.free_tags)),
        file_context: true, poll_next: false, submit_next: false, mode: Mode::Running, control_revision: 0,
        issues: BTreeMap::new(), last_response_ms: 0, poll_ms: 0, recovery_ms: 0, estimate: None, usage: cost::Usage::default(),
        objects: None,
        sheets: open.map(|e| Sheet { rel: e.rel.clone(), taken: false, label: None, error: String::new(),
            imported: false, unknown: 0, retry_ms: 0, attempts: 0, cancelled: false, guard: None, stored: None })
            .collect(),
    })
}


/// The URL the requests of a job go under. An OpenAI-style one loses a
/// `/chat/completions` it ends in, since that is the path of each request.
pub fn endpoint(provider: &Provider) -> Result<String, String> {
    let url = labels::checked_url(&provider.url)?;
    Ok(match provider.kind {
        Kind::Gemini => url,
        Kind::OpenAi => url.trim_end_matches("/chat/completions").into(),
    })
}

fn submit_body(requests: &[(String, Value)], reference: &str) -> Value {
    json!({"batch":{"displayName":reference, "inputConfig":{"requests":{"requests":
        requests.iter().map(|(key, body)| json!({"metadata":{"key":key}, "request":body})).collect::<Vec<_>>()}}}})
}

fn remote_id(response: &Value) -> Result<String, String> {
    let id = response["name"].as_str().ok_or("The batch ID is missing.")?;
    validate_id(id)?;
    Ok(id.into())
}

/// A Gemini batch ID: `batches/` and letters, digits, `_` and `-`.
pub fn validate_id(id: &str) -> Result<(), String> {
    let tail = id.strip_prefix("batches/").ok_or("Expected a batches/... ID.")?;
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
    /// The provider answered with this HTTP status and explanation.
    Status(u16, String),
    /// The request may have arrived: a timeout or a broken connection.
    Unknown(String),
}

impl Failure {
    fn message(&self) -> String {
        match self {
            Failure::NotSent(e) | Failure::Unknown(e) => e.clone(),
            Failure::Status(code, message) => if message.is_empty() { format!("The batch endpoint returned HTTP {code}.") } else { message.clone() },
        }
    }
}

pub struct Transport { client: ureq::Agent, base: String, key: String, kind: Kind }
impl Transport {
    pub fn new(provider: &Provider, key: String) -> Result<Self, String> {
        Ok(Self { client: labels::agent(), base: endpoint(provider)?, key, kind: provider.kind })
    }
    pub fn send(&self, path: &str, body: Option<&Value>) -> Result<Value, Failure> {
        let trace = crate::ai_log::Request::start(&format!("{}/{path}", self.base), body, &self.key);
        let mut status = None;
        let result = self.send_inner(path, body, &mut status);
        let logged = result.as_ref().map(|value| {
            if path.starts_with("batches?") {
                json!({"listed_batches":value["operations"].as_array().map_or(0, Vec::len),
                    "more_pages":value["nextPageToken"].as_str().is_some_and(|s| !s.is_empty())})
            } else { value.clone() }
        }).map_err(Failure::message);
        trace.finish(status, &logged);
        result
    }

    fn send_inner(&self, path: &str, body: Option<&Value>, status: &mut Option<u16>) -> Result<Value, Failure> {
        #[cfg(test)]
        if let Ok(base) = std::env::var("TILEPICKY_TEST_TRANSPORT") {
            let uri: ureq::http::Uri = base.parse().map_err(|_| Failure::NotSent("Invalid test endpoint.".into()))?;
            if uri.scheme_str() != Some("http") || uri.host() != Some("127.0.0.1") {
                return Err(Failure::NotSent("The UI fixture requires a loopback endpoint.".into()));
            }
            return self.send_at(&base, "test-key", path, body, status);
        }
        self.send_at(&self.base, &self.key, path, body, status)
    }

    fn send_at(&self, base: &str, credential: &str, path: &str, body: Option<&Value>, status: &mut Option<u16>) -> Result<Value, Failure> {
        use ureq::{Error, Timeout};
        let (header, key) = if self.kind == Kind::Gemini { ("x-goog-api-key", credential.to_string()) }
            else { ("Authorization", format!("Bearer {credential}")) };
        let url = format!("{base}/{path}");
        let response = match body {
            Some(body) => self.client.post(&url).header(header, &key).send_json(body),
            None => self.client.get(&url).header(header, &key).call(),
        };
        let mut response = response.map_err(|e| match e {
            Error::HostNotFound | Error::ConnectionFailed | Error::Timeout(Timeout::Resolve | Timeout::Connect) =>
                Failure::NotSent("The batch endpoint could not be reached.".into()),
            _ => Failure::Unknown("The connection to the batch endpoint failed or timed out.".into()),
        })?;
        *status = Some(response.status().as_u16());
        if !response.status().is_success() {
            return Err(Failure::Status(response.status().as_u16(), labels::http_error(&mut response, &self.key)));
        }
        let bytes = response.body_mut().with_config().limit(32_000_000).read_to_vec().map_err(|e| {
            let message = match e {
                Error::Timeout(_) => "The batch request timed out while reading the response.",
                Error::BodyExceedsLimit(_) => "The batch response exceeded the size limit.",
                _ => "Could not read the batch response.",
            };
            Failure::Unknown(message.into())
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            crate::ai_log::event("invalid_http_body", json!({"url":url,
                "body":String::from_utf8_lossy(&bytes), "error":error.to_string()}));
            Failure::Unknown("The batch endpoint returned invalid JSON.".into())
        })
    }
}

/// The results of a finished Gemini batch, each as the chat completion that
/// `labels::response` reads. None while the batch still runs.
fn outputs(value: &Value) -> Result<Option<Vec<(String, Value)>>, String> {
    if value["done"] != true { return Ok(None); }
    if !value["error"].is_null() {
        return Err(format!("The provider batch failed. {}", labels::error_detail(&value["error"], "")).trim_end().into());
    }
    let values = value.pointer("/response/inlinedResponses/inlinedResponses").and_then(Value::as_array)
        .ok_or("The completed batch has no inline results.")?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in values {
        let key = item["metadata"]["key"].as_str().ok_or("A batch result has no request ID.")?;
        if !seen.insert(key.to_string()) { return Err("Duplicate batch request ID.".into()); }
        if !item["error"].is_null() {
            out.push((key.into(), json!({"error":item["error"]})));
            continue;
        }
        let mut response = crate::gemini::response(&item["response"]);
        response["usageMetadata"] = item["response"]["usageMetadata"].clone();
        out.push((key.into(), response));
    }
    Ok(Some(out))
}

/// The OpenRouter batch body for the next sheets, up to `limit`. Each request
/// carries the same chat body that the ordinary path sends, with the base64
/// image, or the given public URL in its place.
#[cfg(test)]
fn openai_batch_body(job: &Job, root: &Path, limit: usize, image_url: Option<&str>) -> Result<Value, String> {
    let mut requests = Vec::new();
    for (i, sheet) in job.sheets.iter().enumerate() {
        if sheet.taken { continue; }
        if requests.len() == limit { break; }
        let bytes = std::fs::read(root.join(&sheet.rel)).map_err(|e| format!("Could not read the image: {e}"))?;
        let image = image::load_from_memory(&bytes).map_err(|e| format!("Could not read the image: {e}"))?;
        let mut body = chat_request(job, &image.to_rgba8(), &sheet.rel)?;
        if let Some(url) = image_url { body["messages"][1]["content"][1]["image_url"]["url"] = json!(url); }
        requests.push(json!({"custom_id": key(i), "body": body}));
    }
    Ok(json!({"endpoint":"/v1/chat/completions", "model": job.model, "requests": requests}))
}

/// Submits one prepared OpenRouter batch body and returns its ID.
#[cfg(test)]
fn openai_batch_post(body: &Value, send: Send) -> Result<String, String> {
    let response = send("batches", Some(body)).map_err(|f| f.message())?;
    response["id"].as_str().map(str::to_string).ok_or_else(|| format!("The batch was not accepted: {response}"))
}

/// The results of a finished OpenRouter batch, each as the chat completion
/// that `labels::diagnosed_response` reads. None while the batch still runs.
#[cfg(test)]
fn openai_batch_outputs(value: &Value) -> Result<Option<Vec<(String, Value)>>, String> {
    match value["status"].as_str().unwrap_or_default() {
        "completed" => {
            let results = value["results"].as_array().ok_or("The completed batch has no results.")?;
            let mut out = Vec::new();
            for item in results {
                let id = item["custom_id"].as_str().ok_or("A batch result has no request ID.")?.to_string();
                if !item["error"].is_null() { out.push((id, json!({"error":item["error"]}))); continue; }
                out.push((id, item["response"]["body"].clone()));
            }
            Ok(Some(out))
        }
        "failed" | "expired" | "cancelled" => Err(format!("The batch is {}. {}",
            value["status"].as_str().unwrap_or_default(), value["error"]["message"].as_str().unwrap_or_default()).trim().to_string()),
        _ => Ok(None),
    }
}

/// Writes what a request gave back into the sheet: a label, or the reason
/// there is none.
fn record(sheet: &mut Sheet, job: (&str, &str, &[String]), reply: Result<labels::Reply, String>) {
    match reply {
        Ok(reply) => {
            let label = reply.into_label(job.0, job.1, job.2);
            if label.status == Status::Unlabelable { sheet.error = "The model could not label the sheet.".into(); }
            sheet.label = Some(label);
        }
        Err(error) => sheet.error = error,
    }
}

fn accept(job: &mut Job, group: usize, values: Vec<(String, Value)>) {
    let mut values: std::collections::BTreeMap<_, _> = values.into_iter().collect();
    for &i in &job.groups[group].sheets {
        let reply = values.remove(&key(i)).ok_or("No result returned for this request.".into()).and_then(|v| {
            job.usage.record(&v);
            labels::diagnosed_response(&job.sheets[i].rel, &job.provider.name, &job.model, &v, &job.tag_list)
        });
        record(&mut job.sheets[i], (&job.provider.name, &job.model, &job.tag_list), reply);
    }
}

type Send<'a> = &'a mut dyn FnMut(&str, Option<&Value>) -> Result<Value, Failure>;

/// Does one network operation: label the next sheet, submit the next group
/// of sheets, or ask about a submitted one. The job is saved before it
/// returns, with or without error.
#[cfg(test)]
pub fn advance(job: Job, root: &Path, dir: &Path, mut send: impl FnMut(&str, Option<&Value>) -> Result<Value, Failure>) -> (Job, Result<(), String>) {
    let operation = job.operation();
    advance_operation(job, root, dir, operation, &mut send)
}

fn advance_operation(mut job: Job, root: &Path, dir: &Path, operation: Operation, send: Send) -> (Job, Result<(), String>) {
    let result = match operation {
        Operation::Label => { job.release_groups(); one(&mut job, root, dir, send) }
        Operation::Poll => { job.poll_next = false; if job.provider.kind == Kind::OpenAi { openrouter::poll(&mut job, dir, send) } else { poll(&mut job, dir, send) } }
        Operation::Recover => { job.poll_next = true; job.submit_next = true; recover(&mut job, dir, send) }
        Operation::Submit => { job.poll_next = true; job.submit_next = false;
            if job.provider.kind == Kind::OpenAi { openrouter::submit(&mut job, root, dir, send) } else { submit(&mut job, root, dir, send) } }
        Operation::Cancel => if job.provider.kind == Kind::OpenAi { openrouter::cancel(&mut job, dir) } else { cancel_remote(&mut job, dir, send) },
    };
    (job, result)
}

/// Labels the next sheet with one ordinary request. A request that did not
/// arrive, or that the provider could not take now, goes out again for the
/// same sheet. One whose answer was lost or could not be read may have been
/// billed: it goes out again, up to `UNKNOWN_TRIES` times in all.
fn one(job: &mut Job, root: &Path, dir: &Path, send: Send) -> Result<(), String> {
    let Some(i) = job.sheets.iter().position(|s| !s.taken) else { return Ok(()) };
    crate::ai_log::event("batch_sheet_start", json!({"sheet":job.sheets[i].rel, "provider":job.provider.name,
        "model":job.model, "attempt":job.sheets[i].unknown + 1, "tags_requested":job.tag_list}));
    let book = crate::sidecar::load_book(root)?;
    let existing = book.sheets.get(&job.sheets[i].rel).and_then(|s| s.label.clone());
    let request = image_request(job, root, i, existing);
    job.save(dir)?;
    let reply = match request {
        // The image could not be read: the sheet ends with that reason.
        Err(error) => {
            record(&mut job.sheets[i], (&job.provider.name, &job.model, &job.tag_list), Err(error));
            job.sheets[i].taken = true;
            return job.save(dir);
        }
        Ok(request) => send("chat/completions", Some(&request)),
    };
    finish_one(job, dir, i, reply, now_ms())
}

/// Reads one reply into its sheet. A transient failure leaves the sheet for
/// another try; a billed one counts against `UNKNOWN_TRIES`.
fn finish_one(job: &mut Job, dir: &Path, i: usize, reply: Result<Value, Failure>, now: u64) -> Result<(), String> {
    // One provider of a model on OpenRouter may answer in prose. The
    // next request may reach another.
    let reply = match reply {
        Ok(value) => {
            job.usage.record(&value);
            match labels::diagnosed_response(&job.sheets[i].rel, &job.provider.name, &job.model, &value, &job.tag_list) {
            Err(error) if error == labels::INVALID && job.sheets[i].unknown + 1 < UNKNOWN_TRIES => {
                job.sheets[i].unknown += 1;
                return job.save(dir);
            }
            reply => reply,
            }
        },
        Err(Failure::Unknown(message)) if job.sheets[i].unknown + 1 < UNKNOWN_TRIES => {
            job.sheets[i].unknown += 1;
            return job.save(dir).and(Err(message));
        }
        Err(Failure::Unknown(message)) => Err(format!("{message} No readable answer after {UNKNOWN_TRIES} tries.")),
        Err(failure @ (Failure::NotSent(_) | Failure::Status(401 | 403 | 429 | 500..=599, _))) => {
            // This sheet waits a while before it goes out again; its
            // neighbors keep their turn.
            let sheet = &mut job.sheets[i];
            sheet.attempts = sheet.attempts.saturating_add(1);
            sheet.retry_ms = now + 30_000 * (1u64 << sheet.attempts.min(5).saturating_sub(1));
            return job.save(dir).and(Err(failure.message()));
        }
        Err(failure @ Failure::Status(..)) => Err(failure.message()),
    };
    record(&mut job.sheets[i], (&job.provider.name, &job.model, &job.tag_list), reply);
    job.sheets[i].taken = true;
    job.save(dir)
}

/// Reads the next sheets into one Gemini batch, and saves the group as
/// `Submitting` before the request goes out, so that a crash cannot send it
/// twice. It reads the images and sends nothing. Returns the group's
/// reference, its path, and its body, or `None` when no sheet is left.
fn prepare_submission(job: &mut Job, root: &Path, dir: &Path) -> Result<Option<(String, String, Value)>, String> {
    let (mut requests, mut bytes) = (Vec::new(), 0);
    let book = crate::sidecar::load_book(root)?;
    for i in 0..job.sheets.len() {
        if job.sheets[i].taken { continue; }
        if requests.len() == MAX_REQUESTS { break; }
        let existing = book.sheets.get(&job.sheets[i].rel).and_then(|s| s.label.clone());
        let body = image_request(job, root, i, existing);
        let size = body.as_ref().map_or(0, |b| serde_json::to_vec(b).unwrap().len() + 512);
        match body {
            Ok(_) if size > MAX_BYTES => job.sheets[i].error = "The image request is too large.".into(),
            Ok(_) if !requests.is_empty() && bytes + size > MAX_BYTES => break,
            Ok(body) => { bytes += size; requests.push((i, body)); continue; }
            Err(error) => job.sheets[i].error = error,
        }
        job.sheets[i].taken = true;
    }
    if requests.is_empty() { job.save(dir)?; return Ok(None); }
    let sheets: Vec<_> = requests.iter().map(|(i, _)| *i).collect();
    for &i in &sheets { job.sheets[i].taken = true; }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let reference = format!("Tilepicky-{}-{stamp}", std::process::id());
    job.groups.push(Group { sheets, remote: Remote::Submitting,
        recovery: Some(Recovery { reference: reference.clone(), ..Recovery::default() }), tracking: Tracking::default() });
    job.save(dir)?;
    crate::ai_log::event("batch_submit", json!({"provider":job.provider.name, "model":job.model, "tags_requested":job.tag_list,
        "sheets":requests.iter().map(|(i, _)| json!({"id":key(*i), "sheet":job.sheets[*i].rel})).collect::<Vec<_>>()}));
    let path = format!("models/{}:batchGenerateContent", job.model);
    let requests: Vec<_> = requests.into_iter().map(|(i, body)| (key(i), body)).collect();
    Ok(Some((reference.clone(), path, submit_body(&requests, &reference))))
}

/// Reads the provider's answer to a prepared submission into its group.
/// A submission the provider did not make sends its sheets again; one it
/// rejected fails those sheets.
fn finish_submission(job: &mut Job, dir: &Path, reference: &str, reply: Result<Value, Failure>) -> Result<(), String> {
    let Some(i) = job.groups.iter().position(|g| g.remote == Remote::Submitting
        && g.recovery.as_ref().is_some_and(|r| r.reference == reference)) else { return Ok(()); };
    let result = match reply {
        Ok(response) => remote_id(&response).map(|id| {
            job.groups[i].tracking.id = id.clone(); job.groups[i].remote = Remote::Waiting(id);
        }),
        // The provider made no batch: the sheets wait for the next try.
        Err(failure @ (Failure::NotSent(_) | Failure::Status(401 | 403 | 429 | 503, _))) => {
            job.send_again(i);
            Err(failure.message())
        }
        Err(failure @ Failure::Status(400..500, _)) => {
            for &s in &job.groups[i].sheets { job.sheets[s].error = failure.message(); }
            job.groups[i].remote = Remote::Done;
            Ok(())
        }
        Err(failure) => Err(failure.message()),
    };
    job.save(dir).and(result)
}

/// Submits the next group of sheets as one Gemini batch, and reads the
/// answer. The runner keeps several of these in flight instead; this path
/// serves one step and the tests.
fn submit(job: &mut Job, root: &Path, dir: &Path, send: Send) -> Result<(), String> {
    match prepare_submission(job, root, dir)? {
        Some((reference, path, body)) => {
            let reply = send(&path, Some(&body));
            finish_submission(job, dir, &reference, reply)
        }
        None => Ok(()),
    }
}

/// Recover by the unique reference saved before submission. A reply never
/// assumed: the lookup tells whether Google made the batch. When it did not,
/// the sheets go out again, as if the upload had never started.
fn recover(job: &mut Job, dir: &Path, send: Send) -> Result<(), String> {
    let i = job.groups.iter().position(|g| g.remote == Remote::Submitting && g.recovery.is_some())
        .ok_or("No submission has a recovery reference.")?;
    let Some(recovery) = &mut job.groups[i].recovery else {
        return Err("This older submission has no recovery reference. Open Advanced recovery for help.".into());
    };
    let token: String = recovery.page.bytes().map(|b| format!("%{b:02X}")).collect();
    let path = if token.is_empty() { "batches?pageSize=100".into() } else { format!("batches?pageSize=100&pageToken={token}") };
    let value = send(&path, None).map_err(|f| f.message())?;
    let empty = Vec::new();
    let operations = if value["operations"].is_null() { &empty }
        else { value["operations"].as_array().ok_or("Google returned an invalid batch list.")? };
    for operation in operations {
        if operation["metadata"]["displayName"] == recovery.reference
            && operation["metadata"]["model"] == format!("models/{}", job.model) {
            let id = remote_id(operation)?;
            if !recovery.matches.contains(&id) { recovery.matches.push(id); }
        }
    }
    recovery.page = value["nextPageToken"].as_str().unwrap_or_default().into();
    if !recovery.page.is_empty() { return job.save(dir); }
    let matches = std::mem::take(&mut recovery.matches);
    let reference = recovery.reference.clone();

    crate::ai_log::event("batch_recovery_check", json!({"reference":reference, "matches":matches.len()}));
    match matches.as_slice() {
        // Google never made this batch: its sheets go out again.
        [] => {
            crate::ai_log::event("batch_resubmit", json!({"reference":reference}));
            job.send_again(i);
            job.save(dir)
        }
        [id] => {
            crate::ai_log::event("batch_recovered", json!({"reference":reference, "id":id}));
            job.groups[i].tracking.id = id.clone();
            job.groups[i].remote = Remote::Waiting(id.clone());
            job.groups[i].tracking.recoveries += 1;
            job.groups.rotate_left(i + 1);
            job.save(dir)
        }
        _ => {
            job.groups[i].tracking.recoveries += 1;
            job.groups.rotate_left(i + 1);
            job.save(dir).and(Err("Google returned more than one matching batch. Open Advanced recovery; nothing was resent.".into()))
        }
    }
}

fn poll(job: &mut Job, dir: &Path, send: Send) -> Result<(), String> {
    let Some(i) = job.groups.iter().enumerate().filter(|(_, g)| matches!(g.remote, Remote::Waiting(_)))
        .min_by_key(|(_, g)| g.tracking.checked_ms).map(|(i, _)| i) else { return Ok(()) };
    let Remote::Waiting(id) = job.groups[i].remote.clone() else { unreachable!() };
    validate_id(&id)?;
    let response = send(&id, None).map_err(|f| f.message())?;
    job.groups[i].tracking.state = response["metadata"]["state"].as_str().unwrap_or_default().into();
    if job.groups[i].tracking.state.ends_with("CANCELLED") {
        for &s in &job.groups[i].sheets { job.sheets[s].cancelled = true; }
        job.groups[i].remote = Remote::Done;
        return job.save(dir);
    }
    match outputs(&response) {
        // Ask about the other groups first, next time.
        Ok(None) => { job.groups.rotate_left(i + 1); return job.save(dir); }
        Ok(Some(values)) => accept(job, i, values),
        Err(error) => for &s in &job.groups[i].sheets { job.sheets[s].error = error.clone(); },
    }
    job.groups[i].remote = Remote::Done;
    job.save(dir)
}

fn cancel_remote(job: &mut Job, dir: &Path, send: Send) -> Result<(), String> {
    let Some(i) = job.groups.iter().position(|g| matches!(g.remote, Remote::Waiting(_)) && !g.tracking.cancel_sent) else { return Ok(()) };
    let Remote::Waiting(id) = &job.groups[i].remote else { unreachable!() };
    validate_id(id)?;
    let result = send(&format!("{id}:cancel"), Some(&json!({}))).map_err(|f| f.message());
    if result.is_ok() { job.groups[i].tracking.cancel_sent = true; }
    job.groups.rotate_left(i + 1);
    job.poll_ms = 0;
    job.save(dir).and(result.map(|_| ()))
}

impl Job {
    /// Forgets an unconfirmed submission. Its sheets go out again.
    fn send_again(&mut self, group: usize) {
        if self.groups[group].remote != Remote::Submitting { return; }
        for &i in &self.groups[group].sheets { self.sheets[i].taken = false; }
        self.groups.remove(group);
    }

    /// Lets the sheets of an unfinished group go out with the ordinary
    /// requests. An OpenAI-style job of 0.2 went through OpenRouter's batch
    /// API, which read no local image: its groups never finish, and nothing
    /// asks about them any more.
    fn release_groups(&mut self) {
        for group in self.groups.iter().filter(|g| g.remote != Remote::Done) {
            for &i in &group.sheets {
                let sheet = &mut self.sheets[i];
                (sheet.taken, sheet.error) = (false, String::new());
            }
        }
        self.groups.retain(|g| g.remote == Remote::Done);
    }
}

#[derive(Clone, Copy)]
enum Activity { Preparing, Uploading(usize), Checking, Recovering, Labeling, Cancelling }

impl Activity {
    fn before(operation: Operation) -> Self {
        match operation {
            Operation::Label => Self::Labeling, Operation::Submit => Self::Preparing,
            Operation::Poll => Self::Checking, Operation::Recover => Self::Recovering, Operation::Cancel => Self::Cancelling,
        }
    }

    fn request(path: &str, body: Option<&Value>) -> Self {
        if path.ends_with(":cancel") { Self::Cancelling }
        else if path.ends_with(":batchGenerateContent") {
            let count = body.and_then(|b| b.pointer("/batch/inputConfig/requests/requests"))
                .and_then(Value::as_array).map_or(0, Vec::len);
            Self::Uploading(count)
        } else if path == "chat/completions" { Self::Labeling }
        else if path.starts_with("batches?") { Self::Recovering }
        else { Self::Checking }
    }

    fn description(self, provider: &str, seconds: u64) -> String {
        let action = match self {
            Self::Preparing => "Preparing images for the next upload".into(),
            Self::Uploading(count) => format!("Uploading {count} {} to {provider}", if count == 1 { "sheet" } else { "sheets" }),
            Self::Checking => format!("Checking results from {provider}"),
            Self::Recovering => format!("Looking up the interrupted submission at {provider}"),
            Self::Labeling => format!("Labeling one sheet with {provider}"),
            Self::Cancelling => format!("Requesting cancellation from {provider}"),
        };
        format!("{action} ({seconds} s)")
    }


}

/// The pane displays coordinator snapshots. It never changes a running job's journal.
#[derive(Default)]
pub struct Panel {
    log: Option<crate::ai_log::Log>,
    pub root: PathBuf,
    dir: PathBuf,
    pub job: Option<Job>,
    proposal: Option<Job>,
    cost_preview: Option<cost::Preview>,
    cost_error: String,
    runner: Option<runner::Runner>,
    lock: Option<std::sync::Arc<std::fs::File>>,
    pending_save: Option<mpsc::Sender<runner::SaveReport>>,
    activity: Option<(Activity, Instant)>,
    error: String,
    storage_error: String,
    key: String,
    control: runner::Control,
    resend_confirm: bool,
    copied: Option<Instant>,
    pub settings_requested: bool,
    /// The files chosen in the library tree for the next job, by their
    /// paths below the root, and a short name for them. `None` is the
    /// whole library.
    pub choice: Option<(BTreeSet<String>, String)>,
}

impl Panel {
    fn running(&self) -> bool { self.job.as_ref().is_some_and(|j| !j.done()) }
    pub fn open(&self) -> bool { self.proposal.is_some() || self.resend_confirm }
    pub fn busy(&self) -> bool { self.running() || self.open() }
    pub fn single_block_reason(&self) -> Option<&'static str> {
        if self.busy() { Some("Finish or cancel the library job before labeling a single sheet.") }
        else if !self.root.as_os_str().is_empty() && self.lock.is_none() { Some("Another window controls labeling for this library.") }
        else { None }
    }
    pub fn allows_single(&self) -> bool { self.single_block_reason().is_none() }
    /// Narrows the next job to these sheets, by their paths below the root.
    pub fn choose(&mut self, rels: BTreeSet<String>, name: String) { self.choice = Some((rels, name)); }
    pub fn cleanup_log(&self, root: &Path) -> Result<(), String> {
        let _lock = if self.root == root && self.lock.is_some() { None } else {
            match runner::lock(root) { Ok(lock) => Some(lock), Err(_) => return Ok(()) }
        };
        crate::ai_log::cleanup_finished(root).map_err(|error| error.to_string())
    }
    pub fn finish_before_single(&mut self) {
        if !self.busy() {
            self.stop_runner();
            if let Some(log) = &self.log { log.retire(); }
        }
    }

    fn submission_counts(&self, job: &Job) -> (usize, usize, usize) {
        let (queued, waiting, uncertain) = job.submission_counts();
        if let Some((Activity::Uploading(count), _)) = self.activity {
            (queued.saturating_sub(count), waiting, uncertain + count)
        } else { (queued, waiting, uncertain) }
    }

    fn problems(&self) -> Vec<(String, usize)> {
        let mut messages = Vec::<(String, usize)>::new();
        let mut add = |text: &str, count: usize| {
            if text.is_empty() { return; }
            if let Some((_, n)) = messages.iter_mut().find(|(message, _)| message == text) { *n += count; }
            else { messages.push((text.into(), count)); }
        };
        add(&self.storage_error, 0); add(&self.error, 0);
        if let Some(job) = &self.job {
            if let Some(issue) = job.issues.get("save") { add(&issue.message, 0); }
            for (operation, issue) in &job.issues { if operation != "save" { add(&issue.message, 0); } }
            for sheet in &job.sheets {
                if sheet.label.is_none() && !sheet.cancelled { add(&sheet.error, 1); }
            }
        }
        messages
    }

    pub fn attention(&self) -> Option<String> {
        self.problems().first().map(|(message, _)| {
            let provider = self.job.as_ref().map_or("AI", |job| job.provider.name.as_str());
            format!("{provider}: {}", labels::problem(message).title)
        })
    }

    pub fn status(&self) -> Option<String> {
        let job = self.job.as_ref()?;
        if job.done() && self.problems().is_empty() { return None; }
        Some(format!("AI labels: {} saved / {} | {}", job.saved(), job.sheets.len(), self.state(job)))
    }

    fn log_available(&self) -> bool { self.log.as_ref().is_some_and(crate::ai_log::Log::available) }

    pub fn discard_completed(&mut self, root: &Path) -> Result<(), String> {
        if self.busy() { return Err("Wait for the job to finish or cancel it first.".into()); }
        if self.root != root { return Err("This job belongs to another library.".into()); }
        if self.lock.is_none() { return Err("This window does not own the library job.".into()); }
        self.stop_runner();
        store::clear(&self.dir)?;
        self.job = None;
        self.log = None;
        self.error.clear();
        Ok(())
    }

    fn stop_runner(&mut self) { if let Some(runner) = self.runner.take() { runner.finish(); } }
    fn command(&self, command: runner::Command) { if let Some(runner) = &self.runner { runner.command(command); } }

    fn set_mode(&mut self, mode: Mode) {
        if self.lock.is_none() { self.error = "This window does not own the library job.".into(); return; }
        let revision = now_ms().max(self.control.revision + 1);
        let control = runner::Control { revision, mode };
        match store::set_control(&self.dir, &control) {
            Ok(()) => {
                if let Some(log) = &self.log { log.event("batch_control", json!({"mode":mode})); }
                self.control = control; self.command(runner::Command::Wake);
            }
            Err(error) => self.error = error,
        }
    }

    pub fn tick(&mut self, ctx: &eframe::egui::Context, root: &Path, keys: &crate::ai::Keys, single_running: bool) -> bool {
        if !single_running && !self.running() && self.root != root && !root.as_os_str().is_empty() {
            self.stop_runner();
            *self = Panel { root: root.into(), ..Panel::default() };
            let opened = (|| {
                let lock = runner::lock(root)?;
                store::migrate(root, &legacy_directory(root)?)?;
                Ok::<_, String>((lock, root.to_path_buf()))
            })();
            match opened {
                Ok((lock, dir)) => {
                    self.lock = Some(lock); self.dir = dir;
                    match Job::load(&self.dir) {
                        Ok(job) => {
                            self.log = job.as_ref().map(|job| crate::ai_log::Log::batch(&self.root, &job.log_id));
                            if job.as_ref().is_some_and(Job::done) && let Some(log) = &self.log && log.available() { log.complete(); }
                            self.job = job;
                        }
                        Err(error) => self.error = error,
                    }
                    match store::control(&self.dir) {
                        Ok(control) => self.control = control, Err(error) => self.error = error,
                    }
                }
                Err(error) => self.error = error,
            }
        }
        if let Some(job) = &self.job {
            let key = job.provider.key(keys).unwrap_or_default();
            let secret = job.provider.store_secret(keys);
            if !single_running && self.runner.is_none() && self.storage_error.is_empty() && !job.done() && let Some(lock) = &self.lock {
                self.key = key.clone();
                self.runner = Some(runner::Runner::start(job.clone(), self.root.clone(), key, secret, lock.clone(), ctx.clone(), self.log.clone().unwrap()));
            } else if key != self.key {
                self.key = key.clone(); self.command(runner::Command::Key(key));
            }
        }
        let mut save = false;
        let mut stopped = false;
        if let Some(runner) = &self.runner {
            loop {
                let event = match runner.events.try_recv() {
                    Ok(event) => event,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => { stopped = true; break; }
                };
                match event {
                    runner::Event::Snapshot(job) => { self.job = Some(job); self.storage_error.clear(); }
                    runner::Event::Activity(activity) => self.activity = Some((activity, Instant::now())),
                    runner::Event::Idle => self.activity = None,
                    runner::Event::Import(job, reply) => {
                        self.job = Some(job); self.pending_save = Some(reply); self.activity = None; save = true;
                    }
                    runner::Event::Error(error) => self.storage_error = error,
                }
            }
        }
        if stopped {
            self.stop_runner(); self.activity = None;
            if self.storage_error.is_empty() { self.storage_error = "The job worker stopped. Reload the saved job to continue.".into(); }
        }
        if self.running() { ctx.request_repaint_after(Duration::from_secs(1)); }
        save
    }

    pub fn import(&mut self) -> Vec<(String, Option<Label>)> {
        let Some(reply) = self.pending_save.take() else { return vec![] };
        let Some(job) = &self.job else { return vec![] };
        let _scope = self.log.as_ref().map(crate::ai_log::Log::enter);
        let (report, labels) = save_labels(job, &self.root);
        let _ = reply.send(report);
        labels
    }

    fn cancel(&mut self, _keys: &crate::ai::Keys) { self.set_mode(Mode::Cancelling); }

    fn mode(&self, job: &Job) -> Mode {
        if self.control.revision > job.control_revision { self.control.mode } else { job.mode }
    }
    fn state(&self, job: &Job) -> String {
        if !self.storage_error.is_empty() { return "Job needs attention".into(); }
        if job.done() {
            if job.mode == Mode::Cancelling { return "Cancelled; saved labels kept".into(); }
            let failed = job.sheets.iter().filter(|s| !s.error.is_empty() && s.label.is_none()).count();
            return if failed == 0 { "Finished".into() }
                else if job.saved() == 0 { format!("Labeling failed; {failed} sheets failed") }
                else { format!("Finished with errors; {failed} sheets failed") };
        }
        if self.mode(job) == Mode::Cancelling { return "Cancellation pending".into(); }
        if job.pending_save() {
            return if job.issues.contains_key("save") { "Could not save labels; retry pending".into() } else { "Saving received labels".into() };
        }
        if self.mode(job) == Mode::Paused {
            return if job.provider.kind == Kind::Gemini || job.provider.store.is_some() { "Uploads paused; submitted work can continue".into() } else { "Labeling paused".into() };
        }
        if job.issues.contains_key("upload")
            || (job.provider.kind == Kind::OpenAi && job.sheets.iter().any(|s| !s.taken && s.retry_ms > now_ms())) {
            return "Uploads interrupted; retry pending".into();
        }
        if job.issues.contains_key("check") { return format!("Waiting for {}; connection interrupted", job.provider.name); }
        if job.untaken() && job.groups.iter().filter(|g| g.remote == Remote::Submitting).count() >= job.concurrency.max(1) as usize {
            return "Waiting for upload confirmations".into();
        }
        if job.untaken() {
            return if job.provider.kind == Kind::Gemini || job.provider.store.is_some() { "Uploading remaining sheets".into() } else { "Labeling sheets".into() };
        }
        if job.groups.iter().any(|g| matches!(g.remote, Remote::Waiting(_))) {
            let states: Vec<_> = job.groups.iter().filter(|g| matches!(g.remote, Remote::Waiting(_)))
                .map(|g| g.tracking.state.as_str()).collect();
            let action = if states.iter().all(|s| *s == "BATCH_STATE_PENDING" || *s == "validating") { "Queued at" }
                else if states.iter().all(|s| *s == "BATCH_STATE_RUNNING" || *s == "in_progress") { "Processing at" }
                else { "Waiting for results from" };
            return format!("{action} {}", job.provider.name);
        }
        if job.uncertain() { return "Some sheets need confirmation".into(); }
        "Waiting for the next operation".into()
    }

    pub fn ui(&mut self, ui: &mut eframe::egui::Ui, index: &Index, ai: &crate::ai::Ai, keys: &crate::ai::Keys, single_running: bool) {
        use eframe::egui;
        let configured = ai.chosen(crate::ai::Mode::Batch);
        let ready = configured.is_some_and(|(p, _)| endpoint(p).is_ok() && p.key_source(keys) != crate::ai::KeySource::None);
        let problems = self.problems();
        if self.job.is_none() {
            for (message, _) in &problems { self.settings_requested |= labels::problem(message).show(ui, message, false); }
        }
        if (self.lock.is_none() || (!self.storage_error.is_empty() && self.runner.is_none()))
            && crate::stopped(ui.button("Reload saved job")).clicked() {
            self.stop_runner(); self.job = None; self.root = PathBuf::new();
        }
        let (mut pause, mut cancel, mut retry, mut retry_failed, mut copy) = (None, false, false, false, false);
        let now = now_ms();
        if let Some(job) = &self.job {
            if self.root != index.root { ui.label(format!("Active job for {}", self.root.display())); }
            ui.strong("Saved library job");
            ui.label(format!("{} / {}", job.provider.name, job.model));
            ui.small(crate::ai::method(&job.provider));
            if let Some((provider, model)) = configured
                && (provider.name != job.provider.name || provider.kind != job.provider.kind || provider.url != job.provider.url
                    || model.id.trim_end_matches(":batch") != job.model.trim_end_matches(":batch")) {
                ui.group(|ui| {
                    ui.label(format!("Settings select {} / {} for new jobs.", provider.name, model.id.trim_end_matches(":batch")));
                    ui.label("This saved job keeps its original model, including resume and retry.");
                    ui.label(if job.done() {
                        "Start a new library job below to use the selected model."
                    } else {
                        "Cancel this job or let it finish. Then start a new library job to use the selected model."
                    });
                    ui.small("Saved labels are kept. Use Label unlabeled sheets to continue without replacing them.");
                });
            }
            ui.add_space(6.0);
            ui.strong(self.state(job));
            for (message, count) in problems.iter().take(2) {
                ui.group(|ui| {
                    self.settings_requested |= labels::problem(message).show(ui, message,
                        job.provider.url.starts_with("https://generativelanguage.googleapis.com/"));
                    if *count > 0 { ui.small(format!("Affected sheets: {count}. Saved labels are kept.")); }
                });
            }
            if problems.len() > 2 { ui.label(format!("{} other errors. Open Error details below.", problems.len() - 2)); }
            let [saved, failed, unusable, cancelled] = job.outcomes();
            let finished = saved + failed + unusable + cancelled;
            let total = job.sheets.len();
            ui.add(egui::ProgressBar::new(if total == 0 { 1.0 } else { finished as f32 / total as f32 })
                .fill(if failed > 0 { ui.visuals().warn_fg_color } else { ui.visuals().selection.bg_fill })
                .text(format!("{finished} / {total} finished")));
            ui.label(format!("{saved} saved, {failed} failed, {unusable} unlabelable, {cancelled} cancelled"));
            let mode = self.mode(job);
            ui.horizontal_wrapped(|ui| {
                if !job.done() && mode != Mode::Cancelling {
                    let text = if mode == Mode::Paused { "Resume" }
                        else if job.provider.kind == Kind::Gemini || job.provider.store.is_some() { "Pause uploads" } else { "Pause" };
                    if crate::stopped(ui.button(text)).clicked() { pause = Some(if mode == Mode::Paused { Mode::Running } else { Mode::Paused }); }
                    if crate::stopped(ui.button("Cancel job")).clicked() { cancel = true; }
                }
                if !job.issues.is_empty() && crate::stopped(ui.button("Retry now")).clicked() { retry = true; }
                if job.done() && failed > 0
                    && crate::stopped(ui.add_enabled(!single_running, egui::Button::new("Retry failed sheets"))).clicked() { retry_failed = true; }
                copy = crate::stopped(ui.add_enabled(self.log_available(), egui::Button::new("Copy log")))
                    .on_hover_text("This library job's log. A new job replaces it; exit removes it after completion.").clicked();
                if self.copied.is_some_and(|at| at.elapsed().as_secs() < 3) {
                    ui.label("Copied"); ui.ctx().request_repaint_after(Duration::from_secs(1));
                }
            });
            if let Some((activity, started)) = self.activity {
                ui.horizontal_wrapped(|ui| { ui.spinner(); ui.label(activity.description(&job.provider.name, started.elapsed().as_secs())); });
            }
            let (queued, waiting, uncertain) = self.submission_counts(job);
            ui.label(format!("{queued} not sent, {waiting} at {}, {uncertain} unconfirmed", job.provider.name));
            if job.last_response_ms > 0 { ui.small(format!("Last provider response: {} s ago", now.saturating_sub(job.last_response_ms) / 1000)); }
            if waiting > 0 && job.poll_ms > now { ui.small(format!("Next results check in {} s", (job.poll_ms - now).div_ceil(1000))); }
            if mode != Mode::Paused && let Some(issue) = job.issues.values().min_by_key(|issue| issue.retry_ms) {
                ui.label(format!("Automatic retry in {} s.", issue.retry_ms.saturating_sub(now).div_ceil(1000)));
            }
            egui::ScrollArea::vertical().id_salt("library job details").max_height((ui.available_height() - 180.0).max(60.0)).show(ui, |ui| {
                if let Some(estimate) = &job.estimate {
                    egui::CollapsingHeader::new("Job cost estimate").show(ui, |ui| {
                        estimate.ui(ui);
                        if job.usage.requests > 0 {
                            if let Some(text) = estimate.usage_summary(&job.usage) { ui.label(text); }
                            ui.small(format!("{} reported requests: {} input, {} output tokens including reasoning.",
                                job.usage.requests, job.usage.input, job.usage.output));
                            ui.small("Reported usage can include retries. Missing usage and unconfirmed requests are not included.");
                        }
                    });
                }
                if !job.done() {
                    ui.small(if job.provider.kind == Kind::Gemini {
                        "Closing pauses local work. Google can continue accepted work. Reopen this library to collect results."
                    } else { "Closing pauses labeling. Reopen this library to continue." });
                }
                egui::CollapsingHeader::new(if problems.is_empty() { "Details" } else { "Error details" })
                    .id_salt("library full details").show(ui, |ui| {
                    if job.uncertain() {
                        ui.label("Tilepicky checks interrupted uploads automatically. It will not send these sheets twice automatically.");
                        if mode != Mode::Cancelling && crate::stopped(ui.button("Retry unconfirmed sheets...")).clicked() { self.resend_confirm = true; }
                    }
                    for group in job.groups.iter().filter(|g| !g.tracking.error.is_empty() && !problems.iter().any(|(text, _)| text == &g.tracking.error)) {
                        ui.colored_label(ui.visuals().error_fg_color, format!("{} sheets: {}", group.sheets.len(), group.tracking.error));
                    }
                    for (message, count) in &problems {
                        ui.label(message);
                        if *count > 0 {
                            egui::CollapsingHeader::new(format!("Affected sheets ({count})")).id_salt(message).show(ui, |ui| {
                                for sheet in job.sheets.iter().filter(|s| s.error == *message) { ui.label(&sheet.rel); }
                            });
                        }
                    }
                    ui.label(format!("Tags used for this job: {}", job.tag_list.join(", ")));
                    egui::CollapsingHeader::new("Provider batches").show(ui, |ui| {
                        for group in &job.groups {
                            if !group.tracking.id.is_empty() {
                                ui.label(format!("{} sheets: {}", group.sheets.len(), group.tracking.state)); ui.small(&group.tracking.id);
                            }
                        }
                    });
                });
            });
        }
        if copy && let Some(log) = &self.log {
            match log.text() {
                Ok(text) => { ui.ctx().copy_text(text); self.copied = Some(Instant::now()); }
                Err(error) => self.error = error,
            }
        }
        if let Some(mode) = pause { self.set_mode(mode); }
        if cancel { self.cancel(keys); }
        if retry { self.command(runner::Command::Wake); }
        if retry_failed && !single_running { self.start_retry(runner::Command::RetryFailed, ui.ctx(), keys); }
        if self.busy() { return; }
        let idle = self.lock.is_some() && !single_running && index.error.is_none() && !index.root.as_os_str().is_empty();
        ui.add_space(8.0);
        ui.strong("New library job");
        let mut clear_choice = false;
        // The counts come from the chosen sheets, when there are any. The set
        // itself is cloned only when a job starts, not on every frame.
        let target = self.choice.as_ref().map(|(_, name)| name.clone());
        let labeled = |e: &&crate::index::Entry| e.side.label.as_ref().is_some_and(|l| l.status == Status::Labeled);
        let (total, done, any_unlabeled) = match self.choice.as_ref() {
            Some((rels, _)) => {
                let in_scope = || index.entries.iter().filter(|e| rels.contains(&e.rel));
                (in_scope().count(), in_scope().filter(labeled).count(), in_scope().any(|e| e.side.label.is_none()))
            }
            None => (
                index.entries.len(),
                index.entries.iter().filter(labeled).count(),
                index.entries.iter().any(|e| e.side.label.is_none()),
            ),
        };
        if let Some(name) = &target {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Target: {name}"));
                if ui.button("Whole library").on_hover_text("Choose sheets in the library tree to narrow a job.").clicked() { clear_choice = true; }
            });
        }
        if let Some((provider, model)) = configured {
            ui.label(format!("{} / {}", provider.name, model.id.trim_end_matches(":batch")));
            ui.small(crate::ai::method(provider));
        }
        ui.label(format!("{done} of {total} sheets have usable labels"));
        if !ready { ui.label("Set a library model and its key in Settings (Ctrl+,)."); }
        for (text, scope) in [
            (if target.is_some() { "Label unlabeled in selection..." } else { "Label unlabeled sheets..." }, Scope::Unlabeled),
            (if target.is_some() { "Rerun all in selection..." } else { "Rerun all..." }, Scope::All),
        ] {
            let enabled = ready && idle && (scope == Scope::All || any_unlabeled);
            let button = ui.add_enabled(enabled, egui::Button::new(text).selected(scope == Scope::Unlabeled));
            crate::stop(&button);
            if button.clicked() {
                let (provider, model) = configured.unwrap();
                let (provider, model, concurrency) = (provider.clone(), model.id.clone(), model.concurrency.max(1));
                let set = self.choice.as_ref().map(|(rels, _)| rels.clone());
                let job = match set.as_ref() {
                    Some(set) => prepare_of(index, provider, model, scope, Some(set)),
                    None => prepare(index, provider, model, scope),
                };
                match job {
                    Ok(mut job) => {
                        job.concurrency = concurrency;
                        self.cost_error.clear();
                        self.cost_preview = match cost::Preview::start(job.clone(), index.root.clone(), self.job.as_ref(), ui.ctx().clone()) {
                            Ok(preview) => Some(preview), Err(error) => { self.cost_error = error; None }
                        };
                        self.proposal = Some(job);
                    }
                    Err(e) => self.error = e,
                }
            }
        }
        if clear_choice { self.choice = None; }
        if self.job.is_none() { ui.add_enabled(false, egui::Button::new("Copy log")); }
    }

    fn start_retry(&mut self, command: runner::Command, ctx: &eframe::egui::Context, keys: &crate::ai::Keys) {
        if self.runner.is_none() && let (Some(job), Some(lock)) = (&self.job, &self.lock) {
            self.runner = Some(runner::Runner::start(job.clone(), self.root.clone(),
                job.provider.key(keys).unwrap_or_default(), job.provider.store_secret(keys), lock.clone(), ctx.clone(), self.log.clone().unwrap()));
        }
        self.command(command);
    }

    /// The dialog that asks before a batch starts.
    pub fn confirmation(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui;
        if self.resend_confirm {
            egui::Modal::new(egui::Id::new("retry unconfirmed")).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.heading("Retry unconfirmed sheets?");
                ui.label("Google may already have accepted these sheets. Sending them again can charge you twice.");
                ui.label("Only the unconfirmed sheets will be retried. Accepted batches will keep their IDs.");
                crate::dialogs::footer(ui, |ui| {
                    if ui.button("Send these sheets again").clicked() {
                        self.command(runner::Command::RetryUnconfirmed); self.resend_confirm = false;
                    }
                    if ui.button("Keep waiting").clicked() { self.resend_confirm = false; }
                });
            });
            return;
        }
        if let Some(preview) = &self.cost_preview {
            match preview.receiver.try_recv() {
                Ok(estimate) => {
                    if let Some(job) = &mut self.proposal { job.estimate = Some(estimate); }
                    self.cost_preview = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.cost_error = "Could not calculate the estimate. You can still start the job.".into();
                    self.cost_preview = None;
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            }
        }
        let Some(job) = &self.proposal else { return };
        let (mut start, mut close) = (false, false);
        egui::Modal::new(egui::Id::new("library batch confirmation")).show(ctx, |ui| {
            ui.set_width(500.0_f32.min(ctx.content_rect().width() - 48.0));
            ui.set_max_height((ctx.content_rect().height() - 96.0).max(160.0));
            ui.heading(if self.choice.is_some() { "Label the chosen sheets?" } else { "Label this entire library?" });
            let height = (ctx.content_rect().height() - ui.min_rect().height() - 96.0 - crate::dialogs::footer_height(ui)).max(80.0);
            egui::ScrollArea::vertical().max_height(height).show(ui, |ui| {
                ui.label(self.root.display().to_string());
                if let Some((_, name)) = &self.choice { ui.weak(format!("Target: {name}")); }
                ui.weak(format!("{} / {} (from Settings)", job.provider.name, job.model));
                if job.provider.kind == Kind::Gemini { ui.weak("Google batch requests require a paid API project."); }
                if job.sheets.is_empty() {
                    ui.label("Every sheet has a label already.");
                } else {
                    ui.label(format!("Sheets to label: {}, one request each.", job.sheets.len()));
                    if job.replacing > 0 {
                        ui.label(format!("Existing labels to replace: {}.", job.replacing));
                        ui.label("Existing labels stay until new results arrive. Failed requests keep the old labels.");
                    } else {
                        ui.label(format!("Skipped because they have a label: {}.", job.skipped));
                    }
                    match job.tag_list.is_empty() {
                        true => ui.label("Tags to look for: none."),
                        false => ui.label(format!("Tags to look for: {}.", job.tag_list.join(", "))),
                    };
                    if let Some(estimate) = &job.estimate { estimate.ui(ui); }
                    else if self.cost_preview.is_some() {
                        ui.horizontal(|ui| { ui.spinner(); ui.label("Estimating cost..."); });
                        ui.small("Reading local image sizes and model prices. No sheets are uploaded.");
                    } else { ui.label(&self.cost_error); }
                    ui.weak(if job.provider.kind == Kind::Gemini {
                        "Closing pauses uploads and saves. Google continues accepted work. Reopen this library to collect results."
                    } else { "This provider labels sheets one by one. Closing pauses the job until you reopen this library." });
                }
            });
            crate::dialogs::footer(ui, |ui| {
                start = ui.add_enabled(!job.sheets.is_empty() && self.cost_preview.is_none(), egui::Button::new("Start labeling")).clicked();
                close = ui.button(if job.sheets.is_empty() { "Close" } else { "Cancel" }).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if start {
            self.stop_runner();
            let mut job = self.proposal.take().unwrap();
            job.control_revision = self.control.revision;
            match store::start(&self.dir, &job) {
                Ok(()) => {
                    let log = crate::ai_log::Log::batch(&self.root, &job.log_id);
                    log.event("batch_start", json!({"provider":job.provider.name,"model":job.model,
                        "sheets":job.sheets.len(),"tags_requested":job.tag_list,"prompt":job.prompt,"estimate":job.estimate}));
                    self.log = Some(log); self.job = Some(job); self.error.clear();
                }
                Err(e) => self.error = e,
            }
        }
        if close { self.proposal = None; self.cost_preview = None; self.cost_error.clear(); }
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
            let root = base.join("library");
            let spool = root.clone();
            std::fs::create_dir_all(root.join("folder")).unwrap();
            RgbaImage::from_pixel(8, 4, Rgba([80, 90, 100, 255])).save(root.join("folder/sheet.png")).unwrap();
            Self { base, root, spool }
        }
        fn prepare(&self, kind: Kind) -> Job {
            prepare(&Index::scan(&self.root, [16, 16]), provider(kind), "test:batch".into(), Scope::Unlabeled).unwrap()
        }
        fn advance(&self, job: Job, send: impl FnMut(&str, Option<&Value>) -> Result<Value, Failure>) -> (Job, Result<(), String>) {
            advance(job, &self.root, &self.spool, send)
        }
        fn journal(&self) -> Job { Job::load(&self.spool).unwrap().unwrap() }
    }
    impl Drop for Files { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.base); } }
    pub(super) fn provider(kind: Kind) -> Provider {
        Provider { name: "test".into(), kind, skip: None, store: None, key_env: vec![], url: match kind {
            Kind::Gemini => "https://generativelanguage.googleapis.com/v1beta".into(),
            Kind::OpenAi => "https://openrouter.ai/api/v1".into(),
        } }
    }
    /// Gemini's answer for a finished group, in reverse order.
    fn completed(job: &Job, group: usize, caption: &str) -> Value {
        let text = completion(labeled(caption))["choices"][0]["message"]["content"].clone();
        json!({"done":true, "response":{"inlinedResponses":{"inlinedResponses":job.groups[group].sheets.iter().rev().map(|&i| {
            json!({"metadata":{"key":key(i)}, "response":{"candidates":[{"finishReason":"STOP", "content":{"parts":[{"text":text}]}}]}})
        }).collect::<Vec<_>>()}}})
    }
    fn id(value: &str) -> impl FnMut(&str, Option<&Value>) -> Result<Value, Failure> {
        let value = value.to_string();
        move |_, _| Ok(json!({"name":value}))
    }

    /// Submits one real OpenRouter batch with a base64 image, to settle whether
    /// the batch API accepts base64 as the docs say it does not. Needs
    /// OPENROUTER_API_KEY and a few cents. Run with:
    ///   cargo test -- --ignored a_real_openrouter_batch --nocapture
    #[test]
    #[ignore = "Hits the real OpenRouter batch API; needs OPENROUTER_API_KEY and a few cents."]
    fn a_real_openrouter_batch_reports_whether_base64_is_accepted() {
        let Ok(key) = std::env::var("OPENROUTER_API_KEY") else { eprintln!("OPENROUTER_API_KEY is unset; skipping"); return };
        let files = Files::new();
        let job = prepare(&Index::scan(&files.root, [16, 16]), provider(Kind::OpenAi), "openai/gpt-5-nano".into(), Scope::Unlabeled).unwrap();
        let transport = Transport::new(&job.provider, key).unwrap();
        let mut send = |path: &str, body: Option<&Value>| transport.send(path, body);
        let body = openai_batch_body(&job, &files.root, 1, None).unwrap();
        let image = body["requests"][0]["body"]["messages"][1]["content"][1]["image_url"]["url"].as_str().unwrap();
        eprintln!("base64 image part starts: {}", &image[..image.len().min(48)]);
        let id = openai_batch_post(&body, &mut send).unwrap();
        eprintln!("submitted batch {id}");
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let response = send(&format!("batches/{id}"), None).map_err(|f| f.message()).unwrap();
            eprintln!("status {} counts {}", response["status"], response["request_counts"]);
            match openai_batch_outputs(&response) {
                Ok(None) => continue,
                Ok(Some(values)) => { for (k, v) in values { eprintln!("result {k}: {v}"); } return; }
                Err(error) => { eprintln!("terminal: {error}"); return; }
            }
        }
        eprintln!("still running after five minutes: {id}");
    }

    /// Submits one real OpenRouter batch with a public image URL to a Google
    /// model, to test whether the docs are right that Google's batch cannot
    /// fetch an image URL. Needs OPENROUTER_API_KEY and a few cents.
    #[test]
    #[ignore = "Hits the real OpenRouter batch API; needs OPENROUTER_API_KEY and a few cents."]
    fn a_real_openrouter_batch_url_to_google() {
        let Ok(key) = std::env::var("OPENROUTER_API_KEY") else { eprintln!("OPENROUTER_API_KEY is unset; skipping"); return };
        let files = Files::new();
        let job = prepare(&Index::scan(&files.root, [16, 16]), provider(Kind::OpenAi), "google/gemini-3.8-flash".into(), Scope::Unlabeled).unwrap();
        let transport = Transport::new(&job.provider, key).unwrap();
        let mut send = |path: &str, body: Option<&Value>| transport.send(path, body);
        let url = "https://httpbin.org/image/png";
        let body = openai_batch_body(&job, &files.root, 1, Some(url)).unwrap();
        eprintln!("submitting {url} to {}", job.model);
        let id = openai_batch_post(&body, &mut send).unwrap();
        eprintln!("submitted batch {id}");
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let response = send(&format!("batches/{id}"), None).map_err(|f| f.message()).unwrap();
            eprintln!("status {} counts {}", response["status"], response["request_counts"]);
            match openai_batch_outputs(&response) {
                Ok(None) => continue,
                Ok(Some(values)) => { for (k, v) in values { eprintln!("result {k}: {v}"); } return; }
                Err(error) => { eprintln!("terminal: {error}"); return; }
            }
        }
        eprintln!("still running after five minutes: {id}");
    }

    #[test]
    fn a_scoped_job_keeps_only_the_chosen_sheets() {
        let files = Files::new();
        RgbaImage::from_pixel(8, 4, Rgba([1, 2, 3, 255])).save(files.root.join("top.png")).unwrap();
        let index = Index::scan(&files.root, [16, 16]);
        let only: BTreeSet<String> = ["folder/sheet.png".to_string()].into_iter().collect();
        let job = prepare_of(&index, provider(Kind::Gemini), "test:batch".into(), Scope::Unlabeled, Some(&only)).unwrap();
        assert_eq!(job.sheets.iter().map(|s| s.rel.as_str()).collect::<Vec<_>>(), ["folder/sheet.png"]);
        let whole = prepare(&index, provider(Kind::Gemini), "test:batch".into(), Scope::Unlabeled).unwrap();
        assert_eq!(whole.sheets.len(), 2);
    }

    #[test]
    fn new_jobs_send_file_context_and_old_jobs_keep_their_saved_prompt() {
        let files = Files::new();
        let mut job = files.prepare(Kind::OpenAi);
        let request = image_request(&mut job, &files.root, 0, None).unwrap();
        let expected = labels::sheet_prompt(job.prompt.clone().unwrap(), "folder/sheet.png");
        assert_eq!(request["messages"][1]["content"][0]["text"], expected[1]);
        job.provider = provider(Kind::Gemini);
        let google = image_request(&mut job, &files.root, 0, None).unwrap();
        assert_eq!(google, crate::gemini::request(&request));
        let mut saved = serde_json::to_value(&job).unwrap();
        saved.as_object_mut().unwrap().remove("file_context");
        saved["prompt"] = json!(["Original system", "Original user"]);
        let mut legacy: Job = serde_json::from_value(saved).unwrap();
        let google = image_request(&mut legacy, &files.root, 0, None).unwrap();
        assert_eq!(google["systemInstruction"]["parts"][0]["text"], "Original system");
        assert_eq!(google["contents"][0]["parts"][0]["text"], "Original user");
    }

    #[test]
    fn rework_provider_pending_is_not_processing() {
        let files = Files::new();
        let mut job = files.prepare(Kind::Gemini);
        for sheet in &mut job.sheets { sheet.taken = true; }
        job.groups.push(Group { sheets: vec![0], remote: Remote::Waiting("batches/test".into()), recovery: None,
            tracking: Tracking { state: "BATCH_STATE_PENDING".into(), ..Default::default() } });
        assert_eq!(Panel::default().state(&job), "Queued at test");
        job.groups[0].tracking.state = "BATCH_STATE_RUNNING".into();
        assert_eq!(Panel::default().state(&job), "Processing at test");
        job.groups[0].tracking.state.clear();
        assert_eq!(Panel::default().state(&job), "Waiting for results from test");
    }

    #[test]
    fn terminal_outcomes_do_not_count_a_sheet_twice() {
        let files = Files::new();
        let mut job = files.prepare(Kind::Gemini);
        let original = job.sheets[0].clone();
        job.sheets = vec![original; 4];
        let label = crate::sidecar::Label { status: Status::Labeled, caption: "Torch".into(), tags: vec![],
            provider: "test".into(), model: "test".into(), tag_list: None };
        job.sheets[0].label = Some(label.clone()); job.sheets[0].imported = true;
        job.sheets[1].error = "Failed".into();
        job.sheets[2].label = Some(crate::sidecar::Label { status: Status::Unlabelable, ..label }); job.sheets[2].imported = true;
        job.sheets[3].cancelled = true; job.sheets[3].error = "Cancelled after a failed attempt".into();
        assert_eq!(job.outcomes(), [1, 1, 1, 1]);
        assert_eq!(job.saved(), 1);
    }

    #[test]
    fn completion_waits_for_labels_to_be_saved() {
        let files = Files::new();
        let (job, _) = files.advance(files.prepare(Kind::Gemini), id("batches/test"));
        let reply = completed(&job, 0, "Crystals");
        let (job, result) = files.advance(job, |_, _| Ok(reply.clone()));
        result.unwrap();
        assert!(!job.done(), "Received results are not saved labels.");
    }

    #[test]
    fn cancellation_keeps_the_job_until_the_provider_confirms() {
        let files = Files::new();
        let (job, _) = files.advance(files.prepare(Kind::Gemini), id("batches/test"));
        let mut panel = Panel { root: files.root.clone(), dir: files.spool.clone(), lock: Some(runner::lock(&files.spool).unwrap()),
            job: Some(job), ..Panel::default() };
        panel.cancel(&crate::ai::Keys::default());
        assert!(panel.job.is_some(), "Cancellation must remain visible.");
        assert!(Job::load(&files.spool).unwrap().is_some(), "Cancellation must survive restart.");
    }

    #[test]
    fn provider_processing_remains_visible_during_a_result_check() {
        let files = Files::new();
        let (job, _) = files.advance(files.prepare(Kind::Gemini), id("batches/test"));
        let panel = Panel { activity: Some((Activity::Checking, Instant::now())), ..Panel::default() };
        assert_eq!(panel.state(&job), "Waiting for results from test");
    }

    #[test]
    fn upload_progress_uses_the_actual_group_size_and_elapsed_time() {
        let body = submit_body(&[("a".into(), json!({})), ("b".into(), json!({}))], "reference");
        let activity = Activity::request("models/test:batchGenerateContent", Some(&body));
        assert_eq!(activity.description("Google", 12), "Uploading 2 sheets to Google (12 s)");
        assert_eq!(Activity::request("batches/test", None).description("Google", 3), "Checking results from Google (3 s)");
    }

    #[test]
    fn uploading_sheets_are_not_also_shown_as_queued() {
        let files = Files::new();
        let job = files.prepare(Kind::Gemini);
        let panel = Panel { activity: Some((Activity::Uploading(1), Instant::now())), ..Panel::default() };
        assert_eq!(panel.submission_counts(&job), (0, 0, 1));
        assert_eq!(Activity::Uploading(1).description("Google", 2), "Uploading 1 sheet to Google (2 s)");
    }

    #[test]
    fn provider_processing_is_separate_from_a_network_request() {
        let files = Files::new();
        let mut job = files.prepare(Kind::Gemini);
        job.sheets[0].taken = true;
        job.groups.push(Group { sheets: vec![0], remote: Remote::Waiting("batches/test".into()), recovery: None, tracking: Tracking::default() });
        let panel = Panel::default();
        let state = panel.state(&job);
        assert_eq!(state, "Waiting for results from test");
        assert!(!state.contains("Contacting"));
    }

    #[test]
    fn another_window_cannot_start_a_single_request_or_remove_the_owners_log() {
        let files = Files::new();
        let log = crate::ai_log::Log::single_file(&files.root, "folder/sheet.png");
        log.event("finished", json!({})); log.complete();
        let owner = Panel { root: files.root.clone(), lock: Some(runner::lock(&files.root).unwrap()), ..Panel::default() };
        let other = Panel { root: files.root.clone(), ..Panel::default() };
        assert!(owner.allows_single()); assert!(!other.allows_single());
        other.cleanup_log(&files.root).unwrap(); assert!(log.available());
        owner.cleanup_log(&files.root).unwrap(); assert!(!log.available());
    }

    #[test]
    fn rejected_submissions_keep_the_reason_visible_after_restart() {
        let files = Files::new();
        std::fs::copy(files.root.join("folder/sheet.png"), files.root.join("second.png")).unwrap();
        let error = "The endpoint returned HTTP 402. RESOURCE_EXHAUSTED: Your prepayment credits are depleted.";
        let (job, _) = files.advance(files.prepare(Kind::Gemini), |_, _| Err(Failure::Status(402, error.into())));
        assert!(job.done());
        let panel = Panel { job: Some(files.journal()), ..Panel::default() };
        assert_eq!(panel.problems(), vec![(error.into(), 2)]);
        assert_eq!(panel.attention().as_deref(), Some("test: Prepaid credits depleted"));
        assert!(panel.status().unwrap().contains("Labeling failed"));
        assert_eq!(panel.job.as_ref().unwrap().outcomes(), [0, 2, 0, 0]);
    }

    #[test]
    fn partial_results_and_retry_errors_keep_their_own_state() {
        let files = Files::new();
        std::fs::copy(files.root.join("folder/sheet.png"), files.root.join("second.png")).unwrap();
        let mut job = files.prepare(Kind::Gemini);
        job.sheets[0].label = Some(labels::response(&completion(labeled("Saved")), &[]).unwrap().into_label("test", "test", &[]));
        job.sheets[0].imported = true;
        job.sheets[1].error = "Image could not be processed".into();
        job.sheets[0].taken = true; job.sheets[1].taken = true;
        job.groups.push(Group { sheets: vec![0, 1], remote: Remote::Done, recovery: None, tracking: Tracking::default() });
        let mut panel = Panel { job: Some(job), ..Panel::default() };
        assert!(panel.state(panel.job.as_ref().unwrap()).starts_with("Finished with errors"));
        assert_eq!(panel.problems(), vec![("Image could not be processed".into(), 1)]);
        assert_eq!(panel.job.as_ref().unwrap().outcomes(), [1, 1, 0, 0]);
        panel.job.as_mut().unwrap().sheets[1].cancelled = true;
        assert!(panel.attention().is_none());
        panel.job.as_mut().unwrap().issue("poll", "The endpoint returned HTTP 503.".into(), 100);
        assert!(panel.attention().unwrap().contains("temporarily unavailable"));
        panel.storage_error = "Could not write the job".into();
        assert!(panel.attention().unwrap().contains("Could not save"));
        panel.storage_error.clear(); panel.job.as_mut().unwrap().issues.clear();
        assert!(panel.attention().is_none());
    }

    #[test]
    fn outstanding_batches_have_status_without_the_ai_pane() {
        let files = Files::new();
        let panel = Panel { job: Some(files.prepare(Kind::Gemini)), ..Panel::default() };
        assert_eq!(panel.status().as_deref(), Some("AI labels: 0 saved / 1 | Uploading remaining sheets"));
    }

    #[test]
    fn a_stopped_worker_is_visible_and_cannot_restart_from_a_stale_snapshot() {
        let files = Files::new();
        let job = files.prepare(Kind::Gemini);
        let mut panel = Panel { root: files.root.clone(), dir: files.spool.clone(), job: Some(job),
            runner: Some(runner::Runner::disconnected()), activity: Some((Activity::Checking, Instant::now())), ..Panel::default() };
        panel.tick(&eframe::egui::Context::default(), &files.root, &crate::ai::Keys::default(), false);
        assert!(panel.runner.is_none()); assert!(panel.activity.is_none());
        assert!(panel.storage_error.contains("worker stopped"));
        assert_eq!(panel.state(panel.job.as_ref().unwrap()), "Job needs attention");
    }

    #[test]
    fn preparation_reads_no_image_and_skips_labeled_sheets() {
        let files = Files::new();
        std::fs::write(files.root.join("broken.png"), b"not an image").unwrap();
        RgbaImage::new(4, 4).save(files.root.join("done.png")).unwrap();
        let label = labels::tests::completion(labeled("Done"));
        let label = labels::response(&label, &[]).unwrap().into_label("test", "test", &[]);
        crate::sidecar::store_labels(&files.root, [("done.png", Some(label))]).unwrap();
        let job = files.prepare(Kind::Gemini);
        assert_eq!(job.sheets.iter().map(|s| s.rel.as_str()).collect::<Vec<_>>(), ["broken.png", "folder/sheet.png"]);
        assert_eq!(job.skipped, 1);
        assert_eq!(job.model, "test");
        assert!(Job::load(&files.root).unwrap().is_none());
    }

    #[test]
    fn rerun_keeps_old_labels_until_a_replacement_is_imported() {
        let files = Files::new();
        let old = labels::response(&completion(labeled("Old")), &[]).unwrap().into_label("test", "test", &[]);
        crate::sidecar::store_labels(&files.root, [("folder/sheet.png", Some(old.clone()))]).unwrap();
        let index = Index::scan(&files.root, [16, 16]);
        let job = prepare(&index, provider(Kind::OpenAi), "test:batch".into(), Scope::All).unwrap();
        assert_eq!((job.sheets.len(), job.skipped, job.replacing), (1, 0, 1));
        assert_eq!(crate::sidecar::load_book(&files.root).unwrap().sheets["folder/sheet.png"].label, Some(old.clone()));
        let (failed, _) = files.advance(job.clone(), |_, _| Err(Failure::Status(400, String::new())));
        let mut panel = Panel { root: files.root.clone(), dir: files.spool.clone(), job: Some(failed), ..Panel::default() };
        assert!(panel.import().is_empty());
        assert_eq!(crate::sidecar::load_book(&files.root).unwrap().sheets["folder/sheet.png"].label, Some(old));
        let (job, result) = files.advance(job, |_, _| Ok(completion(labeled("New"))));
        result.unwrap();
        panel.job = Some(job);
        let (reply, received) = mpsc::channel(); panel.pending_save = Some(reply);
        assert_eq!(panel.import().len(), 1);
        assert_eq!(received.recv().unwrap().saved, vec![0]);
        assert_eq!(crate::sidecar::load_book(&files.root).unwrap().sheets["folder/sheet.png"].label.as_ref().unwrap().caption, "New");
    }

    #[test]
    fn clear_discards_finished_results_but_refuses_active_batches() {
        let files = Files::new();
        let job = files.prepare(Kind::OpenAi);
        let mut panel = Panel { root: files.root.clone(), dir: files.spool.clone(), job: Some(job.clone()), ..Panel::default() };
        assert!(panel.discard_completed(&files.root).is_err());
        let (job, result) = files.advance(job, |_, _| Ok(completion(labeled("New"))));
        result.unwrap();
        let (report, _) = save_labels(&job, &files.root);
        let mut job = job;
        for i in report.saved { job.sheets[i].imported = true; }
        job.save(&files.root).unwrap();
        panel.job = Some(job);
        panel.lock = Some(runner::lock(&files.spool).unwrap());
        panel.discard_completed(&files.root).unwrap();
        assert!(panel.import().is_empty());
        assert!(Job::load(&files.spool).unwrap().is_none());
        assert!(!panel.busy());
    }

    /// An OpenAI-style endpoint gets one request per sheet, as Label with AI
    /// sends it, and the job is done when every sheet had its turn.
    #[test]
    fn an_openai_endpoint_labels_one_sheet_at_a_time() {
        let files = Files::new();
        std::fs::write(files.root.join("broken.png"), b"not an image").unwrap();
        RgbaImage::new(4, 4).save(files.root.join("more.png")).unwrap();
        let mut job = files.prepare(Kind::OpenAi);
        let mut sent = 0;
        while !job.remote_done() {
            let result;
            (job, result) = files.advance(job, |path, request| {
                assert_eq!(path, "chat/completions");
                assert_eq!(request.unwrap()["model"], "test");
                sent += 1;
                Ok(completion(labeled("Forest")))
            });
            result.unwrap();
        }
        assert_eq!(sent, 2, "the sheet that cannot be read sends nothing");
        assert!(job.sheets[0].error.contains("Could not read"));
        assert_eq!(job.sheets[1].label.as_ref().unwrap().caption, "Forest");
        assert!(job.groups.is_empty());
        assert!(files.journal().remote_done());
    }

    /// A job asks for the tag list the library had when it started, and
    /// every label records it, though the list changes while the job runs.
    #[test]
    fn a_job_keeps_the_tag_list_it_started_with() {
        let files = Files::new();
        crate::sidecar::store_tag_list(&files.root, &["tree".into()]).unwrap();
        let mut job = files.prepare(Kind::OpenAi);
        crate::sidecar::store_tag_list(&files.root, &["house".into()]).unwrap();
        let result;
        (job, result) = files.advance(job, |_, request| {
            assert!(request.unwrap()["messages"][0]["content"].as_str().unwrap().contains("true or false: tree."));
            Ok(completion(json!({"status":"labeled", "caption":"Forest", "tags":["pines"], "listed":{"tree":true}})))
        });
        result.unwrap();
        let label = job.sheets[0].label.as_ref().unwrap();
        assert_eq!(label.tags, ["pines", "tree"]);
        assert_eq!(label.tag_list, Some(vec!["tree".to_string()]));
        assert_eq!(files.journal().tag_list, ["tree"]);
    }

    #[test]
    fn a_resumed_batch_keeps_its_provider_skip_list() {
        let files = Files::new();
        let mut job = files.prepare(Kind::OpenAi);
        job.provider.skip = Some(vec!["phala".into(), "another-provider".into()]);
        job.save(&files.spool).unwrap();
        let (_, result) = files.advance(files.journal(), |_, body| {
            assert_eq!(body.unwrap()["provider"]["ignore"], json!(["phala", "another-provider"]));
            Ok(completion(labeled("Tree")))
        });
        result.unwrap();
    }

    /// A request that did not arrive, or that the provider could not take
    /// now, goes out again for the same sheet. A refusal ends that sheet.
    #[test]
    fn an_openai_request_that_failed_goes_out_again() {
        let files = Files::new();
        let failures = [Failure::NotSent("offline".into()), Failure::Status(429, String::new()),
            Failure::Status(503, String::new()), Failure::Unknown("timeout".into())];
        for failure in failures {
            let mut failure = Some(failure);
            let (job, result) = files.advance(files.prepare(Kind::OpenAi), |_, _| Err(failure.take().unwrap()));
            assert!(result.is_err());
            assert!(!job.sheets[0].taken && job.sheets[0].error.is_empty());
            let (job, result) = files.advance(job, |_, _| Ok(completion(labeled("Tree"))));
            result.unwrap();
            assert!(job.remote_done() && job.sheets[0].label.is_some());
        }
        let (job, result) = files.advance(files.prepare(Kind::OpenAi), |_, _| Err(Failure::Status(400, String::new())));
        result.unwrap();
        assert!(job.remote_done() && job.sheets[0].error.contains("400"));
    }

    /// A request whose answer was lost or unreadable may have been billed,
    /// so one sheet goes out at most three times that way. Offline tries
    /// do not count.
    #[test]
    fn an_unreadable_answer_is_not_paid_for_forever() {
        let files = Files::new();
        let mut job = files.prepare(Kind::OpenAi);
        let mut sent = 0;
        for _ in 0..10 {
            let result;
            (job, result) = files.advance(job, |_, _| Err(Failure::NotSent("offline".into())));
            assert!(result.is_err());
        }
        while !job.remote_done() {
            let result;
            (job, result) = files.advance(job, |_, _| { sent += 1; Err(Failure::Unknown("Invalid or oversized batch response.".into())) });
            assert_eq!(result.is_err(), sent < 3);
        }
        assert_eq!(sent, 3);
        assert!(job.sheets[0].taken && job.sheets[0].error.contains("3 tries"));
        assert_eq!(files.journal().sheets[0].unknown, 2);
    }

    /// An answer in prose instead of the label goes out again, as a lost
    /// one does, and the third ends the sheet. The next may reach a provider
    /// that keeps to the schema.
    #[test]
    fn an_answer_in_prose_goes_out_again() {
        let files = Files::new();
        RgbaImage::new(4, 4).save(files.root.join("more.png")).unwrap();
        let mut job = files.prepare(Kind::OpenAi);
        let prose = json!({"choices":[{"finish_reason":"stop", "message":{"content":"**Caption:** Trees"}}]});
        for answer in [prose.clone(), completion(labeled("Forest"))] {
            let result;
            (job, result) = files.advance(job, |_, _| Ok(answer.clone()));
            result.unwrap();
        }
        assert_eq!(job.sheets[0].label.as_ref().unwrap().caption, "Forest");
        assert_eq!(job.sheets[0].unknown, 1);
        while !job.remote_done() {
            let result;
            (job, result) = files.advance(job, |_, _| Ok(prose.clone()));
            result.unwrap();
        }
        assert!(job.sheets[1].error == labels::INVALID && job.sheets[1].unknown == 2);
    }

    /// A journal of 0.2 holds OpenRouter batch groups that never finish.
    /// Their sheets go out one at a time instead, and the job ends.
    #[test]
    fn an_openai_journal_of_0_2_finishes() {
        let files = Files::new();
        RgbaImage::new(4, 4).save(files.root.join("more.png")).unwrap();
        let mut job = files.prepare(Kind::OpenAi);
        for s in &mut job.sheets { s.taken = true; }
        job.groups = vec![Group { sheets: vec![0], remote: Remote::Waiting("batch-1".into()), recovery: None, tracking: Tracking::default() },
            Group { sheets: vec![1], remote: Remote::Submitting, recovery: None, tracking: Tracking::default() }];
        assert!(!job.uncertain() && !job.remote_done());
        while !job.remote_done() {
            let result;
            (job, result) = files.advance(job, |path, _| { assert_eq!(path, "chat/completions"); Ok(completion(labeled("Tree"))) });
            result.unwrap();
        }
        assert!(job.groups.is_empty());
        assert!(job.sheets.iter().all(|s| s.label.is_some()));
    }

    #[test]
    fn a_batch_submits_waits_and_saves_its_labels() {
        let files = Files::new();
        std::fs::write(files.root.join("broken.png"), b"not an image").unwrap();
        let (job, result) = files.advance(files.prepare(Kind::Gemini), |path, request| {
            assert_eq!(path, "models/test:batchGenerateContent");
            assert!(files.journal().uncertain(), "the journal says Submitting before the request goes out");
            assert_eq!(request.unwrap()["batch"]["inputConfig"]["requests"]["requests"].as_array().unwrap().len(), 1);
            Ok(json!({"name":"batches/batch-1"}))
        });
        result.unwrap();
        assert!(job.sheets[0].error.contains("Could not read"));
        assert!(matches!(&files.journal().groups[0].remote, Remote::Waiting(id) if id == "batches/batch-1"));
        let (job, result) = files.advance(job, |path, request| {
            assert_eq!(path, "batches/batch-1");
            assert!(request.is_none());
            Ok(json!({"done":false}))
        });
        result.unwrap();
        assert!(!job.remote_done());
        let reply = completed(&job, 0, "Forest");
        let (job, result) = files.advance(job, |_, _| Ok(reply.clone()));
        result.unwrap();
        assert!(job.remote_done());
        assert_eq!(job.sheets[1].label.as_ref().unwrap().caption, "Forest");
        assert!(files.journal().remote_done());
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
        let (_, result) = files.advance(files.prepare(Kind::OpenAi), |_, request| {
            let url = request.unwrap()["messages"][1]["content"][1]["image_url"]["url"].as_str().unwrap().to_string();
            let png = STANDARD.decode(url.strip_prefix("data:image/png;base64,").unwrap()).unwrap();
            assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8(), first);
            Ok(completion(labeled("Waterfall")))
        });
        result.unwrap();
    }

    #[test]
    fn a_submission_that_never_left_goes_out_again() {
        let files = Files::new();
        for failure in [Failure::NotSent("offline".into()), Failure::Status(429, String::new())] {
            let mut failure = Some(failure);
            let (job, result) = files.advance(files.prepare(Kind::Gemini), |_, _| Err(failure.take().unwrap()));
            assert!(result.is_err());
            assert!(job.groups.is_empty() && !job.sheets[0].taken);
            assert!(!files.journal().uncertain());
            let (job, result) = files.advance(job, id("batches/batch-2"));
            result.unwrap();
            assert!(matches!(&job.groups[0].remote, Remote::Waiting(_)));
        }
    }

    #[test]
    fn an_unconfirmed_submission_that_never_left_goes_out_again() {
        let files = Files::new();
        let (_, result) = files.advance(files.prepare(Kind::Gemini), |_, _| Err(Failure::Unknown("timeout".into())));
        assert!(result.is_err());
        let job = files.journal();
        assert!(job.uncertain());
        let (job, result) = files.advance(job, |path, body| {
            assert_eq!(path, "batches?pageSize=100");
            assert!(body.is_none(), "Recovery must never send another paid request.");
            Ok(json!({"operations":[]}))
        });
        result.unwrap();
        assert!(!job.uncertain(), "a batch Google never made is released for another try");
        assert!(!job.sheets[0].taken);
        let (job, result) = files.advance(job, id("batches/batch-3"));
        result.unwrap();
        assert!(matches!(&job.groups[0].remote, Remote::Waiting(_)));
    }

    #[test]
    fn an_unconfirmed_group_does_not_block_new_sheets() {
        let files = Files::new();
        let (mut job, _) = files.advance(files.prepare(Kind::Gemini), |_, _| Err(Failure::Unknown("Lost reply".into())));
        RgbaImage::from_pixel(8, 4, Rgba([80, 90, 100, 255])).save(files.root.join("another.png")).unwrap();
        let extra = files.prepare(Kind::Gemini).sheets.into_iter().find(|s| s.rel == "another.png").unwrap();
        job.sheets.push(extra);
        let (job, _) = files.advance(job, |_, body| {
            assert!(body.is_none());
            Ok(json!({"operations":[]}))
        });
        let job = { job.save(&files.spool).unwrap(); files.journal() };
        let (job, result) = files.advance(job, |path, body| {
            assert!(path.ends_with(":batchGenerateContent"), "Recovery blocked the queued sheets: {path}");
            let requests = body.unwrap().pointer("/batch/inputConfig/requests/requests").unwrap().as_array().unwrap();
            assert_eq!(requests.len(), 2, "the released sheet joins the queued one");
            assert_eq!(requests[0]["metadata"]["key"], "sheet-0");
            assert_eq!(requests[1]["metadata"]["key"], "sheet-1");
            Ok(json!({"name":"batches/next"}))
        });
        result.unwrap();
        assert!(!job.uncertain());
        assert_eq!(job.groups.len(), 1);
    }

    #[test]
    fn a_rejected_new_upload_does_not_resend_an_older_uncertain_group() {
        let files = Files::new();
        let (mut job, _) = files.advance(files.prepare(Kind::Gemini), |_, _| Err(Failure::Unknown("Lost reply".into())));
        RgbaImage::from_pixel(8, 4, Rgba([80, 90, 100, 255])).save(files.root.join("another.png")).unwrap();
        job.sheets.push(files.prepare(Kind::Gemini).sheets.into_iter().find(|s| s.rel == "another.png").unwrap());
        job.submit_next = true;
        let (job, result) = files.advance(job, |_, body| {
            assert!(body.is_some());
            Err(Failure::Status(429, "Try later".into()))
        });
        assert!(result.is_err());
        assert_eq!(job.groups.len(), 1);
        assert_eq!(job.groups[0].remote, Remote::Submitting);
        assert!(job.sheets[0].taken);
        assert!(!job.sheets[1].taken);
    }

    #[test]
    fn recovery_checks_each_unconfirmed_group() {
        let files = Files::new();
        let (mut job, _) = files.advance(files.prepare(Kind::Gemini), |_, _| Err(Failure::Unknown("Lost reply".into())));
        let mut second = job.groups[0].clone();
        second.recovery.as_mut().unwrap().reference = "second".into();
        job.groups.push(second);
        // The first lookup proves the first group absent, so it goes out again.
        let (mut job, result) = files.advance(job, |_, _| Ok(json!({"operations":[]})));
        result.unwrap();
        assert_eq!(job.groups.len(), 1);
        assert_eq!(job.groups[0].recovery.as_ref().unwrap().reference, "second");
        // The next lookup turns to the second group, which Google did make.
        job.submit_next = false;
        let (job, result) = files.advance(job, |_, _| Ok(json!({"operations":[
            {"name":"batches/second", "metadata":{"displayName":"second", "model":"models/test"}}
        ]})));
        result.unwrap();
        assert!(job.groups.iter().any(|g| g.remote == Remote::Waiting("batches/second".into())));
        assert!(!job.uncertain());
    }

    #[test]
    fn recovery_finds_the_saved_reference_across_pages_after_restart() {
        let files = Files::new();
        let (_, result) = files.advance(files.prepare(Kind::Gemini), |_, body| {
            let saved = files.journal();
            let reference = &saved.groups[0].recovery.as_ref().unwrap().reference;
            assert_eq!(body.unwrap()["batch"]["displayName"], *reference);
            Err(Failure::Unknown("Lost reply".into()))
        });
        assert!(result.is_err());
        let mut job = files.journal();
        let reference = job.groups[0].recovery.as_ref().unwrap().reference.clone();
        let (next, result) = files.advance(job, |path, body| {
            assert_eq!(path, "batches?pageSize=100");
            assert!(body.is_none());
            Ok(json!({"operations":[], "nextPageToken":"a/b"}))
        });
        result.unwrap();
        assert!(next.uncertain());
        job = files.journal();
        let (job, result) = files.advance(job, |path, body| {
            assert_eq!(path, "batches?pageSize=100&pageToken=%61%2F%62");
            assert!(body.is_none());
            Ok(json!({"operations":[{"name":"batches/recovered", "metadata":{"displayName":reference, "model":"models/test"}}]}))
        });
        result.unwrap();
        assert_eq!(job.groups[0].remote, Remote::Waiting("batches/recovered".into()));
        assert!(!files.journal().uncertain());
    }

    #[test]
    fn ambiguous_recovery_never_attaches_or_resubmits() {
        let files = Files::new();
        let (job, _) = files.advance(files.prepare(Kind::Gemini), |_, _| Err(Failure::Unknown("Lost reply".into())));
        let reference = job.groups[0].recovery.as_ref().unwrap().reference.clone();
        let (job, result) = files.advance(job, |_, body| {
            assert!(body.is_none());
            Ok(json!({"operations":[
                {"name":"batches/first", "metadata":{"displayName":reference, "model":"models/test"}},
                {"name":"batches/second", "metadata":{"displayName":reference, "model":"models/test"}}
            ]}))
        });
        assert!(result.unwrap_err().contains("more than one"));
        assert!(job.uncertain());
    }

    #[test]
    fn accepted_groups_are_polled_while_other_sheets_wait_for_submission() {
        let files = Files::new();
        RgbaImage::from_pixel(8, 4, Rgba([80, 90, 100, 255])).save(files.root.join("another.png")).unwrap();
        let mut job = files.prepare(Kind::Gemini);
        job.sheets[0].taken = true;
        job.groups.push(Group { sheets: vec![0], remote: Remote::Waiting("batches/accepted".into()), recovery: None, tracking: Tracking::default() });
        job.poll_next = true;
        let reply = completed(&job, 0, "Crystals");
        let (job, result) = files.advance(job, |path, body| {
            assert_eq!(path, "batches/accepted");
            assert!(body.is_none());
            Ok(reply.clone())
        });
        result.unwrap();
        assert!(job.sheets[0].label.is_some());
        assert!(!job.sheets[1].taken);
    }

    #[test]
    fn a_refused_submission_fails_its_sheets() {
        let files = Files::new();
        let (job, result) = files.advance(files.prepare(Kind::Gemini), |_, _| Err(Failure::Status(400, String::new())));
        result.unwrap();
        assert!(job.sheets[0].error.contains("400"));
        assert!(job.remote_done());
    }

    #[test]
    fn provider_explanations_reach_the_saved_sheet_errors() {
        for kind in [Kind::Gemini, Kind::OpenAi] {
            let files = Files::new();
            let message = "The endpoint returned HTTP 400. This model does not support the requested schema.";
            let (job, result) = files.advance(files.prepare(kind), |_, _| Err(Failure::Status(400, message.into())));
            result.unwrap();
            assert!(job.remote_done());
            assert_eq!(files.journal().sheets[0].error, message);
        }
    }

    #[test]
    fn completed_batches_keep_job_and_sheet_error_details() {
        let error = json!({"code":3, "message":"The requested image format is not supported."});
        for response in [json!({"done":true, "error":error}), json!({"done":true,
            "response":{"inlinedResponses":{"inlinedResponses":[{"metadata":{"key":"sheet-0"}, "error":error}]}}
        })] {
            let files = Files::new();
            let (job, result) = files.advance(files.prepare(Kind::Gemini), id("batches/test"));
            result.unwrap();
            let (job, result) = files.advance(job, |_, _| Ok(response.clone()));
            result.unwrap();
            assert!(job.remote_done());
            assert!(files.journal().sheets[0].error.contains("The requested image format is not supported."));
        }
    }

    #[test]
    fn results_follow_their_ids_and_missing_results_fail() {
        let files = Files::new();
        let mut job = files.prepare(Kind::Gemini);
        job.sheets.push(job.sheets[0].clone());
        job.groups.push(Group { sheets: vec![0, 1], remote: Remote::Waiting("batches/x".into()), recovery: None, tracking: Tracking::default() });
        accept(&mut job, 0, vec![(key(1), completion(labeled("Second"))), ("unknown".into(), Value::Null)]);
        assert!(job.sheets[0].label.is_none());
        assert!(!job.sheets[0].error.is_empty());
        assert_eq!(job.sheets[1].label.as_ref().unwrap().caption, "Second");
    }

    #[test]
    fn polls_take_turns() {
        let files = Files::new();
        let mut job = files.prepare(Kind::Gemini);
        job.sheets[0].taken = true;
        job.groups = vec![Group { sheets: vec![0], remote: Remote::Waiting("batches/first".into()), recovery: None, tracking: Tracking::default() },
            Group { sheets: vec![0], remote: Remote::Waiting("batches/second".into()), recovery: None, tracking: Tracking::default() }];
        let (job, _) = files.advance(job, |path, _| { assert_eq!(path, "batches/first"); Ok(json!({"done":false})) });
        let (_, _) = files.advance(job, |path, _| { assert_eq!(path, "batches/second"); Ok(json!({"done":false})) });
    }

    #[test]
    fn repeated_failures_back_off_independently() {
        let files = Files::new();
        let mut job = files.prepare(Kind::Gemini);
        let mut waits = Vec::new();
        for _ in 0..7 { job.issue("upload", "offline".into(), 1_000); waits.push((job.issues["upload"].retry_ms - 1_000) / 1000); }
        assert_eq!(waits, vec![30, 60, 120, 240, 480, 480, 480]);
        job.issue("check", "offline".into(), 1_000);
        assert_eq!(job.issues["check"].retry_ms, 31_000);
    }

    #[test]
    fn batch_errors_keep_the_provider_explanation() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                        std::thread::sleep(Duration::from_millis(10)),
                    Err(e) => panic!("Test server did not receive a connection: {e}"),
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 8192);
            }
            let body = r#"{"error":{"code":400,"message":"The selected model does not support batch requests."}}"#;
            write!(stream, "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()).unwrap();
        });
        // Only this test uses HTTP and a local server. No real key or provider is used.
        let transport = Transport { client: labels::agent(), base, key: "test-key".into(), kind: Kind::Gemini };
        let failure = transport.send("batches/test", None).unwrap_err();
        server.join().unwrap();
        assert!(failure.message().contains("The selected model does not support batch requests."), "{}", failure.message());
    }

    #[test]
    fn gemini_request_and_response_use_the_same_structured_label() {
        let request = labels::request("test", &RgbaImage::new(2, 2), &[], crate::sidecar::FREE_TAGS).unwrap();
        let gemini = crate::gemini::request(&request);
        assert_eq!(gemini["generationConfig"]["responseMimeType"], "application/json");
        assert!(gemini["generationConfig"]["responseJsonSchema"].is_object());
        assert!(gemini.to_string().contains("inlineData"));
        let response = json!({"done":true, "response":{"inlinedResponses":{"inlinedResponses":[{
            "metadata":{"key":"sheet-0"}, "response":{"candidates":[{"finishReason":"STOP", "content":{"parts":[{
                "text":completion(labeled("Tree"))["choices"][0]["message"]["content"]
            }]}}]}
        }]}}});
        let out = outputs(&response).unwrap().unwrap();
        assert_eq!(out[0].0, "sheet-0");
        assert_eq!(labels::response(&out[0].1, &[]).unwrap().into_label("", "", &[]).caption, "Tree");
        let mut bad = response.clone();
        bad["response"]["inlinedResponses"]["inlinedResponses"][0]["response"]["candidates"][0]["finishReason"] = json!("SAFETY");
        let out = outputs(&bad).unwrap().unwrap();
        assert!(labels::response(&out[0].1, &[]).is_err());
        assert!(outputs(&json!({"done":false})).unwrap().is_none());
        let mut twice = response.clone();
        let item = twice["response"]["inlinedResponses"]["inlinedResponses"][0].clone();
        twice["response"]["inlinedResponses"]["inlinedResponses"] = json!([item.clone(), item]);
        assert!(outputs(&twice).is_err());
    }

    #[test]
    fn provider_and_resource_paths_do_not_redirect_keys() {
        assert_eq!(endpoint(&provider(Kind::OpenAi)).unwrap(), "https://openrouter.ai/api/v1");
        let mut p = provider(Kind::OpenAi);
        p.url = "https://example.test/v1/chat/completions".into();
        assert_eq!(endpoint(&p).unwrap(), "https://example.test/v1");
        assert!(validate_id("batches/test_1").is_ok());
        for id in ["", "batches/", "../files", "batches/abc?key=other", "https://other.test", "batches/abc/def", "abc"] {
            assert!(validate_id(id).is_err(), "{id}");
        }
        let mut p = provider(Kind::Gemini);
        p.url = "https://user:password@example.test".into();
        assert!(endpoint(&p).is_err());
    }

    #[test]
    fn library_usage_includes_paid_invalid_retries() {
        let files = Files::new();
        let mut invalid = json!({"choices":[{"finish_reason":"stop", "message":{"content":"**Caption:** Trees"}}]});
        invalid["usage"] = json!({"prompt_tokens":500, "completion_tokens":80});
        let (job, result) = files.advance(files.prepare(Kind::OpenAi), |_, _| Ok(invalid.clone()));
        result.unwrap(); assert!(!job.done());
        let mut valid = completion(labeled("Tile"));
        valid["usage"] = json!({"prompt_tokens":510, "completion_tokens":100});
        let (job, result) = files.advance(job, |_, _| Ok(valid.clone()));
        result.unwrap();
        assert_eq!(job.usage.requests, 2); assert_eq!(job.usage.input, 1010); assert_eq!(job.usage.output, 180);
        assert_eq!(files.journal().usage.output, 180);
    }

    #[test]
    fn google_usage_survives_batch_response_conversion() {
        let files = Files::new();
        let (job, result) = files.advance(files.prepare(Kind::Gemini), id("batches/test")); result.unwrap();
        let mut response = completed(&job, 0, "Tile");
        response["response"]["inlinedResponses"]["inlinedResponses"][0]["response"]["usageMetadata"] =
            json!({"promptTokenCount":1000, "candidatesTokenCount":200, "thoughtsTokenCount":800});
        let (job, result) = files.advance(job, |_, _| Ok(response.clone())); result.unwrap();
        assert_eq!(job.usage.requests, 1); assert_eq!(job.usage.output, 1000);
        let (job, result) = files.advance(job, |_, _| panic!("A completed group must not be counted again.")); result.unwrap();
        assert_eq!(job.usage.requests, 1);
    }

}
