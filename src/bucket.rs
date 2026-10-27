// SPDX-License-Identifier: GPL-3.0-only
//! The bucket in `server/`: put one image, delete it by key, and read back
//! the public URL it is served from. A batch needs a public URL for every
//! image, and the bucket is the small PHP service that serves them.

use serde::{Deserialize, Serialize};

/// Where a bucket is. The token is not here; it lives with the other secrets
/// in `keys.json`, and is passed to each call.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bucket {
    /// The base URL, such as `https://example.org/bucket`.
    pub url: String,
}

impl Bucket {
    /// The base URL without a trailing slash, once it can take a path.
    fn base(&self) -> Result<String, String> {
        let url = self.url.trim().trim_end_matches('/');
        if url.is_empty() { return Err("Set the bucket URL.".into()); }
        let uri: ureq::http::Uri = url.parse().map_err(|_| "The bucket URL is not a URL.")?;
        let scheme = uri.scheme_str().ok_or("The bucket URL must start with http:// or https://.")?;
        if scheme != "http" && scheme != "https" {
            return Err("The bucket URL must start with http:// or https://.".into());
        }
        if uri.authority().is_none() || uri.query().is_some() || url.contains('#') {
            return Err("Use the bucket URL alone, without a query or a fragment.".into());
        }
        Ok(url.into())
    }
}

/// Talks to one bucket. Cheap to make and to clone.
pub struct Client { base: String, token: String, agent: ureq::Agent }

impl Client {
    pub fn new(bucket: &Bucket, token: String) -> Result<Self, String> {
        let base = bucket.base()?;
        if token.trim().is_empty() { return Err("Set the bucket token.".into()); }
        Ok(Self { base, token: token.trim().to_string(), agent: crate::labels::agent() })
    }

    /// Uploads one image and answers its key and the URL the provider fetches.
    pub fn upload(&self, image: &[u8]) -> Result<(String, String), String> {
        let response = self.agent.post(&format!("{}/upload.php", self.base))
            .header("X-Bucket-Token", &self.token)
            .header("Content-Type", "image/png")
            .send(image)
            .map_err(|error| format!("The upload failed: {}", http_message(error)))?;
        answer(response, "upload").and_then(parse_upload)
    }

    /// Deletes one image. A key that is already gone is not an error.
    pub fn delete(&self, key: &str) -> Result<(), String> {
        let response = self.agent.post(&format!("{}/delete.php", self.base))
            .header("X-Bucket-Token", &self.token)
            .header("Content-Type", "application/json")
            .send_json(serde_json::json!({ "key": key }))
            .map_err(|error| format!("The delete failed: {}", http_message(error)))?;
        answer(response, "delete").map(|_| ())
    }
}

/// The key and URL from an upload answer, or the reason it has neither.
fn parse_upload(body: serde_json::Value) -> Result<(String, String), String> {
    let key = body["key"].as_str().filter(|key| !key.is_empty())
        .ok_or("The bucket answered without an image key.")?;
    let url = body["url"].as_str().filter(|url| !url.is_empty())
        .ok_or("The bucket answered without an image URL.")?;
    Ok((key.to_string(), url.to_string()))
}

/// The JSON answer of a call, or the message the bucket put in it.
fn answer(mut response: ureq::http::Response<ureq::Body>, action: &str) -> Result<serde_json::Value, String> {
    let status = response.status().as_u16();
    let bytes = response.body_mut().read_to_vec()
        .map_err(|error| format!("Could not read the {action} answer: {error}"))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    if !(200..300).contains(&status) {
        let message = value["error"].as_str().unwrap_or("");
        return Err(if message.is_empty() { format!("The {action} returned HTTP {status}.") } else { message.to_string() });
    }
    Ok(value)
}

fn http_message(error: ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(code) => format!("HTTP {code}"),
        other => other.to_string(),
    }
}

/// A one-pixel upload, fetch, and delete, to check the URL, the token, and the
/// static files. It leaves nothing behind on a bucket that works.
pub fn self_test(bucket: Bucket, token: String) -> Result<(), String> {
    let client = Client::new(&bucket, token)?;
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(1, 1).write_to(&mut png, image::ImageFormat::Png)
        .map_err(|_| "Could not build a test image.".to_string())?;
    let png = png.into_inner();
    let (key, url) = client.upload(&png)?;
    let fetched = fetch(&client.agent, &url, &png);
    let deleted = client.delete(&key);
    match (fetched, deleted) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(fetch), Err(delete)) => Err(format!("{fetch} {delete}")),
    }
}

/// Fetches an uploaded image and compares it with what was sent.
fn fetch(agent: &ureq::Agent, url: &str, image: &[u8]) -> Result<(), String> {
    let mut response = agent.get(url).call()
        .map_err(|error| format!("The image URL did not serve the image: {}", http_message(error)))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("The image URL returned HTTP {status}. Is `base_url` the public address of the bucket?"));
    }
    let received = response.body_mut().read_to_vec().map_err(|error| format!("Could not read the test image: {error}"))?;
    if received != image { return Err("The image URL served different bytes.".into()); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket(url: &str) -> Bucket { Bucket { url: url.into() } }

    #[test]
    fn a_bucket_url_keeps_its_path_and_loses_its_trailing_slash() {
        assert_eq!(bucket("https://example.org/bucket/").base().unwrap(), "https://example.org/bucket");
        assert_eq!(bucket("  https://example.org  ").base().unwrap(), "https://example.org");
    }

    #[test]
    fn a_bucket_url_that_cannot_take_a_path_is_refused() {
        for url in ["", "   ", "example.org/bucket", "ftp://example.org", "https://example.org/?a=1", "https://example.org/#x"] {
            assert!(bucket(url).base().is_err(), "{url} must be refused");
        }
    }

    #[test]
    fn a_client_needs_a_token() {
        assert!(Client::new(&bucket("https://example.org"), " ".into()).is_err());
        assert!(Client::new(&bucket("https://example.org"), "t".into()).is_ok());
    }

    #[test]
    fn an_upload_answer_carries_a_key_and_a_url() {
        let (key, url) = parse_upload(serde_json::json!({"key":"a1.png","url":"https://example.org/files/a1.png","bytes":628})).unwrap();
        assert_eq!(key, "a1.png");
        assert_eq!(url, "https://example.org/files/a1.png");
        assert!(parse_upload(serde_json::json!({"url":"https://example.org/a.png"})).is_err());
        assert!(parse_upload(serde_json::json!({"key":"a.png"})).is_err());
    }
}
