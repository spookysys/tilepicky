// Vendored from the `palette-quant` crate (MIT), itself a faithful port of
// the incremental quantizer in SuperFamiconv 0.12 (MIT). The crate and the
// reference are MIT-licensed; this file keeps their algorithm. The rest of
// Tilepicky is GPL-3.0-only, and the two licenses are compatible.
//! Tile-aware palette quantization for indexed SNES graphics.
//!
//! A faithful port of the incremental quantizer in SuperFamiconv 0.12
//! (`src/quant/incremental/`), which is itself based on Rilden's
//! TiledPaletteQuant. The phases, constants, tie-breaking, and sample schedule
//! match the reference, so the output matches for the same input and settings.
//!
//! The method, in order:
//! 1. Reduce every pixel to the mode's native color depth and collect each
//!    tile's unique colors with counts.
//! 2. Seed a palette by repeatedly splitting the worst-fitting subpalette.
//! 3. Grow every subpalette to the color capacity, splitting the color that
//!    carries the most error.
//! 4. Replace the weakest color and subpalette (ten rounds), nudging samples
//!    towards their nearest color.
//! 5. Refine with k-means passes that recenter each color on its samples.
//! 6. Assign every tile to its best-fitting subpalette and render its indices.
//!
//! Mixed depth, for Mode 3 or 4: the caller can mark tiles as deep (8bpp).
//! Each pixel of a deep tile enters the method as a tile of its own, so it
//! matches the nearest color of any subpalette and pulls on that color. A
//! deep pixel can also use the caller's deep-only CGRAM entries. They form
//! one more subpalette that only deep pixels can match, and that grows with
//! the others. When finalization drops some of those colors as duplicates,
//! a fill after the run puts new ones where the deep pixels' error is
//! largest. Without deep tiles, every phase runs as before.
//!
//! A deep tile can also hold shallow pixels, which the other layer must draw,
//! for example under an effect on the 8bpp layer. Such a mixed tile gives
//! both: its shallow pixels enter as one shallow tile that ignores the other
//! pixels, and its other pixels enter as deep pixels.

mod color;
mod dither;
mod export;
mod fill;
mod finalize;
mod palette;
mod prng;

pub use color::{oklab_sqdist, srgb_to_oklab, Oklab, Rgb};

pub use dither::Dither;
pub use export::{cgram, export_indexed_png, index_image};

/// Settings a caller controls.
#[derive(Clone)]
pub struct Job {
    /// Bits per pixel: 2, 4, or 8. With deep tiles, the depth of the others.
    pub bpp: u8,
    /// Number of subpalettes.
    pub palettes: usize,
    /// Colors per subpalette, excluding the reserved index 0.
    pub colors: usize,
    /// Dither to apply, if any.
    pub dither: Option<Dither>,
    /// Pixels equal to this color are index 0 and train nothing.
    pub color_zero: Option<Rgb>,
    /// Tile width in pixels.
    pub tile_width: u32,
    /// Tile height in pixels.
    pub tile_height: u32,
    /// One flag per tile, in tile order: true makes the tile deep (8bpp).
    /// Empty, or shorter than the tile count, means shallow for the rest.
    pub deep_tiles: Vec<bool>,
    /// CGRAM entries that only deep pixels use, such as the transparent
    /// slots of the subpalettes or entries above 127.
    pub deep_entries: Vec<u8>,
    /// Extra weight of a deep pixel's error. At 0 it counts like any other
    /// pixel; at 1 it counts twice.
    pub deep_weight: u32,
    /// One flag per image pixel, row-major: true keeps the pixel shallow
    /// inside a deep tile, which makes that tile mixed. Outside deep tiles
    /// the flag does nothing. Empty means no shallow pixels.
    pub shallow_pixels: Vec<bool>,
    /// Seed of the sample order. Another seed gives another result of about
    /// the same quality.
    pub seed: u64,
    /// Runs with the seeds `seed`, `seed + 1`, and so on; the result with
    /// the lowest error wins. The runs share the CPU cores.
    pub seeds: usize,
}

impl Job {
    /// A job with no deep tiles.
    pub fn new(bpp: u8, palettes: usize, colors: usize) -> Self {
        Job {
            bpp,
            palettes,
            colors,
            dither: None,
            color_zero: None,
            tile_width: 8,
            tile_height: 8,
            deep_tiles: Vec::new(),
            deep_entries: Vec::new(),
            deep_weight: 0,
            shallow_pixels: Vec::new(),
            seed: 0,
            seeds: 1,
        }
    }

    /// The CGRAM distance between two subpalettes: 4 at 2bpp, 16 at 4bpp.
    fn stride(&self) -> usize {
        1 << self.bpp
    }

    /// Checks that the deep-only entries are free for deep pixels: not color
    /// zero, not inside a subpalette, inside CGRAM, and each named once.
    pub fn check_deep_entries(&self) -> Result<(), String> {
        let stride = self.stride();
        if !self.deep_entries.is_empty() && !matches!(self.bpp, 2 | 4) {
            return Err(format!("deep-only entries need 2bpp or 4bpp subpalettes, not {}bpp", self.bpp));
        }
        if self.palettes * stride > 256 {
            return Err(format!("{} subpalettes of {stride} entries do not fit in CGRAM", self.palettes));
        }
        for (i, &entry) in self.deep_entries.iter().enumerate() {
            let entry = usize::from(entry);
            if entry == 0 {
                return Err("entry 0 is color zero, not a deep-only entry".into());
            }
            if entry < self.palettes * stride && entry % stride != 0 {
                return Err(format!("entry {entry} is a color of subpalette {}", entry / stride));
            }
            if self.deep_entries[..i].contains(&(entry as u8)) {
                return Err(format!("entry {entry} is named twice"));
            }
        }
        Ok(())
    }
}

/// The result of a quantization.
pub struct Quantized {
    /// One entry per subpalette, each a list of native 5-bit colors.
    pub palettes: Vec<Vec<Rgb>>,
    /// The subpalette each tile uses. A deep tile has 0 here, and a mixed
    /// tile the subpalette of its shallow part.
    pub tile_palette: Vec<usize>,
    /// One flag per tile: true for a deep tile, also when it is mixed.
    pub tile_deep: Vec<bool>,
    /// One index buffer per tile, in tile pixel order. A shallow tile holds
    /// indices into its subpalette, a deep tile holds CGRAM indices. Index 0
    /// is the reserved transparent or color-zero entry. A mixed tile has 0
    /// at its shallow pixels.
    pub tiles: Vec<Vec<u8>>,
    /// One entry per tile: for a mixed tile, the indices of its shallow
    /// pixels into subpalette `tile_palette`, with 0 at its deep pixels.
    /// None for every other tile.
    pub shallow_parts: Vec<Option<Vec<u8>>>,
    /// The colors of `Job::deep_entries`, in the same order. An entry that
    /// the image does not need holds color zero.
    pub deep_colors: Vec<Rgb>,
    /// The error that ranks results: `PIXEL_SHARE` of `pixel_error` and the
    /// rest of `block_error`.
    pub error: f64,
    /// The error of the output against the source reduced to 5-bit color:
    /// the mean squared Oklab distance per pixel.
    pub pixel_error: f64,
    /// The same over the means of 2x2 blocks, as the eye sees a dither.
    pub block_error: f64,
}

/// The share of the per-pixel error in `Quantized::error`. At low
/// resolution, each pixel shows, so a dither that only looks right from a
/// distance must not win on the 2x2 error alone.
pub const PIXEL_SHARE: f64 = 2.0 / 3.0;

/// Quantizes a row-major 8-bit RGB image into palettes and indexed tiles.
///
/// Panics if the deep-only entries fail `Job::check_deep_entries`.
pub fn quantize(image: &[Rgb], width: u32, height: u32, job: &Job) -> Quantized {
    if let Err(problem) = job.check_deep_entries() {
        panic!("{problem}");
    }
    if job.seeds > 1 {
        return best_of_seeds(image, width, height, job);
    }
    quantize_once(image, width, height, job)
}

/// Runs every seed of `job` in parallel and keeps the lowest error. On a
/// tie, the lower seed wins, so the result does not depend on the timing.
fn best_of_seeds(image: &[Rgb], width: u32, height: u32, job: &Job) -> Quantized {
    let runs: Vec<Quantized> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..job.seeds as u64)
            .map(|i| {
                let job = Job { seed: job.seed + i, seeds: 1, ..job.clone() };
                scope.spawn(move || quantize(image, width, height, &job))
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("a seed's run panicked")).collect()
    });
    runs.into_iter()
        .reduce(|best, run| if run.error < best.error { run } else { best })
        .expect("at least two seeds")
}

/// One run with the job's own deep tiles and seed.
fn quantize_once(image: &[Rgb], width: u32, height: u32, job: &Job) -> Quantized {
    let settings = palette::Settings {
        dither: job.dither,
    };
    let (tiles, samples, layout) = palette::extract(image, width, height, job);
    let capacity = job.colors.max(1);
    let max_subpalettes = job.palettes.max(1);
    let deep_capacity = job.deep_entries.len();

    let (subpalettes, deep) = if samples.is_empty() {
        (vec![Vec::new(); max_subpalettes], Vec::new())
    } else {
        palette::run(&tiles, &samples, max_subpalettes, capacity, deep_capacity, settings, job.seed)
    };

    let assignment = palette::assign_subpalettes(&subpalettes, &tiles, settings);
    let tile_colors: Vec<Vec<Oklab>> = tiles.iter().map(|tile| tile.colors.clone()).collect();
    let palettes = finalize::finalize(&subpalettes, &assignment, &tile_colors, capacity, job.color_zero);
    let mut deep_colors = finalize::finalize_deep(&deep, &palettes, deep_capacity, job.color_zero);
    // Finalization drops deep-only colors that round to a color that is
    // there already. With dither, the run moves these colors so little that
    // most of them collapse. Fill the free entries again.
    let zero = job.color_zero.map_or(Rgb::new(0, 0, 0), |c| c.reduce().normalize());
    let kept: Vec<Rgb> = deep_colors.iter().copied().filter(|&c| c != zero).collect();
    if kept.len() < deep_capacity {
        let fixed: Vec<Oklab> = std::iter::once(zero)
            .chain(palettes.iter().flat_map(|p| p.iter().skip(1).copied()))
            .chain(kept.iter().copied())
            .map(srgb_to_oklab)
            .collect();
        let added = fill::fill_deep(&tiles, &fixed, deep_capacity - kept.len());
        let colors: Vec<Oklab> = kept.iter().map(|&c| srgb_to_oklab(c)).chain(added).collect();
        deep_colors = finalize::finalize_deep(&colors, &palettes, deep_capacity, job.color_zero);
    }

    let shallow: Vec<palette::Choices> =
        palettes.iter().map(|p| palette::Choices::subpalette(p)).collect();
    let deep_choices = palette::Choices::entries(deep_entries(job, &palettes, &deep_colors));
    let mut tile_palette = Vec::with_capacity(layout.len());
    let mut rendered = Vec::with_capacity(layout.len());
    let mut shallow_parts = Vec::with_capacity(layout.len());
    for (index, place) in layout.iter().enumerate() {
        let end = layout.get(index + 1).map_or(tiles.len(), |next| next.first);
        if place.mixed {
            let sp = assignment[place.first].min(shallow.len() - 1);
            tile_palette.push(sp);
            shallow_parts.push(Some(palette::render_tile(&tiles[place.first], &shallow[sp], settings)));
        } else {
            shallow_parts.push(None);
        }
        if place.deep {
            if !place.mixed {
                tile_palette.push(0);
            }
            rendered.push(
                tiles[place.first + usize::from(place.mixed)..end]
                    .iter()
                    .map(|pixel| palette::render_tile(pixel, &deep_choices, settings)[0])
                    .collect(),
            );
        } else {
            let sp = assignment[place.first].min(shallow.len() - 1);
            tile_palette.push(sp);
            rendered.push(palette::render_tile(&tiles[place.first], &shallow[sp], settings));
        }
    }

    let mut result = Quantized {
        palettes,
        tile_palette,
        tile_deep: layout.iter().map(|place| place.deep).collect(),
        tiles: rendered,
        shallow_parts,
        deep_colors,
        error: 0.0,
        pixel_error: 0.0,
        block_error: 0.0,
    };
    result.pixel_error = output_error(image, width, height, job, &result, 1);
    result.block_error = output_error(image, width, height, job, &result, 2);
    result.error = PIXEL_SHARE * result.pixel_error + (1.0 - PIXEL_SHARE) * result.block_error;
    result
}

/// The error of `result` against `image` over blocks of `block` x `block`
/// pixels, as `Quantized::pixel_error` and `Quantized::block_error` describe.
fn output_error(image: &[Rgb], width: u32, height: u32, job: &Job, result: &Quantized, block: u32) -> f64 {
    let mut cgram = [Rgb::new(0, 0, 0); 256];
    for (index, color) in deep_entries(job, &result.palettes, &result.deep_colors) {
        cgram[usize::from(index)] = color;
    }
    let (tw, th) = (job.tile_width.max(1), job.tile_height.max(1));
    let across = width.div_ceil(tw);
    let mut output = vec![Oklab::new(0.0, 0.0, 0.0); image.len()];
    for (tile, indices) in result.tiles.iter().enumerate() {
        let (tx, ty) = (tile as u32 % across * tw, tile as u32 / across * th);
        for (i, &index) in indices.iter().enumerate() {
            let (x, y) = (tx + i as u32 % tw, ty + i as u32 / tw);
            if x < width && y < height {
                let shallow = job.shallow_pixels.get((y * width + x) as usize).copied().unwrap_or(false);
                let palette = &result.palettes[result.tile_palette[tile]];
                let color = match &result.shallow_parts[tile] {
                    Some(part) if shallow => palette[usize::from(part[i])],
                    _ if result.tile_deep[tile] => cgram[usize::from(index)],
                    _ => palette[usize::from(index)],
                };
                output[(y * width + x) as usize] = srgb_to_oklab(color);
            }
        }
    }
    let source: Vec<Oklab> = image.iter().map(|c| srgb_to_oklab(c.reduce().normalize())).collect();

    let mean = |colors: &[Oklab], x: u32, y: u32| {
        let (mut l, mut a, mut b) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..block * block {
            let c = colors[((y + i / block) * width + x + i % block) as usize];
            (l, a, b) = (l + c.l, a + c.a, b + c.b);
        }
        let n = (block * block) as f32;
        Oklab::new(l / n, a / n, b / n)
    };
    let (mut total, mut count) = (0.0f64, 0u64);
    for y in (0..height.saturating_sub(block - 1)).step_by(block as usize) {
        for x in (0..width.saturating_sub(block - 1)).step_by(block as usize) {
            total += f64::from(oklab_sqdist(mean(&source, x, y), mean(&output, x, y)));
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { total / count as f64 }
}

/// Every `(CGRAM index, color)` pair that a deep pixel can use: color zero,
/// the subpalettes' colors, and the deep-only entries that hold a color.
fn deep_entries(job: &Job, palettes: &[Vec<Rgb>], deep_colors: &[Rgb]) -> Vec<(u8, Rgb)> {
    let stride = job.stride();
    let zero = job.color_zero.map_or(Rgb::new(0, 0, 0), |c| c.reduce().normalize());
    let mut entries = vec![(0u8, zero)];
    for (k, palette) in palettes.iter().enumerate() {
        for (i, &color) in palette.iter().enumerate().skip(1) {
            entries.push(((k * stride + i) as u8, color));
        }
    }
    for (&entry, &color) in job.deep_entries.iter().zip(deep_colors) {
        if color != zero {
            entries.push((entry, color));
        }
    }
    entries
}

/// Chooses deep tiles where 8bpp lowers the error most, in addition to the
/// tiles that `Job::deep_tiles` marks already.
///
/// It quantizes once with no deep tiles, and measures for each tile how much
/// its error falls if each pixel takes the nearest of all the colors that a
/// deep pixel can use. The shallow pixels of `Job::shallow_pixels` stay
/// shallow, so they do not count. A tile with shallow pixels is deep when it
/// gains anything at all, because the caller draws it on both layers anyway.
/// Of the other unmarked tiles, the `count` with the largest gain are deep.
pub fn choose_deep_tiles(image: &[Rgb], width: u32, height: u32, job: &Job, count: usize) -> Vec<bool> {
    let flat = Job { deep_tiles: Vec::new(), seeds: 1, ..job.clone() };
    let result = quantize(image, width, height, &flat);
    let all: Vec<Oklab> =
        deep_entries(job, &result.palettes, &result.deep_colors).iter().map(|&(_, c)| srgb_to_oklab(c)).collect();
    let (tw, th) = (job.tile_width.max(1), job.tile_height.max(1));
    let across = width.div_ceil(tw);
    let mut deep: Vec<bool> = (0..result.tiles.len()).map(|t| job.deep_tiles.get(t).copied().unwrap_or(false)).collect();
    let mut gains: Vec<(f32, usize)> = Vec::new();
    for (tile, indices) in result.tiles.iter().enumerate() {
        if deep[tile] {
            continue;
        }
        let palette = &result.palettes[result.tile_palette[tile]];
        let (tx, ty) = (tile as u32 % across * tw, tile as u32 / across * th);
        let (mut gain, mut mixed) = (0.0, false);
        for (i, &index) in indices.iter().enumerate() {
            let (x, y) = (tx + i as u32 % tw, ty + i as u32 / tw);
            if x >= width || y >= height {
                continue;
            }
            if job.shallow_pixels.get((y * width + x) as usize).copied().unwrap_or(false) {
                mixed = true;
                continue;
            }
            let source = srgb_to_oklab(image[(y * width + x) as usize].reduce().normalize());
            let now = oklab_sqdist(source, srgb_to_oklab(palette[usize::from(index)]));
            let best = all.iter().map(|&c| oklab_sqdist(source, c)).fold(f32::INFINITY, f32::min);
            gain += now - best;
        }
        if mixed {
            deep[tile] = gain > 0.0;
        } else {
            gains.push((gain, tile));
        }
    }
    gains.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    for &(_, tile) in gains.iter().take(count) {
        deep[tile] = true;
    }
    deep
}

/// Marks each pixel of a region map that has one of the `keys` colors,
/// compared at the SNES's 5-bit depth.
pub fn pixels_from_map(map: &[Rgb], keys: &[Rgb]) -> Vec<bool> {
    let keys: Vec<Rgb> = keys.iter().map(|k| k.reduce()).collect();
    map.iter().map(|c| keys.contains(&c.reduce())).collect()
}

/// Marks each tile that holds at least one pixel of the `keys` colors in a
/// region map. Colors compare at the SNES's 5-bit depth, so the three low
/// bits of each channel do not matter.
pub fn deep_tiles_from_map(
    map: &[Rgb],
    width: u32,
    height: u32,
    tile_width: u32,
    tile_height: u32,
    keys: &[Rgb],
) -> Vec<bool> {
    let keys: Vec<Rgb> = keys.iter().map(|k| k.reduce()).collect();
    let across = width.div_ceil(tile_width);
    let down = height.div_ceil(tile_height);
    let mut deep = vec![false; (across * down) as usize];
    for y in 0..height {
        for x in 0..width {
            if keys.contains(&map[(y * width + x) as usize].reduce()) {
                deep[((y / tile_height) * across + x / tile_width) as usize] = true;
            }
        }
    }
    deep
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: Rgb = Rgb::new(0, 0, 0);

    #[test]
    fn a_region_map_marks_tiles_at_5_bit_depth() {
        // Two tiles side by side. The second holds one pixel that differs
        // from the key only in the three low bits.
        let mut map = vec![BLACK; 16 * 8];
        map[3 * 16 + 12] = Rgb::new(0xfc, 0xff, 0x01);
        let keys = [Rgb::new(0x00, 0xff, 0x00), Rgb::new(0xff, 0xff, 0x00)];
        assert_eq!(deep_tiles_from_map(&map, 16, 8, 8, 8, &keys), [false, true]);
        assert_eq!(deep_tiles_from_map(&map, 16, 8, 8, 8, &keys[..1]), [false, false]);
    }

    #[test]
    fn deep_entries_must_be_free_cgram() {
        let job = |entries: Vec<u8>| Job {
            deep_entries: entries,
            ..Job::new(4, 8, 15)
        };
        assert!(job((1..8).map(|k| k * 16).chain(128..=255).collect()).check_deep_entries().is_ok());
        assert!(job(vec![0]).check_deep_entries().is_err(), "color zero");
        assert!(job(vec![17]).check_deep_entries().is_err(), "a color of subpalette 1");
        assert!(job(vec![130, 130]).check_deep_entries().is_err(), "named twice");
        // With four subpalettes, CGRAM 64 and up is free.
        let four = Job {
            deep_entries: vec![65],
            ..Job::new(4, 4, 15)
        };
        assert!(four.check_deep_entries().is_ok());
        let deep_8bpp = Job {
            deep_entries: vec![200],
            ..Job::new(8, 1, 255)
        };
        assert!(deep_8bpp.check_deep_entries().is_err(), "no subpalettes at 8bpp");
    }

    /// A 16x8 image: a left tile with 48 colors, too many for one subpalette,
    /// and a flat right tile.
    fn two_tiles() -> Vec<Rgb> {
        (0..16 * 8)
            .map(|i| {
                let (x, y) = (i % 16, i / 16);
                if x < 8 {
                    let n = (y * 8 + x) % 48;
                    Rgb::new(40 + (n % 4) as u8 * 50, 40 + (n / 4 % 4) as u8 * 50, 60 + (n / 16) as u8 * 60)
                } else {
                    Rgb::new(200, 40, 40)
                }
            })
            .collect()
    }

    /// The error of the left tile, rebuilt with `color` for each index.
    fn left_tile_error(image: &[Rgb], indices: &[u8], color: impl Fn(u8) -> Rgb) -> f32 {
        indices
            .iter()
            .enumerate()
            .map(|(i, &index)| {
                let source = srgb_to_oklab(image[(i / 8) * 16 + i % 8].reduce().normalize());
                oklab_sqdist(source, srgb_to_oklab(color(index)))
            })
            .sum()
    }

    #[test]
    fn a_deep_tile_takes_colors_from_the_whole_cgram() {
        let image = two_tiles();
        let shallow = Job {
            color_zero: Some(BLACK),
            ..Job::new(4, 8, 15)
        };
        let deep = Job {
            deep_tiles: vec![true, false],
            deep_entries: (1..8).map(|k| k * 16).collect(),
            ..Job::new(4, 8, 15)
        };
        let deep = Job { color_zero: Some(BLACK), ..deep };
        let result = quantize(&image, 16, 8, &deep);
        assert_eq!(result.tile_deep, [true, false]);
        assert_eq!(result.deep_colors.len(), 7);

        // Each index of the deep tile must name an entry that a deep pixel
        // can use: color zero, a subpalette color, or a deep-only entry.
        let mut cgram = vec![None; 256];
        cgram[0] = Some(BLACK);
        for (k, palette) in result.palettes.iter().enumerate() {
            for (i, &color) in palette.iter().enumerate().skip(1) {
                cgram[k * 16 + i] = Some(color);
            }
        }
        for (&entry, &color) in deep.deep_entries.iter().zip(&result.deep_colors) {
            cgram[usize::from(entry)] = Some(color);
        }
        let deep_error = left_tile_error(&image, &result.tiles[0], |index| {
            cgram[usize::from(index)].expect("an entry that a deep pixel can use")
        });

        // At 4bpp, the 48 colors must share one subpalette of 15. As a deep
        // tile, they reach far more colors, so the error must fall well
        // below that.
        let flat = quantize(&image, 16, 8, &shallow);
        let palette = &flat.palettes[flat.tile_palette[0]];
        let shallow_error = left_tile_error(&image, &flat.tiles[0], |index| palette[usize::from(index)]);
        assert!(
            deep_error < shallow_error / 4.0,
            "deep {deep_error} is not well below shallow {shallow_error}"
        );
    }

    #[test]
    fn a_mixed_tile_splits_its_pixels_between_the_layers() {
        let image = two_tiles();
        // The left tile is deep, but its top row stays shallow.
        let job = Job {
            color_zero: Some(BLACK),
            deep_tiles: vec![true, false],
            deep_entries: (1..8).map(|k| k * 16).collect(),
            shallow_pixels: (0..16 * 8).map(|i| i < 8).collect(),
            ..Job::new(4, 8, 15)
        };
        let result = quantize(&image, 16, 8, &job);
        assert_eq!(result.tile_deep, [true, false]);
        assert!(result.shallow_parts[1].is_none(), "a shallow tile has no shallow part");
        let part = result.shallow_parts[0].as_ref().expect("the mixed tile has a shallow part");

        // Each pixel is drawn by one layer only: the other one has index 0.
        for (i, (&deep, &shallow)) in result.tiles[0].iter().zip(part).enumerate() {
            if i < 8 {
                assert_eq!(deep, 0, "pixel {i} is shallow");
                assert_ne!(shallow, 0, "pixel {i} is shallow");
            } else {
                assert_eq!(shallow, 0, "pixel {i} is deep");
            }
        }
        // The top row has only 8 colors. As a part of its own, it must fit
        // its subpalette better than as a row of a whole 4bpp tile, which
        // shares one subpalette with 48 colors.
        let row_error = |indices: &[u8], palette: &[Rgb]| {
            left_tile_error(&image, &indices[..8], |index| palette[usize::from(index)])
        };
        let mixed_error = row_error(part, &result.palettes[result.tile_palette[0]]);
        let flat = quantize(&image, 16, 8, &Job { color_zero: Some(BLACK), ..Job::new(4, 8, 15) });
        let flat_error = row_error(&flat.tiles[0], &flat.palettes[flat.tile_palette[0]]);
        assert!(mixed_error < flat_error, "mixed {mixed_error} is not below flat {flat_error}");
    }

    #[test]
    fn the_choice_makes_the_busiest_tile_deep() {
        let job = Job {
            color_zero: Some(BLACK),
            deep_entries: vec![16, 32],
            ..Job::new(4, 8, 15)
        };
        assert_eq!(choose_deep_tiles(&two_tiles(), 16, 8, &job, 1), [true, false]);
        // A tile with shallow pixels is deep as soon as 8bpp helps its other
        // pixels, and it does not count against the number.
        let job = Job { shallow_pixels: (0..16 * 8).map(|i| i < 8).collect(), ..job };
        assert_eq!(choose_deep_tiles(&two_tiles(), 16, 8, &job, 0), [true, false]);
        // A tile that the job marks stays deep and does not count either.
        let job = Job { shallow_pixels: Vec::new(), deep_tiles: vec![false, true], ..job };
        assert_eq!(choose_deep_tiles(&two_tiles(), 16, 8, &job, 1), [true, true]);
    }

    #[test]
    fn without_deep_tiles_the_result_has_none() {
        let job = Job {
            color_zero: Some(BLACK),
            ..Job::new(4, 8, 15)
        };
        let result = quantize(&two_tiles(), 16, 8, &job);
        assert_eq!(result.tile_deep, [false, false]);
        assert!(result.deep_colors.is_empty());
    }
}
