# tilepicky.json

The library and the project each keep one `tilepicky.json` in their top
folder. It is the book of that tree: for every sheet, the grid it is read
through, where its pixels came from, and its animations. Sheets are keyed by
their path from the top folder, and `tile` at the head of the file is the
size that tree used last.

    {
      "tile": 16,
      "sheets": {
        "kenney_tiny-town/Tilemap/tilemap_packed.png": {
          "animations": [
            { "px": [1280, 64], "frame": [64, 64], "frames": 6, "ms": 100 }
          ]
        },
        "village.png": {
          "tile": [32, 48],
          "provenance": [
            { "source": "kenney_tiny-town/Tilemap/tilemap_packed.png", "rects": [[96, 48, 96, 96]] }
          ],
          "animations": [
            { "px": [256, 32], "frame": [64, 64], "frames": [4, 2], "ms": 100 }
          ]
        }
      }
    }

One number stands for both axes. For `frames`, one number means one row.

A sheet that was labeled with AI also has a `label`:

    "label": {
      "provider": "OpenRouter",
      "model": "google/gemini-2.5-flash",
      "status": "labeled",
      "caption": "Top-down village tiles: grass, dirt paths, wooden houses",
      "tags": ["grass", "house", "pixel art", "village"]
    }

The label describes the whole image, or the first frame of a GIF. `status`
is `unlabelable` when the model could not say what the sheet shows; the
caption and the tags are then empty.

Books from before this format kept island regions under `labels`. The tool
ignores that key and leaves it out the next time it writes the book.

## Color quantization

The project book may hold a `quantize` list. Each group shares one setting
across the sheets it covers, so a scene shares its palettes. The `target` is
a folder or a list of sheets. Each member's `pins` array stores tile depth overrides in row order, from left to right.
Values are `"auto"`, `"4bpp"`, or `"8bpp"`. Missing entries use automatic depth.
Old boolean values remain readable: `false` means automatic depth, and `true` means forced 8bpp.

    "quantize": [
      {
        "target": { "dir": "props" },
        "setting": {
          "mode": "snes",
          "snes": { "depth": "bpp4_plus8", "palettes": 8, "colors": 15, "dither": "checker", "auto_pct": 25, "extra_8bpp": 8 }
        },
        "pins": { "props/tree.png": ["auto", "8bpp", "4bpp"] }
      },
      {
        "target": { "files": ["village.png"] },
        "setting": { "mode": "general", "general": { "kind": "indexed", "format": "rgb565", "colors": 256, "key": true, "dither": "none" } }
      }
    ]

The saved SNES `colors` value excludes transparent index 0. The UI shows this value plus one as **Palette entries**.
General indexed `colors` includes the transparent entry when `key` is true. Formats with separate alpha count RGB colors.

**Apply** saves settings and pins. Changing a project sheet's tile grid clears its saved pins.

A sheet belongs to at most one group. Apply updates the same group directly.
The panel asks before it replaces overlapping groups. A concurrent change on disk prevents an outdated panel from overwriting the groups.
The preview preserves the source pixels. Export saves the selected sheet's current quantized result.

Writes replace the complete book through a temporary file. A malformed or unreadable
book blocks writes until you fix it. Keep backups of your project images and books.

## Jobs and search indexes

The root `ai_batch` record stores the library job, its progress, and its pause or cancellation requests.
The job retains its original model, provider, prompt, and tags. API keys stay in the app configuration.
Use the app to change job state. Manual edits can conflict with its coordinator.

Image vectors live in `embeddings.json`; caption and tag vectors live in `embeddings-text.json`.
Each index records its model and provider identity. A changed model or provider requires new vectors.
Label edits invalidate caption vectors, but leave image-only vectors valid.

See the [user guide](../README.md) for job controls and search index generation.
