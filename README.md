# Tilepicky

You have a folder of sprite packs, and a map that needs a fence, three trees
and a house. Tilepicky is a small desktop tool for getting those out of the
packs and into a tilesheet of your own.

<https://github.com/spookysys/tilepicky>

![A tilesheet of your own is built from two packs, found through the search box](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/demo.gif)

That is the whole loop: open a sheet of your own, search the packs for what
the map needs, select the tiles, hold the button until they lift, and carry
them over.

## The library and the project

Two folders: the packs you collected, and the tilesheets you make. Tilepicky
reads each one with all its subfolders.

It never changes anything in your **library**. It only writes a
`tilepicky.json` there, which remembers grids, animations, and AI labels
for each sheet, so it can show them the same way next time. Your **project**
is where it writes tilesheets, with a `tilepicky.json` of its own beside
them.

Start the tool without folders and each panel will ask for one; click it, or
use the right-click menu of either tree, and pick a different one whenever
you like. Both paths are kept in `~/.config/tilepicky/settings.json`. You can
also name them on the command line:

    tilepicky <library dir> <project dir>

It draws with OpenGL, which every machine has. The binaries on the releases
page can draw with wgpu as well, with `--wgpu`; a build of your own leaves
wgpu out, because it is half the compile time. Add it with
`cargo install tilepicky --features wgpu` if you want it.

## Install

[Download a release](https://github.com/spookysys/tilepicky/releases/latest)
for Linux, Windows, or macOS. Extract the archive first.
On Windows, open `tilepicky.exe`. On Linux or macOS, run `./tilepicky`
from the extracted folder. The Mac download supports Intel and Apple Silicon.

For Linux desktop integration, install the extracted files:

    install -Dm755 tilepicky ~/.local/bin/tilepicky
    install -Dm644 tilepicky.desktop ~/.local/share/applications/tilepicky.desktop
    install -Dm644 icon.png ~/.local/share/icons/hicolor/128x128/apps/tilepicky.png

The desktop entry runs `tilepicky` from your PATH. If your desktop cannot find
it, set `Exec=` in that entry to the binary's full path.

To build from a source checkout, use `cargo install --locked --path .`.

## Layout

Files on the left, sheets on the right: **Source** above is the pack sheet
you opened, **Canvas** below is the tilesheet you are building.

![The left column with both trees, the source sheet above, and the tilesheet being built below](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/screenshot.png)

Each panel has a header line with its grid fields, its zoom, what you have
selected, the name of the sheet, and the tile under the pointer. The buttons
at its right end open the side panels.

Your tilesheet has an eye, `E`. Switch it on when you want to ask questions
rather than make changes: hover over a tile and a tooltip names the pack its
pixels came from, and every pixel from that same pack lights up with it.
Hover beside the sheet and it tells you about the sheet as a whole. Nothing
selects or edits while the eye is on, and it starts off.

## Label one library sheet

Open a library sheet, then open **AI assist** with its header button or `I`.
In Settings, choose an instant model with image input and structured JSON
output. Use an OpenAI-compatible provider URL, such as
`https://openrouter.ai/api/v1`, and enter its key or set its key environment
variable.

Choose **Label with AI** in the AI panel, in the sheet's right-click menu, or
in its file's right-click menu. The tool sends the whole sheet to the model,
and the model returns a caption and up to 12 tags. Opening a panel or a menu
sends nothing. You can keep browsing while the request runs. One sheet is
labeled at a time, and the request stops after 60 seconds.

The AI panel shows the caption and the tags of the open sheet. The label goes
into the sheet's entry in `tilepicky.json`, with the provider, the model, and
a fingerprint of the pixels. When the pixels change, the panel says that the
label is stale. A grid change does not make a label stale.

Choose **Remove AI label...** to remove the caption and the tags. You confirm
the sheet first. The image and the grid stay.

A GIF is labeled by its first frame. An image larger than 2048 pixels on an
edge is made smaller for the model; the file does not change. Check the
labels: a model can misidentify pixel art or miss small details.

## Label an entire library

Open the library folder, open **AI assist** (`I`), and choose
**Label entire library with AI...**. You do not have to open a sheet first.
Choose the batch model and provider key in Settings. Library batches support
Google Gemini and OpenRouter; the model must accept images and structured
JSON output.

The tool reads every image in the library, subfolders included, and sends
nothing yet. Sheets that already have a current label are skipped. The
confirmation shows the number of sheets to label, the sheets skipped, the
files it could not read, the approximate request size, and the maximum
output tokens. These numbers describe the size of the job; they are not a
price quote. Nothing is sent until you choose **Start batch**.

Each sheet is one request, the same one that **Label with AI** sends. A large
library goes out in more than one provider batch. Progress and errors show in
AI assist, and each label goes into `tilepicky.json` when it arrives. The
batch IDs are kept under the configuration folder, so a batch continues when
you open the library again. Keys stay in key storage.

Run the action again to retry the sheets that failed. A sheet that changes
after you start the batch is not sent. If a submission is interrupted before
its ID arrives, the tool does not send it again: find the batch in the
provider's batch list and attach its ID in AI assist. While a library batch
runs, you cannot label or remove the label of a single sheet.

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

Open a sheet the tool has never seen and it works the tile size out from
the picture itself, by finding the pitch at which the picture repeats. A
picture that does not repeat, a title screen or a mockup, is read as one
whole tile. It does not find a gap or an offset yet, so set those yourself
for a pack that uses them.

Reading a grid means walking the whole sheet, so the answer goes into
`tilepicky.json` at once and is never worked out twice. It is marked there
as read rather than chosen, and setting the grid yourself clears the mark:
what you choose always wins, and the tool never mistakes its own guess for
your decision.

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
| right click | clear the selection; inside the selection it clears the tiles; on a library sheet it opens the AI label menu |
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
| E | switch the eye of your tilesheet on or off |
| I | open or close library AI assist |
| Ctrl+F | jump to the search box |
| Ctrl+Z, Ctrl+Y | undo, and take the step again |
| Ctrl+S, Ctrl+Shift+S | save, save as |
| Ctrl+T | trim empty columns on the right and empty rows at the bottom |
| drag the right or bottom edge of the canvas | resize your tilesheet |
| Ctrl+wheel, `+` / `-` | zoom |
| Escape | clear the selection, or cancel a drag |

Undo keeps the last 64 steps of a sheet, and a step is more than a change of
pixels: the tile size, the gap, the offset and every animation you store or
change are all on the same list. The library sheet has a list of its own,
since its grid and its animations are yours to change even though its pixels
are not.

While you drag a block, two keys change what the drop does. Ctrl copies, and
leaves the tiles it came from where they are. Alt swaps: whatever lies where
the block lands goes back to the place the block came from. A sign on the
block says which is in force. A block from the library can only be copied,
because the library never changes.

Drop a block on an empty tilesheet panel and it starts a new tilesheet, at
the tile size of the block, and asks for a name when you first save it.
`Ctrl+Tab` out of the source sheet does the same when the canvas is empty,
at the tile size of the sheet you come from.

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
instead. Its grid and animations go with it, and a tilesheet you have open
survives its own file moving.

## Search

Type words in the box. Each word matches a prefix: `gra` finds `grass`.
All words must match, and the tree shows the files that match. Open the menu
beside the box to choose what the words match: folder names, file names,
captions, and tags. The captions and the tags come from **Label with AI**.

Search runs on your machine and sends nothing to a model. A stale label
still matches, because the tool does not read every image to check it.

## tilepicky.json

Each folder's book remembers grids, animations, pixel origins, and AI labels.
The images stay ordinary image files. See the [file format](docs/tilepicky-json.md)
if you want to read or change the metadata yourself.

## Tile sizes

A pack and your tilesheet need not agree on tile size. A copy is pixel for
pixel: the block lands with its top left corner on the tile you chose, and
transparent pixels pad it out to whole tiles. Changing a sheet's tile size
moves no pixels at all; it changes the grid you see and the tiles you can
pick.

## Provenance tracking

Your tilesheet remembers where each of its pixels came from. Copy a block
out of a pack and it carries the name of that pack; copy it on from one
tilesheet to another and the original name goes with it. Switch on the eye
and hover, and a tooltip tells you:

    kenney_tiny-town/Tilemap/tilemap_packed.png

Six months later, when you want three more tiles in that style, you can ask
the sheet where it got them.

## Credits

The packs in the pictures are [Kenney](https://kenney.nl)'s and
[ArMM1998](https://opengameart.org/content/zelda-like-tilesets-and-sprites)'s,
both public domain (CC0). The animated scene is a mockup from the
[Epic RPG World](https://rafaelmatos.itch.io/epic-rpg-world-collection) packs
by RafaelMatos, from a purchased copy.

## Licence

Tilepicky is free software under the GNU General Public License, version 3.
The whole text is in `LICENSE`.
