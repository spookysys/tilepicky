# AI interface rework plan

Status: implemented and reviewed with automated tests and native screenshots.
Delivery checks are recorded with the commit and installation report.
Baseline: commit `6d68605`.

## Result

A sheet has one dialog for its label, request, and log.
A library has one side panel for its labeling job and shared options.
The status bar opens the active job's dialog or panel.
Both scopes support Gemini and OpenAI-compatible providers.

## Entry points and ownership

| Entry point | Action | Owner of the result and log |
| --- | --- | --- |
| Source header: AI label... | Open the selected sheet's dialog without sending a request | That sheet's dialog |
| Sheet and file-tree context menus: Label with AI... | Open that sheet's dialog without sending a request | That sheet's dialog |
| Library AI panel | Start, monitor, pause, resume, or cancel a library job | Library panel |
| Active job in the status bar | Reopen its dialog or panel | The existing job |

Remove the duplicated Single sheet section from the side panel.
Keep generation and removal controls in the sheet dialog.
Both context menus have one AI item that opens that dialog.
Keep the existing AI panel shortcut and update its tooltip and shortcut legend.
The source header action must have a text label and keyboard focus.

## Single-sheet dialog

Identify the target by its library and relative path, independently of the current selection.
Opening another sheet must not redirect an existing request, its result, or its log.
The dialog must work when reopened from the status bar after the selected library changes.
If its target no longer exists, show that fact and keep the available diagnostics.
Before saving a result, check that the image and saved label still match the submitted versions.
A fixed path alone does not protect against edits to the file while a request runs.
Keep newer local changes and report a conflict instead of silently overwriting them.

Show the saved caption and tags first when a label exists.
Show which provider and model created that saved label.
Separately identify the model that a new request will use.
Keep prompt details behind Current prompt... and identify the preview as current, not historical.
The current preview includes the target sheet's filename context and the tag list for the next request.
The request log remains the source for the exact historical request.
Show the current tag list in a compact summary with an Edit library options link.
That link opens the shared options for the dialog's library. Do not duplicate the options form.
Opening the dialog or following a settings link must never send a request.

| State | Main action | Other information and actions |
| --- | --- | --- |
| No label | Label this sheet | Next model, Close; Copy log disabled if unavailable |
| Saved label | Label again | Saved result and model, Remove label..., Copy log |
| Request running | Cancel request | Progress, elapsed time, request model, Copy log, Close |
| Request failed | Retry | Error, retained label if present, Copy log, Close |
| No usable result | Retry | Explain that the model could not identify the image; retain any prior label |
| Another job occupies the slot | Label action disabled | Reason and Open current job |
| Missing model or key | Label action disabled | Specific setup explanation and a link to the relevant settings |

Closing the dialog or pressing Escape closes the view only. The request continues.
Cancellation must be explicit. Explain that an accepted request can still incur charges.
A failed, cancelled, or unlabelable replacement must not erase an existing usable label.
Keep the latest outcome attached to its target during the session.
Keep primary actions and Copy log visible while long results scroll.
Return keyboard focus to the entry point when the dialog closes.

## Library panel

Move the panel outside the source pane so it uses the available window height.
Keep a compact job summary and action area visible. Scroll sheet errors, provider details, and options below it.
Do not make the entire statistics table fixed. It would crowd out the controls in a small window.
Verify this arrangement in the native UI before completing the provider changes.
The canvas must not reduce the space available for job controls.

When idle, show the library, its label coverage, and its configured model.
Use Label unlabeled sheets as the primary action.
Keep Rerun all and Clear all labels as secondary actions with explicit confirmation.
State what each action replaces or removes. Retain existing labels when replacement requests fail.
Show the actual sheet count before sending the job.

When a job exists, show its own library, provider, model, and execution method.
After a library switch, identify the active job's library explicitly. Do not imply that it belongs to the newly opened library.
Shared options must identify their target library too. Bind every options action to that explicit target.
Use Google batch for remote batch work and One sheet at a time for sequential requests.
Library is the scope. The execution method is a separate fact.
Do not expose the internal :batch model suffix as a user-facing model name.

Keep Pause uploads or Pause, Resume, Cancel job, and Copy log beside the job summary.
Show Retry failed sheets only when applicable.
Keep technical recovery details behind Details. Normal recovery must not require a provider batch ID.
Keep the duplicate global Settings button removed.

Put shared tag options under Options for new labels.
Explain that these options apply to both scopes, but do not change an existing job.
Do not add editable prompt settings as part of this rework.

## Status and feedback

| Evidence | Main message |
| --- | --- |
| Local images remain to send | Uploading sheets, or Labeling sheets for sequential requests |
| Provider confirms pending | Queued at the provider |
| Provider confirms processing | Processing at the provider |
| Provider state is mixed or unknown | Waiting for results from the provider |
| Submission outcome is uncertain | Checking whether the provider accepted the upload |
| Transient failure | A short error and the next retry time |
| Paused Google uploads | Uploads paused; accepted work can continue |
| Results received but not saved | Saving labels, or a persistent save error |
| Terminal job | Saved, failed, unlabelable, and cancelled counts |

Do not infer processing merely because a provider accepted a job.
Keep the last provider response and next check time in secondary text.
A check countdown is not an estimate of completion time.
Saved, failed, unlabelable, and cancelled counts must be mutually exclusive and account for every terminal sheet.
Count the outcome of this attempt separately from the number of sheets that already have usable labels.
For example, an unlabelable rerun can retain an old label without counting as a new saved label.
Do not retain an old failure as the headline after a later successful attempt.

The status bar shows the active job when no temporary notice takes priority.
Single-sheet status opens its target dialog. Library status opens the library panel.
Errors remain available in the job view after a temporary status message disappears.
Both Copy log buttons show a neutral Copied confirmation beside the button.
A copy failure stays visible beside that action. Reserve error colors for errors.
Give primary actions, secondary actions, and disabled actions distinct visual treatment.
Use text as well as color to communicate state.

## Provider support and compatibility

Extract shared Gemini request conversion and response parsing from the batch-only module.
Use them for ordinary single-sheet requests and for batch results.
Keep one shared prompt, schema, tag validation, filename context, and redaction path.
Preserve provider errors, blocked responses, response limits, missing candidates, and invalid structured replies in the log.
The UI must explain these failures without showing raw JSON by default.

The single-sheet action must accept configured Gemini providers.
The model selector and the request dispatcher must agree about supported providers.
Use Single sheet and Library as the names of model defaults in Settings.
Keep old settings readable, including models stored with the :batch suffix.
Avoid a broad settings-schema rewrite unless the implementation requires it.

Offer gemini-flash-latest for new Google single-sheet and library configurations.
Migrate the previously shipped Google default as requested, while preserving custom models and provider keys.
Settings do not record whether an unchanged shipped model was deliberately selected. Do not claim the migration can detect that intent.
Keep every existing job's provider, model, prompt, tags, and file-context policy unchanged.
Record the resolved model version in the log when the provider supplies it.

Google documents the latest alias as a moving target that can include preview or experimental releases.
Do not describe the alias as a pinned stable model.
Verify request and batch compatibility against the official API documentation during implementation.

Sources:

- [Google model aliases](https://ai.google.dev/gemini-api/docs/models#latest)
- [Google image requests](https://ai.google.dev/gemini-api/docs/image-understanding)
- [Google structured outputs](https://ai.google.dev/gemini-api/docs/structured-output)

## Job and log rules to preserve

Only one labeling job occupies the app's active slot, whether single-sheet or library-wide.
This controls jobs managed by Tilepicky. It cannot promise that a cancelled request has stopped on the provider's server.
A paused library job still occupies that slot.
Switching the selected sheet, library, or model must not start another job or redirect the active one.
Keep at most one outstanding library job in that library's tilepicky.json.
Keep the existing library ownership lock so another window cannot write that job concurrently.
Do not replace the existing coordinator or automatically resend uncertain submissions.

Each library retains one current log file for either job type.
A new job replaces it. Closing the app removes it only after the job has finished.
An unfinished library job and its log survive restart.
An interrupted single-sheet request remains diagnosable and is not automatically resent.
Copy log is disabled when the matching log is unavailable or has been replaced.
The sheet dialog copies its sheet job's log. The library panel copies its library job's log.
Keep the existing size limit, redaction, and protection against late worker writes.

## Implementation sequence

1. Add regression tests for fixed targets, close versus cancel, stale results, retained labels, and accurate provider states.
2. Implement explicit dialog targets and status-bar navigation without changing job storage.
3. Build the dialog and full-height library panel with local fixtures. Inspect small-window screenshots before proceeding.
4. Add state-aware actions, clipboard feedback, and explicit completion summaries. Recheck the affected screenshots.
5. Extract shared Gemini conversion and add single-sheet transport support with fake responses.
6. Update provider defaults and settings compatibility tests. Keep accepted jobs on their saved configuration.
7. Update README, the shortcut legend, module descriptions, and test-drive documentation.
8. Run automated checks and the complete private native UI pass. Correct and recheck any findings.
9. Commit named files, inspect the commit, push, and verify the remote revision.
10. Build and install the tested revision. Verify the installed binary and report whether the running app needs a restart.

Primary files: src/main.rs, src/dialogs.rs, src/labels.rs, src/ai.rs, src/batch.rs, and src/ai_log.rs.
A small shared Gemini module can replace duplicated conversion code.
Change the coordinator and storage only where tests show a required behavior is missing.
Keep these changes incremental. Do not introduce a general job framework, job history, or another queue.
Keep the existing log lifecycle and settings format unless a specific requirement makes a change necessary.

## Validation

The automated suite passes 170 tests in each build configuration. Clippy passes for all targets in both configurations.
The interactive fixture remains excluded from automatic tests and was run separately.

| Area | Evidence |
| --- | --- |
| Fixed targets and navigation | Regression tests and native selection, close, and status-bar checks |
| Changes during a request | Regression tests reject changed images and changed saved labels |
| Existing labels | Failed, cancelled, and unlabelable native retries retain the previous label |
| Shared options | Explicit-root tests and native options and prompt views |
| Provider support | Loopback transport tests and native requests in both provider formats |
| Settings | Migration and custom-model preservation tests |
| Job ownership and restart | Existing coordinator tests and a paused native job reopened with its provider ID and log |
| Progress | Native queued, processing, connection failure, partial completion, retry, and completion views |
| Terminal counts | A regression checks all four outcomes without double counting |
| Logs | Lifecycle and redaction tests, native feedback, restart preservation, and deletion after completion and exit |
| Small windows | Native 1000 x 700 and 1200 x 900 views, including 125 percent text scale |
| Overflow | A failing regression exposed toolbar overlap; bounded toolbars now pass and native screenshots confirm the fix |
| Prompt preview | The long preview scrolls and keeps Close visible in the small window |
| Entry points | The source header and both context menus open the same dialog without sending a request |
| Keyboard | Native Escape, Tab, Shift+Tab, and the panel shortcut; automated focus and modal checks |
| Clear and retry | Native confirmation, label removal, and retry of a failed batch sheet |

The existing coordinator suite covers uncertain submissions, throttling, cancellation failures, concurrent writers, and failed saves.
This pass used copied game art, isolated settings, and a local fake provider. It sent no paid requests.
Live Google and OpenRouter behavior was not retested. The README recording still shows the earlier layout and says so.
See [the test-drive record](test-drive.md) for the implementation pass.

## Preparation completed

The source review and screenshot review are complete for the baseline.
The review found hidden log controls, inaccurate queued status, red success feedback, weak action hierarchy, and unclear recovery actions.
This plan includes those findings and the agreed separation of single-sheet and library work.
The implementation and native review described above followed this preparation.

## Review adjustments

The subsequent menu review consolidates the context-menu actions into one dialog entry.
Generate and remove labels from that dialog.
Keep shared options reachable from the sheet dialog after removing the duplicated side-panel controls.
Protect single-sheet results from intervening image or label edits, using the same principle as the existing batch save guard.
Keep attempt outcomes separate from existing label coverage.
Check the compact fixed controls in screenshots early, before committing to the rest of the layout.
Preserve the current coordinator and storage. This is a focused interface and provider extension, not a replacement job system.
