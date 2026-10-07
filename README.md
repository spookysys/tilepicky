# Tilepicky

Tilepicky is a desktop tool to extract tiles from sprite sheets and assemble them into custom tilesets. You browse your sprite library, select tiles, and place them on an editable canvas.

<https://github.com/spookysys/tilepicky>

![A tilesheet of your own is built from two packs, found through the search box](media/demo.gif)

Open a project sheet, search the library for the tiles you need, select a region, and drag or copy the tiles onto the canvas.

## Library and project folders

Tilepicky organizes your work into two directories:

- **Library**: Holds your collected sprite sheets. Tilepicky treats this directory as read-only. It creates only a `tilepicky.json` metadata file to record grids, animations, and labels.
- **Project**: Holds the tilesheets you create and edit. Tilepicky writes output image files here, along with a project-specific `tilepicky.json`.

Use **Folders** to open or change the library and project folders. Both paths are stored in `~/.config/tilepicky/settings.json`.

You can also pass paths as command line arguments:

    tilepicky <library dir> <project dir>

Source builds use OpenGL unless you enable WGPU. Builds with WGPU support, including release downloads, use WGPU by default.
Use `--glow` to select OpenGL, or `--wgpu` to select WGPU when available. To build with WGPU support from source, install with:

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

Use the top bar to switch between two workspaces:

- **Library** shows sheet thumbnails, a folder tree, search, and a selected-sheet inspector.
  Click a thumbnail to inspect it. Use its checkbox to select it for an AI task.
  **Without tags** shows sheets that have no saved label.
- **Build** places the source sheet beside the project canvas. Narrow windows stack the canvases.
  The file trees remain available beside the canvases. **Use in tilesheet** opens Build with the inspected source.

Switching workspaces keeps open sheets, selections, zoom, and unsaved project changes.
Project edit shortcuts operate only in Build. Saving remains available in both workspaces.
**Colors and export...** opens the existing palette, preview, and export tools for the project.

**Improve search...** captures the selected files and folders. With no selection, it targets the whole library.
Review the scope, choose a task, and review its confirmation before any generation starts.
**Activity** holds library tagging jobs and search index progress. You can close it and keep browsing.
**AI setup...** opens a resizable settings window.

Each Build canvas has a header with grid parameters, zoom, and selection details.
The recordings in this README show the earlier layout. Their tile operations still apply in Build.

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
- The filter menu beside the input toggles target fields: folder names, file names, AI captions, AI tags, image meaning, and caption and tag meaning.

Word search runs locally. Every query word must match an enabled word field. Either meaning source can add matching sheets.
Meaning search needs saved embeddings; see [Search by meaning](#search-by-meaning).
The gallery combines separate source rankings. It does not compare raw similarities from different models.
The inspector names the matching sources. The folder tree keeps its folder order.
**Compare search** shows separate result lists for words, image meaning, and label meaning.
Each meaning column shows index coverage and offers recovery after a failed query.

## Labeling sheets with AI

Tilepicky can generate searchable captions and tags for library sheets using multimodal vision models.

![Read a saved example label and find a dungeon sheet through its weapon tag](media/ai-labels.gif)

The recording shows an earlier layout with saved example labels. It shows the AI panel, **Show AI label...**, and a search for `weapon`.
The search finds the dungeon sheet through its tags. The recording sends no requests.

### Label a single sheet

1. Inspect a library sheet and click **Review / generate tags...**.
   You can also choose **Generate Tags (AI)...** from the sheet or file-tree context menu.
2. Choose the **Captions and tags** model and its provider key in **AI setup...** (`Ctrl+,`).
   Both Google Gemini and OpenAI-compatible image models are supported.
3. Click **Label this sheet**. The dialog shows progress, the result, or the error.
4. Use **Label again** to replace a saved label, or **Retry** after a failed request.
   Opening the dialog does not send a request.

The model returns a caption, freeform tags, and matching tags from your list. The prompt asks for at most the library's free-tag count, 16 by default; a reply that returns more is kept whole.
Tilepicky writes the label to `tilepicky.json`. The dialog shows the saved model separately from the next request's model.
Closing the dialog or pressing Escape keeps the request running. Click its status-bar entry to reopen it.
The dialog remains attached to its sheet when you select another sheet or library.
A failed or unlabelable replacement keeps an existing usable label. Results cannot overwrite an image or label changed during the request.

The sheet dialog and Activity each have a **Copy log** button for their own job.
Copying shows **Copied** beside that button. A button stays disabled when the matching job has no log.
A saved label alone does not provide a request log.

Only one labeling job can run at a time. An outstanding batch also blocks single-sheet requests while paused.
Each library stores one current job log in `.tilepicky-ai-log.jsonl`, beside `tilepicky.json`.
A new job replaces that file, whether it labels one sheet or the library.
The sheet dialog's button refers to that dialog's target request. The library button refers to the library batch.

Finished jobs keep their log until you exit. Exiting removes finished logs and keeps unfinished logs.
Restarting a batch continues its log. An interrupted single-sheet request keeps its log for diagnosis and does not resend automatically.
On restart, that single-sheet job becomes a finished interruption. Its log remains available until the next exit.
Retrying sheets within a batch continues that batch's log unless another job has replaced it.

Logs include prompts, requested tags, model replies, parsed tags, timing, and errors. They exclude API keys and image data.
Each log keeps up to 2 MiB of recent entries. Copied logs report when earlier entries were discarded.
Sheet names and generated captions remain in the log. Older shared logs are removed when the updated app starts.

Each request times out after 60 seconds. **Cancel** stops waiting, but the provider may still bill the request.

To delete a label, select **Remove label...** in the sheet dialog and confirm.
**Current prompt...** shows what a new request for this sheet would send, including its current library tags and filename context.

OpenRouter settings have a **skip** field for provider slugs, separated by commas. It defaults to `phala`, including for older settings.
Clear the field to allow all providers. Ordinary requests use this list. OpenRouter batches use the account routing settings.
A batch keeps the provider settings it started with.

New requests include the filename and library-relative folder as optional clues. They never include the absolute path.
The prompt tells the model to use visible content and treat names as data, not instructions.
Existing batches keep their original prompt and context policy. The request log includes the exact context sent.

GIF sheets submit their first frame. Images with dimensions exceeding 2048 pixels are scaled down before submission.

### Search by meaning

Open **Improve search...** and choose **Review image index...** or **Review label index...**.
The dialog captures the selected sheets. File and folder context menus also provide scoped index actions.

- **Images** embeds each sheet's image. Labels are optional.
- **Captions and tags** embeds saved label text without the image. It skips sheets without labels.

The confirmation shows the provider, model, source type, and number of sheets. Generation can incur provider charges.
Preparation and generation run in the background. Cancel stops further requests; completed batches remain saved.
An active request may still incur a charge. Failed runs keep completed batches, so a retry need not repeat them.

Image vectors live in `embeddings.json`. Label vectors live independently in `embeddings-text.json`.
Each file records its model and provider identity. Changed sources need new vectors. Changed models or providers require regeneration.
Old vectors without a provider identity also need regeneration. Damaged files produce an error and remain unchanged.

Choose **Image meaning** and **Label meaning** models in **AI setup...**. Image search requires a model that accepts images and text.
Label meaning can use a separate text-only model. **Use image model** preserves the shared-model option.
The shipped default is `voyageai/voyage-multimodal-3.5` through an OpenAI-compatible endpoint.

**Include captions and tags in image embeddings**, under **Advanced task options**, combines label text with the image input.
Leave it off to compare image embeddings against independent label embeddings. Changing this option requires regenerating the image vectors.
Label edits invalidate the affected label vectors and combined image vectors.

Use **image meaning** and **caption and tag meaning** in the search filter to compare the saved sources.
Disable the other fields to test one source alone. **Minimum similarity** controls how close a vector must be.
The gallery ranks matches. The folder tree keeps its folder order.

Meaning search sends your query to the selected embedding provider after a short pause in typing.
Each source uses its configured model and provider. Compatible sources can reuse a query vector. Word search needs no network.
Failed queries show an error and a **Retry meaning search** button. They do not retry on every frame.

### Label an entire library

To label multiple library sheets in bulk:

1. Open the library folder.
2. Select sheets, open **Improve search...**, and choose **Review tag generation...**. Then choose the unlabeled-sheets action.
3. Choose **Captions and tags** in **AI setup...** and enter its provider key. **Try one sheet...** previews a small job first.
4. Review the unlabeled sheet count and the tags to look for, then click **Start labeling**.

Use **Rerun all...** to label every sheet again with the current library model and tag list.
Review the request count and confirm with **Start labeling**. Existing labels stay until new results arrive.

To label part of a library, choose a folder or mark a group in the tree first.
Click a folder to choose it alone; it wears the selection colour and the panel target becomes that folder.
Ctrl+click folders and files to add or remove them from a marked group; a marked folder brings every sheet below it, in its subfolders too, so folders and files can share one selection. Marked rows wear the selection colour.
Right-click a marked row and select **Generate labels for selection (AI)...**.
For an unmarked folder, **Generate labels for folder (AI)...** takes that folder alone.
The panel names the target and offers **Label unlabeled in selection...** and **Rerun all in selection...**.
Use **Whole library** to clear the choice.

The confirmation estimates the job's cost in USD before you start. **Estimate details** shows input and output token ranges and price sources.
It reads local image sizes and fetches OpenRouter's public model prices. It does not upload sheets to calculate the estimate.
Google estimates use a dated table of batch prices. The table expires instead of silently keeping old prices.
With **Sheet storage**, the OpenRouter estimate uses the batch rate, about half the live catalog price.
For `gemini-flash-latest`, the confirmation names its price assumption because the alias can change.
Unsupported models, expired prices, or a failed price lookup show an unavailable estimate. You can still start the job.

The range includes estimated image, prompt, response, and reasoning tokens. It is not a spending limit.
Without prior usage, output assumes 256 to 1536 tokens per sheet. Actual use can exceed that range.
For a matching fixed model and tag list, at least five reported requests from the previous job refine the output range.
Retries, taxes, route prices, and provider fees can change the final bill.
**Job cost estimate** retains the initial estimate and shows reported usage at those rates while the job runs.
The library book keeps the estimate and usage across restarts. Missing usage and unconfirmed requests are not included in reported usage.

Failed and unlabelable requests keep their old usable labels.

Use **Clear all labels...** to remove all saved AI captions and tags in the library, including its subfolders.
Confirm with **Clear all**. Images, grids, animations, and the list of tags to look for stay.
This cannot be undone. Wait for labeling to finish or cancel it before you clear labels or start another batch.

A batch keeps the tag list it started with, even if you change the list while it runs.

OpenRouter with **Sheet storage** uploads images to your bucket and submits batches that can run while Tilepicky is closed.
OpenRouter and its model providers fetch those images through signed URLs. Tilepicky deletes the objects after a confirmed terminal result.
An ambiguous submission keeps its objects, because accepted work may still need them. Configure a bucket lifecycle rule for eventual cleanup.
The excluded-host list applies to ordinary requests. OpenRouter batch routing uses the account settings.

Other OpenAI-compatible endpoints use concurrent ordinary requests, even when storage is configured. The app must remain open for these jobs.
Google Gemini uses batches of up to 100 sheets. Both batch methods count accepted, unfinished batches against the configured concurrency limit.
Activity shows progress and saves completed labels into `tilepicky.json`.
Invalid label replies can retry up to three times. Cancelling a job stops new requests and discards late label results.


Google batch requests require an API key from a project with billing enabled. The Free tier does not support batches.
Tilepicky offers `gemini-flash-latest` for both scopes. This alias follows Google's latest Flash release, which can change.
The previously shipped Google default migrates to this alias. Existing jobs keep their saved model.
Choose one captioning model for single sheets and library tagging.
With **Enable batch processing** off, library tagging sends concurrent requests to that model, including on Google.
Each model remembers its **Maximum concurrent requests** limit.
Enable batch processing to select a separate batch model and its concurrency limit.
Native batches require Google or OpenRouter with sheet storage. Existing jobs keep their saved model and transport.
Choose a captioning model that accepts images. Embedding models make vectors for search.
Failures show their cause and a next step beside the job status. Billing errors link to Google billing when applicable.
API key errors offer a Settings button. Saved labels remain unchanged when a request fails.
Repeated errors appear once, with the affected sheet count. **Error details** keeps the full explanations and affected sheet paths.

When no newer message is visible, the status bar shows the outstanding batch's progress and state.
Click the batch status to open **Activity** and its controls.
A library failure keeps a status-bar notice, including after the job finishes. Click it to open the recovery actions.
A failed single-sheet request keeps a clickable notice while its current request log remains available.

The job view separates saved labels, provider work, unsent sheets, and uploads awaiting confirmation.
The progress bar counts finished outcomes, including failures and cancellations. **Labels saved** counts usable labels written to the library.
The main message distinguishes queued work, processing, and an unknown provider state. The last provider response, next check, and current request appear below it.
**Details**, or **Error details** after a failure, keeps the job's original tags and provider batch information available.

**Pause uploads** stops new submissions after the current request finishes. Tilepicky still collects accepted results.
For a provider that labels sheets one by one, the button is **Pause**. **Resume** continues the same job.
**Cancel job** stops new submissions. For a Google batch it asks Google to cancel accepted work, and the job stays visible as **Cancellation pending** until Google confirms an outcome.
An OpenRouter batch has no cancel call: Tilepicky stops waiting and releases the uploaded objects, but the batch may still finish at OpenRouter and bill. Its sheet labels are not collected. Saved labels stay.

Closing pauses local uploads and result collection. Google can continue work it has accepted.
Reopen the same library to continue. Pause and cancellation requests survive restart.
Each library has at most one outstanding job, stored under `ai_batch` in its root `tilepicky.json`.
The record includes progress, provider batch IDs, and pause or cancellation requests. API keys stay in the app configuration.
The job follows the library when you move the folder. Older configuration journals migrate automatically without another submission.
Only one Tilepicky window can own a library job at a time.

Tilepicky saves each submission's reference before sending it. A lost reply triggers an automatic lookup.
An uncertain submission stays separate while other work can continue. A submission that is out or awaiting confirmation counts against the model's number, so a flaky connection cannot pile up more paid work than the model allows.
When the lookup proves Google never made the batch, Tilepicky sends its sheets again on its own, so an interrupted upload resumes. **Retry unconfirmed sheets...** remains for a submission the lookup could not resolve, and explains the possible duplicate charge before a resend.
You do not need to find or attach a provider batch ID.

Network operations retry independently, with a delay of up to eight minutes after repeated failures.
A successful status check does not discard an upload or cancellation error. **Retry now** requests another attempt.
**Retry failed sheets** becomes available after a job finishes with failures.
A failed library write retries from stored results without another model request.
New jobs record the image and existing label before submission, so delayed results cannot silently replace newer work.

Single-sheet actions live in the sheet dialog. Library jobs live in **Activity**.
The panel identifies its execution method as **Google batch**, **OpenRouter batch**, or **Several sheets at a time**.
A running job keeps its original provider, model, prompt, and tags.
Resume and retry also use the saved model. Changing Settings selects the model for new jobs.
The panel shows a notice when the saved job and Settings use different models.
To use the selected model, cancel the saved job or let it finish. Then choose **Label unlabeled sheets...**.
Saved labels stay. **Rerun all...** also replaces existing labels.
**Copy log** includes the current job summary and recent diagnostics, without keys or image data.

### Model discovery

**AI setup...** assigns models to tasks. Choosing **Captions and tags** sets the model for single-sheet and library requests.
**Advanced task options** retains a separate library override.

The provider catalogue refreshes when you open setup, and after an hour while it remains open.
**Refresh available models** requests an immediate update. A private local cache keeps the last catalogue available after a failed refresh.
Refreshing the catalogue never changes a selected model and never uploads sheets.

The shortlist shows up to three compatible models, ordered by their listed input price.
These are starting points, not measured quality recommendations. Output prices, routing, and image charges can change the total cost.
**Choose another model** searches the compatible catalogue. **Use a custom model ID** accepts an exact provider ID.

Some providers do not report input capabilities. Those models appear only when you enable unverified capabilities.
Gemini reports generation methods but does not establish image support in its model list.
Try one sheet before you start a large job with an unverified model.
An ID containing `latest` can change at the provider. Use a versioned ID when you need a fixed model.

### Tags to look for

Open **Edit library options...** from the sheet dialog or Activity.
Both links open the same form for their stated library. Separate tags with commas, then select **Save options**.
Changes apply to new requests in both scopes. Existing jobs keep their original tags.
The prompt asks the model to check each tag independently, including secondary content.
It asks for every matching tag, with the spelling from your list, and no duplicate synonyms.
The model can still miss visible content. These tags do not count toward the free-tag count the prompt asks for.

**Free tags per sheet** sets the most tags the model may add of its own, from 0 to 64. A new library starts at 16. The prompt only asks: a reply is kept whole even when the model returns more.

A new library starts with `character, NPC, hero, landscape, building, indoor, UI, font, animation, props, background`. **Reset tags** brings that list back. The list and the free-tag count belong to the library: Tilepicky writes them at the top of the library's `tilepicky.json`.

Each label records the list its request used. **Current prompt...** in the sheet dialog previews the next request.
**Prompt template...** in library options shows the shared text before each request adds its filename context.
The log contains the original request. The popup's current prompt can differ from that request. Changing the list does not relabel anything.
To apply the new list, select **Label again**, or remove the label and label the library again.

API keys are stored separately in `~/.config/tilepicky/keys.json` with restricted file permissions (`0600`).

## Keyboard navigation

Build retains the canvas and file-tree keyboard controls. Library uses Tab to reach its gallery controls.

![A house and two trees go from the Tiny Town pack into a new tilesheet without the mouse: the arrows open the pack, Ctrl+Tab moves between panes, Shift and the arrows select, Ctrl+C and Ctrl+V copy, Ctrl+T trims, and Ctrl+S saves](media/keyboard.gif)

In the recording, the arrows walk the library tree and open the Tiny Town pack. `Ctrl+Tab` moves to the source sheet, and `Shift` with the arrows selects a house. `Ctrl+C` copies it, and `Ctrl+Tab` into the empty canvas starts a new tilesheet, where `Ctrl+V` pastes it. Two trees follow the same way. `Ctrl+T` trims the canvas to what it holds, and `Ctrl+S` names and saves it.

The following canvas navigation applies in **Build**. Library excludes hidden canvases from pane navigation.

### Pane and widget navigation

- `Ctrl+Tab` / `Ctrl+Shift+Tab`: Moves focus between pane bodies. Navigation cycles column by column: library tree, project tree, source sheet, project canvas, side panels, and status bar.
- `Tab` / `Shift+Tab`: Cycles through every interactive input and button in the same column order.
- `Ctrl+Tab` from the source sheet into an empty project canvas creates a new tilesheet initialized with that source tile size.
- The active pane displays a highlighted blue title.

### File tree controls

- `Up` / `Down`: Moves cursor between rows without opening files.
- `Right` / `Left`: Expands or collapses the selected folder.
- `Enter` or `Space`: Opens the selected file, or toggles folder expansion.
- `Shift+Up` / `Shift+Down`: Extends the marked group from where the cursor started. In the library tree it marks folders and files alike.

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

![Two blocks of water tiles become animations: paste, A, set the frames, Store](media/animation-panel.gif)

Stored animations are saved in `tilepicky.json` using pixel coordinates. Changing a sheet's tile size preserves existing animations. Stored animations transfer automatically when copying or dragging tiles.

Animated GIFs play directly in the library panel. Copying an animated region extracts moving frames into an unrolled strip with an animation definition applied. Static regions copy as single frames.

![A waterfall is taken out of an animated GIF and lands as a marked strip](media/animation.gif)

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

## Color quantization preview

The paintbrush button in the tilesheet header opens the **Color quantization**
panel. It previews how a sheet looks once its colors are reduced. It changes
no source pixels on disk. The preview runs in the background and refreshes after image or setting changes.

Choose what the setting applies to:

- The open sheet.
- A marked group of files.
- A folder, chosen through **Color quantization...** in its context menu.

The panel shows the target, and a **paths** button lists the sheets it covers.
A group shares one palette, so a whole scene is quantized together. A sheet
can hold only one setting. **Apply** updates the same group directly and asks before it replaces overlapping groups.
Opening a target shows one of its sheets, after any required save decision.
SNES groups require matching tile sizes without gaps or offsets. Reduce color depth and Limit palette colors support other grids.
Settings and painted pins follow file renames and copies. Deleting files removes their saved targets and pins.
When a moved sheet has a different setting from its destination folder, the source setting stays with the sheet.
The destination group becomes an explicit file group to prevent overlap.

Choose one option from **Color reduction method**. Its settings appear under a separate heading.
**Palette entries** counts every slot, including transparency when reserved. Four entries with transparency allow three visible colors.
**Palettes** controls the number of separate SNES palettes. Each SNES palette always reserves index 0 for transparency.
RGBA formats preserve alpha separately; their palette limit counts RGB colors, as the panel explains.

**Palette preview** shows the computed colors, grouped by SNES palette when applicable.
`T` marks a transparent entry. Hover over a swatch for its index, hex value, and RGB values.
Click a color to copy its hex value. Reduce color depth does not create a palette.

- **SNES** uses tile palettes and hardware color limits, as described below.
- **Reduce color depth** reduces each pixel's color precision without a palette. Image dimensions stay the same.
  Choose `rgb565`, `rgba5658`, or `rgb233`.
- **Limit palette colors** builds a palette with a maximum number of colors and maps pixels to it.
  Choose `rgb565`, `rgba5658`, `rgb888`, or `rgba8888`. The formats without alpha
  can reserve color 0 as transparent. A palette has up to 256 colors.
**SNES** is tile-aware. Pick the tile depth (`2bpp`, `4bpp`, `8bpp`, or
  `4bpp + 8bpp`), the number of palettes, and the total entries per palette. Index 0
  of every palette is transparent. With `4bpp + 8bpp`, the **auto** slider
  promotes a share of the tiles to 8bpp, and the **paint** and **erase**
  tools pin or unpin a tile by hand.

Dithering is offered in both modes.

**Save Quantized...** saves the preview shown when the file dialog opens. It does not overwrite the source sheet.
Indexed output preserves the palette and pixel indices. Downsample output and formats with per-pixel alpha use RGBA PNG.
A failed write preserves the previous output file. PNG does not store the target engine's tile arrangement.

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
- Hold `Shift` to lay the tiles over what is already there: transparent
  pixels of the dragged block leave the sheet beneath them visible. It
  works alone or with `Ctrl`, since a blend is already a copy.
- A cursor badge indicates the active mode. `Alt` overrides `Ctrl` and `Shift`; a
  library tile is otherwise duplicated, and with `Shift` it is laid over
  instead.

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

**Captions and tags**, **Image meaning**, and **Label meaning** select the models for new requests. Existing jobs keep their saved configuration.
**How library jobs run** explains the execution method. **Set up Gemini...** opens its key field without changing the selected model.

Give an OpenAI-compatible provider **Sheet storage** to use OpenRouter batch processing. The fields are the S3 endpoint (host only, no path), region, bucket, access key, and a path-style switch for MinIO. The secret key sits with the API keys, readable by your user alone. The bucket may stay private: Tilepicky hands OpenRouter a signed URL for each sheet. Any S3-compatible store works, for example Cloudflare R2, Backblaze B2, or MinIO. **Test connection** uploads one small object, fetches it through its signed URL, and deletes it, so a setup mistake shows here rather than mid-job.

Tilepicky deletes each object as soon as its batch ends. Add a bucket lifecycle rule that deletes objects older than two days, to cover a crash after upload or a discarded job. Two days also matches the signed URL, which outlives the 24-hour batch window.

If the reply to a batch submission is lost, Tilepicky does not send that batch again, because OpenRouter offers no client reference to find it. Those sheets end with an error. Retrying them can bill twice.

**API keys** shows one field per configured provider. The built-in providers and models are ready for their keys.
Open **Advanced connections and models** to add custom models or change connections. It starts collapsed.
Removing a provider or model asks for confirmation and explains which active selections it clears.

Click **Done**, press Escape, or close the Settings window to save. An outside click leaves Settings open.
Closing Tilepicky also saves edits from an open Settings window. A save error keeps the app open.
If saving fails, Settings stays open with the error and a **Retry save** button.
The keyboard-shortcut checkbox controls the legend below the file trees in Build.

Use the menu beside the search field to choose folders, files, captions, and tags for search matching.

Settings are stored in `~/.config/tilepicky/settings.json`.

### Setting up a bucket

Use any S3-compatible object store. Cloudflare R2 is a common choice: 10 GB free and no egress fees, though it asks for a payment method before it activates, even on the free tier. Backblaze B2 is an alternative.

The store must be reachable from the internet, because OpenRouter fetches the signed URL. A MinIO instance on your own machine works for **Test connection** but not for a batch.

1. Make an account, enable the object store, and create a bucket.
2. Create an API token scoped to that one bucket, read and write objects. It gives an access key ID and a secret access key.
3. Copy the S3 endpoint. For R2 it is `https://<account id>.r2.cloudflarestorage.com`, and the region is `auto`.
4. In **Sheet storage**, tick it and fill endpoint, region, bucket, and access key. Put the secret in the secret key field; it is kept in `keys.json`, readable by your user alone. Leave path-style off for R2 and turn it on for MinIO.
5. Click **Test connection**. Then label a small library to prove the path, and set the bucket lifecycle rule described above.

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
