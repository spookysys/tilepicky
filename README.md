# Tilepick

A small desktop tool to browse a large set of sprite sheets, search them, and
copy cells into tilemaps of your own. Every sheet is on a 32 px grid.

## Run

    cargo run --release -- [--tile N] <source dir> <destination dir>

`--tile` sets the default tile size in pixels (32 when absent). Each sheet
can override it with the "tile" field in its header; the override is stored
in `tilepick.json`.

The source directory holds the original sheets. The destination directory
holds the tilemaps you make. The tool creates it when it does not exist.

## Formats

The tool reads PNG, GIF, JPEG, WebP, BMP, and TGA. It writes one
format only: 32 bit RGBA PNG with straight alpha, which every engine reads.

## Layout

The left column holds the search box, the tree of source sheets, and the tree
of your tilemaps. The top panel shows the source sheet you opened. The bottom
panel shows your tilemap.

## Keys

| Key | Effect |
| --- | --- |
| click | select one cell |
| drag | select a range of cells; near the edge of the view it scrolls |
| press and hold ~250 ms | lift the tile under the pointer (or the whole selection) and drag it; drop with Ctrl held to copy inside your tilemap |
| shift+click | select the rectangle from the last clicked cell to this one |
| Ctrl+click | add or remove one cell |
| Ctrl+shift+click | add that rectangle to the selection |
| Ctrl+A | select the whole sheet |
| right click | clear the selection |
| Ctrl+C | copy the selection of the active panel |
| Ctrl+X | cut: copy, then clear the cells (your tilemap only) |
| Ctrl+V | paste at the selected cell of your tilemap |
| Delete | clear the selection in your tilemap |
| A | mark the selected area as an animation strip, or unmark it |
| Ctrl+Z | undo |
| Ctrl+S | save |
| Ctrl+Shift+S | save as |
| Ctrl+T | trim empty columns on the right and empty rows at the bottom |
| drag the right or bottom edge of the canvas | resize your tilemap |
| Ctrl+wheel, + / - | zoom |
| Escape | clear the selection |

Every edit is written to disk at once. Undo stays available while the tool
runs.

## Search

Type words in the search box. The trees show only the files whose path holds
every word: a word matches when it is the prefix of a word in a file or
folder name. Search inside the tiles (captions, tags) is planned for a later
version.

## tilepick.json

Each directory, the source and the one with your tilemaps, holds one
`tilepick.json`. It describes the sheets in that directory: the grid (tile
size, gap, offset), the origin of cells, and animations. The file is keyed
by the path relative to the directory.

    {
      "erw/erw_grass_land/Tilesets/Tileset-Terrain.png": {
        "tile": 32,
        "animations": [ { "px": [1280, 64], "frame": [64, 64], "frames": 6, "ms": 100 } ]
      },
      "terrain.png": {
        "tile": 32,
        "provenance": [
          { "source": "erw/erw_sewers/Props/atlas-props.png", "rects": [[96, 48, 96, 96]] }
        ],
        "animations": [ { "px": [256, 32], "frame": [64, 64], "frames": 3, "ms": 100 } ]
      }
,
        "animations": [ { "px": [256, 32], "frame": [64, 64], "frames": 3, "ms": 100 } ]
      }
    }

For your tilemaps, the tool writes the entry on Ctrl+S, together with the
PNG. For a source sheet, a grid setting or an animation you store with `A`
is written at once. The tool re-reads the file before it writes an entry,
so changes you make by hand survive.

## Tile sizes

The source and your tilemap may use different tile sizes. A copy is pixel for
pixel: the block lands with its top-left on the target cell and is padded
with transparent pixels to whole cells. Provenance and animations are pixel records,
so the tile size does not touch them. An animation is a place on the
bitmap, in pixels, so the tile size does not touch it. Changing a sheet's tile size re-grids the
cell records by pixel position, and a source sheet gets its derived cell
names again at the new grid.

## Your tilemaps

A tilemap is a `name.png` with its entry in `tilepick.json`. The entry records,
for each cell, the source sheet and the cell in that sheet. The search box
works on your tilemaps too, with the same rules. `provenance` lists, per
source file, the pixel rectangles `[x, y, w, h]` of your tilemap that came
from it. Where in the source they came from is not recorded: the source file
answers that. In memory the tool keeps one source index per pixel, so
overwrites are exact; the rectangles are computed when you save.

## Animations

An animation is a strip of frames, left to right: a pixel rectangle and a
frame size, stored as `{ "px": [x, y], "frame": [w, h], "frames": n, "ms": t }`.
Select the strip and press `A`: the animation panel opens on the right and
plays the selection, one frame per column. Set the number of frames; the
frame width follows (it must divide the strip's pixel width). Press `A`
again, or the Store button, to store it. A stored animation travels with the
block when you copy or drag it. The fields edit the stored animation under
the selection.
