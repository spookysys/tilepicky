# Tilesheet-to-Tiled workflow

Test date: 2026-10-28.

The target is a usable authoring sequence: search, assemble, assign properties, rearrange, save, reopen, and use the result in Tiled.
This pass uses generated artwork, a private X11 display, and isolated application settings.
It sends no AI requests and does not edit the user's library or project.

## Changes

Tile metadata now participates in the existing block edits and sheet history.
A lightweight history step records property edits without copying image pixels.
Repeated changes to one property field share a history step until the edit ends.
Canvas resizing converts tile IDs through cell coordinates, so a width change does not move metadata to another row.

Normal replacement carries source properties. Shift-drag keeps destination properties while it merges pixels.
A paste refuses metadata-bearing tiles with a different tile size, because no unique destination tile exists.
Grid changes stop while tiles have metadata. The user guide describes these rules in one place.

Save As retains the Tiled fields read with the original sheet and selects the filename stem in its dialog.
The save path parses the destination tileset before replacing image pixels.
The properties panel respects Inspect mode, and turquoise markers identify tiles with metadata.

## Automated checks

The initial move regression failed: the destination had no `terrain` property.
The fixed regression moves a tile, grows the canvas, clears the destination, undoes the clear, saves, and reloads its metadata.
Additional checks cover property undo and redo, replacement versus merging, different tile sizes, sparse swaps, trim, and canvas resizing.
The save checks preserve Tiled fields across Save As and reject malformed tileset JSON before changing PNG bytes.

The full suite passes 334 tests, with three intentionally ignored tests.
The ignored tests require an interactive fixture or paid provider requests.
Clippy passes for all targets. The locked release build passes.
These checks use the default OpenGL build. They do not establish WGPU behavior.

The sandbox blocked three tests from binding local mock HTTP servers.
Those tests passed when the full suite ran outside the sandbox with isolated test settings.
An earlier assertion compared saved format fields with an empty in-memory template; it now checks the properties that the file operation must preserve.

## Native checks

The native pass used actual mouse and keyboard events in Tilepicky and Tiled 1.12.2.
Screenshot evidence and generated fixtures remain under `private/workflow-review/`.

| Action | Observed result | Evidence |
| --- | --- | --- |
| Search for `terrain` | The generated library sheet appears. | `01-search-before.png` |
| Open Build and paste two tiles | The source stays above the project. Two selected tiles appear in the project. | `06-properties-before.png` |
| Assign `terrain=grass` and save | The tileset stores the property on IDs 0 and 1. | `08-saved-before.png` |
| Reopen in the updated build | The values remain, and turquoise markers identify the two tiles. | `09-properties-after.png` |
| Drag both tiles to column 3, row 1 | The saved metadata moves to IDs 11 and 12. The original cells lose the properties. | `10-moved-after.png` |
| Change the value to `forest`, Undo, then Redo | One Undo restores `grass`; Redo restores `forest`. | `11-property-undo.png`, `12-property-redo.png` |
| Save As `meadow-copy` | The result is `meadow-copy.png` and `meadow-copy.tsj`. Both properties survive. | `13-save-as.png`, `14-save-as-result.png` |
| Open the original tileset in Tiled | Tiled identifies tile 11 and its custom `terrain` property. | `20-tiled-tile.png` |
| Open properties in Inspect mode | The final release build disables the editors and explains how to enable them. | `27-inspect-properties.png` |
| Paint and save a Tiled map | Seven cells reference tile 11 through `meadow.tsj`. The saved map contains GID 12 with `firstgid=1`. | `26-tiled-map.png`, `project/test-map.tmx` |

The private displays, application processes, and window manager stopped after the checks.
The test's entry in the desktop recent-file list was removed. Screenshots and generated fixtures remain in the ignored evidence folder.

The [open issues](issues.md) record the remaining interoperability and usability work.
This pass does not verify the game's compiler, paid labeling, or imported animation and Wang-set transfers.
