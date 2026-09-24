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

Labels contain the provider, model, image fingerprint, sheet label, and island labels.
Detect islands writes regions without a sheet label, provider, or model.
Removing AI labels keeps these regions.
Island regions are lists of pixel rectangles `[x, y, width, height]`.
Their coordinates do not change when you adjust the tile grid.
The fingerprint covers image dimensions and decoded pixels, using the first GIF frame.

Writes replace the complete book through a temporary file. A malformed or unreadable
book blocks writes until you fix it. Keep backups of your project images and books.
