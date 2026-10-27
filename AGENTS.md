# Tilepicky

Visibility: publishable

A desktop tool in Rust on egui/eframe: browse sprite sheets, search them,
and copy tiles into tilesheets of your own. `README.md` is the document for
the person who uses the tool; this file is for an agent that works on it.
Each file under `src/` opens with a comment that says what it owns.

## Build, run, test

- `cargo run --release -- [<library dir> [<project dir>]]` starts the tool.
  `README.md` says which build draws with what.
- `cargo test` runs the unit tests. `the_real_packs_read_as_they_did`
  checks `src/detect.rs` against `tools/grid-cases.json` and skips every case
  whose file is not on this machine. A case marked `pass` must keep passing;
  turning a `fail` into a `pass` is the work.
- To grow that suite, set the grid of a sheet in the app, which writes it
  into the folder's book (`tilepicky.json`), and then fold the books into the
  cases:

      TILEPICKY_SYNC_CASES=assets,mytilesheets cargo test sync_the_cases -- --nocapture

  It keeps what the old file said each sheet is, sets `expect` to what the
  reader does today, and prints how many read right. A sheet that is not a
  grid at all, a mockup or a title screen, needs its `kind` set by hand:
  the app has no way to say so.
- `cargo clippy --all-targets` runs clean.
- `server/` is the PHP bucket that hosts the images of an OpenRouter batch.
  Cargo does not build it; `server/README.md` says how to run it, how to test
  it, and what the API is. `src/bucket.rs` is the client for it.
- The tree is not rustfmt-clean. Do not run `cargo fmt` on a whole file.
  Keep a line at 160 columns or less, as `rustfmt.toml` says.

## Folders that stay local

`assets/` holds a sample library. `mytiles/`, `mytilemaps/` and
`mytilesheets/` are working folders; `mytiles/` is the scratch one, and its
names show in every screenshot. `tools/` holds scripts and stays ignored,
except `tools/grid-cases.json`, which the grid test reads. `private/` and
`worktrees/` come from the `git-repo-layout` skill.

## Media

`media/*.gif` are the README pictures, up to 1000 px wide. `docs/screencast.md`
says how to record them.
