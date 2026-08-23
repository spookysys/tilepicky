# Tilepick

A small desktop tool to browse a large set of sprite sheets, search them, and
copy cells into tilemaps of your own. Every sheet is on a 32 px grid.

## Run

    cargo run --release -- <source dir> <destination dir>

The source directory holds the original sheets. The destination directory
holds the tilemaps you make. The tool creates it when it does not exist.

## Layout

The left column holds the search box, the tree of source sheets, and the tree
of your tilemaps. The top panel shows the source sheet you opened. The bottom
panel shows your tilemap.

## Keys

| Key | Effect |
| --- | --- |
| click | select one cell |
| drag | select a range of cells; near the edge of the view it scrolls |
| shift+click | select the rectangle from the last clicked cell to this one |
| Ctrl+click | add or remove one cell |
| Ctrl+shift+click | add that rectangle to the selection |
| Ctrl+A | select the whole sheet |
| right click | clear the selection |
| Ctrl+C | copy the selection of the active panel |
| Ctrl+V | paste at the selected cell of your tilemap |
| Delete | clear the selection in your tilemap |
| A | mark the selected area as an animation strip, or unmark it |
| Ctrl+Z | undo |
| Ctrl+S | save |
| Ctrl+T | trim empty columns on the right and empty rows at the bottom |
| drag the right or bottom edge of the canvas | resize your tilemap |
| Ctrl+wheel, + / - | zoom |
| Escape | clear the selection |

Every edit is written to disk at once. Undo stays available while the tool
runs.

## Search

Type words in the search box. The trees show only the files that match every
word. A word matches when it is the prefix of a word in the file path, or of a
tag on a cell. When you open a sheet, the cells that match are highlighted.

Tags on cells come from the individual sprite files that sit beside a sheet.
At the first start, the tool finds each sprite inside the sheets in its own
folder, its parent folder, and the grandparent folder. The cells a sprite
covers get the words of its file name. The result is cached under
`~/.cache/tilepick/`.

## tilepick.json

Each directory, the source and the one with your tilemaps, holds one
`tilepick.json`. It describes the sheets in that directory: tags for a whole
sheet, tags and origins for single cells, and animations. The file is keyed by
the path relative to the directory.

    {
      "erw_grass_land/Tilesets/Tileset-Terrain.png": {
        "tags": ["grass", "dirt", "cliff"],
        "cells": { "3,4": { "tags": ["flower"] } },
        "animations": [ { "x": 40, "y": 2, "w": 2, "h": 2, "frames": 6, "ms": 100 } ]
      },
      "terrain.png": {
        "cells": {
          "0,0": { "src": "erw_sewers/Props/atlas-props.png", "at": [12, 7], "tags": ["laboratory", "potions"] }
        },
        "animations": [ { "x": 8, "y": 1, "w": 2, "h": 2, "frames": 3, "ms": 100 } ]
      }
    }

For your tilemaps, the tool writes the entry on Ctrl+S, together with the
PNG. For a source sheet, an animation you store with `A` is written at once.
The tool re-reads the file before it writes an entry, so tags you add by hand
survive. The derived cell names from the indexer do not go into this file;
they stay in the cache.

## Your tilemaps

A tilemap is a `name.png` with its entry in `tilepick.json`. The entry records,
for each cell, the source sheet, the cell in that sheet, and the tags. The
search box works on your tilemaps too, with the same rules.

## Animations

An animation is a strip of frames, left to right. Each frame is `w` x `h`
cells. Select the strip and press `A`: the animation panel opens on the right
and plays the selection, one frame per column. Set the number of frames; the
frame width follows. Press `A` again, or the Store button, to store it. For a
chest of 2x3 cells with 6 frames, select the 12x3 area, set frames to 6,
store. A stored animation travels with the block when you copy or drag it.
The panel stays open for sheets that have stored animations, and the fields
edit the stored animation under the selection.
