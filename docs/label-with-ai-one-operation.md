# Keep labeling and embedding sources independent

Status: replaces the earlier proposal to require one combined operation.

Labeling produces captions and tags. Search can use their words directly, their embeddings, or image embeddings.
These sources need separate controls so the user can compare their results and costs.

The library AI panel offers two embedding sources:

- Images, saved in `embeddings.json`.
- Captions and tags, saved in `embeddings-text.json`.

The image source can optionally include captions and tags. This provides a combined input for comparison.
It does not replace the independent text source. Neither image generation nor word search requires a successful labeling operation.

Each source records its model and provider identity. Changed source content invalidates the affected vectors.
Generation preserves completed batches. A retry skips saved vectors whose source still matches.

The search filter exposes both meaning sources and the original word fields independently.
Enabled sources add matches. The minimum similarity control applies to both meaning sources.
Results keep their folder order. A future ranked view can expose scores if comparisons show that it helps.
