// SPDX-License-Identifier: GPL-3.0-only
//! Regression tests for workspace isolation and independent search sources.

use super::*;
use crate::storage::tests::Folder;
use image::{Rgba, RgbaImage};

struct Fixture {
    app: App,
    ctx: egui::Context,
    _library: Folder,
    _project: Folder,
}

fn fixture() -> Fixture {
    let library = Folder::new();
    let project = Folder::new();
    RgbaImage::from_pixel(16, 16, Rgba([80, 160, 20, 255])).save(library.0.join("source.png")).unwrap();
    let mut canvas = RgbaImage::new(32, 32);
    canvas.put_pixel(0, 0, Rgba([200, 30, 80, 255]));
    canvas.save(project.0.join("canvas.png")).unwrap();
    let mut settings = settings::Settings::default();
    settings.library.path = Some(library.0.clone());
    settings.project.path = Some(project.0.clone());
    settings.library.tile = Some(Pair::of([16, 16]));
    settings.project.tile = Some(Pair::of([16, 16]));
    let mut app = App::new(settings, None);
    app.damaged.clear();
    let ctx = egui::Context::default();
    app.library.open(&ctx, 0).unwrap();
    app.project.open(&ctx, 0).unwrap();
    app.project.sheet.as_mut().unwrap().tile = [16, 16];
    app.library.sheet.as_mut().unwrap().tile = [16, 16];
    app.library.sheet.as_mut().unwrap().sel = Sel::rect((0, 0), (0, 0));
    app.project.sheet.as_mut().unwrap().sel = Sel::rect((0, 0), (0, 0));
    app.clip = app.library.sheet.as_ref().unwrap().copy();
    // No test may start a network request, including under a slow test runner.
    app.query_at = std::time::Instant::now() + Duration::from_secs(3600);
    Fixture { app, ctx, _library: library, _project: project }
}

fn key_event(key: Key, modifiers: Modifiers) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

#[test]
fn library_shortcuts_leave_the_hidden_project_unchanged() {
    for event in [key_event(Key::V, Modifiers::COMMAND), egui::Event::Paste("tiles".into()),
        key_event(Key::Delete, Modifiers::NONE), key_event(Key::T, Modifiers::COMMAND),
        key_event(Key::Z, Modifiers::COMMAND), key_event(Key::X, Modifiers::COMMAND)] {
        for workspace in [workspace::Workspace::Library, workspace::Workspace::Build] {
            let mut f = fixture();
            // Give Undo a real edit to reverse, and leave transparent borders for Trim.
            f.app.project.sheet.as_mut().unwrap().paste(&f.ctx, (0, 0), f.app.clip.as_ref().unwrap());
            f.app.clip.as_mut().unwrap().img.put_pixel(0, 0, Rgba([1, 2, 3, 255]));
            let before = f.app.project.sheet.as_ref().unwrap().img.clone();
            let dirty = f.app.project.sheet.as_ref().unwrap().dirty;
            f.app.workspace = workspace;
            // A stale focus from Build must not bypass the workspace guard.
            f.app.active = Panel::Project;
            f.app.pane = (Panel::Project, Spot::Sheet);
            f.ctx.memory_mut(|m| m.request_focus(project_id()));
            let mut output = f.ctx.run_ui(egui::RawInput { events: vec![event.clone()], ..Default::default() }, |_| {
                f.app.handle_keys(&f.ctx);
            });
            output.textures_delta.clear();
            if workspace == workspace::Workspace::Library {
                assert!(f.app.project.sheet.as_ref().unwrap().img == before, "Shortcut changed the hidden project: {event:?}");
                assert_eq!(f.app.project.sheet.as_ref().unwrap().dirty, dirty);
            } else {
                assert!(f.app.project.sheet.as_ref().unwrap().img != before, "The control shortcut did not edit Build: {event:?}");
            }
        }
    }
}

#[test]
fn library_pane_navigation_cannot_enter_hidden_canvases() {
    let mut f = fixture();
    f.app.workspace = workspace::Workspace::Library;
    assert_eq!(f.app.body((Panel::Library, Spot::Tree)), Some(library_tree_id()));
    for pane in [(Panel::Project, Spot::Tree), (Panel::Project, Spot::Sheet), (Panel::Library, Spot::Sheet)] {
        assert_eq!(f.app.body(pane), None);
    }
    f.app.pane = (Panel::Library, Spot::Tree);
    f.app.active = Panel::Library;
    f.ctx.memory_mut(|m| m.request_focus(library_tree_id()));
    for direction in [1, -1, 1, 1] {
        f.app.press_pane(&f.ctx, direction);
        assert!(f.app.pane == (Panel::Library, Spot::Tree));
        assert_eq!(f.ctx.memory(|m| m.focused()), Some(library_tree_id()));
    }
    f.app.workspace = workspace::Workspace::Build;
    assert_eq!(f.app.body((Panel::Project, Spot::Sheet)), Some(project_id()));
    assert_eq!(f.app.body((Panel::Library, Spot::Sheet)), Some(library_id()));
}

fn independent_models(f: &mut Fixture) -> (QueryKey, QueryKey) {
    let (provider, model) = f.app.settings.ai.chosen_embed().unwrap();
    let image = QueryKey { text: "forest".into(), provider: embed::identity(provider), model: model.id.clone() };
    let text_model = ai::Model { provider: model.provider.clone(), id: "fixture-text-embedding".into(), role: ai::Role::Embed };
    f.app.settings.ai.text_embed = Some(ai::ModelRef { provider: text_model.provider.clone(), model: text_model.id.clone() });
    let text = QueryKey { model: text_model.id.clone(), ..image.clone() };
    f.app.settings.ai.models.push(text_model);
    f.app.settings.search.embeddings = true;
    f.app.settings.search.text_embeddings = true;
    f.app.query = "forest".into();
    f.app.qwords = index::words("forest");
    let index = &mut f.app.library.index;
    index.embed_model = image.model.clone();
    index.embed_provider = image.provider.clone();
    index.text_embed_model = text.model.clone();
    index.text_embed_provider = text.provider.clone();
    index.entries[0].embed = Some(vec![1.0, 0.0]);
    index.entries[0].text_embed = Some(vec![0.0, 1.0]);
    (image, text)
}

#[test]
fn text_queries_use_their_own_model_and_reject_other_model_replies() {
    let mut f = fixture();
    let (image, text) = independent_models(&mut f);
    f.app.embed_query = Some((image.clone(), vec![1.0, 0.0]));
    let (tx, rx) = std::sync::mpsc::channel();
    f.app.text_query.pending = Some((text.clone(), rx));
    tx.send(Ok(vec![0.0, 1.0])).unwrap();
    f.app.ensure_query_embedding(&f.ctx);
    assert_eq!(f.app.library.embed_query, Some(vec![1.0, 0.0]));
    assert_eq!(f.app.library.text_embed_query, Some(vec![0.0, 1.0]));
    assert_eq!(f.app.text_query.cached.as_ref().unwrap().0, text);

    f.app.text_query.cached = None;
    let (tx, rx) = std::sync::mpsc::channel();
    f.app.text_query.pending = Some((image, rx));
    tx.send(Ok(vec![1.0, 0.0])).unwrap();
    f.app.ensure_query_embedding(&f.ctx);
    assert!(f.app.text_query.cached.is_none());
    assert!(f.app.library.text_embed_query.is_none());
    assert_eq!(f.app.library.embed_query, Some(vec![1.0, 0.0]));
    assert!(f.app.text_query.pending.is_none());
}

#[test]
fn refreshing_a_query_clears_only_the_stale_source_vector() {
    let mut f = fixture();
    let (image, mut text) = independent_models(&mut f);
    f.app.embed_query = Some((image, vec![1.0, 0.0]));
    text.text = "old query".into();
    f.app.text_query.cached = Some((text, vec![0.0, 1.0]));
    f.app.library.embed_query = Some(vec![1.0, 0.0]);
    f.app.library.text_embed_query = Some(vec![0.0, 1.0]);
    f.app.refresh_query();
    assert_eq!(f.app.library.embed_query, Some(vec![1.0, 0.0]));
    assert!(f.app.library.text_embed_query.is_none());
}
