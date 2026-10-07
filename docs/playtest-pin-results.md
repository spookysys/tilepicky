# Tile depth and export play-test

Date: 2026-10-07.

This pass used generated sheets, isolated settings, and native mouse and keyboard actions on a private display.
The first interaction checks used the test build. Preview and export checks used the release build.
Screenshot names refer to the local `private/playtest-pin/shots/` evidence directory.
Numbered command files record the input sequences. No AI requests ran during this pass.

| Case | Result | Observation and evidence |
| --- | --- | --- |
| P01 | PASS | Mixed depth exposes Force 4bpp, Force 8bpp, and Auto beneath Pin tile depth. Screenshot 03. |
| P02 | PASS | A forced 4bpp tile has a blue outline and a 4 badge. Exported shallow tiles use at most 15 visible colors. Screenshots 04 and 46. |
| P03 | PASS | Forced 8bpp uses a yellow outline and an 8 badge. Automatic tiles have neither badge. Screenshots 05 and 47. |
| P04 | PASS | One held stroke filled every crossed tile in a row. Screenshot 06. |
| P05 | PASS | A diagonal stroke filled its intermediate cells. It did not alter adjacent rows outside the stroke. Screenshot 10. |
| P06 | PASS | Leaving the canvas and releasing stopped painting. Returning without a button held made no further change. Screenshot 10. |
| P07 | PASS | Auto removed both forced depths. It left source pixels unchanged. Screenshot 11 and the source image checks. |
| P08 | PASS | Right-click removed pins with either force tool selected. No conflicting menu appeared. Screenshot 12. |
| P09 | PASS | Right-drag removed the crossed pins continuously. Screenshot 12. |
| P10 | PASS | The next left click used the selected force tool again. Screenshot 12. |
| P11 | PASS | At 100% automatic allocation, the forced 4bpp tile remained shallow. The result contained 47 deep tiles. Screenshot 21. |
| P12 | PASS | All 48 tiles remained forced 4bpp at 100% automatic allocation. The deep count was zero. Screenshot 24. |
| P13 | PASS | Three forced 8bpp tiles remained deep at zero automatic allocation. The count explains that it includes forced tiles. Screenshots 25 and 47. |
| P14 | PASS | Zooming preserved tile identity. Painting the outer rows and columns at 1x reached the intended tiles. Screenshots 19-25. |
| P15 | FIXED | Escape restored selection, copy, delete, and pixel Undo. Inspection mode now blocks pin changes. Screenshots 48-49, 60-61. |
| P16 | PASS | Undo removed one complete row stroke; Redo restored it. Source pixels stayed unchanged. Screenshots 07-08. |
| P17 | PASS | Apply retained both depths across sheet changes. Saved metadata matched the displayed pins. Screenshots 15 and 41-42. |
| P18 | PASS | Restart retained the saved forced depths and their preview. Screenshots 17-18 and 47. |
| P19 | PASS | The legacy fixture retained its three boolean 8bpp pins. Other tiles remained automatic. Screenshot 14. |
| P20 | PASS | Fixed depth hid the painting tools and overlays. Returning to mixed depth restored the pins. Screenshots 27-28. |
| P21 | PASS | A folder group retained independent pins on 128x96 and 64x64 sheets. Metadata contains 48 and 16 entries respectively. Screenshots 41-42. |
| P22 | FIXED | A separate target previously reset settings and pins. The retest retained mixed depth, palette settings, and the 4bpp pin. Screenshots 44 and 52-54. |
| P23 | PASS | Clearing one sheet left the other member's saved group, preview, and 8bpp pin unchanged. Screenshots 55 and 65. |
| P24 | FIXED | The popup stays open while typing. Changing from 16 to 8 pixels clears obsolete pins and explains the change. Screenshots 59 and 62-64. |
| F01 | PASS | At 820x600, the controls scroll. Tools, Apply, export, and palettes remain reachable beside the canvas. Screenshots 29-31. |
| F04 | PASS | Native export produced a 128x96 indexed PNG with a 256-entry SNES palette and transparent index zero. Screenshots 45-46. |
| F05 | PASS | Reopening export and cancelling left one output file and restored usable app controls. Screenshot 47. |

## Findings

The Apply row moved vertically when the deep-tile count appeared after preview completion.
A click at its previous position could miss the button. The count now occupies a row while computation runs.
Screenshots 34 and 38 show the old positions. Screenshots 40-43 show the stable row after the fix.

Choosing one member as a separate quantization target previously reset the working settings and pins.
The target now inherits the open member's saved settings and pins before the replacement confirmation.
Other members retain their settings and pins. A regression test checks both the split and a later clear.
The confirmation uses Replace settings and Cancel, and explains which sheets retain their settings.

The grid menu closed when its value button switched to a text field.
The same popup contains library tile size, gap, and offset fields. The popup now closes only on an outside click.
The project popup explains that changing tile size clears saved depth pins.

Inspection mode previously allowed a selected pin tool to change overrides.
The sheet now blocks pin events in inspection mode. Keyboard pin Undo also respects that mode.

## Image checks

The mixed export contains three forced deep tiles in its first row.
Those tiles contain 43, 44, and 41 colors, including any transparent pixels.
The remaining first-row tiles contain 11, 12, 14, 15, and 15 colors.
All 45 forced shallow tiles passed the 15-color limit check.

The export did not modify any project image.
The ordinary delete, Undo, and Save sequence restored the original pinboard pixels exactly.
Saving can change PNG encoding, so this comparison uses decoded RGBA pixels.

The dense fixture was slow in the unoptimized test build. The release build completed its previews without blocking input.
Private display watchdogs stopped expired sessions. Pending commands did not count as executed tests.
The final suite passed 321 tests, with four ignored. Clippy passed for all targets.
The initial final-suite attempt lacked loopback socket permission. The rerun allowed the local test servers.

Library tile size, gap, and offset accepted typed values in the same popup (66-67).
The saved values were 8 pixels, 1 pixel, and 2 pixels respectively.

The updated replacement dialog passed Cancel and Replace settings (69-71). The other sheet retained its 8bpp pin.
The final 820x600 retest kept the tools and Apply row reachable (72).
All original library and project images retain their decoded RGBA pixels.
No panic appeared in the reviewed native logs. The private pinning display and its processes stopped cleanly.

The feature commit is `1147f7a`. The group, grid, inspection, and stable-control fixes are in `eb124e5`.
