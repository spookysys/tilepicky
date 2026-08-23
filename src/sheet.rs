//! One sheet on screen: an image on a 32 px grid with a selection. The source
//! panel and your own tilemap are the same thing; only the edits differ.

use crate::index::{matches, path_words, Names};
use crate::sidecar::{self, Animation, Cell, Sidecar, TILE};
use eframe::egui::{self, Color32, Id, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Ui, Vec2};
use image::{Rgba, RgbaImage};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A GPU texture side; big sheets are drawn in chunks of this size.
const CHUNK: u32 = 2048;
const ZOOMS: [f32; 10] = [0.25, 0.5, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0];
/// Touchpads report points, not wheel clicks; this many points make one zoom step.
const POINTS_PER_STEP: f32 = 50.0;

/// A zoom level that steps through fixed values. Every view has its own.
#[derive(Clone, Copy, Debug)]
pub struct Zoom {
    pub level: f32,
    /// Touchpad points collected between steps.
    acc: f32,
}

impl Zoom {
    pub fn new(level: f32) -> Self {
        Self { level, acc: 0.0 }
    }

    pub fn step(&mut self, dir: i32) {
        let i = ZOOMS.iter().position(|&z| z >= self.level).unwrap_or(0) as i32 + dir;
        self.level = ZOOMS[i.clamp(0, ZOOMS.len() as i32 - 1) as usize];
    }

    /// Steps for each Ctrl+wheel click this frame. Returns the number of steps taken.
    pub fn wheel(&mut self, ui: &Ui) -> i32 {
        let mut steps = 0;
        ui.input(|i| {
            for e in &i.events {
                if let egui::Event::MouseWheel { unit, delta, modifiers, .. } = e {
                    if !modifiers.ctrl {
                        continue;
                    }
                    match unit {
                        egui::MouseWheelUnit::Line => steps += delta.y.signum() as i32,
                        egui::MouseWheelUnit::Point | egui::MouseWheelUnit::Page => self.acc += delta.y / POINTS_PER_STEP,
                    }
                }
            }
        });
        let whole = self.acc.trunc();
        self.acc -= whole;
        steps += whole as i32;
        if steps != 0 {
            self.step(steps);
        }
        steps
    }
}

/// A rectangle of cells, inclusive on both ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Area {
    fn between(a: (u32, u32), b: (u32, u32)) -> Self {
        Self { x0: a.0.min(b.0), y0: a.1.min(b.1), x1: a.0.max(b.0), y1: a.1.max(b.1) }
    }
    pub fn cols(&self) -> u32 {
        self.x1 - self.x0 + 1
    }
    pub fn rows(&self) -> u32 {
        self.y1 - self.y0 + 1
    }
}

/// The selected cells. Any shape; the bounding area gives it a position.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sel {
    cells: BTreeSet<(u32, u32)>,
}

impl Sel {
    pub fn rect(a: (u32, u32), b: (u32, u32)) -> Self {
        let r = Area::between(a, b);
        let cells = (r.y0..=r.y1).flat_map(|y| (r.x0..=r.x1).map(move |x| (x, y))).collect();
        Self { cells }
    }
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
    pub fn len(&self) -> usize {
        self.cells.len()
    }
    pub fn contains(&self, c: (u32, u32)) -> bool {
        self.cells.contains(&c)
    }
    pub fn toggle(&mut self, c: (u32, u32)) {
        if !self.cells.remove(&c) {
            self.cells.insert(c);
        }
    }
    pub fn union(mut self, other: &Sel) -> Self {
        self.cells.extend(other.cells.iter().copied());
        self
    }
    pub fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.cells.iter().copied()
    }
    pub fn bounds(&self) -> Option<Area> {
        let x0 = self.cells.iter().map(|c| c.0).min()?;
        let x1 = self.cells.iter().map(|c| c.0).max()?;
        let y0 = self.cells.iter().map(|c| c.1).min()?;
        let y1 = self.cells.iter().map(|c| c.1).max()?;
        Some(Area { x0, y0, x1, y1 })
    }
    /// The top-left of the bounding area.
    pub fn origin(&self) -> Option<(u32, u32)> {
        self.bounds().map(|b| (b.x0, b.y0))
    }
}

/// A strip that is not stored: the selection, played with a frame count.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub area: Area,
    pub frames: u32,
    pub ms: u32,
}

impl Draft {
    pub fn animation(&self) -> Animation {
        let b = self.area;
        Animation { x: b.x0, y: b.y0, w: b.cols() / self.frames, h: b.rows(), frames: self.frames, ms: self.ms }
    }
}

/// What the pointer did on a sheet this frame.
#[derive(Default)]
pub struct ViewEvent {
    pub interacted: bool,
    /// A block drag began; the value is the grabbed cell, relative to the selection.
    pub drag_block: Option<(u32, u32)>,
    /// A canvas resize drag ended.
    pub resized: bool,
}

/// What Ctrl+C carries: pixels plus the origin of every cell.
pub struct Block {
    pub cols: u32,
    pub rows: u32,
    pub img: RgbaImage,
    pub cells: Vec<Option<Cell>>,
    /// Which cells of the bounding area belong to the block.
    pub mask: Vec<bool>,
    /// Stored animations that lie inside the block, relative to its top-left.
    pub animations: Vec<Animation>,
}

impl Block {
    /// One line that says what the block is, for the system clipboard.
    pub fn note(&self) -> String {
        let mut from: Vec<&str> = self.cells.iter().flatten().filter_map(|c| c.src.as_deref()).collect();
        from.sort_unstable();
        from.dedup();
        format!("tilepick: {}x{} cells from {}", self.cols, self.rows, from.join(", "))
    }
}

pub struct Sheet {
    pub rel: String,
    /// The directory whose book describes this sheet.
    pub dir: PathBuf,
    pub img: RgbaImage,
    /// The book entry: tags, origins, animations. Saved with the sheet.
    pub side: Sidecar,
    /// Derived names of cells from the indexer. Read only.
    pub names: Names,
    pub words: Vec<String>,
    chunks: Vec<(Rect, TextureHandle)>,
    pub sel: Sel,
    anchor: Option<(u32, u32)>,
    /// The selection when a range drag began; the dragged rectangle is added to it.
    base: Sel,
    /// An edge drag in progress: the fixed x and y of the bounding area, if that axis moves.
    resize: Option<(Option<u32>, Option<u32>)>,
    /// A canvas edge drag in progress: which of width and height follow the pointer.
    canvas_resize: Option<(bool, bool)>,
    pub zoom: Zoom,
    pub hits: Vec<bool>,
    pub hover: Option<(u32, u32)>,
    scroll_to: Option<Vec2>,
    /// Where the sheet was drawn last frame, in screen space, and its visible part.
    pub screen: Rect,
    pub clip: Rect,
    /// Edits since the last save.
    pub dirty: bool,
    /// How the current selection plays as a strip, before it is stored.
    draft: Option<Draft>,
    /// The animation panel is open. It also opens by itself when animations are stored.
    pub anim_panel: bool,
    pub preview_zoom: Zoom,
    /// The pointer was over the animation preview last frame.
    pub preview_hovered: bool,
    undo: Vec<(RgbaImage, Sidecar)>,
}

impl Sheet {
    pub fn open(ctx: &egui::Context, dir: &Path, rel: &str, side: Sidecar, names: Names, words: Vec<String>) -> Result<Self, String> {
        let img = image::open(dir.join(rel)).map_err(|e| format!("{rel}: {e}"))?.to_rgba8();
        Ok(Self::from_image(ctx, dir, rel, img, side, names, words))
    }

    pub fn new_empty(ctx: &egui::Context, dir: &Path, rel: &str, cols: u32, rows: u32) -> Self {
        let img = RgbaImage::new(cols * TILE, rows * TILE);
        Self::from_image(ctx, dir, rel, img, Sidecar::default(), Names::new(), path_words(rel))
    }

    fn from_image(ctx: &egui::Context, dir: &Path, rel: &str, img: RgbaImage, side: Sidecar, names: Names, words: Vec<String>) -> Self {
        let mut s = Self {
            rel: rel.to_string(),
            dir: dir.to_path_buf(),
            img,
            side,
            names,
            words,
            chunks: Vec::new(),
            sel: Sel::default(),
            anchor: None,
            base: Sel::default(),
            resize: None,
            canvas_resize: None,
            zoom: Zoom::new(2.0),
            hits: Vec::new(),
            hover: None,
            scroll_to: None,
            screen: Rect::NOTHING,
            clip: Rect::NOTHING,
            dirty: false,
            draft: None,
            anim_panel: false,
            preview_zoom: Zoom::new(2.0),
            preview_hovered: false,
            undo: Vec::new(),
        };
        s.upload(ctx);
        s
    }

    pub fn cols(&self) -> u32 {
        self.img.width().div_ceil(TILE)
    }
    pub fn rows(&self) -> u32 {
        self.img.height().div_ceil(TILE)
    }

    fn upload(&mut self, ctx: &egui::Context) {
        let (w, h) = self.img.dimensions();
        let side = CHUNK.min(ctx.input(|i| i.max_texture_side) as u32).max(TILE);
        self.chunks.clear();
        let mut y = 0;
        while y < h {
            let ch = side.min(h - y);
            let mut x = 0;
            while x < w {
                let cw = side.min(w - x);
                let sub = image::imageops::crop_imm(&self.img, x, y, cw, ch).to_image();
                let color = egui::ColorImage::from_rgba_unmultiplied([cw as usize, ch as usize], sub.as_raw());
                let tex = ctx.load_texture(format!("{}@{x},{y}", self.rel), color, TextureOptions::NEAREST);
                let rect = Rect::from_min_size(Pos2::new(x as f32, y as f32), Vec2::new(cw as f32, ch as f32));
                self.chunks.push((rect, tex));
                x += cw;
            }
            y += ch;
        }
    }

    /// Marks the cells that match the query. Empty query: no marks.
    pub fn compute_hits(&mut self, query: &[String]) {
        let n = (self.cols() * self.rows()) as usize;
        if query.is_empty() {
            self.hits = vec![false; n];
            return;
        }
        let cols = self.cols();
        self.hits = (0..n)
            .map(|i| {
                let (x, y) = (i as u32 % cols, i as u32 / cols);
                let cell = self.side.get(x, y);
                let names = self.names.get(&Sidecar::key(x, y));
                matches(query, |q| {
                    self.words.iter().any(|w| w.starts_with(q))
                        || names.is_some_and(|ws| ws.iter().any(|w| w.starts_with(q)))
                        || cell.is_some_and(|c| {
                            c.tags.iter().any(|t| t.starts_with(q))
                                || c.src.as_deref().is_some_and(|s| path_words(s).iter().any(|w| w.starts_with(q)))
                        })
                })
            })
            .collect();
    }

    /// All words known for one cell: the book's tags and the derived names.
    pub fn cell_tags(&self, x: u32, y: u32) -> Vec<String> {
        let mut out: Vec<String> = self.side.get(x, y).map(|c| c.tags.clone()).unwrap_or_default();
        for w in self.names.get(&Sidecar::key(x, y)).into_iter().flatten() {
            if !out.contains(w) {
                out.push(w.clone());
            }
        }
        out
    }

    pub fn cell_px(&self) -> f32 {
        TILE as f32 * self.zoom.level
    }

    /// The cell under a screen position. Positions past the right or bottom
    /// edge, but still inside the panel, give cells beyond the canvas, so
    /// that a drop there can grow it.
    pub fn cell_at(&self, p: Pos2) -> Option<(u32, u32)> {
        if !self.clip.contains(p) || p.x < self.screen.min.x || p.y < self.screen.min.y {
            return None;
        }
        let d = (p - self.screen.min) / self.cell_px();
        Some((d.x as u32, d.y as u32))
    }

    /// Draws the sheet and handles pointer input. While a block drag is in
    /// progress (`dragging`), the sheet only draws.
    pub fn view(&mut self, ui: &mut Ui, id: Id, dragging: bool, editable: bool) -> ViewEvent {
        let mut event = ViewEvent::default();
        let zoom = self.zoom.level;
        let cell_px = TILE as f32 * zoom;
        /// Room past the right and bottom edges, where the canvas handles sit.
        const MARGIN: f32 = 12.0;
        let size = Vec2::new(self.cols() as f32 * cell_px, self.rows() as f32 * cell_px);
        let mut area = egui::ScrollArea::both().id_salt((id, "scroll")).auto_shrink([false, false]);
        if let Some(offset) = self.scroll_to.take() {
            area = area.scroll_offset(offset);
        }
        let mut rezoom: Option<(Vec2, f32)> = None;
        let mut own_drag = false;
        let out = area.show(ui, |ui| {
            let margin = if editable { Vec2::splat(MARGIN) } else { Vec2::ZERO };
            let (outer, _) = ui.allocate_exact_size(size + margin, Sense::hover());
            let rect = Rect::from_min_size(outer.min, size);
            let resp = ui.interact(outer, id, Sense::click_and_drag());
            self.screen = rect;
            self.clip = ui.clip_rect();
            let painter = ui.painter_at(rect);
            checkerboard(&painter, rect);
            for (px, tex) in &self.chunks {
                let r = Rect::from_min_size(rect.min + px.min.to_vec2() * zoom, px.size() * zoom);
                painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            let cell_rect = |x: u32, y: u32| {
                Rect::from_min_size(rect.min + Vec2::new(x as f32, y as f32) * cell_px, Vec2::splat(cell_px))
            };
            if cell_px >= 16.0 {
                let grid = Stroke::new(1.0, Color32::from_black_alpha(40));
                for x in 0..=self.cols() {
                    let sx = rect.min.x + x as f32 * cell_px;
                    painter.line_segment([Pos2::new(sx, rect.min.y), Pos2::new(sx, rect.max.y)], grid);
                }
                for y in 0..=self.rows() {
                    let sy = rect.min.y + y as f32 * cell_px;
                    painter.line_segment([Pos2::new(rect.min.x, sy), Pos2::new(rect.max.x, sy)], grid);
                }
            }
            let cols = self.cols();
            for (i, hit) in self.hits.iter().enumerate() {
                if *hit {
                    let r = cell_rect(i as u32 % cols, i as u32 / cols);
                    painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 220, 0, 70));
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_rgb(255, 220, 0)), egui::StrokeKind::Inside);
                }
            }
            let orange = Color32::from_rgb(255, 140, 0);
            for a in &self.side.animations {
                let (x0, y0, x1, y1) = a.area();
                let r = cell_rect(x0, y0).union(cell_rect(x1, y1));
                painter.rect_stroke(r, 0.0, Stroke::new(2.0, orange), egui::StrokeKind::Inside);
                for f in 1..a.frames {
                    let sx = rect.min.x + (a.x + f * a.w) as f32 * cell_px;
                    painter.line_segment([Pos2::new(sx, r.min.y), Pos2::new(sx, r.max.y)], Stroke::new(1.0, orange));
                }
                painter.text(
                    r.left_bottom() + Vec2::new(3.0, -2.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{} frames of {}x{}", a.frames, a.w, a.h),
                    egui::FontId::proportional(11.0),
                    orange,
                );
            }
            let blue = Color32::from_rgb(80, 160, 255);
            for (x, y) in self.sel.iter() {
                let r = cell_rect(x, y);
                painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(80, 160, 255, 50));
                // A border only where the neighbour is outside the selection.
                let w = 2.0;
                if !self.sel.contains((x, y.wrapping_sub(1))) {
                    painter.line_segment([r.left_top(), r.right_top()], Stroke::new(w, blue));
                }
                if !self.sel.contains((x, y + 1)) {
                    painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(w, blue));
                }
                if !self.sel.contains((x.wrapping_sub(1), y)) {
                    painter.line_segment([r.left_top(), r.left_bottom()], Stroke::new(w, blue));
                }
                if !self.sel.contains((x + 1, y)) {
                    painter.line_segment([r.right_top(), r.right_bottom()], Stroke::new(w, blue));
                }
            }

            let (cols, rows) = (self.cols(), self.rows());
            let to_cell = move |p: Pos2| {
                let d = (p - rect.min) / cell_px;
                ((d.x.max(0.0) as u32).min(cols - 1), (d.y.max(0.0) as u32).min(rows - 1))
            };
            self.hover = resp.hover_pos().map(to_cell);
            let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
            if dragging {
                return;
            }
            // Near an edge of the selection the pointer becomes a resize handle.
            let bounds_rect = self.sel.bounds().map(|b| cell_rect(b.x0, b.y0).union(cell_rect(b.x1, b.y1)));
            let edges_at = |p: Pos2| -> Option<(bool, bool, bool, bool)> {
                const GRIP: f32 = 6.0;
                let r = bounds_rect?;
                if !r.expand(GRIP).contains(p) {
                    return None;
                }
                let e = (
                    (p.x - r.min.x).abs() <= GRIP,
                    (p.x - r.max.x).abs() <= GRIP,
                    (p.y - r.min.y).abs() <= GRIP,
                    (p.y - r.max.y).abs() <= GRIP,
                );
                (e.0 || e.1 || e.2 || e.3).then_some(e)
            };
            // The right and bottom edges of the canvas are handles too.
            let canvas_edge_at = |p: Pos2| -> Option<(bool, bool)> {
                const GRIP: f32 = 5.0;
                if !editable {
                    return None;
                }
                let w = p.x >= rect.max.x - GRIP && p.x <= rect.max.x + MARGIN && p.y <= rect.max.y + MARGIN;
                let h = p.y >= rect.max.y - GRIP && p.y <= rect.max.y + MARGIN && p.x <= rect.max.x + MARGIN;
                (w || h).then_some((w, h))
            };
            let resize_icon = |w: bool, h: bool| match (w, h) {
                (true, true) => egui::CursorIcon::ResizeNwSe,
                (true, false) => egui::CursorIcon::ResizeHorizontal,
                _ => egui::CursorIcon::ResizeVertical,
            };
            if let Some(p) = resp.hover_pos() {
                if let Some((w, h)) = canvas_edge_at(p).filter(|_| edges_at(p).is_none()) {
                    ui.output_mut(|o| o.cursor_icon = resize_icon(w, h));
                } else if let Some((l, r, t, b)) = edges_at(p) {
                    let icon = match (l || r, t || b) {
                        (true, true) if (l && t) || (r && b) => egui::CursorIcon::ResizeNwSe,
                        (true, true) => egui::CursorIcon::ResizeNeSw,
                        (true, false) => egui::CursorIcon::ResizeHorizontal,
                        _ => egui::CursorIcon::ResizeVertical,
                    };
                    ui.output_mut(|o| o.cursor_icon = icon);
                } else if self.sel.contains(to_cell(p)) {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grab);
                }
            }
            if resp.secondary_clicked() {
                self.sel = Sel::default();
                event.interacted = true;
            }
            if resp.drag_started() {
                let press = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
                if let Some(p) = press {
                    let c = to_cell(p);
                    let edges = edges_at(p);
                    if let Some(wh) = canvas_edge_at(p).filter(|_| edges.is_none()) {
                        self.canvas_resize = Some(wh);
                        self.snapshot();
                    } else if let (Some((l, r, t, b)), Some(area)) = (edges, self.sel.bounds()) {
                        // The opposite edge stays; the grabbed edge follows the pointer.
                        let fx = if l { Some(area.x1) } else if r { Some(area.x0) } else { None };
                        let fy = if t { Some(area.y1) } else if b { Some(area.y0) } else { None };
                        self.resize = Some((fx, fy));
                    } else if self.sel.contains(c) {
                        // A drag that starts on the selection carries the block.
                        let o = self.sel.origin().unwrap();
                        event.drag_block = Some((c.0 - o.0, c.1 - o.1));
                    } else {
                        self.base = if ctrl { self.sel.clone() } else { Sel::default() };
                        self.anchor = Some(c);
                        self.sel = self.base.clone().union(&Sel::rect(c, c));
                    }
                    event.interacted = true;
                }
            }
            if resp.dragged() && event.drag_block.is_none() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let c = to_cell(p);
                    if let Some((w, h)) = self.canvas_resize {
                        ui.output_mut(|o| o.cursor_icon = resize_icon(w, h));
                        let want = |v: f32| ((v / cell_px).round() as u32).max(1);
                        let cols = if w { want(p.x - rect.min.x) } else { self.cols() };
                        let rows = if h { want(p.y - rect.min.y) } else { self.rows() };
                        if (cols, rows) != (self.cols(), self.rows()) {
                            self.set_size(ui.ctx(), cols, rows);
                        }
                    } else if let (Some((fx, fy)), Some(area)) = (self.resize, self.sel.bounds()) {
                        let (x0, x1) = fx.map_or((area.x0, area.x1), |f| (f.min(c.0), f.max(c.0)));
                        let (y0, y1) = fy.map_or((area.y0, area.y1), |f| (f.min(c.1), f.max(c.1)));
                        self.sel = Sel::rect((x0, y0), (x1, y1));
                    } else if let Some(a) = self.anchor {
                        self.sel = self.base.clone().union(&Sel::rect(a, c));
                    }
                    event.interacted = true;
                }
            }
            if resp.drag_stopped() {
                self.resize = None;
                if self.canvas_resize.take().is_some() {
                    event.resized = true;
                }
            }
            own_drag = resp.dragged() && event.drag_block.is_none();
            if resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let c = to_cell(p);
                    if ctrl && shift {
                        // Add the rectangle from the last clicked cell to this one.
                        let a = self.anchor.unwrap_or(c);
                        self.sel = self.sel.clone().union(&Sel::rect(a, c));
                    } else if ctrl {
                        self.sel.toggle(c);
                        self.anchor = Some(c);
                    } else if shift {
                        let a = self.anchor.unwrap_or(c);
                        self.sel = Sel::rect(a, c);
                    } else {
                        self.anchor = Some(c);
                        self.sel = Sel::rect(c, c);
                    }
                    event.interacted = true;
                }
            }
            if event.interacted {
                resp.request_focus();
            }
            if resp.hovered() {
                let before = self.zoom.level;
                if self.zoom.wheel(ui) != 0 {
                    if let Some(p) = resp.hover_pos() {
                        rezoom = Some((p - rect.min, self.zoom.level / before));
                    }
                }
            }
        });
        // Keep the cell under the pointer where it is.
        if let Some((at, k)) = rezoom {
            self.scroll_to = Some(out.state.offset + at * (k - 1.0));
        }
        // Dragging near or past the edge of the view scrolls it.
        let pointer = ui.input(|i| i.pointer.latest_pos());
        let scroll_drag = own_drag || (dragging && pointer.is_some_and(|p| self.clip.contains(p)));
        if let (true, Some(p)) = (scroll_drag, pointer) {
            const EDGE: f32 = 24.0;
            let zone = self.clip.shrink(EDGE);
            let inside = Pos2::new(p.x.clamp(zone.min.x, zone.max.x), p.y.clamp(zone.min.y, zone.max.y));
            let out_by = p - inside;
            if out_by != Vec2::ZERO {
                let step = Vec2::new(out_by.x.clamp(-40.0, 40.0), out_by.y.clamp(-40.0, 40.0)) * 0.4;
                self.scroll_to = Some(out.state.offset + step);
                ui.ctx().request_repaint();
            }
        }
        event
    }

    /// Texture and UV of one cell, for a preview.
    pub fn cell_uv(&self, x: u32, y: u32) -> Option<(egui::TextureId, Rect)> {
        let p = Pos2::new((x * TILE) as f32, (y * TILE) as f32);
        let (r, tex) = self.chunks.iter().find(|(r, _)| r.contains(p))?;
        let min = Pos2::new((p.x - r.min.x) / r.width(), (p.y - r.min.y) / r.height());
        let size = Vec2::new(TILE as f32 / r.width(), TILE as f32 / r.height());
        Some((tex.id(), Rect::from_min_size(min, size)))
    }

    // --- edits --------------------------------------------------------------

    pub fn copy(&self) -> Option<Block> {
        let b = self.sel.bounds()?;
        let mut img = RgbaImage::new(b.cols() * TILE, b.rows() * TILE);
        let mut cells = Vec::new();
        let mut mask = Vec::new();
        for y in b.y0..=b.y1 {
            for x in b.x0..=b.x1 {
                let chosen = self.sel.contains((x, y));
                mask.push(chosen);
                if !chosen {
                    cells.push(None);
                    continue;
                }
                let mut empty = true;
                for py in 0..TILE {
                    for px in 0..TILE {
                        let (ix, iy) = (x * TILE + px, y * TILE + py);
                        let p = if ix < self.img.width() && iy < self.img.height() { *self.img.get_pixel(ix, iy) } else { Rgba([0; 4]) };
                        empty &= p.0[3] == 0;
                        img.put_pixel((x - b.x0) * TILE + px, (y - b.y0) * TILE + py, p);
                    }
                }
                let known = self.side.get(x, y).cloned();
                let tags = self.cell_tags(x, y);
                cells.push(match known {
                    Some(c) => Some(Cell { src: c.src.or_else(|| Some(self.rel.clone())), at: c.at.or(Some([x, y])), tags }),
                    None if empty => None,
                    None => Some(Cell { src: Some(self.rel.clone()), at: Some([x, y]), tags }),
                });
            }
        }
        let animations = self
            .side
            .animations
            .iter()
            .filter(|a| {
                let (x0, y0, x1, y1) = a.area();
                x0 >= b.x0 && y0 >= b.y0 && x1 <= b.x1 && y1 <= b.y1
            })
            .map(|a| Animation { x: a.x - b.x0, y: a.y - b.y0, ..a.clone() })
            .collect();
        Some(Block { cols: b.cols(), rows: b.rows(), img, cells, mask, animations })
    }

    fn snapshot(&mut self) {
        self.undo.push((self.img.clone(), self.side.clone()));
        if self.undo.len() > 64 {
            self.undo.remove(0);
        }
        self.dirty = true;
    }

    /// Grows the canvas so that it holds the given cell count.
    fn grow(&mut self, cols: u32, rows: u32) {
        if cols <= self.cols() && rows <= self.rows() {
            return;
        }
        let mut img = RgbaImage::new(cols.max(self.cols()) * TILE, rows.max(self.rows()) * TILE);
        image::imageops::replace(&mut img, &self.img, 0, 0);
        self.img = img;
    }

    /// Writes the masked cells of the block and selects them.
    fn put(&mut self, at: (u32, u32), block: &Block) {
        self.grow(at.0 + block.cols, at.1 + block.rows);
        let mut sel = Sel::default();
        for (i, cell) in block.cells.iter().enumerate() {
            if !block.mask[i] {
                continue;
            }
            let (bx, by) = (i as u32 % block.cols, i as u32 / block.cols);
            let (x, y) = (at.0 + bx, at.1 + by);
            let tile = image::imageops::crop_imm(&block.img, bx * TILE, by * TILE, TILE, TILE);
            image::imageops::replace(&mut self.img, &*tile, (x * TILE) as i64, (y * TILE) as i64);
            self.side.set(x, y, cell.clone());
            sel.toggle((x, y));
        }
        for a in &block.animations {
            let a = Animation { x: a.x + at.0, y: a.y + at.1, ..a.clone() };
            let (x0, y0, x1, y1) = a.area();
            self.side.animations.retain(|o| {
                let (ox0, oy0, ox1, oy1) = o.area();
                ox1 < x0 || ox0 > x1 || oy1 < y0 || oy0 > y1
            });
            self.side.animations.push(a);
        }
        self.side.animations.sort_by_key(|a| (a.y, a.x));
        self.sel = sel;
    }

    fn clear_cells(&mut self, s: &Sel) {
        for (x, y) in s.iter() {
            for py in 0..TILE {
                for px in 0..TILE {
                    let (ix, iy) = (x * TILE + px, y * TILE + py);
                    if ix < self.img.width() && iy < self.img.height() {
                        self.img.put_pixel(ix, iy, Rgba([0; 4]));
                    }
                }
            }
            self.side.set(x, y, None);
        }
        self.side.animations.retain(|a| !s.iter().any(|(x, y)| a.contains(x, y)));
    }

    pub fn paste(&mut self, ctx: &egui::Context, at: (u32, u32), block: &Block) {
        self.snapshot();
        self.put(at, block);
        self.upload(ctx);
    }

    /// Clears `from`, then puts the block at `at`: one undo step.
    pub fn move_block(&mut self, ctx: &egui::Context, from: &Sel, at: (u32, u32), block: &Block) {
        self.snapshot();
        self.clear_cells(from);
        self.put(at, block);
        self.upload(ctx);
    }

    pub fn clear_selection(&mut self, ctx: &egui::Context) {
        if self.sel.is_empty() {
            return;
        }
        self.snapshot();
        let s = self.sel.clone();
        self.clear_cells(&s);
        self.upload(ctx);
    }

    pub fn undo(&mut self, ctx: &egui::Context) {
        if let Some((img, side)) = self.undo.pop() {
            self.img = img;
            self.side = side;
            self.dirty = true;
            self.upload(ctx);
        }
    }

    /// Marks the selected area as an animation strip, or unmarks it. A new
    /// strip starts with one frame per column; set the frame count afterwards.
    /// The draft strip for the current selection. It resets when the
    /// selection bounds change, and keeps its frame count otherwise.
    pub fn draft(&mut self) -> Option<&mut Draft> {
        let b = self.sel.bounds()?;
        if self.draft.as_ref().is_none_or(|d| d.area != b) {
            self.draft = Some(Draft { area: b, frames: b.cols(), ms: 100 });
        }
        self.draft.as_mut()
    }

    /// The panel shows when the selection touches a stored animation, or
    /// while a draft opened with `A` still matches the selection.
    pub fn show_anim_panel(&self) -> bool {
        let on_stored = self.sel.iter().any(|(x, y)| self.side.animation_at(x, y).is_some());
        let on_draft = self.anim_panel && self.draft.as_ref().is_some_and(|d| Some(d.area) == self.sel.bounds());
        on_stored || on_draft
    }

    /// Opens the panel with a draft for the current selection.
    pub fn open_anim_panel(&mut self) {
        self.anim_panel = true;
        self.draft();
    }

    /// The stored animation under the selection, if there is one.
    pub fn stored_animation(&self) -> Option<Animation> {
        let (x, y) = self.sel.origin()?;
        self.side.animation_at(x, y).cloned()
    }

    /// Stores the draft strip as an animation, or removes the stored one
    /// that covers the same area.
    pub fn toggle_animation(&mut self) -> Result<(), String> {
        let Some(b) = self.sel.bounds() else { return Ok(()) };
        let same = |a: &Animation| a.area() == (b.x0, b.y0, b.x1, b.y1);
        if self.side.animations.iter().any(same) {
            self.snapshot();
            self.side.animations.retain(|a| !same(a));
            return Ok(());
        }
        let Some(d) = self.draft().cloned() else { return Ok(()) };
        if b.cols() % d.frames != 0 {
            return Err(format!("the strip is {} cells wide; the frame count must divide that", b.cols()));
        }
        self.snapshot();
        self.side.animations.retain(|a| !self.sel.iter().any(|(x, y)| a.contains(x, y)));
        self.side.animations.push(d.animation());
        self.side.animations.sort_by_key(|a| (a.y, a.x));
        Ok(())
    }

    /// Changes the frame count of the animation under the selection. The strip
    /// keeps its width, so the count must divide it.
    pub fn set_animation(&mut self, frames: u32, ms: u32) -> Result<(), String> {
        let Some((x, y)) = self.sel.origin() else { return Ok(()) };
        let Some(a) = self.side.animation_at(x, y) else { return Ok(()) };
        let width = a.w * a.frames;
        if frames == 0 || width % frames != 0 {
            return Err(format!("the strip is {width} cells wide; the frame count must divide that"));
        }
        self.snapshot();
        let a = self.side.animation_at_mut(x, y).unwrap();
        a.w = width / frames;
        a.frames = frames;
        a.ms = ms.max(1);
        Ok(())
    }

    /// Sets the canvas size in cells. Cells outside the new size are lost,
    /// with their origins and animations. No snapshot: the caller takes one.
    fn set_size(&mut self, ctx: &egui::Context, cols: u32, rows: u32) {
        let mut img = RgbaImage::new(cols * TILE, rows * TILE);
        let keep = image::imageops::crop_imm(&self.img, 0, 0, (cols * TILE).min(self.img.width()), (rows * TILE).min(self.img.height()));
        image::imageops::replace(&mut img, &*keep, 0, 0);
        self.img = img;
        self.side.cells.retain(|k, _| {
            k.split_once(',').and_then(|(x, y)| Some((x.parse::<u32>().ok()?, y.parse::<u32>().ok()?))).is_some_and(|(x, y)| x < cols && y < rows)
        });
        self.side.animations.retain(|a| {
            let (_, _, x1, y1) = a.area();
            x1 < cols && y1 < rows
        });
        self.sel = Sel::default();
        self.upload(ctx);
    }

    /// Cuts empty columns on the right and empty rows at the bottom. At least
    /// one cell stays.
    pub fn trim(&mut self, ctx: &egui::Context) {
        let used = |x: u32, y: u32| {
            self.side.get(x, y).is_some()
                || (0..TILE).any(|py| (0..TILE).any(|px| self.img.get_pixel(x * TILE + px, y * TILE + py).0[3] > 0))
        };
        let (cols, rows) = (self.cols(), self.rows());
        let mut new_cols = 0;
        let mut new_rows = 0;
        for y in 0..rows {
            for x in 0..cols {
                if used(x, y) {
                    new_cols = new_cols.max(x + 1);
                    new_rows = new_rows.max(y + 1);
                }
            }
        }
        for a in &self.side.animations {
            let (_, _, x1, y1) = a.area();
            new_cols = new_cols.max(x1 + 1);
            new_rows = new_rows.max(y1 + 1);
        }
        let (new_cols, new_rows) = (new_cols.max(1), new_rows.max(1));
        if new_cols == cols && new_rows == rows {
            return;
        }
        self.snapshot();
        self.img = image::imageops::crop_imm(&self.img, 0, 0, new_cols * TILE, new_rows * TILE).to_image();
        self.sel = Sel::default();
        self.upload(ctx);
    }

    /// Writes the image and the book entry.
    pub fn save(&mut self) -> Result<(), String> {
        self.img.save(self.dir.join(&self.rel)).map_err(|e| e.to_string())?;
        self.save_entry()
    }

    /// Writes only the book entry; for sheets whose pixels are not edited.
    pub fn save_entry(&mut self) -> Result<(), String> {
        sidecar::store_entry(&self.dir, &self.rel, &self.side)?;
        self.dirty = false;
        Ok(())
    }
}

/// Light and lighter squares behind transparent pixels, drawn only where visible.
fn checkerboard(painter: &egui::Painter, rect: Rect) {
    const STEP: f32 = 16.0;
    painter.rect_filled(rect, 0.0, Color32::from_gray(235));
    let dark = Color32::from_gray(215);
    let vis = painter.clip_rect().intersect(rect);
    if vis.is_negative() {
        return;
    }
    let x0 = ((vis.min.x - rect.min.x) / STEP).floor() as i32;
    let x1 = ((vis.max.x - rect.min.x) / STEP).ceil() as i32;
    let y0 = ((vis.min.y - rect.min.y) / STEP).floor() as i32;
    let y1 = ((vis.max.y - rect.min.y) / STEP).ceil() as i32;
    for y in y0..y1 {
        for x in x0..x1 {
            if (x + y) % 2 == 0 {
                let min = rect.min + Vec2::new(x as f32, y as f32) * STEP;
                painter.rect_filled(Rect::from_min_size(min, Vec2::splat(STEP)).intersect(rect), 0.0, dark);
            }
        }
    }
}
