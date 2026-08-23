//! Scans a directory of sheets, turns paths into search words, and names the
//! cells of each atlas by finding its individual sprites inside it.

use crate::sidecar::{self, Sidecar, TILE};
use image::RgbaImage;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Derived names of cells: `"x,y"` to words. Machine output, kept apart
/// from the user's book.
pub type Names = BTreeMap<String, Vec<String>>;

pub struct Entry {
    pub rel: String,
    pub words: Vec<String>,
    /// The book entry: the user's tags, origins, and animations.
    pub side: Sidecar,
    pub names: Names,
}

pub struct Index {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
}

impl Index {
    /// Lists every PNG and GIF under `root`, sorted by path.
    pub fn scan(root: &Path) -> Self {
        let mut rels: Vec<String> = walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter(|e| {
                let ext = e.path().extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
                ext == "png" || ext == "gif"
            })
            .filter_map(|e| e.path().strip_prefix(root).ok().map(|p| p.to_string_lossy().into_owned()))
            .collect();
        rels.sort();
        let mut book = sidecar::load_book(root);
        let entries = rels
            .into_iter()
            .map(|rel| {
                let side = book.remove(&rel).unwrap_or_default();
                let mut words = path_words(&rel);
                words.extend(side.tags.iter().flat_map(|t| words_of(t)));
                Entry { words, side, names: Names::new(), rel }
            })
            .collect();
        Self { root: root.to_path_buf(), entries }
    }

    pub fn position(&self, rel: &str) -> Option<usize> {
        self.entries.binary_search_by(|e| e.rel.as_str().cmp(rel)).ok()
    }

    /// True when every query word is the prefix of a word that describes the file.
    pub fn entry_matches(e: &Entry, query: &[String]) -> bool {
        matches(query, |q| {
            e.words.iter().any(|w| w.starts_with(q))
                || e.names.values().any(|ws| ws.iter().any(|w| w.starts_with(q)))
                || e.side.cells.values().any(|c| {
                    c.tags.iter().any(|t| t.starts_with(q))
                        || c.src.as_deref().is_some_and(|s| path_words(s).iter().any(|w| w.starts_with(q)))
                })
        })
    }

    pub fn visible(&self, query: &[String]) -> Option<Vec<bool>> {
        if query.is_empty() {
            return None;
        }
        Some(self.entries.iter().map(|e| Self::entry_matches(e, query)).collect())
    }
}

/// Lower-case letter runs, at least two letters long, without duplicates.
pub fn words(s: &str) -> Vec<String> {
    words_of(s)
}

fn words_of(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in s.chars().chain(std::iter::once(' ')) {
        if ch.is_alphabetic() {
            cur.extend(ch.to_lowercase());
        } else if !cur.is_empty() {
            if cur.chars().count() >= 2 && !out.contains(&cur) {
                out.push(cur.clone());
            }
            cur.clear();
        }
    }
    out
}

/// Words of every path component; the file extension is dropped.
pub fn path_words(rel: &str) -> Vec<String> {
    let stem = Path::new(rel).with_extension("");
    let mut out = Vec::new();
    for w in words(&stem.to_string_lossy()) {
        if !out.contains(&w) {
            out.push(w);
        }
    }
    out
}

pub fn query_words(q: &str) -> Vec<String> {
    words(q)
}

pub fn matches(query: &[String], has: impl Fn(&str) -> bool) -> bool {
    query.iter().all(|q| has(q))
}

// ---------------------------------------------------------------------------
// Naming the cells of atlases.

pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
}

/// What the background pass returns: derived cell names per sheet.
pub type Derived = BTreeMap<String, Names>;

#[derive(Serialize, Deserialize, Default)]
struct Cache {
    files: BTreeMap<String, CacheEntry>,
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    stamp: u64,
    sprites: u64,
    names: Names,
}

struct Dims {
    rel: String,
    w: u32,
    h: u32,
    stamp: u64,
}

/// Matches every small PNG against the larger sheets in its own folder, the
/// parent folder, and the grandparent folder. Cached between runs.
pub fn derive(root: PathBuf, rels: Vec<String>, cache_path: PathBuf, progress: Arc<Progress>) -> Derived {
    let dims: Vec<Dims> = rels
        .iter()
        .filter(|r| r.to_ascii_lowercase().ends_with(".png"))
        .filter_map(|rel| {
            let path = root.join(rel);
            let (w, h) = png_dims(&path)?;
            Some(Dims { rel: rel.clone(), w, h, stamp: stamp(&path) })
        })
        .collect();

    let is_sprite = |d: &Dims| d.w <= 512 && d.h <= 512 && d.w >= 8 && d.h >= 8;
    let is_sheet = |d: &Dims| d.w >= 128 && d.h >= 128 && d.w * d.h >= 256 * 256;

    let mut by_dir: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for (i, d) in dims.iter().enumerate() {
        if is_sheet(d) {
            by_dir.entry(parent(&d.rel)).or_default().push(i);
        }
    }

    let mut work: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (si, s) in dims.iter().enumerate() {
        if !is_sprite(s) {
            continue;
        }
        let mut dir = parent(&s.rel);
        for _ in 0..3 {
            if let Some(sheets) = by_dir.get(&dir) {
                for &sh in sheets {
                    let t = &dims[sh];
                    if sh != si && t.w * t.h >= 2 * s.w * s.h {
                        work.entry(sh).or_default().push(si);
                    }
                }
            }
            match dir.parent() {
                Some(p) => dir = p.to_path_buf(),
                None => break,
            }
        }
    }

    let cache: Cache = std::fs::read_to_string(&cache_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let fingerprint = |sprites: &[usize]| {
        let mut h = DefaultHasher::new();
        for &i in sprites {
            dims[i].rel.hash(&mut h);
            dims[i].stamp.hash(&mut h);
        }
        h.finish()
    };

    let mut fresh: BTreeMap<String, CacheEntry> = BTreeMap::new();
    let mut todo: Vec<(usize, Vec<usize>, u64)> = Vec::new();
    for (sh, sprites) in work {
        let fp = fingerprint(&sprites);
        let d = &dims[sh];
        match cache.files.get(&d.rel) {
            Some(c) if c.stamp == d.stamp && c.sprites == fp => {
                fresh.insert(d.rel.clone(), CacheEntry { stamp: c.stamp, sprites: c.sprites, names: c.names.clone() });
            }
            _ => todo.push((sh, sprites, fp)),
        }
    }
    progress.total.store(todo.len(), Ordering::Relaxed);

    let computed: Vec<(String, CacheEntry)> = todo
        .par_iter()
        .filter_map(|(sh, sprites, fp)| {
            let d = &dims[*sh];
            let names = name_cells(&root, &d.rel, sprites.iter().map(|&i| dims[i].rel.as_str()));
            progress.done.fetch_add(1, Ordering::Relaxed);
            Some((d.rel.clone(), CacheEntry { stamp: d.stamp, sprites: *fp, names }))
        })
        .collect();
    fresh.extend(computed);

    if let Some(dir) = cache_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let out = Cache { files: fresh };
    if let Ok(s) = serde_json::to_string(&out) {
        let _ = std::fs::write(&cache_path, s);
    }

    out.files.into_iter().filter(|(_, v)| !v.names.is_empty()).map(|(k, v)| (k, v.names)).collect()
}

/// Finds each sprite in the sheet and tags the cells it covers with the words
/// of the sprite's file name.
fn name_cells<'a>(root: &Path, sheet_rel: &str, sprites: impl Iterator<Item = &'a str>) -> Names {
    let mut names = Names::new();
    let Ok(sheet) = image::open(root.join(sheet_rel)).map(|i| i.to_rgba8()) else {
        return names;
    };
    for rel in sprites {
        let Ok(sprite) = image::open(root.join(rel)).map(|i| i.to_rgba8()) else {
            continue;
        };
        let ws = words(Path::new(rel).file_stem().map(|s| s.to_string_lossy()).as_deref().unwrap_or(""));
        for (ox, oy) in find(&sheet, &sprite) {
            let (w, h) = sprite.dimensions();
            for cy in oy / TILE..=(oy + h - 1) / TILE {
                for cx in ox / TILE..=(ox + w - 1) / TILE {
                    let cell = names.entry(Sidecar::key(cx, cy)).or_default();
                    for w in &ws {
                        if !cell.contains(w) {
                            cell.push(w.clone());
                        }
                    }
                }
            }
        }
    }
    names
}

/// Grid-aligned positions where every opaque pixel of `sprite` equals `sheet`.
fn find(sheet: &RgbaImage, sprite: &RgbaImage) -> Vec<(u32, u32)> {
    let (w, h) = sprite.dimensions();
    let (sw, sh) = sheet.dimensions();
    if w > sw || h > sh {
        return Vec::new();
    }
    let opaque: Vec<(u32, u32, [u8; 4])> =
        sprite.enumerate_pixels().filter(|(_, _, p)| p.0[3] > 0).map(|(x, y, p)| (x, y, p.0)).collect();
    // A sprite with almost no pixels would match by chance.
    if opaque.len() < 16 {
        return Vec::new();
    }
    let (fx, fy, fv) = opaque[0];
    let mut hits = Vec::new();
    let mut oy = 0;
    while oy + h <= sh {
        let mut ox = 0;
        while ox + w <= sw {
            if sheet.get_pixel(ox + fx, oy + fy).0 == fv
                && opaque.iter().all(|&(x, y, v)| sheet.get_pixel(ox + x, oy + y).0 == v)
            {
                hits.push((ox, oy));
            }
            ox += TILE;
        }
        oy += TILE;
    }
    hits
}

fn parent(rel: &str) -> PathBuf {
    Path::new(rel).parent().map(Path::to_path_buf).unwrap_or_default()
}

fn stamp(path: &Path) -> u64 {
    let Ok(m) = std::fs::metadata(path) else { return 0 };
    let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    t ^ (m.len() << 20)
}

/// Width and height from the PNG header, without decoding the image.
fn png_dims(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut head = [0u8; 24];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(head[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(head[20..24].try_into().ok()?);
    Some((w, h))
}

pub fn cache_path(root: &Path) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."));
    let canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut h = DefaultHasher::new();
    canon.hash(&mut h);
    base.join("tilepick").join(format!("{:016x}.json", h.finish()))
}
