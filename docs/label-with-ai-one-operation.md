# Keep labeling and embedding sources independent

Status: implemented. This note replaces the earlier proposal to require one combined operation.
See the [user guide](../README.md#search-by-meaning) for current controls.

Labeling produces captions and tags. Search can use their words directly, their embeddings, or image embeddings.
These sources need separate controls so the user can compare their results and costs.

**Improve search...** offers two embedding sources:

- Images, saved in `embeddings.json`.
- Captions and tags, saved in `embeddings-text.json`.

The image source excludes captions and tags. The text source excludes images.
Both sources use the Search embeddings model by default, with an optional separate caption embedding model.
Selecting both sources runs separate requests in sequence and saves separate indexes.
Neither image embedding generation nor word search requires a successful labeling operation.

Each source records its model and provider identity. Changed source content invalidates the affected vectors.
Generation preserves completed batches. A retry skips saved vectors whose source still matches.

The search filter exposes both meaning sources and the original word fields independently.
Enabled sources add matches. The minimum similarity control applies to both meaning sources.
The tree keeps its folder order. The gallery ranks matches.
**Compare search** shows separate word, image, and label result lists. Similarities from different models are not directly comparable.
