# Settings UX review

This review covers the AI controls in Settings. It uses both the code and native screenshots.
The ranked findings describe the initial state. The implementation review below records the changes inspected afterward.
The final review includes native interaction evidence supplied by the implementation agent.

## Evidence

The code review covered `settings_ui`, `providers_ui`, and `models_ui` in `src/ai.rs`.
It also covered `settings_popup` in `src/main.rs` and the processing descriptions in `src/batch.rs`.

The native screenshots showed these states:

- Settings opened with an OpenRouter Library model selected in the model editor.
- Settings scrolled down to reveal the Active models selectors.
- The model scope menu with Single sheet and Library choices.
- A Google Library model selected in the model editor.

At the captured scale, the first view hid the active model selectors below the visible area.
The provider fields occupied most of that area.
The Google model editor could appear below an OpenRouter provider editor, because the two selectors are independent.

## Ranked findings and proposals

### 1. The active choices are hard to find

**Finding:** Settings starts with provider internals. A user must scroll past both editors to choose the models that requests use.
The screenshot of the initial view shows only the Active models heading near the bottom.

**Proposal:** Put Active models first. Keep the Single sheet and Library choices together.
Put provider and model configuration below them.
Explain that these choices apply to new requests. An outstanding job keeps its saved configuration.

### 2. Library scope does not explain how processing works

**Finding:** The model editor says "One sheet at a time" for an OpenAI-compatible endpoint.
That text does not explain the consequence or offer a route to Google batches.
The active Library selector has no processing explanation.

**Proposal:** Show an amber notice below the active Library selector when it uses sequential requests.
Show the same notice when editing a Library model with that processing method.
Use this wording:

> One sheet at a time
>
> Tilepicky sends one image request at a time. Keep Tilepicky open while the library job runs.
>
> For Google to process sheets as a batch, configure Gemini and select it for Library.

Add a **Configure Gemini...** action that opens the relevant configuration.
Opening configuration must not change the active model or send a request.

For an OpenRouter endpoint, an optional detail can say:

> Tilepicky does not use OpenRouter's batch API for local images.

Keep URL restrictions out of the main notice. They do not help the user choose the next action.
Use amber information styling. Sequential requests work, so this is not a request failure.

For a Gemini Library selection, use a neutral explanation:

> Google batch
>
> Google processes uploaded sheets as a batch. Submitted work can continue after you close Tilepicky.

Do not imply that sheets still queued on the PC have reached Google.

### 3. Gemini configuration can retain the wrong endpoint

**Finding:** The provider editor changes the API kind without changing the old endpoint or environment defaults.
A newly added provider starts with the OpenAI defaults.
Changing only its kind to Gemini can therefore leave an incompatible endpoint.

**Proposal:** The configuration action should select an existing Gemini provider or create one with the correct defaults.
Update stock endpoint and environment defaults when the API kind changes.
Preserve custom values and make their meaning clear.
Add a regression test for the stock-default transition.

The setup route should make the Google API key and Gemini Library model easy to locate.
The user must still choose the active Library model explicitly.

### 4. Closing and saving lack a visible control

**Finding:** Settings closes on an outside click or Escape. The close transition writes both the settings and keys files.
The screenshots show no Done button or explanation of when changes are saved.

**Proposal:** Add a visible **Done** button and this explanation:

> Changes are saved when you close Settings.

Keep save failures visible. Do not report success if either file could not be written.
Keep the completion control accessible when the contents need to scroll.

### 5. Routine configuration competes with advanced fields

**Finding:** Fields named "kind", "env", "skip", and "id" require knowledge of the implementation.
The independent provider and model editors can show different providers at the same time.
That arrangement can make the displayed API key appear to belong to the model below it.

**Proposal:** Show the provider selection and API key directly.
Consider an Advanced section for the API type, endpoint, environment variables, and OpenRouter routing exclusions.
Use explicit field labels such as **API type**, **Environment variables**, and **Model ID**.
Explain that routing exclusions name OpenRouter hosts, rather than configured Tilepicky providers.
Separate the two editors clearly, or offer an action from a model to its provider configuration.

This is a further layout improvement. It is separate from the focused corrections above.

### 6. Provider removal has hidden consequences

**Finding:** Removing a provider also removes its configured models and clears their active selections.
The current Remove button performs this operation immediately.

**Proposal:** Show the consequences in a confirmation before removing a provider.
Use specific wording that identifies the provider and affected model choices.
This remains a separate improvement unless the implementation explicitly includes it.

## Focused implementation and verification

The implementation includes Active models first, processing notices, Gemini configuration navigation, stock-default handling, and a visible Done button.
The user chose to keep API keys visible and collapse provider and model setup.
Provider and model removal now ask for confirmation.

After the changes, inspect native screenshots at the normal scale and a larger text scale.
Check the first Settings view without scrolling.
Check both processing methods, an absent Gemini key, and the route back from Gemini configuration.
Verify that configuration navigation leaves the active model unchanged.
Verify that changing the API kind preserves custom endpoints and updates compatible stock defaults.
Verify that Done remains visible and save failures remain readable.

This review did not send requests to an external AI provider.

## Implementation review

A second code review inspected the revised Settings controls and their save path.
A new native screenshot showed the routine Settings view in a 1000 by 700 window with 125 percent text scale.
These observations do not replace the interaction tests.

### Improvements confirmed in the screenshot

- Both active model selectors appear near the top without scrolling.
- The serial-processing notice explains that closing Tilepicky pauses the job.
- The notice includes the OpenRouter limitation and a Configure Gemini action.
- Both provider key fields are visible, with their key status beneath them.
- Done and the save explanation remain above the scroll area.

The provider and model setup section falls below the visible area at this size.
That is acceptable because routine model selection and key entry remain visible.
The code keeps setup collapsed and explains its purpose when the user opens it.

### Improvements confirmed in the code

- Configure Gemini prepares a Gemini provider and Library model without assigning either active model slot.
- The action focuses the corresponding API key field.
- The API kind change updates stock connection defaults while preserving custom values.
- Provider and model removal require a second action and describe the affected active choices.
- The model editor uses explicit field labels and separates library scope from its processing method.

Further native screenshots showed Gemini key focus, provider removal confirmation, the Library selector, and the Google routine view.
The key-focus screenshot retained both OpenRouter active selections and showed focus in the Google key field.
The confirmation screenshot showed the affected models and active selections, with separate Remove provider and Keep controls.
The implementation agent confirmed that Keep left the provider intact.
The Google view showed an empty key field with "No key configured" and a disabled library start action.

### Save failure correction

The initial follow-up found that a running request could hide a temporary save-error message until it expired.
The final code uses a persistent Settings error instead.
Settings remains open after either settings or key storage fails.
The error appears above the scroll area, and Done becomes Retry save.

A native screenshot confirmed the visible error and Retry save control after a simulated storage failure.
The implementation agent restored the storage target and confirmed that Retry save succeeded.
This resolves the material save-feedback concern from the earlier review.

### Further refinements

The advanced provider and model selectors remain independent.
Their section labels now reduce ambiguity, but selecting a model does not select its provider editor.
A future edit-provider action could connect those controls directly.
This is a secondary improvement, not a reason to block the simpler routine view.

## Review conclusion

The revised Settings view puts routine choices and credentials first.
It explains sequential library requests and gives a direct route to Gemini configuration without changing the active selection.
Advanced setup stays available with an explanation of its purpose.
The final code and native screenshots resolve the material findings from this review.
The independent advanced selectors remain a possible refinement.

The implementation agent reported that all 173 automated tests passed.
This reviewer inspected the code and screenshots but did not independently rerun those tests.
No live provider request was needed for these Settings checks.


## Follow-up: dialog actions and processing descriptions

Completion actions now use a shared bottom-right footer. Cancel sits beside the primary action.
The sheet dialog and prompt preview scroll above Close. Settings scrolls above Done or Retry save.
Library options keeps Reset tags and Prompt template in the content, separate from Cancel and Save options.

The two processing descriptions now explain the effect of closing Tilepicky.
Serial jobs require the app to stay open. Google continues work on sheets that it has accepted.
Both descriptions explain what happens when the user reopens the library.
Settings also saves open edits when the app closes. A failed save prevents that close.

The library confirmation shows an estimated USD range. Estimate details explains prices, token ranges, and assumptions.
The response limit stays in those details. It is separate from estimated use.
OpenRouter uses public catalog prices. Google uses dated batch rates and names the price assumption for the latest alias.
The library job keeps its estimate and reported usage, including reasoning. The UI states when a price is unavailable.
