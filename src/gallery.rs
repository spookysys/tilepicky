// SPDX-License-Identifier: GPL-3.0-only
//! A virtual sheet gallery. A bounded worker decodes the visible thumbnails.

use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::SystemTime;

const CACHE_LIMIT: usize = 160;
const THUMB_SIZE: u32 = 256;
const CARD_HEIGHT: f32 = 192.0;

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
}

struct Request { generation: u64, key: Key, context: egui::Context }
struct Reply { generation: u64, key: Key, image: Result<egui::ColorImage, String> }
struct Cached { texture: Result<egui::TextureHandle, String>, used: u64 }

pub struct Gallery {
    root: PathBuf,
    requests: SyncSender<Request>,
    replies: Receiver<Reply>,
    pending: HashSet<Key>,
    cache: HashMap<Key, Cached>,
    frame: u64,
    generation: u64,
}

impl Default for Gallery {
    fn default() -> Self {
        let (requests, receiver) = mpsc::sync_channel::<Request>(12);
        let (sender, replies) = mpsc::sync_channel(12);
        std::thread::spawn(move || {
            while let Ok(request) = receiver.recv() {
                let image = thumbnail(&request.key.path);
                if sender.send(Reply { generation: request.generation, key: request.key, image }).is_err() { break; }
                request.context.request_repaint();
            }
        });
        Self { root: PathBuf::new(), requests, replies, pending: HashSet::new(), cache: HashMap::new(), frame: 0, generation: 0 }
    }
}

fn thumbnail(path: &std::path::Path) -> Result<egui::ColorImage, String> {
    let mut reader = image::ImageReader::open(path).map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(32768);
    limits.max_image_height = Some(32768);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| e.to_string())?.thumbnail(THUMB_SIZE, THUMB_SIZE).to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
}

fn ordered_indices(count: usize, order: &[usize]) -> Vec<usize> {
    let mut seen = HashSet::new();
    order.iter().copied().filter(|&i| i < count && seen.insert(i)).collect()
}

impl Gallery {
    fn prepare(&mut self, ui: &egui::Ui, index: &crate::index::Index) {
        if self.root != index.root {
            self.root.clone_from(&index.root);
            self.generation = self.generation.wrapping_add(1);
            self.cache.clear();
            self.pending.clear();
        }
        self.frame = self.frame.wrapping_add(1);
        while let Ok(reply) = self.replies.try_recv() {
            if reply.generation != self.generation { continue; }
            self.pending.remove(&reply.key);
            let texture = reply.image.map(|image| ui.ctx().load_texture("sheet thumbnail", image, egui::TextureOptions::NEAREST));
            self.cache.insert(reply.key, Cached { texture, used: self.frame });
        }
        while self.cache.len() > CACHE_LIMIT {
            let key = self.cache.iter().min_by_key(|(_, value)| value.used).map(|(key, _)| key.clone()).unwrap();
            self.cache.remove(&key);
        }
    }

    /// Show the selected sheet with the same bounded thumbnail cache.
    pub fn preview(&mut self, ui: &mut egui::Ui, index: &crate::index::Index, i: usize, max_size: egui::Vec2) {
        if i >= index.entries.len() { return; }
        self.prepare(ui, index);
        let size = egui::vec2(max_size.x.min(ui.available_width()).max(1.0), max_size.y.max(1.0));
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter().rect_filled(rect, 5.0, ui.visuals().extreme_bg_color);
        self.paint_preview(ui, index, i, rect, &response);
        if !self.pending.is_empty() { ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)); }
    }

    fn paint_preview(&mut self, ui: &egui::Ui, index: &crate::index::Index, i: usize, rect: egui::Rect, response: &egui::Response) {
        let path = index.root.join(&index.entries[i].rel);
        let metadata = std::fs::metadata(&path).ok();
        let key = Key { path, modified: metadata.as_ref().and_then(|m| m.modified().ok()),
            len: metadata.as_ref().map_or(0, |m| m.len()) };
        if let Some(cached) = self.cache.get_mut(&key) {
            cached.used = self.frame;
            match &cached.texture {
                Ok(texture) => {
                    let size = texture.size_vec2();
                    let scale = ((rect.width() - 12.0) / size.x).min((rect.height() - 12.0) / size.y).min(2.0);
                    let target = egui::Rect::from_center_size(rect.center(), size * scale.max(0.01));
                    ui.painter().image(texture.id(), target,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                }
                Err(error) => {
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Preview unavailable",
                        egui::FontId::proportional(12.0), ui.visuals().weak_text_color());
                    response.clone().on_hover_text(error);
                }
            }
        } else {
            if !self.pending.contains(&key)
                && self.requests.try_send(Request { generation: self.generation, key: key.clone(), context: ui.ctx().clone() }).is_ok() {
                self.pending.insert(key);
            }
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Loading preview...",
                egui::FontId::proportional(12.0), ui.visuals().weak_text_color());
        }
    }

    pub fn show_ordered(
        &mut self, ui: &mut egui::Ui, index: &crate::index::Index, order: &[usize],
        selected: Option<usize>, marked: &mut HashSet<usize>,
    ) -> Option<usize> {
        self.prepare(ui, index);
        let indices = ordered_indices(index.entries.len(), order);
        if indices.is_empty() {
            ui.add_space(24.0);
            ui.heading(if index.root.as_os_str().is_empty() { "Open a library to browse your sheets" } else { "No matching sheets" });
            return None;
        }
        let spacing = ui.spacing().item_spacing.x;
        let columns = ((ui.available_width() + spacing) / (176.0 + spacing)).floor().max(1.0) as usize;
        let width = ((ui.available_width() - spacing * (columns - 1) as f32) / columns as f32).max(40.0);
        let rows = indices.len().div_ceil(columns);
        let mut opened = None;
        egui::ScrollArea::vertical().id_salt("library_gallery").show_rows(ui, CARD_HEIGHT, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for &i in indices.iter().skip(row * columns).take(columns) {
                        let entry = &index.entries[i];
                        ui.push_id(&entry.rel, |ui| {
                            ui.allocate_ui_with_layout(egui::vec2(width, CARD_HEIGHT), egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.set_max_width(width);
                                let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 130.0), egui::Sense::click());
                                ui.painter().rect_filled(rect, 5.0, ui.visuals().extreme_bg_color);
                                self.paint_preview(ui, index, i, rect, &response);
                                if selected == Some(i) || response.hovered() {
                                    let stroke = if selected == Some(i) { ui.visuals().selection.stroke } else { ui.visuals().widgets.hovered.fg_stroke };
                                    ui.painter().rect_stroke(rect, 5.0, stroke, egui::StrokeKind::Inside);
                                }
                                if response.on_hover_text(&entry.rel).clicked() { opened = Some(i); }
                                ui.horizontal(|ui| {
                                    let mut checked = marked.contains(&i);
                                    if ui.checkbox(&mut checked, "").on_hover_text("Select for Improve search").changed() {
                                        if checked { marked.insert(i); } else { marked.remove(&i); }
                                    }
                                    let name = entry.rel.rsplit('/').next().unwrap_or(&entry.rel);
                                    if ui.add(egui::Button::selectable(selected == Some(i), name).truncate()).on_hover_text(&entry.rel).clicked() {
                                        opened = Some(i);
                                    }
                                });
                                let summary = entry.side.label.as_ref().map(|label| {
                                    if label.tags.is_empty() { label.caption.clone() } else { label.tags.join(", ") }
                                }).unwrap_or_else(|| "No captions or tags".to_string());
                                ui.add(egui::Label::new(egui::RichText::new(&summary).small().weak()).truncate()).on_hover_text(summary);
                            });
                        });
                    }
                });
            }
        });
        if !self.pending.is_empty() { ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)); }
        opened
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_keeps_rank_and_ignores_invalid_or_duplicate_entries() {
        assert_eq!(ordered_indices(4, &[3, 1, 8, 1, 0]), vec![3, 1, 0]);
        assert!(ordered_indices(3, &[]).is_empty());
    }

    #[test]
    fn thumbnail_preserves_aspect_ratio_and_bounds_size() {
        let unique = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("tilepicky-thumb-{}-{unique}.png", std::process::id()));
        image::RgbaImage::new(1024, 128).save(&path).unwrap();
        let result = thumbnail(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(result.unwrap().size, [256, 32]);
    }

    #[test]
    fn missing_thumbnail_is_an_error() {
        assert!(thumbnail(std::path::Path::new("/nonexistent-tilepicky-thumbnail.png")).is_err());
    }
}
