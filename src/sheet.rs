// SPDX-License-Identifier: GPL-3.0-or-later
//! One sheet on screen: an image on a 32 px grid with a selection. The source
//! panel and your own tilemap are the same thing; only the edits differ.

use crate::sidecar::{self, Animation, Pair, Provenance, Sidecar};
use eframe::egui::{self, Color32, Id, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Ui, Vec2};
use image::{Rgba, RgbaImage};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A GPU texture side; big sheets are drawn in chunks of this size.
const CHUNK: u32 = 2048;
const ZOOMS: [f32; 10] = [0.25, 0.5, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0];
/// Touchpads report points, not wheel clicks; this many points make one zoom step.
const POINTS_PER_STEP: f32 = 50.0;

/// The zoom that keeps the pixels even. One image pixel must cover a whole
/// number of screen pixels, or a whole number of image pixels must cover one
/// screen pixel. With nearest sampling any other factor makes some rows and
/// columns of the image thicker than their neighbours.
fn even_zoom(level: f32, ppp: f32) -> f32 {
    let s = level * ppp;
    let even = if s >= 1.0 { s.round() } else { 1.0 / (1.0 / s).round().max(1.0) };
    even / ppp
}

/// A point moved onto the screen's pixel grid. The sheet must start on a
/// whole screen pixel for the same reason.
fn even_pos(p: Pos2, ppp: f32) -> Pos2 {
    Pos2::new((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp)
}

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
                        // f32::signum maps 0.0 to +1; a tilt-only event must
                        // not count as a zoom click.
                        egui::MouseWheelUnit::Line if delta.y > 0.0 => steps += 1,
                        egui::MouseWheelUnit::Line if delta.y < 0.0 => steps -= 1,
                        egui::MouseWheelUnit::Line => {}
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
    /// Adds (`on`) or removes the cells of a rectangle.
    pub fn set_rect(&mut self, cols: std::ops::RangeInclusive<u32>, rows: std::ops::RangeInclusive<u32>, on: bool) {
        for y in rows {
            for x in cols.clone() {
                if on {
                    self.cells.insert((x, y));
                } else {
                    self.cells.remove(&(x, y));
                }
            }
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

/// A block of frames that is not stored: the selection, played with a frame
/// grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub area: Area,
    /// Frames in a row, and the number of rows.
    pub frames: [u32; 2],
    pub ms: u32,
}

impl Draft {
    pub fn animation(&self, tile: [u32; 2]) -> Animation {
        let b = self.area;
        let [c, r] = self.frames;
        Animation {
            px: [b.x0 * tile[0], b.y0 * tile[1]],
            frame: [b.cols() * tile[0] / c.max(1), b.rows() * tile[1] / r.max(1)],
            frames: Pair::strip(self.frames),
            ms: self.ms,
        }
    }

    /// The selection in pixels, and whether the frame grid divides it.
    pub fn area_px(&self, tile: [u32; 2]) -> [u32; 2] {
        [self.area.cols() * tile[0], self.area.rows() * tile[1]]
    }
    pub fn fits(&self, tile: [u32; 2]) -> bool {
        let [w, h] = self.area_px(tile);
        let [c, r] = self.frames;
        c > 0 && r > 0 && w % c == 0 && h % r == 0
    }
}

/// One straight piece of the selection's boundary: a grid line, the run of
/// cells along it, and which way is out of the selection.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub line: u32,
    /// Outward means increasing x (or y).
    pub far: bool,
    pub extent: std::ops::RangeInclusive<u32>,
}

/// An edge of the selection being dragged. An edit rectangle spans from the
/// clicked edge line to the pointer, across the edge's extent. Dragged
/// outward it adds its cells; dragged inward it removes them.
#[derive(Clone, Debug)]
struct EdgeDrag {
    base: Sel,
    /// A vertical edge (a run of cells in a column), if one was grabbed.
    ex: Option<Edge>,
    /// A horizontal edge (a run of cells in a row), if one was grabbed.
    ey: Option<Edge>,
}

impl EdgeDrag {
    /// The selection with the edit rectangle applied, for the pointer at grid
    /// lines `lx`, `ly`.
    fn apply(&self, lx: u32, ly: u32) -> Sel {
        let mut sel = self.base.clone();
        let span = |e: &Edge, at: u32| {
            let outward = (at > e.line) == e.far;
            (e.line.min(at)..=e.line.max(at) - 1, outward)
        };
        let cols = self.ex.as_ref().filter(|e| lx != e.line).map(|e| span(e, lx));
        let rows = self.ey.as_ref().filter(|e| ly != e.line).map(|e| span(e, ly));
        if let (Some((cols, outward)), Some(e)) = (&cols, &self.ex) {
            sel.set_rect(cols.clone(), e.extent.clone(), *outward);
        }
        if let (Some((rows, outward)), Some(e)) = (&rows, &self.ey) {
            sel.set_rect(e.extent.clone(), rows.clone(), *outward);
        }
        // A corner dragged outward on both axes also adds the corner block.
        if let (Some((cols, true)), Some((rows, true))) = (cols, rows) {
            sel.set_rect(cols, rows, true);
        }
        sel
    }
}

impl Sel {
    /// The vertical boundary edge on grid line `line` that passes row `y`:
    /// selected on one side, not on the other, extended up and down while
    /// that holds.
    fn vertical_edge(&self, line: u32, y: u32) -> Option<Edge> {
        let side = |yy: u32| {
            let left = line > 0 && self.contains((line - 1, yy));
            let right = self.contains((line, yy));
            match (left, right) {
                (true, false) => Some(true),
                (false, true) => Some(false),
                _ => None,
            }
        };
        let far = side(y)?;
        let mut y0 = y;
        while y0 > 0 && side(y0 - 1) == Some(far) {
            y0 -= 1;
        }
        let mut y1 = y;
        while side(y1 + 1) == Some(far) {
            y1 += 1;
        }
        Some(Edge { line, far, extent: y0..=y1 })
    }

    /// The horizontal boundary edge on grid line `line` that passes column `x`.
    fn horizontal_edge(&self, line: u32, x: u32) -> Option<Edge> {
        let side = |xx: u32| {
            let above = line > 0 && self.contains((xx, line - 1));
            let below = self.contains((xx, line));
            match (above, below) {
                (true, false) => Some(true),
                (false, true) => Some(false),
                _ => None,
            }
        };
        let far = side(x)?;
        let mut x0 = x;
        while x0 > 0 && side(x0 - 1) == Some(far) {
            x0 -= 1;
        }
        let mut x1 = x;
        while side(x1 + 1) == Some(far) {
            x1 += 1;
        }
        Some(Edge { line, far, extent: x0..=x1 })
    }
}

/// What the pointer did on a sheet this frame.
#[derive(Default)]
pub struct ViewEvent {
    pub interacted: bool,
    /// A block drag began by holding on this cell.
    pub drag_block: Option<(u32, u32)>,
    /// A canvas resize drag ended.
    pub resized: bool,
    /// Right click inside the selection: delete its content, keep the selection.
    pub delete: bool,
}

/// Which source file each pixel came from: an index into `sources`, or -1.
/// The working model of provenance; the rectangles in the book are only its
/// saved form.
#[derive(Clone, Default)]
pub struct ProvMap {
    pub sources: Vec<String>,
    idx: Vec<i32>,
    w: u32,
    h: u32,
}

impl ProvMap {
    pub fn new(w: u32, h: u32) -> Self {
        Self { sources: Vec::new(), idx: vec![-1; (w * h) as usize], w, h }
    }

    /// Paints the book's rectangles; later entries win where they overlap.
    pub fn from_side(w: u32, h: u32, provenance: &[Provenance]) -> Self {
        let mut m = Self::new(w, h);
        for p in provenance {
            let v = m.intern(&p.source);
            for r in &p.rects {
                m.fill(r[0], r[1], r[2], r[3], v);
            }
        }
        m
    }

    pub fn intern(&mut self, name: &str) -> i32 {
        match self.sources.iter().position(|s| s == name) {
            Some(i) => i as i32,
            None => {
                self.sources.push(name.to_string());
                self.sources.len() as i32 - 1
            }
        }
    }

    pub fn get(&self, x: u32, y: u32) -> Option<&str> {
        if x >= self.w || y >= self.h {
            return None;
        }
        let v = self.idx[(y * self.w + x) as usize];
        (v >= 0).then(|| self.sources[v as usize].as_str())
    }

    /// The raw index at a pixel, for mapping between tables.
    pub fn index_at(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.w || y >= self.h {
            return None;
        }
        let v = self.idx[(y * self.w + x) as usize];
        (v >= 0).then_some(v as usize)
    }

    pub fn set(&mut self, x: u32, y: u32, v: i32) {
        if x < self.w && y < self.h {
            self.idx[(y * self.w + x) as usize] = v;
        }
    }

    pub fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, v: i32) {
        for yy in y..(y + h).min(self.h) {
            for xx in x..(x + w).min(self.w) {
                self.idx[(yy * self.w + xx) as usize] = v;
            }
        }
    }

    pub fn any_in(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> bool {
        (y0..y1.min(self.h)).any(|y| (x0..x1.min(self.w)).any(|x| self.idx[(y * self.w + x) as usize] >= 0))
    }

    /// The same map on a resized canvas; cut or extended with empty.
    pub fn resized(&self, w: u32, h: u32) -> Self {
        let mut m = Self { sources: self.sources.clone(), idx: vec![-1; (w * h) as usize], w, h };
        for y in 0..h.min(self.h) {
            for x in 0..w.min(self.w) {
                m.idx[(y * w + x) as usize] = self.idx[(y * self.w + x) as usize];
            }
        }
        m
    }

    /// Greedy meshing back to rectangles, grouped by source, for the book.
    pub fn extract(&self) -> Vec<Provenance> {
        let mut scratch = self.idx.clone();
        let mut by_source: std::collections::BTreeMap<&str, Vec<[u32; 4]>> = std::collections::BTreeMap::new();
        for y in 0..self.h {
            for x in 0..self.w {
                let v = scratch[(y * self.w + x) as usize];
                if v < 0 {
                    continue;
                }
                // The run of `v` to the right, then as many equal rows below.
                let mut w = 1;
                while x + w < self.w && scratch[(y * self.w + x + w) as usize] == v {
                    w += 1;
                }
                let mut h = 1;
                'rows: while y + h < self.h {
                    for xx in x..x + w {
                        if scratch[((y + h) * self.w + xx) as usize] != v {
                            break 'rows;
                        }
                    }
                    h += 1;
                }
                for yy in y..y + h {
                    for xx in x..x + w {
                        scratch[(yy * self.w + xx) as usize] = -1;
                    }
                }
                by_source.entry(self.sources[v as usize].as_str()).or_default().push([x, y, w, h]);
            }
        }
        by_source.into_iter().map(|(source, rects)| Provenance { source: source.to_string(), rects }).collect()
    }
}

/// What Ctrl+C carries: pixels plus the origin of every cell.
pub struct Block {
    /// The tile size of the sheet the block was copied from.
    pub tile: [u32; 2],
    pub cols: u32,
    pub rows: u32,
    pub img: RgbaImage,
    /// Which source file each pixel came from.
    pub prov: ProvMap,
    /// Which cells of the bounding area belong to the block.
    pub mask: Vec<bool>,
    /// Stored animations that lie inside the block, relative to its top-left.
    pub animations: Vec<Animation>,
}

impl Block {
    /// One line that says what the block is, for the system clipboard.
    pub fn note(&self) -> String {
        let mut from: Vec<&str> = self.prov.sources.iter().map(String::as_str).collect();
        from.sort_unstable();
        format!("tilepick: {}x{} cells from {}", self.cols, self.rows, from.join(", "))
    }
}

/// A canvas edge drag in progress. It holds the canvas as it was when the
/// drag started, and each step cuts the new size from that copy. A step that
/// makes the canvas smaller therefore does not lose the pixels: a step that
/// makes it larger again gives them back.
struct CanvasDrag {
    /// Which of width and height follow the pointer.
    wh: (bool, bool),
    img: RgbaImage,
    prov: ProvMap,
    side: Sidecar,
}

pub struct Sheet {
    pub rel: String,
    /// The directory whose book describes this sheet.
    pub dir: PathBuf,
    /// The sheet's tile size in pixels: the grid drawn over the bitmap.
    pub tile: [u32; 2],
    /// Pixels between neighbouring tiles, and before the first one.
    pub gap: u32,
    pub offset: [u32; 2],
    /// The value in the header's gap field, applied when the drag ends.
    pub gap_edit: u32,
    /// The last click, for the double-click-and-drag lift shortcut: its
    /// time, its cell, and the selection it replaced.
    last_click: Option<(f64, (u32, u32), Sel)>,
    pub img: RgbaImage,
    /// The book entry: grid, origins, animations. Saved with the sheet.
    pub side: Sidecar,
    chunks: Vec<(Rect, TextureHandle)>,
    pub sel: Sel,
    anchor: Option<(u32, u32)>,
    /// The selection when a range drag began; the dragged rectangle is added to it.
    base: Sel,
    /// An edge drag in progress: the selection and its bounds at the start,
    /// and the fixed x and y of the bounds, for the axes that move.
    resize: Option<EdgeDrag>,
    /// A canvas edge drag in progress.
    canvas_resize: Option<CanvasDrag>,
    pub zoom: Zoom,
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
    /// Screen pixels in one point, as the window reports them. The drawing
    /// needs it to keep the image pixels even.
    pub ppp: f32,
    /// The pointer was over the animation preview last frame.
    pub preview_hovered: bool,
    /// Which source file each pixel came from, in image coordinates.
    pub prov: ProvMap,
    /// The frames of an animated GIF; empty for still images.
    frames: Vec<RgbaImage>,
    frame_ms: u32,
    cur_frame: usize,
    undo: Vec<(RgbaImage, Sidecar, ProvMap)>,
}

impl Sheet {
    /// `inherit` is the grid (tile, gap, offset) to assume for whatever the
    /// entry does not name: the settings of the sheet that was open before.
    pub fn open(ctx: &egui::Context, dir: &Path, rel: &str, inherit: ([u32; 2], u32, [u32; 2]), side: Sidecar) -> Result<Self, String> {
        let path = dir.join(rel);
        let (frames, frame_ms) = if rel.to_ascii_lowercase().ends_with(".gif") { decode_gif(&path) } else { (Vec::new(), 0) };
        let img = match frames.first() {
            Some(f) => f.clone(),
            None => image::open(&path).map_err(|e| format!("{rel}: {e}"))?.to_rgba8(),
        };
        let mut sheet = Self::from_image(ctx, dir, rel, inherit, img, side);
        sheet.frames = frames;
        sheet.frame_ms = frame_ms;
        Ok(sheet)
    }

    pub fn new_empty(ctx: &egui::Context, dir: &Path, rel: &str, tile: [u32; 2], cols: u32, rows: u32) -> Self {
        let img = RgbaImage::new(cols * tile[0], rows * tile[1]);
        Self::from_image(ctx, dir, rel, (tile, 0, [0, 0]), img, Sidecar::default())
    }

    #[allow(clippy::too_many_arguments)]
    fn from_image(ctx: &egui::Context, dir: &Path, rel: &str, inherit: ([u32; 2], u32, [u32; 2]), img: RgbaImage, side: Sidecar) -> Self {
        let tile = side.tile.map(Pair::xy).unwrap_or(inherit.0);
        let prov = ProvMap::from_side(img.width(), img.height(), &side.provenance);
        let mut s = Self {
            rel: rel.to_string(),
            dir: dir.to_path_buf(),
            tile,
            gap: side.gap.unwrap_or(inherit.1),
            offset: side.offset.map(Pair::xy).unwrap_or(inherit.2),
            gap_edit: side.gap.unwrap_or(inherit.1),
            last_click: None,
            img,
            side,
            chunks: Vec::new(),
            sel: Sel::default(),
            anchor: None,
            base: Sel::default(),
            resize: None,
            canvas_resize: None,
            zoom: Zoom::new(2.0),
            hover: None,
            // A sheet that opens starts at its top-left. The scroll area is
            // shared by every sheet of a panel, so without this a new sheet
            // would inherit where the last one stood.
            scroll_to: Some(Vec2::ZERO),
            screen: Rect::NOTHING,
            clip: Rect::NOTHING,
            dirty: false,
            draft: None,
            anim_panel: false,
            preview_zoom: Zoom::new(2.0),
            ppp: 1.0,
            preview_hovered: false,
            prov,
            frames: Vec::new(),
            frame_ms: 0,
            cur_frame: 0,
            undo: Vec::new(),
        };
        s.upload(ctx);
        s
    }

    /// Distance from one tile's origin to the next, per axis, on the image.
    pub fn pitch(&self) -> [u32; 2] {
        [self.tile[0] + self.gap, self.tile[1] + self.gap]
    }
    pub fn cols(&self) -> u32 {
        (self.img.width().saturating_sub(self.offset[0]) + self.gap).div_ceil(self.pitch()[0]).max(1)
    }
    pub fn rows(&self) -> u32 {
        (self.img.height().saturating_sub(self.offset[1]) + self.gap).div_ceil(self.pitch()[1]).max(1)
    }
    /// Where a cell's pixels sit on the image: x0, y0, one past x1, y1.
    fn cell_img_rect(&self, x: u32, y: u32) -> (u32, u32, u32, u32) {
        let p = self.pitch();
        let (ox, oy) = (self.offset[0] + x * p[0], self.offset[1] + y * p[1]);
        (ox, oy, ox + self.tile[0], oy + self.tile[1])
    }

    /// Re-uploads only the pixels inside an image rectangle (x0, y0, one
    /// past x1, y1), into the chunks it touches. Cheap for small edits on a
    /// large canvas.
    fn upload_region(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) {
        let (w, h) = self.img.dimensions();
        let (x1, y1) = (x1.min(w), y1.min(h));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for (rect, tex) in &mut self.chunks {
            let (cx0, cy0) = (rect.min.x as u32, rect.min.y as u32);
            let (cx1, cy1) = (cx0 + rect.width() as u32, cy0 + rect.height() as u32);
            let (ix0, iy0, ix1, iy1) = (x0.max(cx0), y0.max(cy0), x1.min(cx1), y1.min(cy1));
            if ix0 >= ix1 || iy0 >= iy1 {
                continue;
            }
            let sub = image::imageops::crop_imm(&self.img, ix0, iy0, ix1 - ix0, iy1 - iy0).to_image();
            let color = egui::ColorImage::from_rgba_unmultiplied([(ix1 - ix0) as usize, (iy1 - iy0) as usize], sub.as_raw());
            tex.set_partial([(ix0 - cx0) as usize, (iy0 - cy0) as usize], color, TextureOptions::NEAREST);
        }
    }

    fn upload(&mut self, ctx: &egui::Context) {
        let (w, h) = self.img.dimensions();
        let side = CHUNK.min(ctx.input(|i| i.max_texture_side) as u32).max(64);
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

    /// The source file a view cell's pixels came from, if one is recorded.
    pub fn cell_source(&self, x: u32, y: u32) -> Option<&str> {
        let (ix0, iy0, ix1, iy1) = self.cell_img_rect(x, y);
        self.prov.get(ix0, iy0).or_else(|| {
            (iy0..iy1).flat_map(|iy| (ix0..ix1).map(move |ix| (ix, iy))).find_map(|(ix, iy)| self.prov.get(ix, iy))
        })
    }

    /// The zoom the sheet is drawn with: the chosen level, moved to the
    /// nearest factor that keeps the image pixels even.
    pub fn zoom_px(&self) -> f32 {
        even_zoom(self.zoom.level, self.ppp)
    }

    /// The same for the animation preview.
    pub fn preview_zoom_px(&self) -> f32 {
        even_zoom(self.preview_zoom.level, self.ppp)
    }

    /// One cell's screen size, per axis.
    pub fn cell_px(&self) -> Vec2 {
        let p = self.pitch();
        Vec2::new(p[0] as f32, p[1] as f32) * self.zoom_px()
    }

    /// The strip that covers a cell, if one does.
    pub fn animation_at(&self, x: u32, y: u32) -> Option<Animation> {
        let r = self.cell_px_rect(x, y);
        self.side.animations.iter().find(|a| a.px_overlaps(r)).cloned()
    }

    /// The logical pixel rectangle of one cell, as if the sheet were packed:
    /// the coordinate space of cell records and animations. On a sheet
    /// without gaps it equals the image rectangle.
    fn cell_px_rect(&self, x: u32, y: u32) -> (u32, u32, u32, u32) {
        let [tw, th] = self.tile;
        (x * tw, y * th, (x + 1) * tw, (y + 1) * th)
    }

    /// The cell under a screen position. Positions past the right or bottom
    /// edge, but still inside the panel, give cells beyond the canvas, so
    /// that a drop there can grow it.
    pub fn cell_at(&self, p: Pos2) -> Option<(u32, u32)> {
        if !self.clip.contains(p) || p.x < self.screen.min.x || p.y < self.screen.min.y {
            return None;
        }
        let c = self.cell_px();
        let off = Vec2::new(self.offset[0] as f32, self.offset[1] as f32) * self.zoom_px();
        let d = p - self.screen.min - off;
        Some(((d.x / c.x).max(0.0) as u32, (d.y / c.y).max(0.0) as u32))
    }

    /// Draws the sheet and handles pointer input. While a block drag is in
    /// progress (`dragging`), the sheet only draws.
    pub fn view(&mut self, ui: &mut Ui, id: Id, dragging: bool, editable: bool) -> ViewEvent {
        let mut event = ViewEvent::default();
        // An animated GIF plays in place.
        if self.frames.len() > 1 {
            let ms = self.frame_ms.max(20) as u64;
            let k = ((ui.input(|i| i.time) * 1000.0) as u64 / ms) as usize % self.frames.len();
            if k != self.cur_frame {
                self.cur_frame = k;
                self.img = self.frames[k].clone();
                let (w, h) = self.img.dimensions();
                self.upload_region(0, 0, w, h);
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(ms));
        }
        self.ppp = ui.ctx().pixels_per_point();
        let zoom = self.zoom_px();
        let cell_px = self.cell_px();
        let tile_px = Vec2::new(self.tile[0] as f32, self.tile[1] as f32) * zoom;
        let off_px = Vec2::new(self.offset[0] as f32, self.offset[1] as f32) * zoom;
        /// Room past the right and bottom edges, where the canvas handles sit.
        const MARGIN: f32 = 12.0;
        let size = Vec2::new(self.cols() as f32 * cell_px.x, self.rows() as f32 * cell_px.y);
        let mut area = egui::ScrollArea::both().id_salt((id, "scroll")).auto_shrink([false, false]);
        if let Some(offset) = self.scroll_to.take() {
            area = area.scroll_offset(offset);
        }
        let mut rezoom: Option<(Vec2, f32)> = None;
        let mut own_drag = false;
        let out = area.show(ui, |ui| {
            let margin = if editable { Vec2::splat(MARGIN) } else { Vec2::ZERO };
            let (outer, _) = ui.allocate_exact_size(size + margin, Sense::hover());
            // The image starts on a whole screen pixel; see `even_pos`.
            let rect = Rect::from_min_size(even_pos(outer.min, self.ppp), size);
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
                Rect::from_min_size(rect.min + off_px + Vec2::new(x as f32 * cell_px.x, y as f32 * cell_px.y), tile_px)
            };
            if tile_px.min_elem() >= 16.0 {
                let grid = Stroke::new(1.0, Color32::from_black_alpha(40));
                if self.gap == 0 && self.offset == [0, 0] {
                    for x in 0..=self.cols() {
                        let sx = rect.min.x + x as f32 * cell_px.x;
                        painter.line_segment([Pos2::new(sx, rect.min.y), Pos2::new(sx, rect.max.y)], grid);
                    }
                    for y in 0..=self.rows() {
                        let sy = rect.min.y + y as f32 * cell_px.y;
                        painter.line_segment([Pos2::new(rect.min.x, sy), Pos2::new(rect.max.x, sy)], grid);
                    }
                } else {
                    // With gaps the grid is not a set of lines; frame each tile.
                    for y in 0..self.rows() {
                        for x in 0..self.cols() {
                            painter.rect_stroke(cell_rect(x, y), 0.0, grid, egui::StrokeKind::Outside);
                        }
                    }
                }
            }
            let orange = Color32::from_rgb(255, 140, 0);
            for a in &self.side.animations {
                let (x0, y0, x1, y1) = a.px_rect();
                let r = Rect::from_min_max(
                    rect.min + Vec2::new(x0 as f32, y0 as f32) * zoom,
                    rect.min + Vec2::new(x1 as f32, y1 as f32) * zoom,
                );
                painter.rect_stroke(r, 0.0, Stroke::new(2.0, orange), egui::StrokeKind::Inside);
                let [c, rows] = a.grid();
                for f in 1..c {
                    let sx = rect.min.x + (a.px[0] + f * a.frame[0]) as f32 * zoom;
                    painter.line_segment([Pos2::new(sx, r.min.y), Pos2::new(sx, r.max.y)], Stroke::new(1.0, orange));
                }
                for f in 1..rows {
                    let sy = rect.min.y + (a.px[1] + f * a.frame[1]) as f32 * zoom;
                    painter.line_segment([Pos2::new(r.min.x, sy), Pos2::new(r.max.x, sy)], Stroke::new(1.0, orange));
                }
                let count = if rows == 1 { format!("{c}") } else { format!("{c}x{rows}") };
                painter.text(
                    r.left_bottom() + Vec2::new(3.0, -2.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{count} frames of {}x{} px", a.frame[0], a.frame[1]),
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
                let d = p - rect.min - off_px;
                (((d.x / cell_px.x).max(0.0) as u32).min(cols - 1), ((d.y / cell_px.y).max(0.0) as u32).min(rows - 1))
            };
            self.hover = resp.hover_pos().map(to_cell);
            let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
            if dragging {
                return;
            }
            // Near a boundary of the selection the pointer becomes a handle.
            // Both the vertical and the horizontal boundary are looked up.
            let edges_at = |sel: &Sel, p: Pos2| -> Option<(Option<Edge>, Option<Edge>)> {
                const GRIP: f32 = 6.0;
                let (fx, fy) = ((p.x - rect.min.x - off_px.x) / cell_px.x, (p.y - rect.min.y - off_px.y) / cell_px.y);
                let (lx, ly) = (fx.round(), fy.round());
                let c = to_cell(p);
                let near_x = (fx - lx).abs() * cell_px.x <= GRIP && lx >= 0.0 && lx <= cols as f32;
                let near_y = (fy - ly).abs() * cell_px.y <= GRIP && ly >= 0.0 && ly <= rows as f32;
                let ex = near_x.then(|| sel.vertical_edge(lx as u32, c.1)).flatten();
                let ey = near_y.then(|| sel.horizontal_edge(ly as u32, c.0)).flatten();
                (ex.is_some() || ey.is_some()).then_some((ex, ey))
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
                if let Some((w, h)) = canvas_edge_at(p).filter(|_| edges_at(&self.sel, p).is_none()) {
                    ui.output_mut(|o| o.cursor_icon = resize_icon(w, h));
                } else if let Some((ex, ey)) = edges_at(&self.sel, p) {
                    let icon = match (&ex, &ey) {
                        (Some(x), Some(y)) if x.far == y.far => egui::CursorIcon::ResizeNwSe,
                        (Some(_), Some(_)) => egui::CursorIcon::ResizeNeSw,
                        (Some(_), None) => egui::CursorIcon::ResizeHorizontal,
                        _ => egui::CursorIcon::ResizeVertical,
                    };
                    ui.output_mut(|o| o.cursor_icon = icon);
                } else if self.sel.contains(to_cell(p)) {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grab);
                }
            }
            if resp.secondary_clicked() {
                let inside = resp.interact_pointer_pos().is_some_and(|p| self.sel.contains(to_cell(p)));
                if inside && editable {
                    // Delete what is selected; the selection itself stays.
                    event.delete = true;
                } else {
                    self.sel = Sel::default();
                }
                event.interacted = true;
            }
            // Holding still on a tile lifts it: the whole selection when the
            // press is inside it, else the pressed tile alone. A drag that
            // moves before the hold ends is a selection instead.
            if resp.is_pointer_button_down_on() && !resp.dragged() && self.resize.is_none() && self.canvas_resize.is_none() {
                const HOLD_S: f64 = 0.25;
                const STILL_PX: f32 = 4.0;
                let (origin, t0, now, at) = ui.input(|i| (i.pointer.press_origin(), i.pointer.press_start_time(), i.time, i.pointer.latest_pos()));
                if let (Some(o), Some(t0), Some(at)) = (origin, t0, at) {
                    let still = (at - o).length() <= STILL_PX;
                    let plain = edges_at(&self.sel, o).is_none() && canvas_edge_at(o).is_none() && rect.contains(o);
                    // A quick second press on the same cell lifts at once.
                    let double = self
                        .last_click
                        .as_ref()
                        .is_some_and(|(t_click, cell, _)| t0 > *t_click && t0 - t_click < 0.35 && *cell == to_cell(o));
                    if plain && (double || (still && now - t0 >= HOLD_S)) {
                        if double {
                            // The first click replaced the selection; put it
                            // back so the lift takes what was really selected.
                            if let Some((_, _, prev)) = self.last_click.take() {
                                self.sel = prev;
                            }
                        }
                        self.last_click = None;
                        event.drag_block = Some(to_cell(o));
                        event.interacted = true;
                    } else if still && plain {
                        // Keep painting while the hold ripens, even without motion.
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
                    }
                }
            }
            if resp.drag_started() {
                let press = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
                if let Some(p) = press {
                    let c = to_cell(p);
                    let edges = edges_at(&self.sel, p);
                    if let Some(wh) = canvas_edge_at(p).filter(|_| edges.is_none()) {
                        // The undo step goes on the stack when the drag ends;
                        // until then the copy is the base of every step.
                        self.canvas_resize = Some(CanvasDrag {
                            wh,
                            img: self.img.clone(),
                            prov: self.prov.clone(),
                            side: self.side.clone(),
                        });
                        self.dirty = true;
                    } else if let Some((ex, ey)) = edges {
                        self.resize = Some(EdgeDrag { base: self.sel.clone(), ex, ey });
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
                    if let Some((w, h)) = self.canvas_resize.as_ref().map(|d| d.wh) {
                        ui.output_mut(|o| o.cursor_icon = resize_icon(w, h));
                        let want = |v: f32, o: f32, c: f32| (((v - o) / c).round() as u32).max(1);
                        let cols = if w { want(p.x - rect.min.x, off_px.x, cell_px.x) } else { self.cols() };
                        let rows = if h { want(p.y - rect.min.y, off_px.y, cell_px.y) } else { self.rows() };
                        if (cols, rows) != (self.cols(), self.rows()) {
                            self.set_size(ui.ctx(), cols, rows);
                        }
                    } else if let Some(d) = &self.resize {
                        // The pointer snaps to the nearest grid line.
                        let lx = ((p.x - rect.min.x - off_px.x) / cell_px.x).round().clamp(0.0, cols as f32) as u32;
                        let ly = ((p.y - rect.min.y - off_px.y) / cell_px.y).round().clamp(0.0, rows as f32) as u32;
                        self.sel = d.apply(lx, ly);
                    } else if let Some(a) = self.anchor {
                        self.sel = self.base.clone().union(&Sel::rect(a, c));
                    }
                    event.interacted = true;
                }
            }
            if resp.drag_stopped() {
                self.resize = None;
                if let Some(d) = self.canvas_resize.take() {
                    // The whole drag is one undo step: the canvas before it.
                    self.push_undo((d.img, d.side, d.prov));
                    event.resized = true;
                }
            }
            own_drag = resp.dragged() && event.drag_block.is_none();
            if resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let c = to_cell(p);
                    self.last_click = Some((ui.input(|i| i.time), c, self.sel.clone()));
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

    /// Draws a pixel region of the sheet at `to` on screen, scaled by `zoom`.
    /// The region may cross texture chunks.
    pub fn draw_px_rect(&self, painter: &egui::Painter, region: Rect, to: Pos2, zoom: f32) {
        for (r, tex) in &self.chunks {
            let part = r.intersect(region);
            if part.is_negative() {
                continue;
            }
            let uv = Rect::from_min_max(
                Pos2::new((part.min.x - r.min.x) / r.width(), (part.min.y - r.min.y) / r.height()),
                Pos2::new((part.max.x - r.min.x) / r.width(), (part.max.y - r.min.y) / r.height()),
            );
            let screen = Rect::from_min_size(to + (part.min - region.min) * zoom, part.size() * zoom);
            painter.image(tex.id(), screen, uv, Color32::WHITE);
        }
    }

    // --- edits --------------------------------------------------------------

    pub fn copy(&self) -> Option<Block> {
        self.copy_sel(&self.sel.clone())
    }

    /// A block of the given cells; the selection itself is not touched.
    pub fn copy_sel(&self, sel: &Sel) -> Option<Block> {
        // A GIF unrolls into a strip only where something actually moves;
        // a still region is just a picture.
        if self.frames.len() > 1 && self.region_animates(sel) {
            return self.copy_strip(sel);
        }
        let b = sel.bounds()?;
        let [tw, th] = self.tile;
        let mut img = RgbaImage::new(b.cols() * tw, b.rows() * th);
        let mut prov = ProvMap::new(b.cols() * tw, b.rows() * th);
        let own = self.rel.clone();
        let mut mask = Vec::new();
        for y in b.y0..=b.y1 {
            for x in b.x0..=b.x1 {
                let chosen = sel.contains((x, y));
                mask.push(chosen);
                if !chosen {
                    continue;
                }
                let mut empty = true;
                let (ix0, iy0, _, _) = self.cell_img_rect(x, y);
                for py in 0..th {
                    for px in 0..tw {
                        let (ix, iy) = (ix0 + px, iy0 + py);
                        let p = if ix < self.img.width() && iy < self.img.height() { *self.img.get_pixel(ix, iy) } else { Rgba([0; 4]) };
                        empty &= p.0[3] == 0;
                        img.put_pixel((x - b.x0) * tw + px, (y - b.y0) * th + py, p);
                        if let Some(name) = self.prov.get(ix, iy) {
                            let v = prov.intern(name);
                            prov.set((x - b.x0) * tw + px, (y - b.y0) * th + py, v);
                        }
                    }
                }
                // A cell of this sheet itself, unless it is empty or traced.
                if !empty {
                    let v = prov.intern(&own);
                    for py in 0..th {
                        for px in 0..tw {
                            let (bx, by) = ((x - b.x0) * tw + px, (y - b.y0) * th + py);
                            if prov.get(bx, by).is_none() && self.prov.get(ix0 + px, iy0 + py).is_none() {
                                prov.set(bx, by, v);
                            }
                        }
                    }
                }
            }
        }
        // Animations whose pixels lie inside the selection travel along,
        // moved to the block's origin.
        let (px0, py0) = ((b.x0 * tw) as i64, (b.y0 * th) as i64);
        let sel_px = (b.x0 * tw, b.y0 * th, (b.x1 + 1) * tw, (b.y1 + 1) * th);
        let animations = self
            .side
            .animations
            .iter()
            .filter(|a| {
                let r = a.px_rect();
                r.0 >= sel_px.0 && r.1 >= sel_px.1 && r.2 <= sel_px.2 && r.3 <= sel_px.3
            })
            .map(|a| a.shifted(-px0, -py0))
            .collect();
        Some(Block { tile: self.tile, cols: b.cols(), rows: b.rows(), img, prov, mask, animations })
    }

    /// Whether the selected cells differ between any two frames.
    fn region_animates(&self, sel: &Sel) -> bool {
        let first = &self.frames[0];
        let pixel = |f: &RgbaImage, x: u32, y: u32| {
            if x < f.width() && y < f.height() { f.get_pixel(x, y).0 } else { [0; 4] }
        };
        self.frames[1..].iter().any(|frame| {
            sel.iter().any(|(x, y)| {
                let (ix0, iy0, ix1, iy1) = self.cell_img_rect(x, y);
                (iy0..iy1).any(|iy| (ix0..ix1).any(|ix| pixel(first, ix, iy) != pixel(frame, ix, iy)))
            })
        })
    }

    /// The selection cut from every frame of an animated GIF, laid out left
    /// to right: a ready animation strip, marked as one.
    fn copy_strip(&self, sel: &Sel) -> Option<Block> {
        let b = sel.bounds()?;
        let [tw, th] = self.tile;
        let n = self.frames.len() as u32;
        let (fw, fh) = (b.cols() * tw, b.rows() * th);
        let mut img = RgbaImage::new(fw * n, fh);
        let mut prov = ProvMap::new(fw * n, fh);
        let v = prov.intern(&self.rel);
        prov.fill(0, 0, fw * n, fh, v);
        for (f, frame) in self.frames.iter().enumerate() {
            for y in b.y0..=b.y1 {
                for x in b.x0..=b.x1 {
                    if !sel.contains((x, y)) {
                        continue;
                    }
                    let (ix0, iy0, _, _) = self.cell_img_rect(x, y);
                    for py in 0..th {
                        for px in 0..tw {
                            let (ix, iy) = (ix0 + px, iy0 + py);
                            let p = if ix < frame.width() && iy < frame.height() { *frame.get_pixel(ix, iy) } else { Rgba([0; 4]) };
                            img.put_pixel(f as u32 * fw + (x - b.x0) * tw + px, (y - b.y0) * th + py, p);
                        }
                    }
                }
            }
        }
        // The selection's shape repeats for every frame.
        let cols = b.cols() * n;
        let mut mask = vec![false; (cols * b.rows()) as usize];
        for f in 0..n {
            for y in 0..b.rows() {
                for x in 0..b.cols() {
                    mask[(y * cols + f * b.cols() + x) as usize] = sel.contains((b.x0 + x, b.y0 + y));
                }
            }
        }
        let animations = vec![Animation { px: [0, 0], frame: [fw, fh], frames: Pair::strip([n, 1]), ms: self.frame_ms.max(20) }];
        Some(Block { tile: self.tile, cols, rows: b.rows(), img, prov, mask, animations })
    }

    fn snapshot(&mut self) {
        let state = (self.img.clone(), self.side.clone(), self.prov.clone());
        self.push_undo(state);
    }

    fn push_undo(&mut self, state: (RgbaImage, Sidecar, ProvMap)) {
        self.undo.push(state);
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
        let (w, h) = self.img_size_for(cols.max(self.cols()), rows.max(self.rows()));
        let mut img = RgbaImage::new(w, h);
        image::imageops::replace(&mut img, &self.img, 0, 0);
        self.img = img;
        self.prov = self.prov.resized(w, h);
    }

    /// The image size that holds the given cell count on this sheet's grid.
    fn img_size_for(&self, cols: u32, rows: u32) -> (u32, u32) {
        let p = self.pitch();
        (self.offset[0] + cols * p[0] - self.gap, self.offset[1] + rows * p[1] - self.gap)
    }

    /// Writes the block, pixel for pixel, onto this sheet's grid. The block
    /// may come from a sheet with another tile size; it lands with its
    /// top-left on the target cell and is padded with transparent pixels to
    /// whole cells. Unmasked source cells stay holes.
    fn put(&mut self, at: (u32, u32), block: &Block) {
        let ([dw, dh], [sw, sh]) = (self.tile, block.tile);
        let (bw, bh) = block.img.dimensions();
        let (dcols, drows) = (bw.div_ceil(dw), bh.div_ceil(dh));
        self.grow(at.0 + dcols, at.1 + drows);
        let src_masked = |bxp: u32, byp: u32| {
            bxp < bw && byp < bh && block.mask[((byp / sh) * block.cols + bxp / sw) as usize]
        };
        // The block's source names, interned into this sheet's table.
        let mapped: Vec<i32> = block.prov.sources.iter().map(|n| self.prov.intern(n)).collect();
        let mut sel = Sel::default();
        for j in 0..drows {
            for i in 0..dcols {
                let any = (0..dh).any(|py| (0..dw).any(|px| src_masked(i * dw + px, j * dh + py)));
                if !any {
                    continue;
                }
                let (x, y) = (at.0 + i, at.1 + j);
                let (ix0, iy0, _, _) = self.cell_img_rect(x, y);
                for py in 0..dh {
                    for px in 0..dw {
                        let (bxp, byp) = (i * dw + px, j * dh + py);
                        let (v, p) = if src_masked(bxp, byp) {
                            let idx = block.prov.index_at(bxp, byp).map_or(-1, |k| mapped[k]);
                            (*block.img.get_pixel(bxp, byp), idx)
                        } else {
                            (Rgba([0; 4]), -1)
                        };
                        self.img.put_pixel(ix0 + px, iy0 + py, v);
                        self.prov.set(ix0 + px, iy0 + py, p);
                    }
                }
                sel.toggle((x, y));
            }
        }
        for a in &block.animations {
            let a = a.shifted((at.0 * dw) as i64, (at.1 * dh) as i64);
            let r = a.px_rect();
            self.side.animations.retain(|o| !o.px_overlaps(r));
            self.side.animations.push(a);
        }
        self.side.animations.sort_by_key(|a| (a.px[1], a.px[0]));
        self.sel = sel;
    }

    fn clear_cells(&mut self, s: &Sel) {
        let [tw, th] = self.tile;
        for (x, y) in s.iter() {
            let (ix0, iy0, _, _) = self.cell_img_rect(x, y);
            for py in 0..th {
                for px in 0..tw {
                    let (ix, iy) = (ix0 + px, iy0 + py);
                    if ix < self.img.width() && iy < self.img.height() {
                        self.img.put_pixel(ix, iy, Rgba([0; 4]));
                    }
                }
            }
            let (cx0, cy0, cx1, cy1) = self.cell_img_rect(x, y);
            self.prov.fill(cx0, cy0, cx1 - cx0, cy1 - cy0, -1);
        }
        let rects: Vec<_> = s.iter().map(|(x, y)| self.cell_px_rect(x, y)).collect();
        self.side.animations.retain(|a| !rects.iter().any(|r| a.px_overlaps(*r)));
    }

    pub fn paste(&mut self, ctx: &egui::Context, at: (u32, u32), block: &Block) {
        self.snapshot();
        let before = self.img.dimensions();
        self.put(at, block);
        if self.img.dimensions() != before {
            self.upload(ctx);
        } else {
            let (bw, bh) = block.img.dimensions();
            let (x0, y0, _, _) = self.cell_img_rect(at.0, at.1);
            self.upload_region(x0, y0, x0 + bw.div_ceil(self.tile[0]) * self.tile[0], y0 + bh.div_ceil(self.tile[1]) * self.tile[1]);
        }
    }

    /// Clears `from`, then puts the block at `at`: one undo step.
    pub fn move_block(&mut self, ctx: &egui::Context, from: &Sel, at: (u32, u32), block: &Block) {
        self.snapshot();
        let before = self.img.dimensions();
        self.clear_cells(from);
        self.put(at, block);
        if self.img.dimensions() != before {
            self.upload(ctx);
        } else {
            if let Some(b) = from.bounds() {
                let (x0, y0, _, _) = self.cell_img_rect(b.x0, b.y0);
                let (_, _, x1, y1) = self.cell_img_rect(b.x1, b.y1);
                self.upload_region(x0, y0, x1, y1);
            }
            let (bw, bh) = block.img.dimensions();
            let (x0, y0, _, _) = self.cell_img_rect(at.0, at.1);
            self.upload_region(x0, y0, x0 + bw.div_ceil(self.tile[0]) * self.tile[0], y0 + bh.div_ceil(self.tile[1]) * self.tile[1]);
        }
    }

    /// Exchanges the block with the cells it lands on: those cells travel
    /// back to the place the block came from. One undo step.
    pub fn swap_block(&mut self, ctx: &egui::Context, from: &Sel, at: (u32, u32), block: &Block) {
        let Some(b) = from.bounds() else { return };
        // The cells the block covers when it lands, in its own shape.
        let mut there = Sel::default();
        for j in 0..block.rows {
            for i in 0..block.cols {
                if block.mask[(j * block.cols + i) as usize] {
                    there.toggle((at.0 + i, at.1 + j));
                }
            }
        }
        let Some(other) = self.copy_sel(&there) else { return };
        self.move_block(ctx, from, at, block);
        self.put((b.x0, b.y0), &other);
        let (x0, y0, _, _) = self.cell_img_rect(b.x0, b.y0);
        let (_, _, x1, y1) = self.cell_img_rect(b.x1, b.y1);
        self.upload_region(x0, y0, x1, y1);
        // The block the user carried stays selected, at its new place.
        self.sel = there;
    }

    pub fn clear_selection(&mut self, ctx: &egui::Context) {
        if self.sel.is_empty() {
            return;
        }
        let _ = ctx;
        self.snapshot();
        let s = self.sel.clone();
        self.clear_cells(&s);
        if let Some(b) = s.bounds() {
            let (x0, y0, _, _) = self.cell_img_rect(b.x0, b.y0);
            let (_, _, x1, y1) = self.cell_img_rect(b.x1, b.y1);
            self.upload_region(x0, y0, x1, y1);
        }
    }

    pub fn undo(&mut self, ctx: &egui::Context) {
        if let Some((img, side, prov)) = self.undo.pop() {
            self.img = img;
            self.side = side;
            self.prov = prov;
            self.dirty = true;
            self.upload(ctx);
        }
    }

    /// Marks the selected area as an animation strip, or unmarks it. A new
    /// strip starts with one frame per column; set the frame count afterwards.
    /// The draft for the current selection. It follows the selection and
    /// keeps its numbers, so that the user can resize the selection until
    /// the frames divide it.
    pub fn draft(&mut self) -> Option<&mut Draft> {
        let b = self.sel.bounds()?;
        match &mut self.draft {
            Some(d) => d.area = b,
            None => self.draft = Some(Draft { area: b, frames: [b.cols(), 1], ms: 100 }),
        }
        self.draft.as_mut()
    }

    /// The panel shows when the selection touches a stored animation, or
    /// while a draft opened with `A` still matches the selection.
    pub fn show_anim_panel(&self) -> bool {
        let on_stored = self.sel.iter().any(|(x, y)| {
            let r = self.cell_px_rect(x, y);
            self.side.animations.iter().any(|a| a.px_overlaps(r))
        });
        // The panel stays open while there is a selection to play; a new
        // selection does not close it, so its edges can be dragged.
        let on_draft = self.anim_panel && !self.sel.is_empty();
        on_stored || on_draft
    }

    /// Opens the panel with a fresh draft for the current selection.
    pub fn open_anim_panel(&mut self) {
        self.anim_panel = true;
        self.draft = None;
        self.draft();
    }

    /// The animation under the selection, in this sheet's grid.
    pub fn stored_animation(&self) -> Option<Animation> {
        let (x, y) = self.sel.origin()?;
        self.animation_at(x, y)
    }

    /// The stored animation whose pixels equal the given one's.
    fn stored_mut(&mut self, view: &Animation) -> Option<&mut Animation> {
        let r = view.px_rect();
        self.side.animations.iter_mut().find(|a| a.px_rect() == r)
    }

    /// Stores the draft strip as an animation, or removes the stored one
    /// that covers the same pixels.
    pub fn toggle_animation(&mut self) -> Result<(), String> {
        let Some(b) = self.sel.bounds() else { return Ok(()) };
        let [tw, th] = self.tile;
        let sel_px = (b.x0 * tw, b.y0 * th, (b.x1 + 1) * tw, (b.y1 + 1) * th);
        let same = |a: &Animation| a.px_rect() == sel_px;
        if self.side.animations.iter().any(same) {
            self.snapshot();
            self.side.animations.retain(|a| !same(a));
            return Ok(());
        }
        if self.gap != 0 || self.offset != [0, 0] {
            return Err("this sheet has gaps between tiles; copy the strip to a tilemap and mark it there".into());
        }
        let Some(d) = self.draft().cloned() else { return Ok(()) };
        if !d.fits([tw, th]) {
            let [w, h] = d.area_px([tw, th]);
            return Err(format!("the block is {w}x{h} px; the frame numbers must divide that"));
        }
        self.snapshot();
        self.side.animations.retain(|a| !a.px_overlaps(sel_px));
        self.side.animations.push(d.animation([tw, th]));
        self.side.animations.sort_by_key(|a| (a.px[1], a.px[0]));
        Ok(())
    }

    /// Changes the frame grid of the animation under the selection. The
    /// block keeps its pixels, so each number must divide its side.
    pub fn set_animation(&mut self, frames: [u32; 2], ms: u32) -> Result<(), String> {
        let Some(view) = self.stored_animation() else { return Ok(()) };
        let Some(a) = self.stored_mut(&view) else { return Ok(()) };
        let [c, r] = a.grid();
        let (w, h) = (a.frame[0] * c, a.frame[1] * r);
        if frames[0] == 0 || frames[1] == 0 || w % frames[0] != 0 || h % frames[1] != 0 {
            return Err(format!("the block is {w}x{h} px; the frame numbers must divide that"));
        }
        self.snapshot();
        let a = self.stored_mut(&view).unwrap();
        a.frame = [w / frames[0], h / frames[1]];
        a.frames = Pair::strip(frames);
        a.ms = ms.max(1);
        Ok(())
    }

    /// Changes the gap or the offset: a view change like the tile size.
    pub fn set_gap_offset(&mut self, ctx: &egui::Context, gap: u32, offset: [u32; 2]) {
        if (gap, offset) == (self.gap, self.offset) {
            return;
        }
        self.gap = gap;
        self.offset = offset;
        self.gap_edit = gap;
        self.side.gap = (gap != 0).then_some(gap);
        self.side.offset = (offset != [0, 0]).then_some(Pair::of(offset));
        self.dirty = true;
        self.sel = Sel::default();
        self.draft = None;
        self.upload(ctx);
    }

    /// Changes the sheet's tile size: a view change, nothing is rewritten.
    /// The cell records keep their grid until the next edit; animations are
    /// pixel strips and never cared. Derived names are cleared until the
    /// indexer renames them at the new grid.
    pub fn set_tile(&mut self, ctx: &egui::Context, new: [u32; 2]) {
        if new == self.tile || new[0] == 0 || new[1] == 0 {
            return;
        }
        self.tile = new;
        self.side.tile = Some(Pair::of(new));
        self.dirty = true;
        self.sel = Sel::default();
        self.draft = None;
        self.upload(ctx);
    }

    /// Sets the canvas size in cells while a canvas edge drag runs. The
    /// pixels come from the copy the drag holds, not from the canvas on
    /// screen. Cells outside the new size are lost, with their origins and
    /// their animations.
    fn set_size(&mut self, ctx: &egui::Context, cols: u32, rows: u32) {
        let (w, h) = self.img_size_for(cols, rows);
        let Some(base) = &self.canvas_resize else { return };
        let mut img = RgbaImage::new(w, h);
        let keep = image::imageops::crop_imm(&base.img, 0, 0, w.min(base.img.width()), h.min(base.img.height()));
        image::imageops::replace(&mut img, &*keep, 0, 0);
        let prov = base.prov.resized(w, h);
        let animations = base
            .side
            .animations
            .iter()
            .filter(|a| {
                let r = a.px_rect();
                r.2 <= w && r.3 <= h
            })
            .cloned()
            .collect();
        self.img = img;
        self.prov = prov;
        self.side.animations = animations;
        self.sel = Sel::default();
        self.upload(ctx);
    }

    /// Cuts empty columns on the right and empty rows at the bottom. At least
    /// one cell stays.
    pub fn trim(&mut self, ctx: &egui::Context) {
        let [tw, th] = self.tile;
        let used = |x: u32, y: u32| {
            let (ix0, iy0, ix1, iy1) = self.cell_img_rect(x, y);
            self.prov.any_in(ix0, iy0, ix1, iy1)
                || (0..th).any(|py| {
                    (0..tw).any(|px| {
                        let (ix, iy) = (ix0 + px, iy0 + py);
                        ix < self.img.width() && iy < self.img.height() && self.img.get_pixel(ix, iy).0[3] > 0
                    })
                })
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
            let r = a.px_rect();
            new_cols = new_cols.max(r.2.div_ceil(self.pitch()[0]));
            new_rows = new_rows.max(r.3.div_ceil(self.pitch()[1]));
        }
        let (new_cols, new_rows) = (new_cols.max(1), new_rows.max(1));
        if new_cols == cols && new_rows == rows {
            return;
        }
        self.snapshot();
        let (w, h) = self.img_size_for(new_cols, new_rows);
        self.img = image::imageops::crop_imm(&self.img, 0, 0, w, h).to_image();
        self.prov = self.prov.resized(self.img.width(), self.img.height());
        self.sel = Sel::default();
        self.upload(ctx);
    }

    /// Writes the image and the book entry. A tilemap's entry always names
    /// its grid, so it is never lost between runs.
    pub fn save(&mut self) -> Result<(), String> {
        self.img.save(self.dir.join(&self.rel)).map_err(|e| e.to_string())?;
        self.side.tile = Some(Pair::of(self.tile));
        self.side.provenance = self.prov.extract();
        self.save_entry()
    }

    /// Writes only the book entry; for sheets whose pixels are not edited.
    /// A stored entry always names its tile size, so the file explains itself.
    pub fn save_entry(&mut self) -> Result<(), String> {
        let mut side = self.side.clone();
        if !side.is_empty() {
            side.tile = Some(Pair::of(self.tile));
        }
        sidecar::store_entry(&self.dir, &self.rel, &side)?;
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


#[cfg(test)]
mod tests {
    use super::*;

    /// A changed tile size must survive save and reopen.
    #[test]
    fn tile_size_persists() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!("tilepick-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut sheet = Sheet::new_empty(&ctx, &dir, "map.png", [32, 32], 4, 4);
        sheet.save().unwrap();
        sheet.set_tile(&ctx, [64, 48]);
        sheet.save().unwrap();
        let book = sidecar::load_book(&dir);
        let entry = book.get("map.png").cloned();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(entry.and_then(|e| e.tile).map(Pair::xy), Some([64, 48]));
    }
}


#[cfg(test)]
mod prov_tests {
    use super::*;

    /// Painting the extracted rectangles again gives the same map.
    #[test]
    fn provenance_round_trips() {
        let mut m = ProvMap::new(96, 64);
        let a = m.intern("packs/a.png");
        let b = m.intern("packs/b.png");
        m.fill(0, 0, 32, 32, a);
        m.fill(32, 0, 64, 64, b);
        m.fill(16, 16, 16, 16, -1); // a hole from a clear
        let side = m.extract();
        let again = ProvMap::from_side(96, 64, &side);
        for y in 0..64 {
            for x in 0..96 {
                assert_eq!(m.get(x, y), again.get(x, y), "at {x},{y}");
            }
        }
        // Grouped by file, and the hole split rects for a.
        assert_eq!(side.len(), 2);
        assert!(side.iter().any(|p| p.source == "packs/b.png" && p.rects == vec![[32, 0, 64, 64]]));
    }
}

/// The frames of a GIF, composited to full canvas, and the first delay in ms.
fn decode_gif(path: &Path) -> (Vec<RgbaImage>, u32) {
    use image::AnimationDecoder;
    let Ok(file) = std::fs::File::open(path) else { return (Vec::new(), 0) };
    let Ok(decoder) = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file)) else {
        return (Vec::new(), 0);
    };
    let Ok(frames) = decoder.into_frames().collect_frames() else { return (Vec::new(), 0) };
    let ms = frames.first().map(|f| f.delay().numer_denom_ms().0).unwrap_or(100);
    (frames.into_iter().map(|f| f.into_buffer()).collect(), ms.max(1))
}
