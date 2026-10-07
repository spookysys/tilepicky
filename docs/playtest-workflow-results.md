# Transfer and search workflow results

Date: 2026-10-07.

This pass used native mouse and keyboard actions on a private display with isolated settings.
The fixture contained eight generated library sheets and five project sheets. All AI requests used a local mock provider.
Screenshots remain in the private workflow evidence folder. No paid requests were made.

| Case | Result | Observation and evidence |
| --- | --- | --- |
| W01 | PASS | Dragged a gallery sheet into the nested group folder. The copied image, grid, label, and provenance matched the source (003-004). |
| W02 | PASS | Repeated the transfer. The second file used a copy suffix, and both files retained the original bytes (005). |
| W03 | PASS | Dragged across the workspace transition and pressed Escape. No second-sheet file or canvas change appeared (006). |
| W04 | PASS | Dropped a complete sheet on the blank canvas. Immediate Undo cleared the placement; Redo restored it (007-009). |
| W05 | PASS | Imported another file while the canvas had unsaved edits. The canvas remained open and dirty, with its undo history intact (010-012). |
| W06 | PASS | Enabled Without tags and labeled one sheet through the local provider. Gallery and tree removed it, and the count changed from four to three (013, 017-019). |
| W07 | PASS | Combined the nature query with Without tags. Two sheets matched; disabling that filter showed three. Clearing the query restored four unlabeled sheets (014-016). |
| W08 | FIXED | Generated both indexes and compared a word query. Changing the embedding model exposed indefinite waiting text. The retest shows the mismatch and a generation action (024-030). |

## Issues found and corrected

The comparison columns previously displayed an indefinite wait after the selected model changed.
No query could start because the stored vectors belonged to the previous model.
The columns now explain the model or provider mismatch and show the saved and selected model names.
The review action opens generation for the affected source. Missing indexes also have a review action.
Screenshots 028 and 029 show the failure and the corrected state. Screenshot 030 shows the working review action.

Improve search previously called the entire library "selected sheets", even with a query showing only one result.
The heading now identifies the entire library and explains that search filters do not limit this scope.
Checkbox selection still limits the scope. Screenshots 030 and 031 verify both paths.

## File checks

All eight library images retained their original bytes.
The three imported images matched their source files byte for byte.
The two labeled imports retained their labels, grid size, and provenance.
The unlabeled import retained its grid and provenance without gaining a label.
The canvas save preserved the placement restored by Redo.

The mock saved eight image vectors and five caption vectors through separate requests.
Semantic results used fixed mock vectors. This verifies the interaction, not real search quality.

Missing-index guidance and the caption-only review action passed a separate native retest (032-033).
