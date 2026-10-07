# Native workflow play-test

Date: 2026-10-07.

This is a new pass after the earlier acceptance checks. Every case starts as PLANNED.
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
| P01 | PLANNED | Open mixed 4bpp + 8bpp controls. Find Force 4bpp, Force 8bpp, and Auto without reading source code. |
| P02 | PLANNED | Force one colorful tile to 4bpp. Inspect its outline, preview, and palette limit. |
| P03 | PLANNED | Force another tile to 8bpp. Distinguish its outline from forced 4bpp and automatic depth. |
| P04 | PLANNED | Paint a row while holding the mouse button. Every crossed tile receives the chosen override. |
| P05 | PLANNED | Drag diagonally across tile corners. Inspect missed cells, unexpected neighbors, and repeated changes. |
| P06 | PLANNED | Drag out of the canvas and release. Return without a button held; painting must stop. |
| P07 | PLANNED | Select Auto and paint both forced depths. Remove their overrides without changing source pixels. |
| P08 | PLANNED | Right-click forced tiles with each force tool selected. Remove overrides without opening a conflicting menu. |
| P09 | PLANNED | Hold the right mouse button and drag across forced tiles. Inspect whether removal matches the displayed instructions. |
| P10 | PLANNED | Return to the left mouse button after right-click removal. The selected force tool still works. |
| P11 | PLANNED | Change the automatic 8bpp allocation after forcing both depths. Forced tiles retain their explicit choices. |
| P12 | PLANNED | Force all tiles to 4bpp, then increase automatic allocation. Automatic selection must not override them. |
| P13 | PLANNED | Force several tiles to 8bpp with a smaller automatic allocation. Preserve them and explain the resulting count. |
| P14 | PLANNED | Zoom in and out, then paint at canvas edges. Overrides follow the tile under the pointer. |
| P15 | PLANNED | Switch between tile painting and ordinary selection. Selection, copy, and delete must not leave painting active. |
| P16 | PLANNED | Paint overrides, then press Undo and Redo. Record the behavior and check that unrelated pixels remain safe. |
| P17 | PLANNED | Apply settings, switch sheets, and return. Both forced depths and Auto retain their saved meaning. |
| P18 | PLANNED | Restart after applying overrides. Compare outlines, preview, counts, and saved metadata with the earlier state. |
| P19 | PLANNED | Open a fixture with legacy boolean pins. Existing 8bpp pins remain forced; other tiles remain automatic. |
| P20 | PLANNED | Switch mixed depth to fixed depth and back. No stale painting controls or unexplained override loss appear. |
| P21 | PLANNED | Apply a folder group across two differently sized sheets. Overrides stay attached to the correct sheet and tile. |
| P22 | PLANNED | Move one sheet into a separate quantization group. Check both previews and retained overrides. |
| P23 | PLANNED | Clear quantization on one grouped sheet. Inspect the other sheet for accidental setting or override changes. |
| P24 | PLANNED | Change the grid after pinning tiles. Verify invalid overrides cannot silently attach to unrelated tiles. |

## AI setup and generation

| Case | Status | Actions and expected result |
| --- | --- | --- |
| A01 | PLANNED | Open fresh AI setup. Follow the visible path from connection to caption model without duplicate task choices. |
| A02 | PLANNED | Browse caption models, search, and select one. The active task updates; unrelated tasks remain unchanged. |
| A03 | PLANNED | Open an empty or failed catalogue. Recover through custom model entry without closing all settings. |
| A04 | PLANNED | Browse embedding models. Inspect image/text capability guidance and prices before selecting a model. |
| A05 | PLANNED | Set two caption models to different concurrency limits. Switch between them, reopen settings, and restart. |
| A06 | PLANNED | Enable batch processing, choose its model, then disable it. Ordinary jobs name the caption model and request limit. |
| A07 | PLANNED | Re-enable batch processing. Preserve its saved choice and explain any missing native batch requirements. |
| A08 | PLANNED | Select one embedding model for both indexes. Open generation with image and caption sources checked. |
| A09 | PLANNED | Enable a separate caption embedding model, then disable it. Confirm both indexes use the shared model again. |
| A10 | PLANNED | Select two sheets, apply a search filter, and open generation. The confirmation clearly identifies the actual scope. |
| A11 | PLANNED | Generate both indexes with the local fixture. Inspect progress, completion, and separate stored vectors. |
| A12 | PLANNED | Cancel the first index during a delayed reply. The second index must not start afterward. |
| A13 | PLANNED | Change the embedding model after an index exists. Inspect replacement guidance and cancel without losing the index. |
| A14 | PLANNED | Trigger a local label failure, then retry. Preserve the old label until success and clear stale error text. |
| A15 | PLANNED | Start a small local concurrent job. Change settings while it runs; the existing job retains its captured model. |
| A16 | PLANNED | Complete a budgeted live single-sheet label and a tiny multi-sheet job. Inspect saved labels, usage, and errors. |

## Transfers, search, and editing

| Case | Status | Actions and expected result |
| --- | --- | --- |
| W01 | PLANNED | Drag a gallery sheet into a nested project folder. Verify destination, metadata, and unchanged source bytes. |
| W02 | PLANNED | Repeat the transfer into the same folder. Preserve both files with clear collision-safe names. |
| W03 | PLANNED | Start a drag, cross workspace boundaries, then press Escape. No file, pixel, or selection change remains. |
| W04 | PLANNED | Drop a complete sheet onto the canvas, then immediately Undo and Redo. Focus follows the edited project. |
| W05 | PLANNED | Import a whole sheet while the open canvas has unsaved edits. Preserve the canvas and its undo history. |
| W06 | PLANNED | Filter Without tags, then generate a label. Gallery and tree update consistently without stale counts. |
| W07 | PLANNED | Combine word search and Without tags, then clear each separately. Counts and visible sheets remain consistent. |
| W08 | PLANNED | Compare word, image, and caption search with saved fixture indexes. Show missing or incompatible indexes clearly. |

## Layout, export, and completion

| Case | Status | Actions and expected result |
| --- | --- | --- |
| F01 | PLANNED | Resize to 820 x 600 with painting controls open. Reach all tools, palette controls, and Apply without overlap. |
| F02 | PLANNED | Resize AI setup and its model browser. Reach search, selection, custom entry, and close controls. |
| F03 | PLANNED | Open Activity during a job, after failure, and after completion. Inspect scope, progress, and stale spinners. |
| F04 | PLANNED | Export mixed-depth output to an isolated destination. Verify dimensions, palette data, and unchanged source pixels. |
| F05 | PLANNED | Open export again and cancel. Confirm no extra file and no blocked app controls. |
| F06 | PLANNED | Review screenshots and logs. Record clipping, unclear language, flicker, and missing feedback, including minor issues. |
| F07 | PLANNED | Retest every fixed issue with its original action sequence. Record the new screenshot and relevant commit. |
| F08 | PLANNED | Stop private test processes. Verify final files, commit contents, build, and cumulative live test cost. |

## Findings

No observations are recorded yet. Add each finding with its case, visible symptom, severity, fix, and native retest evidence.
A confusing interaction is a finding even when the application does not crash.
