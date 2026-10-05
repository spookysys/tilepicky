// SPDX-License-Identifier: GPL-3.0-only
//! The AI providers and models the tool may call, and the settings page
//! that edits them. A provider is an endpoint of one of two kinds: an
//! OpenAI-style chat endpoint (OpenAI, OpenRouter), or Google's Gemini API.
//! A model names its provider; the stored `:batch` suffix marks library scope.
//! API keys live in a separate file, readable by the owner alone; see `Keys`.

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The stored suffix for library scope. It does not select a provider API.
const BATCH: &str = ":batch";

/// The kind of endpoint a provider speaks.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Chat completions as OpenAI defines them; OpenRouter speaks them too.
    OpenAi,
    /// Google's Gemini API.
    Gemini,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::OpenAi => "OpenAI endpoint",
            Kind::Gemini => "Gemini endpoint",
        }
    }

    /// The URL and the environment variables a new provider of this kind
    /// starts with.
    fn defaults(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Kind::OpenAi => ("https://api.openai.com/v1", &["OPENAI_API_KEY"]),
            Kind::Gemini => ("https://generativelanguage.googleapis.com/v1beta", &["GOOGLE_API_KEY", "GEMINI_API_KEY"]),
        }
    }
}

/// The scope of a model: one sheet or a library job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Instant,
    Batch,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Instant => "Single sheet",
            Mode::Batch => "Library",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Provider {
    pub name: String,
    pub kind: Kind,
    pub url: String,
    /// The environment variables that may hold the key. The first one that
    /// is set wins.
    #[serde(default)]
    pub key_env: Vec<String>,
    /// None uses the OpenRouter default. An empty list explicitly skips nobody.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<Vec<String>>,
    /// Where to upload sheets so a batch can fetch them as public URLs. Only
    /// OpenAI-style providers use it, and only for a library job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<crate::s3::Store>,
}

/// The `keys.json` name for a provider's object-storage secret. It is separate
/// from the provider key, so an empty one is dropped on save.
pub fn s3_secret_name(provider: &str) -> String { format!("{provider} (S3 secret)") }

/// Where a provider's key comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// Typed into the settings.
    Typed,
    /// The named environment variable.
    Env(String),
    None,
}

impl Provider {
    fn update_kind_defaults(&mut self, old: Kind) {
        let (old_url, old_env) = old.defaults();
        let (url, env) = self.kind.defaults();
        if self.url == old_url { self.url = url.into(); }
        if self.key_env == old_env { self.key_env = env.iter().map(|s| s.to_string()).collect(); }
    }

    fn new(name: &str, kind: Kind) -> Self {
        let (url, env) = kind.defaults();
        Provider { name: name.into(), kind, skip: None, store: None, url: url.into(), key_env: env.iter().map(|s| s.to_string()).collect() }
    }

    pub fn is_openrouter(&self) -> bool {
        self.kind == Kind::OpenAi && crate::labels::checked_url(&self.url).ok()
            .and_then(|url| url.parse::<ureq::http::Uri>().ok())
            .is_some_and(|url| url.host().is_some_and(|host| host.eq_ignore_ascii_case("openrouter.ai")))
    }

    pub fn skipped(&self) -> Vec<String> {
        self.skip.clone().unwrap_or_else(|| if self.is_openrouter() { vec!["phala".into()] } else { vec![] })
    }

    /// Only OpenRouter receives its routing options.
    pub fn route(&self, mut body: serde_json::Value) -> serde_json::Value {
        if self.is_openrouter() {
            let skip = self.skipped();
            if !skip.is_empty() { body["provider"]["ignore"] = serde_json::json!(skip); }
        }
        body
    }

    /// Resolve the key only when the user starts a request. It comes from
    /// where `key_source` says it does.
    pub fn key(&self, keys: &Keys) -> Option<String> {
        match self.key_source(keys) {
            KeySource::Typed => keys.get(&self.name).map(str::to_string),
            KeySource::Env(name) => std::env::var(name).ok(),
            KeySource::None => None,
        }
    }

    /// The typed object-storage secret, if any. It never comes from the
    /// provider's key or its environment variables.
    pub fn store_secret(&self, keys: &Keys) -> Option<String> {
        keys.get(&s3_secret_name(&self.name)).map(str::to_string)
    }

    pub fn key_source(&self, keys: &Keys) -> KeySource {
        self.key_source_in(keys, |name| std::env::var(name).ok())
    }

    /// A typed key wins over the environment. Blanks are no key.
    fn key_source_in(&self, keys: &Keys, env: impl Fn(&str) -> Option<String>) -> KeySource {
        if keys.get(&self.name).is_some() {
            return KeySource::Typed;
        }
        match self.key_env.iter().find(|name| env(name).is_some_and(|v| !v.trim().is_empty())) {
            Some(name) => KeySource::Env(name.clone()),
            None => KeySource::None,
        }
    }
}

/// A model on one of the providers.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub provider: String,
    /// The provider model ID, with the internal `:batch` suffix for library scope.
    pub id: String,
}

impl Model {
    pub fn mode(&self) -> Mode {
        if self.id.ends_with(BATCH) { Mode::Batch } else { Mode::Instant }
    }

    /// Writes or removes the `:batch` end of the id.
    fn set_batch(&mut self, batch: bool) {
        match (batch, self.mode()) {
            (true, Mode::Instant) => self.id.push_str(BATCH),
            (false, Mode::Batch) => self.id.truncate(self.id.len() - BATCH.len()),
            _ => {}
        }
    }

    fn is(&self, r: &ModelRef) -> bool {
        r.provider == self.provider && r.model == self.id
    }

    fn reference(&self) -> ModelRef {
        ModelRef { provider: self.provider.clone(), model: self.id.clone() }
    }

    /// How a list names the model: its id, then its provider.
    fn label(&self) -> String {
        format!("{} ({}, {})", self.id.trim_end_matches(BATCH), self.provider, self.mode().label())
    }
}

/// One model, named by its provider and its id.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

/// The providers, the models on them, and which model answers an instant
/// request and which runs a batch.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Ai {
    #[serde(default)]
    pub providers: Vec<Provider>,
    #[serde(default)]
    pub models: Vec<Model>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instant: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<ModelRef>,
}

/// A fresh install offers OpenRouter and Google models, without keys.
/// Single-sheet requests use either provider format. Library jobs use several
/// chat requests at once or Google's Gemini batch endpoint.
impl Default for Ai {
    fn default() -> Self {
        let mut openrouter = Provider::new("OpenRouter", Kind::OpenAi);
        openrouter.url = "https://openrouter.ai/api/v1".into();
        openrouter.key_env = vec!["OPENROUTER_API_KEY".into()];
        let google = Provider::new("Google", Kind::Gemini);
        let on = |provider: &str, id: &str| Model { provider: provider.into(), id: id.into() };
        let models = vec![
            on("OpenRouter", "~deepseek/deepseek-flash-latest"),
            on("OpenRouter", "~deepseek/deepseek-flash-latest:batch"),
            on("OpenRouter", "z-ai/glm-5.3-flash"),
            on("OpenRouter", "z-ai/glm-5.3-flash:batch"),
            on("Google", "gemini-flash-latest:batch"),
            on("Google", "gemini-flash-latest"),
        ];
        let (instant, batch) = (Some(models[0].reference()), Some(models[1].reference()));
        Ai { providers: vec![openrouter, google], models, instant, batch }
    }
}

/// Models an older tool shipped and this one does not: `openrouter/free`
/// sent each request to whichever free model was up, and MiMo has no batch
/// endpoint on OpenRouter.
const RETIRED: [(&str, &str); 3] =
    [("OpenRouter", "openrouter/free"), ("OpenRouter", "xiaomi/mimo-v2.5"), ("OpenRouter", "xiaomi/mimo-v2.5:batch")];

impl Ai {
    /// Fills in what a settings file from an older tool lacks. A model
    /// without an id is dropped, and so is a model the tool once shipped
    /// and retired; the shipped models take its place. Without any model
    /// left, the shipped models come in, on the providers that exist; a
    /// choice that names no model falls back to the shipped one, when that
    /// exists.
    pub fn heal(&mut self) {
        let google = self.providers.iter().any(|p| p.name == "Google" && p.kind == Kind::Gemini
            && p.url.trim_end_matches('/') == "https://generativelanguage.googleapis.com/v1beta");
        if google {
            for model in self.models.iter_mut().filter(|m| m.provider == "Google") {
                if model.id == "gemini-3.7-flash" { model.id = "gemini-flash-latest".into(); }
                if model.id == "gemini-3.7-flash:batch" { model.id = "gemini-flash-latest:batch".into(); }
            }
            for slot in [&mut self.instant, &mut self.batch].into_iter().flatten().filter(|s| s.provider == "Google") {
                if slot.model == "gemini-3.7-flash" { slot.model = "gemini-flash-latest".into(); }
                if slot.model == "gemini-3.7-flash:batch" { slot.model = "gemini-flash-latest:batch".into(); }
            }
            let mut seen = std::collections::HashSet::new();
            self.models.retain(|m| seen.insert((m.provider.clone(), m.id.clone())));
        }
        let before = self.models.len();
        self.models.retain(|m| !m.id.is_empty() && !RETIRED.contains(&(m.provider.as_str(), m.id.as_str())));
        let retired = self.models.len() < before;
        let shipped = Ai::default();
        if self.models.is_empty() || retired {
            for m in shipped.models {
                if self.providers.iter().any(|p| p.name == m.provider) && !self.models.contains(&m) {
                    self.models.push(m);
                }
            }
        }
        if google && !self.models.iter().any(|m| m.provider == "Google" && m.id == "gemini-flash-latest") {
            self.models.push(Model { provider: "Google".into(), id: "gemini-flash-latest".into() });
        }
        for (mode, fallback) in [(Mode::Instant, shipped.instant), (Mode::Batch, shipped.batch)] {
            if self.chosen(mode).is_none() {
                let fallback = fallback.filter(|r| self.models.iter().any(|m| m.is(r) && m.mode() == mode));
                match mode {
                    Mode::Instant => self.instant = fallback,
                    Mode::Batch => self.batch = fallback,
                }
            }
        }
    }

    /// The model chosen for a mode, with its provider, while both exist.
    pub fn chosen(&self, mode: Mode) -> Option<(&Provider, &Model)> {
        let r = match mode {
            Mode::Instant => self.instant.as_ref()?,
            Mode::Batch => self.batch.as_ref()?,
        };
        let m = self.models.iter().find(|m| m.is(r) && m.mode() == mode)?;
        let p = self.providers.iter().find(|p| p.name == m.provider)?;
        Some((p, m))
    }
}

/// The typed API keys, one per provider name, in `keys.json` beside the
/// settings. The file is written readable by the owner alone.
#[derive(Default)]
pub struct Keys(BTreeMap<String, String>);

impl Keys {
    fn file() -> Option<PathBuf> {
        crate::settings::dir().map(|d| d.join("keys.json"))
    }

    /// The typed keys, and the error when the file could not be read. The
    /// tool then has no typed keys, and `save` refuses to overwrite the file
    /// until the user says yes; see `rewrite`.
    pub fn load() -> (Self, Option<String>) {
        Self::file().map_or_else(|| (Self::default(), None), |p| Self::load_from(&p))
    }

    fn load_from(path: &std::path::Path) -> (Self, Option<String>) {
        match crate::storage::read(path) {
            Ok(keys) => (Keys(keys), None),
            Err(e) => (Self::default(), Some(e)),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::file().ok_or("No configuration directory available.")?, false)
    }

    /// Writes the keys over a file that could not be read. The user agreed
    /// to lose what it held.
    pub fn rewrite(&self) -> Result<(), String> {
        self.save_to(&Self::file().ok_or("No configuration directory available.")?, true)
    }

    fn save_to(&self, path: &std::path::Path, overwrite: bool) -> Result<(), String> {
        if !overwrite {
            crate::storage::read::<BTreeMap<String, String>>(path)?;
        }
        let kept: BTreeMap<&String, &String> = self.0.iter().filter(|(_, v)| !v.trim().is_empty()).collect();
        crate::storage::write_private(path, &kept)
    }

    pub fn get(&self, provider: &str) -> Option<&str> {
        self.0.get(provider).map(String::as_str).filter(|k| !k.trim().is_empty())
    }

    /// The typed key of a provider, to edit; empty when there is none.
    fn entry(&mut self, provider: &str) -> &mut String {
        self.0.entry(provider.to_string()).or_default()
    }

    /// The key follows a provider that changes its name.
    fn rename(&mut self, old: &str, new: &str) {
        if let Some(v) = self.0.remove(old) { self.0.insert(new.to_string(), v); }
        if let Some(v) = self.0.remove(&s3_secret_name(old)) { self.0.insert(s3_secret_name(new), v); }
    }
}

/// The settings page: providers, configured models, and active models.
/// Each of the first two shows one item, chosen in a selector, so the page
/// stays short. Edits land in place; the caller writes both files when the
/// dialog closes.
pub fn settings_ui(ui: &mut egui::Ui, ai: &mut Ai, keys: &mut Keys) {
    let Ai { providers, models, instant, batch } = ai;
    ui.strong("Active models");
    ui.small("Used for new requests. Existing jobs keep their original models.");
    egui::Grid::new("defaults").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        for (mode, slot) in [(Mode::Instant, &mut *instant), (Mode::Batch, &mut *batch)] {
            ui.label(mode.label());
            let current = slot.as_ref().and_then(|r| models.iter().find(|m| m.is(r) && m.mode() == mode));
            let active_name = |m: &Model| format!("{} ({})", m.id.trim_end_matches(BATCH), m.provider);
            let text = current.map_or("none".to_string(), active_name);
            egui::ComboBox::from_id_salt(("default", mode.label())).selected_text(text).show_ui(ui, |ui| {
                for m in models.iter().filter(|m| m.mode() == mode) {
                    ui.selectable_value(slot, Some(m.reference()), active_name(m));
                }
            });
            ui.end_row();
        }
    });
    let mut configure = ui.data_mut(|d| d.remove_temp::<bool>(egui::Id::new("configure Gemini"))).unwrap_or(false);
    if let Some(selected) = batch.as_ref().and_then(|r| models.iter().find(|m| m.is(r)))
        && let Some(provider) = providers.iter().find(|p| p.name == selected.provider) {
        ui.add_space(4.0);
        if provider.kind == Kind::OpenAi { configure |= endpoint_notice(ui); }
        else {
            ui.strong("Google batch");
            ui.label("Google labels uploaded sheets together. After upload, you can close Tilepicky while Google works.");
            ui.label("Reopen this library to collect the labels. Sheets not yet uploaded wait until you return.");
        }
    }
    let focus = if configure {
        let (provider, model) = gemini_setup(providers, models);
        show(ui, "provider", provider); show(ui, "model", model);
        Some(provider)
    } else { None };
    ui.separator();
    ui.strong("API keys");
    ui.label("Add a key for each provider you want to use. The built-in models are already configured.");
    egui::Grid::new("provider keys").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        for (i, provider) in providers.iter().enumerate() {
            ui.label(&provider.name);
            let field = ui.add(egui::TextEdit::singleline(keys.entry(&provider.name)).password(true)
                .id(egui::Id::new(("provider key", &provider.name))).hint_text("API key").desired_width(f32::INFINITY));
            if focus == Some(i) { field.request_focus(); field.scroll_to_me(None); }
            ui.end_row();
            ui.label("");
            ui.small(match provider.key_source(keys) {
                KeySource::Typed => "Key entered".to_string(),
                KeySource::Env(name) => format!("Using environment variable {name}"),
                KeySource::None => "No key configured".to_string(),
            });
            ui.end_row();
        }
    });
    ui.add_space(6.0);
    egui::CollapsingHeader::new("Provider and model setup").show(ui, |ui| {
        ui.label("Use this section to add custom models or providers, or change their connection settings.");
        ui.small("A provider is the service you connect to. A model is the AI you choose from that service.");
        ui.strong("Providers");
        providers_ui(ui, providers, models, instant, batch, keys);
        ui.separator();
        ui.strong("Models");
        if models_ui(ui, providers, models, instant, batch) {
            ui.data_mut(|d| d.insert_temp(egui::Id::new("configure Gemini"), true));
            ui.ctx().request_repaint();
        }
    });

}

/// Explain the library execution method where the user chooses it.
fn endpoint_notice(ui: &mut egui::Ui) -> bool {
    ui.strong("Several sheets at a time");
    ui.label("Keep Tilepicky open while it labels the library. Closing Tilepicky pauses the job.");
    ui.label("Reopen this library to continue.");
    ui.small("Want processing to continue while Tilepicky is closed? Add a Google key and choose Gemini for Library.");
    ui.button("Set up Gemini...").clicked()
}

/// Prepare the editor without changing either active model or sending a request.
fn gemini_setup(providers: &mut Vec<Provider>, models: &mut Vec<Model>) -> (usize, usize) {
    let provider = providers.iter().position(|p| p.kind == Kind::Gemini).unwrap_or_else(|| {
        let name = (1..).map(|n| if n == 1 { "Google".to_string() } else { format!("Google {n}") })
            .find(|name| providers.iter().all(|p| &p.name != name)).unwrap();
        providers.push(Provider::new(&name, Kind::Gemini)); providers.len() - 1
    });
    let name = &providers[provider].name;
    let model = models.iter().position(|m| &m.provider == name && m.mode() == Mode::Batch).unwrap_or_else(|| {
        models.push(Model { provider: name.clone(), id: "gemini-flash-latest:batch".into() }); models.len() - 1
    });
    (provider, model)
}

/// Which item a section shows. egui remembers it while the tool runs; the
/// index is kept inside the list.
fn shown(ui: &egui::Ui, what: &str, len: usize) -> usize {
    let i: usize = ui.data(|d| d.get_temp(egui::Id::new(("ai settings", what)))).unwrap_or(0);
    i.min(len.saturating_sub(1))
}

fn show(ui: &egui::Ui, what: &str, i: usize) {
    ui.data_mut(|d| d.insert_temp(egui::Id::new(("ai settings", what)), i));
}

/// The selector of a section, with its Add and Remove buttons. Returns the
/// item shown, and whether it is to be removed.
fn selector(ui: &mut egui::Ui, what: &str, names: Vec<String>, add: &mut dyn FnMut()) -> (usize, bool) {
    let mut sel = shown(ui, what, names.len());
    let mut remove = false;
    ui.horizontal(|ui| {
        let text = names.get(sel).cloned().unwrap_or_else(|| "none".to_string());
        egui::ComboBox::from_id_salt(what).selected_text(text).width(320.0).show_ui(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                ui.selectable_value(&mut sel, i, name);
            }
        });
        if ui.button("Add").clicked() {
            add();
            sel = names.len();
        }
        if !names.is_empty() && ui.button("Remove").clicked() {
            remove = true;
        }
    });
    let confirmation = ui.make_persistent_id(("remove configuration", what));
    if remove { ui.data_mut(|d| d.insert_temp(confirmation, sel)); }
    remove = false;
    if ui.data(|d| d.get_temp::<usize>(confirmation)) == Some(sel) && let Some(name) = names.get(sel) {
        ui.group(|ui| {
            ui.label(format!("Remove {name}?"));
            ui.label(if what == "provider" { "This also removes its models and clears their active selections." }
                else { "This clears any active selection that uses this model." });
            ui.horizontal(|ui| {
                remove = ui.button(format!("Remove {what}")).clicked();
                if remove || ui.button("Keep").clicked() { ui.data_mut(|d| { d.remove::<usize>(confirmation); }); }
            });
        });
    }
    show(ui, what, sel);
    (sel, remove)
}

/// One provider at a time: its endpoint and its key. A removed provider
/// takes its models with it.
fn providers_ui(
    ui: &mut egui::Ui,
    providers: &mut Vec<Provider>,
    models: &mut Vec<Model>,
    instant: &mut Option<ModelRef>,
    batch: &mut Option<ModelRef>,
    keys: &mut Keys,
) {
    // The name is what the models, the defaults, and the key hold on to, so
    // no two providers share one.
    let names: Vec<String> = providers.iter().map(|p| p.name.clone()).collect();
    let fresh = (1..).map(|n| if n == 1 { "New provider".to_string() } else { format!("New provider {n}") })
        .find(|name| !names.contains(name)).unwrap();
    let (sel, remove) = selector(ui, "provider", names.clone(), &mut || providers.push(Provider::new(&fresh, Kind::OpenAi)));
    if let Some(p) = providers.get_mut(sel) {
        egui::Grid::new("provider fields").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("Name");
            let old = p.name.clone();
            let field = ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(f32::INFINITY));
            let taken = names.iter().enumerate().any(|(i, n)| i != sel && *n == p.name);
            if field.changed() && taken {
                p.name = old;
            } else if field.changed() {
                // The models, the defaults, and the key follow the name.
                for m in models.iter_mut().filter(|m| m.provider == old) {
                    m.provider = p.name.clone();
                }
                for r in [&mut *instant, &mut *batch].into_iter().flatten() {
                    if r.provider == old {
                        r.provider = p.name.clone();
                    }
                }
                keys.rename(&old, &p.name);
            }
            ui.end_row();
            ui.label("API type");
            let old_kind = p.kind;
            egui::ComboBox::from_id_salt("kind")
                .selected_text(p.kind.label())
                .show_ui(ui, |ui| {
                    for k in [Kind::OpenAi, Kind::Gemini] {
                        ui.selectable_value(&mut p.kind, k, k.label());
                    }
                })
                .response
                .on_hover_text("OpenAI: chat completions, as OpenAI and OpenRouter speak them. Gemini: Google's Gemini API.");
            if old_kind != p.kind { p.update_kind_defaults(old_kind); }
            ui.end_row();
            ui.label("URL");
            ui.add(egui::TextEdit::singleline(&mut p.url).desired_width(f32::INFINITY));
            ui.end_row();
            if p.is_openrouter() {
                ui.label("Excluded hosts");
                let id = ui.make_persistent_id(("provider skip", &p.name, &p.url));
                let mut skip = ui.data_mut(|data| data.get_temp::<String>(id)).unwrap_or_else(|| p.skipped().join(", "));
                let edit = ui.add(egui::TextEdit::singleline(&mut skip).id(id).desired_width(f32::INFINITY))
                    .on_hover_text("OpenRouter provider slugs to skip, separated by commas. Empty allows all providers.");
                if edit.changed() { p.skip = Some(crate::labels::parse_list(&skip)); }
                // Keep separators while the user types the next provider.
                let focused = edit.has_focus();
                ui.data_mut(|data| {
                    if focused { data.insert_temp(id, skip); } else { data.remove::<String>(id); }
                });
                ui.end_row();
            }
            ui.label("Environment variables");
            let mut env = p.key_env.join(", ");
            if ui.add(egui::TextEdit::singleline(&mut env).desired_width(f32::INFINITY)).changed() {
                p.key_env = env.split([',', ' ']).filter(|s| !s.is_empty()).map(str::to_string).collect();
            }
            ui.end_row();
            ui.label("");
            ui.weak(match p.key_source(keys) {
                KeySource::Typed => "the typed key is used".to_string(),
                KeySource::Env(name) => format!("the key comes from ${name}"),
                KeySource::None => "no key: type one, or set the variable".to_string(),
            });
            ui.end_row();
            if p.kind == Kind::OpenAi {
                ui.label("Sheet storage");
                let mut on = p.store.is_some();
                if ui.checkbox(&mut on, "Upload sheets so a batch can fetch them").changed() {
                    p.store = on.then(crate::s3::Store::default);
                }
                ui.end_row();
                let has_secret = p.store_secret(keys).is_some();
                if let Some(store) = &mut p.store {
                    for (label, value) in [("Endpoint", &mut store.endpoint), ("Region", &mut store.region),
                        ("Bucket", &mut store.bucket), ("Access key", &mut store.access_key)] {
                        ui.label(label);
                        ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
                        ui.end_row();
                    }
                    ui.label("");
                    ui.checkbox(&mut store.path_style, "Path-style URLs (MinIO)");
                    ui.end_row();
                    ui.label("Secret key");
                    ui.add(egui::TextEdit::singleline(keys.entry(&s3_secret_name(&p.name))).password(true)
                        .hint_text("Secret access key").desired_width(f32::INFINITY));
                    ui.end_row();
                    ui.label("");
                    ui.weak(if has_secret { "The secret key is set" } else { "No secret key yet" });
                    ui.end_row();
                }
            }
        });
    }
    if remove && sel < providers.len() {
        let gone = providers.remove(sel);
        models.retain(|m| m.provider != gone.name);
        for r in [&mut *instant, &mut *batch] {
            if r.as_ref().is_some_and(|r| r.provider == gone.name) {
                *r = None;
            }
        }
    }
}

/// One model at a time: its provider, ID, and scope. Active selections follow edits.
fn models_ui(ui: &mut egui::Ui, providers: &[Provider], models: &mut Vec<Model>, instant: &mut Option<ModelRef>, batch: &mut Option<ModelRef>) -> bool {
    let mut configure = false;
    let names = models.iter().map(Model::label).collect();
    let first = providers.first().map(|p| p.name.clone()).unwrap_or_default();
    let (sel, remove) = selector(ui, "model", names, &mut || models.push(Model { provider: first.clone(), id: String::new() }));
    if let Some(m) = models.get_mut(sel) {
        let before = m.reference();
        egui::Grid::new("model fields").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("provider");
            egui::ComboBox::from_id_salt("model provider").selected_text(m.provider.clone()).show_ui(ui, |ui| {
                for p in providers {
                    ui.selectable_value(&mut m.provider, p.name.clone(), &p.name);
                }
            });
            ui.end_row();
            ui.label("Model ID");
            let mut mode = m.mode();
            let mut id = m.id.trim_end_matches(BATCH).to_string();
            if ui.add(egui::TextEdit::singleline(&mut id).desired_width(f32::INFINITY)).changed() {
                m.id = id.trim_end_matches(BATCH).to_string();
                m.set_batch(mode == Mode::Batch);
            }
            ui.end_row();
            ui.label("Use for");
            egui::ComboBox::from_id_salt("model scope").selected_text(mode.label()).show_ui(ui, |ui| {
                for scope in [Mode::Instant, Mode::Batch] { ui.selectable_value(&mut mode, scope, scope.label()); }
            });
            m.set_batch(mode == Mode::Batch);
            ui.end_row();
            if mode == Mode::Batch && let Some(provider) = providers.iter().find(|p| p.name == m.provider) {
                ui.label("Processing");
                ui.label(match provider.kind {
                    Kind::OpenAi => "Several sheets at a time",
                    Kind::Gemini => "Google batch",
                });
                ui.end_row();
            }
        });
        if m.mode() == Mode::Batch && providers.iter().any(|p| p.name == m.provider && p.kind == Kind::OpenAi) {
            configure = endpoint_notice(ui);
        }
        let after = m.reference();
        if after != before {
            for r in [&mut *instant, &mut *batch].into_iter().flatten() {
                if *r == before {
                    *r = after.clone();
                }
            }
        }
    }
    if remove && sel < models.len() {
        let gone = models.remove(sel).reference();
        for r in [&mut *instant, &mut *batch] {
            if r.as_ref() == Some(&gone) {
                *r = None;
            }
        }
    }
    configure
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_setup_reuses_configuration_and_keeps_active_models() {
        let mut ai = Ai::default(); let instant = ai.instant.clone(); let batch = ai.batch.clone();
        ai.providers.retain(|p| p.kind != Kind::Gemini); ai.models.retain(|m| m.provider != "Google");
        let (provider, model) = gemini_setup(&mut ai.providers, &mut ai.models);
        assert_eq!(ai.providers[provider].url, Kind::Gemini.defaults().0);
        assert_eq!(ai.models[model].id, "gemini-flash-latest:batch");
        let configured = ai.clone();
        assert_eq!(gemini_setup(&mut ai.providers, &mut ai.models), (provider, model));
        assert_eq!(ai, configured); assert_eq!(ai.instant, instant); assert_eq!(ai.batch, batch);
    }

    #[test]
    fn changing_provider_kind_updates_only_stock_connection_fields() {
        let mut provider = Provider::new("test", Kind::OpenAi);
        provider.kind = Kind::Gemini; provider.update_kind_defaults(Kind::OpenAi);
        assert_eq!(provider.url, Kind::Gemini.defaults().0);
        assert_eq!(provider.key_env, Kind::Gemini.defaults().1);
        provider.url = "https://example.test/api".into(); provider.key_env = vec!["CUSTOM_KEY".into()];
        provider.kind = Kind::OpenAi; provider.update_kind_defaults(Kind::Gemini);
        assert_eq!(provider.url, "https://example.test/api"); assert_eq!(provider.key_env, ["CUSTOM_KEY"]);
    }

    #[test]
    fn settings_draw_with_an_openrouter_skip_field() {
        let ctx = egui::Context::default();
        let mut ai = Ai::default();
        let mut keys = Keys::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| settings_ui(ui, &mut ai, &mut keys));
        output.textures_delta.clear();
    }

    #[test]
    fn old_settings_default_to_phala_but_an_empty_skip_list_stays_empty() {
        let mut p: Provider = serde_json::from_value(serde_json::json!({
            "name":"Router", "kind":"open_ai", "url":"https://openrouter.ai/api/v1"
        })).unwrap();
        assert_eq!(p.skipped(), ["phala"]);
        assert_eq!(p.route(serde_json::json!({}))["provider"]["ignore"], serde_json::json!(["phala"]));
        p.skip = Some(vec![]);
        let saved = serde_json::to_value(&p).unwrap();
        let p: Provider = serde_json::from_value(saved).unwrap();
        assert!(p.skipped().is_empty());
        assert!(p.route(serde_json::json!({})).get("provider").is_none());
    }

    #[test]
    fn skip_lists_only_reach_openrouter() {
        let mut p = Ai::default().providers.remove(0);
        p.skip = Some(vec!["phala".into(), "another-provider".into()]);
        let body = serde_json::json!({"model":"vision", "messages":[]});
        assert_eq!(p.route(body.clone())["provider"]["ignore"], serde_json::json!(["phala", "another-provider"]));
        for url in ["https://api.openai.com/v1", "https://openrouter.ai.example/v1", "https://example.test/openrouter.ai"] {
            p.url = url.into();
            assert_eq!(p.route(body.clone()), body);
        }
        p.url = "https://openrouter.ai/api/v1".into();
        p.kind = Kind::Gemini;
        assert_eq!(p.route(body.clone()), body);
    }

    #[test]
    fn the_defaults_name_a_model_for_each_mode() {
        let ai = Ai::default();
        let name = |c: Option<(&Provider, &Model)>| c.map(|(p, m)| (p.name.clone(), m.id.clone()));
        assert_eq!(name(ai.chosen(Mode::Instant)), Some(("OpenRouter".into(), "~deepseek/deepseek-flash-latest".into())));
        assert_eq!(name(ai.chosen(Mode::Batch)), Some(("OpenRouter".into(), "~deepseek/deepseek-flash-latest:batch".into())));
    }

    /// A file from 0.2 names the models it shipped. They go, the new ones
    /// come in, and the choices follow; a model the user added stays.
    #[test]
    fn retired_models_give_way_to_the_shipped_ones() {
        let on = |id: &str| Model { provider: "OpenRouter".into(), id: id.into() };
        let mut ai = Ai {
            models: vec![on("xiaomi/mimo-v2.5"), on("openrouter/free"), on("xiaomi/mimo-v2.5:batch"), on("mine/vision")],
            instant: Some(on("openrouter/free").reference()),
            batch: Some(on("xiaomi/mimo-v2.5:batch").reference()),
            ..Ai::default()
        };
        ai.heal();
        let ids: Vec<_> = ai.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["mine/vision", "~deepseek/deepseek-flash-latest", "~deepseek/deepseek-flash-latest:batch",
            "z-ai/glm-5.3-flash", "z-ai/glm-5.3-flash:batch", "gemini-flash-latest:batch", "gemini-flash-latest"]);
        assert_eq!(ai.chosen(Mode::Instant).map(|(_, m)| m.id.as_str()), Some("~deepseek/deepseek-flash-latest"));
        assert_eq!(ai.chosen(Mode::Batch).map(|(_, m)| m.id.as_str()), Some("~deepseek/deepseek-flash-latest:batch"));
    }

    /// A file from the tool that kept models inside the providers: no
    /// models, and choices that name nothing.
    #[test]
    fn an_old_file_heals_to_the_shipped_models() {
        let mut ai = Ai { models: vec![Model { provider: "OpenRouter".into(), id: String::new() }], ..Ai::default() };
        ai.batch = Some(ModelRef { provider: "Google".into(), model: "gemini-3.7-flash".into() });
        ai.heal();
        assert_eq!(ai.models, Ai::default().models);
        assert_eq!(ai.chosen(Mode::Instant).map(|(_, m)| m.id.as_str()), Some("~deepseek/deepseek-flash-latest"));
        assert_eq!(ai.chosen(Mode::Batch).map(|(_, m)| m.id.as_str()), Some("~deepseek/deepseek-flash-latest:batch"));
    }

    #[test]
    fn google_defaults_migrate_without_changing_custom_models_or_other_defaults() {
        let mut ai = Ai::default();
        ai.models.retain(|m| m.provider != "Google");
        ai.models.push(Model { provider: "Google".into(), id: "gemini-3.7-flash:batch".into() });
        ai.models.push(Model { provider: "Google".into(), id: "custom-model".into() });
        ai.batch = Some(ModelRef { provider: "Google".into(), model: "gemini-3.7-flash:batch".into() });
        let instant = ai.instant.clone(); let providers = ai.providers.clone();
        ai.heal();
        assert_eq!(ai.instant, instant); assert_eq!(ai.providers, providers);
        assert_eq!(ai.batch.as_ref().unwrap().model, "gemini-flash-latest:batch");
        assert!(ai.models.iter().any(|m| m.id == "custom-model"));
        assert!(ai.models.iter().any(|m| m.id == "gemini-flash-latest" && m.mode() == Mode::Instant));
        let healed = ai.clone(); ai.heal(); assert_eq!(ai, healed);
    }

    #[test]
    fn the_batch_end_of_the_id_is_the_mode() {
        let mut m = Model { provider: "P".into(), id: "x".into() };
        assert_eq!(m.mode(), Mode::Instant);
        m.set_batch(true);
        m.set_batch(true);
        assert_eq!((m.id.as_str(), m.mode()), ("x:batch", Mode::Batch));
        m.set_batch(false);
        assert_eq!((m.id.as_str(), m.mode()), ("x", Mode::Instant));
    }

    #[test]
    fn a_damaged_key_file_is_kept_until_the_user_says_yes() {
        let dir = crate::storage::tests::Folder::new();
        let path = dir.0.join("keys.json");
        std::fs::write(&path, b"[]").unwrap();
        let (keys, error) = Keys::load_from(&path);
        assert!(error.unwrap().contains("keys.json"));
        assert!(keys.0.is_empty());
        assert!(keys.save_to(&path, false).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"[]");
        keys.save_to(&path, true).unwrap();
        assert!(Keys::load_from(&path).1.is_none());
        std::fs::write(&path, br#"{"X": "k"}"#).unwrap();
        let (keys, notice) = Keys::load_from(&path);
        assert_eq!((keys.get("X"), notice), (Some("k"), None));
    }

    #[test]
    fn a_typed_key_wins_over_the_environment() {
        let p = Provider::new("X", Kind::Gemini);
        let mut keys = Keys::default();
        let env = |name: &str| (name == "GEMINI_API_KEY").then(|| "k".to_string());
        assert_eq!(p.key_source_in(&keys, env), KeySource::Env("GEMINI_API_KEY".into()));
        assert_eq!(p.key_source_in(&keys, |_| None), KeySource::None);
        assert_eq!(p.key_source_in(&keys, |_| Some(" ".into())), KeySource::None);
        *keys.entry("X") = "typed".into();
        assert_eq!(p.key_source_in(&keys, env), KeySource::Typed);
    }
}
