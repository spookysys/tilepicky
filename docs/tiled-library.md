# Tiled tilesets in the library

Status: phase 1 is implemented on 2026-10-28. That is the reader, the
pairing, the grid precedence, the three filters, and the copy fix. The
panel, the cross-links, and the project rules are proposals; each section
below says which one it is.

The library shows the Tiled tilesets of a pack beside the images they use.
A person can find a tileset, read its data, and copy it to a project.
A pack without Tiled files looks exactly as it looks today.

## The problem

A Tiled tileset is a second description of an image: a grid, and data for
each cell. The pack ships the image and the tileset as two files. A library
that lists files shows both, and the person must guess how they relate.

The relation has two shapes, and they point in opposite directions:

| Shape | Files | What the image is |
| --- | --- | --- |
| Atlas | one image, many tiles | the backing store of the tileset |
| Image collection | one tileset, many images | the images are the content |

An atlas image is raw material. A person does not browse it, because the
tileset says what is in it. A collection member is a finished sprite. A
person browses it as it is, because it is already one sheet.

A single rule cannot serve both. "Hide every image that a tileset names"
hides 6,118 useful sprites in the ERW pack. "Show every image" puts 125 raw
atlases back in the view that the tileset card was meant to replace.

## The model

The library lists **entries**, not files.

An entry is one image. Its key stays the image path, because the book,
labels, embeddings, provenance, and `files::relocate` all hang off that path.

    pub struct Entry {
        pub rel: String,             // the image, or the tileset file when it stands alone
        pub tiled: Option<Tiled>,    // the tileset that names this image
        pub in_tileset: bool,        // an atlas image, or a collection sprite
        pub tileset_only: bool,      // a collection tileset, with no image of its own
        pub tiled_error: Option<String>,
        pub tiled_note: Option<String>,
        // ...
    }

    pub struct Tiled {
        pub file: String,            // the .tsx or .tsj
        pub format: Format,          // Tsx or Tsj
        pub name: String,
        pub version: String,
        pub tile: [u32; 2],
        pub margin: u32,             // Tiled has one margin for both axes
        pub spacing: u32,            // and one spacing
        pub columns: u32,            // 0 when the file leaves it to the image size
        pub tilecount: u32,          // 0 when the file does not state it
        pub shape: Shape,            // Atlas { image, size } or Collection { images }
        pub counts: Counts,          // animated tiles, wang sets, collision shapes
    }

The tileset changes how the card draws. It does not create a second entry.
A library with a `sky.png` and a `sky.tsj` shows one card, not two.

A collection tileset has no image to stand on. It gets an entry of its own,
keyed by the tileset path, with `tileset_only: true`. An atlas tileset has an
image, so it rides on that image and gets no entry of its own.

`Entry::sheet_rel() -> Option<&str>` returns the image, or `None` for a
tileset-only entry. The gallery, labels, embeddings, quantization, and
`Sheet::open` take that value. Then the compiler refuses a pixel operation
on an entry that has no pixels.

### The buckets

A card answers to one or two of the three buckets, and it shows once:

| Bucket | Rule | ERW |
| --- | --- | --- |
| Tilesets | a `.tsx` or `.tsj` | 141 cards |
| Images in tilesets | an image that a tileset names | 6,243 |
| Images not in tilesets | every other image | 8,674 |

An atlas entry answers to the first bucket and the second. When both boxes
are on, the atlas draws as the tileset card. The image is still one entry,
and the cross-links below reach it. This avoids two rows for one file in one
folder.

An image may be named by more than one tileset. ERW has three such images,
each with two tilesets of the same grid and different data. The first by
name rides on the image card. Each later one gets a card of its own, with a
note that another tileset stands for that image, so neither file is
invisible.

## The filters

Three checkboxes in the library toolbar, under the heading. They appear
only when the folder has a tileset:

    [x] Tilesets (141)
    [ ] Images in tilesets (6,243)
    [x] Images not in tilesets (8,674)

Each count is taken before its own filter applies. The default is the two
boxes above. It hides the 6,118 collection sprites, and the person asked
for exactly that.

The boxes are independent. A dropdown of preset views cannot express "all
images, no tileset cards", which is the view for cropping a raw atlas.

| Tilesets | Images in tilesets | You see |
| --- | --- | --- |
| on | off | 141 tileset cards and 8,674 loose images |
| on | on | the same cards, and the 6,118 collection sprites |
| off | on | 125 atlas images and 6,118 collection sprites, as images |
| off | off | the 8,674 loose images |

`Index::visible` builds the tree mask and `Entry::shows` answers for one
card in the gallery. Both read the same view, so the tree and the gallery
agree. The gallery does not filter its order on its own.

`Index::effective_view` returns the view that a folder can offer. A folder
without a tileset gets `View::everything()`, so a filter that a person set
for one pack cannot empty the next one. The library toolbar hides the three
boxes in that folder, so the person sees no control that does nothing.

`Without tags` in the current library trims the gallery order and leaves the
tree alone. A second filter on that path would double the disagreement. The
view filters go into the visibility mask, and the tags filter should follow.

The settings persist in `Settings` with `#[serde(default)]`, next to
`hide_legend`. A filter that a person must set on every launch is a filter
that they stop using.

## The cross-links

Status: phase 2. None of these buttons exist yet.

The hidden images stay one click away. This is what makes hiding safe.

- The image card has a small button `Tileset` that opens the tileset card.
- The tileset card has a button `Image` that opens the atlas image.
- A collection card lists its tiles in the panel. Each row links to that
  sprite.
- The collection card has one button `Show these N sprites` that turns on
  `Images in tilesets`.

A link may open an entry that the filter hides. The filter stays as the
person left it, and the panel says where the person came from.

Double-click keeps its current meaning, which is to open the card in the
canvas. A cross-link is a button, because a gesture must be learned.

## The panel

Status: phase 2. The library inspector shows a read-only summary already:
the file, its summary line, the grid, and any note. The tab and the blocks
below are the plan.

One component, used twice: `Build` edits, `Library` inspects. One code path,
and an `editable` flag. The library panel writes nothing.

The library inspector gains a tab: `Sheet | Tiled`. The tab appears only
when the entry has a tileset. Two right panels would press on the gallery.

The blocks, in this order, and the reason for the order:

1. **Identity**: file, format, atlas or collection, version. A person must
   know which file they read before they trust it.
2. **Grid**: tile size, margin, spacing, columns, tile count, image size.
   Then one line: `agrees with this sheet`, or `differs`. With a button
   `Use the tileset grid`.
3. **Data counts**: animations, collision shapes, wangsets, properties, tile
   classes. With a toggle that marks those cells on the sheet.
4. **The selected tile**: its properties, its animation, its collision.
   Read-only, with one button to copy the sheet to the project.
5. **Tileset-level fields**, collapsed: `properties`, `wangsets`,
   `terrains`, `transformations`, `editorsettings`.
6. **Health**: the checks below.
7. **Maps that use it**, on demand: path, `firstgid`, layer names.

The grid is the highest-value block. The tileset is a number that a person
wrote. Pixel detection is a guess. So the precedence is:

1. A grid that the person set. `Sidecar.read == false`.
2. The grid of the tileset.
3. Pixel detection.

The book is not written when the tileset supplies the grid. A tileset that
later changes must change the sheet.

ERW has no tile properties at all. Its value is wang sets, collision, and
animation. The panel therefore leads with those three, not with properties.
A panel that copies the `Build` order would put the empty block first.

## Reading both formats

Read `.tsj` and `.tsx`. Write `.tsj` only.

`src/tiled.rs` reads both formats into one `Tiled`. `src/tileset.rs` speaks
JSON only, because a project writes JSON and keeps unknown fields as
`serde_json::Value`. An XML writer cannot make that promise in the same
way, so the library reads a TSX and never writes one.

The XML reader uses `quick-xml`: small, no unsafe, and it keeps the source
position for error messages. A hand parser for TSX is possible and fragile,
because the pack uses attributes, nested `tile` elements, and entities in
names.

A `.tsj` that a copy carries keeps every field, because the copy moves the
bytes and rewrites only `image`. A `.tsx` stays in the library, and the
status line says so. Converting one is a separate action, with a backup.
Two files with two grids for one image is the failure to avoid.

### The pairing

Pair by the `image` path inside the tileset, resolved against the folder of
the tileset file. Do not pair by file name, and do not pair by folder.

ERW has no other rule that works. `erw_cemetery/TiledMap Editor/Tilesets/`
holds `Terrain - cemetery.tsx`, and its image is
`../../Tilesets/Tileset-Terrain.png`. The names differ, the folders differ,
and the tileset folder has three spellings across the pack.

When the resolved path does not exist, retry with a case-insensitive match
on every path element. Then report the mismatch. ERW ships one such
reference: `Props/Animated/FOG-1.png` against a folder named `animated`.
It works on Windows, where the pack was made, and fails on Linux.

## Health checks

Phase 1 reports the first and the seventh, as a note on the card. A file
that cannot be read at all becomes the card's error line. The rest are
phase 2.

Tiled reports the checks only when a person opens the file, and the library
is where a person wants to find them first.

- The image of an atlas does not exist, or the case differs.
- `imagewidth` or `imageheight` disagrees with the file.
- `columns` disagrees with image width, margin, spacing, and tile size.
- `tilecount` disagrees with `columns` times rows.
- A tile id is past `tilecount`.
- An animation frame names a tile id that does not exist.
- Two tilesets name one image.
- A `.tsx` and a `.tsj` beside one image give different grids.

A broken tileset keeps its card and its image. The card shows the error,
and the sheet still opens. The library must not drop an entry because a
sidecar is wrong.

## What this does not do

- Edit maps. The panel lists the maps that use a tileset, and no more.
- Edit library tilesets in place. Inspect and copy, never fork a pack.
- Evaluate wang or terrain rules. Show the data, do not paint with it.
- Convert a format during a copy, or write a second tileset beside one.
- Show embedded tilesets of a map as cards. That is a map object with a
  `firstgid`, not a sheet.

## Phases

1. Done: the TSX reader, the pairing, the grid precedence, the card, the
   three filters, and the copy fix.
2. The panel and the cross-links: identity, grid agreement, data counts,
   selected tile, health, and the buttons that jump between an image and
   its tileset.
3. The rules for a project that has a `.tsx`: convert on request, or report
   why not. The library reads both formats already.
4. Search words (`animated`, `collision`, `wang`, `collection`, class names,
   property names), the overlay marks, and the map list.
5. Collection cards: tile list, link to each sprite, preview contact sheet.

### What a copy does

`files::import_sheet` copies the image bytes, the book entry, and the grid
that the tileset states. A `.tsj` travels beside the copied image, under
the copied image's name, and is retargeted to it. Properties, animations,
and collision reach the project. A `.tsx` stays in the library, and the
status line says so: this tool does not convert yet.

A name that a tileset already holds is skipped, so a copy never overwrites
a tileset. The image, the tileset, and the book entry are chosen together
under one name, and a failure removes what the copy wrote.

The card and the copy landed together. A card that says "this sheet has
data" next to a button that drops the data is worse than no card.

## Evidence from the ERW pack

`assets/erw` holds 12 zone folders. The scan below is from 2026-10-28.

| Item | Count |
| --- | --- |
| `.tsx` files | 141 |
| `.tsj` files | 0 |
| Atlas tilesets | 128 |
| Collection tilesets | 13 |
| Images named by a tileset | 6,243 |
| Images named by two tilesets | 3 |
| Images in no tileset | 8,674 |
| `.tmx` maps | 173 |

| Tileset data | Files | Occurrences |
| --- | --- | --- |
| `wangset` | 91 | 283 sets |
| `objectgroup` collision | 22 | 1,854 shapes |
| `animation` | 44 | 1,374 frames |
| `properties` | 0 | 0 |
| `terrain`, `tileoffset`, `transformations` | 0 | 0 |

The three images with two tilesets are `depths Platform3`,
`old prison blood pool`, and `volcano lava passage`. Each pair has the same
grid and different data.

## Alternatives and why they lost

- **A second tree node for each tileset file.** Two rows for one sheet, and
  a person must learn which row edits what. The card model has no duplicate
  to learn.
- **Preset views in a dropdown.** It cannot reach the state "all images, no
  tileset cards", which is the view for cropping an atlas.
- **Hide every image that a tileset names.** It hides 6,118 sprites, and 13
  cards then stand in for a browseable catalogue.
- **Key the entry on the tileset path.** Labels, embeddings, provenance, and
  `relocate` key on the image path. A tileset key would split the identity
  of one sheet. Renaming the image would need new resolution rules.
