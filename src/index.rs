// SPDX-License-Identifier: GPL-3.0-only
//! Scans a directory of sheets and searches their names, captions, and tags.

use crate::embed;
use crate::settings::SearchIn;
use crate::sidecar::{self, Pair, QuantizeGroup, QuantizeTarget, Sidecar};
use std::path::{Path, PathBuf};

/// The formats the tool reads. It always writes 32 bit RGBA PNG.
pub const IMAGE_EXTS: [&str; 7] = ["png", "gif", "jpg", "jpeg", "webp", "bmp", "tga"];

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
    pub fn scan(root: &Path, default_tile: [u32; 2]) -> Self {
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
                }
            }
        }
        rels.sort();
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
        let entries = rels
            .into_iter()
            .map(|rel| {
                let side = book.sheets.remove(&rel).unwrap_or_default();
                let (dirs, name) = rel.rsplit_once('/').unwrap_or(("", &rel));
                let (dir_words, name_words) = (words(dirs), path_words(name));
                let vector = embed::current_vector(root, &rel, &side, &embeddings, embed::InputMode::Image);
                let text_embed = embed::current_vector(root, &rel, &side, &text_embeddings, embed::InputMode::Text);
                Entry { dir_words, name_words, side, rel, embed: vector, text_embed }
            })
            .collect();
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
                let mut out: Vec<String> = rels.iter().filter(|r| self.position(r).is_some()).cloned().collect();
                out.sort();
                out.dedup();
                out
            }
            QuantizeTarget::Dir(dir) => self
                .entries
                .iter()
                .filter(|e| dir.is_empty() || e.rel.starts_with(&format!("{dir}/")))
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
        let starts = |ws: &[String], q: &str| ws.iter().any(|w| w.starts_with(q));
        let label = e.side.label.as_ref();
        let caption = label.filter(|_| search.captions).map(|l| words(&l.caption)).unwrap_or_default();
        let tags = label.filter(|_| search.tags).map(|l| words(&l.tags.join(" "))).unwrap_or_default();
        let prefix = matches(query, |q| (search.folders && starts(&e.dir_words, q)) || (search.files && starts(&e.name_words, q))
            || starts(&caption, q) || starts(&tags, q));
        let similar = |query: Option<&[f32]>, vector: Option<&[f32]>| {
            query.zip(vector).is_some_and(|(q, v)| !q.is_empty() && q.len() == v.len()
                && embed::cosine(q, v) >= search.similarity.clamp(0.0, 1.0))
        };
        let semantic = search.embeddings && similar(embed, e.embed.as_deref());
        let text_semantic = search.text_embeddings && similar(text_query, e.text_embed.as_deref());
        prefix || semantic || text_semantic
    }

    pub fn visible(&self, query: &[String], search: SearchIn, embed: Option<&[f32]>, text_query: Option<&[f32]>) -> Option<Vec<bool>> {
        if query.is_empty() {
            return None;
        }
        Some(self.entries.iter().map(|e| Self::entry_matches(e, query, search, embed, text_query)).collect())
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
        Entry { dir_words: words("props"), name_words: path_words(rel), side: Sidecar::default(), rel: rel.into(), embed, text_embed: None }
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

}
