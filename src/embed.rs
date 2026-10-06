// SPDX-License-Identifier: GPL-3.0-only
//! Independent image and label embeddings for semantic search.
//! Each source has its own file, model identity, and source hashes.
//! The selected model embeds the query as text in the same vector space.

use crate::ai::{Kind, Provider};
use crate::sidecar::Book;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The file that holds the vectors, at the top of the library.
pub const FILE: &str = "embeddings.json";
/// The least cosine similarity a sheet needs to match a query.
pub const FLOOR: f32 = 0.3;
/// How many sheets one request carries. Images make each request large.
const BATCH: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputMode {
    #[default]
    Image,
    Text,
}

impl InputMode {
    fn file(self) -> &'static str {
        match self { Self::Image => FILE, Self::Text => "embeddings-text.json" }
    }
}

/// The vectors of one library, and the model that made them.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Embeddings {
    /// The embedding model. Vectors from another model do not match it.
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub labels: bool,
    #[serde(default)]
    pub sheets: BTreeMap<String, Vector>,
}

/// One sheet's vector and the hash of its source content.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Vector {
    pub hash: String,
    pub vec: Vec<f32>,
}

/// Reads the vectors of a library. A missing file is an empty set.
pub fn read(root: &Path) -> Result<Embeddings, String> {
    read_mode(root, InputMode::Image)
}

pub fn read_mode(root: &Path, mode: InputMode) -> Result<Embeddings, String> {
    let data: Embeddings = crate::storage::read(&root.join(mode.file()))?;
    let mut dimensions = None;
    for vector in data.sheets.values() {
        validate_vector(&vector.vec, &mut dimensions)?;
    }
    Ok(data)
}

/// Writes the vectors of a library, replacing the file.
pub fn write(root: &Path, embeddings: &Embeddings, mode: InputMode) -> Result<(), String> {
    crate::storage::write(&root.join(mode.file()), embeddings)
}

/// The cosine similarity of two vectors. Zero when either is empty.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    if a.iter().chain(b).any(|n| !n.is_finite()) { return 0.0; }
    let dot: f64 = a.iter().zip(b).map(|(&x, &y)| f64::from(x) * f64::from(y)).sum();
    let norm = |v: &[f32]| v.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>().sqrt();
    let denominator = norm(a) * norm(b);
    if denominator == 0.0 { 0.0 } else { (dot / denominator).clamp(-1.0, 1.0) as f32 }
}

/// One sheet that needs a vector: its path, the request input part, and the
/// hash of the image it was made from.
pub struct Job {
    pub rel: String,
    pub input: Value,
    pub hash: String,
}

/// The caption and tags of a sheet's label, when it has one.
fn label_text(book: &Book, rel: &str) -> Option<String> {
    label_text_side(book.sheets.get(rel)?)
}

fn label_text_side(side: &crate::sidecar::Sidecar) -> Option<String> {
    let label = side.label.as_ref()?;
    let mut text = label.caption.trim().to_string();
    for tag in &label.tags {
        let tag = tag.trim();
        if tag.is_empty() { continue; }
        if !text.is_empty() { text.push(' '); }
        text.push_str(tag);
    }
    (!text.is_empty()).then_some(text)
}

/// The hash of what a vector is made from: the image, and the label text when
/// it is included, so that either change re-embeds the sheet.
fn content_hash(bytes: &[u8], text: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    if let Some(text) = text { hasher.update(text.as_bytes()); }
    format!("{:x}", hasher.finalize())
}

/// The sheets to embed: every sheet whose image, or its included label, changed,
/// or whose vector is missing, when the model matches. A new model re-embeds all.
pub fn jobs(root: &Path, book: &Book, rels: &[String], done: &Embeddings, model: &str, labels: bool, mode: InputMode) -> Result<Vec<Job>, String> {
    let labels = mode == InputMode::Image && labels;
    rels.iter().filter_map(|rel| match job(root, book, rel, done, model, labels, mode) {
        Ok(Some(job)) => Some(Ok(job)), Ok(None) => None, Err(error) => Some(Err(error)),
    }).collect()
}

fn job(root: &Path, book: &Book, rel: &str, done: &Embeddings, model: &str, labels: bool, mode: InputMode) -> Result<Option<Job>, String> {
    let Some(Source { bytes, text, hash }) = source(root, book, rel, labels, mode)? else { return Ok(None) };
    if done.model == model && done.labels == labels && done.sheets.get(rel).is_some_and(|v| v.hash == hash) { return Ok(None); }
    if mode == InputMode::Text { return Ok(Some(Job { rel: rel.into(), input: json!(text.unwrap()), hash })); }
    let url = crate::labels::embed_image(&bytes).map_err(|e| format!("Cannot embed {rel}: {e}"))?;
    let mut content = Vec::new();
    if let Some(text) = text { content.push(json!({"type":"text", "text":text})); }
    content.push(json!({"type":"image_url", "image_url":{"url":url}}));
    Ok(Some(Job { rel: rel.into(), input: json!({"content":content}), hash }))
}

struct Source { bytes: Vec<u8>, text: Option<String>, hash: String }

fn source(root: &Path, book: &Book, rel: &str, labels: bool, mode: InputMode) -> Result<Option<Source>, String> {
    if mode == InputMode::Text {
        let Some(text) = label_text(book, rel) else { return Ok(None) };
        let hash = content_hash(b"text\0", Some(&text));
        return Ok(Some(Source { bytes: Vec::new(), text: Some(text), hash }));
    }
    let bytes = std::fs::read(root.join(rel)).map_err(|e| format!("Cannot read {rel}: {e}"))?;
    let text = if labels { label_text(book, rel) } else { None };
    let hash = content_hash(&bytes, text.as_deref());
    Ok(Some(Source { bytes, text, hash }))
}

/// Returns a vector only while its source still matches the saved hash.
pub fn current_vector(root: &Path, rel: &str, side: &crate::sidecar::Sidecar, data: &Embeddings, mode: InputMode) -> Option<Vec<f32>> {
    let vector = data.sheets.get(rel)?;
    let text = label_text_side(side);
    let hash = match mode {
        InputMode::Text => content_hash(b"text\0", Some(text.as_deref()?)),
        InputMode::Image => content_hash(&std::fs::read(root.join(rel)).ok()?, if data.labels { text.as_deref() } else { None }),
    };
    (vector.hash == hash).then(|| vector.vec.clone())
}

/// Identifies the endpoint without storing its URL or credentials in the library.
pub fn identity(provider: &Provider) -> String {
    content_hash(provider.url.trim().trim_end_matches('/').trim_end_matches("/embeddings").as_bytes(), Some("openai"))
}

/// The request body for one batch of inputs. A text query is a plain string;
/// a sheet is a content part that carries its image, and its label when asked.
pub fn request(model: &str, inputs: &[Value]) -> Value {
    json!({"model": model, "input": inputs, "encoding_format": "float"})
}

/// How many sheets still need a vector. It reads the images but does not
/// decode them, so a plan can be counted without the work of a run.
pub fn pending(root: &Path, book: &Book, rels: &[String], done: &Embeddings, model: &str, labels: bool, mode: InputMode) -> Result<usize, String> {
    let labels = mode == InputMode::Image && labels;
    let mut count = 0;
    for rel in rels {
        let Some(Source { hash, .. }) = source(root, book, rel, labels, mode)? else { continue };
        if !(done.model == model && done.labels == labels && done.sheets.get(rel).is_some_and(|v| v.hash == hash)) { count += 1; }
    }
    Ok(count)
}

fn validate_vector(vector: &[f32], dimensions: &mut Option<usize>) -> Result<(), String> {
    if vector.is_empty() || vector.iter().any(|n| !n.is_finite()) || vector.iter().all(|n| *n == 0.0) {
        return Err("The endpoint returned an empty, zero, or invalid embedding.".into());
    }
    if dimensions.is_some_and(|n| n != vector.len()) {
        return Err("The embedding dimensions do not match.".into());
    }
    *dimensions = Some(vector.len());
    Ok(())
}

/// The vectors of a reply, in the order of the input. An endpoint that drops
/// the index falls back to the order it returned.
pub fn response(value: &Value, count: usize) -> Result<Vec<Vec<f32>>, String> {
    if let Some(error) = value.get("error").filter(|e| !e.is_null()) {
        return Err(format!("The endpoint rejected the request. {}", crate::labels::error_detail(error, "")).trim_end().into());
    }
    let data = value.get("data").and_then(Value::as_array).ok_or("The embedding endpoint returned no data.")?;
    if data.len() != count { return Err("The endpoint returned the wrong number of embeddings.".into()); }
    let mut out: Vec<Option<Vec<f32>>> = vec![None; count];
    let mut dimensions = None;
    let indexed = data.iter().any(|item| item.get("index").is_some());
    for (fallback, item) in data.iter().enumerate() {
        let i = if indexed {
            item.get("index").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok())
                .ok_or("An embedding index is invalid or missing.")?
        } else { fallback };
        if i >= count || out[i].is_some() { return Err("An embedding index is duplicate or out of range.".into()); }
        let vector = item.get("embedding").and_then(Value::as_array).ok_or("An embedding is missing.")?
            .iter().map(|n| n.as_f64().map(|n| n as f32).ok_or("An embedding contains a nonnumeric value."))
            .collect::<Result<Vec<_>, _>>()?;
        validate_vector(&vector, &mut dimensions)?;
        out[i] = Some(vector);
    }
    out.into_iter().enumerate().map(|(i, v)| v.ok_or_else(|| format!("The endpoint returned no vector for input {i}."))).collect()
}

/// An endpoint that serves embeddings: an OpenAI-style provider's
/// `/embeddings` path. Only such a provider serves one.
pub struct Endpoint {
    client: ureq::Agent,
    url: String,
    key: String,
}

impl Endpoint {
    pub fn new(provider: &Provider, key: String) -> Result<Self, String> {
        if provider.kind != Kind::OpenAi {
            return Err("Embeddings need an OpenAI-style endpoint, such as OpenRouter.".into());
        }
        let base = crate::labels::checked_url(&provider.url)?;
        let url = if base.ends_with("/embeddings") { base } else { format!("{base}/embeddings") };
        Ok(Self { client: crate::labels::agent(), url, key })
    }

    pub fn send(&self, body: &Value) -> Result<Value, String> {
        let trace = crate::ai_log::Request::start(&self.url, Some(body), &self.key);
        let mut status = None;
        let result = (|| {
            let mut response = self.client.post(&self.url)
                .header("Authorization", format!("Bearer {}", self.key)).send_json(body)
                .map_err(|e| match e {
                    ureq::Error::Timeout(_) => "The embedding request timed out.",
                    _ => "Could not reach the embedding endpoint.",
                })?;
            status = Some(response.status().as_u16());
            if !response.status().is_success() { return Err(crate::labels::http_error(&mut response, &self.key)); }
            let bytes = response.body_mut().with_config().limit(8_388_608).read_to_vec().map_err(|e| match e {
                ureq::Error::Timeout(_) => "The embedding request timed out while reading the response.",
                _ => "Could not read the embedding response.",
            })?;
            serde_json::from_slice(&bytes).map_err(|_| "The embedding endpoint returned invalid JSON.".into())
        })();
        trace.finish(status, &result);
        result
    }
}

/// One generation over a library, run on a worker thread. It sends the
/// labels that need a vector, writes the file, and answers with how many
/// sheets it embedded.
pub struct Run {
    pub root: PathBuf,
    pub mode: InputMode,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub result: std::sync::mpsc::Receiver<Result<usize, String>>,
    /// How many sheets the plan counted.
    pub total: usize,
}

/// What a run needs: the library, the model, and what to embed.
pub struct Plan {
    pub mode: InputMode,
    pub provider: String,
    pub root: PathBuf,
    pub model: String,
    /// How many sheets the run will embed, for the message before it starts.
    pub total: usize,
    pub book: Book,
    pub rels: Vec<String>,
    pub labels: bool,
    pub done: Embeddings,
}

impl Run {
    pub fn start(
        plan: Plan,
        send: impl Fn(&Value) -> Result<Value, String> + Send + 'static,
        wake: impl FnOnce() + Send + 'static,
    ) -> Result<Self, String> {
        let Plan { root, model, provider, total, book, rels, labels, done, mode } = plan;
        let labels = mode == InputMode::Image && labels;
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancelled = cancel.clone();
        let run_root = root.clone();
        let (tx, result) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("generate embeddings".into()).spawn(move || {
            let outcome = (|| {
                let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
                    .open(root.join(".tilepicky-embeddings.lock")).map_err(|e| e.to_string())?;
                lock.try_lock().map_err(|_| "Another embedding run is active for this library.".to_string())?;
                if read_mode(&root, mode)? != done { return Err("The embeddings changed. Start the operation again.".into()); }
                let mut current = if done.model == model && done.provider == provider && done.labels == labels { done } else { Embeddings::default() };
                current.model = model.clone();
                current.provider = provider;
                current.labels = labels;
                let mut count = 0;
                for paths in rels.chunks(BATCH) {
                    if cancelled.load(std::sync::atomic::Ordering::Relaxed) { return Err("Embedding generation was cancelled.".into()); }
                    let batch = jobs(&root, &book, paths, &current, &model, labels, mode)?;
                    let checked_send = |body: &Value| {
                        verify_sources(&root, &batch, labels, mode)?;
                        let response = send(body)?;
                        verify_sources(&root, &batch, labels, mode)?;
                        Ok(response)
                    };
                    count += embed_all(&root, &model, &batch, &mut current, mode, &cancelled, &checked_send)?;
                }
                Ok(count)
            })();
            let _ = tx.send(outcome);
            wake();
        }).map_err(|_| "Could not start the embeddings thread.")?;
        Ok(Self { result, total, root: run_root, mode, cancel })
    }
}

fn verify_sources(root: &Path, batch: &[Job], labels: bool, mode: InputMode) -> Result<(), String> {
    let book = if labels || mode == InputMode::Text { crate::sidecar::load_book(root)? } else { Book::default() };
    for job in batch {
        if source(root, &book, &job.rel, labels, mode)?.is_none_or(|source| source.hash != job.hash) {
            return Err(format!("{} changed during embedding generation. Start the operation again.", job.rel));
        }
    }
    Ok(())
}

impl Drop for Run {
    fn drop(&mut self) { self.cancel.store(true, std::sync::atomic::Ordering::Relaxed); }
}

/// Saves each successful batch. A failed batch preserves earlier results.
fn embed_all(root: &Path, model: &str, jobs: &[Job], done: &mut Embeddings, mode: InputMode,
    cancel: &std::sync::atomic::AtomicBool, send: &impl Fn(&Value) -> Result<Value, String>) -> Result<usize, String> {
    let mut dimensions = done.sheets.values().next().map(|v| v.vec.len());
    for batch in jobs.chunks(BATCH) {
        let inputs: Vec<Value> = batch.iter().map(|j| j.input.clone()).collect();
        let value = send(&request(model, &inputs))?;
        let vectors = response(&value, inputs.len())?;
        for vector in &vectors { validate_vector(vector, &mut dimensions)?; }
        for (job, vec) in batch.iter().zip(vectors) {
            done.sheets.insert(job.rel.clone(), Vector { hash: job.hash.clone(), vec });
        }
    }
    if cancel.load(std::sync::atomic::Ordering::Relaxed) { return Err("Embedding generation was cancelled.".into()); }
    if !jobs.is_empty() { write(root, done, mode)?; }
    Ok(jobs.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    use std::time::Duration;

    fn folder_with(names: &[&str]) -> crate::storage::tests::Folder {
        let folder = crate::storage::tests::Folder::new();
        for (i, name) in names.iter().enumerate() {
            RgbaImage::from_pixel(4, 4, Rgba([i as u8, 2, 3, 255])).save(folder.0.join(name)).unwrap();
        }
        folder
    }

    fn reply(vectors: &[Vec<f32>]) -> Value {
        json!({"data": vectors.iter().enumerate().map(|(i, v)| json!({"index": i, "embedding": v})).collect::<Vec<_>>()})
    }

    #[test]
    fn cosine_is_one_for_the_same_direction() {
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0, "different lengths do not match");
        assert_eq!(cosine(&[], &[]), 0.0);
    }

    /// A sheet with an unchanged image is not embedded again; a new model
    /// re-embeds every sheet; including a label
    /// changes the hash and so re-embeds the sheet.
    #[test]
    fn jobs_skip_what_is_already_embedded() {
        use crate::sidecar::{Label, Sidecar, Status};
        let folder = folder_with(&["a.png", "b.png"]);
        let rels: Vec<String> = ["a.png", "b.png"].iter().map(|s| s.to_string()).collect();
        let empty = Book::default();
        let all = jobs(&folder.0, &empty, &rels, &Embeddings::default(), "m", false, InputMode::Image).unwrap();
        assert_eq!(all.len(), 2, "both images need vectors");
        assert_eq!(pending(&folder.0, &empty, &rels, &Embeddings::default(), "m", false, InputMode::Image).unwrap(), 2, "the plan counts the same");
        let hash = all[0].hash.clone();
        let mut done = Embeddings { model: "m".into(), ..Default::default() };
        done.sheets.insert("a.png".into(), Vector { hash, vec: vec![1.0, 0.0] });
        assert_eq!(jobs(&folder.0, &empty, &rels, &done, "m", false, InputMode::Image).unwrap().len(), 1, "the unchanged sheet stays");
        assert_eq!(pending(&folder.0, &empty, &rels, &done, "m", false, InputMode::Image).unwrap(), 1);
        assert_eq!(jobs(&folder.0, &empty, &rels, &done, "other", false, InputMode::Image).unwrap().len(), 2, "a new model re-embeds all");
        let mut book = Book::default();
        book.sheets.insert("a.png".into(), Sidecar { label: Some(Label { provider: "p".into(), model: "m".into(),
            status: Status::Labeled, caption: "Tree".into(), tags: vec!["green".into()], tag_list: None }), ..Default::default() });
        assert_eq!(pending(&folder.0, &book, &rels, &done, "m", true, InputMode::Image).unwrap(), 2, "including the label re-embeds a.png");
    }

    #[test]
    fn a_reply_is_put_in_the_order_of_the_input() {
        let value = json!({"data":[{"index":1, "embedding":[0.0, 1.0]}, {"index":0, "embedding":[1.0, 0.0]}]});
        assert_eq!(response(&value, 2).unwrap(), vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        assert!(response(&json!({"data":[]}), 1).is_err());
        assert!(response(&json!({"error":{"message":"bad key"}}), 1).is_err());
    }

    #[test]
    fn a_run_embeds_writes_and_reports() {
        let folder = folder_with(&["a.png", "b.png"]);
        let rels: Vec<String> = ["a.png", "b.png"].iter().map(|s| s.to_string()).collect();
        let total = pending(&folder.0, &Book::default(), &rels, &Embeddings::default(), "m", false, InputMode::Image).unwrap();
        let plan = Plan { mode: InputMode::Image, provider: "provider".into(), root: folder.0.clone(), model: "m".into(), total, book: Book::default(),
            rels, labels: false, done: Embeddings::default() };
        let run = Run::start(plan,
            |body| {
                let n = body["input"].as_array().unwrap().len();
                Ok(reply(&(0..n).map(|i| vec![1.0, i as f32]).collect::<Vec<_>>()))
            }, || {}).unwrap();
        assert_eq!(run.result.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(), 2);
        let done = read(&folder.0).unwrap();
        assert_eq!(done.model, "m");
        assert_eq!(done.sheets.len(), 2);
        assert_eq!(done.sheets["a.png"].vec, vec![1.0, 0.0]);
    }
    #[test]
    fn malformed_vectors_and_indexes_are_rejected() {
        for embedding in [json!([]), json!([0, 0]), json!([1, "x"]), json!([1e100])] {
            assert!(response(&json!({"data": [{"embedding": embedding}]}), 1).is_err());
        }
        for data in [
            json!([{"index": 0, "embedding": [1]}, {"index": 0, "embedding": [1]}]),
            json!([{"index": 0, "embedding": [1]}, {"index": 2, "embedding": [1]}]),
            json!([{"index": 0, "embedding": [1]}, {"embedding": [1]}]),
            json!([{"embedding": [1]}, {"embedding": [1, 2]}]),
            json!([{"index": -1, "embedding": [1]}, {"index": 1, "embedding": [1]}]),
        ] { assert!(response(&json!({"data": data}), 2).is_err()); }
        assert_eq!(cosine(&[f32::MAX], &[f32::MAX]), 1.0);
        assert_eq!(cosine(&[f32::NAN], &[1.0]), 0.0);
    }

    fn plan(folder: &crate::storage::tests::Folder, rels: Vec<String>) -> Plan {
        Plan { mode: InputMode::Image, provider: "p".into(), root: folder.0.clone(), model: "m".into(),
            total: rels.len(), book: Book::default(), rels, labels: false, done: Embeddings::default() }
    }

    #[test]
    fn source_errors_are_reported_and_old_files_are_preserved() {
        let folder = folder_with(&["a.png"]);
        assert!(jobs(&folder.0, &Book::default(), &["missing.png".into()], &Embeddings::default(),
            "m", false, InputMode::Image).is_err());
        std::fs::write(folder.0.join(FILE), b"broken").unwrap();
        let run = Run::start(plan(&folder, vec!["a.png".into()]), |_| panic!("must not send"), || {}).unwrap();
        assert!(run.result.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
        assert_eq!(std::fs::read(folder.0.join(FILE)).unwrap(), b"broken");
    }

    #[test]
    fn successful_batches_survive_a_later_failure_without_old_model_vectors() {
        let names: Vec<String> = (0..17).map(|n| format!("{n}.png")).collect();
        let folder = folder_with(&names.iter().map(String::as_str).collect::<Vec<_>>());
        let mut p = plan(&folder, names);
        p.done = Embeddings { model: "old".into(), provider: "old-provider".into(), ..Default::default() };
        p.done.sheets.insert("old.png".into(), Vector { hash: "old".into(), vec: vec![1.0, 2.0, 3.0] });
        write(&folder.0, &p.done, InputMode::Image).unwrap();
        let count = std::sync::atomic::AtomicUsize::new(0);
        let run = Run::start(p, move |body| {
            if count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 1 { return Err("failed".into()); }
            Ok(reply(&vec![vec![1.0, 2.0]; body["input"].as_array().unwrap().len()]))
        }, || {}).unwrap();
        assert!(run.result.recv_timeout(Duration::from_secs(5)).unwrap().is_err());
        let data = read(&folder.0).unwrap();
        assert_eq!(data.sheets.len(), 16);
        assert_eq!(data.model, "m");
        assert_eq!(data.provider, "p");
        assert!(!data.sheets.contains_key("old.png"));
    }

    #[test]
    fn cancelling_during_a_request_discards_the_reply() {
        let folder = folder_with(&["a.png"]);
        let (started, waiting) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let (finished, finish) = std::sync::mpsc::channel();
        let run = Run::start(plan(&folder, vec!["a.png".into()]), move |_| {
            started.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(reply(&[vec![1.0]]))
        }, move || { finished.send(()).unwrap(); }).unwrap();
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(run);
        release.send(()).unwrap();
        finish.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!folder.0.join(FILE).exists());
    }

    #[test]
    fn text_vectors_are_independent_and_expire_when_labels_change() {
        use crate::sidecar::{Label, Sidecar, Status};
        let folder = folder_with(&["a.png", "b.png"]);
        let mut p = plan(&folder, vec!["a.png".into(), "b.png".into()]);
        p.mode = InputMode::Text;
        p.book.sheets.insert("a.png".into(), Sidecar { label: Some(Label { provider: "p".into(), model: "m".into(),
            status: Status::Labeled, caption: "Tree".into(), tags: vec!["green".into()], tag_list: None }), ..Default::default() });
        crate::storage::write(&folder.0.join(crate::sidecar::BOOK), &p.book).unwrap();
        let mut side = p.book.sheets["a.png"].clone();
        let run = Run::start(p, |body| {
            assert_eq!(body["input"], json!(["Tree green"]));
            Ok(reply(&[vec![1.0, 2.0]]))
        }, || {}).unwrap();
        assert_eq!(run.result.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(), 1);
        assert!(!folder.0.join(FILE).exists());
        let data = read_mode(&folder.0, InputMode::Text).unwrap();
        assert!(current_vector(&folder.0, "a.png", &side, &data, InputMode::Text).is_some());
        side.label.as_mut().unwrap().caption = "Rock".into();
        assert!(current_vector(&folder.0, "a.png", &side, &data, InputMode::Text).is_none());
    }

    #[test]
    fn changed_sources_do_not_save_a_stale_reply() {
        let folder = folder_with(&["a.png"]);
        let image_path = folder.0.join("a.png");
        let run = Run::start(plan(&folder, vec!["a.png".into()]), move |_| {
            RgbaImage::from_pixel(4, 4, Rgba([99, 2, 3, 255])).save(&image_path).unwrap();
            Ok(reply(&[vec![1.0]]))
        }, || {}).unwrap();
        let error = run.result.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err();
        assert!(error.contains("changed during"));
        assert!(!folder.0.join(FILE).exists());
    }

    #[test]
    fn a_selected_run_keeps_other_vectors_from_the_same_model() {
        let folder = folder_with(&["a.png", "b.png"]);
        let mut p = plan(&folder, vec!["a.png".into()]);
        p.done = Embeddings { model: "m".into(), provider: "p".into(), ..Default::default() };
        p.done.sheets.insert("b.png".into(), Vector { hash: "unchanged".into(), vec: vec![1.0] });
        write(&folder.0, &p.done, InputMode::Image).unwrap();
        let run = Run::start(p, |_| Ok(reply(&[vec![1.0]])), || {}).unwrap();
        assert_eq!(run.result.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(), 1);
        assert_eq!(read(&folder.0).unwrap().sheets.len(), 2);
    }

}
