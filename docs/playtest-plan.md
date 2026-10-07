# Native workflow play-test

Date: 2026-10-07.

This is a new pass after the earlier acceptance checks. The tables include the native pinning, AI, and workflow results.
A case passes only after native mouse or keyboard actions and inspection of the visible result.
Record the screenshot name and any saved-file check beside the result. Do not infer a pass from unit tests.

Use a private display, isolated settings, and generated sheets. Keep the user's library and project unchanged.
Use a fresh fixture with a blank canvas, a colorful sheet, transparent tiles, and two differently sized sheets in one folder.
Include saved labels, an unlabeled sheet, and filenames with spaces. Keep an untouched copy for comparisons.
Use local provider fixtures for failures and delayed replies. Live AI checks have a separate total budget of EUR 1.
Record live requests and their costs before further paid requests. Do not send private images or communications.

PASS means the observed result meets the expectation. FIXED requires a recorded failure and a successful native retest.
BLOCKED names the missing prerequisite. NOT RUN remains separate from PASS.

## Tile depth painting

| Case | Status | Actions and expected result |
| --- | --- | --- |
| P01 | PASS | Open mixed 4bpp + 8bpp controls. Find Force 4bpp, Force 8bpp, and Auto without reading source code. |
| P02 | PASS | Force one colorful tile to 4bpp. Inspect its outline, preview, and palette limit. |
| P03 | PASS | Force another tile to 8bpp. Distinguish its outline from forced 4bpp and automatic depth. |
| P04 | PASS | Paint a row while holding the mouse button. Every crossed tile receives the chosen override. |
| P05 | PASS | Drag diagonally across tile corners. Inspect missed cells, unexpected neighbors, and repeated changes. |
| P06 | PASS | Drag out of the canvas and release. Return without a button held; painting must stop. |
| P07 | PASS | Select Auto and paint both forced depths. Remove their overrides without changing source pixels. |
| P08 | PASS | Right-click forced tiles with each force tool selected. Remove overrides without opening a conflicting menu. |
| P09 | PASS | Hold the right mouse button and drag across forced tiles. Inspect whether removal matches the displayed instructions. |
| P10 | PASS | Return to the left mouse button after right-click removal. The selected force tool still works. |
| P11 | PASS | Change the automatic 8bpp allocation after forcing both depths. Forced tiles retain their explicit choices. |
| P12 | PASS | Force all tiles to 4bpp, then increase automatic allocation. Automatic selection must not override them. |
| P13 | PASS | Force several tiles to 8bpp with a smaller automatic allocation. Preserve them and explain the resulting count. |
| P14 | PASS | Zoom in and out, then paint at canvas edges. Overrides follow the tile under the pointer. |
| P15 | FIXED | Switch between tile painting and ordinary selection. Selection, copy, and delete must not leave painting active. |
| P16 | PASS | Paint overrides, then press Undo and Redo. Record the behavior and check that unrelated pixels remain safe. |
| P17 | PASS | Apply settings, switch sheets, and return. Both forced depths and Auto retain their saved meaning. |
| P18 | PASS | Restart after applying overrides. Compare outlines, preview, counts, and saved metadata with the earlier state. |
| P19 | PASS | Open a fixture with legacy boolean pins. Existing 8bpp pins remain forced; other tiles remain automatic. |
| P20 | PASS | Switch mixed depth to fixed depth and back. No stale painting controls or unexplained override loss appear. |
| P21 | PASS | Apply a folder group across two differently sized sheets. Overrides stay attached to the correct sheet and tile. |
| P22 | FIXED | Move one sheet into a separate quantization group. Check both previews and retained overrides. |
| P23 | PASS | Clear quantization on one grouped sheet. Inspect the other sheet for accidental setting or override changes. |
| P24 | FIXED | Change the grid after pinning tiles. Verify invalid overrides cannot silently attach to unrelated tiles. |

## AI setup and generation

| Case | Status | Actions and expected result |
| --- | --- | --- |
| A01 | PASS | Open fresh AI setup. Follow the visible path from connection to caption model without duplicate task choices. |
| A02 | PASS | Browse caption models, search, and select one. The active task updates; unrelated tasks remain unchanged. |
| A03 | FIXED | Open an empty or failed catalogue. Recover through custom model entry without closing all settings. |
| A04 | PASS | Browse embedding models. Inspect image/text capability guidance and prices before selecting a model. |
| A05 | PASS | Set two caption models to different concurrency limits. Switch between them, reopen settings, and restart. |
| A06 | PASS | Enable batch processing, choose its model, then disable it. Ordinary jobs name the caption model and request limit. |
| A07 | FIXED | Re-enable batch processing. Preserve its saved choice and explain any missing native batch requirements. |
| A08 | PASS | Select one embedding model for both indexes. Open generation with image and caption sources checked. |
| A09 | PASS | Enable a separate caption embedding model, then disable it. Confirm both indexes use the shared model again. |
| A10 | PASS | Select two sheets, apply a search filter, and open generation. The confirmation clearly identifies the actual scope. |
| A11 | PASS | Generate both indexes with the local fixture. Inspect progress, completion, and separate stored vectors. |
| A12 | PASS | Cancel the first index during a delayed reply. The second index must not start afterward. |
| A13 | PASS | Change the embedding model after an index exists. Inspect replacement guidance and cancel without losing the index. |
| A14 | PASS | Trigger a local label failure, then retry. Preserve the old label until success and clear stale error text. |
| A15 | PASS | Start a small local concurrent job. Change settings while it runs; the existing job retains its captured model. |
| A16 | PASS | Complete a budgeted live single-sheet label and a tiny multi-sheet job. Inspect saved labels, usage, and errors. |

## Transfers, search, and editing

| Case | Status | Actions and expected result |
| --- | --- | --- |
| W01 | PASS | Drag a gallery sheet into a nested project folder. Verify destination, metadata, and unchanged source bytes. |
| W02 | PASS | Repeat the transfer into the same folder. Preserve both files with clear collision-safe names. |
| W03 | PASS | Start a drag, cross workspace boundaries, then press Escape. No file, pixel, or selection change remains. |
| W04 | PASS | Drop a complete sheet onto the canvas, then immediately Undo and Redo. Focus follows the edited project. |
| W05 | PASS | Import a whole sheet while the open canvas has unsaved edits. Preserve the canvas and its undo history. |
| W06 | PASS | Filter Without tags, then generate a label. Gallery and tree update consistently without stale counts. |
| W07 | PASS | Combine word search and Without tags, then clear each separately. Counts and visible sheets remain consistent. |
| W08 | FIXED | Compare word, image, and caption search with saved fixture indexes. Show missing or incompatible indexes clearly. |

## Layout, export, and completion

| Case | Status | Actions and expected result |
| --- | --- | --- |
| F01 | PASS | Resize to 820 x 600 with painting controls open. Reach all tools, palette controls, and Apply without overlap. |
| F02 | PASS | Resize AI setup and its model browser. Reach search, selection, custom entry, and close controls. |
| F03 | FIXED | Open Activity during a job, after failure, and after completion. Inspect scope, progress, and stale spinners. |
| F04 | PASS | Export mixed-depth output to an isolated destination. Verify dimensions, palette data, and unchanged source pixels. |
| F05 | PASS | Open export again and cancel. Confirm no extra file and no blocked app controls. |
| F06 | PASS | Review screenshots and logs. Record clipping, unclear language, flicker, and missing feedback, including minor issues. |
| F07 | PASS | Retest every fixed issue with its original action sequence. Record the new screenshot and relevant commit. |
| F08 | PASS | Stop private test processes. Verify final files, commit contents, build, and cumulative live test cost. |

## Findings

The completed [AI pass](playtest-ai-results.md) records A01-A15, F02, and F03, with screenshots and native fix retests.
The completed [workflow pass](playtest-workflow-results.md) records W01-W08, including the search guidance and scope fixes.

A16 passed through native release UI actions: one live single-sheet caption and a two-sheet concurrent library job.
The [live validation report](live-ai-validation.md) records the saved labels, request counts, and budget.

The [pinning and export pass](playtest-pin-results.md) records P01-P24, layout, and export checks.
All 56 cases have results: 49 PASS and seven FIXED after native retests.
No case remains planned, blocked, or excluded. Provider-native batch jobs and object storage were outside the live test scope.
Each report records confusing interactions as findings, even when the application did not crash.

## Final verification

The full suite passed 321 tests, with four intentionally ignored. Clippy passed for all targets.
The release build passed with the locked dependency set. Its source matches commit `eb124e5`.
The private displays and helper processes stopped. Isolated live credentials were removed.
Original fixture image pixels stayed unchanged. The user library, project, and settings were not modified.
Conservative live accounting totals USD 0.11211586, approximately EUR 0.09949, within the EUR 1 limit.

Achievement commits: `1147f7a` adds pinning; `e9b367a` fixes search guidance; `7921378` fixes AI guidance and retry progress.
Commit `eb124e5` fixes group pins, grid typing, inspection protection, and moving Apply controls.
The linked reports contain screenshot names, failure descriptions, and native retest evidence.
