# Tilepicky

A desktop tool in Rust on egui/eframe: browse sprite sheets, search them,
and copy tiles into tilesheets of your own. `README.md` is the document for
the person who uses the tool; this file is for an agent that works on it.

## Build, run, test

- `cargo run --release -- [<library dir> [<project dir>]]` starts the tool.
  `--glow` draws with OpenGL instead of wgpu.
- `cargo test` runs the unit tests. Most write their own files under the
  temp dir. One reads the real packs: `the_real_packs_read_as_they_did`
  checks `src/detect.rs` against `tools/grid-cases.json`, and skips every
  case whose file is not on this machine, so it passes where the packs are
  missing. A case marked `pass` must keep passing; turning a `fail` into a
  `pass` is the work.
- To grow that suite, set the grid of a sheet in the app, which writes it
  into the book of its folder, and then fold the books into the cases:

      TILEPICKY_SYNC_CASES=assets,mytilesheets cargo test sync_the_cases -- --nocapture

  It keeps what the old file said each sheet is, sets `expect` to what the
  reader does today, and prints how many read right. A sheet that is not a
  grid at all, a mockup or a title screen, needs its `kind` set by hand:
  the app has no way to say so.
- `cargo clippy --all-targets` reports 20 old "collapsible if" warnings.
  Do not add to them.
- The tree is not rustfmt-clean. Do not run `cargo fmt` on a whole file.
  Keep a line at 160 columns or less, as `rustfmt.toml` says.

## Where things are

- `src/main.rs`: the app: the panels, the headers, the keys, the dialogs and
  popups, and the drag of a block between the panels.
- `src/sheet.rs`: one sheet on screen: the grid, the selection, the copy
  and paste, the provenance map, the eye mode, and the animations.
- `src/sidecar.rs`: `tilepicky.json`, the book of a folder: the grid of
  each sheet, where its pixels came from, and its animations.
- `src/index.rs`: the scan of a folder, and the search.
- `src/detect.rs`: reads the tile size of a sheet that the book does not know.
- `src/tree.rs`: the file trees of the left column.
- `src/settings.rs`: `~/.config/tilepicky/settings.json`.
- `src/ai.rs`: the AI providers, models, and keys (`keys.json`, mode 0600),
  and their settings page. The AI features themselves are not built yet, and
  `AI_VISIBLE` and `LIBRARY_EYE_VISIBLE` in `src/main.rs` hide their UI for
  a release. The code behind both flags stays; flip a flag to bring it back.

## Folders that stay local

`assets/` holds a sample library, `mytiles/` and `mytilesheets/` hold the
user's tilesheets, and `tools/` holds scripts. All four are gitignored, and
so are `private/` and `worktrees/`; see the `git-repo-layout` skill.

## How to write here

Comments, the README, and commit messages follow the global `AGENTS.md`:
short sentences, the active voice, one word for one meaning. Text that a
file already holds keeps its voice. Name the files in every commit.

## Recording a screencast for the README

The pictures in `README.md` are GIFs in `media/`, 1000 px wide. Record them on
a display of your own, never on the user's: your synthesized keys land on
whatever window has the focus, and the user may be working. A private display
also gives the same window size and the same result every time.

You need `Xvfb`, `openbox`, `ffmpeg` and `python-xlib`. The driver scripts
live in the session scratchpad under `ui/`, and the venv one level above
them, in the scratchpad itself.

```
Xvfb :77 -screen 0 1520x1030x24 -nolisten tcp &
DISPLAY=:77 openbox &
env -u WAYLAND_DISPLAY DISPLAY=:77 WINIT_X11_SCALE_FACTOR=1 ./target/release/tilepicky &
```

Four things that each cost an hour to find out:

- **Unset `WAYLAND_DISPLAY`.** winit 0.29 removed `WINIT_UNIX_BACKEND`, and
  it now picks Wayland whenever that variable is set, whatever `DISPLAY`
  says. The window opens on the user's screen instead of yours.
- **`WINIT_X11_SCALE_FACTOR=1`** makes the app draw one pixel per point. On a
  scaled display it draws at 1.8, and the text is unreadable once the capture
  is scaled down to 1000 px.
- **`ffmpeg -f x11grab` reads the root window.** On the user's Wayland
  session that gives a black rectangle, because XWayland windows are
  redirected. On Xvfb nothing composites, so it works.
- **openbox draws the title bar**, which carries the name and the version.
  Its frame has no name of its own: find it as the parent of the app window,
  not by searching for the title.

Frame it narrow. A window wider than its content leaves half the picture
empty, and the README shows the picture about 880 px wide, so everything in
it shrinks. Around 900 by 930 fits the two panels with little waste. Zoom a
sheet until it fills its panel, and show a different pack in each picture:
two captures of the same tiles are one capture.

Decide the take before you film it. Say what a reader should think when
they see it, and build a project that makes them think it: the tool picks
tiles out of a pack into a tilesheet of your own, so the film pulls a whole
house out of a packed sheet and plants two trees beside it. Paste replaces
pixels, it does not blend them, so a transparent sprite dropped on a filled
tile leaves a hole; a sheet that fills up reads as work, a map with holes in
it reads as a broken tilesheet. Caption the keys: a film of a keyboard with
no keys in it looks the same as a film of a mouse.

Drive the app with XTEST through python-xlib. Stop the instance you started
last: the driver takes the first window it finds that is named Tilepicky, so
a stray one takes every key while the screenshot shows the window on top of
it. Prepare the state off camera and record only the part worth watching. Do not count the rows of a tree: the
keys begin on the title of a pane, so one Down is spent entering it, and the
count is wrong by one in a way that opens the wrong file. Walk to the end,
where a tree holds the keys, and step back from there.

```
DISPLAY=:77 ffmpeg -y -f x11grab -draw_mouse 0 -framerate 15 \
  -video_size <w>x<h> -i :77.0+<x>,<y> -t 22 demo.mp4
ffmpeg -i demo.mp4 -vf "fps=10,scale=1000:-1:flags=lanczos,split[a][b];\
[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer:bayer_scale=3" \
  -loop 0 media/<name>.gif
```

Point the project folder at something tidy first. `mytilesheets/` holds a
small, well named set for this; `mytiles/` is a scratch folder and its names
show in every frame.
