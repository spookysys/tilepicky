# Native AI workflow play-test

Date: 2026-10-07.

This pass used a new synthetic library, fresh settings, and a private Xvfb display.
A01-A15 and F02/F03 used a loopback provider with synthetic keys.
A later native release pass closed A16 with three bounded live requests, as recorded below.
The source PNG files match the untouched fixture copy byte for byte.

Screenshot names below refer to the local `private/playtest-ai/shots/` evidence directory.
The driver records each mouse and keyboard sequence in numbered command files.

| Case | Result | Observed result and evidence |
| --- | --- | --- |
| A01 | PASS | Provider connections precede one caption task picker. There is no second task assignment section. `02-ai-setup.png`. |
| A02 | PASS | Refreshed the catalogue, searched for recent, and selected the matching model. The embedding model stayed unchanged. `06-catalog-filter.png`, `07-caption-selected.png`. |
| A03 | FIXED | HTTP 503 retained cached choices. Custom model entry recovered without closing settings. Error advice now offers retry or custom entry. `08-catalog-failure-custom.png`, `11-custom-selected.png`, `65-catalog-error-fixed.png`. |
| A04 | PASS | The image embedding catalogue excluded the text-only fixture model. Prices and the shared image/text requirement were visible. Selected the visual model. `13-embedding-catalog.png`, `14-embedding-selected.png`. |
| A05 | PASS | Set caption-a to two requests and caption-b to four. Both limits survived switching, closing settings, and restart. Saved JSON confirms both. `16-model-b-four.png` through `20-restart-b-four.png`. |
| A06 | PASS | Disabling batch processing used caption-b for concurrent requests. The running job showed its limit of four. The confirmation named the model. `24-ordinary-confirm.png`, `59-job-old-model.png`. |
| A07 | FIXED | The batch model survived disabling and re-enabling. Unsupported native batches disabled generation and explained the provider requirement. Removed the incorrect extra request to configure an already configured caption model. `61-batch-unavailable-feedback.png`, `77-batch-enabled-retained.png`, `78-batch-guidance-fixed.png`. |
| A08 | PASS | Checked image and caption sources together. Confirmation showed the same visual model for both indexes. `34-selected-filter-scope.png`, `37-embedding-confirm.png`. |
| A09 | PASS | Enabled a separate caption embedding model and selected embed-alternate. Disabling the override restored the shared model for both sources. `32-separate-alternate.png`, `37-embedding-confirm.png`. |
| A10 | PASS | Selected two sheets, filtered to one visible match, then reviewed generation. Review scope retained both names and confirmation counted two. `35-embedding-confirm.png`, `37-embedding-confirm.png`. |
| A11 | PASS | Generated both sources. Activity reported each completed index. Each JSON store contained two vectors under the shared model. `38-embedding-running.png`, `39-embedding-activity.png`. |
| A12 | PASS | Cancelled the image request before its delayed reply. No caption request followed. The mock recorded only one image call for embed-fixture. Both previous indexes remained. `48-first-index-cancelled.png`, `49-no-second-index.png`. |
| A13 | PASS | A changed model produced explicit replacement guidance for both indexes. Cancel returned without starting a request. Later cancellation also preserved existing indexes. `41-replacement-guidance.png`, `42-replacement-cancelled.png`. |
| A14 | PASS | A local label failure preserved the previous label. Retry succeeded and removed the error text from the dialog and status bar. `51-label-error-old-retained.png`, `52-retry-success.png`. |
| A15 | PASS | Started two concurrent caption-b requests, then selected caption-a. The job retained caption-b and its limit of four. Activity explained the distinction. `58-change-caption-during-job.png`, `59-job-old-model.png`, `60-job-finished-new-default.png`. |
| A16 | PASS | Native release UI clicks saved one live caption, then two concurrent library captions. Activity showed 2/2 saved and zero failures. Labels survived restart. Evidence: `private/live-native-ai/shots/02-single-result.png`, `06-two-complete.png`, `10-houses-saved-offline.png`, `11-pines-saved-offline.png`, and `12-usage-offline.png`. See `live-ai-validation.md` for costs and limits. |
| F02 | PASS | At 820 x 600, both settings windows remained usable. Search, selection, custom entry, scroll, and close controls were reachable. `62-narrow-ai-setup.png` through `68-layout-closed.png`. |
| F03 | FIXED | Transient failures previously appeared finished while waiting for retry. The corrected display shows zero finished, two remaining, and a retry countdown. Recovery ends with two saved and no spinner. `71-activity-provider-failure.png`, `74-retry-progress-fixed.png`, `75-retry-completed.png`. |

## Findings and fixes

- Caption custom-entry help incorrectly discussed embedding inputs. It now describes image input and structured label output.
- The shortlist described every suggestion as stable, even when a preview appeared. It now describes stable models as a preference.
- Catalogue HTTP 503 advice incorrectly emphasized the API key. Authentication failures retain key advice; other HTTP failures suggest retry or custom entry.
- Unsupported native batch settings also showed a misleading generic model setup message. Each missing prerequisite now has its own guidance.
- Temporary request failures counted as completed sheets. Only terminal failures now contribute to finished counts.
- Ordinary requests waiting for retry said uploads were interrupted. They now say labeling was interrupted.
- While every unfinished request awaited retry, the activity line still showed an active labeling spinner. It now shows the retry countdown.

The catalogue fixes passed native retests in `64-guidance-fixed.png` through `67-narrow-custom-controls.png`.
The batch setup fix passed in `78-batch-guidance-fixed.png`.
The progress fix passed its original failure sequence and automatic recovery in screenshots 74 and 75.

The pass also found misleading scope wording when no checkbox selection remained.
A filtered view with one visible sheet opened Improve search for the whole library, labeled as selected sheets.
Screenshot `53-scoped-tag-job.png` records this separate finding. The workflow test pass owns its fix and retest.

## Verification limits

The first cancellation attempt reached the second source before the click. It does not count as the A12 pass.
The repeated attempt cancelled the first source promptly, then waited for its delayed reply before checking the mock log.

A coordinate error opened the clear-label confirmation. Cancel closed it; no labels were removed.
This does not count as an intentional clear-label workflow test.

The catalogue regression subset passed 11 tests. The batch subset passed 47 tests, with two paid tests ignored.
A new regression checks pending retry counts, retry waiting, and terminal failure counts.
The existing terminal-outcome fixture now marks its failed sheet terminal explicitly.

The native log contained no panic. Private test processes were stopped after the pass.
No application settings or provider credentials outside the isolated fixture were changed.
