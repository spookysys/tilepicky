// SPDX-License-Identifier: GPL-3.0-only
//! The AI providers and models the tool may call, and the settings page
//! that edits them. A provider is an endpoint of one of two kinds: an
//! OpenAI-style chat endpoint (OpenAI, OpenRouter), or Google's Gemini API.
//! A model names its provider; a model id that ends in `:batch` is a batch
//! model. The API keys live in a file of their own, readable by the owner
//! alone; see `Keys`.

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The end of a model id that marks a batch model.
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

/// How a model is used: one request with one answer, or a batch job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Instant,
    Batch,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Instant => "instant",
            Mode::Batch => "batch",
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
}

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
    fn new(name: &str, kind: Kind) -> Self {
        let (url, env) = kind.defaults();
        Provider { name: name.into(), kind, url: url.into(), key_env: env.iter().map(|s| s.to_string()).collect() }
    }

    pub fn key_source(&self, keys: &Keys) -> KeySource {
        self.key_source_in(keys, |name| std::env::var(name).ok())
    }

    /// A typed key wins over the environment.
    fn key_source_in(&self, keys: &Keys, env: impl Fn(&str) -> Option<String>) -> KeySource {
        if keys.get(&self.name).is_some() {
            return KeySource::Typed;
        }
        match self.key_env.iter().find(|name| env(name).is_some_and(|v| !v.is_empty())) {
            Some(name) => KeySource::Env(name.clone()),
            None => KeySource::None,
        }
    }
}

/// A model on one of the providers.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub provider: String,
    /// The id the endpoint knows, with `:batch` at the end for a batch model.
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
        format!("{} ({})", self.id, self.provider)
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

/// What a fresh install offers: OpenRouter for instant answers and for
/// batch jobs, with the free router and a cheap model; and Google, whose
/// batch endpoint takes images, which OpenRouter's does not yet. No keys.
impl Default for Ai {
    fn default() -> Self {
        let mut openrouter = Provider::new("OpenRouter", Kind::OpenAi);
        openrouter.url = "https://openrouter.ai/api/v1".into();
        openrouter.key_env = vec!["OPENROUTER_API_KEY".into()];
        let google = Provider::new("Google", Kind::Gemini);
        let on = |provider: &str, id: &str| Model { provider: provider.into(), id: id.into() };
        let models = vec![
            on("OpenRouter", "xiaomi/mimo-v2.5"),
            on("OpenRouter", "openrouter/free"),
            on("OpenRouter", "xiaomi/mimo-v2.5:batch"),
            on("Google", "gemini-3.7-flash:batch"),
        ];
        let (instant, batch) = (Some(models[1].reference()), Some(models[2].reference()));
        Ai { providers: vec![openrouter, google], models, instant, batch }
    }
}

impl Ai {
    /// Fills in what a settings file from an older tool lacks. A model
    /// without an id is dropped; without any model left, the shipped
    /// models come in, on the providers that exist; a choice that names no
    /// model falls back to the shipped one, when that exists.
    pub fn heal(&mut self) {
        self.models.retain(|m| !m.id.is_empty());
        let shipped = Ai::default();
        if self.models.is_empty() {
            self.models = shipped.models.into_iter().filter(|m| self.providers.iter().any(|p| p.name == m.provider)).collect();
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

    pub fn load() -> Self {
        Self::file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .map(Keys)
            .unwrap_or_default()
    }

    /// Writes the keys that are not empty. A failure is silent, like the
    /// settings.
    pub fn save(&self) {
        let Some(path) = Self::file() else { return };
        let kept: BTreeMap<&String, &String> = self.0.iter().filter(|(_, v)| !v.is_empty()).collect();
        let Ok(json) = serde_json::to_string_pretty(&kept) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = write_private(&path, &json);
    }

    pub fn get(&self, provider: &str) -> Option<&str> {
        self.0.get(provider).map(String::as_str).filter(|k| !k.is_empty())
    }

    /// The typed key of a provider, to edit; empty when there is none.
    fn entry(&mut self, provider: &str) -> &mut String {
        self.0.entry(provider.to_string()).or_default()
    }

    /// The key follows a provider that changes its name.
    fn rename(&mut self, old: &str, new: &str) {
        if let Some(v) = self.0.remove(old) {
            self.0.insert(new.to_string(), v);
        }
    }
}

/// Writes a file that only its owner can read.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(text.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The settings page: the providers, the models, and the two defaults.
/// Each of the first two shows one item, chosen in a selector, so the page
/// stays short. Edits land in place; the caller writes both files when the
/// dialog closes.
pub fn settings_ui(ui: &mut egui::Ui, ai: &mut Ai, keys: &mut Keys) {
    let Ai { providers, models, instant, batch } = ai;
    ui.strong("Providers");
    providers_ui(ui, providers, models, instant, batch, keys);
    ui.add_space(8.0);
    ui.strong("Models");
    models_ui(ui, providers, models, instant, batch);
    ui.add_space(8.0);
    ui.strong("Defaults");
    egui::Grid::new("defaults").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        for (mode, slot) in [(Mode::Instant, &mut *instant), (Mode::Batch, &mut *batch)] {
            ui.label(mode.label());
            let current = slot.as_ref().and_then(|r| models.iter().find(|m| m.is(r) && m.mode() == mode));
            let text = current.map_or("none".to_string(), Model::label);
            egui::ComboBox::from_id_salt(("default", mode.label())).selected_text(text).show_ui(ui, |ui| {
                for m in models.iter().filter(|m| m.mode() == mode) {
                    ui.selectable_value(slot, Some(m.reference()), m.label());
                }
            });
            ui.end_row();
        }
    });
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
    let names = providers.iter().map(|p| p.name.clone()).collect();
    let (sel, remove) = selector(ui, "provider", names, &mut || providers.push(Provider::new("New provider", Kind::OpenAi)));
    if let Some(p) = providers.get_mut(sel) {
        egui::Grid::new("provider fields").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("name");
            let old = p.name.clone();
            if ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(f32::INFINITY)).changed() {
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
            ui.label("kind");
            egui::ComboBox::from_id_salt("kind")
                .selected_text(p.kind.label())
                .show_ui(ui, |ui| {
                    for k in [Kind::OpenAi, Kind::Gemini] {
                        ui.selectable_value(&mut p.kind, k, k.label());
                    }
                })
                .response
                .on_hover_text("OpenAI: chat completions, as OpenAI and OpenRouter speak them. Gemini: Google's Gemini API.");
            ui.end_row();
            ui.label("URL");
            ui.add(egui::TextEdit::singleline(&mut p.url).desired_width(f32::INFINITY));
            ui.end_row();
            ui.label("key");
            let typed = keys.entry(&p.name);
            ui.add(egui::TextEdit::singleline(typed).password(true).hint_text("empty: the environment variable below").desired_width(f32::INFINITY));
            ui.end_row();
            ui.label("env");
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

/// One model at a time: its provider, its id, and the batch mark. A default
/// follows the model it named through an edit.
fn models_ui(ui: &mut egui::Ui, providers: &[Provider], models: &mut Vec<Model>, instant: &mut Option<ModelRef>, batch: &mut Option<ModelRef>) {
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
            ui.label("id");
            ui.add(egui::TextEdit::singleline(&mut m.id).desired_width(f32::INFINITY));
            ui.end_row();
            ui.label("");
            let mut on = m.mode() == Mode::Batch;
            if ui.checkbox(&mut on, "batch").on_hover_text("A batch model: the id ends in \":batch\".").changed() {
                m.set_batch(on);
            }
            ui.end_row();
        });
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_name_a_model_for_each_mode() {
        let ai = Ai::default();
        let name = |c: Option<(&Provider, &Model)>| c.map(|(p, m)| (p.name.clone(), m.id.clone()));
        assert_eq!(name(ai.chosen(Mode::Instant)), Some(("OpenRouter".into(), "openrouter/free".into())));
        assert_eq!(name(ai.chosen(Mode::Batch)), Some(("OpenRouter".into(), "xiaomi/mimo-v2.5:batch".into())));
    }

    /// A file from the tool that kept models inside the providers: no
    /// models, and choices that name nothing.
    #[test]
    fn an_old_file_heals_to_the_shipped_models() {
        let mut ai = Ai { models: vec![Model { provider: "OpenRouter".into(), id: String::new() }], ..Ai::default() };
        ai.batch = Some(ModelRef { provider: "Google".into(), model: "gemini-3.7-flash".into() });
        ai.heal();
        assert_eq!(ai.models, Ai::default().models);
        assert_eq!(ai.chosen(Mode::Instant).map(|(_, m)| m.id.as_str()), Some("openrouter/free"));
        assert_eq!(ai.chosen(Mode::Batch).map(|(_, m)| m.id.as_str()), Some("xiaomi/mimo-v2.5:batch"));
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
    fn a_typed_key_wins_over_the_environment() {
        let p = Provider::new("X", Kind::Gemini);
        let mut keys = Keys::default();
        let env = |name: &str| (name == "GEMINI_API_KEY").then(|| "k".to_string());
        assert_eq!(p.key_source_in(&keys, env), KeySource::Env("GEMINI_API_KEY".into()));
        assert_eq!(p.key_source_in(&keys, |_| None), KeySource::None);
        *keys.entry("X") = "typed".into();
        assert_eq!(p.key_source_in(&keys, env), KeySource::Typed);
    }
}
