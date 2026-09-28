// SPDX-License-Identifier: GPL-3.0-only
//! One library job: selected sheets, each sent as the request that
//! Label with AI sends. Google's Gemini API takes them as a batch. An
//! OpenAI-style endpoint takes them one at a time: OpenRouter's batch API
//! reads only images at public URLs, and a library lies on this machine.
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
    /// Requests for this sheet that may have arrived but gave no answer the
    /// tool could read. See `one`.
    #[serde(default)]
    pub unknown: u32,
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

#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    pub provider: Provider,
    pub model: String,
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
    /// Alternate submissions and polling so early results can arrive before the whole library is sent.
    #[serde(default)]
    poll_next: bool,
}

impl Job {
    fn untaken(&self) -> bool { self.sheets.iter().any(|s| !s.taken) }
    pub fn done(&self) -> bool { !self.untaken() && self.groups.iter().all(|g| g.remote == Remote::Done) }
    /// A Gemini batch went out and no reply confirmed it. An OpenAI-style job
    /// is never so: its groups are released; see `release_groups`.
    pub fn uncertain(&self) -> bool {
        self.provider.kind == Kind::Gemini && self.groups.iter().any(|g| g.remote == Remote::Submitting)
    }
    fn progress(&self) -> String {
        let labeled = self.sheets.iter().filter(|s| s.label.as_ref().is_some_and(|l| l.status == Status::Labeled)).count();
        let failed = self.sheets.iter().filter(|s| !s.error.is_empty()).count();
        let base = format!("Labeled: {labeled} of {}. Failed: {failed}.", self.sheets.len());
        if self.provider.kind != Kind::Gemini { return base; }
        let (queued, waiting, uncertain) = self.submission_counts();
        format!("{base}\nQueued: {queued}. At provider: {waiting}. Unconfirmed: {uncertain}.")
    }
    fn submission_counts(&self) -> (usize, usize, usize) {
        let queued = self.sheets.iter().filter(|s| !s.taken).count();
        let waiting = self.groups.iter().filter(|g| matches!(g.remote, Remote::Waiting(_))).map(|g| g.sheets.len()).sum();
        let uncertain = self.groups.iter().filter(|g| g.remote == Remote::Submitting).map(|g| g.sheets.len()).sum();
        (queued, waiting, uncertain)
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

/// The request body for one sheet in a Gemini batch.
fn body(job: &Job, img: &image::RgbaImage) -> Result<Value, String> {
    Ok(gemini_request(&labels::request(&job.model, img, &job.tag_list)?))
}

#[derive(Clone, Copy, PartialEq)]
pub enum Scope { Unlabeled, All }

/// Lists the requested sheets. It reads no image and sends nothing.
pub fn prepare(index: &Index, provider: Provider, model: String, scope: Scope) -> Result<Job, String> {
    endpoint(&provider)?;
    if let Some(error) = &index.error { return Err(error.clone()); }
    let labeled = index.entries.iter().filter(|e| e.side.label.is_some()).count();
    let all = scope == Scope::All;
    let open = index.entries.iter().filter(|e| all || e.side.label.is_none());
    Ok(Job {
        provider, model: model.trim_end_matches(":batch").into(), groups: vec![], skipped: if all { 0 } else { labeled },
        replacing: if all { labeled } else { 0 }, tag_list: index.tag_list.clone(), poll_next: false,
        sheets: open.map(|e| Sheet { rel: e.rel.clone(), taken: false, label: None, error: String::new(), imported: false, unknown: 0 })
            .collect(),
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
        if item["response"]["candidates"].as_array().is_none_or(|a| a.len() != 1)
            || !item["response"]["promptFeedback"]["blockReason"].is_null() {
            out.push((key.into(), Value::Null));
            continue;
        }
        let candidate = &item["response"]["candidates"][0];
        let parts = candidate["content"]["parts"].as_array();
        let text = parts.map(|p| p.iter().filter(|p| p["thought"] != true).filter_map(|p| p["text"].as_str()).collect::<String>());
        out.push((key.into(), json!({"choices":[{"finish_reason": if candidate["finishReason"] == "STOP" {"stop"} else {"invalid"},
            "message":{"content":text}}]})));
    }
    Ok(Some(out))
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
        let reply = values.remove(&key(i)).ok_or("No result returned for this request.".into()).and_then(|v|
            labels::diagnosed_response(&job.sheets[i].rel, &job.provider.name, &job.model, &v, &job.tag_list));
        record(&mut job.sheets[i], (&job.provider.name, &job.model, &job.tag_list), reply);
    }
}

type Send<'a> = &'a mut dyn FnMut(&str, Option<&Value>) -> Result<Value, Failure>;

/// Does one network operation: label the next sheet, submit the next group
/// of sheets, or ask about a submitted one. The job is saved before it
/// returns, with or without error.
pub fn advance(mut job: Job, root: &Path, dir: &Path, mut send: impl FnMut(&str, Option<&Value>) -> Result<Value, Failure>) -> (Job, Result<(), String>) {
    let waiting = job.groups.iter().any(|g| matches!(g.remote, Remote::Waiting(_)));
    let result = if job.provider.kind == Kind::OpenAi {
        job.release_groups();
        one(&mut job, root, dir, &mut send)
    } else if waiting && (job.poll_next || (!job.untaken() && !job.uncertain())) {
        job.poll_next = false;
        poll(&mut job, dir, &mut send)
    } else if job.uncertain() {
        job.poll_next = true;
        recover(&mut job, dir, &mut send)
    } else if job.untaken() {
        job.poll_next = true;
        submit(&mut job, root, dir, &mut send)
    } else {
        poll(&mut job, dir, &mut send)
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
    let request = image::open(root.join(&job.sheets[i].rel)).map_err(|e| format!("Could not read the image: {e}"))
        .and_then(|img| labels::request(&job.model, &img.to_rgba8(), &job.tag_list))
        .map(|body| job.provider.route(body));
    let reply = match request {
        Err(error) => Err(error),
        Ok(request) => match send("chat/completions", Some(&request)) {
            // One provider of a model on OpenRouter may answer in prose. The
            // next request may reach another.
            Ok(value) => match labels::diagnosed_response(&job.sheets[i].rel, &job.provider.name, &job.model, &value, &job.tag_list) {
                Err(error) if error == labels::INVALID && job.sheets[i].unknown + 1 < UNKNOWN_TRIES => {
                    job.sheets[i].unknown += 1;
                    return job.save(dir);
                }
                reply => reply,
            },
            Err(Failure::Unknown(message)) if job.sheets[i].unknown + 1 < UNKNOWN_TRIES => {
                job.sheets[i].unknown += 1;
                return job.save(dir).and(Err(message));
            }
            Err(Failure::Unknown(message)) => Err(format!("{message} No readable answer after {UNKNOWN_TRIES} tries.")),
            Err(failure @ (Failure::NotSent(_) | Failure::Status(429 | 500..=599, _))) => {
                return job.save(dir).and(Err(failure.message()));
            }
            Err(failure @ Failure::Status(..)) => Err(failure.message()),
        },
    };
    record(&mut job.sheets[i], (&job.provider.name, &job.model, &job.tag_list), reply);
    job.sheets[i].taken = true;
    job.save(dir)
}

/// Reads the next sheets, and submits them as one Gemini batch. The group is saved
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
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let reference = format!("Tilepicky-{}-{stamp}", std::process::id());
    job.groups.push(Group { sheets, remote: Remote::Submitting,
        recovery: Some(Recovery { reference: reference.clone(), ..Recovery::default() }) });
    job.save(dir)?;
    crate::ai_log::event("batch_submit", json!({"provider":job.provider.name, "model":job.model, "tags_requested":job.tag_list,
        "sheets":requests.iter().map(|(i, _)| json!({"id":key(*i), "sheet":job.sheets[*i].rel})).collect::<Vec<_>>()}));
    let path = format!("models/{}:batchGenerateContent", job.model);
    let requests: Vec<_> = requests.into_iter().map(|(i, body)| (key(i), body)).collect();
    let g = job.groups.len() - 1;
    let result = match send(&path, Some(&submit_body(&requests, &reference))) {
        Ok(response) => remote_id(&response).map(|id| job.groups[g].remote = Remote::Waiting(id)),
        // The provider made no batch: the sheets wait for the next try.
        Err(failure @ (Failure::NotSent(_) | Failure::Status(429 | 503, _))) => {
            job.send_again();
            Err(failure.message())
        }
        Err(failure @ Failure::Status(400..500, _)) => {
            for &i in &job.groups[g].sheets { job.sheets[i].error = failure.message(); }
            job.groups[g].remote = Remote::Done;
            Ok(())
        }
        Err(failure) => Err(failure.message()),
    };
    job.save(dir).and(result)
}

/// Recover by the unique reference saved before submission. Never resend an uncertain request.
fn recover(job: &mut Job, dir: &Path, send: Send) -> Result<(), String> {
    let i = job.groups.iter().position(|g| g.remote == Remote::Submitting).ok_or("No submission needs recovery.")?;
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
    let result = match matches.as_slice() {
        [id] => {
            crate::ai_log::event("batch_recovered", json!({"reference":recovery.reference, "id":id}));
            job.groups[i].remote = Remote::Waiting(id.clone());
            Ok(())
        }
        [] => Err("Google has not listed this submission yet. Recovery will retry without sending the sheets again.".into()),
        _ => Err("Google returned more than one matching batch. Open Advanced recovery; nothing was resent.".into()),
    };
    job.save(dir).and(result)
}

fn poll(job: &mut Job, dir: &Path, send: Send) -> Result<(), String> {
    let Some(i) = job.groups.iter().position(|g| matches!(g.remote, Remote::Waiting(_))) else { return Ok(()) };
    let Remote::Waiting(id) = job.groups[i].remote.clone() else { unreachable!() };
    validate_id(&id)?;
    let response = send(&id, None).map_err(|f| f.message())?;
    match outputs(&response) {
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

    /// Lets the sheets of an unfinished group go out one at a time. An
    /// OpenAI-style job of 0.2 went through OpenRouter's batch API, which
    /// read no local image: its groups never finish, and nothing asks about
    /// them any more.
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

    pub fn busy(&self) -> bool { self.running() || self.task.is_some() || self.open() }

    /// A compact status for the main window when no newer message takes its place.
    pub fn status(&self) -> Option<String> {
        let job = self.job.as_ref()?;
        if job.done() && self.error.is_empty() { return None; }
        let labeled = job.sheets.iter().filter(|s| s.label.as_ref().is_some_and(|l| l.status == Status::Labeled)).count();
        let failed = job.sheets.iter().filter(|s| !s.error.is_empty()).count();
        let pending = job.sheets.iter().filter(|s| s.label.is_none() && s.error.is_empty()).count();
        let state = if job.uncertain() {
                if job.groups.iter().any(|g| g.remote == Remote::Submitting && g.recovery.is_none()) { "needs attention" } else { "recovering" }
            }
            else if self.task.is_some() { "working" }
            else if !self.error.is_empty() { "retry pending" } else { "waiting" };
        if job.provider.kind == Kind::Gemini {
            let (queued, waiting, uncertain) = job.submission_counts();
            Some(format!("AI batch: {queued} queued, {waiting} at provider, {uncertain} unconfirmed, {labeled} labeled, {failed} failed ({state})"))
        } else {
            Some(format!("AI batch: {labeled} labeled, {pending} pending, {failed} failed ({state})"))
        }
    }

    /// Prevents a completed journal from restoring labels after Clear all.
    pub fn discard_completed(&mut self, root: &Path) -> Result<(), String> {
        if self.busy() { return Err("Wait for the batch to finish or cancel it first.".into()); }
        if self.root != root { return Err("The batch panel belongs to another library.".into()); }
        if self.dir.as_os_str().is_empty() { return Err("The batch journal directory is unavailable.".into()); }
        match std::fs::remove_file(self.dir.join("state.json")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Could not remove the completed batch journal: {e}")),
        }
        self.job = None;
        (self.error, self.failures, self.next_check, self.retry) = (String::new(), 0, None, false);
        Ok(())
    }

    /// Records a failure. The next try waits longer after each one, up to 8 minutes.
    pub fn fail(&mut self, error: String) {
        crate::ai_log::event("batch_error", json!({"error":error, "retry":self.failures + 1}));
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
        if self.task.is_none() && due && let Some(job) = &self.job && !job.done() {
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
        crate::ai_log::event("batch_saved", json!({"provider":job.provider.name, "model":job.model, "labels":labels}));
        for sheet in job.sheets.iter_mut().filter(|s| s.label.is_some()) { sheet.imported = true; }
        if let Err(e) = job.save(&self.dir) { self.fail(e); }
        labels
    }

    /// Forgets the batch here, and asks the provider to cancel what it still runs.
    /// Labels already in the book stay.
    fn cancel(&mut self, keys: &crate::ai::Keys) {
        let Some(job) = self.job.take() else { return };
        crate::ai_log::event("batch_cancel", json!({"provider":job.provider.name, "model":job.model}));
        // A running worker finishes first; `tick` forgets its result too.
        if self.task.is_none() { self.forget(&job, keys); }
        (self.error, self.failures, self.next_check) = (String::new(), 0, None);
    }

    fn forget(&self, job: &Job, keys: &crate::ai::Keys) {
        let _ = std::fs::remove_file(self.dir.join("state.json"));
        let ids: Vec<_> = job.groups.iter().filter_map(|g| if let Remote::Waiting(id) = &g.remote { Some(id.clone()) } else { None }).collect();
        if let Some(Ok(transport)) = job.provider.key(keys).map(|key| Transport::new(&job.provider, key)) {
            std::thread::spawn(move || for id in ids {
                let path = format!("{id}:cancel");
                let _ = transport.send(&path, Some(&json!({})));
            });
        }
    }

    /// What the batch does now, in one line.
    fn state(&self, job: &Job) -> String {
        let wait = self.next_check.map_or(0, |t| t.saturating_duration_since(Instant::now()).as_secs());
        if job.uncertain() {
            if self.task.is_some() { "Checking the provider to recover the submission...".into() }
            else if !self.error.is_empty() { format!("{} Next check in {wait} s.", self.error) }
            else { "Recovering the interrupted submission...".into() }
        } else if self.task.is_some() {
            if job.provider.kind == Kind::OpenAi { "Labeling the sheets one by one...".into() }
            else { format!("Contacting {}...", job.provider.name) }
        } else if !self.error.is_empty() {
            format!("{} Next try in {wait} s.", self.error)
        } else if job.done() {
            "Done.".into()
        } else {
            format!("Waiting for the provider, which can take up to 24 hours. Next check in {wait} s.")
        }
    }

    pub fn ui(&mut self, ui: &mut eframe::egui::Ui, index: &Index, ai: &crate::ai::Ai, keys: &crate::ai::Keys, single_running: bool) {
        use eframe::egui;
        let configured = ai.chosen(crate::ai::Mode::Batch);
        let ready = configured.is_some_and(|(p, _)| endpoint(p).is_ok() && p.key_source(keys) != crate::ai::KeySource::None);
        if self.running() && self.root != index.root { ui.weak(format!("For {}", self.root.display())); }
        if let Some(job) = &self.job {
            ui.weak(format!("Batch model: {} / {}", job.provider.name, job.model));
            ui.label(self.state(job));
            ui.label(job.progress());
            if self.running() { ui.ctx().request_repaint_after(Duration::from_secs(1)); }
            egui::CollapsingHeader::new("Failed sheets").show(ui, |ui| {
                for s in job.sheets.iter().filter(|s| !s.error.is_empty()) { ui.label(format!("{}: {}", s.rel, s.error)); }
            });
        } else {
            if let Some((provider, model)) = configured {
                ui.weak(format!("Batch model: {} / {}", provider.name, model.id.trim_end_matches(":batch")));
            }
            if !self.error.is_empty() { ui.colored_label(egui::Color32::LIGHT_RED, &self.error); }
        }
        if self.job.as_ref().is_some_and(Job::uncertain) {
            egui::CollapsingHeader::new("Advanced recovery").show(ui, |ui| {
                ui.weak("Find the batch in the provider's batch list and attach its ID. If there is none, send the sheets again.");
                crate::stopped(ui.text_edit_singleline(&mut self.attach_id));
                ui.horizontal(|ui| {
                    if crate::stopped(ui.button("Attach batch ID")).clicked() {
                        match validate_id(self.attach_id.trim()) {
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
            });
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
        if !ready && !self.running() { ui.weak("Set a batch model and its key in Settings."); }
        let idle = !self.busy() && !single_running && index.error.is_none() && !index.root.as_os_str().is_empty();
        for (text, scope) in [("Label the unlabeled sheets...", Scope::Unlabeled), ("Rerun all...", Scope::All)] {
            let button = ui.add_enabled(ready && idle && !index.entries.is_empty(), egui::Button::new(text));
            crate::stop(&button);
            if button.clicked() {
                let (provider, model) = configured.unwrap();
                match prepare(index, provider.clone(), model.id.clone(), scope) {
                    Ok(job) => self.proposal = Some(job),
                    Err(e) => self.error = e,
                }
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
                ui.label(format!("At most {} output tokens.", job.sheets.len() * 4096));
                match job.tag_list.is_empty() {
                    true => ui.label("Tags to look for: none."),
                    false => ui.label(format!("Tags to look for: {}.", job.tag_list.join(", "))),
                };
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
        fn prepare(&self, kind: Kind) -> Job {
            prepare(&Index::scan(&self.root, [16, 16]), provider(kind), "test:batch".into(), Scope::Unlabeled).unwrap()
        }
        fn advance(&self, job: Job, send: impl FnMut(&str, Option<&Value>) -> Result<Value, Failure>) -> (Job, Result<(), String>) {
            advance(job, &self.root, &self.spool, send)
        }
        fn journal(&self) -> Job { Job::load(&self.spool).unwrap().unwrap() }
    }
    impl Drop for Files { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.base); } }
    fn provider(kind: Kind) -> Provider {
        Provider { name: "test".into(), kind, skip: None, key_env: vec![], url: match kind {
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

    #[test]
    fn outstanding_batches_have_status_without_the_ai_pane() {
        let files = Files::new();
        let job = files.prepare(Kind::Gemini);
        let mut panel = Panel { job: Some(job), ..Panel::default() };
        assert_eq!(panel.status().as_deref(), Some("AI batch: 1 queued, 0 at provider, 0 unconfirmed, 0 labeled, 0 failed (waiting)"));
        panel.job.as_mut().unwrap().groups.push(Group { sheets: vec![0], remote: Remote::Submitting, recovery: None });
        assert!(panel.status().unwrap().contains("needs attention"));
        panel.job.as_mut().unwrap().groups.clear();
        panel.error = "Temporary failure".into();
        assert!(panel.status().unwrap().contains("retry pending"));
        let (_tx, rx) = mpsc::channel();
        panel.task = Some(rx);
        assert!(panel.status().unwrap().contains("working"));
        panel.task = None;
        panel.error.clear();
        panel.job.as_mut().unwrap().sheets[0].taken = true;
        assert!(panel.status().is_none());
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
        assert!(!files.spool.exists());
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
        let (failed, _) = files.advance(job.clone(), |_, _| Err(Failure::Status(401, String::new())));
        let mut panel = Panel { root: files.root.clone(), dir: files.spool.clone(), job: Some(failed), ..Panel::default() };
        assert!(panel.import().is_empty());
        assert_eq!(crate::sidecar::load_book(&files.root).unwrap().sheets["folder/sheet.png"].label, Some(old));
        let (job, result) = files.advance(job, |_, _| Ok(completion(labeled("New"))));
        result.unwrap();
        panel.job = Some(job);
        assert_eq!(panel.import().len(), 1);
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
        panel.job = Some(job);
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
        while !job.done() {
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
        assert!(files.journal().done());
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
            assert!(job.done() && job.sheets[0].label.is_some());
        }
        let (job, result) = files.advance(files.prepare(Kind::OpenAi), |_, _| Err(Failure::Status(401, String::new())));
        result.unwrap();
        assert!(job.done() && job.sheets[0].error.contains("401"));
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
        while !job.done() {
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
        while !job.done() {
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
        job.groups = vec![Group { sheets: vec![0], remote: Remote::Waiting("batch-1".into()), recovery: None },
            Group { sheets: vec![1], remote: Remote::Submitting, recovery: None }];
        assert!(!job.uncertain() && !job.done());
        while !job.done() {
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
    fn an_unconfirmed_submission_is_not_sent_twice() {
        let files = Files::new();
        let (_, result) = files.advance(files.prepare(Kind::Gemini), |_, _| Err(Failure::Unknown("timeout".into())));
        assert!(result.is_err());
        let mut job = files.journal();
        assert!(job.uncertain());
        let (_, result) = files.advance(job.clone(), |path, body| {
            assert_eq!(path, "batches?pageSize=100");
            assert!(body.is_none(), "Recovery must never send another paid request.");
            Ok(json!({"operations":[]}))
        });
        assert!(result.is_err());
        job.send_again();
        let (job, result) = files.advance(job, id("batches/batch-3"));
        result.unwrap();
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
        job.groups.push(Group { sheets: vec![0], remote: Remote::Waiting("batches/accepted".into()), recovery: None });
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
        let (job, result) = files.advance(files.prepare(Kind::Gemini), |_, _| Err(Failure::Status(401, String::new())));
        result.unwrap();
        assert!(job.sheets[0].error.contains("401"));
        assert!(job.done());
    }

    #[test]
    fn provider_explanations_reach_the_saved_sheet_errors() {
        for kind in [Kind::Gemini, Kind::OpenAi] {
            let files = Files::new();
            let message = "The endpoint returned HTTP 400. This model does not support the requested schema.";
            let (job, result) = files.advance(files.prepare(kind), |_, _| Err(Failure::Status(400, message.into())));
            result.unwrap();
            assert!(job.done());
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
            assert!(job.done());
            assert!(files.journal().sheets[0].error.contains("The requested image format is not supported."));
        }
    }

    #[test]
    fn results_follow_their_ids_and_missing_results_fail() {
        let files = Files::new();
        let mut job = files.prepare(Kind::Gemini);
        job.sheets.push(job.sheets[0].clone());
        job.groups.push(Group { sheets: vec![0, 1], remote: Remote::Waiting("batches/x".into()), recovery: None });
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
        job.groups = vec![Group { sheets: vec![0], remote: Remote::Waiting("batches/first".into()), recovery: None },
            Group { sheets: vec![0], remote: Remote::Waiting("batches/second".into()), recovery: None }];
        let (job, _) = files.advance(job, |path, _| { assert_eq!(path, "batches/first"); Ok(json!({"done":false})) });
        let (_, _) = files.advance(job, |path, _| { assert_eq!(path, "batches/second"); Ok(json!({"done":false})) });
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
        let request = labels::request("test", &RgbaImage::new(2, 2), &[]).unwrap();
        let gemini = gemini_request(&request);
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
}
