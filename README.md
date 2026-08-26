# Tilepicky

A small desktop tool to browse a large set of sprite sheets, search them, and
copy tiles into tilesheets of your own. Each sheet has its own grid.

<https://github.com/spookysys/tilepicky>

![A tilesheet of your own is built from two packs, found through the search box](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/demo.gif)

That is the loop: open a tilesheet of your own, search the packs for what the
map needs, select the tiles, hold the button until they lift, and carry them
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

The tool draws with wgpu. Add `--glow` to draw with OpenGL instead.

## Install

    cargo install --path .
    install -Dm644 tilepicky.desktop ~/.local/share/applications/tilepicky.desktop
    install -Dm644 icon.png ~/.local/share/icons/hicolor/128x128/apps/tilepicky.png

The desktop entry runs `tilepicky` from your PATH, which `cargo install` puts
in `~/.cargo/bin`. If your desktop session does not see that directory, write
the whole path in the `Exec=` line. The entry gives the window its icon and
its name in the dock.

## Layout

The left column holds the search box, the tree of your library, and the tree
of your project. Beside the search box, ☰ picks what the search matches on.
The top panel, **Source**, shows the library sheet you opened. The bottom
panel, **Canvas**, shows the tilesheet you are building.

![The left column with both trees, the source sheet above, and the tilesheet being built below](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/screenshot.png)

Each panel has a header line. It holds the grid fields, the zoom, the
selection, the name of the sheet, and the tile under the pointer. At its
right end sit the buttons that open the side panels. In your tilesheet,
switch on the eye and hover over a tile, and a tooltip names the sheet its
pixels came from:

    kenney_tiny-town/Tilemap/tilemap_packed.png

See "Provenance tracking" for how the tool remembers that.

Your tilesheet also has an eye, `E`. With the eye on, the panel is for
looking: hover over a place, and a tooltip says what it is; nothing there
selects, drags, or edits until the eye is off again. The pixels that came
from one sheet light up together under the pointer, and the tooltip names
that sheet. The free area beside the sheet lights up the whole view and
tells about the whole sheet: its name, its size in pixels and tiles, its
file format and color type, and its animations. The eye is off at each
start.

Select tiles and press `A`, or click 🎬 in the header, and the
animation panel opens on the right. See "Animations".

## From the keyboard

The whole loop works without the mouse. The window is seven panes, and Tab
walks them in the order you read them: the library tree, the source sheet and
its animation panel, then the project tree, the canvas and its animation
panel, and the status bar at the foot. Shift+Tab walks back, and Ctrl+Tab
crosses between the library half and the project half without leaving the
pane you are in, so a sheet meets a sheet.

The arrows work inside the pane that holds the keys, and never leave it. In a
sheet they move the selection, and Shift extends it. In a file tree they move
a cursor over the folders and files, opening nothing until you press Enter;
Right and Left unfold and fold a folder. The title of the pane that holds the
keys is deep blue, and so is the selection of its sheet, so a glance says
where you are.

![A whole house, then a column of trees, bushes and ground, are picked out of a packed pack into a tilesheet of your own, without the mouse](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/keyboard.gif)

The legend along the foot of the window holds the keys worth knowing. The
settings, behind the gear, hide it once you no longer need it.

## Animations

An animation is a block of frames on the bitmap: a place in pixels, a frame
size, the frames in a row and the number of rows, and the time per frame. The
frames play left to right, then the next row down.

Select the tiles that hold the frames and press `A`. The animation panel
opens on the right and plays them. The cell field gives the size of one cell
of the animation, in tiles. It works like the tile field: drag it for the
width, turn the mouse wheel for the height, click it to type.

Whole frames fill the selection from its top left corner. Tiles that no whole
frame reaches turn grey, and the animation leaves them out. While the panel
is open, a new selection grows in whole cells. The cell field itself never
moves the selection. Press `M`, or the Store button, to store the animation;
on a stored one, the same key, or the Unmark button, removes it. Press `A`
again to close the panel.

![Two blocks of water tiles become animations: paste, A, set the frames, Store](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/animation-panel.gif)

A stored animation travels with the block when you copy or drag it. When the
selection lies on a stored animation, the fields edit that one. An animation
lives in pixels, so changing the sheet's tile size leaves it alone. Its cell
size is then read back in whatever tiles the sheet wears; when they do not
divide its frames, the field says the frame size in pixels instead, because
a rounded number of tiles would not be true of it.

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
| tile | the size of one tile, `32` or `32x48` |
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
| click | select one tile |
| drag | select a range of tiles; near the edge of the view it scrolls |
| press and hold ~250 ms | lift the tile under the pointer, or the whole selection, and drag it |
| double click and drag | lift at once, without the wait |
| drag an edge of the selection | move that edge; out of the selection adds tiles, into it removes them |
| shift+click | select the rectangle from the last clicked tile to this one |
| Ctrl+click | add or remove one tile |
| Ctrl+shift+click | add that rectangle to the selection |
| Ctrl+A | select the whole sheet |
| arrows | leave the selection on the side you press, and take one tile there |
| Shift+arrows | hold one corner of the selection and walk the other |
| Ctrl+arrows | step to the end of the filled tiles, or over a gap to the next of them |
| Alt+arrows | walk the whole selection, shape and all; the tiles stay where they are |
| Tab | the next pane, to the right and then down |
| Shift+Tab | the pane before it |
| Ctrl+Tab | the other half of the window, LIBRARY or PROJECT, on the same pane |
| right click | clear the selection; inside the selection it clears the tiles |
| Ctrl+C | copy the selection of the active panel |
| Ctrl+X | cut: copy, then clear the tiles (your tilesheet only) |
| Ctrl+V | paste at the selected tile of your tilesheet |
| Delete | clear the selection in your tilesheet |
| arrows at the edge of a sheet | one more press hands the keys to what lies that way |
| arrows on a sheet title | Right and Left walk the fields and buttons; Down goes back to the grid |
| arrows in a file tree | move the cursor over the folders and files; nothing opens |
| Enter or Space in a file tree | open the file under the cursor, or unfold the folder |
| Right / Left in a file tree | open or close the folder you stand on |
| A | open or close the animation panel |
| M | store the animation under the selection, or unmark a stored one |
| E | switch the eye of your tilesheet on or off |
| Ctrl+, | open the settings |
| Ctrl+Z | undo, in the sheet you are in |
| Ctrl+Shift+Z, Ctrl+Y | take that step again |
| Ctrl+S | save |
| Ctrl+Shift+S | save as |
| Ctrl+T | trim empty columns on the right and empty rows at the bottom |

Undo holds the last 64 steps of a sheet, and a step is more than a change of
pixels: the tile size, the gap, the offset, and every animation you store,
unmark or change go on the same list. A run of grid changes counts as one
step, so a drag over the tile field takes you back to where the drag began.
The library sheet has its own list, since its grid and its animations are
yours to change even though its pixels are not.
| drag the right or bottom edge of the canvas | resize your tilesheet |
| Ctrl+wheel, + / - | zoom |
| Escape | clear the selection, or cancel a drag |

The window holds up to seven panes: two file trees, two sheets, the two
animation panels while they are open, and the status bar. Tab walks them to
the right and then down, and wraps: library tree, source sheet, source
animation, project tree, project sheet, project animation, the status bar,
and around again. Shift+Tab walks
them the other way. Ctrl+Tab swaps the two halves of the window and stays on
the same pane, so a tree meets a tree and a sheet meets a sheet. A pane that
is not open is stepped over. The title of the pane that holds the keys is
deep blue; the others are grey, and a sheet's selection follows its title:
blue while the keys are on that grid, grey while they are elsewhere. A click puts the keys where you clicked.

Tab gives a pane back as you left it, and leaves you on its title the first
time you go there. Ctrl+Tab lands where the work is: the grid of a sheet, the
rows of a tree.

Inside a pane the arrows move you to the nearest place that way on screen,
and they never leave the pane. Only Tab does that. In a sheet the arrows
belong to the selection; on the top row one more press upwards reaches the
title, and from the title, or from any field beside it, Down goes back to the
grid.

In a file tree the arrows move a cursor, shown as a pale band. The file on
show wears the solid colour, and the two are not the same thing: walking the
tree changes nothing in the sheet panes. Press Enter, or Space, to open the
file the cursor stands on, or to unfold the folder it stands on. A click opens a file
as it always did. Hold Shift down for a whole run of arrows: the corner
that walks is remembered while you hold it, so the selection grows and
shrinks. Let Shift go, and the next run grows again on the side you press.
With the animation panel open, every step above is one cell, not one tile.

While you drag a block, two keys change what the drop does. Ctrl copies: the
tiles it came from stay as they are. Alt exchanges the two places: what lies
where the block lands travels back to the place the block came from. A sign
in the corner of the block shows which one is in force. A block from the
library panel can only be copied, because the library never changes.

A drop on an empty tilesheet panel starts a new tilesheet. It takes the tile size
of the block you dropped, and it asks for a name at the first save.

Your tilesheet is written when you save it. Undo stays available while the tool
runs.

## Settings

The gear at the right end of the status line, or `Ctrl+,`, opens the
settings. A checkbox shows or hides the legend of keys in the lower left
corner; a click on the legend hides it too, after a question. Settings go
to `~/.config/tilepicky/settings.json`.

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
| right click a folder | new folder, rename, delete; open its location; copy its path |
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
folder name. The ☰ button beside the box picks what the search matches on:
folder names, file names, or both.

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
pixel: the block lands with its top-left on the target tile and is padded
with transparent pixels to whole tiles. Provenance and animations are pixel
records, so the tile size does not touch them. Changing the tile size of a
sheet only changes the grid you see and the tiles you can select.

## Provenance tracking

A tilesheet is a `name.png` with its entry in `tilepicky.json`. The entry
remembers where every pixel came from. A block you copy from a library sheet
carries the path of that sheet. A block you copy from another tilesheet keeps
the origin it had. With the eye on, hover over a tile, and a tooltip names
its origin. In the entry, `provenance` lists per source sheet the pixel
rectangles `[x, y, w, h]` of your tilesheet that hold pixels from it.

The search box works on your tilesheets too, with the same rules.

## Licence

Tilepicky is free software under the GNU General Public License, version 3. The
whole text is in `LICENSE`.
