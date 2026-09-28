// SPDX-License-Identifier: GPL-3.0-only
//! One current job log per library. Finished logs are removed on exit.

use serde_json::{Value, json};
use std::{cell::RefCell, collections::VecDeque, io::Write, path::{Path, PathBuf},
    sync::{Arc, Mutex}, time::{Instant, SystemTime, UNIX_EPOCH}};

const LIMIT: usize = 2 * 1024 * 1024;
const JOB_FILE: &str = ".tilepicky-ai-log.jsonl";
thread_local! { static CURRENT: RefCell<Option<Log>> = const { RefCell::new(None) }; }

#[derive(Clone)]
pub struct Log(Arc<Mutex<State>>);
struct State {
    path: Option<PathBuf>,
    id: String,
    title: String,
    entries: VecDeque<String>,
    bytes: usize,
    truncated: bool,
    loaded: bool,
    done: bool,
    sheet: Option<String>,
    retired: bool,
    secrets: Vec<String>,
    error: Option<String>,
}

/// Old versions mixed all jobs in these files. They cannot become a log for one job.
pub fn init() {
    if let Some(dir) = crate::settings::dir() {
        for name in ["current.jsonl", "previous.jsonl"] {
            if let Err(error) = std::fs::remove_file(dir.join("ai-log").join(name))
                && error.kind() != std::io::ErrorKind::NotFound { eprintln!("Could not remove the old shared AI log: {error}"); }
        }
    }
}

pub fn id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!("{}-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

impl Log {
    #[cfg(test)]
    pub fn single(sheet: &str) -> Self { Self::new(None, id(), format!("Single-sheet label: {sheet}")) }
    pub fn single_file(root: &Path, sheet: &str) -> Self {
        let log = Self::new(Some(root.join(JOB_FILE)), id(), format!("Single-sheet label: {sheet}"));
        log.0.lock().unwrap().sheet = Some(sheet.into());
        log
    }
    pub fn reopen_single(root: &Path) -> Option<(PathBuf, Self)> {
        let header = read_header(&root.join(JOB_FILE)).ok()?;
        let sheet = header["sheet"].as_str()?;
        let log = Self::new(Some(root.join(JOB_FILE)), header["job"].as_str()?.into(), header["title"].as_str()?.into());
        if header["done"] != true {
            log.event("single_interrupted", json!({"sheet":sheet,"message":"The app closed before this request finished. It was not resent."}));
            log.complete();
        }
        Some((root.join(sheet), log))
    }
    pub fn complete(&self) { self.set_done(true); }
    pub fn resume(&self) { self.set_done(false); }
    fn set_done(&self, done: bool) {
        if let Ok(mut state) = self.0.lock() {
            if state.path.is_some() && !state.loaded && let Err(error) = state.load() { state.error = Some(error.to_string()); return; }
            state.done = done;
            if let Err(error) = state.persist() { state.error = Some(error.to_string()); }
        }
    }
    /// A replaced request can still finish in its worker. Its late replies must not replace the new job's file.
    pub fn retire(&self) { if let Ok(mut state) = self.0.lock() { state.retired = true; } }
    pub fn batch(root: &Path, id: &str) -> Self {
        Self::new(Some(root.join(JOB_FILE)), if id.is_empty() { "legacy".into() } else { id.into() }, "Library labeling job".into())
    }
    fn new(path: Option<PathBuf>, id: String, title: String) -> Self {
        Self(Arc::new(Mutex::new(State { path, id, title, entries: VecDeque::new(), bytes: 0,
            truncated: false, loaded: false, done: false, sheet: None, retired: false, secrets: vec![], error: None })))
    }
    /// The scope follows this worker only. Concurrent jobs never share a destination.
    pub fn enter(&self) -> Scope {
        Scope(CURRENT.with(|current| current.replace(Some(self.clone()))))
    }
    pub fn event(&self, kind: &str, data: Value) {
        if let Ok(mut state) = self.0.lock() {
            let value = json!({"time_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
                "event":kind,"data":data});
            if let Err(error) = state.append(value, LIMIT) { state.error = Some(error.to_string()); }
        }
    }
    pub fn available(&self) -> bool {
        let Ok(state) = self.0.lock() else { return false };
        match &state.path {
            Some(path) => state.matches_file(path),
            None => !state.entries.is_empty() || state.error.is_some(),
        }
    }
    pub fn text(&self) -> Result<String, String> {
        let mut state = self.0.lock().map_err(|_| "The job log is unavailable.")?;
        if state.path.is_some() { state.load().map_err(|e| format!("Could not read the job log: {e}"))?; }
        if state.entries.is_empty() && state.error.is_none() { return Err("No log is available for this job.".into()); }
        let mut out = format!("Tilepicky AI debug log\n{}\nJob: {}\nKeys and image data are omitted.\n\n", state.title, state.id);
        if state.truncated { out += "Earlier entries were discarded because this job reached the log size limit.\n"; }
        if let Some(error) = &state.error { out += &format!("Log write error: {error}\n"); }
        for entry in &state.entries { out += entry; }
        Ok(out)
    }
}

fn read_header(path: &Path) -> std::io::Result<Value> {
    use std::io::{BufRead, Read};
    let mut line = String::new();
    std::io::BufReader::new(std::fs::File::open(path)?.take(4096)).read_line(&mut line)?;
    serde_json::from_str(&line).map_err(std::io::Error::other)
}

pub fn cleanup_finished(root: &Path) -> std::io::Result<()> {
    let path = root.join(JOB_FILE);
    match read_header(&path) {
        Ok(header) if header["done"] == true => std::fs::remove_file(path),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

pub struct Scope(Option<Log>);
impl Drop for Scope { fn drop(&mut self) { CURRENT.with(|current| current.replace(self.0.take())); } }

pub fn current() -> Option<Log> { CURRENT.with(|current| current.borrow().clone()) }

pub fn event(kind: &str, data: Value) {
    if let Some(log) = CURRENT.with(|current| current.borrow().clone()) { log.event(kind, data); }
}

/// A request retains its own log even if the current scope changes before its reply arrives.
pub struct Request { id: String, started: Instant, log: Option<Log> }
impl Request {
    pub fn start(url: &str, body: Option<&Value>, key: &str) -> Self {
        let log = CURRENT.with(|current| current.borrow().clone());
        if let Some(log) = &log && !key.is_empty() && let Ok(mut state) = log.0.lock()
            && !state.secrets.iter().any(|s| s == key) { state.secrets.push(key.into()); }
        let id = id();
        if let Some(log) = &log { log.event("http_request", json!({"id":id,"url":url,"body":body})); }
        Self { id, started: Instant::now(), log }
    }
    pub fn finish(&self, status: Option<u16>, result: &Result<Value, String>) {
        if let Some(log) = &self.log {
            log.event("http_result", json!({"id":self.id,"elapsed_ms":self.started.elapsed().as_millis(),
                "http_status":status,"result":result}));
        }
    }
}

fn scrub(value: Value, secrets: &[String], depth: usize) -> Value {
    if depth > 24 { return json!("[depth limit]"); }
    match value {
        Value::Object(map) => Value::Object(map.into_iter().map(|(mut key, value)| {
            let hidden = matches!(key.to_ascii_lowercase().as_str(), "authorization" | "x-goog-api-key" | "api_key" | "apikey"
                | "inline_data" | "inlinedata" | "image_url" | "thoughtsignature");
            for secret in secrets { key = key.replace(secret, "[redacted]"); }
            (key, if hidden { json!("[omitted]") } else { scrub(value, secrets, depth + 1) })
        }).collect()),
        Value::Array(values) => Value::Array(values.into_iter().take(256).map(|v| scrub(v, secrets, depth + 1)).collect()),
        Value::String(mut text) => {
            for secret in secrets { text = text.replace(secret, "[redacted]"); }
            if text.contains("data:image/") { return json!("[image data omitted]"); }
            if let Ok(nested @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str::<Value>(&text) {
                return json!(scrub(nested, secrets, depth + 1).to_string());
            }
            if text.chars().count() > 16_384 { text = text.chars().take(16_384).collect::<String>() + " [truncated]"; }
            json!(text)
        }
        other => other,
    }
}

impl State {
    fn header(&self) -> String {
        json!({"job":self.id,"title":self.title,"sheet":self.sheet,"done":self.done,"truncated":self.truncated}).to_string() + "\n"
    }
    fn matches_file(&self, path: &Path) -> bool { read_header(path).is_ok_and(|header| header["job"] == self.id) }
    fn persist(&self) -> std::io::Result<()> {
        if self.retired { return Ok(()); }
        if let Some(path) = &self.path {
            let mut bytes = self.header().into_bytes();
            for line in &self.entries { bytes.extend_from_slice(line.as_bytes()); }
            crate::storage::replace(path, true, |file| file.write_all(&bytes))?;
        }
        Ok(())
    }
    fn load(&mut self) -> std::io::Result<()> {
        use std::io::Read;
        let path = self.path.as_ref().unwrap();
        self.entries.clear(); self.bytes = 0; self.truncated = false; self.loaded = true;
        if !self.matches_file(path) { return Ok(()); }
        let mut text = String::new();
        std::fs::File::open(path)?.take((LIMIT + 4096) as u64).read_to_string(&mut text)?;
        let mut lines = text.lines();
        if let Some(header) = lines.next().and_then(|l| serde_json::from_str::<Value>(l).ok()) {
            self.truncated = header["truncated"] == true;
            self.done = header["done"] == true;
            self.sheet = header["sheet"].as_str().map(str::to_string);
        }
        for line in lines { self.entries.push_back(line.to_string() + "\n"); self.bytes += line.len() + 1; }
        Ok(())
    }
    fn append(&mut self, value: Value, limit: usize) -> std::io::Result<()> {
        if self.retired { return Ok(()); }
        // The UI and worker share this handle. Read existing entries once when a job resumes.
        if self.path.is_some() && !self.loaded { self.load()?; }
        let mut entry = serde_json::to_string(&scrub(value, &self.secrets, 0))? + "\n";
        if entry.len() > limit { entry = json!({"event":"entry_omitted","reason":"Entry exceeded the log size limit."}).to_string() + "\n"; }
        let mut rewrite = false;
        while self.bytes + entry.len() > limit {
            let Some(old) = self.entries.pop_front() else { break };
            self.bytes -= old.len(); self.truncated = true; rewrite = true;
        }
        self.bytes += entry.len(); self.entries.push_back(entry.clone());
        if let Some(path) = &self.path {
            if rewrite || !self.matches_file(path) {
                self.persist()?;
            } else {
                let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
                file.write_all(entry.as_bytes())?; file.flush()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_job_types_keep_unfinished_logs_and_remove_finished_logs_on_exit() {
        for single in [false, true] {
            let folder = crate::storage::tests::Folder::new();
            let log = if single { Log::single_file(&folder.0, "sheet.png") } else { Log::batch(&folder.0, "batch") };
            log.event("started", json!({})); cleanup_finished(&folder.0).unwrap(); assert!(log.available());
            if single { assert!(Log::reopen_single(&folder.0).unwrap().1.text().unwrap().contains("started")); }
            log.complete(); assert!(log.available());
            cleanup_finished(&folder.0).unwrap(); assert!(!log.available());
        }
    }

    #[test]
    fn an_interrupted_single_request_is_diagnosable_until_the_next_exit() {
        let folder = crate::storage::tests::Folder::new();
        let log = Log::single_file(&folder.0, "sheet.png"); log.event("single_start", json!({})); drop(log);
        cleanup_finished(&folder.0).unwrap();
        let (path, log) = Log::reopen_single(&folder.0).unwrap();
        assert_eq!(path, folder.0.join("sheet.png")); assert!(log.text().unwrap().contains("single_interrupted"));
        cleanup_finished(&folder.0).unwrap(); assert!(!log.available());
    }

    #[test]
    fn a_single_request_and_a_batch_replace_the_same_library_file() {
        let folder = crate::storage::tests::Folder::new();
        let single = Log::single_file(&folder.0, "sheet.png"); single.event("single_only", json!({})); single.complete(); single.retire();
        let batch = Log::batch(&folder.0, "batch"); batch.event("batch_only", json!({}));
        single.event("late_single_result", json!({}));
        assert!(!single.available()); assert!(Log::reopen_single(&folder.0).is_none());
        assert!(!batch.text().unwrap().contains("single_only")); assert!(!batch.text().unwrap().contains("late_single_result"));
        batch.complete(); batch.resume(); cleanup_finished(&folder.0).unwrap(); assert!(batch.available());
    }

    #[test]
    fn concurrent_jobs_and_delayed_replies_keep_separate_logs() {
        let single = Log::single("sheet.png");
        let folder = crate::storage::tests::Folder::new();
        let batch = Log::batch(&folder.0, "batch-one");
        std::thread::scope(|threads| {
            for (log, marker) in [(single.clone(), "single-only"), (batch.clone(), "batch-only")] {
                threads.spawn(move || {
                    let scope = log.enter();
                    let request = Request::start("https://example.invalid", Some(&json!({"marker":marker})), "secret-value");
                    drop(scope);
                    request.finish(Some(200), &Ok(json!({"marker":marker,"key_echo":"secret-value"})));
                });
            }
        });
        let one = single.text().unwrap(); let many = batch.text().unwrap();
        assert!(one.contains("single-only") && !one.contains("batch-only"));
        assert!(many.contains("batch-only") && !many.contains("single-only"));
        assert!(!one.contains("secret-value") && !many.contains("secret-value"));
    }

    #[test]
    fn batch_logs_survive_restart_and_new_jobs_replace_them() {
        let folder = crate::storage::tests::Folder::new();
        let old = Log::batch(&folder.0, "old"); assert!(!old.available());
        old.event("old_batch", json!({})); drop(old);
        let reopened = Log::batch(&folder.0, "old"); assert!(reopened.available());
        reopened.event("resumed", json!({})); assert!(reopened.text().unwrap().contains("old_batch"));
        let next = Log::batch(&folder.0, "new");
        assert!(!next.available()); assert!(next.text().is_err());
        next.event("new_batch", json!({}));
        assert!(!next.text().unwrap().contains("old_batch"));
        assert!(!reopened.available()); assert!(reopened.text().is_err());
        std::fs::remove_file(folder.0.join(JOB_FILE)).unwrap();
        assert!(!next.available()); assert!(next.text().is_err());
    }

    #[test]
    fn single_logs_are_ephemeral_and_late_cancelled_work_cannot_change_a_retry() {
        let old = Log::single("sheet.png"); old.event("cancelled", json!({}));
        let retry = Log::single("sheet.png"); assert!(!retry.available());
        old.event("late_reply", json!({}));
        retry.event("new_request", json!({}));
        assert!(!retry.text().unwrap().contains("late_reply"));
        drop(retry);
        assert!(!Log::single("sheet.png").available());
    }

    #[test]
    fn logs_keep_tag_evidence_without_keys_or_images() {
        let folder = crate::storage::tests::Folder::new(); let log = Log::batch(&folder.0, "redaction");
        let _scope = log.enter();
        let trace = Request::start("https://example.invalid", Some(&json!({"api_key":"test-secret", "inlineData":{"data":"image-bytes"},
            "content":"{\"tags\":[\"crystal\"],\"listed\":{\"weapon\":false},\"message\":\"test-secret\"}",
            "image_url":{"url":"data:image/png;base64,hidden"},"listed":{"key":true}})), "test-secret");
        trace.finish(Some(200), &Ok(json!({"tags":["purple","indoor"]})));
        let text = log.text().unwrap();
        for hidden in ["test-secret", "image-bytes", "base64,hidden"] { assert!(!text.contains(hidden)); }
        for kept in ["crystal", "weapon", "false", "purple", "indoor", "\"key\":true"] { assert!(text.contains(kept)); }
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(folder.0.join(JOB_FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn bounded_logs_report_discarded_entries_and_keep_the_latest_error() {
        let folder = crate::storage::tests::Folder::new(); let log = Log::batch(&folder.0, "bounded");
        for n in 0..20 { log.0.lock().unwrap().append(json!({"sequence":n,"text":"x".repeat(40)}), 180).unwrap(); }
        let reopened = Log::batch(&folder.0, "bounded"); let text = reopened.text().unwrap();
        assert!(text.contains("\"sequence\":19")); assert!(!text.contains("\"sequence\":0,"));
        assert!(text.contains("Earlier entries were discarded"));
        assert!(std::fs::metadata(folder.0.join(JOB_FILE)).unwrap().len() < 400);
    }
}
