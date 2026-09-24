# Island detection

The app first groups grid cells into rectangles. A rectangle can include transparent
cells. Entirely empty regions are discarded, and configured grid gaps stay out
of pixel comparisons, crops, and highlights. Detection uses no AI.

The detector starts with rectangular runs of occupied and empty cells. It then
compares cuts and joins using one integer score. Joins can cross earlier cuts,
split a neighboring rectangle, or include the empty corners around an object.
Only improvements are accepted. Fixed traversal order resolves ties.

The score rewards internal continuity and charges for each island and each empty
cell inside it. Shared edges use the existing appearance comparison plus the
amount of contact. Sparse contact gives weaker positive support. Transparent
space does not subtract from that support; no contact still cuts. Summed-area
tables make rectangle scores inexpensive; only neighboring regions propose joins.
This is a local search, not a guarantee of the best possible partition.

A final pass can join rectangles into irregular islands. It sums the existing
edge scores across each pair's shared boundary and merges the strongest positive
pair. It recalculates the boundaries after each join. Zero and negative totals
stay separate. This pass adds no cells and uses no reward for reducing the number
of islands. It can still merge touching objects with similar appearances.

## Parameter search

The original search, before the final merge pass, varied the island cost, empty-cell cost, and separation weight. It
uses the local `Overworld.png` and `Inner.png` examples from the Zelda-like pack.
The annotations are in `src/islands/partition.rs`.

Seven objects select the parameters. Eight other objects report validation
results but do not select the winner. Each extra fragment costs 400 points.
Extra occupied cells gathered from outside an object cost 100 points per object's
worth of occupied cells. Lower scores are better. This favors keeping objects
whole while still penalizing large merged groups.

The tested grid contains 29 valid configurations. The selected weights are 64
for an island, 64 for an empty cell, and 128 for separation. Continuity has weight
64. Configurations that cheaply bridge empty cells or cannot cut a clear boundary
are excluded before evaluation.

| Detector | Tuning error | Validation error |
| --- | ---: | ---: |
| Previous connected components | 2582 | 3241 |
| Selected rectangular detector | 750 | 1503 |

These historical scores exclude the final merge pass. Visual review found the
rectangular result worse in parts of the sheet despite its lower score.
They are scores on a small annotated subset of two related sheets, not an
accuracy estimate for arbitrary libraries. Some neighboring objects still merge,
and some roof details remain separate. The normal tests also check geometry,
transparency, deterministic results, and joins that undo earlier cuts.

To run the search with the final merge pass and the example assets present:

```sh
cargo test --release tune_parameters -- --ignored --nocapture
```

To compare both detectors visually, choose an output directory:

```sh
TILEPICKY_RECTANGLE_PREVIEW=/tmp/tilepicky-preview cargo test tilemap_examples -- --nocapture
```

This writes SVG comparisons and does not modify the source sheets or their books.
Normal tests skip the local image examples when the assets are absent. The
explicit parameter search requires both sheets. It does not change the chosen
constants automatically.

Existing saved islands remain unchanged until you choose **Detect islands** again.
