//! The tile-aware incremental k-means quantizer.
//!
//! Ported from SuperFamiconv 0.12's `src/quant/incremental/`, keeping its
//! phases, constants, tie-breaking, and sample schedule.

use crate::quant::color::{mean_oklab, oklab_sqdist, srgb_to_oklab, Oklab, Rgb};
use crate::quant::dither::{Dither, DITHER_WEIGHT, MAX_CANDIDATES};
use crate::quant::prng::Prng;
use crate::quant::Job;

/// Iterations that replace weak entries.
const REPLACE_ITERATIONS: usize = 10;
/// k-means refinement passes after the nudging phase.
const REFINEMENT_ITERATIONS: usize = 4;
/// Fraction of samples drawn per nudging round.
const FRACTION_OF_PIXELS: f32 = 0.1;
/// Error share below which a palette's worst color is weak enough to replace.
const MIN_COLOR_FACTOR: f32 = 0.5;
/// Error share below which a whole subpalette is weak enough to replace.
const MIN_PALETTE_FACTOR: f32 = 0.5;
/// Nudge fraction.
const REPLACE_ALPHA: f32 = 0.3;
/// Nudge fraction for the final round.
const FINAL_ALPHA: f32 = 0.5;
/// Nudge fraction if dithering.
const REPLACE_ALPHA_D: f32 = 0.1;
/// Nudge fraction for the final round if dithering.
const FINAL_ALPHA_D: f32 = 0.2;

/// Settings fixed for a whole quantization.
#[derive(Clone, Copy)]
pub(crate) struct Settings {
    pub dither: Option<Dither>,
}

impl Settings {
    pub(crate) fn is_dithered(self) -> bool {
        self.dither.is_some()
    }

    /// Snaps an Oklab color to the 5-bit grid and back.
    pub(crate) fn quantize_color(self, color: Oklab) -> Oklab {
        srgb_to_oklab(color.to_reduced().normalize())
    }
}

/// A pixel inside a tile: its position and an index into the tile's colors.
#[derive(Clone, Copy)]
pub(crate) struct TilePixel {
    pub x: u32,
    pub y: u32,
    pub color: usize,
}

use std::cell::Cell;

/// A tile's unique 5-bit colors, their counts, and its pixels.
#[derive(Clone)]
pub(crate) struct TileData {
    pub colors: Vec<Oklab>,
    pub counts: Vec<u32>,
    pub pixels: Vec<TilePixel>,
    /// One entry per tile pixel in source order: its color index, or `None`
    /// for an index-0 (transparent or color-zero) pixel. Rendering walks this
    /// so a dropped pixel does not shift the rest of the tile.
    pub slots: Vec<Option<usize>>,
    /// The subpalette this tile last matched, a search hint.
    pub hint: Cell<usize>,
    /// The tile's top-left position and size, for turning a slot into a dot.
    pub origin: (u32, u32),
    pub size: (u32, u32),
    /// A one-pixel piece of a deep (8bpp) tile. Its pixel can take any color,
    /// so it may also match the deep-only subpalette.
    pub deep: bool,
    /// How many times each pixel counts in the error: 1, or more for a deep
    /// pixel that the caller weights up.
    pub weight: u32,
}

impl TileData {
    fn is_empty(&self, settings: Settings) -> bool {
        if settings.is_dithered() {
            self.pixels.is_empty()
        } else {
            self.colors.is_empty()
        }
    }
}

/// One training sample, one per non-index-0 pixel.
#[derive(Clone, Copy)]
pub(crate) struct Sample {
    pub tile: usize,
    pub x: u32,
    pub y: u32,
    pub color: Oklab,
}

/// A tile grid position, for the tiling helper.
pub(crate) struct TileView {
    pub x: u32,
    pub y: u32,
}

/// Splits `width` x `height` into tile grid positions.
pub(crate) fn tiles(width: u32, height: u32, tw: u32, th: u32) -> Vec<TileView> {
    let mut out = Vec::new();
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            out.push(TileView { x, y });
            x += tw;
        }
        y += th;
    }
    out
}

/// Where an image tile's data starts, and whether the tile is deep or mixed.
///
/// A shallow tile is one `TileData`. A deep tile is one `TileData` per pixel,
/// in tile pixel order, so each pixel can match a color of its own. A mixed
/// tile is a deep tile with shallow pixels: one shallow `TileData` with only
/// those pixels comes first, then the deep pixels, where each shallow pixel
/// is an empty one.
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub first: usize,
    pub deep: bool,
    pub mixed: bool,
}

/// Reduces every tile's pixels to unique 5-bit colors and collects samples.
pub(crate) fn extract(
    image: &[Rgb],
    width: u32,
    height: u32,
    job: &Job,
) -> (Vec<TileData>, Vec<Sample>, Vec<Layout>) {
    let (tw, th) = (job.tile_width.max(1), job.tile_height.max(1));
    let mut tile_data = Vec::new();
    let mut samples = Vec::new();
    let mut layout = Vec::new();
    let region = Region {
        image,
        width,
        height,
        zero: reduced_color_of(job.color_zero),
    };

    let shallow = |x: u32, y: u32| {
        x < width && y < height && job.shallow_pixels.get((y * width + x) as usize).copied().unwrap_or(false)
    };
    let all = |_: u32, _: u32| true;
    for (index, view) in tiles(width, height, tw, th).into_iter().enumerate() {
        let deep = job.deep_tiles.get(index).copied().unwrap_or(false);
        let mixed = deep && (0..th).any(|row| (0..tw).any(|col| shallow(view.x + col, view.y + row)));
        layout.push(Layout {
            first: tile_data.len(),
            deep,
            mixed,
        });
        if mixed {
            let tile = region.collect((view.x, view.y), (tw, th), false, 1, shallow, tile_data.len(), &mut samples);
            tile_data.push(tile);
        }
        if deep {
            let weight = 1 + job.deep_weight;
            let deep_pixel = |x, y| !shallow(x, y);
            for row in 0..th {
                for col in 0..tw {
                    let origin = (view.x + col, view.y + row);
                    let tile = region.collect(origin, (1, 1), true, weight, deep_pixel, tile_data.len(), &mut samples);
                    tile_data.push(tile);
                }
            }
        } else {
            let tile = region.collect((view.x, view.y), (tw, th), false, 1, all, tile_data.len(), &mut samples);
            tile_data.push(tile);
        }
    }

    (tile_data, samples, layout)
}

/// The image that `extract` cuts into tiles.
struct Region<'a> {
    image: &'a [Rgb],
    width: u32,
    height: u32,
    zero: Rgb,
}

impl Region<'_> {
    /// Collects one tile. Each pixel counts `weight` times and adds that many
    /// samples. A pixel that `takes` refuses is index 0 in this tile, because
    /// another part of the cell draws it.
    #[allow(clippy::too_many_arguments)]
    fn collect(
        &self,
        origin: (u32, u32),
        size: (u32, u32),
        deep: bool,
        weight: u32,
        takes: impl Fn(u32, u32) -> bool,
        tile_index: usize,
        samples: &mut Vec<Sample>,
    ) -> TileData {
        let (tw, th) = size;
        let mut reduced: Vec<Rgb> = Vec::new();
        let mut colors: Vec<Oklab> = Vec::new();
        let mut counts: Vec<u32> = Vec::new();
        let mut pixels: Vec<TilePixel> = Vec::new();
        let mut slots: Vec<Option<usize>> = Vec::with_capacity((tw * th) as usize);

        for row in 0..th {
            for col in 0..tw {
                let (px, py) = (origin.0 + col, origin.1 + row);
                if px >= self.width || py >= self.height || !takes(px, py) {
                    slots.push(None);
                    continue;
                }
                let source = self.image[(py * self.width + px) as usize];
                let reduced_color = source.reduce();
                if reduced_color == self.zero {
                    slots.push(None);
                    continue;
                }

                let oklab = srgb_to_oklab(reduced_color.normalize());
                let index = match reduced.iter().position(|&c| c == reduced_color) {
                    Some(pos) => {
                        counts[pos] += weight;
                        pos
                    }
                    None => {
                        reduced.push(reduced_color);
                        colors.push(oklab);
                        counts.push(weight);
                        colors.len() - 1
                    }
                };
                pixels.push(TilePixel {
                    x: px,
                    y: py,
                    color: index,
                });
                slots.push(Some(index));
                for _ in 0..weight {
                    samples.push(Sample {
                        tile: tile_index,
                        x: px,
                        y: py,
                        color: oklab,
                    });
                }
            }
        }

        TileData {
            colors,
            counts,
            pixels,
            slots,
            hint: Cell::new(0),
            origin,
            size,
            deep,
            weight,
        }
    }
}

fn reduced_color_of(color_zero: Option<Rgb>) -> Rgb {
    color_zero.map_or(Rgb::new(0, 0, 0), |c| c.reduce())
}

/// Runs the quantizer, returning one color list per subpalette, and the
/// colors of the deep-only subpalette, which only deep pixels can use.
///
/// The deep-only subpalette joins after the seed phase, at the mean of the
/// deep pixels, and grows to `deep_capacity` with the others. No phase
/// splits it, copies it, or replaces it as a whole.
pub(crate) fn run(
    tiles: &[TileData],
    samples: &[Sample],
    max_subpalettes: usize,
    capacity: usize,
    deep_capacity: usize,
    settings: Settings,
    seed: u64,
) -> (Vec<Vec<Oklab>>, Vec<Oklab>) {
    let mut cycle = SampleCycle::new(samples.len(), seed);
    let base_iterations = (FRACTION_OF_PIXELS * samples.len() as f32) as usize;
    let (iterations, alpha, final_alpha) = if settings.is_dithered() {
        (base_iterations / 5, REPLACE_ALPHA_D, FINAL_ALPHA_D)
    } else {
        (base_iterations, REPLACE_ALPHA, FINAL_ALPHA)
    };

    let mut palette = seed_palette(tiles, samples, &mut cycle, max_subpalettes, alpha, iterations, settings);
    let deep_mean = mean_oklab(samples.iter().filter(|s| tiles[s.tile].deep).map(|s| s.color));
    if let (true, Some(mean)) = (deep_capacity > 0, deep_mean) {
        palette.subpalettes.push(Subpalette::deep_only(vec![mean], settings));
    }
    for _ in 1..capacity.max(deep_capacity) {
        grow_palette(&mut palette, tiles, samples, &mut cycle, alpha, iterations, capacity, deep_capacity);
    }

    let mut min_palette = palette.clone();
    let mut min_mse = palette.mse(tiles);
    for _ in 0..REPLACE_ITERATIONS {
        palette = replace_weakest(&palette, tiles, MIN_COLOR_FACTOR, MIN_PALETTE_FACTOR);
            nudge(&mut palette, tiles, samples, &mut cycle, alpha, iterations);
            let mse = palette.mse(tiles);
        if mse < min_mse {
            min_mse = mse;
            min_palette = palette.clone();
        }
    }
    palette = min_palette;

    if !settings.is_dithered() {
        palette.quantize();
    }

    nudge(&mut palette, tiles, samples, &mut cycle, final_alpha, iterations * 10);

    if !settings.is_dithered() {
        palette.quantize();
        for _ in 0..REFINEMENT_ITERATIONS {
            palette = refine(&palette, tiles);
        }
    }

    palette.quantize();
    palette.colors()
}



/// Assigns every tile to its best-fitting subpalette.
pub(crate) fn assign_subpalettes(
    colors: &[Vec<Oklab>],
    tiles: &[TileData],
    settings: Settings,
) -> Vec<usize> {
    let palette = Palette::from_colors(colors.to_vec(), settings);
    tiles
        .iter()
        .map(|tile| {
            if tile.is_empty(settings) {
                0
            } else {
                palette.best_fit(tile).0
            }
        })
        .collect()
}

/// The finished colors that a tile can match, and the index to write for
/// each. Rendering matches against the finished palettes, converted back to
/// Oklab, as the reference's output pass does.
pub(crate) struct Choices {
    colors: Vec<Oklab>,
    indices: Vec<u8>,
}

impl Choices {
    /// One subpalette. Entry 0 is the color-zero slot.
    pub(crate) fn subpalette(palette: &[Rgb]) -> Self {
        Choices {
            colors: palette.iter().map(|&c| srgb_to_oklab(c)).collect(),
            indices: (0..palette.len()).map(|i| i as u8).collect(),
        }
    }

    /// Every `(CGRAM index, color)` pair that a deep pixel can use.
    pub(crate) fn entries(entries: impl IntoIterator<Item = (u8, Rgb)>) -> Self {
        let (indices, colors) = entries.into_iter().map(|(i, c)| (i, srgb_to_oklab(c))).unzip();
        Choices { colors, indices }
    }

    fn view(&self) -> SubpaletteView<'_> {
        SubpaletteView {
            colors: &self.colors,
            quantized: &self.colors,
        }
    }
}

/// Renders one tile as indices from `choices`.
pub(crate) fn render_tile(tile: &TileData, choices: &Choices, settings: Settings) -> Vec<u8> {
    let view = choices.view();
    let mut out = vec![0u8; tile.slots.len()];
    let across = tile.size.0.max(1);
    for (p, slot) in tile.slots.iter().enumerate() {
        let Some(color_index) = slot else {
            // Index 0: transparent or the color-zero key.
            continue;
        };
        let color = tile.colors[*color_index];
        // Entry 0 is a candidate too, so a dark pixel can match the
        // color-zero slot, as the reference's output pass allows.
        let index = if let Some(pattern) = settings.dither {
            let x = tile.origin.0 + (p as u32 % across);
            let y = tile.origin.1 + (p as u32 / across);
            view.nearest_dithered(pattern, x, y, color, None).0
        } else {
            view.nearest(color).0
        };
        out[p] = choices.indices[index];
    }
    out
}

/// Cycles through samples in a seeded random order, reshuffling when exhausted.
struct SampleCycle {
    order: Vec<usize>,
    index: usize,
    prng: Prng,
}

impl SampleCycle {
    fn new(len: usize, seed: u64) -> Self {
        let mut prng = Prng::new(seed);
        let mut order: Vec<usize> = (0..len).collect();
        prng.shuffle(&mut order);
        SampleCycle {
            order,
            index: 0,
            prng,
        }
    }

    fn next(&mut self) -> usize {
        if self.index >= self.order.len() {
            self.prng.shuffle(&mut self.order);
            self.index = 0;
        }
        let i = self.order[self.index];
        self.index += 1;
        i
    }
}

/// Index and value of the largest element.
fn argmax(values: &[f32]) -> usize {
    values
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(i, _)| i)
}

/// Index and value of the smallest element.
fn argmin(values: &[f32]) -> usize {
    values
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(i, _)| i)
}

/// Index and values of the smallest and second smallest.
fn smallest_two(values: impl IntoIterator<Item = f32>) -> (usize, f32, f32) {
    let (mut best_i, mut best_v) = (0usize, f32::INFINITY);
    let mut second_v = f32::INFINITY;
    for (i, v) in values.into_iter().enumerate() {
        if v < best_v {
            second_v = best_v;
            best_v = v;
            best_i = i;
        } else if v < second_v {
            second_v = v;
        }
    }
    (best_i, best_v, second_v)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tie {
    Win,
    Lose,
}

/// A palette of subpalettes.
#[derive(Clone)]
struct Palette {
    subpalettes: Vec<Subpalette>,
    settings: Settings,
}

impl Palette {
    fn new(subpalettes: Vec<Subpalette>, settings: Settings) -> Self {
        Palette {
            subpalettes,
            settings,
        }
    }

    fn seeded(color: Oklab, settings: Settings) -> Self {
        Palette::new(vec![Subpalette::from_colors(vec![color], settings)], settings)
    }

    fn from_colors(colors: Vec<Vec<Oklab>>, settings: Settings) -> Self {
        let subpalettes = colors
            .into_iter()
            .map(|c| Subpalette::from_colors(c, settings))
            .collect();
        Palette::new(subpalettes, settings)
    }

    fn settings(&self) -> Settings {
        self.settings
    }

    fn len(&self) -> usize {
        self.subpalettes.len()
    }

    fn subpalettes(&self) -> &[Subpalette] {
        &self.subpalettes
    }

    /// The subpalettes' colors, and the deep-only subpalette's colors apart.
    fn colors(self) -> (Vec<Vec<Oklab>>, Vec<Oklab>) {
        let mut deep = Vec::new();
        let mut colors = Vec::new();
        for sp in self.subpalettes {
            if sp.deep_only {
                deep = sp.colors;
            } else {
                colors.push(sp.colors);
            }
        }
        (colors, deep)
    }

    /// The number of subpalettes that shallow tiles can use. The deep-only
    /// subpalette, if any, comes after them.
    fn regular(&self) -> usize {
        self.subpalettes.iter().filter(|sp| !sp.deep_only).count()
    }

    fn duplicate_palette(&mut self, index: usize) {
        self.subpalettes.push(self.subpalettes[index].clone());
    }

    fn duplicate_color(&mut self, palette: usize, color_index: usize) {
        self.subpalettes[palette].duplicate_color(color_index);
    }

    fn copy_subpalette(&mut self, from: usize, to: usize) {
        self.subpalettes[to] = self.subpalettes[from].clone();
    }

    fn copy_color(&mut self, palette: usize, from: usize, to: usize) {
        self.subpalettes[palette].copy_color(from, to);
    }

    /// Index and distances of the best two subpalettes for `tile`.
    fn best_two(&self, tile: &TileData) -> (usize, f32, f32) {
        let dither = self.settings.dither;
        let hint = tile.hint.get().min(self.subpalettes.len() - 1);
        let mut best = (hint, self.subpalettes[hint].view().distance(tile, dither));
        let mut second = f32::INFINITY;

        for (i, sp) in self.subpalettes.iter().enumerate() {
            if i == hint || (sp.deep_only && !tile.deep) {
                continue;
            }
            let Some(d) = sp.view().distance_within(tile, dither, second, Tie::Win) else {
                continue;
            };
            if d < best.1 || (d == best.1 && i < best.0) {
                second = best.1;
                best = (i, d);
            } else {
                second = second.min(d);
            }
        }
        tile.hint.set(best.0);
        (best.0, best.1, second)
    }

    /// Index and distance of the best-fitting subpalette for `tile`.
    fn best_fit(&self, tile: &TileData) -> (usize, f32) {
        let dither = self.settings.dither;
        let hint = tile.hint.get().min(self.subpalettes.len() - 1);
        let mut best = (hint, self.subpalettes[hint].view().distance(tile, dither));

        for (i, sp) in self.subpalettes.iter().enumerate() {
            if i == hint || (sp.deep_only && !tile.deep) {
                continue;
            }
            let tie = if i < best.0 { Tie::Win } else { Tie::Lose };
            if let Some(d) = sp.view().distance_within(tile, dither, best.1, tie) {
                best = (i, d);
            }
        }
        tile.hint.set(best.0);
        best
    }

    /// Nudges the color nearest to `sample` in the tile's best subpalette.
    fn nudge(&mut self, tile: &TileData, sample: &Sample, alpha: f32) {
        let (subpalette_index, _) = self.best_fit(tile);
        let view = self.subpalettes[subpalette_index].view();
        let (color_index, target) = if let Some(pattern) = self.settings.dither {
            let (color_index, _, target) =
                view.nearest_dithered(pattern, sample.x, sample.y, sample.color, None);
            (color_index, target)
        } else {
            (view.nearest(sample.color).0, sample.color)
        };
        self.subpalettes[subpalette_index].nudge(color_index, target, alpha);
    }

    /// Mean squared error over all tiles, each matched to its best subpalette.
    fn mse(&self, tiles: &[TileData]) -> f32 {
        let mut total = 0.0f64;
        let mut count = 0u64;
        for tile in tiles {
            if tile.is_empty(self.settings) {
                continue;
            }
            let (sp_idx, _) = self.best_fit(tile);
            let sp_view = self.subpalettes[sp_idx].view();
            sp_view.for_each_match(tile, self.settings.dither, |_, d, c| {
                total += f64::from(d) * f64::from(c);
                count += u64::from(c);
                Some(())
            });
        }
        if count == 0 {
            0.0
        } else {
            (total / count as f64) as f32
        }
    }

    fn quantize(&mut self) {
        for sp in &mut self.subpalettes {
            sp.quantize();
        }
    }
}

/// A single subpalette: colors and their 5-bit-snapped forms.
#[derive(Clone)]
struct Subpalette {
    colors: Vec<Oklab>,
    quantized: Vec<Oklab>,
    settings: Settings,
    /// Only deep pixels can use this subpalette.
    deep_only: bool,
}

impl Subpalette {
    fn from_colors(colors: Vec<Oklab>, settings: Settings) -> Self {
        let quantized = if settings.is_dithered() {
            colors.iter().map(|&c| settings.quantize_color(c)).collect()
        } else {
            colors.clone()
        };
        Subpalette {
            colors,
            quantized,
            settings,
            deep_only: false,
        }
    }

    fn deep_only(colors: Vec<Oklab>, settings: Settings) -> Self {
        Subpalette {
            deep_only: true,
            ..Subpalette::from_colors(colors, settings)
        }
    }

    /// A subpalette of the same kind with other colors.
    fn with_colors(&self, colors: Vec<Oklab>) -> Self {
        Subpalette {
            deep_only: self.deep_only,
            ..Subpalette::from_colors(colors, self.settings)
        }
    }

    fn len(&self) -> usize {
        self.colors.len()
    }

    fn view(&self) -> SubpaletteView<'_> {
        SubpaletteView {
            colors: &self.colors,
            quantized: &self.quantized,
        }
    }

    fn nudge(&mut self, index: usize, target: Oklab, alpha: f32) {
        let color = &mut self.colors[index];
        color.l = (1.0 - alpha) * color.l + alpha * target.l;
        color.a = (1.0 - alpha) * color.a + alpha * target.a;
        color.b = (1.0 - alpha) * color.b + alpha * target.b;
        if self.settings.is_dithered() {
            self.quantized[index] = self.settings.quantize_color(self.colors[index]);
        }
    }

    fn duplicate_color(&mut self, index: usize) {
        self.colors.push(self.colors[index]);
        self.quantized.push(self.quantized[index]);
    }

    fn copy_color(&mut self, from: usize, to: usize) {
        self.colors[to] = self.colors[from];
        self.quantized[to] = self.quantized[from];
    }

    fn quantize(&mut self) {
        for color in &mut self.colors {
            *color = self.settings.quantize_color(*color);
        }
        self.quantized = self.colors.clone();
    }
}

/// A read-only view over a subpalette's colors.
#[derive(Clone, Copy)]
struct SubpaletteView<'a> {
    colors: &'a [Oklab],
    quantized: &'a [Oklab],
}

impl<'a> SubpaletteView<'a> {
    fn nearest(&self, color: Oklab) -> (usize, f32) {
        self.nearest_exclude(color, None)
    }

    fn nearest_exclude(&self, color: Oklab, exclude: Option<usize>) -> (usize, f32) {
        let mut best = (0usize, f32::INFINITY);
        for (i, &c) in self.colors.iter().enumerate() {
            if Some(i) == exclude {
                continue;
            }
            let d = oklab_sqdist(c, color);
            if d < best.1 {
                best = (i, d);
            }
        }
        best
    }

    fn nearest_two(&self, color: Oklab) -> (usize, f32, f32) {
        smallest_two(self.colors.iter().map(|&c| oklab_sqdist(c, color)))
    }

    fn nearest_dithered(
        &self,
        pattern: Dither,
        x: u32,
        y: u32,
        color: Oklab,
        exclude: Option<usize>,
    ) -> (usize, f32, Oklab) {
        let (index, dist, biased, _) = self.candidates(pattern, color, exclude)[pattern.rank(x, y)];
        (index, dist, biased)
    }

    /// Calls `f(index, dist, count)` for every match, stopping if `f` returns `None`.
    fn for_each_match(
        &self,
        tile: &TileData,
        dither: Option<Dither>,
        mut f: impl FnMut(usize, f32, u32) -> Option<()>,
    ) -> Option<()> {
        if let Some(pattern) = dither {
            let mut matcher = self.matcher(pattern, tile);
            for pixel in 0..tile.pixels.len() {
                let (index, dist) = matcher.best(pixel);
                f(index, dist, tile.weight)?;
            }
        } else {
            for (&color, &count) in tile.colors.iter().zip(&tile.counts) {
                let (index, dist) = self.nearest(color);
                f(index, dist, count)?;
            }
        }
        Some(())
    }

    /// Calls `f(index, dist, second_dist, count)` for every match, never stopping early.
    fn for_each_match_two(
        &self,
        tile: &TileData,
        dither: Option<Dither>,
        mut f: impl FnMut(usize, f32, f32, u32),
    ) {
        if let Some(pattern) = dither {
            let mut matcher = self.matcher(pattern, tile);
            for pixel in 0..tile.pixels.len() {
                let (index, dist) = matcher.best(pixel);
                let second = matcher.second_dist(pixel);
                f(index, dist, second, tile.weight);
            }
        } else {
            for (&color, &count) in tile.colors.iter().zip(&tile.counts) {
                let (index, dist, second) = self.nearest_two(color);
                f(index, dist, second, count);
            }
        }
    }

    fn matcher(self, pattern: Dither, tile: &'a TileData) -> Matcher<'a> {
        Matcher {
            view: self,
            pattern,
            tile,
            candidates: vec![None; tile.colors.len()],
            second: Vec::new(),
        }
    }

    /// Candidate matches for `color`, ordered by brightness.
    fn candidates(
        &self,
        pattern: Dither,
        color: Oklab,
        exclude: Option<usize>,
    ) -> Candidates {
        let mut error = (0.0f32, 0.0f32, 0.0f32);
        let mut candidates: Candidates =
            [(0usize, 0.0f32, Oklab::new(0.0, 0.0, 0.0), 0.0f32); MAX_CANDIDATES];

        for candidate in &mut candidates[..pattern.candidates()] {
            let biased = Oklab::new(
                color.l + error.0 * DITHER_WEIGHT,
                color.a + error.1 * DITHER_WEIGHT,
                color.b + error.2 * DITHER_WEIGHT,
            );
            let (index, dist) = self.nearest_exclude(biased, exclude);
            let brightness = self.colors[index].l;
            *candidate = (index, dist, biased, brightness);

            let reduced = self.quantized[index];
            error.0 += color.l - reduced.l;
            error.1 += color.a - reduced.a;
            error.2 += color.b - reduced.b;
        }

        candidates[..pattern.candidates()].sort_by(|a, b| a.3.total_cmp(&b.3));
        candidates
    }

    fn distance(&self, tile: &TileData, dither: Option<Dither>) -> f32 {
        self.distance_within(tile, dither, f32::INFINITY, Tie::Win)
            .unwrap_or(f32::INFINITY)
    }

    fn distance_within(
        &self,
        tile: &TileData,
        dither: Option<Dither>,
        limit: f32,
        tie: Tie,
    ) -> Option<f32> {
        let mut sum = 0.0f32;
        self.for_each_match(tile, dither, |_, dist, count| {
            sum += dist * count as f32;
            let over = sum > limit || (sum == limit && tie == Tie::Lose);
            (!over).then_some(())
        })?;
        Some(sum)
    }
}

/// Candidates of one color: index, squared distance, biased color, brightness.
type Candidates = [(usize, f32, Oklab, f32); MAX_CANDIDATES];

/// Matches a tile's pixels against one subpalette when dithering.
struct Matcher<'a> {
    view: SubpaletteView<'a>,
    pattern: Dither,
    tile: &'a TileData,
    candidates: Vec<Option<Candidates>>,
    second: Vec<[Option<f32>; MAX_CANDIDATES]>,
}

impl Matcher<'_> {
    fn best(&mut self, pixel: usize) -> (usize, f32) {
        let p = self.tile.pixels[pixel];
        let (view, pattern, color) = (self.view, self.pattern, self.tile.colors[p.color]);
        let candidates =
            self.candidates[p.color].get_or_insert_with(|| view.candidates(pattern, color, None));
        let (index, dist, _, _) = candidates[pattern.rank(p.x, p.y)];
        (index, dist)
    }

    fn second_dist(&mut self, pixel: usize) -> f32 {
        let p = self.tile.pixels[pixel];
        let (index, _) = self.best(pixel);
        if self.second.is_empty() {
            self.second = vec![[None; MAX_CANDIDATES]; self.tile.colors.len()];
        }
        let (view, pattern, color) = (self.view, self.pattern, self.tile.colors[p.color]);
        let rank = pattern.rank(p.x, p.y);
        *self.second[p.color][rank]
            .get_or_insert_with(|| view.candidates(pattern, color, Some(index))[rank].1)
    }
}

/// Seeds single-color palettes by splitting off the worst-fitting palette.
fn seed_palette(
    tiles: &[TileData],
    samples: &[Sample],
    cycle: &mut SampleCycle,
    max_subpalettes: usize,
    alpha: f32,
    iterations: usize,
    settings: Settings,
) -> Palette {
    let mean = mean_oklab(samples.iter().map(|s| s.color)).expect("samples are non-empty");
    let mut palette = Palette::seeded(mean, settings);
    let mut split_index = 0usize;

    for _ in 1..max_subpalettes {
        palette.duplicate_palette(split_index);
        nudge(&mut palette, tiles, samples, cycle, alpha, iterations);

        let mut distances = vec![0.0f32; palette.len()];
        for tile in tiles {
            if tile.is_empty(settings) {
                continue;
            }
            let (sp_index, dist) = palette.best_fit(tile);
            distances[sp_index] += dist;
        }
        split_index = argmax(&distances);
    }
    palette
}

/// Grows every subpalette that is below its capacity by one color, splitting
/// the color that carries the most error.
#[allow(clippy::too_many_arguments)]
fn grow_palette(
    palette: &mut Palette,
    tiles: &[TileData],
    samples: &[Sample],
    cycle: &mut SampleCycle,
    alpha: f32,
    iterations: usize,
    capacity: usize,
    deep_capacity: usize,
) {
    let mut total_color_distances: Vec<Vec<f32>> = palette
        .subpalettes()
        .iter()
        .map(|sp| vec![0.0f32; sp.len()])
        .collect();
    if palette.subpalettes().iter().any(|sp| sp.len() > 1) {
        for tile in tiles {
            if tile.colors.is_empty() {
                continue;
            }
            let (sp_index, _) = palette.best_fit(tile);
            let view = palette.subpalettes()[sp_index].view();
            view.for_each_match(tile, palette.settings().dither, |i, d, c| {
                total_color_distances[sp_index][i] += d * c as f32;
                Some(())
            });
        }
    }

    for (idx, distances) in total_color_distances.iter().enumerate() {
        let sp = &palette.subpalettes()[idx];
        let cap = if sp.deep_only { deep_capacity } else { capacity };
        if sp.len() < cap {
            let split_idx = if sp.len() > 1 { argmax(distances) } else { 0 };
            palette.duplicate_color(idx, split_idx);
        }
    }

    nudge(palette, tiles, samples, cycle, alpha, iterations);
}

/// Replaces the weakest color and subpalette with a copy of the best.
fn replace_weakest(
    palette: &Palette,
    tiles: &[TileData],
    min_color_factor: f32,
    min_palette_factor: f32,
) -> Palette {
    let settings = palette.settings();
    let n = palette.len();
    let regular = palette.regular();
    let mut nearest_palette_of = vec![0usize; tiles.len()];
    let mut total_palette_mse = vec![0.0f32; n];
    let mut removed_palette_mse = vec![0.0f32; n];
    let (mut max_palette_index, mut min_palette_index) = (0usize, 0usize);

    if n > 1 {
        for (j, tile) in tiles.iter().enumerate() {
            let (index, min_dist, second_dist) = palette.best_two(tile);
            total_palette_mse[index] += min_dist;
            removed_palette_mse[index] += second_dist;
            nearest_palette_of[j] = index;
        }
        // Only the regular subpalettes are candidates for a whole-palette swap.
        max_palette_index = argmax(&total_palette_mse[..regular]);
        min_palette_index = argmin(&removed_palette_mse[..regular]);
    }

    let mut replaced = palette.clone();

    if palette.subpalettes()[0].len() > 1 {
        let mut total_color_mse: Vec<Vec<f32>> = palette
            .subpalettes()
            .iter()
            .map(|sp| vec![0.0f32; sp.len()])
            .collect();
        let mut second_color_mse: Vec<Vec<f32>> = palette
            .subpalettes()
            .iter()
            .map(|sp| vec![0.0f32; sp.len()])
            .collect();

        for (j, tile) in tiles.iter().enumerate() {
            let subpalette_index = nearest_palette_of[j];
            let view = palette.subpalettes()[subpalette_index].view();
            view.for_each_match_two(tile, settings.dither, |i, dist, second_dist, c| {
                total_color_mse[subpalette_index][i] += dist * c as f32;
                second_color_mse[subpalette_index][i] += second_dist * c as f32;
            });
        }

        for palette_index in 0..n {
            let max_color_index = argmax(&total_color_mse[palette_index]);
            let min_color_index = argmin(&second_color_mse[palette_index]);
            let should_replace = min_color_index != max_color_index
                && second_color_mse[palette_index][min_color_index]
                    < min_color_factor * total_color_mse[palette_index][max_color_index];
            if should_replace {
                replaced.copy_color(palette_index, max_color_index, min_color_index);
            }
        }
    }

    if regular > 1
        && min_palette_index != max_palette_index
        && removed_palette_mse[min_palette_index]
            < min_palette_factor * total_palette_mse[max_palette_index]
    {
        replaced.copy_subpalette(max_palette_index, min_palette_index);
    }

    replaced
}

/// Draws `count` samples and nudges the nearest color in each best subpalette.
fn nudge(
    palette: &mut Palette,
    tiles: &[TileData],
    samples: &[Sample],
    cycle: &mut SampleCycle,
    alpha: f32,
    count: usize,
) {
    for _ in 0..count {
        let sample = &samples[cycle.next()];
        palette.nudge(&tiles[sample.tile], sample, alpha);
    }
}

/// Recenters every subpalette color on the mean of the samples assigned to it.
fn refine(palette: &Palette, tiles: &[TileData]) -> Palette {
    let settings = palette.settings();
    let mut counts: Vec<Vec<u32>> = palette
        .subpalettes()
        .iter()
        .map(|sp| vec![0u32; sp.len()])
        .collect();
    let mut sums: Vec<Vec<(f32, f32, f32)>> = palette
        .subpalettes()
        .iter()
        .map(|sp| vec![(0.0, 0.0, 0.0); sp.len()])
        .collect();

    for tile in tiles {
        if tile.is_empty(settings) {
            continue;
        }
        let (subpalette_index, _) = palette.best_fit(tile);
        let view = palette.subpalettes()[subpalette_index].view();
        for (&color, &count) in tile.colors.iter().zip(&tile.counts) {
            let (color_index, _) = view.nearest(color);
            counts[subpalette_index][color_index] += count;
            let s = &mut sums[subpalette_index][color_index];
            s.0 += color.l * count as f32;
            s.1 += color.a * count as f32;
            s.2 += color.b * count as f32;
        }
    }

    let subpalettes = palette
        .subpalettes()
        .iter()
        .enumerate()
        .map(|(spi, sp)| {
            let colors = sp
                .colors
                .iter()
                .enumerate()
                .map(|(ci, &current)| {
                    let n = counts[spi][ci];
                    if n == 0 {
                        current
                    } else {
                        let (l, a, b) = sums[spi][ci];
                        Oklab::new(l / n as f32, a / n as f32, b / n as f32)
                    }
                })
                .collect();
            sp.with_colors(colors)
        })
        .collect();

    Palette::new(subpalettes, settings)
}
