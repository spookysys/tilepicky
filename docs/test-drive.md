# Interactive test pass

This pass used disposable library and project folders on private X11 displays.
The checks used mouse clicks, drags, keyboard input, screenshots, and saved files.
The checks did not change the user's sheets.

## Results

| Area | Result |
| --- | --- |
| PNG, BMP, JPEG, TGA, WebP, animated GIF | Opened each format. Confirmed GIF playback. |
| Tile editing | Selected, copied, cut, pasted, cleared, undid, redid, trimmed, and saved. Checked the saved pixels. |
| Dragging | Moved tiles, copied with Ctrl-drag, and dragged library tiles into a project. |
| Project files | Created, duplicated, renamed, moved into a folder, and deleted a copied sheet. Metadata followed the sheet. |
| Folders | Created, renamed, and deleted a disposable nested folder. |
| Search | Searched names and tags. Disabled caption and tag matching, checked the result, and restored the filters. |
| Grid and navigation | Changed tile size, gap, offset, and zoom. Used arrows, Shift+arrows, and pane navigation. |
| Source information | The eye mode tooltip identified the copied tiles' source. |
| Animations | Created, changed timing, removed, restored with Undo, copied between sheets, saved, and reopened. |
| Unsaved changes | Exercised Save, Discard, and Cancel when switching sheets. Opened and cancelled Save As. |
| Settings | Added and edited a provider and model, entered a test key, changed defaults, toggled batch mode, and removed the provider. |
| Restart | Verified saved grid metadata, copied animation, provider settings, hidden legend, and local batch state. |
| AI labels | Inspected labels and prompts, removed one label, and cleared all labels. The tag list remained. |
| Single-sheet AI | OpenRouter returned and saved a caption and tags for a copied sample sheet. A cancelled repeat kept the prior label. |
| Batch controls | Tested Rerun all, replacement counts, skipping labeled sheets, and confirmation cancellation. |
| Batch recovery | Used a loopback endpoint to test errors, Try again now, restart, and Cancel batch. |
| Gemini batch | A paid key submitted through `gemini-flash-latest`, which resolved to Gemini 3.8 Flash. Verified restart, caption and tag import, and cancellation. |
| Shortcut legend | Updated the text and inspected its layout. Verified that Settings can hide and restore the legend after the fix. |
| Installed app | Opened the installed binary, loaded a sheet, opened the AI pane, and opened Settings without a crash. |
| Native folder chooser | Opened both folder choosers, cancelled them, and selected different library and project folders. |
| File manager | Used "Open location". The file manager opened the correct folder and selected the requested image. |

The completed live batch exposed the positional tag response described below.
After the fix, the app fetched that completed batch again and saved its caption and tags without another generation request.
The separate cancellation check removed the local journal. Google returned a cancellation error for its sheet.
The native desktop checks used a separate D-Bus session and private display.
The installed executable matched the release build.

## Fixes and regression tests

The first pass recorded failing tests and deferred fixes at the user's request.
The user then requested fixes. The regressions now run in the normal suite.

### Restore the hidden shortcut legend

Settings previously consumed the same click twice when restoring a hidden legend.
The frame now captures the legend state before drawing either panel.
This keeps the layout consistent and draws Settings once per frame.

The regression drives the application's UI with pointer events:

```sh
cargo test --features wgpu settings_can_restore_the_hidden_shortcut_legend
```

The interactive check also hid the legend, reopened Settings, and restored the legend.

### Preserve provider error details

HTTP errors now retain the provider's message and status name, when present.
Single requests and batch requests share the bounded error reader.
It removes an echoed API key and limits the displayed explanation.
Unreadable, oversized, or non-JSON responses retain the HTTP status.
Completed batch errors retain explanations for both jobs and individual sheets.

The regressions cover the HTTP response, saved errors, and completed batch results:

```sh
cargo test --features wgpu batch_errors_keep_the_provider_explanation
cargo test --features wgpu provider_explanations_reach_the_saved_sheet_errors
cargo test --features wgpu completed_batches_keep_job_and_sheet_error_details
cargo test --features wgpu provider_errors_hide_keys_and_bound_untrusted_responses
```

The live failure was HTTP 400 with `FAILED_PRECONDITION` and "Precondition check failed."
The test key belonged to a Free tier project.
[Google's pricing](https://ai.google.dev/gemini-api/docs/pricing) excludes batch requests from the Free tier.
A paid key allowed the same sample sheet, normal prompt, and tags to be submitted.
The batch confirmation now states that Google requires a paid API project.

### Import positional tag answers

Gemini returned `listed` as an array of booleans despite the requested object schema.
The parser rejected that response before the fix.
It now maps complete boolean arrays to the tag order in the request.
It rejects wrong lengths, non-boolean answers, empty tag lists, and duplicate names.
The shared prompt also states explicitly that `listed` must be an object with the exact tag names as keys.
The app imported the completed live response with one labeled sheet and no failures.

```sh
cargo test --features wgpu ordered_tag_answers_use_the_requested_tag_order
cargo test --features wgpu ambiguous_tag_arrays_are_rejected
```

### Document the renderer default

Builds with WGPU use WGPU by default. Builds without it use OpenGL.
The README and agent instructions now describe this behavior.
The renderer default has not changed.
The regression checks both build configurations:

```sh
cargo test the_default_renderer_matches_the_documented_build
cargo test --features wgpu the_default_renderer_matches_the_documented_build
```

The interactive pass used explicit `--glow`.

## Other changes in this pass

The shortcut legend includes cut, clear, trim, animation, inspection, and Settings shortcuts.
It distinguishes the library context menu from project tile deletion.
The README places the legend below the trees and the search filters beside the search field.
It also documents Google's paid-project requirement and the `gemini-flash-latest` alias.

## Automated checks

- `cargo test --locked`: 109 passed, with no ignored tests.
- `cargo test --locked --features wgpu`: 109 passed, with no ignored tests.
- `cargo clippy --locked --all-targets --features wgpu`: passed.
- `cargo build --release --locked --features wgpu`: passed.
