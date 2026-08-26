# Tilepicky

You have a folder of sprite packs, and a map that needs a fence, three trees
and a house. Tilepicky is a small desktop tool for getting those out of the
packs and into a tilesheet of your own.

<https://github.com/spookysys/tilepicky>

![A tilesheet of your own is built from two packs, found through the search box](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/demo.gif)

That is the whole loop: open a sheet of your own, search the packs for what
the map needs, select the tiles, hold the button until they lift, and carry
them over. The sheets in the pictures are from [Kenney](https://kenney.nl)
and from [ArMM1998](https://opengameart.org/content/zelda-like-tilesets-and-sprites),
both in the public domain (CC0).

## The library and the project

Two folders: the packs you collected, and the tilesheets you make. Tilepicky
reads each one with all its subfolders.

It never changes anything in your **library**. It only writes a
`tilepicky.json` there, which remembers the grid and the animations you set
on each sheet, so it can show them the same way next time. Your **project**
is where it writes tilesheets, with a `tilepicky.json` of its own beside
them.

Start the tool without folders and each panel will ask for one; click it, or
use the right-click menu of either tree, and pick a different one whenever
you like. Both paths are kept in `~/.config/tilepicky/settings.json`. You can
also name them on the command line:

    cargo run --release -- [<library dir> [<project dir>]]

The tool draws with wgpu. Add `--glow` to draw with OpenGL instead.

## Install

    cargo install --path .
    install -Dm644 tilepicky.desktop ~/.local/share/applications/tilepicky.desktop
    install -Dm644 icon.png ~/.local/share/icons/hicolor/128x128/apps/tilepicky.png

The desktop entry gives the window its name and icon in the dock. It runs
`tilepicky` from your PATH, where `cargo install` puts it, in
`~/.cargo/bin`. If your desktop session does not look there, write the whole
path in the `Exec=` line.

## Layout

Files on the left, sheets on the right: **Source** above is the pack sheet
you opened, **Canvas** below is the tilesheet you are building.

![The left column with both trees, the source sheet above, and the tilesheet being built below](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/screenshot.png)

Each panel has a header line with its grid fields, its zoom, what you have
selected, the name of the sheet, and the tile under the pointer. The buttons
at the right end open the side panels.

Your tilesheet has an eye, `E`. Switch it on when you want to ask questions
rather than make changes: hover over a tile and a tooltip names the pack its
pixels came from, and every pixel from that same pack lights up with it.
Hover beside the sheet and it tells you about the sheet as a whole. Nothing
selects or edits while the eye is on, and it starts off.

## From the keyboard

You can do the whole job without the mouse.

`Tab` goes to the next panel, `Ctrl+Tab` jumps between the library and your
own tilesheets. Inside a panel the arrows do the work: in a sheet they move
the selection, and `Shift` makes it bigger; in a file tree they move a cursor,
where `Enter` opens the file and Right and Left open and close a folder. Then
`Ctrl+C` there and `Ctrl+V` here.

The panel your keys are in has a blue title, and so does its selection.

![Picking a whole house, and then a column of trees, out of a pack and into a tilesheet of your own, without touching the mouse](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/keyboard.gif)

The legend at the foot of the window lists the keys worth knowing. Hide it in
the settings when you no longer need it.

## Animations

Mark a strip of tiles as an animation and the sheet remembers it: which
tiles, how big one frame is, and how fast it plays.

Select the tiles and press `A`. The panel opens on the right and plays them
straight away. The `cell` field says how many tiles make one frame, `1x1` for
a row of single tiles or `2x2` for something drawn two tiles across, and `ms`
is how long each frame is on screen. Tiles that no whole frame reaches turn
grey and are left out.

Press `M`, or the Store button, to keep it. The same key on a stored one
removes it, and `A` closes the panel.

![Two blocks of water tiles become animations: paste, A, set the frames, Store](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/animation-panel.gif)

A stored animation travels with the tiles when you copy or drag them, and
selecting it again brings its numbers back into the fields. It is remembered
in pixels, so changing the sheet's tile size leaves it alone; if the new
tiles no longer divide its frames, the `cell` field says their size in pixels
instead.

An animated GIF plays in the library panel. When you copy a region that moves
between the frames, the frames unroll into one strip, marked as an
animation. A region that stands still gives one picture.

![A waterfall is taken out of an animated GIF and lands as a marked strip](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/animation.gif)

The scene in that picture is a mockup from the [Epic RPG World](https://rafaelmatos.itch.io/epic-rpg-world-collection)
packs by RafaelMatos, from a purchased copy.

## Formats

Tilepicky reads PNG, GIF, JPEG, WebP, BMP and TGA. It writes one format:
32 bit RGBA PNG with straight alpha.

## The grid

Every sheet is read through a grid, and each sheet keeps its own.

| Field | Meaning |
| --- | --- |
| tile | the size of one tile, `32` or `32x48` |
| gap | pixels between neighbouring tiles, `1` or `1x2` (Kenney sheets use 1) |
| offset | pixels before the first tile, `4` or `4x8`; `-3` when the first tile starts before the edge |

All three fields answer the same three gestures. Drag one sideways for the
width, turn the wheel over it for the height, or click it and type. A field
showing a single `32` means both, and changing the height alone makes it
`32x48`.

A new sheet starts at the tile size that library or project used last, or at
32 px.

## Keys

| Key | Effect |
| --- | --- |
| click | select one tile |
| drag | select a range of tiles; near the edge of the view it scrolls |
| press and hold ~250 ms | lift the tile under the pointer, or the whole selection, and drag it |
| double click and drag | lift at once, without the wait |
| drag an edge of the selection | move that edge; outwards adds tiles, inwards removes them |
| shift+click | select the rectangle from the last clicked tile to this one |
| Ctrl+click | add or remove one tile |
| Ctrl+shift+click | add that rectangle to the selection |
| Ctrl+A | select the whole sheet |
| right click | clear the selection; inside the selection it clears the tiles |
| arrows | step the selection out of itself on the side you press |
| Shift+arrows | hold one corner and walk the other |
| Ctrl+arrows | jump to the end of the filled tiles, or across a gap to the next of them |
| Alt+arrows | walk the whole selection, shape and all; the tiles stay put |
| Tab, Shift+Tab | the next panel, or the one before |
| Ctrl+Tab | the other half of the window, on the same kind of panel |
| Ctrl+C, Ctrl+X, Ctrl+V | copy, cut, paste; cut and paste work on your tilesheet only |
| Delete | clear the selected tiles of your tilesheet |
| Enter or Space in a file tree | open the file under the cursor, or unfold the folder |
| Right, Left in a file tree | unfold and fold the folder you stand on |
| A | open or close the animation panel |
| M | store the animation under the selection, or unmark a stored one |
| E | switch the eye on or off |
| Ctrl+F | jump to the search box |
| Ctrl+Z, Ctrl+Y | undo, and take the step again |
| Ctrl+S, Ctrl+Shift+S | save, save as |
| Ctrl+T | trim empty columns on the right and empty rows at the bottom |
| drag the right or bottom edge of the canvas | resize your tilesheet |
| Ctrl+wheel, `+` / `-` | zoom |
| Escape | clear the selection, or cancel a drag |

Undo keeps the last 64 steps of a sheet, and a step is more than a change of
pixels: the tile size, the gap, the offset and every animation you store or
change are all on the same list. A whole drag over the tile field counts as
one step, so undo takes you back to where the drag began. The library sheet
has a list of its own, since its grid and its animations are yours to change
even though its pixels are not.

While you drag a block, two keys change what the drop does. Ctrl copies, and
leaves the tiles it came from where they are. Alt swaps: whatever lies where
the block lands goes back to the place the block came from. A sign on the
block says which is in force. A block from the library can only be copied,
because the library never changes.

Drop a block on an empty tilesheet panel and it starts a new tilesheet, at
the tile size of the block, and asks for a name when you first save it.

## Settings

The gear at the right end of the status line, or `Ctrl+,`, opens the
settings. There is one so far: whether to show the legend of keys in the
corner. Clicking the legend hides it too, after asking. Settings go to
`~/.config/tilepicky/settings.json`.

## Files

Both trees answer the same actions, but only the project tree changes
anything: the library is read and never written.

| Action | Effect |
| --- | --- |
| click | open the file |
| arrows | move the cursor; nothing opens until you press Enter |
| Shift+up, Shift+down | grow the marked group in the project |
| Ctrl+click, shift+click | mark one file, or a range |
| drag across the files | mark every file the pointer crosses |
| press and hold ~250 ms, then drag | carry the file, or the marked group, into a folder |
| right click a file | rename, duplicate, delete; open its location; copy its path |
| right click a folder | new folder, rename, delete; open its location; copy its path |
| right click the free space | new folder, refresh |

A carried file moves into the folder under the pointer; hold Ctrl to copy it
instead. Its entry in the book follows it, and a tilesheet you have open
keeps its identity when its file moves. A name the target folder already
holds is refused, and the other files still move.

"Open location" asks your file manager to show the file. "Copy relative
path" gives the path as you would type it from where tilepicky runs; "Copy
absolute path" gives the whole path, with your home directory as `~`.

## Search

Type words in the box, and the trees show only the files whose path holds
every one of them. A word matches from the start: `gra` finds `grass`. The
☰ button beside the box chooses whether to match folder names, file names,
or both. It searches your own tilesheets by the same rules.

## tilepicky.json

The library and the project each keep one `tilepicky.json` in their top
folder. It is the book of that tree: for every sheet, the grid it is read
through, where its pixels came from, and its animations. Sheets are keyed by
their path from the top folder, and `tile` at the head of the file is the
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

A pack and your tilesheet need not agree on tile size. A copy is pixel for
pixel: the block lands with its top left corner on the tile you chose, and
transparent pixels pad it out to whole tiles. Changing a sheet's tile size
changes only the grid you see and the tiles you can pick; it moves no pixels,
and leaves provenance and animations alone, because both are kept in pixels.

## Provenance tracking

Your tilesheet remembers where each of its pixels came from. Copy a block
out of a pack and it carries the name of that pack; copy it on from one
tilesheet to another and the original name goes with it. Switch on the eye
and hover, and a tooltip tells you:

    kenney_tiny-town/Tilemap/tilemap_packed.png

Six months later, when you want three more tiles in that style, you can ask
the sheet where it got them. In the book, `provenance` lists for each source
the pixel rectangles `[x, y, w, h]` of your sheet that came from it.

## Licence

Tilepicky is free software under the GNU General Public License, version 3.
The whole text is in `LICENSE`.
