# Map recording verification

Date: 2026-10-28.

## Native workflow

The release app ran on an isolated Xvfb display with generated artwork, a private D-Bus session, and isolated settings.
No real project sheets or maps were used.
The final release executable was also launched and checked after the last source change.
The test processes were stopped after verification. The screenshot page remains open.

| Check | Observed result |
| --- | --- |
| Existing sheet, unknown map use | A swap waits at the Record / No maps / Cancel dialog. |
| Cancel | The sheet remains unchanged. |
| Record | The waiting swap completes and shows two changed references. |
| Save | The sheet and recording save. The map remains unchanged. |
| Reopen | The recording remains active and retains its count. |
| Update maps | The native picker selects the test map. The preview reports four affected references. |
| Update | The map changes from GIDs `1,2,1,2` to `2,1,2,1`. Its backup retains the original values. |
| Tiled | Tiled opens the updated map and displays the original artwork order. |
| Pixel check | Every painted map cell has identical RGBA pixels before and after the sheet swap and map update. |
| Clear | The confirmation explains that clearing the record does not undo edits or update maps. Cancel preserves it. |
| New sheet | No recording controls or warning appear on the canvas. Tiled settings remain in the side panel. |
| Stop recording | The next protected swap warns before applying. |
| No maps | Protection and recording turn off. The unrecorded swap marks the retained record incomplete. The map remains unchanged. |

The screenshots are retained locally in a browser evidence page. They are not part of the repository.

## Automated checks

`cargo test --locked`: 345 passed, 3 ignored.
The mock HTTP tests need loopback access. They passed outside the restricted sandbox.
`cargo clippy --locked --all-targets`: clean.
`cargo build --locked --release`: passed with the default OpenGL backend.

Regression tests cover recording composition, Undo and Redo, save/reopen, new-sheet defaults, copy defaults, resize, trim, and growth during paste.
Updater tests cover nested groups, chunks, tile objects, flip flags, backup contents, repeated updates, and retry after an interrupted write.
They also cover deleted references, compressed data, templates, range collisions, wrong tilesets, and changed map files.

## Tiled sidebar

The map controls and tile property editor share one Tiled side panel, opened with the tag button or `P`.
Native checks confirmed that the canvas has no separate Maps button or recording row.
Choosing Record in the warning opens the side panel and displays the change count beside the selected tile's properties.
The generated `terrain` property followed the recorded swap and remained editable in the same panel.
A new sheet shows an unchecked Used in maps setting, without recording controls.

The [user guide](../README.md#map-change-recording) defines the supported workflow.
[Open issues](issues.md#map-update-coverage) lists the unsupported cases and the checks needed to extend coverage.
