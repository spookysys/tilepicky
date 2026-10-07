# Live AI validation

Test date: 2026-10-07.

The module tests used two generated 128 x 128 PNG sheets with trees and houses.
The native UI pass used three synthetic sheets, with two tree sheets and one house sheet.
No user images, library metadata, private messages, or files went to a provider.
The tests did not change the saved AI configuration.

## Results

| Check | Provider and exact model | Result |
| --- | --- | --- |
| Two concurrent caption requests | OpenRouter, `google/gemini-2.5-flash` | Both passed the application label parser. Captions identified the trees and houses correctly. |
| Direct Google caption request | Google, `gemini-2.5-flash` | HTTP 404. Google said this model was unavailable to new users. |
| Current Google caption request | Google, `gemini-3.1-flash-lite` | Passed. The caption identified four coniferous trees with green canopies and brown trunks. |
| Image embedding | OpenRouter, `voyageai/voyage-multimodal-3.5` | Passed. The application image input builder produced one valid 1024-dimensional vector. |
| Two text embeddings in one request | OpenRouter, `voyageai/voyage-multimodal-3.5` | Passed. Both vectors contained 1024 dimensions. |
| Cross-modal retrieval | Same Voyage model | Tree image similarity was 0.533 with the tree description and 0.180 with the house description. |
| Model discovery | Google and OpenRouter | Both catalogues returned models. Google listed the unavailable 2.5 Flash model despite its inference failure. |

The OpenRouter captions described a white background, although the source PNG used transparency.
The object descriptions and tags were useful. These simple fixtures do not measure quality across a full sprite library.

An initial image probe used an incorrect array in the temporary test harness and received HTTP 400.
The corrected probe used `embed::jobs`, which already builds the proper object with a `content` array.
The application did not need an embedding format change.

## Coverage

The probes exercised `labels::request`, provider routing, label transport, Gemini conversion, and the application label parser.
The embedding probes exercised the application input builder, request builder, transport, and vector validator.
The first probes were direct application-module tests. A later native release UI pass tested the complete library job coordinator.
Neither pass tested native provider batch jobs, object storage, cancellation billing, or a large library.
A model catalogue entry does not establish access for an account. The real request can still fail.

## Native release UI validation

The release build included commit `eb124e5`. The app ran on a fresh private display with isolated configuration.
Actual UI clicks submitted one sheet, then a library job with two unlabeled sheets.
Both jobs used OpenRouter `google/gemini-2.5-flash`, with batch processing disabled and a concurrency limit of two.
The confirmation correctly skipped the first saved label. Its estimate was less than USD 0.01.
The two library requests started two milliseconds apart, before either response arrived.
Activity finished with two saved labels and no failures. All three sheets had usable captions and tags.

A restart without credentials confirmed the saved house and pine labels in the inspector.
The Activity panel retained the completed job and reported usage below USD 0.01.
The tree captions again described a white background despite source transparency.

Private screenshots in `private/live-native-ai/shots/` record the UI evidence:

- `02-single-result.png`: the saved single-sheet caption.
- `04-two-sheet-estimate.png`: two remaining sheets and one skipped label.
- `06-two-complete.png`: two saved labels and zero failures.
- `10-houses-saved-offline.png` and `11-pines-saved-offline.png`: labels retained after restart.
- `12-usage-offline.png`: the retained job and usage display.

The app, private display, and helper processes were stopped after inspection.
The isolated credentials were removed. The user configuration was unchanged.

## Cost and limits

The user allowed EUR 1 for all live tests. The initial operational ceiling was USD 0.40.
The later native pass reserved at most USD 0.15, with a cumulative ceiling of USD 0.26114746.
Every paid request used a fixed model ID. No request was retried. Search tools and object storage were unused.
Each caption request allowed 4096 output tokens. Preflight reserves also covered reasoning tokens and input images.
A file marker prevented each temporary probe from running twice accidentally.

Ten inference requests were sent: seven caption requests and three embedding requests.
The native pass sent exactly three requests, with no retries. A watchdog bounded its request count and duration.
Two requests failed before a useful result: the unavailable Google model and the malformed test harness input.

| Successful request | Input tokens | Output tokens | Reported or estimated USD |
| --- | ---: | ---: | ---: |
| OpenRouter trees | 492 | 78 | 0.00034260 |
| OpenRouter houses | 492 | 72 | 0.00032760 |
| Google trees, Flash-Lite | 1322 | 76 | 0.00044450 |
| Voyage image | 89 reported | n/a | 0.00003000 |
| Voyage text pair | 23 | n/a | 0.00000276 |
| Native UI single tree sheet | 517 | 73 | 0.00033760 |
| Native UI library houses | 517 | 54 | 0.00029010 |
| Native UI library pines | 519 | 74 | 0.00034070 |
| Total successful requests | | | 0.00211586 |

OpenRouter reported upstream costs for the captions, although its own charge was zero because the route used BYOK.
The total includes those upstream costs. Google's cost uses its returned token counts and published standard rates.
The embedding rows use the returned cost values.

The failed calls returned no usage. The accounting retains their full preflight reserves of USD 0.11.
The native pass added USD 0.00096840, including BYOK upstream costs.
Successful costs plus the failed-call reserves total USD 0.11211586, about EUR 0.09949.
This is a conservative accounting total, not an invoice amount.

The [ECB reference rate](https://www.ecb.europa.eu/stats/policy_and_exchange_rates/euro_reference_exchange_rates/html/index.en.html)
for 2026-10-06 was USD 1.1269 per EUR.

Rates checked before use:

- [OpenRouter Gemini 2.5 Flash](https://openrouter.ai/google/gemini-2.5-flash): USD 0.30 per million input tokens and USD 2.50 per million output tokens.
- [Google pricing](https://ai.google.dev/gemini-api/docs/pricing): Flash-Lite 3.1 costs USD 0.25 per million input tokens and USD 1.50 per million output tokens.
- [Voyage multimodal pricing](https://openrouter.ai/voyageai/voyage-multimodal-3.5): USD 0.12 per million text tokens and USD 0.60 per billion image pixels.
- [Google thinking limits](https://ai.google.dev/gemini-api/docs/generate-content/thinking) informed the conservative reasoning reserve.
- [OpenRouter embedding input format](https://openrouter.ai/docs/api/api-reference/embeddings/create-embeddings) defines multimodal input objects.

The temporary live test module was removed after validation. Normal tests cannot trigger these paid calls.
Private test artifacts retain the synthetic images, response data, price snapshots, and the budget record.
