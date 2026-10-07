// SPDX-License-Identifier: GPL-3.0-only
//! Model discovery never changes a saved model choice. Provider metadata describes
//! compatibility, not the quality of captions for sprite sheets.
use super::{Ai, Keys, Kind, Model, Provider, Role, DEFAULT_CONCURRENCY};
use eframe::egui;
use serde_json::Value;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Task { #[default] Tags, Search, Text, Batch }


#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Entry {
    id: String,
    name: String,
    tags: bool,
    search: bool,
    text: bool,
    unknown: bool,
    input_price: Option<f64>,
    #[serde(default)]
    output_price: Option<f64>,
    #[serde(default)]
    created: u64,
    #[serde(default)]
    generation: bool,
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

fn parse(value: &Value, kind: Kind, embeddings: bool) -> Result<Vec<Entry>, String> {
    let field = if kind == Kind::Gemini { "models" } else { "data" };
    let list = value.get(field).and_then(Value::as_array).ok_or("The provider returned no model list.")?;
    let mut entries = Vec::new();
    for item in list {
        let raw_id = item.get(if kind == Kind::Gemini { "name" } else { "id" }).and_then(Value::as_str).unwrap_or("");
        let id = if kind == Kind::Gemini { raw_id.strip_prefix("models/").unwrap_or(raw_id) } else { raw_id };
        if id.trim().is_empty() || id.len() > 512 { continue; }
        let name = item.get(if kind == Kind::Gemini { "displayName" } else { "name" }).and_then(Value::as_str).unwrap_or(id);
        let (tags, search, text, unknown) = if kind == Kind::Gemini {
            let methods = strings(item, "supportedGenerationMethods");
            // The models endpoint does not report image input. Keep these in
            // the unverified list until the user tries a sheet.
            (false, false, false, methods.iter().any(|m| m == "generateContent"))
        } else {
            let inputs = strings(&item["architecture"], "input_modalities");
            let outputs = strings(&item["architecture"], "output_modalities");
            let image = inputs.iter().any(|x| x == "image");
            let text = inputs.iter().any(|x| x == "text");
            let tags = image && outputs.iter().any(|x| x == "text") && !embeddings;
            let search = image && text && (embeddings || outputs.iter().any(|x| x == "embeddings"));
            (tags, search, text && (embeddings || outputs.iter().any(|x| x == "embeddings")), inputs.is_empty() || outputs.is_empty())
        };
        let input_price = item["pricing"]["prompt"].as_str().and_then(|s| s.parse::<f64>().ok())
            .or_else(|| item["pricing"]["prompt"].as_f64()).filter(|p| p.is_finite() && *p >= 0.0);
        let output_price = item["pricing"]["completion"].as_str().and_then(|s| s.parse::<f64>().ok())
            .or_else(|| item["pricing"]["completion"].as_f64()).filter(|p| p.is_finite() && *p >= 0.0);
        let created = item.get("created").and_then(Value::as_u64).unwrap_or(0);
        let generation = kind == Kind::Gemini && strings(item, "supportedGenerationMethods").iter().any(|m| m == "generateContent");
        entries.push(Entry { id: id.to_owned(), name: name.to_owned(), tags, search, text, unknown, input_price, output_price, created, generation });
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    entries.dedup_by(|a, b| a.id == b.id);
    Ok(entries)
}

fn compatible(entry: &Entry, task: Task) -> bool {
    match task { Task::Tags | Task::Batch => entry.tags || entry.generation, Task::Search => entry.search, Task::Text => entry.text }
}

fn free(entry: &Entry) -> bool {
    entry.id.ends_with(":free") || (entry.input_price == Some(0.0) && entry.output_price.is_none_or(|price| price == 0.0))
}
fn preview(entry: &Entry) -> bool {
    let id = entry.id.to_lowercase();
    ["preview", "experimental", "-exp", ":extended"].iter().any(|part| id.contains(part))
}
fn suggestions(entries: &[Entry], task: Task) -> Vec<&Entry> {
    let mut candidates: Vec<_> = entries.iter().filter(|e| compatible(e, task) && !free(e))
        .filter(|e| !e.generation || !["tts", "audio", "live", "image", "robotics"].iter().any(|part| e.id.contains(part))).collect();
    // Prefer stable recent entries. A release date does not establish quality.
    candidates.sort_by(|a, b| preview(a).cmp(&preview(b)).then(b.created.cmp(&a.created)).then(b.id.cmp(&a.id)));
    let mut picks = Vec::new();
    for entry in &candidates {
        let author = entry.id.split('/').next().unwrap_or("");
        if !picks.iter().any(|e: &&Entry| e.id.split('/').next().unwrap_or("") == author) { picks.push(*entry); }
        if picks.len() == 3 { return picks; }
    }
    for entry in candidates {
        if !picks.iter().any(|e| e.id == entry.id) { picks.push(entry); }
        if picks.len() == 3 { break; }
    }
    picks
}

fn fetch(provider: &Provider, key: Option<&str>) -> Result<Vec<Entry>, String> {
    let base = crate::labels::checked_url(&provider.url)?;
    #[cfg(test)]
    let base = if let Ok(endpoint) = std::env::var("TILEPICKY_TEST_TRANSPORT") {
        let uri: ureq::http::Uri = endpoint.parse().map_err(|_| "Invalid test endpoint.")?;
        if uri.scheme_str() != Some("http") || uri.host() != Some("127.0.0.1") {
            return Err("Test transport must use loopback.".into());
        }
        endpoint
    } else { base };
    let client = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(12)))
        .max_redirects(0).build().new_agent();
    let mut entries = Vec::new();
    let paths: &[(&str, bool)] = if provider.is_openrouter() { &[("models", false), ("embeddings/models", true)] }
        else { &[("models", false)] };
    let started = Instant::now();
    for (path, embeddings) in paths {
        let mut page = String::new();
        for _ in 0..20 {
            if started.elapsed() > Duration::from_secs(30) { return Err("The model catalogue took too long to load. Try again.".into()); }
            let mut request = client.get(format!("{}/{path}", base.trim_end_matches('/')));
            if provider.kind == Kind::Gemini {
                request = request.query("pageSize", "1000");
                if !page.is_empty() { request = request.query("pageToken", &page); }
            }
            if let Some(key) = key {
                request = if provider.kind == Kind::Gemini { request.header("x-goog-api-key", key) }
                    else { request.header("Authorization", format!("Bearer {key}")) };
            }
            // Do not show response bodies or transport URLs: either may echo credentials.
            let mut response = request.call().map_err(|error| match error {
                ureq::Error::StatusCode(code @ (401 | 403)) => format!("The catalogue request failed (HTTP {code}). Check the API key and account access."),
                ureq::Error::StatusCode(code) => format!("The catalogue request failed (HTTP {code}). Try again later or use a custom model ID."),
                _ => "Could not connect to the model catalogue. Check the provider address and try again.".into(),
            })?;
            let value: Value = response.body_mut().with_config().limit(8 * 1024 * 1024).read_json()
                .map_err(|_| "The provider returned an invalid or oversized catalogue.")?;
            entries.extend(parse(&value, provider.kind, *embeddings)?);
            if entries.len() > 20_000 { return Err("The model catalogue is too large. Use a custom model ID.".into()); }
            if provider.kind != Kind::Gemini { break; }
            page = value.get("nextPageToken").and_then(Value::as_str).unwrap_or("").to_owned();
            if page.is_empty() { break; }
        }
        if !page.is_empty() { return Err("The provider catalogue has too many pages. Use a custom model ID.".into()); }
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    entries.dedup_by(|a, b| a.id == b.id);
    Ok(entries)
}

#[derive(Default)]
struct Cache {
    identity: String,
    entries: Vec<Entry>,
    error: Option<String>,
    running: bool,
    updated: Option<u64>,
    attempted: Option<Instant>,
}
#[derive(Serialize, Deserialize)]
struct DiskCache { updated: u64, entries: Vec<Entry> }
fn cache_path(identity: &str) -> Option<std::path::PathBuf> {
    crate::settings::dir().map(|path| path.join("model-catalogues").join(format!("{:x}.json", Sha256::digest(format!("v2:{identity}").as_bytes()))))
}
fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
fn read_cache(path: &std::path::Path) -> Option<DiskCache> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(8 * 1024 * 1024).read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[derive(Clone, Default)]
struct View {
    cache: Arc<Mutex<Cache>>,
    busy: Arc<AtomicBool>,
    provider: String,
    include_free: bool,
    filter: String,
    custom: String,
    all: bool,
    unknown: bool,
    notice: Option<String>,
}

fn supports_task(provider: &Provider, task: Task) -> bool {
    match task {
        Task::Tags => true,
        Task::Batch => provider.kind == Kind::Gemini || provider.is_openrouter(),
        Task::Search | Task::Text => provider.kind == Kind::OpenAi,
    }
}

fn price_summary(entry: &Entry) -> String {
    let price = |value: Option<f64>| value.map_or_else(|| "not listed".to_owned(), |p| format!("${:.2}", p * 1_000_000.0));
    format!("Per million tokens: input {}, output {}", price(entry.input_price), price(entry.output_price))
}

fn select(ai: &mut Ai, provider: &str, id: &str, task: Task) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() { return Err("Enter a model ID.".into()); }
    let connection = ai.providers.iter().find(|p| p.name == provider).ok_or("Choose a provider first.")?;
    if task == Task::Batch && !supports_task(connection, task) {
        return Err("Provider batch jobs require Google or OpenRouter. OpenRouter also needs object storage in Connections.".into());
    }
    if matches!(task, Task::Search | Task::Text) && connection.kind != Kind::OpenAi {
        return Err("Search embeddings require an OpenAI-compatible embedding endpoint.".into());
    }
    let role = match task {
        Task::Tags | Task::Batch => Role::Chat { single: true, library: true, concurrency: DEFAULT_CONCURRENCY },
        Task::Search | Task::Text => Role::Embed,
    };
    let model = Model { provider: provider.to_owned(), id: id.to_owned(), role };
    let reference = model.reference();
    if let Some(existing) = ai.models.iter_mut().find(|m| m.is(&reference)) {
        if existing.is_embed() != matches!(task, Task::Search | Task::Text) {
            return Err("This model is already configured for another kind of task. Check its role in Advanced connections and models.".into());
        }
        if matches!(task, Task::Tags | Task::Batch) {
            let concurrency = existing.concurrency().unwrap_or(DEFAULT_CONCURRENCY);
            existing.role = Role::Chat { single: true, library: true, concurrency };
        } else { existing.role = role; }
    } else { ai.models.push(model); }
    match task {
        Task::Tags => { ai.single = Some(reference); },
        Task::Batch => ai.library = Some(reference),
        Task::Search => ai.embed = Some(reference),
        Task::Text => ai.text_embed = Some(reference),
    }
    Ok(())
}

pub(super) fn ui_for_task(ui: &mut egui::Ui, ai: &mut Ai, keys: &Keys, task: Task) {
    let state_id = egui::Id::new(("model catalogue view", format!("{task:?}")));
    let mut view = ui.data(|d| d.get_temp::<View>(state_id)).unwrap_or_default();
    if !ai.providers.iter().any(|p| p.name == view.provider && supports_task(p, task)) {
        let selected = match task { Task::Tags => &ai.single, Task::Batch => &ai.library, Task::Search => &ai.embed, Task::Text => &ai.text_embed };
        view.provider = selected.as_ref().filter(|m| ai.providers.iter().any(|p| p.name == m.provider && supports_task(p, task))).map(|m| m.provider.clone())
            .or_else(|| ai.providers.iter().find(|p| supports_task(p, task)).map(|p| p.name.clone())).unwrap_or_default();
    }
    ui.small("Models refresh hourly. Saved choices stay unchanged.");
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("catalogue provider").selected_text(&view.provider).show_ui(ui, |ui| {
            for provider in ai.providers.iter().filter(|p| supports_task(p, task)) {
                ui.selectable_value(&mut view.provider, provider.name.clone(), &provider.name);
            }
        });
    });
    if let Some(provider) = ai.providers.iter().find(|p| p.name == view.provider).cloned() {
        let identity = format!("{:?}:{}:{}", provider.kind, provider.name, provider.url);
        let mut cache = view.cache.lock().unwrap_or_else(|p| p.into_inner());
        // A new provider gets a new cache. An old worker can only write its old cache.
        if cache.identity != identity {
            drop(cache);
            view.cache = Arc::new(Mutex::new(Cache { identity, ..Cache::default() }));
            cache = view.cache.lock().unwrap_or_else(|p| p.into_inner());
        }
        let can_fetch = provider.kind != Kind::Gemini || !matches!(provider.key_source(keys), super::KeySource::None);
        let busy_now = view.busy.load(Ordering::Acquire);
        let refresh = ui.add_enabled(!busy_now && can_fetch, egui::Button::new("Refresh available models")).clicked();
        let stale = cache.attempted.is_none_or(|attempted| attempted.elapsed() >= Duration::from_secs(3600));
        let editing_key = ui.ctx().text_edit_focused();
        if !busy_now && can_fetch && (refresh || (stale && !editing_key && !cfg!(test))) {
            cache.attempted = Some(Instant::now());
            let disk_path = cache_path(&cache.identity);
            view.busy.store(true, Ordering::Release);
            let busy = view.busy.clone();
            cache.running = true;
            cache.error = None;
            let destination = view.cache.clone();
            let key = provider.key(keys);
            let ctx = ui.ctx().clone();
            let requested_provider = provider.clone();
            std::thread::spawn(move || {
                if let Some(saved) = disk_path.as_deref().and_then(read_cache) {
                    let mut cache = destination.lock().unwrap_or_else(|p| p.into_inner());
                    if cache.updated.is_none() { cache.entries = saved.entries; cache.updated = Some(saved.updated); }
                    ctx.request_repaint();
                }
                let result = fetch(&requested_provider, key.as_deref());
                let mut save_error = None;
                if let (Ok(entries), Some(path)) = (&result, disk_path) {
                    let saved = DiskCache { updated: now(), entries: entries.clone() };
                    if crate::storage::write_private(&path, &saved).is_err() {
                        save_error = Some("Models loaded, but the offline catalogue could not be saved.".into());
                    }
                }
                let mut cache = destination.lock().unwrap_or_else(|p| p.into_inner());
                cache.running = false;
                match result {
                    Ok(entries) => { cache.entries = entries; cache.updated = Some(now()); cache.error = save_error; }
                    Err(error) => cache.error = Some(error),
                }
                busy.store(false, Ordering::Release);
                ctx.request_repaint();
            });
        }
        if !can_fetch { ui.small("Enter this provider's API key above to load its catalogue."); }
        if cache.running { ui.spinner(); ui.label("Checking available models..."); ui.ctx().request_repaint_after(Duration::from_millis(100)); }
        if let Some(error) = &cache.error { ui.colored_label(ui.visuals().error_fg_color, error); }
        if let Some(updated) = cache.updated { ui.small(format!("Last checked {} minutes ago. Prices can change.", now().saturating_sub(updated) / 60)); }
        let mut chosen = None;
        if cache.updated.is_some() {
            let picks = suggestions(&cache.entries, task);
            ui.small("Starting points favor stable, recent releases from different authors. Free routes are excluded.")
                .on_hover_text("Release dates and model IDs determine the order, not quality. Free routes can have stricter usage limits.");
            if picks.is_empty() { ui.label("No verified matches. Choose a configured model or inspect unverified models below."); }
            if provider.kind == Kind::Gemini { ui.small("Google lists generation methods, but not image support. Try one sheet first."); }
            for entry in picks {
                let suffix = if entry.unknown { " (image support unverified)" } else { "" };
                if ui.button(format!("Use {}{suffix}", entry.name)).on_hover_text(&entry.id).clicked() { chosen = Some(entry.id.clone()); }
                ui.small(price_summary(entry));
            }
            ui.checkbox(&mut view.all, "Choose another model");
            if view.all {
                let allow_unknown = matches!(task, Task::Tags | Task::Batch) || provider.kind != Kind::Gemini;
                ui.checkbox(&mut view.include_free, "Include free routes");
                ui.add(egui::TextEdit::singleline(&mut view.filter).hint_text("Find a model"));
                if allow_unknown {
                    ui.checkbox(&mut view.unknown, "Include models with unverified capabilities");
                }
                let query = view.filter.to_lowercase();
                egui::ScrollArea::vertical().id_salt("catalogue matches").max_height(180.0).show(ui, |ui| {
                    for entry in cache.entries.iter().filter(|e| compatible(e, task) || (allow_unknown && view.unknown && e.unknown))
                        .filter(|e| view.include_free || !free(e))
                        .filter(|e| e.id.to_lowercase().contains(&query) || e.name.to_lowercase().contains(&query)) {
                        let suffix = if !entry.unknown { "" } else { " (unverified)" };
                        if ui.selectable_label(false, format!("{}{suffix}", entry.name)).on_hover_text(&entry.id).clicked() {
                            chosen = Some(entry.id.clone());
                        }
                    }
                });
                ui.small("Unverified models may not accept images or return the required result. Try one sheet first.");
            }
        }
        drop(cache);
        if let Some(id) = chosen {
            view.notice = Some(select(ai, &view.provider, &id, task)
                .map_or_else(|error| error, |()| format!("Selected {id}. This applies to new requests.")));
        }
    }
    egui::CollapsingHeader::new("Use a custom model ID").show(ui, |ui| {
        ui.add(egui::TextEdit::singleline(&mut view.custom).hint_text("Exact model ID from your provider"));
        let supported = ai.providers.iter().any(|p| p.name == view.provider && supports_task(p, task));
        if ui.add_enabled(supported && !view.custom.trim().is_empty(), egui::Button::new("Use this model")).clicked() {
            view.notice = Some(select(ai, &view.provider, &view.custom, task)
                .map_or_else(|error| error, |()| format!("Selected {}. Try one sheet before a library job.", view.custom.trim())));
        }
        if !supported { ui.small("Add a compatible provider in Connections first."); }
        ui.small(match task {
            Task::Tags | Task::Batch => "Custom models are unverified. Caption models must accept images and return structured JSON labels.",
            Task::Search => "Custom models are unverified. Image search needs a model that embeds images and text in the same space.",
            Task::Text => "Custom models are unverified. Caption search needs a model that embeds text.",
        });
    });
    if let Some(notice) = &view.notice { ui.label(notice); }
    ui.data_mut(|d| d.insert_temp(state_id, view));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn choosing_text_embeddings_does_not_change_image_or_tag_models() {
        let mut ai = Ai::default();
        let original_image = ai.embed.clone();
        let original_tags = ai.single.clone();
        select(&mut ai, "OpenRouter", "text-only", Task::Text).unwrap();
        assert_eq!(ai.embed, original_image);
        assert_eq!(ai.single, original_tags);
        assert_eq!(ai.chosen_text_embed().unwrap().1.id, "text-only");
        assert!(select(&mut ai, "OpenRouter", "text-only", Task::Tags).is_err());
        assert_eq!(ai.single, original_tags);
    }

    #[test]
    fn choosing_batch_preserves_caption_model_and_concurrency() {
        let mut ai = Ai::default();
        let caption = ai.single.clone();
        select(&mut ai, "Google", "gemini-batch", Task::Batch).unwrap();
        assert_eq!(ai.single, caption);
        assert_eq!(ai.library.as_ref().unwrap().model, "gemini-batch");
    }

    #[test]
    fn shortlist_prefers_stable_recent_models_from_different_authors() {
        let mut value = json!({"data": [
            {"id":"a/free:free", "created":999, "pricing":{"prompt":"0"}},
            {"id":"a/new", "created":100}, {"id":"a/other", "created":99},
            {"id":"b/new", "created":98}, {"id":"c/old", "created":97},
            {"id":"d/preview", "created":200}
        ]});
        for entry in value["data"].as_array_mut().unwrap() {
            entry["architecture"] = json!({"input_modalities":["image"],"output_modalities":["text"]});
        }
        let entries = parse(&value, Kind::OpenAi, false).unwrap();
        assert_eq!(suggestions(&entries, Task::Tags).iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["a/new", "b/new", "c/old"]);
    }

    #[test]
    fn provider_filters_and_selection_reject_unsupported_batch_endpoints() {
        let mut ai = Ai::default();
        let provider = Provider::new("Custom", Kind::OpenAi);
        assert!(!supports_task(&provider, Task::Batch));
        assert!(supports_task(&provider, Task::Search));
        assert!(supports_task(&provider, Task::Text));
        let google = Provider::new("Google", Kind::Gemini);
        assert!(supports_task(&google, Task::Batch));
        assert!(!supports_task(&google, Task::Search));
        assert!(!supports_task(&google, Task::Text));
        ai.providers.push(provider);
        let original = ai.library.clone();
        assert!(select(&mut ai, "Custom", "model", Task::Batch).unwrap_err().contains("require Google or OpenRouter"));
        assert_eq!(ai.library, original);
        assert!(!ai.models.iter().any(|m| m.provider == "Custom"));
    }

    #[test]
    fn prices_use_million_token_units_and_report_missing_values() {
        let entries = parse(&json!({"data":[{"id":"priced", "pricing":{"prompt":"0.000003", "completion":"0.000015"}},
            {"id":"unknown"}]}), Kind::OpenAi, false).unwrap();
        assert_eq!(price_summary(&entries[0]), "Per million tokens: input $3.00, output $15.00");
        assert!(price_summary(&entries[1]).contains("not listed"));
    }

    #[test]
    fn offline_cache_retains_capabilities_and_timestamp() {
        let folder = crate::storage::tests::Folder::new();
        let path = folder.0.join("catalogue.json");
        let entries = parse(&json!({"data":[{"id":"text", "architecture":{
            "input_modalities":["text"], "output_modalities":["embeddings"]}}]}), Kind::OpenAi, true).unwrap();
        let saved = DiskCache { updated: 123, entries: entries.clone() };
        crate::storage::write_private(&path, &saved).unwrap();
        let loaded = read_cache(&path).unwrap();
        assert_eq!(loaded.updated, 123);
        assert_eq!(loaded.entries, entries);
        assert!(!compatible(&loaded.entries[0], Task::Search));
        assert!(compatible(&loaded.entries[0], Task::Text));
        std::fs::write(&path, b"broken").unwrap();
        assert!(read_cache(&path).is_none());
    }

    #[test]
    fn capability_filters_exclude_irrelevant_models() {
        let entries = parse(&json!({"data": [
            {"id":"vision", "architecture":{"input_modalities":["text","image"],"output_modalities":["text"]}},
            {"id":"audio", "architecture":{"input_modalities":["audio"],"output_modalities":["text"]}},
            {"id":"paint", "architecture":{"input_modalities":["text"],"output_modalities":["image"]}},
            {"id":"vectors", "architecture":{"input_modalities":["text","image"],"output_modalities":["embeddings"]}},
            {"id":"text-vectors", "architecture":{"input_modalities":["text"],"output_modalities":["embeddings"]}},
            {"id":"unknown"}
        ]}), Kind::OpenAi, false).unwrap();
        assert_eq!(suggestions(&entries, Task::Tags).iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["vision"]);
        assert_eq!(suggestions(&entries, Task::Search).iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["vectors"]);
        assert!(entries.iter().find(|e| e.id == "unknown").unwrap().unknown);
    }
    #[test]
    fn gemini_generation_method_does_not_prove_image_support() {
        let entries = parse(&json!({"models":[
            {"name":"models/gemini-example", "supportedGenerationMethods":["generateContent"]},
            {"name":"models/embed", "supportedGenerationMethods":["embedContent"]}
        ]}), Kind::Gemini, false).unwrap();
        assert!(entries.iter().find(|e| e.id == "gemini-example").unwrap().unknown);
        assert_eq!(suggestions(&entries, Task::Tags)[0].id, "gemini-example");
        assert!(!entries.iter().find(|e| e.id == "embed").unwrap().unknown);
    }
    #[test]
    fn choosing_tags_preserves_batch_and_search_models() {
        let mut ai = Ai::default();
        let search = ai.embed.clone();
        let batch = ai.library.clone();
        select(&mut ai, "OpenRouter", "new-vision", Task::Tags).unwrap();
        assert_eq!(ai.library, batch);
        assert_eq!(ai.single.as_ref().unwrap().model, "new-vision");
        assert_eq!(ai.embed, search);
        select(&mut ai, "OpenRouter", "new-vision", Task::Tags).unwrap();
        assert_eq!(ai.models.iter().filter(|m| m.id == "new-vision").count(), 1);
    }
    #[test]
    fn shortlist_is_bounded_and_prices_are_validated() {
        let entries = parse(&json!({"data": (0..6).map(|i| json!({"id":format!("m{i}"), "pricing":{"prompt":format!("{}", i)},
            "architecture":{"input_modalities":["image"],"output_modalities":["text"]}})).collect::<Vec<_>>() }), Kind::OpenAi, false).unwrap();
        assert_eq!(suggestions(&entries, Task::Tags).iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["m5", "m4", "m3"]);
        assert!(parse(&json!({"error":"bad"}), Kind::OpenAi, false).is_err());
    }
}
