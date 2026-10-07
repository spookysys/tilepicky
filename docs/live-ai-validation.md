# Live AI validation

Test date: 2026-10-07.

The tests used two generated 128 x 128 PNG sheets. One contained four trees; the other contained four houses.
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
Two caption transports ran concurrently. This did not test the complete library job coordinator against a live provider.

These were direct application-module tests, separate from the earlier native UI tests.
They did not test native provider batch jobs, object storage, cancellation billing, or a large library.
A model catalogue entry does not establish access for an account. The real request can still fail.

## Cost and limits

The user allowed EUR 1 for all live tests. The operational ceiling was USD 0.40.
Every paid request used a fixed model ID. No automatic retries, search tools, or object storage were enabled.
Each caption request allowed 4096 output tokens. Preflight reserves also covered reasoning tokens and input images.
A file marker prevented each temporary probe from running twice accidentally.

Seven inference requests were sent: four caption requests and three embedding requests.
Two requests failed before a useful result: the unavailable Google model and the malformed test harness input.

| Successful request | Input tokens | Output tokens | Reported or estimated USD |
| --- | ---: | ---: | ---: |
| OpenRouter trees | 492 | 78 | 0.00034260 |
| OpenRouter houses | 492 | 72 | 0.00032760 |
| Google trees, Flash-Lite | 1322 | 76 | 0.00044450 |
| Voyage image | 89 reported | n/a | 0.00003000 |
| Voyage text pair | 23 | n/a | 0.00000276 |
| Total successful requests | | | 0.00114746 |

OpenRouter reported upstream costs for the captions, although its own charge was zero because the route used BYOK.
The total includes those upstream costs. Google's cost uses its returned token counts and published standard rates.
The embedding rows use the returned cost values.

The failed calls returned no usage. The accounting retains their full preflight reserves of USD 0.11.
Successful costs plus these reserves total USD 0.11114746, about EUR 0.099.
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
