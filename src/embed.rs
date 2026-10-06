// SPDX-License-Identifier: GPL-3.0-only
//! Embeddings of sheet images, for semantic search. Each sheet gets one
//! vector, stored beside the library in `embeddings.json`. A vector is the
//! model's answer for the sheet's image. The search query is embedded as text
//! by the same model, so both share one space.

use crate::ai::{Kind, Provider};
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

/// The vectors of one library, and the model that made them.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Embeddings {
    /// The embedding model. Vectors from another model do not match it.
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub sheets: BTreeMap<String, Vector>,
}

/// One sheet's vector, and the hash of the text it was made from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Vector {
    pub hash: String,
    pub vec: Vec<f32>,
}

/// The hash of the image bytes, so that a changed image is embedded again.
pub fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Reads the vectors of a library. A missing file is an empty set.
pub fn read(root: &Path) -> Result<Embeddings, String> {
    crate::storage::read(&root.join(FILE))
}

/// Writes the vectors of a library, replacing the file.
pub fn write(root: &Path, embeddings: &Embeddings) -> Result<(), String> {
    crate::storage::write(&root.join(FILE), embeddings)
}

/// The cosine similarity of two vectors. Zero when either is empty.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let (na, nb): (f32, f32) = (a.iter().map(|x| x * x).sum(), b.iter().map(|x| x * x).sum());
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na.sqrt() * nb.sqrt()) }
}

/// One sheet that needs a vector: its path, the request input part, and the
/// hash of the image it was made from.
pub struct Job {
    pub rel: String,
    pub input: Value,
    pub hash: String,
}

/// The sheets to embed: every sheet whose image changed or whose vector is
/// missing, when the model matches. A new model re-embeds all of them.
pub fn jobs(root: &Path, rels: &[String], done: &Embeddings, model: &str) -> Vec<Job> {
    let same = done.model == model;
    rels.iter()
        .filter_map(|rel| {
            let bytes = std::fs::read(root.join(rel)).ok()?;
            let hash = hash_bytes(&bytes);
            if same && done.sheets.get(rel).is_some_and(|v| v.hash == hash) {
                return None;
            }
            let url = crate::labels::embed_image(&bytes).ok()?;
            let input = json!({"content":[{"type":"image_url", "image_url":{"url":url}}]});
            Some(Job { rel: rel.clone(), input, hash })
        })
        .collect()
}

/// The request body for one batch of inputs. A text query is a plain string;
/// a sheet is a content part that carries its image URL.
pub fn request(model: &str, inputs: &[Value]) -> Value {
    json!({"model": model, "input": inputs, "encoding_format": "float"})
}

/// How many sheets still need a vector. It reads the images but does not
/// decode them, so a plan can be counted without the work of a run.
pub fn pending(root: &Path, rels: &[String], done: &Embeddings, model: &str) -> usize {
    let same = done.model == model;
    rels.iter()
        .filter(|rel| {
            let Ok(bytes) = std::fs::read(root.join(rel)) else { return false };
            if image::guess_format(&bytes).is_err() { return false; }
            !(same && done.sheets.get(*rel).is_some_and(|v| v.hash == hash_bytes(&bytes)))
        })
        .count()
}

/// The vectors of a reply, in the order of the input. An endpoint that drops
/// the index falls back to the order it returned.
pub fn response(value: &Value, count: usize) -> Result<Vec<Vec<f32>>, String> {
    if let Some(error) = value.get("error").filter(|e| !e.is_null()) {
        return Err(format!("The endpoint rejected the request. {}", crate::labels::error_detail(error, "")).trim_end().into());
    }
    let data = value.get("data").and_then(Value::as_array).ok_or("The embedding endpoint returned no data.")?;
    let mut out: Vec<Option<Vec<f32>>> = vec![None; count];
    for (fallback, item) in data.iter().enumerate() {
        let i = item.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(fallback);
        let vec: Vec<f32> = item.get("embedding").and_then(Value::as_array)
            .ok_or("An embedding is missing.")?
            .iter().map(|n| n.as_f64().unwrap_or(0.0) as f32).collect();
        if i < out.len() { out[i] = Some(vec); }
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
    pub result: std::sync::mpsc::Receiver<Result<usize, String>>,
    /// How many sheets the run will embed.
    pub total: usize,
}

impl Run {
    pub fn start(
        root: PathBuf, model: String, total: usize, rels: Vec<String>, done: Embeddings,
        send: impl Fn(&Value) -> Result<Value, String> + Send + 'static,
        wake: impl FnOnce() + Send + 'static,
    ) -> Result<Self, String> {
        let (tx, result) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("generate embeddings".into()).spawn(move || {
            // The images are read and decoded here, off the UI thread.
            let jobs = jobs(&root, &rels, &done, &model);
            let mut done = done;
            done.model = model.clone();
            let outcome = embed_all(&root, &model, &jobs, &mut done, &send);
            let _ = tx.send(outcome);
            wake();
        }).map_err(|_| "Could not start the embeddings thread.")?;
        Ok(Self { result, total })
    }
}

/// Embeds every job in batches and writes the file once at the end. A sheet
/// keeps its old vector when its batch fails; the error names the batch.
fn embed_all(root: &Path, model: &str, jobs: &[Job], done: &mut Embeddings, send: &impl Fn(&Value) -> Result<Value, String>) -> Result<usize, String> {
    for batch in jobs.chunks(BATCH) {
        let inputs: Vec<Value> = batch.iter().map(|j| j.input.clone()).collect();
        let value = send(&request(model, &inputs))?;
        let vectors = response(&value, inputs.len())?;
        for (job, vec) in batch.iter().zip(vectors) {
            done.sheets.insert(job.rel.clone(), Vector { hash: job.hash.clone(), vec });
        }
    }
    write(root, done)?;
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
    /// re-embeds every sheet; an unreadable file is skipped.
    #[test]
    fn jobs_skip_what_is_already_embedded() {
        let folder = folder_with(&["a.png", "b.png"]);
        std::fs::write(folder.0.join("broken.png"), b"nope").unwrap();
        let rels: Vec<String> = ["a.png", "b.png", "broken.png"].iter().map(|s| s.to_string()).collect();
        let all = jobs(&folder.0, &rels, &Embeddings::default(), "m");
        assert_eq!(all.len(), 2, "the unreadable file is skipped");
        assert_eq!(pending(&folder.0, &rels, &Embeddings::default(), "m"), 2, "the plan counts the same");
        let hash = all[0].hash.clone();
        let mut done = Embeddings { model: "m".into(), ..Default::default() };
        done.sheets.insert("a.png".into(), Vector { hash, vec: vec![1.0, 0.0] });
        assert_eq!(jobs(&folder.0, &rels, &done, "m").len(), 1, "the unchanged sheet stays");
        assert_eq!(pending(&folder.0, &rels, &done, "m"), 1);
        assert_eq!(jobs(&folder.0, &rels, &done, "other").len(), 2, "a new model re-embeds all");
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
        let total = pending(&folder.0, &rels, &Embeddings::default(), "m");
        let run = Run::start(folder.0.clone(), "m".into(), total, rels, Embeddings::default(),
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
}
