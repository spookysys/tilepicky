# Tilepicky

A small desktop tool to browse a large set of sprite sheets, search them, and
copy cells into tilesheets of your own. Each sheet has its own grid.

<https://github.com/spookysys/tilepicky>

![A tilesheet of your own is built from two packs, found through the search box](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/demo.gif)

That is the loop: open a tilesheet of your own, search the packs for what the
map needs, select the cells, hold the button until they lift, and carry them
over. The sheets in the pictures are from [Kenney](https://kenney.nl) and
from [ArMM1998](https://opengameart.org/content/zelda-like-tilesets-and-sprites),
both in the public domain (CC0).

## The library and the project

Tilepicky works with two folders. It reads each one with all its subfolders.

Your **library** holds the tilesheets and packs you collected. The
tool helps you browse and search them, and copy what you need into your own
tilesheets. It leaves the images as they are, and writes only a `tilepicky.json`
that remembers their grids and animations.

Your **project** holds the tilesheets you make and edit. The tool writes them
there, with a `tilepicky.json` beside them.

You can start the tool without a folder. A panel without a folder tells you
so. Click in the panel to open the folder dialog. The right-click menu of each
tree has the item "Set library folder" or "Set project folder". When a folder
is set, the item reads "Change ...", and you can choose a different folder at
any time. The tool stores both folders in `~/.config/tilepicky/settings.json`.

You can also name them when you start the tool:

    cargo run --release -- [<library dir> [<project dir>]]

## Layout

The left column holds the search box, the tree of your library, and the tree
of your project. The top panel shows the library sheet you opened. The bottom
panel shows the tilesheet you are building.

![The three panels: the trees on the left, the library sheet above, the tilesheet below](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/screenshot.png)

Each panel has a header line. It holds the grid fields, the zoom, the
selection, the name of the sheet, and the cell under the pointer. In your
tilesheet, the cell also names the sheet its pixels came from:

    cell 4,2 <- kenney_tiny-town/Tilemap/tilemap_packed.png

See "Provenance tracking" for how the tool remembers that.

Select cells and press `A`, and the animation panel opens on the right. See
"Animations".

## Animations

An animation is a block of frames on the bitmap: a place in pixels, a frame
size, the frames in a row and the number of rows, and the time per frame. The
frames play left to right, then the next row down.

Select the cells that hold the frames and press `A`. The animation panel
opens on the right and plays them. The frames field works like the tile
field: drag it for the frames in a row, turn the mouse wheel for the number
of rows, click it to type. The frame size follows, so each number must divide
its side of the selection. The panel says so when it does not, and it stays
open while you drag the edges of the selection, until the numbers fit. Press
`A` again, or the Store button, to store the animation.

![Two blocks of water cells become animations: paste, A, set the frames, Store](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/animation-panel.gif)

A stored animation travels with the block when you copy or drag it. When the
selection lies on a stored animation, the fields edit that one.

An animated GIF plays in the library panel. When you copy a region that moves
between the frames, the frames unroll into one strip, marked as an
animation. A region that stands still gives one picture.

![A waterfall is taken out of an animated GIF and lands as a marked strip](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/animation.gif)

The scene in that picture is a mockup from the [Epic RPG World](https://rafaelmatos.itch.io/epic-rpg-world-collection)
packs by RafaelMatos, from a purchased copy.

## Formats

The tool reads PNG, GIF, JPEG, WebP, BMP, and TGA. It writes one
format only: 32 bit RGBA PNG with straight alpha.

## The grid

The header of each panel holds the grid fields. A sheet keeps its own values.

| Field | Meaning |
| --- | --- |
| tile | the size of one cell, `32` or `32x48` |
| gap | pixels between neighbouring tiles, `1` or `1x2` (Kenney sheets use 1) |
| offset | pixels before the first tile, `4` or `4x8`; `-3` when the first tile starts before the edge |

Every field takes the same three actions. Drag it left and right to adjust
the first number, the width. A single number changes as one. A pair changes
only its width. Turn the mouse wheel over it to adjust the second number, the
height. A single `32` then becomes `32x48`. Click it to type any value.

A new sheet starts with the tile size its library or project used last, or
32 px.

## Keys

| Key | Effect |
| --- | --- |
| click | select one cell |
| drag | select a range of cells; near the edge of the view it scrolls |
| press and hold ~250 ms | lift the tile under the pointer, or the whole selection, and drag it |
| double click and drag | lift at once, without the wait |
| drag an edge of the selection | move that edge; out of the selection adds cells, into it removes them |
| shift+click | select the rectangle from the last clicked cell to this one |
| Ctrl+click | add or remove one cell |
| Ctrl+shift+click | add that rectangle to the selection |
| Ctrl+A | select the whole sheet |
| right click | clear the selection; inside the selection it clears the cells |
| Ctrl+C | copy the selection of the active panel |
| Ctrl+X | cut: copy, then clear the cells (your tilesheet only) |
| Ctrl+V | paste at the selected cell of your tilesheet |
| Delete | clear the selection in your tilesheet |
| A | open the animation panel; again, store the animation or remove it |
| Ctrl+Z | undo |
| Ctrl+S | save |
| Ctrl+Shift+S | save as |
| Ctrl+T | trim empty columns on the right and empty rows at the bottom |
| drag the right or bottom edge of the canvas | resize your tilesheet |
| Ctrl+wheel, + / - | zoom |
| Escape | clear the selection, or cancel a drag |

While you drag a block, two keys change what the drop does. Ctrl copies: the
cells it came from stay as they are. Alt exchanges the two places: what lies
where the block lands travels back to the place the block came from. A sign
in the corner of the block shows which one is in force. A block from the
library panel can only be copied, because the library never changes.

A drop on an empty tilesheet panel starts a new tilesheet. It takes the tile size
of the block you dropped, and it asks for a name at the first save.

Your tilesheet is written when you save it. Undo stays available while the tool
runs.

## Files

Both trees answer the same actions. The project tree also changes files; the
library tree does not.

| Action | Effect |
| --- | --- |
| click | open the file |
| up, down | walk the files of the panel you last worked in, and open them |
| Enter | open the file the group ends on |
| shift+up, shift+down | grow the marked group in the project |
| Ctrl+click, shift+click | mark one file, or a range |
| drag across the files | mark every file the pointer crosses |
| press and hold ~250 ms, then drag | carry the file, or the marked group, into a folder |
| right click a file | rename, duplicate, delete; open its location; copy its path |
| right click a folder | new folder, rename, delete |
| right click the free space | new folder, refresh |

A carried file moves into the folder under the pointer. Hold Ctrl on the drop
to copy it instead. The book entry follows the file, and an open tilesheet
keeps its identity when its own file moves. A name that the target folder
holds already is refused, and the other files still move.

"Open location" asks the desktop's file manager to show the file. "Copy
relative path" writes the path as you would type it in the directory where
tilepicky runs; "Copy absolute path" writes the whole path, with your home
directory as `~`.

## Search

Type words in the search box. The trees show only the files whose path holds
every word: a word matches when it is the prefix of a word in a file or
folder name. Search inside the tiles (captions, tags) is planned for a later
version.

## tilepicky.json

The library and the project each hold one `tilepicky.json` in their top
folder. It describes every sheet in the tree below: the grid (tile size, gap,
offset), where the pixels came from, and animations. The sheets are keyed by
their path from the top folder, and `tile` at the start of the file is the
size that tree used last.

    {
      "tile": 16,
      "sheets": {
        "kenney_tiny-town/Tilemap/tilemap_packed.png": {
          "animations": [
            { "px": [1280, 64], "frame": [64, 64], "frames": 6, "ms": 100 }
          ]
        },
        "village.png": {
          "tile": [32, 48],
          "provenance": [
            { "source": "kenney_tiny-town/Tilemap/tilemap_packed.png", "rects": [[96, 48, 96, 96]] }
          ],
          "animations": [
            { "px": [256, 32], "frame": [64, 64], "frames": [4, 2], "ms": 100 }
          ]
        }
      }
    }

One number stands for both axes. For `frames`, one number means one row.

## Tile sizes

A library sheet and your tilesheet may use different tile sizes. A copy is pixel for
pixel: the block lands with its top-left on the target cell and is padded
with transparent pixels to whole cells. Provenance and animations are pixel
records, so the tile size does not touch them. Changing the tile size of a
sheet only changes the grid you see and the cells you can select.

## Provenance tracking

A tilesheet is a `name.png` with its entry in `tilepicky.json`. The entry
remembers where every pixel came from. A block you copy from a library sheet
carries the path of that sheet. A block you copy from another tilesheet keeps
the origin it had. Hover over a cell, and the header names its origin. In the
entry, `provenance` lists per source sheet the pixel rectangles `[x, y, w, h]`
of your tilesheet that hold pixels from it.

The search box works on your tilesheets too, with the same rules.

## Licence

Tilepicky is free software under the GNU General Public License, version 3. The
whole text is in `LICENSE`.
