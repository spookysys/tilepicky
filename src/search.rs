// SPDX-License-Identifier: GPL-3.0-only
//! Text matching and packed island results.

use crate::{index::{self, Entry, Index}, islands, labels, settings::{SearchIn, SearchView},
    sidecar::{Label, Pair, Provenance, Saved, Sidecar, Status, StoredIsland}};
use image::RgbaImage;
use std::{collections::BTreeMap, path::{Path, PathBuf}, time::{Duration, Instant, SystemTime}};

#[derive(Clone)]
pub struct Hit { pub rel: String, pub identity: String, pub grid: crate::Grid, pub island: StoredIsland }
pub struct Packed { pub img: RgbaImage, pub side: Sidecar }
pub struct Output { pub hits: Vec<Hit>, pub visible: Vec<bool>, pub packed: Option<Packed>, pub notices: Vec<String> }

#[derive(Clone)]
struct Cached {
    modified: SystemTime, len: u64, identity: String, grid: crate::Grid, regions: Vec<StoredIsland>,
    tile: Option<Pair<u32>>, gap: Option<Pair<u32>>, offset: Option<Pair<i32>>, fallback: [u32; 2],
}
type Cache = BTreeMap<PathBuf, Cached>;

fn words(label: &Label, options: SearchIn) -> Vec<String> {
    if label.status != Status::Labeled { return vec![]; }
    let mut words = if options.captions { index::query_words(&label.caption) } else { vec![] };
    if options.tags { for tag in &label.tags { words.extend(index::query_words(tag)); } }
    words
}

fn base_words(entry: &Entry, current: bool, options: SearchIn) -> Vec<String> {
    let mut base = Vec::new();
    if options.files { base.extend(entry.name_words.clone()); }
    if options.folders { base.extend(entry.dir_words.clone()); }
    if current && let Some(label) = entry.side.labels.as_ref().and_then(|s| s.sheet.as_ref()) { base.extend(words(label, options)); }
    base
}

fn matches(query: &[String], base: &[String], label: Option<&Label>, options: SearchIn) -> bool {
    let extra = label.map(|l| words(l, options)).unwrap_or_default();
    query.iter().all(|q| base.iter().chain(&extra).any(|w| w.starts_with(q)))
}

fn run(root: &Path, entries: &[Entry], fallback: [u32; 2], query: &[String], options: SearchIn, cache: &mut Cache) -> Output {
    let mut out = Output { hits: vec![], visible: vec![false; entries.len()], packed: None, notices: vec![] };
    for (i, entry) in entries.iter().enumerate() {
        let possible = base_words(entry, true, options);
        if !matches(query, &possible, None, options) && !entry.side.labels.as_ref().is_some_and(|saved|
            saved.islands.iter().any(|island| matches(query, &possible, island.label.as_ref(), options))) { continue; }
        let path = root.join(&entry.rel);
        let result = (|| {
            let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
            let modified = meta.modified().map_err(|e| e.to_string())?;
            let cached = cache.get(&path).filter(|c| c.modified == modified && c.len == meta.len()
                && c.tile == entry.side.tile && c.gap == entry.side.gap && c.offset == entry.side.offset && c.fallback == fallback);
            let data = if let Some(cached) = cached { cached.clone() } else {
                let img = image::open(&path).map_err(|e| e.to_string())?.to_rgba8();
                let grid = islands::grid(&img, &entry.side, fallback);
                let regions = islands::regions(&img, grid);
                let data = Cached { modified, len: meta.len(), identity: labels::identity(&img), grid, regions,
                    tile: entry.side.tile, gap: entry.side.gap, offset: entry.side.offset, fallback };
                cache.insert(path, data.clone());
                data
            };
            let current = entry.side.labels.as_ref().is_some_and(|s| s.current(&data.identity));
            let base = base_words(entry, current, options);
            let regions = if current { &entry.side.labels.as_ref().unwrap().islands } else { &data.regions };
            out.visible[i] = matches(query, &base, None, options);
            for island in regions {
                if island.rects.is_empty() { continue; }
                if matches(query, &base, current.then_some(island.label.as_ref()).flatten(), options) {
                    out.visible[i] = true;
                    out.hits.push(Hit { rel: entry.rel.clone(), identity: data.identity.clone(), grid: data.grid, island: island.clone() });
                }
            }
            Ok::<_, String>(())
        })();
        if let Err(e) = result { out.notices.push(format!("{}: {e}", entry.rel)); }
    }
    if options.view == SearchView::Virtual && !out.hits.is_empty() {
        match pack(root, &out.hits) { Ok(packed) => out.packed = Some(packed), Err(e) => out.notices.push(e) }
    }
    out
}

fn gcd(mut a: u32, mut b: u32) -> u32 { while b != 0 { (a, b) = (b, a % b); } a.max(1) }

/// A deterministic shelf layout keeps original pixels and source regions intact.
pub fn pack(root: &Path, hits: &[Hit]) -> Result<Packed, String> {
    if hits.is_empty() { return Err("No matching islands.".into()); }
    if hits.iter().any(|h| h.island.bounds()[2..].iter().any(|&size| size == 0 || size > 16_000_000)) {
        return Err("Invalid island dimensions. Detect islands again.".into());
    }
    let tile = hits.iter().fold(hits[0].grid.0, |t, h| [gcd(t[0], h.grid.0[0]), gcd(t[1], h.grid.0[1])]);
    let sizes: Vec<_> = hits.iter().map(|h| {
        let [_, _, w, h] = h.island.bounds();
        [w.div_ceil(tile[0]) * tile[0], h.div_ceil(tile[1]) * tile[1]]
    }).collect();
    let area: u64 = sizes.iter().map(|s| (s[0] as u64 + tile[0] as u64) * (s[1] as u64 + tile[1] as u64)).sum();
    if area > 16_000_000 { return Err("The virtual sheet is too large. Narrow the search.".into()); }
    let width = ((area as f64).sqrt().ceil() as u32).max(sizes.iter().map(|s| s[0]).max().unwrap()).div_ceil(tile[0]) * tile[0];
    let mut order: Vec<_> = (0..hits.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(sizes[i][1]), i));
    let mut places = vec![[0, 0]; hits.len()];
    let (mut x, mut y, mut row_height) = (0, 0, 0);
    for i in order {
        let [w, h] = sizes[i];
        if x > 0 && x + w > width { x = 0; y += row_height + tile[1]; row_height = 0; }
        places[i] = [x, y];
        x += w + tile[0];
        row_height = row_height.max(h);
    }
    let height = y + row_height;
    if width as u64 * height as u64 > 16_000_000 { return Err("The virtual sheet is too large. Narrow the search.".into()); }
    let mut img = RgbaImage::new(width, height);
    let mut regions = Vec::new();
    let mut provenance = Vec::new();
    let mut source: Option<(String, RgbaImage)> = None;
    for (i, hit) in hits.iter().enumerate() {
        if source.as_ref().is_none_or(|(rel, _)| *rel != hit.rel) {
            let image = image::open(root.join(&hit.rel)).map_err(|e| e.to_string())?.to_rgba8();
            if labels::identity(&image) != hit.identity { return Err("A source image changed during search. Run the search again.".into()); }
            source = Some((hit.rel.clone(), image));
        }
        let source = &source.as_ref().unwrap().1;
        if hit.island.rects.iter().any(|&[x, y, w, h]| x.saturating_add(w) > source.width() || y.saturating_add(h) > source.height()) {
            return Err("Saved island regions extend outside their image. Label the sheet again.".into());
        }
        let [left, top, _, _] = hit.island.bounds();
        let [x, y] = places[i];
        let crop = labels::crop(source, &hit.island);
        image::imageops::replace(&mut img, &crop, x as i64, y as i64);
        let rects: Vec<_> = hit.island.rects.iter().map(|&[sx, sy, w, h]| [x + sx - left, y + sy - top, w, h]).collect();
        regions.push(StoredIsland { rects: rects.clone(), label: hit.island.label.clone() });
        provenance.push(Provenance { source: hit.rel.clone(), rects });
    }
    let side = Sidecar { tile: Some(Pair::of(tile)), provenance, labels: Some(Saved {
        provider: String::new(), model: String::new(), identity: labels::identity(&img),
        sheet: None, islands: regions,
    }), ..Sidecar::default() };
    Ok(Packed { img, side })
}

#[derive(Clone, PartialEq)]
struct Query { root: PathBuf, text: String, options: SearchIn, revision: u64 }
struct Finished { query: Query, output: Output, cache: Cache }
#[derive(Default)]
pub struct Engine {
    pub output: Option<Output>,
    requested: Option<Query>,
    task: Option<std::sync::mpsc::Receiver<Finished>>,
    cache: Cache,
    revision: u64,
    due: Option<Instant>,
}
impl Engine {
    pub fn invalidate(&mut self) { self.revision += 1; self.output = None; self.due = Some(Instant::now() + Duration::from_millis(180)); }
    pub fn tick(&mut self, ctx: &eframe::egui::Context, index: &Index, text: &str, options: SearchIn) -> bool {
        let wanted = Query { root: index.root.clone(), text: text.into(), options, revision: self.revision };
        let mut updated = false;
        if let Some(rx) = &self.task {
            match rx.try_recv() {
                Ok(finished) => {
                    self.cache = finished.cache;
                    self.task = None;
                    if finished.query == wanted { self.output = Some(finished.output); updated = true; }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.task = None;
                    self.output = Some(Output { hits: vec![], visible: vec![false; index.entries.len()], packed: None,
                        notices: vec!["Search failed. Change the query to retry.".into()] });
                    updated = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {},
            }
        }
        if self.task.is_none() && self.requested.as_ref() != Some(&wanted) {
            if self.due.is_some_and(|due| due > Instant::now()) {
                ctx.request_repaint_after(Duration::from_millis(180)); return updated;
            }
            self.requested = Some(wanted.clone());
            if text.trim().is_empty() || index.root.as_os_str().is_empty() { self.output = None; return true; }
            let entries = index.entries.clone();
            let fallback = index.tile;
            let mut cache = std::mem::take(&mut self.cache);
            cache.retain(|path, _| path.starts_with(&wanted.root));
            let (tx, rx) = std::sync::mpsc::channel();
            let wake = ctx.clone();
            self.task = Some(rx);
            std::thread::spawn(move || {
                let output = run(&wanted.root, &entries, fallback, &index::query_words(&wanted.text), wanted.options, &mut cache);
                let _ = tx.send(Finished { query: wanted, output, cache });
                wake.request_repaint();
            });
        }
        updated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    struct Library(PathBuf);
    impl Library {
        fn new() -> Self {
            let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("tilepicky-search-{}-{stamp}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Library { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    fn label(caption: &str, tags: &[&str]) -> Label {
        Label { status: Status::Labeled, caption: caption.into(), tags: tags.iter().map(|s| (*s).into()).collect() }
    }
    fn fixture() -> (Library, Vec<Entry>) {
        let root = Library::new();
        let mut img = RgbaImage::from_pixel(8, 4, Rgba([255, 0, 0, 255]));
        for y in 0..4 { for x in 4..8 { img.put_pixel(x, y, Rgba([0, 0, 255, 255])); } }
        img.save(root.0.join("village.png")).unwrap();
        let saved = Saved { provider: "test".into(), model: "test".into(), identity: labels::identity(&img),
            sheet: Some(label("Medieval village", &["pixel art"])), islands: vec![
                StoredIsland { rects: vec![[0, 0, 4, 4]], label: Some(label("Red house", &["building"])) },
                StoredIsland { rects: vec![[4, 0, 4, 4]], label: Some(label("Blue tree", &["plant"])) },
            ] };
        let entry = Entry { rel: "village.png".into(), dir_words: vec![], name_words: vec!["village".into()],
            side: Sidecar { tile: Some(Pair::of([4, 4])), labels: Some(saved), ..Sidecar::default() } };
        (root, vec![entry])
    }
    #[test]
    fn combines_sheet_context_and_island_words_and_respects_options() {
        let (root, entries) = fixture();
        let mut cache = Cache::new();
        let options = SearchIn::default();
        let out = run(&root.0, &entries, [4, 4], &index::query_words("mediev build"), options, &mut cache);
        assert_eq!(out.visible, [true]);
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].island.label.as_ref().unwrap().caption, "Red house");
        let out = run(&root.0, &entries, [4, 4], &index::query_words("house tree"), options, &mut cache);
        assert!(out.hits.is_empty());
        let out = run(&root.0, &entries, [4, 4], &index::query_words("build"), SearchIn { tags: false, ..options }, &mut cache);
        assert_eq!(out.visible, [false]);
    }
    #[test]
    fn stale_labels_do_not_match_but_names_still_do() {
        let (root, mut entries) = fixture();
        entries[0].side.labels.as_mut().unwrap().identity = "old pixels".into();
        let mut cache = Cache::new();
        let out = run(&root.0, &entries, [4, 4], &index::query_words("house"), SearchIn::default(), &mut cache);
        assert_eq!(out.visible, [false]);
        let out = run(&root.0, &entries, [4, 4], &index::query_words("village"), SearchIn::default(), &mut cache);
        assert_eq!(out.visible, [true]);
        assert!(out.hits.iter().all(|h| h.island.label.is_none()));
    }
    #[test]
    fn packing_preserves_pixels_labels_holes_and_source_names() {
        let (root, mut entries) = fixture();
        entries[0].side.labels.as_mut().unwrap().islands[0].rects = vec![[0, 0, 2, 4], [3, 0, 1, 4]];
        let out = run(&root.0, &entries, [4, 4], &index::query_words("village"), SearchIn::default(), &mut Cache::new());
        let a = pack(&root.0, &out.hits).unwrap();
        let b = pack(&root.0, &out.hits).unwrap();
        assert_eq!(a.img, b.img);
        assert_eq!(a.side.labels, b.side.labels);
        let islands = &a.side.labels.as_ref().unwrap().islands;
        assert_eq!(islands.len(), 2);
        for (i, island) in islands.iter().enumerate() {
            assert_eq!(island.label, out.hits[i].island.label);
            assert_eq!(a.side.provenance[i].source, "village.png");
            let [x, y, _, _] = island.bounds();
            assert_eq!(a.img.get_pixel(x, y).0, if i == 0 { [255, 0, 0, 255] } else { [0, 0, 255, 255] });
        }
        let [x, y, _, _] = islands[0].bounds();
        assert_eq!(a.img.get_pixel(x + 2, y).0[3], 0);
        let ctx = eframe::egui::Context::default();
        let mut sheet = crate::sheet::Sheet::from_search(&ctx, &root.0, "village", a);
        assert!(sheet.virtual_sheet);
        let block = sheet.copy_sel(&crate::sheet::Sel::rect((0, 0), (sheet.cols() - 1, sheet.rows() - 1))).unwrap();
        assert!(block.prov.extract().iter().all(|p| p.source == "village.png"));
        assert!(sheet.label_input().is_err());
        assert!(sheet.save().is_err());
        sheet.save_entry().unwrap();
        assert!(!root.0.join(crate::sidecar::BOOK).exists());
    }
    #[test]
    fn grid_changes_keep_labels_and_old_settings_load() {
        let (root, mut entries) = fixture();
        let mut cache = Cache::new();
        let options: SearchIn = serde_json::from_str(r#"{"folders":true,"files":true}"#).unwrap();
        assert!(options.captions && options.tags);
        assert_eq!(options.view, SearchView::Files);
        let a = run(&root.0, &entries, [4, 4], &index::query_words("house"), options, &mut cache);
        entries[0].side.tile = Some(Pair::of([2, 2]));
        let b = run(&root.0, &entries, [4, 4], &index::query_words("house"), options, &mut cache);
        assert_eq!(a.hits[0].island, b.hits[0].island);
        assert_eq!(b.hits[0].grid.0, [2, 2]);
    }
}
