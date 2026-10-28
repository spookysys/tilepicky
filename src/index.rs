// SPDX-License-Identifier: GPL-3.0-only
//! Scans a directory of sheets and searches their names, captions, and tags.

use crate::embed;
use crate::settings::{SearchIn, View};
use crate::sidecar::{self, Pair, QuantizeGroup, QuantizeTarget, Sidecar};
use crate::tiled;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The formats the tool reads. It always writes 32 bit RGBA PNG.
pub const IMAGE_EXTS: [&str; 7] = ["png", "gif", "jpg", "jpeg", "webp", "bmp", "tga"];

/// What a card stands for. An image that a tileset describes can wear either
/// face; the view filters decide which one shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    Image,
    Tileset,
}

pub struct Entry {
    pub rel: String,
    /// The words of the folders on the path, and of the file name, lower case.
    pub dir_words: Vec<String>,
    pub name_words: Vec<String>,
    /// The book entry: grid, origins, animations, AI label.
    pub side: Sidecar,
    /// The sheet's embedding, when the library has one; see `embed`.
    pub embed: Option<Vec<f32>>,
    pub text_embed: Option<Vec<f32>>,
    /// The Tiled tileset that describes this image, when one does. Only an
    /// image tileset fills this: a collection has no image to stand on.
    pub tiled: Option<tiled::Tiled>,
    /// Some tileset names this image, as an atlas or as a collection member.
    pub in_tileset: bool,
    /// The tileset that names this image as one tile of a collection. An
    /// atlas entry carries that tileset in `tiled` instead.
    pub tileset_of: Option<String>,
    /// This entry stands for a tileset file, and has no pixels. Every path
    /// that decodes an image must ask `sheet_rel` first.
    pub tileset_only: bool,
    /// Why the tileset of this entry could not be read.
    pub tiled_error: Option<String>,
    /// A warning about the tileset: a path whose case differs, an image that
    /// is not there, or a second tileset for one image.
    pub tiled_note: Option<String>,
}

impl Entry {
    /// The image file, when this entry has pixels at all.
    pub fn sheet_rel(&self) -> Option<&str> {
        (!self.tileset_only).then_some(&self.rel)
    }

    /// Whether this entry carries a Tiled tileset, either its own or one that
    /// stands on its image.
    pub fn has_tileset(&self) -> bool {
        self.tileset_only || self.tiled.is_some()
    }

    /// The tileset file that describes this entry: the atlas that names the
    /// image, or the collection that holds it as one tile.
    pub fn tileset_file(&self) -> Option<&str> {
        self.tiled.as_ref().map(|t| t.file.as_str()).or(self.tileset_of.as_deref())
    }

    /// Whether the view filters leave this entry in the library.
    pub fn shows(&self, view: View) -> bool {
        if self.tileset_only {
            view.tilesets
        } else if self.tiled.is_some() {
            // The same file has two faces here: the tileset that describes it
            // and the image itself. Either box shows it, and it shows once.
            view.tilesets || view.in_tilesets
        } else if self.in_tileset {
            view.in_tilesets
        } else {
            view.loose
        }
    }

    /// Which face the card wears. A tileset card wins when both boxes are on,
    /// because two rows for one file in one folder reads as two files.
    pub fn card(&self, view: View) -> Card {
        if self.tileset_only || (self.tiled.is_some() && view.tilesets) { Card::Tileset } else { Card::Image }
    }
}

pub struct Index {
    /// The folder this side reads. Empty when none is chosen yet.
    pub root: PathBuf,
    pub error: Option<String>,
    pub entries: Vec<Entry>,
    /// Every directory under the root, so that empty folders show too.
    pub dirs: Vec<String>,
    /// The run's default tile size, for entries that name none.
    pub tile: [u32; 2],
    /// The tags a labeling request asks for; see `sidecar::Book::tag_list`.
    pub tag_list: Vec<String>,
    /// The most free tags a labeling request asks for; see `sidecar::Book::free_tags`.
    pub free_tags: usize,
    /// The model the library's embeddings came from. Empty when there are none.
    pub embed_model: String,
    pub embed_provider: String,
    pub embed_labels: bool,
    pub embed_error: Option<String>,
    pub text_embed_model: String,
    pub text_embed_provider: String,
    pub text_embed_error: Option<String>,
    /// The color quantization groups of this project.
    pub quantize: Vec<QuantizeGroup>,
}

impl Index {
    /// Lists every image under `root` that the tool reads, sorted by path.
    /// This is the project: a tileset beside a sheet belongs to that sheet,
    /// and the tree shows the sheets alone.
    pub fn scan(root: &Path, default_tile: [u32; 2]) -> Self {
        Self::scan_folder(root, default_tile, false)
    }

    /// Lists the images of a pack, and the Tiled tilesets that describe them.
    /// `docs/tiled-library.md` says how a tileset pairs with its image.
    pub fn scan_library(root: &Path, default_tile: [u32; 2]) -> Self {
        Self::scan_folder(root, default_tile, true)
    }

    fn scan_folder(root: &Path, default_tile: [u32; 2], read_tilesets: bool) -> Self {
        // No folder chosen for this side yet: nothing to list.
        if root.as_os_str().is_empty() {
            return Self {
                root: PathBuf::new(),
                error: None,
                entries: Vec::new(),
                dirs: Vec::new(),
                tile: default_tile,
                tag_list: Vec::new(),
                free_tags: sidecar::FREE_TAGS,
                embed_model: String::new(),
                embed_provider: String::new(),
                embed_labels: false,
                embed_error: None,
                text_embed_model: String::new(),
                text_embed_provider: String::new(),
                text_embed_error: None,
                quantize: Vec::new(),
            };
        }
        let mut rels: Vec<String> = Vec::new();
        let mut tiled_files: Vec<String> = Vec::new();
        let mut dirs: Vec<String> = Vec::new();
        for e in walkdir::WalkDir::new(root).into_iter().filter_map(Result::ok) {
            let Some(rel) = e.path().strip_prefix(root).ok().map(relative_path) else {
                continue;
            };
            if rel.is_empty() {
                continue;
            }
            if e.file_type().is_dir() {
                dirs.push(rel);
            } else if e.file_type().is_file() {
                let ext = e.path().extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
                if IMAGE_EXTS.contains(&ext.as_str()) {
                    rels.push(rel);
                } else if read_tilesets && (ext == "tsx" || ext == "tsj") {
                    tiled_files.push(rel);
                }
            }
        }
        rels.sort();
        tiled_files.sort();
        dirs.sort();
        let (mut book, error) = match sidecar::load_book(root) {
            Ok(book) => (book, None), Err(e) => (sidecar::Book::default(), Some(e)),
        };
        let (embeddings, embed_error) = match embed::read(root) {
            Ok(data) => (data, None), Err(error) => (Default::default(), Some(error)),
        };
        let (text_embeddings, text_embed_error) = match embed::read_mode(root, embed::InputMode::Text) {
            Ok(data) => (data, None), Err(error) => (Default::default(), Some(error)),
        };
        let mut entries: Vec<Entry> = rels
            .into_iter()
            .map(|rel| {
                let side = book.sheets.remove(&rel).unwrap_or_default();
                let (dirs, name) = rel.rsplit_once('/').unwrap_or(("", &rel));
                let (dir_words, name_words) = (words(dirs), path_words(name));
                let vector = embed::current_vector(root, &rel, &side, &embeddings, embed::InputMode::Image);
                let text_embed = embed::current_vector(root, &rel, &side, &text_embeddings, embed::InputMode::Text);
                Entry {
                    dir_words, name_words, side, rel, embed: vector, text_embed,
                    tiled: None, in_tileset: false, tileset_of: None, tileset_only: false, tiled_error: None, tiled_note: None,
                }
            })
            .collect();
        // Pair every tileset with the image it names. The image stays the
        // key of the pair, because the book, the labels, and every transfer
        // address it by that path. A tileset without an image of its own
        // stands alone.
        let at: HashMap<String, usize> = entries.iter().enumerate().map(|(i, e)| (e.rel.clone(), i)).collect();
        let mut extra = Vec::new();
        for file in tiled_files {
            match tiled::read(root, &file) {
                Err(error) => extra.push(standing(&file, None, Some(error), None)),
                Ok(t) => match t.image() {
                    Some(image) => {
                        let resolved = tiled::resolve(root, &file, image);
                        let found = resolved.rel.as_deref().and_then(|rel| at.get(rel).copied());
                        match found {
                            Some(i) if entries[i].tiled.is_none() => {
                                entries[i].tiled = Some(t);
                                entries[i].in_tileset = true;
                                entries[i].tiled_note = resolved.problem;
                            }
                            // A second tileset for one image keeps its own
                            // card, so that neither file is invisible.
                            other => {
                                let note = resolved.problem
                                    .or_else(|| other.map(|_| "Another tileset stands for this image.".to_string()));
                                extra.push(standing(&file, Some(t), None, note));
                            }
                        }
                    }
                    None => {
                        if let tiled::Shape::Collection { images } = &t.shape {
                            for source in images {
                                if let Some(rel) = tiled::resolve(root, &file, source).rel
                                    && let Some(&i) = at.get(&rel) {
                                    entries[i].in_tileset = true;
                                    // The first collection keeps the link when
                                    // two of them hold the same image.
                                    if entries[i].tileset_of.is_none() {
                                        entries[i].tileset_of = Some(file.clone());
                                    }
                                }
                            }
                        }
                        extra.push(standing(&file, Some(t), None, None));
                    }
                },
            }
        }
        entries.extend(extra);
        entries.sort_by(|a, b| a.rel.cmp(&b.rel));
        let tile = book.tile.map(Pair::xy).unwrap_or(default_tile);
        let tag_list = sidecar::tag_list(&book);
        let free_tags = sidecar::free_tags(&book);
        let quantize = book.quantize;
        Self {
            root: root.to_path_buf(),
            error,
            entries,
            dirs,
            tile,
            tag_list,
            free_tags,
            embed_model: embeddings.model,
            embed_provider: embeddings.provider,
            embed_labels: embeddings.labels,
            embed_error,
            text_embed_model: text_embeddings.model,
            text_embed_provider: text_embeddings.provider,
            text_embed_error,
            quantize,
        }
    }

    pub fn position(&self, rel: &str) -> Option<usize> {
        self.entries.binary_search_by(|e| e.rel.as_str().cmp(rel)).ok()
    }

    /// The sheets a quantization target covers, in tree order. A folder
    /// covers every sheet below it, the root folder included.
    pub fn quantize_members(&self, target: &QuantizeTarget) -> Vec<String> {
        match target {
            QuantizeTarget::Files(rels) => {
                let mut out: Vec<String> = rels.iter()
                    .filter(|r| self.position(r).is_some_and(|i| !self.entries[i].tileset_only))
                    .cloned().collect();
                out.sort();
                out.dedup();
                out
            }
            QuantizeTarget::Dir(dir) => self
                .entries
                .iter()
                .filter(|e| !e.tileset_only && (dir.is_empty() || e.rel.starts_with(&format!("{dir}/"))))
                .map(|e| e.rel.clone())
                .collect(),
        }
    }

    /// A short name for a target, for the panel: the file, `N files`, or
    /// `dir/*`.
    pub fn target_name(&self, target: &QuantizeTarget) -> String {
        match target {
            QuantizeTarget::Files(rels) if rels.len() == 1 => rels[0].clone(),
            QuantizeTarget::Files(rels) => format!("{} files", rels.len()),
            QuantizeTarget::Dir(dir) if dir.is_empty() => "*".to_string(),
            QuantizeTarget::Dir(dir) => format!("{dir}/*"),
        }
    }

    /// True when every query word is the prefix of a word in the fields that
    /// `search` names, or when the sheet's embedding is close to `embed`.
    /// The embedding is a sibling of the text fields: either one can match.
    pub fn entry_matches(e: &Entry, query: &[String], search: SearchIn, embed: Option<&[f32]>, text_query: Option<&[f32]>) -> bool {
        Self::match_scores(e, query, search, embed, text_query).iter().any(Option::is_some)
    }

    pub fn visible(
        &self, query: &[String], search: SearchIn, embed: Option<&[f32]>, text_query: Option<&[f32]>, view: View,
    ) -> Option<Vec<bool>> {
        if query.is_empty() && view.all() {
            return None;
        }
        Some(self.entries.iter().map(|e| {
            e.shows(view) && (query.is_empty() || Self::entry_matches(e, query, search, embed, text_query))
        }).collect())
    }

    /// How many cards each view box would show. An atlas image counts in
    /// both the first and the second, because it has both faces.
    pub fn view_counts(&self) -> (usize, usize, usize) {
        let mut counts = (0, 0, 0);
        for e in &self.entries {
            if e.has_tileset() {
                counts.0 += 1;
            }
            if !e.tileset_only && (e.tiled.is_some() || e.in_tileset) {
                counts.1 += 1;
            }
            if !e.has_tileset() && !e.in_tileset {
                counts.2 += 1;
            }
        }
        counts
    }

    /// The view this folder can offer. A folder without a tileset has no
    /// tileset bucket to hide, so its three boxes do not apply. That keeps a
    /// filter set for one pack from emptying the next one.
    pub fn effective_view(&self, view: View) -> View {
        if self.entries.iter().any(Entry::has_tileset) { view } else { View::everything() }
    }

    /// Orders matching sheets by reciprocal rank fusion across the three search sources.
    /// Equal source scores share a rank. Paths settle ties without changing source relevance.
    pub fn ranked(&self, query: &[String], search: SearchIn, image_query: Option<&[f32]>, text_query: Option<&[f32]>) -> Vec<usize> {
        if query.is_empty() {
            return (0..self.entries.len()).collect();
        }
        let mut sources: [Vec<(usize, f32)>; 3] = std::array::from_fn(|_| Vec::new());
        for (i, entry) in self.entries.iter().enumerate() {
            for (source, score) in sources.iter_mut().zip(Self::match_scores(entry, query, search, image_query, text_query)) {
                if let Some(score) = score { source.push((i, score)); }
            }
        }
        let mut fused = vec![0.0_f64; self.entries.len()];
        for source in &mut sources {
            source.sort_by(|a, b| b.1.total_cmp(&a.1));
            let mut rank = 1;
            for (position, &(index, score)) in source.iter().enumerate() {
                if position > 0 && score != source[position - 1].1 { rank = position + 1; }
                // The offset limits the influence of a single source's first result.
                fused[index] += 1.0 / (60.0 + rank as f64);
            }
        }
        let mut results: Vec<usize> = (0..self.entries.len()).filter(|&i| fused[i] > 0.0).collect();
        results.sort_by(|&a, &b| fused[b].total_cmp(&fused[a]).then_with(|| self.entries[a].rel.cmp(&self.entries[b].rel)));
        results
    }

    /// Names the sources that include this sheet in the results.
    pub fn match_sources(e: &Entry, query: &[String], search: SearchIn, image_query: Option<&[f32]>, text_query: Option<&[f32]>) -> Vec<&'static str> {
        if query.is_empty() { return Vec::new(); }
        ["Words", "Image meaning", "Label meaning"].into_iter()
            .zip(Self::match_scores(e, query, search, image_query, text_query))
            .filter_map(|(name, score)| score.map(|_| name)).collect()
    }

    fn match_scores(e: &Entry, query: &[String], search: SearchIn, image_query: Option<&[f32]>, text_query: Option<&[f32]>) -> [Option<f32>; 3] {
        let mut literal = Vec::new();
        if search.folders { literal.extend(e.dir_words.iter().cloned()); }
        if search.files { literal.extend(e.name_words.iter().cloned()); }
        if let Some(label) = &e.side.label {
            if search.captions { literal.extend(words(&label.caption)); }
            if search.tags { literal.extend(words(&label.tags.join(" "))); }
        }
        let prefix = matches(query, |q| literal.iter().any(|w| w.starts_with(q)));
        let exact = query.iter().filter(|q| literal.contains(q)).count() as f32;
        let semantic = |enabled: bool, q: Option<&[f32]>, v: Option<&[f32]>| {
            if !enabled { return None; }
            let (q, v) = q.zip(v)?;
            if q.is_empty() || q.len() != v.len() { return None; }
            let score = embed::cosine(q, v);
            (score >= search.similarity.clamp(0.0, 1.0)).then_some(score)
        };
        [prefix.then_some(exact), semantic(search.embeddings, image_query, e.embed.as_deref()),
            semantic(search.text_embeddings, text_query, e.text_embed.as_deref())]
    }
}

/// An entry that stands for a tileset file, with no image of its own.
fn standing(file: &str, tiled: Option<tiled::Tiled>, error: Option<String>, note: Option<String>) -> Entry {
    let (dirs, name) = file.rsplit_once('/').unwrap_or(("", file));
    Entry {
        dir_words: words(dirs),
        name_words: path_words(name),
        side: Sidecar::default(),
        rel: file.to_string(),
        embed: None,
        text_embed: None,
        tiled,
        in_tileset: false,
        tileset_of: None,
        tileset_only: true,
        tiled_error: error,
        tiled_note: note,
    }
}

/// Lower-case runs of letters and digits, without duplicates.
pub fn words(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in s.chars().chain(std::iter::once(' ')) {
        if ch.is_alphanumeric() {
            cur.extend(ch.to_lowercase());
        } else if !cur.is_empty() {
            if !out.contains(&cur) {
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
    words(&stem.to_string_lossy())
}

pub fn matches(query: &[String], has: impl Fn(&str) -> bool) -> bool {
    query.iter().all(|q| has(q))
}

/// Book keys and tree paths use one separator on every platform.
fn relative_path(path: &Path) -> String {
    path.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(rel: &str, embed: Option<Vec<f32>>) -> Entry {
        Entry {
            dir_words: words("props"), name_words: path_words(rel), side: Sidecar::default(), rel: rel.into(), embed,
            text_embed: None, tiled: None, in_tileset: false, tileset_of: None, tileset_only: false,
            tiled_error: None, tiled_note: None,
        }
    }

    /// The embedding is a sibling of the text fields: a sheet can match by
    /// meaning when no word matches, and the checkbox turns it off. With no
    /// vector ready, nothing matches by meaning.
    #[test]
    fn a_sheet_matches_by_meaning_beside_the_words() {
        let e = entry("props/tree.png", Some(vec![1.0, 0.0]));
        let search = SearchIn::default();
        let query = words("cozy");
        assert!(!Index::entry_matches(&e, &query, search, None, None), "no prefix match, no vector");
        assert!(Index::entry_matches(&e, &query, search, Some(&[1.0, 0.0]), None), "a close vector matches");
        assert!(!Index::entry_matches(&e, &query, search, Some(&[0.0, 1.0]), None), "a far vector does not");
        let off = SearchIn { embeddings: false, ..search };
        assert!(!Index::entry_matches(&e, &query, off, Some(&[1.0, 0.0]), None), "the checkbox turns it off");
        assert!(Index::entry_matches(&e, &words("tree"), search, None, None), "the words still match on their own");
    }

    /// A folder target covers the sheets below it; a file target names them.
    #[test]
    fn a_quantize_target_resolves_to_its_members() {
        let entry = |rel: &str| Entry {
            rel: rel.into(),
            dir_words: Vec::new(),
            name_words: Vec::new(),
            side: Sidecar::default(),
            embed: None,
            text_embed: None,
            tiled: None,
            in_tileset: false,
            tileset_of: None,
            tileset_only: false,
            tiled_error: None,
            tiled_note: None,
        };
        let index = Index {
            root: PathBuf::new(),
            error: None,
            entries: vec![entry("props/tree.png"), entry("props/rock.png"), entry("village.png")],
            dirs: vec!["props".into()],
            tile: [8, 8],
            tag_list: Vec::new(),
            free_tags: sidecar::FREE_TAGS,
            embed_model: String::new(),
            embed_provider: String::new(),
            embed_labels: false,
            embed_error: None,
            text_embed_model: String::new(),
            text_embed_provider: String::new(),
            text_embed_error: None,
            quantize: Vec::new(),
        };
        assert_eq!(index.quantize_members(&QuantizeTarget::Dir("props".into())), ["props/tree.png", "props/rock.png"]);
        assert_eq!(index.quantize_members(&QuantizeTarget::Dir(String::new())), ["props/tree.png", "props/rock.png", "village.png"]);
        assert_eq!(
            index.quantize_members(&QuantizeTarget::Files(vec!["village.png".into(), "gone.png".into()])),
            ["village.png"]
        );
        assert_eq!(index.target_name(&QuantizeTarget::Dir("props".into())), "props/*");
        assert_eq!(index.target_name(&QuantizeTarget::Files(vec!["a.png".into()])), "a.png");
        assert_eq!(index.target_name(&QuantizeTarget::Files(vec!["a.png".into(), "b.png".into()])), "2 files");
    }
    #[test]
    fn text_and_image_search_have_independent_switches() {
        let mut e = entry("tree.png", Some(vec![1.0, 0.0]));
        e.text_embed = Some(vec![0.0, 1.0]);
        let mut search = SearchIn { files: false, folders: false, captions: false, tags: false, ..SearchIn::default() };
        let query = words("forest");
        assert!(!Index::entry_matches(&e, &query, search, Some(&[0.0, 1.0]), Some(&[0.0, 1.0])));
        search.embeddings = false;
        search.text_embeddings = true;
        assert!(Index::entry_matches(&e, &query, search, Some(&[0.0, 1.0]), Some(&[0.0, 1.0])));
        search.similarity = 0.0;
        assert!(!Index::entry_matches(&e, &query, search, None, Some(&[1.0])));
        search.similarity = 0.9;
        assert!(!Index::entry_matches(&e, &query, search, None, Some(&[1.0, 1.0])));
    }

    #[test]
    fn file_search_keeps_dimensions_numbers_and_single_letters() {
        assert_eq!(words("8x8 16 A a Ø 16"), ["8x8", "16", "a", "ø"]);
        let search = SearchIn { folders: false, embeddings: false, ..SearchIn::default() };
        let tiles = entry("terrain_16x16_02.png", None);
        assert!(Index::entry_matches(&tiles, &words("16x16 02"), search, None, None));
        assert!(!Index::entry_matches(&tiles, &words("8x8"), search, None, None));
        let letter = entry("a.png", None);
        assert!(Index::entry_matches(&letter, &words("a"), search, None, None));
        assert!(!Index::entry_matches(&letter, &words("b"), search, None, None));
    }

    /// The library scan pairs a tileset with the image it names, and a
    /// collection gets a card of its own standing on the tileset file. A
    /// project scan reads images only, so its tree does not change.
    #[test]
    fn a_library_scan_pairs_tilesets_with_their_images() {
        let dir = crate::storage::tests::Folder::new();
        std::fs::create_dir_all(dir.0.join("Tiles")).unwrap();
        std::fs::create_dir_all(dir.0.join("Props")).unwrap();
        for rel in ["Tiles/atlas.png", "Tiles/loose.png", "Props/a.png", "Props/b.png"] {
            image::RgbaImage::new(4, 4).save(dir.0.join(rel)).unwrap();
        }
        std::fs::write(dir.0.join("Tiles/atlas.tsx"),
            r#"<tileset name="Atlas" tilewidth="4" tileheight="4" columns="1" tilecount="1">
                 <image source="atlas.png" width="4" height="4"/>
               </tileset>"#).unwrap();
        std::fs::write(dir.0.join("Props/props.tsx"),
            r#"<tileset name="Props" tilewidth="4" tileheight="4" tilecount="2">
                 <tile id="0"><image source="a.png"/></tile>
                 <tile id="1"><image source="b.png"/></tile>
               </tileset>"#).unwrap();

        let index = Index::scan_library(&dir.0, [8, 8]);
        let at = |rel: &str| index.entries.iter().position(|e| e.rel == rel).unwrap();
        assert_eq!(index.entries[at("Tiles/atlas.png")].tiled.as_ref().unwrap().name, "Atlas");
        assert!(index.entries[at("Props/a.png")].in_tileset);
        assert!(index.entries[at("Props/b.png")].in_tileset);
        assert_eq!(index.entries[at("Props/a.png")].tileset_of.as_deref(), Some("Props/props.tsx"));
        assert_eq!(index.entries[at("Props/a.png")].tileset_file(), Some("Props/props.tsx"));
        assert_eq!(index.entries[at("Tiles/atlas.png")].tileset_of, None, "an atlas names its image, not the other way");
        assert_eq!(index.entries[at("Tiles/atlas.png")].tileset_file(), Some("Tiles/atlas.tsx"));
        assert_eq!(index.entries[at("Tiles/loose.png")].tileset_file(), None);
        assert!(index.entries[at("Props/props.tsx")].tileset_only);
        assert_eq!(index.entries[at("Props/props.tsx")].sheet_rel(), None);
        assert_eq!(index.view_counts(), (2, 3, 1));

        let view = View::default();
        let shown: Vec<&str> = index.entries.iter().filter(|e| e.shows(view)).map(|e| e.rel.as_str()).collect();
        assert_eq!(shown, ["Props/props.tsx", "Tiles/atlas.png", "Tiles/loose.png"]);
        assert_eq!(index.entries[at("Tiles/atlas.png")].card(view), Card::Tileset);
        let images = View { tilesets: false, in_tilesets: true, loose: false };
        assert_eq!(index.entries[at("Tiles/atlas.png")].card(images), Card::Image);
        assert!(!index.entries[at("Tiles/loose.png")].shows(images));

        let project = Index::scan(&dir.0, [8, 8]);
        assert_eq!(project.entries.len(), 4, "the four images, and no tileset card");
        assert!(project.entries.iter().all(|e| !e.has_tileset() && !e.in_tileset));
        // A folder with no tileset ignores the three boxes, so a filter set
        // for another pack cannot empty it.
        let empty = View { tilesets: true, in_tilesets: false, loose: false };
        assert_eq!(project.effective_view(empty), View::everything());
        assert!(project.entries.iter().all(|e| e.shows(project.effective_view(empty))));
    }

    fn search_index(entries: Vec<Entry>) -> Index {
        let mut index = Index::scan(Path::new(""), [16, 16]);
        index.entries = entries;
        index
    }

    #[test]
    fn ranked_search_preserves_visible_membership_and_empty_query_order() {
        let mut label_match = entry("c.png", None);
        label_match.text_embed = Some(vec![1.0, 0.0]);
        let index = search_index(vec![entry("a.png", Some(vec![1.0, 0.0])), entry("tree.png", None),
            label_match, entry("d.png", Some(vec![0.0, 1.0]))]);
        for flags in 0..8 {
            let search = SearchIn { files: flags & 1 != 0, folders: false, captions: false, tags: false,
                embeddings: flags & 2 != 0, text_embeddings: flags & 4 != 0, similarity: 0.8 };
            let query = words("tree");
            let expected: Vec<usize> = index.visible(&query, search, Some(&[1.0, 0.0]), Some(&[1.0, 0.0]), View::default())
                .unwrap().iter().enumerate().filter_map(|(i, &visible)| visible.then_some(i)).collect();
            let mut actual = index.ranked(&query, search, Some(&[1.0, 0.0]), Some(&[1.0, 0.0]));
            actual.sort_unstable();
            assert_eq!(actual, expected);
            assert_eq!(index.ranked(&[], search, None, None), vec![0, 1, 2, 3]);
        }
    }

    #[test]
    fn exact_words_precede_prefixes_and_equal_matches_use_paths() {
        let index = search_index(vec![entry("z/tree.png", None), entry("a/trees.png", None), entry("b/tree.png", None)]);
        let search = SearchIn { folders: false, ..SearchIn::default() };
        assert_eq!(index.ranked(&words("tree"), search, None, None), vec![2, 0, 1]);
        assert_eq!(Index::match_sources(&index.entries[0], &words("tree"), search, None, None), vec!["Words"]);
    }

    #[test]
    fn fusion_uses_source_ranks_instead_of_raw_similarity() {
        let mut a = entry("a.png", Some(vec![1.0, 0.0]));
        a.text_embed = Some(vec![0.6, 0.8]);
        let mut b = entry("b.png", Some(vec![0.99, 0.1]));
        b.text_embed = Some(vec![1.0, 0.0]);
        let mut index = search_index(vec![a, b]);
        let search = SearchIn { files: false, folders: false, captions: false, tags: false,
            embeddings: true, text_embeddings: true, similarity: 0.5 };
        // Each sheet leads one source, so the path decides the tied fusion score.
        assert_eq!(index.ranked(&words("forest"), search, Some(&[1.0, 0.0]), Some(&[1.0, 0.0])), vec![0, 1]);
        index.entries[0].embed = Some(vec![0.7, 0.7]);
        index.entries[1].embed = Some(vec![0.6, 0.8]);
        assert_eq!(index.ranked(&words("forest"), search, Some(&[1.0, 0.0]), Some(&[1.0, 0.0])), vec![0, 1]);
        assert_eq!(Index::match_sources(&index.entries[0], &words("forest"), search, Some(&[1.0, 0.0]), Some(&[1.0, 0.0])),
            vec!["Image meaning", "Label meaning"]);
    }

    #[test]
    fn agreement_between_sources_precedes_a_single_source_match() {
        let index = search_index(vec![entry("a.png", Some(vec![1.0, 0.0])), entry("tree.png", Some(vec![0.8, 0.6]))]);
        let search = SearchIn { folders: false, similarity: 0.5, ..SearchIn::default() };
        assert_eq!(index.ranked(&words("tree"), search, Some(&[1.0, 0.0]), None), vec![1, 0]);
    }

}
