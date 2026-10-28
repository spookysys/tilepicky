# Open issues

This list distinguishes confirmed limits from reports that still need a reproduction.
The [workflow report](tileset-workflow-results.md) records the latest completed checks.

## Referenced tile data needs a separate interoperability pass

Scalar properties and ordinary per-tile fields follow the tested edits.
Imported Tiled data can also refer to other tile IDs, files, or project definitions.
The current transfer code does not define all these relationships across partial copies and different folders.
Animation frame references, Wang sets, and relative file properties need fixtures before those workflows can be considered supported.

Check a partial animation copy, moving its frames, resizing its sheet, and copying a file property into another folder.
Verify the result in Tiled, including referenced tiles and files. Preserve or refuse each unsupported operation explicitly.
Map reference updates are explicit. See [map update coverage](#map-update-coverage) for the supported subset.

## Map update coverage

Map change recording supports explicit updates of external `.tsj` references in Tiled JSON maps with array tile data.
The [user guide](../README.md#map-change-recording) defines its controls and supported edits.

The updater refuses TMX, embedded tilesets, compressed or encoded layers, object templates, and tileset ID range collisions.
It also refuses maps that use deleted tiles or that changed externally after their last update.
Add fixtures for each format and verify them in Tiled before extending support.
Tests must retain flip flags, other tilesets, custom data, and backup recovery.

Cut records removal. A later paste does not reconnect the removed tile identity.
Use drag to relocate tiles while recording. Test clipboard identity across deferred confirmation, repeated pastes, and Undo before adding cut relocation.

The first selection of a map assumes it uses the layout from when recording began; the preview states this requirement.
Map discovery and external edits cannot establish that relationship automatically.
Renaming or moving sheet files does not rewrite map paths. Handle those path changes in Tiled.
Undo stops at recording boundaries; crossing a boundary needs a defined mapping before that restriction can be removed.

## Saves are atomic per file

`Sheet::save` validates the tileset before writing the PNG, then writes the tileset and the folder book.
Each replacement is atomic. A later disk failure can still leave an earlier file saved.
The error reports the incomplete write, and the sheet remains dirty.
A future transaction design needs fault injection between each write and a documented recovery rule.

## Earlier usability findings need reproduction

The October 27 interactive review reported the following issues. This pass did not establish their current status:

- Quantized exports appear among source sheets. The export button and file dialog need a desktop retest.
- Disabled search-generation controls do not explain every missing prerequisite.
- The color panel can open below its heading. The animation panel needs clearer first-use instructions.
- The search placeholder resembles a query. The Without tags checkbox square reportedly does not respond like its label.
- Keyboard selection lacks a persistent coordinate readout. Focus navigation and first clicks need a focused test.
- The application does not restore the open sheets and workspace after restart.

Reproduce each finding with isolated settings and native input. Record the action, visible result, and saved-file effects before choosing a fix.
Search currently filters the project tree as well as the library tree. The October 28 test had to clear `terrain` to find `meadow.png`.
Review whether this behavior fits the source-to-project workflow before changing it.

## Game pipeline verification remains separate

The completed workflow ends at Tiled. It does not establish acceptance by a game compiler or compliance with viewport palette budgets.
Use a representative map and the game's own compiler for that check. Animation export also needs a separate end-to-end test.
