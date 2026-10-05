// SPDX-License-Identifier: GPL-3.0-only
//! The OpenRouter batch path: upload each sheet, hand the provider a URL,
//! submit, poll, and delete the objects again. OpenRouter batch images are
//! URL-only, so the tool hosts them in an S3-compatible bucket meantime.

use super::*;

/// The object-storage side of a batch. A test supplies its own.
pub trait Objects: std::marker::Send + Sync {
    /// Uploads one PNG and returns a URL the provider can fetch.
    fn upload(&self, key: &str, png: &[u8]) -> Result<String, String>;
    /// Deletes one uploaded object. A failure is not fatal.
    fn delete(&self, key: &str);
}

/// Object storage over the S3 client.
pub struct Storage(crate::s3::Client);
impl Storage {
    pub fn new(client: crate::s3::Client) -> Self { Self(client) }
}
impl Objects for Storage {
    fn upload(&self, key: &str, png: &[u8]) -> Result<String, String> {
        self.0.put(key, png, "image/png")?;
        self.0.presigned_get(key, URL_SECONDS)
    }
    fn delete(&self, key: &str) { let _ = self.0.delete(key); }
}

/// The lifetime of a sheet URL, and the age a bucket lifecycle rule should
/// delete objects at. The batch window is 24 hours.
const URL_SECONDS: u64 = 2 * 24 * 60 * 60;

/// The object key for one sheet, by content, so a repeat reuses it.
fn object_key(png: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("tilepicky/{:x}.png", Sha256::digest(png))
}

fn batch_body(model: &str, requests: &[(usize, Value)]) -> Value {
    json!({"endpoint":"/v1/chat/completions", "model": model,
        "requests": requests.iter().map(|(i, body)| json!({"custom_id": key(*i), "body": body})).collect::<Vec<_>>()})
}

/// Uploads the next sheets and submits them as one OpenRouter batch.
pub fn submit(job: &mut Job, root: &Path, dir: &Path, send: Send) -> Result<(), String> {
    let Some(objects) = job.objects.clone() else { return Err("Object storage is not set for this provider.".into()) };
    let (mut requests, mut taken) = (Vec::new(), Vec::new());
    for i in 0..job.sheets.len() {
        if job.sheets[i].taken { continue; }
        if requests.len() == MAX_REQUESTS { break; }
        let bytes = std::fs::read(root.join(&job.sheets[i].rel)).map_err(|e| format!("Could not read the image: {e}"))?;
        let image = image::load_from_memory(&bytes).map_err(|e| format!("Could not read the image: {e}"))?;
        let png = labels::png_bytes(&image.to_rgba8())?;
        let object = object_key(&png);
        let url = match objects.upload(&object, &png) {
            Ok(url) => url,
            Err(error) => { job.sheets[i].error = error; job.sheets[i].taken = true; continue; }
        };
        job.sheets[i].stored = Some(object);
        let mut body = chat_request(job, &image.to_rgba8(), &job.sheets[i].rel)?;
        body["messages"][1]["content"][1]["image_url"]["url"] = json!(url);
        requests.push((i, body));
        taken.push(i);
    }
    if requests.is_empty() { return job.save(dir); }
    for &i in &taken { job.sheets[i].taken = true; }
    job.groups.push(Group { sheets: taken.clone(), remote: Remote::Submitting, recovery: None, tracking: Tracking::default() });
    job.save(dir)?;
    let group = job.groups.len() - 1;
    crate::ai_log::event("batch_submit", json!({"provider":job.provider.name, "model":job.model, "sheets":taken.len(),
        "detail":requests.iter().map(|(i, _)| json!({"id":key(*i), "sheet":job.sheets[*i].rel, "object":job.sheets[*i].stored})).collect::<Vec<_>>()}));
    match send("batches", Some(&batch_body(&job.model.clone(), &requests))) {
        Ok(response) => match response["id"].as_str() {
            Some(id) => { job.groups[group].tracking.id = id.to_string(); job.groups[group].remote = Remote::Waiting(id.to_string()); }
            None => {
                delete_group(job, group);
                for &i in &taken { job.sheets[i].error = "The batch was accepted without an ID.".into(); }
                job.groups[group].remote = Remote::Done;
            }
        },
        Err(failure @ (Failure::NotSent(_) | Failure::Status(401 | 403 | 429 | 500..=599, _))) => {
            // The batch did not go out: the sheets may try again.
            delete_group(job, group);
            job.groups.remove(group);
            for &i in &taken { job.sheets[i].taken = false; }
            return job.save(dir).and(Err(failure.message()));
        }
        Err(failure) => {
            // The batch may exist. OpenRouter has no client reference to find
            // it again, so the sheets end here rather than being sent twice.
            delete_group(job, group);
            for &i in &taken { job.sheets[i].error =
                format!("{} The batch may have been accepted, so it was not sent again; retrying can bill twice.", failure.message()); }
            job.groups[group].remote = Remote::Done;
        }
    }
    job.save(dir)
}

/// Deletes the objects of a group and clears their keys.
fn delete_group(job: &mut Job, group: usize) {
    let Some(objects) = job.objects.clone() else { return };
    for &s in &job.groups[group].sheets {
        if let Some(object) = job.sheets[s].stored.take() { objects.delete(&object); }
    }
}

/// OpenRouter has no cancel call. The batch may still finish upstream, so the
/// tool stops waiting, lets go of the objects, and marks the sheets cancelled.
pub fn cancel(job: &mut Job, dir: &Path) -> Result<(), String> {
    let mut left = false;
    for i in 0..job.groups.len() {
        if matches!(job.groups[i].remote, Remote::Waiting(_)) {
            left = true;
            delete_group(job, i);
            for &s in &job.groups[i].sheets { job.sheets[s].cancelled = true; }
            job.groups[i].remote = Remote::Done;
        }
    }
    if left {
        job.issue("cancel", "A batch already sent to OpenRouter may still finish and bill; its labels are not collected.".into(), now_ms());
    }
    job.save(dir)
}

/// Reads the next OpenRouter batch and, when it finishes, its results.
pub fn poll(job: &mut Job, dir: &Path, send: Send) -> Result<(), String> {
    let Some(i) = job.groups.iter().enumerate().filter(|(_, g)| matches!(g.remote, Remote::Waiting(_)))
        .min_by_key(|(_, g)| g.tracking.checked_ms).map(|(i, _)| i) else { return Ok(()) };
    let Remote::Waiting(id) = job.groups[i].remote.clone() else { unreachable!() };
    let response = send(&format!("batches/{id}"), None).map_err(|f| f.message())?;
    job.groups[i].tracking.state = response["status"].as_str().unwrap_or_default().into();
    match outputs(&response) {
        Ok(None) => { job.groups.rotate_left(i + 1); return job.save(dir); }
        Ok(Some(values)) => accept(job, i, values),
        Err(error) => for &s in &job.groups[i].sheets { job.sheets[s].error = error.clone(); },
    }
    delete_group(job, i);
    job.groups[i].remote = Remote::Done;
    job.save(dir)
}

/// The results of a finished OpenRouter batch, each as the chat completion
/// that `labels::diagnosed_response` reads. None while it still runs.
fn outputs(value: &Value) -> Result<Option<Vec<(String, Value)>>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::tests::{completion, labeled};
    use image::{Rgba, RgbaImage};
    use std::sync::{Arc, Mutex};

    struct Fake { uploaded: Mutex<Vec<String>>, deleted: Mutex<Vec<String>> }
    impl Objects for Fake {
        fn upload(&self, key: &str, _png: &[u8]) -> Result<String, String> {
            self.uploaded.lock().unwrap().push(key.to_string());
            Ok(format!("https://example.test/{key}"))
        }
        fn delete(&self, key: &str) { self.deleted.lock().unwrap().push(key.to_string()); }
    }

    /// Upload, submit, poll a finished batch, save the labels, delete the
    /// objects. No network.
    #[test]
    fn an_openrouter_batch_uploads_submits_polls_and_deletes() {
        let dir = crate::storage::tests::Folder::new();
        let root = dir.0.join("library");
        std::fs::create_dir_all(&root).unwrap();
        RgbaImage::from_pixel(8, 8, Rgba([1, 2, 3, 255])).save(root.join("sheet.png")).unwrap();
        let mut provider = crate::batch::tests::provider(Kind::OpenAi);
        provider.store = Some(crate::s3::Store { endpoint: "https://example.test".into(), region: "auto".into(),
            bucket: "b".into(), access_key: "k".into(), path_style: false });
        let mut job = crate::batch::prepare(&crate::index::Index::scan(&root, [16, 16]), provider,
            "openai/gpt-5-nano".into(), crate::batch::Scope::Unlabeled).unwrap();
        let fake = Arc::new(Fake { uploaded: Mutex::new(Vec::new()), deleted: Mutex::new(Vec::new()) });
        job.objects = Some(fake.clone());

        let mut paths = Vec::new();
        {
            let mut send = |path: &str, _body: Option<&Value>| -> Result<Value, Failure> {
                paths.push(path.to_string());
                Ok(json!({"id":"batch-1"}))
            };
            submit(&mut job, &root, &root, &mut send).unwrap();
        }
        assert_eq!(paths, ["batches"]);
        assert!(job.sheets[0].taken);
        assert!(matches!(&job.groups[0].remote, Remote::Waiting(id) if id == "batch-1"));
        assert_eq!(job.sheets[0].stored.as_ref(), fake.uploaded.lock().unwrap().first());
        assert!(job.objects.is_some());

        let body = completion(labeled("Tree"));
        let reply = json!({"status":"completed", "results":[{"custom_id":"sheet-0", "error":null, "response":{"body":body}}]});
        {
            let mut send = |_path: &str, _body: Option<&Value>| -> Result<Value, Failure> { Ok(reply.clone()) };
            poll(&mut job, &root, &mut send).unwrap();
        }
        assert!(job.remote_done());
        assert_eq!(job.sheets[0].label.as_ref().unwrap().caption, "Tree");
        assert!(job.sheets[0].stored.is_none());
        assert_eq!(fake.deleted.lock().unwrap().len(), 1);
    }

    /// A submit whose reply is lost does not send the batch again. The sheets
    /// end with a reason, the objects go, and the group closes.
    #[test]
    fn a_lost_submit_reply_does_not_resend_the_batch() {
        let dir = crate::storage::tests::Folder::new();
        let root = dir.0.join("library");
        std::fs::create_dir_all(&root).unwrap();
        RgbaImage::from_pixel(8, 8, Rgba([1, 2, 3, 255])).save(root.join("sheet.png")).unwrap();
        let mut provider = crate::batch::tests::provider(Kind::OpenAi);
        provider.store = Some(crate::s3::Store { endpoint: "https://example.test".into(), region: "auto".into(),
            bucket: "b".into(), access_key: "k".into(), path_style: false });
        let mut job = crate::batch::prepare(&crate::index::Index::scan(&root, [16, 16]), provider,
            "openai/gpt-5-nano".into(), crate::batch::Scope::Unlabeled).unwrap();
        let fake = Arc::new(Fake { uploaded: Mutex::new(Vec::new()), deleted: Mutex::new(Vec::new()) });
        job.objects = Some(fake.clone());
        let mut sent = 0;
        {
            let mut send = |_path: &str, _body: Option<&Value>| -> Result<Value, Failure> {
                sent += 1;
                Err(Failure::Unknown("timed out".into()))
            };
            submit(&mut job, &root, &root, &mut send).unwrap();
        }
        assert_eq!(sent, 1, "a lost reply must not send the batch twice");
        assert!(job.sheets[0].taken);
        assert!(job.sheets[0].error.contains("not sent again"));
        assert!(matches!(job.groups[0].remote, Remote::Done));
        assert!(job.sheets[0].stored.is_none());
        assert_eq!(fake.deleted.lock().unwrap().len(), 1);
    }
}
