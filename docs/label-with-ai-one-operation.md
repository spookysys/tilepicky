# Combine labeling and embeddings into one operation

Status: proposal, not built. Read this after testing the current split.

## The problem

Today the user runs two operations. **Generate Tags (AI)** makes a caption and
tags. **Generate Embeddings (AI)** makes the vectors for search by meaning. A
library becomes searchable by meaning only after the second operation, and it is
easy to forget. The image embedding needs no label, so the split asks for work
that the tool can do by itself.

## The goal

One operation, **Label with AI**, makes both for every chosen sheet:

- a caption and tags, from a vision model;
- a vector, from the embedding model.

The user cannot run one without the other. After the operation, word search and
search by meaning both work.

## What happens for one sheet

1. Read the image.
2. Send the image to the vision model. It answers with the caption and the tags.
3. Send the image, and the caption and the tags, to the embedding model. It
   answers with the vector.
4. Save the label and the vector in the library.

This is two network calls for one sheet. The vector carries the picture and the
words.

## The rules

- A label that fails leaves the sheet unlabeled. The tool writes no vector for
  that sheet.
- A vector that fails keeps the label. The job records that the sheet needs a
  vector, and a retry makes only the vector.
- An edit to a label re-embeds that sheet, because the vector's key holds the
  label text.
- A change of the embedding model re-embeds every sheet, as it does today.

## Cost

The confirm must estimate both calls:

- the label call, as `src/batch/cost.rs` does today;
- the embedding call, from the embedding model's price and an image token
  estimate.

Show one total, or two lines and a total. Say that it is a range.

## What stays

- The Embeddings model choice in Settings.
- The **Include captions and tags** switch. In the combined operation it is the
  normal case, and the sheet's own label goes into the vector.
- A repair entry that re-embeds without relabeling. It serves a changed
  embedding model and a failed vector.

## What changes

- The library panel offers one **Label with AI** action. It does not offer a
  separate embeddings action.
- The right-click menus offer one item.
- The job journal records both steps for each sheet: the label state and the
  vector state.
- The AI panel shows both counts: labels saved and vectors saved.

## Open questions

- Order and failure. If the label succeeds and the vector fails, is the job
  finished with a warning, or does it keep trying the vector? Proposal: finish
  the job, mark the vector as pending, and offer a retry.
- The batch path. A Gemini batch returns labels for many sheets at once. The
  embedding calls then run one for each sheet, or in small groups. Does the job
  hold the labels until the vectors are made?
- Cost accuracy. The image token count for the embedding model is not known
  exactly. Use a range, as the label estimate does.
- Two providers. The vision model and the embedding model can sit on different
  providers. The job then needs both keys.

## Test plan

- A small library. One operation writes a label and a vector for each sheet.
- A failing embedding call keeps the label and leaves the vector pending.
- A failing label writes no vector.
- An edit to a label re-embeds only that sheet.
- A change of the embedding model re-embeds every sheet.

## Rollout

1. Build the combined job beside the current split.
2. Keep the split for one release, behind the same button.
3. Remove the standalone embeddings action when the combined one is proven.
