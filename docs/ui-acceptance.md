# Native UI acceptance checks

Date: 2026-10-07.

The pass used generated image sheets, synthetic labels, isolated settings, and a private display.
AI requests used a local fixture provider. No paid provider calls or private images were used.
Tests exercised the native desktop UI through mouse and keyboard events. Captured screenshots were inspected.
The release app covered editing and palettes. A test build redirected AI transport to the local fixture.

PASS means the observed result matched the case. FIXED means the initial failure passed a later retest.
Screenshot numbers refer to local QA evidence, which is not published.

## Navigation and layout

- N01 PASS: Start with populated folders; inspect thumbnail layout and counts.
- N02 PASS: Select a thumbnail; check preview, filename, caption, and actions.
- N03 PASS: Switch Library -> Build -> Library; retain selection and sheet.
- N04 PASS: Resize to a narrow window; verify stacked canvases and reachable navigation.
- N05 PASS: Restore wide size; inspect panel widths and clipping.
- N06 PASS: Open and close Folders without changing paths.
- N07 PASS: Open Activity with no job; inspect empty state.
- N08 PASS: Toggle keyboard hints; inspect layout.

## Search and library selection

- S01 PASS: Search a filename prefix; verify matching sheets and count.
- S02 PASS: Search a saved caption word absent from filenames.
- S03 PASS: Search two words; require both in local word search.
- S04 PASS: Search nonsense; inspect empty results and clear recovery.
- S05 PASS: Toggle Without tags; verify only unlabeled sheets remain.
- S06 PASS: Select multiple thumbnails by checkbox; inspect scope count.
- S07 PASS: Clear selection; verify scope returns to whole library.
- S08 PASS: Open Compare search with no query; inspect guidance.
- S09 PASS: Compare a word query without vector indexes; inspect all columns.
- S10 PASS: Toggle search fields; verify changed result set.
- S11 PASS: Expand nested library folder and open a sheet from its tree.
- S12 PASS: Scroll gallery to last row; verify thumbnails and selection.

## Canvas editing and files

- E01 PASS: Copy a source tile and paste into a blank project.
- E02 PASS: Undo paste; verify blank pixels.
- E03 PASS: Redo paste and save; inspect saved image pixels.
- E04 PASS: Copy a rectangular multi-tile selection.
- E05 PASS: Delete project selection; undo restores it.
- E06 PASS: Switch workspaces with unsaved edits; retain edits.
- E07 PASS: Press project edit shortcuts in Library; project remains unchanged.
- E08 PASS: Toggle Inspect; editing is disabled and provenance is visible.
- E09 PASS: Zoom canvas in and out; retain usable grid alignment.
- E10 PASS: Copy a complete sheet through the inspector action.
- E11 PASS: Copy the same sheet again; preserve original and use collision-safe name.
- E12 PASS: Drag a gallery sheet into the Project tree.
- E13 FIXED: Drag a gallery sheet onto a project canvas; undo the placement.
- E14 PASS: Drag a library tree sheet into a Project folder.
- E15 PASS: Cancel a sheet drag with Escape; no file or pixels change.
- E16 PASS: Switch project sheets with unsaved changes; inspect save/discard/cancel.

## Color reduction and palettes

- Q01 PASS: Open colors on a project; inspect preview and palette viewer.
- Q02 PASS: Select each reduction method; show only relevant controls.
- Q03 PASS: SNES 2bpp uses four entries including transparency.
- Q04 PASS: Indexed four-entry palette with transparency has at most three opaque colors.
- Q05 PASS: Disable indexed transparency; allow four opaque colors.
- Q06 PASS: Inspect downsample format labels and transparency explanation.
- Q07 PASS: Change dither; preview updates without blocking navigation.
- Q08 PASS: Hover palette swatches; inspect index/RGB/hex information.
- Q09 PASS: Apply and clear reduction; verify project remains editable.
- Q10 PASS: Open export and cancel; no unexpected file write.
- Q11 PASS: Inspect mixed-depth controls and palette groups.
- Q12 PASS: Inspect color controls at narrow window size.
- Q13 PASS: Export an indexed PNG beside the source; validate dimensions, transparency, and the unchanged source.

## AI settings and job setup

- A01 FIXED: Open AI setup; one model picker per task, with connections above tasks.
- A02 PASS: Batch off hides separate batch model.
- A03 PASS: Change caption model; restore its individual concurrency limit.
- A04 PASS: Edit a concurrency limit, switch away and back; retain it.
- A05 PASS: Enable batch; reveal batch model and concurrency.
- A06 PASS: Change caption model while batch enabled; retain batch choice.
- A07 PASS: Disable batch; ordinary jobs use caption model.
- A08 PASS: Close and reopen settings; retain choices and limits.
- A09 PASS: Restart app; retain saved settings.
- A10 PASS: Missing API key blocks job start with useful guidance.
- A11 PASS: Open selected-sheet tag dialog; show saved label and request model separately.
- A12 PASS: Open current prompt; verify target and scope.
- A13 PASS: Improve search retains selected scope when settings opens.
- A14 PASS: Open image/text generation confirmations; inspect missing setup and costs.
- A15 PASS: Inspect provider catalogue failure state and custom model fallback.
- A16 PASS: Use local provider fixture to complete a single-sheet request.
- A17 PASS: Use local provider fixture for concurrent library requests; check limit and saved labels.
- A18 PASS: Local provider error and cancellation preserve prior labels.

- A19 PASS: Google catalogue shows local fixture models with unverified image support warnings (119).
- A20 PASS: The shortlist excludes free routes by default and shows input/output prices (121).
- A21 PASS: Add a custom embedding model; the task picker updates immediately (124-125).
- A22 PASS: Select image and caption indexes together; review both sources before generation (115-116).
- A23 PASS: Generate both indexes; save 18 vectors in each file through separate sequential requests (118).
- A24 PASS: Change the embedding model; review the warning before replacing each index (126).
- A25 PASS: Cancel image generation; do not start caption generation or overwrite saved vectors after the delayed reply (132-133).
- A26 PASS: Keep keyboard hints in the Build toolbar and out of AI setup (112, 134).

## Finish

- F01 PASS: Inspect every captured screenshot for clipping, overlap, misleading text, or missing feedback.
- F02 PASS: Check application logs for panic or rendering failures.
- F03 PASS: Check saved images and metadata against expected actions.
- F04 PASS: Stop the private app, display, session bus, portal, and mock provider; check remaining processes.

## Evidence and corrections

Whole-sheet canvas drops initially retained Library focus. Immediate Undo now restores the project canvas (75-77).
The shortcut legend initially left too little space for Project files. Its bounded scroll area preserves the tree (78).
The Without tags filter now applies to both the gallery and tree (79).
Activity now distinguishes concurrent requests from native batches and removes running indicators when jobs finish (109-110).
AI setup now has one model picker per task and task-specific catalogue browsing (112-125).

The local OpenAI-compatible job reached three concurrent requests. Google reached two and saved all 18 labels.
Single-sheet errors and cancellation preserved the previous label, including after a delayed response (95, 103-104).
Both embedding indexes saved 18 vectors. The log showed image batches of 16 and 2, followed by text batches of 16 and 2.
The cancellation retest sent one image batch and no caption batch. Both previous 18-vector indexes remained unchanged (132-133).

All 18 source images matched the generated originals. Four imported files matched their sources byte for byte.
The project metadata retained grids, provenance, and copied labels. Application logs showed no panic or rendering failure.
The final automated run passed 314 tests, with four ignored. Clippy passed, and the release build completed.

## Coverage limits

Live provider availability, model quality, paid billing, native provider batches, and object storage were not tested against external services.
The local fixtures verify request conversion, concurrency, progress, cancellation, and saved results.
The native export chooser passed cancel and save checks on the isolated session bus. All private test processes stopped.
The fixture images do not cover every animated or unusually large image format.

Final release checks: the export chooser starts beside the source (138). The exported indexed PNG retains 128 x 96 pixels and transparency (139).
The source bytes remain unchanged. Keyboard hints can be hidden and restored (140). AI setup remains usable at 820 x 600 (141).
The final process check found no processes on the private display. The release application log was empty.

The release command-line help check preserves synthetic legacy logs and creates no settings file.
