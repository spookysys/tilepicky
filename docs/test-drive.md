# Interactive test pass

This pass used disposable library and project folders on private X11 displays.
The checks used mouse clicks, drags, keyboard input, screenshots, and saved files.
The pass did not change the user's sheets or application settings.

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
| Gemini batch | Submitted one sample sheet with the normal prompt and tag list. Google returned HTTP 400. |
| Shortcut legend | Updated the text and inspected its layout. Found the visibility bug below. |

The live Gemini failure prevented checks of remote cancellation and result import.
Existing transport tests cover those operations with fake responses; they do not prove live service compatibility.
The native folder chooser and desktop file-manager integration were not exercised on the private display.

## Fixes for later

The user requested that this pass record bugs without fixing them.
Each regression below failed when run.
They are explicitly ignored in the normal suite until the fixes are ready.

### Restore the hidden shortcut legend

1. Hide the legend in Settings.
2. Close Settings, then reopen it.
3. Click "Show keyboard shortcuts".

The legend stays hidden.
The frame draws the status bar before the side panel when the legend is hidden.
The checkbox changes that state, so the frame draws the status bar again afterward.
The second Settings draw consumes the same click and reverses the change.

The regression drives the application's UI with pointer events:

```sh
cargo test --features wgpu settings_can_restore_the_hidden_shortcut_legend -- --ignored
```

Fix the frame's layout decision so it draws Settings once.

### Preserve provider error details

The live Gemini submission failed with HTTP 400.
The app displayed "The provider refused the batch (HTTP 400)."
It discarded the response body, so this pass could not determine Google's reason.
Do not assume that the model, key, or request schema caused the rejection.

The regression uses a local server with an explanatory HTTP 400 response.
It proves that the transport loses that explanation; it does not reproduce Google's unknown rejection reason.

```sh
cargo test --features wgpu batch_errors_keep_the_provider_explanation -- --ignored
```

Preserve a bounded, safe provider error message, then repeat the live Gemini test.
Verify submission, restart, import, and cancellation after the cause is known.

### Keep OpenGL as the default with WGPU enabled

The README says OpenGL is the default.
A build with both renderers selects WGPU through `eframe::Renderer::default()`.
The interactive pass used explicit `--glow`; this discrepancy came from checking the launch configuration.

```sh
cargo test --features wgpu the_default_renderer_is_opengl_with_wgpu_available -- --ignored
```

Choose OpenGL explicitly as the default while keeping `--wgpu` available.

## Changes made in this pass

The shortcut legend now includes cut, clear, trim, animation, inspection, and Settings shortcuts.
It distinguishes the library context menu from project tile deletion.
The README now places the legend below the trees and the search filters beside the search field.
The renderer helper and UI drawing method expose existing behavior to regression tests.
They do not fix the recorded bugs.

## Automated checks

- `cargo test --locked --features wgpu`: 101 passed, with the three known bugs explicitly ignored.
- `cargo test --locked --features wgpu -- --ignored`: all three known bug tests failed as expected.
- `cargo clippy --locked --all-targets --features wgpu`: passed.
- `cargo build --release --locked --features wgpu`: passed.
