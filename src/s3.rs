// SPDX-License-Identifier: GPL-3.0-only
//! A small S3-compatible client: put one object, delete it, and presign a
//! GET that a provider can fetch. It signs with AWS Signature Version 4, in
//! either host style (`bucket.host`) or path style (`host/bucket`).
#![allow(dead_code)] // Wired into the batch path next.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

type HmacSha256 = Hmac<Sha256>;

/// A bucket to upload sheets to. The secret key is not here; it lives with
/// the other secrets in `keys.json`, and is passed to each call.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Store {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    /// Path-style URLs, as MinIO and some stores use.
    #[serde(default)]
    pub path_style: bool,
}

impl Store {
    pub fn complete(&self) -> bool {
        !self.endpoint.trim().is_empty() && !self.region.trim().is_empty()
            && !self.bucket.trim().is_empty() && !self.access_key.trim().is_empty()
    }

    /// The scheme and host of the endpoint, without the trailing slash.
    fn scheme_host(&self) -> Result<(String, String), String> {
        let endpoint = self.endpoint.trim().trim_end_matches('/');
        let (scheme, rest) = endpoint.split_once("://").ok_or("The endpoint must start with http:// or https://.")?;
        if scheme != "http" && scheme != "https" { return Err("The endpoint must start with http:// or https://.".into()); }
        if rest.is_empty() || rest.contains('/') { return Err("The endpoint must be a host, with no path.".into()); }
        Ok((scheme.into(), rest.into()))
    }

    /// The host and object path for one key.
    fn target(&self, key: &str) -> Result<(String, String), String> {
        let (_, host) = self.scheme_host()?;
        let key = key.trim_start_matches('/');
        if self.path_style { Ok((host, format!("/{}/{}", self.bucket, key))) }
        else { Ok((format!("{}.{}", self.bucket, host), format!("/{key}"))) }
    }
}

/// Signs requests to one bucket. Cheap to make and to clone.
pub struct Client { store: Store, secret: String, agent: ureq::Agent }

impl Client {
    pub fn new(store: Store, secret: String) -> Result<Self, String> {
        store.scheme_host()?;
        Ok(Self { store, secret, agent: crate::labels::agent() })
    }

    pub fn store(&self) -> &Store { &self.store }

    fn url(&self, host: &str, uri: &str) -> Result<String, String> {
        let (scheme, _) = self.store.scheme_host()?;
        Ok(format!("{scheme}://{host}{uri}"))
    }

    /// Uploads one object. `content_type` is signed and sent.
    pub fn put(&self, key: &str, bytes: &[u8], content_type: &str) -> Result<(), String> {
        let (amzdate, date) = amz_time(now_secs());
        let (host, path) = self.store.target(key)?;
        let uri = uri_encode(&path, true);
        let payload = sha256_hex(bytes);
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_string(), content_type.to_string());
        headers.insert("host".to_string(), host.clone());
        headers.insert("x-amz-content-sha256".to_string(), payload.clone());
        headers.insert("x-amz-date".to_string(), amzdate.clone());
        let ctx = Context { access: &self.store.access_key, secret: &self.secret, region: &self.store.region, amzdate: &amzdate, date: &date };
        let auth = authorization("PUT", &uri, "", &headers, &payload, &ctx);
        let response = self.agent.put(&self.url(&host, &uri)?)
            .header("Authorization", &auth)
            .header("Content-Type", content_type)
            .header("x-amz-content-sha256", &payload)
            .header("x-amz-date", &amzdate)
            .send(bytes)
            .map_err(|e| format!("The upload failed: {}", http_message(e)))?;
        check(response.status().as_u16(), "upload")
    }

    /// Removes one object. A missing object is not an error.
    pub fn delete(&self, key: &str) -> Result<(), String> {
        let (amzdate, date) = amz_time(now_secs());
        let (host, path) = self.store.target(key)?;
        let uri = uri_encode(&path, true);
        let payload = sha256_hex(b"");
        let mut headers = BTreeMap::new();
        headers.insert("host".to_string(), host.clone());
        headers.insert("x-amz-content-sha256".to_string(), payload.clone());
        headers.insert("x-amz-date".to_string(), amzdate.clone());
        let ctx = Context { access: &self.store.access_key, secret: &self.secret, region: &self.store.region, amzdate: &amzdate, date: &date };
        let auth = authorization("DELETE", &uri, "", &headers, &payload, &ctx);
        let response = self.agent.delete(&self.url(&host, &uri)?)
            .header("Authorization", &auth)
            .header("x-amz-content-sha256", &payload)
            .header("x-amz-date", &amzdate)
            .call()
            .map_err(|e| format!("The delete failed: {}", http_message(e)))?;
        if response.status().as_u16() == 404 { return Ok(()); }
        check(response.status().as_u16(), "delete")
    }

    /// A URL the provider can fetch for `expires` seconds. The URL carries
    /// its own signature, so the bucket can stay private.
    pub fn presigned_get(&self, key: &str, expires: u64) -> Result<String, String> {
        let (amzdate, date) = amz_time(now_secs());
        let (host, path) = self.store.target(key)?;
        let uri = uri_encode(&path, true);
        let mut params = BTreeMap::new();
        params.insert("X-Amz-Algorithm".to_string(), "AWS4-HMAC-SHA256".to_string());
        params.insert("X-Amz-Credential".to_string(),
            format!("{}/{date}/{}/s3/aws4_request", self.store.access_key, self.store.region));
        params.insert("X-Amz-Date".to_string(), amzdate.clone());
        params.insert("X-Amz-Expires".to_string(), expires.to_string());
        params.insert("X-Amz-SignedHeaders".to_string(), "host".to_string());
        let query = canonical_query(&params);
        let mut headers = BTreeMap::new();
        headers.insert("host".to_string(), host.clone());
        let ctx = Context { access: &self.store.access_key, secret: &self.secret, region: &self.store.region, amzdate: &amzdate, date: &date };
        let signature = signature("GET", &uri, &query, &headers, "UNSIGNED-PAYLOAD", &ctx);
        Ok(format!("{}?{query}&X-Amz-Signature={signature}", self.url(&host, &uri)?))
    }
}

fn check(status: u16, action: &str) -> Result<(), String> {
    if (200..300).contains(&status) { Ok(()) } else { Err(format!("The {action} returned HTTP {status}.")) }
}

fn http_message(error: ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(code) => format!("HTTP {code}"),
        other => other.to_string(),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// The signing material shared by every request to one bucket.
struct Context<'a> { access: &'a str, secret: &'a str, region: &'a str, amzdate: &'a str, date: &'a str }

/// The request signature and its date, for one request.
fn authorization(method: &str, uri: &str, query: &str, headers: &BTreeMap<String, String>, payload_hash: &str, ctx: &Context) -> String {
    let (block, signed) = canonical_headers(headers);
    let signature = signature_of(method, uri, query, &block, &signed, payload_hash, ctx);
    let scope = format!("{}/{}/s3/aws4_request", ctx.date, ctx.region);
    format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}", ctx.access)
}

/// The bare signature, as a presigned URL carries it.
fn signature(method: &str, uri: &str, query: &str, headers: &BTreeMap<String, String>, payload_hash: &str, ctx: &Context) -> String {
    let (block, signed) = canonical_headers(headers);
    signature_of(method, uri, query, &block, &signed, payload_hash, ctx)
}

fn signature_of(method: &str, uri: &str, query: &str, block: &str, signed: &str, payload_hash: &str, ctx: &Context) -> String {
    let canonical = format!("{method}\n{uri}\n{query}\n{block}\n{signed}\n{payload_hash}");
    let scope = format!("{}/{}/s3/aws4_request", ctx.date, ctx.region);
    let to_sign = format!("AWS4-HMAC-SHA256\n{}\n{scope}\n{}", ctx.amzdate, sha256_hex(canonical.as_bytes()));
    hex(&hmac(&signing_key(ctx.secret, ctx.date, ctx.region), &to_sign))
}

fn canonical_headers(headers: &BTreeMap<String, String>) -> (String, String) {
    let mut block = String::new();
    let mut names = Vec::new();
    for (name, value) in headers {
        let name = name.to_ascii_lowercase();
        block.push_str(&format!("{name}:{}\n", value.trim()));
        names.push(name);
    }
    (block, names.join(";"))
}

fn canonical_query(params: &BTreeMap<String, String>) -> String {
    params.iter().map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false))).collect::<Vec<_>>().join("&")
}

fn signing_key(secret: &str, date: &str, region: &str) -> [u8; 32] {
    let key = hmac(format!("AWS4{secret}").as_bytes(), date);
    let key = hmac(&key, region);
    let key = hmac(&key, "s3");
    hmac(&key, "aws4_request")
}

fn hmac(key: &[u8], data: &str) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts a key of any size");
    mac.update(data.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&mac.finalize().into_bytes());
    out
}

fn sha256_hex(data: &[u8]) -> String { hex(&Sha256::digest(data)) }

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes { out.push_str(&format!("{byte:02x}")); }
    out
}

/// Percent-encode as SigV4 wants: unreserved characters stay, and `/` stays
/// only where the path holds it.
fn uri_encode(text: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The AWS request time and its date, from Unix seconds.
fn amz_time(seconds: u64) -> (String, String) {
    let (year, month, day, hour, minute, second) = civil(seconds);
    (format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z"), format!("{year:04}{month:02}{day:02}"))
}

/// Civil date from Unix seconds (Howard Hinnant's algorithm).
fn civil(seconds: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (seconds / 86_400) as i64;
    let rem = seconds % 86_400;
    let (hour, minute, second) = ((rem / 3_600) as u32, ((rem % 3_600) / 60) as u32, (rem % 60) as u32);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day, hour, minute, second)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCESS: &str = "AKIAIOSFODNN7EXAMPLE";
    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    const DATE: &str = "20130524";
    const AMZ: &str = "20130524T000000Z";

    /// The signed GET in the AWS "Signature Version 4 test suite" example.
    #[test]
    fn a_signed_request_matches_the_aws_example() {
        let mut headers = BTreeMap::new();
        headers.insert("host".to_string(), "examplebucket.s3.amazonaws.com".to_string());
        headers.insert("range".to_string(), "bytes=0-9".to_string());
        headers.insert("x-amz-content-sha256".to_string(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string());
        headers.insert("x-amz-date".to_string(), AMZ.to_string());
        let ctx = Context { access: ACCESS, secret: SECRET, region: "us-east-1", amzdate: AMZ, date: DATE };
        let auth = authorization("GET", "/test.txt", "", &headers,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", &ctx);
        assert_eq!(auth, "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
            SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
            Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41");
    }

    /// The presigned GET in the AWS "using query parameters" example.
    #[test]
    fn a_presigned_url_matches_the_aws_example() {
        let mut params = BTreeMap::new();
        params.insert("X-Amz-Algorithm".to_string(), "AWS4-HMAC-SHA256".to_string());
        params.insert("X-Amz-Credential".to_string(), format!("{ACCESS}/{DATE}/us-east-1/s3/aws4_request"));
        params.insert("X-Amz-Date".to_string(), AMZ.to_string());
        params.insert("X-Amz-Expires".to_string(), "86400".to_string());
        params.insert("X-Amz-SignedHeaders".to_string(), "host".to_string());
        let query = canonical_query(&params);
        let mut headers = BTreeMap::new();
        headers.insert("host".to_string(), "examplebucket.s3.amazonaws.com".to_string());
        let ctx = Context { access: ACCESS, secret: SECRET, region: "us-east-1", amzdate: AMZ, date: DATE };
        let signature = signature("GET", "/test.txt", &query, &headers, "UNSIGNED-PAYLOAD", &ctx);
        assert_eq!(signature, "aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404");
        assert_eq!(query, "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host");
    }

    #[test]
    fn host_and_path_style_pick_the_right_target() {
        let mut store = Store { endpoint: "https://s3.us-east-1.amazonaws.com".into(), region: "us-east-1".into(),
            bucket: "examplebucket".into(), access_key: ACCESS.into(), path_style: false };
        assert_eq!(store.target("tiles/a b.png").unwrap(),
            ("examplebucket.s3.us-east-1.amazonaws.com".into(), "/tiles/a b.png".into()));
        store.path_style = true; store.endpoint = "http://127.0.0.1:9000".into();
        assert_eq!(store.target("/tiles/a b.png").unwrap(), ("127.0.0.1:9000".into(), "/examplebucket/tiles/a b.png".into()));
        assert_eq!(uri_encode("/tiles/a b.png", true), "/tiles/a%20b.png");
    }

    #[test]
    fn the_date_converts_from_unix_seconds() {
        assert_eq!(amz_time(1_369_353_600), (AMZ.to_string(), DATE.to_string()));
        assert_eq!(amz_time(0), ("19700101T000000Z".to_string(), "19700101".to_string()));
    }
}
