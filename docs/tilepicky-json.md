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
      "identity": "3f1c...",
      "status": "labeled",
      "caption": "Top-down village tiles: grass, dirt paths, wooden houses",
      "tags": ["grass", "house", "pixel art", "village"]
    }

`identity` is a SHA-256 fingerprint of the image dimensions and the decoded
pixels, from the first frame of a GIF. When the pixels change, the label is
stale. `status` is `unlabelable` when the model could not say what the sheet
shows; the caption and the tags are then empty.

Books from before this format kept island regions under `labels`. The tool
ignores that key and leaves it out the next time it writes the book.

Writes replace the complete book through a temporary file. A malformed or unreadable
book blocks writes until you fix it. Keep backups of your project images and books.
