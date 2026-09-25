# Tilepicky

Tilepicky is a desktop tool to extract tiles from sprite sheets and assemble them into custom tilesets. You browse your sprite library, select tiles, and place them on an editable canvas.

<https://github.com/spookysys/tilepicky>

![A tilesheet of your own is built from two packs, found through the search box](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/demo.gif)

Open a project sheet, search the library for the tiles you need, select a region, and drag or copy the tiles onto the canvas.

## Library and project folders

Tilepicky organizes your work into two directories:

- **Library**: Holds your collected sprite sheets. Tilepicky treats this directory as read-only. It creates only a `tilepicky.json` metadata file to record grids, animations, and labels.
- **Project**: Holds the tilesheets you create and edit. Tilepicky writes output image files here, along with a project-specific `tilepicky.json`.

If you start Tilepicky without directory arguments, both panels prompt you to select a folder. You can change folders through the context menu in either file tree. Both paths are stored in `~/.config/tilepicky/settings.json`.

You can also pass paths as command line arguments:

    tilepicky <library dir> <project dir>

Tilepicky renders with OpenGL by default. Release builds also support WGPU with the `--wgpu` flag. To build with WGPU support from source, install with:

    cargo install tilepicky --features wgpu

## Install

Download binary releases for Linux, Windows, or macOS from the [releases page](https://github.com/spookysys/tilepicky/releases/latest). Extract the archive before running.

- On Windows, run `tilepicky.exe`.
- On Linux or macOS, run `./tilepicky` from the extracted directory. The macOS release supports both Intel and Apple Silicon hardware.

To install on Linux with desktop environment integration:

    install -Dm755 tilepicky ~/.local/bin/tilepicky
    install -Dm644 tilepicky.desktop ~/.local/share/applications/tilepicky.desktop
    install -Dm644 icon.png ~/.local/share/icons/hicolor/128x128/apps/tilepicky.png

The desktop launcher assumes `tilepicky` exists in your `PATH`. If your system cannot find the binary, update `Exec=` in `tilepicky.desktop` with the absolute path.

To compile and install from source, run:

    cargo install --locked --path .

## Layout

The window splits into two main columns:

- **Left column**: File trees. The library tree sits above, and the project tree sits below.
- **Right column**: Sheet viewports. The source sheet from the library sits above, and your project canvas sits below.

![The left column with both trees, the source sheet above, and the tilesheet being built below](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/screenshot.png)

Each sheet viewport includes a header toolbar. The toolbar displays grid parameters (tile size, gap, offset), current zoom level, selection coordinates, sheet filename, and hover tile coordinates. Buttons on the right edge open side panels.

### Inspector mode (Eye)

Press `E` to toggle inspector mode on the project canvas. When inspector mode is active:

- Hovering over any tile displays a tooltip with the original source sheet path.
- All tiles derived from that same source sheet highlight across the canvas.
- Hovering over empty space displays general sheet properties.
- Selection and editing actions are disabled while inspector mode remains active.

## Search

The search input sits above the library tree. Press `Ctrl+F` to focus the search field.

Search matches query terms as prefixes against sheet metadata:

- Entering `gra` matches `grass`.
- Multiple terms require all words to match.
- The filter menu beside the input toggles target fields: folder names, file names, AI captions, and AI tags.

Search runs locally and synchronously on your machine. It makes no network requests.

## Labeling sheets with AI

Tilepicky can generate searchable captions and tags for library sheets using multimodal vision models.

### Label a single sheet

1. Open a library sheet.
2. Open the **AI assist** panel with `I` or the header toolbar button.
3. Configure an OpenAI-compatible provider URL, API key, and model in Settings (`Ctrl+,`). The model must support image input and structured JSON output. Tilepicky ships with OpenRouter and `z-ai/glm-5.3-flash`; set `OPENROUTER_API_KEY` or type the key in Settings.
4. Select **Label with AI** in the panel or from the sheet context menu.

The model returns one caption and up to 12 tags. Tilepicky writes this label to `tilepicky.json`. You can continue working while the request runs in the background. Requests abort after 60 seconds, or when you click **Cancel**.

To delete existing labels, select **Remove AI label...** from the context menu and confirm.

GIF sheets submit their first frame. Images with dimensions exceeding 2048 pixels are scaled down before submission.

### Label an entire library

To label multiple library sheets in bulk:

1. Open the library folder.
2. Open **AI assist** (`I`) and select **Label entire library with AI...**.
3. Choose a batch model and provider key in Settings. The default is `z-ai/glm-5.3-flash:batch` on OpenRouter.
4. Review the unlabeled sheet count and token limits, then click **Start batch**.

With an OpenAI-compatible provider such as OpenRouter, Tilepicky sends one ordinary request per sheet in the background, one after another. OpenRouter's batch API accepts images only at public URLs, and your library stays on your machine. With Google Gemini, Tilepicky submits requests through the Gemini batch API in batches of up to 100 sheets. The AI assist panel displays progress and current state. Completed labels are written directly to `tilepicky.json`.

Batch state persists in `~/.config/tilepicky/` across application restarts. If a network error occurs, the batch worker retries automatically with exponential backoff up to 8 minutes. You can also click **Try again now**, **Cancel batch**, or **Send again**.

API keys are stored separately in `~/.config/tilepicky/keys.json` with restricted file permissions (`0600`).

## Keyboard navigation

Tilepicky supports complete operation using the keyboard.

![A house and two trees go from the Tiny Town pack into a new tilesheet without the mouse: the arrows open the pack, Ctrl+Tab moves between panes, Shift and the arrows select, Ctrl+C and Ctrl+V copy, Ctrl+T trims, and Ctrl+S saves](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/keyboard.gif)

In the recording, the arrows walk the library tree and open the Tiny Town pack. `Ctrl+Tab` moves to the source sheet, and `Shift` with the arrows selects a house. `Ctrl+C` copies it, and `Ctrl+Tab` into the empty canvas starts a new tilesheet, where `Ctrl+V` pastes it. Two trees follow the same way. `Ctrl+T` trims the canvas to what it holds, and `Ctrl+S` names and saves it.

### Pane and widget navigation

- `Ctrl+Tab` / `Ctrl+Shift+Tab`: Moves focus between pane bodies. Navigation cycles column by column: library tree, project tree, source sheet, project canvas, side panels, and status bar.
- `Tab` / `Shift+Tab`: Cycles through every interactive input and button in the same column order.
- `Ctrl+Tab` from the source sheet into an empty project canvas creates a new tilesheet initialized with that source tile size.
- The active pane displays a highlighted blue title.

### File tree controls

- `Up` / `Down`: Moves cursor between rows without opening files.
- `Right` / `Left`: Expands or collapses the selected folder.
- `Enter` or `Space`: Opens the selected file, or toggles folder expansion.
- `Shift+Up` / `Shift+Down` (Project tree only): Extends multi-file selection.

### Sheet controls

- `Arrows`: Steps the selection boundary in the pressed direction.
- `Shift+Arrows`: Expands or shrinks the selection rectangle.
- `Ctrl+Arrows`: Jumps cursor to the boundary of filled tiles or across gaps.
- `Alt+Arrows`: Moves the selection rectangle without moving tile contents.
- `Enter`: Begins editing a focused text field.
- `Escape` or `Enter`: Exits text field editing and returns keyboard control to the pane.

## Animations

You can define tile animations directly on a sheet:

1. Select a sequence of tiles.
2. Press `A` to open the animation panel and preview playback.
3. Set the `cell` parameter to specify frame size in tiles (such as `1x1` or `2x2`).
4. Set the `ms` parameter to configure frame duration in milliseconds.
5. Press `M` or click **Store** to save the animation. Pressing `M` again removes the definition.

![Two blocks of water tiles become animations: paste, A, set the frames, Store](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/animation-panel.gif)

Stored animations are saved in `tilepicky.json` using pixel coordinates. Changing a sheet's tile size preserves existing animations. Stored animations transfer automatically when copying or dragging tiles.

Animated GIFs play directly in the library panel. Copying an animated region extracts moving frames into an unrolled strip with an animation definition applied. Static regions copy as single frames.

![A waterfall is taken out of an animated GIF and lands as a marked strip](https://raw.githubusercontent.com/spookysys/tilepicky/main/media/animation.gif)

## Formats

Tilepicky reads PNG, GIF, JPEG, WebP, BMP, and TGA image formats.

Tilepicky exports sheets exclusively as 32-bit RGBA PNG files with straight alpha transparency.

Saving an edited project sheet from another format prompts for a PNG name and leaves the original file untouched.

## Grid configuration

Each sheet maintains independent grid parameters:

| Field | Description | Examples |
| --- | --- | --- |
| `tile` | Width and height of one tile in pixels | `32`, `32x48` |
| `gap` | Pixel spacing between adjacent tiles | `1`, `1x2` |
| `offset` | Pixel margin before the first tile row and column | `4`, `4x8`, `-3` |

You can modify each grid field with three input methods:

- Drag horizontally to adjust width.
- Rotate the mouse wheel over the field to adjust height.
- Click to enter numeric values directly.

Entering a single value sets uniform width and height.

Tilepicky analyzes new sheets automatically to detect repeating pixel pitch. Images without repeating patterns default to a single tile covering the full image dimensions.

Auto-detected grids are saved immediately to `tilepicky.json`. Manual grid adjustments override detected values and clear the auto-detected flag.

## Shortcuts

### Sheet operations

| Shortcut | Action |
| --- | --- |
| Click | Select single tile |
| Drag | Select rectangular tile region; scrolls near viewport edges |
| Click and hold (~250 ms) | Lift selection and begin drag operation |
| Double click and drag | Lift selection immediately and begin drag |
| Drag selection edge | Resize selection boundary |
| Shift+Click | Select rectangular range from previous selection anchor |
| Ctrl+Click | Toggle single tile in selection |
| Ctrl+Shift+Click | Add rectangular region to active selection |
| Ctrl+A | Select entire sheet |
| Right-click | Clear selection; clears tile contents if clicked inside selection; opens AI menu on library sheet |
| Ctrl+C, Ctrl+X, Ctrl+V | Copy, cut, and paste tiles (cut and paste apply to project canvas only) |
| Delete / Backspace | Clear selected tiles on project canvas |
| Ctrl+T | Trim empty outer rows and columns on project canvas |
| Ctrl+Z, Ctrl+Y / Ctrl+Shift+Z | Undo and redo operations |
| Ctrl+S, Ctrl+Shift+S | Save, save as |
| Ctrl+Scroll, `+` / `-` | Adjust zoom |
| Escape | Clear selection, cancel active drag, or leave text field |

### Drag modifiers

While dragging tiles:

- Hold `Ctrl` to duplicate tiles instead of moving them.
- Hold `Alt` to swap tiles between source and destination positions.
- A cursor badge indicates the active mode. Library tiles can only be duplicated.

Dragging tiles onto an empty project canvas creates a new tilesheet matching the source tile dimensions.

## File operations

File trees accept the following mouse and keyboard actions:

| Action | Result |
| --- | --- |
| Click | Open file |
| Up / Down | Move cursor without opening file |
| Right / Left | Expand or collapse selected folder |
| Enter / Space | Open selected file or toggle folder expansion |
| Shift+Up / Shift+Down | Extend marked file group (project tree only) |
| Ctrl+Click, Shift+Click | Select single file or range of files |
| Drag across rows | Select all crossed files |
| Click, hold (~250 ms), and drag | Move selected files into destination folder |
| Ctrl+Drag files | Duplicate selected files into destination folder |
| Right-click file | Context menu: rename, duplicate, delete, reveal in file manager, copy path |
| Right-click folder | Context menu: new folder, rename, delete, reveal in file manager, copy path |
| Right-click empty area | Context menu: new folder, refresh tree |

Library files are read-only and cannot be moved, renamed, or deleted within Tilepicky. Moving project files updates references in `tilepicky.json`.

## Settings

Press `Ctrl+,` or click the gear icon on the status bar to open Settings.

Settings configure:

- AI provider endpoints, API keys, and model selections for instant labeling and batch jobs.
- Visibility of the keyboard shortcut legend in the status bar.
- Target fields for search matching (folders, files, captions, tags).

Settings are stored in `~/.config/tilepicky/settings.json`.

## Provenance tracking

Tilepicky tracks the origin of pixels pasted into project sheets. When you copy tiles from a library sheet, the pixel data retains the source filename. If you copy tiles between project sheets, the original source path is preserved.

Activate inspector mode (`E`) and hover over any tile to view its source pack and filename in a tooltip:

    kenney_tiny-town/Tilemap/tilemap_packed.png

## Credits

Sample packs shown in documentation media:

- [Kenney](https://kenney.nl) (CC0)
- [ArMM1998](https://opengameart.org/content/zelda-like-tilesets-and-sprites) (CC0)
- [Epic RPG World](https://rafaelmatos.itch.io/epic-rpg-world-collection) by RafaelMatos (licensed copy)

## License

Tilepicky is free software distributed under the GNU General Public License, version 3. See `LICENSE` for the complete license text.
