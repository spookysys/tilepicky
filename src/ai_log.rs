// SPDX-License-Identifier: GPL-3.0-only
//! Bounded local AI diagnostics. Keys and image data never belong in the log.

use serde_json::{Value, json};
use std::{io::Write, path::{Path, PathBuf}, sync::{Mutex, OnceLock}, time::{Instant, SystemTime, UNIX_EPOCH}};

const LIMIT: u64 = 2 * 1024 * 1024;
static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

struct Log {
    dir: PathBuf,
    secrets: Vec<String>,
    error: Option<String>,
}

pub fn init() {
    if let Some(dir) = crate::settings::dir() {
        let _ = LOG.set(Mutex::new(Log { dir: dir.join("ai-log"), secrets: vec![], error: None }));
        event("session", json!({"version":env!("CARGO_PKG_VERSION"), "pid":std::process::id()}));
    }
}

pub fn secret(key: &str) {
    if key.is_empty() { return; }
    if let Some(log) = LOG.get() && let Ok(mut log) = log.lock() && !log.secrets.iter().any(|s| s == key) {
        log.secrets.push(key.into());
    }
}

pub fn event(kind: &str, data: Value) {
    if let Some(log) = LOG.get() && let Ok(mut log) = log.lock() {
        let value = json!({"time_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
            "event":kind, "data":data});
        if let Err(error) = log.append(value, LIMIT) { log.error = Some(error.to_string()); }
    }
}

pub fn text() -> Result<String, String> {
    let mut log = LOG.get().ok_or("The AI log is unavailable.")?.lock().map_err(|_| "The AI log is unavailable.")?;
    log.read().map_err(|e| format!("Could not read the AI log: {e}"))
}

/// Pairs each HTTP result with its request, including concurrent single and batch work.
pub struct Request { id: String, started: Instant }
impl Request {
    pub fn start(url: &str, body: Option<&Value>, key: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        secret(key);
        let id = format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        event("http_request", json!({"id":id, "url":url, "body":body}));
        Self { id, started: Instant::now() }
    }
    pub fn finish(&self, status: Option<u16>, result: &Result<Value, String>) {
        event("http_result", json!({"id":self.id, "elapsed_ms":self.started.elapsed().as_millis(),
            "http_status":status, "result":result}));
    }
}

fn scrub(value: Value, secrets: &[String], depth: usize) -> Value {
    if depth > 24 { return json!("[depth limit]"); }
    match value {
        Value::Object(map) => Value::Object(map.into_iter().map(|(mut key, value)| {
            let hidden = matches!(key.to_ascii_lowercase().as_str(), "authorization" | "x-goog-api-key" | "api_key" | "apikey"
                | "key" | "inline_data" | "inlinedata" | "image_url" | "thoughtsignature");
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

impl Log {
    fn append(&mut self, value: Value, limit: u64) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join("current.jsonl");
        let mut bytes = serde_json::to_vec(&scrub(value, &self.secrets, 0))?;
        if bytes.len() as u64 > limit {
            bytes = serde_json::to_vec(&json!({"event":"entry_omitted", "reason":"The entry exceeded the log size limit."}))?;
        }
        bytes.push(b'\n');
        if std::fs::metadata(&path).is_ok_and(|m| m.len() + bytes.len() as u64 > limit) {
            let previous = self.dir.join("previous.jsonl");
            if previous.exists() { std::fs::remove_file(&previous)?; }
            std::fs::rename(&path, previous)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(&bytes)?;
        file.flush()
    }

    fn read(&mut self) -> std::io::Result<String> {
        let mut out = String::from("Tilepicky AI debug log\nContains sheet names, prompts, model responses, and parsed tags.\n\n");
        if let Some(error) = &self.error { out += &format!("Log write error: {error}\n"); }
        for name in ["previous.jsonl", "current.jsonl"] {
            let path = self.dir.join(name);
            match read_bounded(&path) {
                Ok(text) => out += &text,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }
}

fn read_bounded(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)?.take(LIMIT + 1).read_to_string(&mut text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logs_keep_tag_evidence_without_keys_or_images() {
        let dir = crate::storage::tests::Folder::new();
        let mut log = Log { dir: dir.0.clone(), secrets: vec!["test-secret".into()], error: None };
        log.append(json!({"api_key":"test-secret", "inlineData":{"data":"image-bytes"},
            "content":"{\"tags\":[\"crystal\"],\"listed\":{\"weapon\":false},\"message\":\"test-secret\"}",
            "image_url":{"url":"data:image/png;base64,hidden"}}), LIMIT).unwrap();
        let text = log.read().unwrap();
        for hidden in ["test-secret", "image-bytes", "base64,hidden"] { assert!(!text.contains(hidden)); }
        assert!(text.contains("crystal") && text.contains("weapon") && text.contains("false"));
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.0.join("current.jsonl")).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn copied_logs_keep_raw_and_saved_tags_for_comparison() {
        let dir = crate::storage::tests::Folder::new();
        let mut log = Log { dir: dir.0.clone(), secrets: vec![], error: None };
        let list = vec!["weapon".into(), "indoor".into()];
        let raw = crate::labels::tests::completion(json!({"status":"labeled", "caption":"Crystals", "tags":["Purple", "purple"],
            "listed":{"weapon":false,"indoor":true}}));
        let label = crate::labels::response(&raw, &list).unwrap().into_label("test", "test", &list);
        log.append(json!({"tags_requested":list, "raw_response":raw, "saved":label}), LIMIT).unwrap();
        let text = log.read().unwrap();
        let entry: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(entry["saved"]["tags"], json!(["indoor", "purple"]));
        let model: Value = serde_json::from_str(entry["raw_response"]["choices"][0]["message"]["content"].as_str().unwrap()).unwrap();
        assert_eq!(model["tags"], json!(["Purple", "purple"]));
        assert_eq!(model["listed"], json!({"weapon":false,"indoor":true}));
    }

    #[test]
    fn logs_rotate_and_survive_a_restart() {
        let dir = crate::storage::tests::Folder::new();
        let mut log = Log { dir: dir.0.clone(), secrets: vec![], error: None };
        for n in 0..20 { log.append(json!({"sequence":n,"text":"x".repeat(40)}), 180).unwrap(); }
        let mut reopened = Log { dir: dir.0.clone(), secrets: vec![], error: None };
        let text = reopened.read().unwrap();
        assert!(text.contains("\"sequence\":19"));
        assert!(!text.contains("\"sequence\":0,"));
        for name in ["current.jsonl", "previous.jsonl"] { assert!(std::fs::metadata(dir.0.join(name)).unwrap().len() <= 180); }
    }
}
