// SPDX-License-Identifier: GPL-3.0-only
//! The OpenRouter batch path: upload each sheet, hand the provider a URL,
//! submit, poll, and delete the objects again. OpenRouter batch images are
//! URL-only, so the tool hosts them in an S3-compatible bucket meantime.

use super::*;

/// The object-storage side of a batch. A test supplies its own.
pub trait Objects: std::marker::Send + Sync {
    /// Uploads one PNG and returns a URL the provider can fetch.
    fn upload(&self, key: &str, png: &[u8]) -> Result<String, String>;
    /// Deletes one uploaded object. A failure remains visible in the job.
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// Object storage over the S3 client.
pub struct Storage(crate::s3::Client);
impl Storage {
    pub fn new(client: crate::s3::Client) -> Self { Self(client) }
}
impl Objects for Storage {
    fn upload(&self, key: &str, png: &[u8]) -> Result<String, String> {
        let url = self.0.presigned_get(key, URL_SECONDS)?;
        self.0.put(key, png, "image/png")?;
        Ok(url)
    }
    fn delete(&self, key: &str) -> Result<(), String> { self.0.delete(key) }
}

/// The lifetime of a sheet URL, and the age a bucket lifecycle rule should
/// delete objects at. The batch window is 24 hours.
const URL_SECONDS: u64 = 2 * 24 * 60 * 60;

/// Each upload has its own key, including identical images in other jobs.
fn object_key() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("tilepicky/{}-{stamp}-{sequence}.png", std::process::id())
}

fn batch_body(model: &str, requests: &[(usize, Value)]) -> Value {
    json!({"endpoint":"/v1/chat/completions", "model": model,
        "requests": requests.iter().map(|(i, body)| json!({"custom_id": key(*i), "body": body})).collect::<Vec<_>>()})
}

/// Uploads the next sheets and submits them as one OpenRouter batch.
pub fn submit(job: &mut Job, root: &Path, dir: &Path, send: Send) -> Result<(), String> {
    let Some(objects) = job.objects.clone() else { return Err("Object storage is not set for this provider.".into()) };
    let book = crate::sidecar::load_book(root)?;
    let (mut requests, mut taken) = (Vec::new(), Vec::new());
    for i in 0..job.sheets.len() {
        if job.sheets[i].taken { continue; }
        if requests.len() == MAX_REQUESTS { break; }
        // A failed cleanup must not lose its key or create another object.
        if let Err(error) = delete_sheet(job, i, objects.as_ref()) {
            job.sheets[i].error = error;
            job.sheets[i].taken = true;
            continue;
        }
        let existing = book.sheets.get(&job.sheets[i].rel).and_then(|s| s.label.clone());
        let mut body = match image_request(job, root, i, existing) {
            Ok(body) => body,
            Err(error) => { job.sheets[i].error = error; job.sheets[i].taken = true; continue; }
        };
        body.as_object_mut().unwrap().remove("provider");
        use base64::Engine;
        let encoded = body["messages"][1]["content"][1]["image_url"]["url"].as_str()
            .and_then(|url| url.strip_prefix("data:image/png;base64,")).ok_or("The request has no PNG image.")?;
        let png = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| "The request image is invalid.")?;
        let object = object_key();
        job.sheets[i].stored = Some(object.clone());
        let url = match objects.upload(&object, &png) {
            Ok(url) => url,
            Err(error) => {
                // A failed PUT can still have stored the object upstream.
                let cleanup = delete_sheet(job, i, objects.as_ref()).err();
                job.sheets[i].error = cleanup.map_or(error.clone(), |cleanup| format!("{error} {cleanup}"));
                job.sheets[i].taken = true;
                continue;
            }
        };
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
            Some(id) if !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)) => {
                job.groups[group].tracking.id = id.to_string();
                job.groups[group].remote = Remote::Waiting(id.to_string());
            }
            _ => {
                for &i in &taken { job.sheets[i].error = "The batch was accepted without an ID.".into(); }
                job.groups[group].remote = Remote::Done;
            }
        },
        Err(failure @ (Failure::NotSent(_) | Failure::Status(401 | 403 | 429, _))) => {
            // The batch did not go out: the sheets may try again.
            delete_group(job, group);
            job.groups.remove(group);
            for &i in &taken { job.sheets[i].taken = false; }
            return job.save(dir).and(Err(failure.message()));
        }
        Err(failure @ Failure::Status(400..=499, _)) => {
            delete_group(job, group);
            for &i in &taken { job.sheets[i].error = failure.message(); }
            job.groups[group].remote = Remote::Done;
        }
        Err(failure) => {
            // The batch may exist. OpenRouter has no client reference to find
            // it again, so the sheets end here rather than being sent twice.
            // Keep the images available if the provider accepted this batch.
            for &i in &taken { job.sheets[i].error =
                format!("{} The batch may have been accepted, so it was not sent again; retrying can bill twice.", failure.message()); }
            job.groups[group].remote = Remote::Done;
        }
    }
    job.save(dir)
}

/// A saved submission without an ID must never send the same paid work again.
pub fn close_unconfirmed(job: &mut Job) {
    for group in &mut job.groups {
        if group.remote != Remote::Submitting { continue; }
        for &i in &group.sheets {
            job.sheets[i].error = "The batch submission was interrupted. It may have been accepted; retrying can bill twice.".into();
        }
        group.remote = Remote::Done;
    }
}

/// Removes an object before its sheet can upload another one.
fn delete_sheet(job: &mut Job, sheet: usize, objects: &dyn Objects) -> Result<(), String> {
    let Some(object) = job.sheets[sheet].stored.clone() else { return Ok(()) };
    match objects.delete(&object) {
        Ok(()) => {
            job.sheets[sheet].stored = None;
            if job.sheets.iter().all(|sheet| sheet.stored.is_none()) { job.issues.remove("cleanup"); }
            Ok(())
        }
        Err(error) => {
            let message = format!("{object}: {error}");
            let previous = job.issues.get("cleanup").map(|i| i.message.clone()).unwrap_or_default();
            if !previous.contains(&object) {
                job.issue("cleanup", format!("{previous}\nRemove this uploaded object from your bucket, or let its lifecycle rule remove it:\n{message}"),
                    now_ms());
            }
            Err(format!("Object cleanup must finish before another upload: {message}"))
        }
    }
}

/// Deletes the objects of a group and clears only successfully removed keys.
fn delete_group(job: &mut Job, group: usize) {
    let Some(objects) = job.objects.clone() else { return };
    for sheet in job.groups[group].sheets.clone() { let _ = delete_sheet(job, sheet, objects.as_ref()); }
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
        fn delete(&self, key: &str) -> Result<(), String> { self.deleted.lock().unwrap().push(key.to_string()); Ok(()) }
    }

    #[test]
    fn a_rejected_submission_cannot_upload_again_while_cleanup_fails() {
        struct Leaky { uploads: Mutex<usize> }
        impl Objects for Leaky {
            fn upload(&self, _: &str, _: &[u8]) -> Result<String, String> {
                *self.uploads.lock().unwrap() += 1; Ok("https://example.test/image.png".into())
            }
            fn delete(&self, _: &str) -> Result<(), String> { Err("HTTP 403".into()) }
        }
        let dir = crate::storage::tests::Folder::new();
        RgbaImage::new(2, 2).save(dir.0.join("a.png")).unwrap();
        let mut job = prepare(&crate::index::Index::scan(&dir.0, [16, 16]),
            crate::batch::tests::provider(Kind::OpenAi), "test".into(), Scope::Unlabeled).unwrap();
        let objects = Arc::new(Leaky { uploads: Mutex::new(0) }); job.objects = Some(objects.clone());
        assert!(submit(&mut job, &dir.0, &dir.0, &mut |_, _| Err(Failure::Status(429, "Wait".into()))).is_err());
        let key = job.sheets[0].stored.clone(); let message = job.issues["cleanup"].message.clone();
        submit(&mut job, &dir.0, &dir.0, &mut |_, _| panic!("cleanup blocks another submission")).unwrap();
        assert_eq!(*objects.uploads.lock().unwrap(), 1);
        assert_eq!(job.sheets[0].stored, key);
        assert_eq!(job.issues["cleanup"].message, message, "retries do not grow the cleanup report");
        assert!(job.sheets[0].taken && job.sheets[0].error.contains("cleanup must finish"));
    }

    #[test]
    fn failed_cleanup_keeps_the_object_key_and_explains_how_to_remove_it() {
        struct CannotDelete;
        impl Objects for CannotDelete {
            fn upload(&self, _: &str, _: &[u8]) -> Result<String, String> { unreachable!() }
            fn delete(&self, _: &str) -> Result<(), String> { Err("HTTP 403".into()) }
        }
        let dir = crate::storage::tests::Folder::new();
        RgbaImage::new(2, 2).save(dir.0.join("a.png")).unwrap();
        let mut job = prepare(&crate::index::Index::scan(&dir.0, [16, 16]),
            crate::batch::tests::provider(Kind::OpenAi), "test".into(), Scope::Unlabeled).unwrap();
        job.sheets[0].stored = Some("tilepicky/test.png".into());
        job.objects = Some(Arc::new(CannotDelete));
        job.groups.push(Group { sheets: vec![0], remote: Remote::Done, recovery: None, tracking: Tracking::default() });
        delete_group(&mut job, 0);
        assert_eq!(job.sheets[0].stored.as_deref(), Some("tilepicky/test.png"));
        assert!(job.issues["cleanup"].message.contains("tilepicky/test.png: HTTP 403"));
        job.save(&dir.0).unwrap();
        assert!(Job::load(&dir.0).unwrap().unwrap().issues.contains_key("cleanup"));
    }

    #[test]
    fn missing_images_fail_individually_and_do_not_block_the_batch() {
        let dir = crate::storage::tests::Folder::new();
        for name in ["a.png", "b.png"] { RgbaImage::new(2, 2).save(dir.0.join(name)).unwrap(); }
        let mut job = prepare(&crate::index::Index::scan(&dir.0, [16, 16]),
            crate::batch::tests::provider(Kind::OpenAi), "test".into(), Scope::Unlabeled).unwrap();
        std::fs::remove_file(dir.0.join("a.png")).unwrap();
        job.objects = Some(Arc::new(Fake { uploaded: Mutex::new(vec![]), deleted: Mutex::new(vec![]) }));
        submit(&mut job, &dir.0, &dir.0, &mut |_, body| {
            assert_eq!(body.unwrap()["requests"].as_array().unwrap().len(), 1);
            Ok(json!({"id":"batch-1"}))
        }).unwrap();
        assert!(!job.sheets.iter().find(|s| s.rel == "a.png").unwrap().error.is_empty());
        assert_eq!(job.groups.len(), 1);
    }

    #[test]
    fn each_upload_owns_a_different_object() {
        assert_ne!(object_key(), object_key());
    }

    #[test]
    fn a_restart_closes_an_unconfirmed_submission_without_resending() {
        let dir = crate::storage::tests::Folder::new();
        let mut job = prepare(&crate::index::Index::scan(&dir.0, [16, 16]),
            crate::batch::tests::provider(Kind::OpenAi), "test".into(), Scope::Unlabeled).unwrap();
        job.groups.push(Group { sheets: vec![], remote: Remote::Submitting, recovery: None, tracking: Tracking::default() });
        close_unconfirmed(&mut job);
        assert!(job.done());
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
    /// end with a reason. Objects stay until the bucket lifecycle removes them.
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
        assert!(job.sheets[0].stored.is_some());
        assert!(fake.deleted.lock().unwrap().is_empty());
    }
}
