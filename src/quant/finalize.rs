//! Finalizing the quantizer's palettes, as the reference does.
//!
//! After the incremental quantizer returns subpalettes in Oklab, the reference
//! in `quantize_image` reshapes each one before packing:
//! 1. reduce every color to native depth and drop duplicates,
//! 2. truncate to the color capacity,
//! 3. if fewer than the capacity survive, refit the tiles assigned to that
//!    subpalette with Wu's method and k-means until the palette fills,
//! 4. put the color-zero entry at index 0,
//! 5. sort by a perceptual key.
//!
//! This is what turns the Oklab result into the packed palette, so the port
//! must do it too or the packed colors differ even when the Oklab does not.

use quantette::{
    kmeans::{Kmeans, KmeansOptions},
    wu::{BinnerF32x3, WuF32x3},
    PaletteSize,
};

use crate::quant::color::{oklab_to_rgb8, Oklab, Rgb};

/// quantette's own Oklab, used for the Wu and k-means fit.
type QOklab = quantette::deps::palette::Oklab;

/// Number of refit attempts when a palette is short of the capacity.
const REFIT_ATTEMPTS: usize = 4;

/// Reduces a color list to native depth, dropping duplicates and transparent.
fn dedup_reduced(colors: &[Oklab]) -> Vec<Rgb> {
    let mut reduced: Vec<Rgb> = Vec::new();
    for &color in colors {
        let rgb = oklab_to_rgb8(color).reduce();
        if rgb != Rgb::new(0, 0, 0) && !reduced.contains(&rgb) {
            reduced.push(rgb);
        }
    }
    reduced
}

/// Fits `capacity` colors to `colors` with Wu's method then k-means.
fn fit_palette(colors: &[Oklab], capacity: usize) -> Vec<Oklab> {
    if colors.is_empty() {
        return Vec::new();
    }
    let Some(k) = PaletteSize::try_from_u16(capacity.min(u16::MAX as usize) as u16) else {
        return Vec::new();
    };
    let colors: Vec<QOklab> = colors
        .iter()
        .map(|c| QOklab::new(c.l, c.a, c.b))
        .collect();
    let binner = BinnerF32x3::oklab_from_srgb8();
    let Ok(wu) = WuF32x3::run_slice(&colors, binner) else {
        return Vec::new();
    };
    let seeds = wu.palette(k);
    let Ok(kmeans) = Kmeans::run_slice(&colors, seeds, KmeansOptions::new()) else {
        return Vec::new();
    };
    kmeans
        .into_palette()
        .into_iter()
        .map(|c| Oklab::new(c.l, c.a, c.b))
        .collect()
}

/// Fills `current` to `capacity` by refitting `colors`, as the reference does.
fn grow_to_capacity(mut current: Vec<Rgb>, colors: &[Oklab], capacity: usize) -> Vec<Rgb> {
    let mut k = capacity;
    for _ in 0..REFIT_ATTEMPTS {
        if current.len() >= capacity || k >= colors.len() {
            break;
        }
        k += capacity - current.len();
        let mut next = dedup_reduced(&fit_palette(colors, k));
        next.truncate(capacity);
        if next.len() > current.len() {
            current = next;
        }
    }
    current
}

/// Perceived luma in 0..=1, as the reference computes it.
fn perceived_luma(color: Rgb) -> f32 {
    let linear = |v: u8| {
        let v = f32::from(v) / f32::from(u8::MAX);
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (linear(color.r), linear(color.g), linear(color.b));
    (r * r * 0.299 + g * g * 0.587 + b * b * 0.114).sqrt()
}

/// The reference's perceptual sort key: hue band, then luma, then max channel.
fn visual_sort_key(color: Rgb) -> (f32, f32, f32) {
    let r = f32::from(color.r) / f32::from(u8::MAX);
    let g = f32::from(color.g) / f32::from(u8::MAX);
    let b = f32::from(color.b) / f32::from(u8::MAX);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    let luma = perceived_luma(color);
    let sat = if max <= 0.0 { 0.0 } else { delta / (max + min) };
    let band = 360.0 / 8.0;
    let hue_rot = (hue + band / 2.0) % 360.0;
    let hue_grouped = if sat < 0.005 { -1.0 } else { (hue_rot / band).round() };
    (hue_grouped, luma, max)
}

/// Builds the final packed palettes from the quantizer's Oklab subpalettes.
pub(crate) fn finalize(
    subpalettes: &[Vec<Oklab>],
    tile_palette: &[usize],
    tile_colors: &[Vec<Oklab>],
    capacity: usize,
    color_zero: Option<Rgb>,
) -> Vec<Vec<Rgb>> {
    let zero = color_zero.map_or(Rgb::new(0, 0, 0), |c| c.reduce());
    subpalettes
        .iter()
        .enumerate()
        .map(|(index, subpalette)| {
            let mut reduced = dedup_reduced(subpalette);
            reduced.truncate(capacity);
            if reduced.len() < capacity {
                let assigned: Vec<Oklab> = tile_colors
                    .iter()
                    .zip(tile_palette)
                    .filter(|&(_, &assigned)| assigned == index)
                    .flat_map(|(colors, _)| colors.iter().copied())
                    .collect();
                reduced = grow_to_capacity(reduced, &assigned, capacity);
            }
            // Index 0 is the shared color zero.
            reduced.retain(|&c| c != zero);
            reduced.insert(0, zero);
            let zero_entry = reduced[0];
            let mut rest: Vec<Rgb> = reduced[1..].to_vec();
            rest.sort_by(|a, b| {
                visual_sort_key(*a)
                    .partial_cmp(&visual_sort_key(*b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut out = vec![zero_entry];
            out.extend_from_slice(&rest);
            // The palette is native 5-bit; expose it scaled to 8-bit.
            out.into_iter().map(|c| c.normalize()).collect()
        })
        .collect()
}

/// Builds the deep-only colors from the quantizer's Oklab list.
///
/// A color that is already in a subpalette, or that is color zero, is of no
/// use here, because a deep pixel reaches that color anyway. Such colors are
/// dropped, and the free entries are filled with color zero, unused.
pub(crate) fn finalize_deep(
    deep: &[Oklab],
    palettes: &[Vec<Rgb>],
    capacity: usize,
    color_zero: Option<Rgb>,
) -> Vec<Rgb> {
    let zero = color_zero.map_or(Rgb::new(0, 0, 0), |c| c.reduce());
    let taken: Vec<Rgb> = palettes.iter().flatten().map(|c| c.reduce()).collect();
    let mut reduced = dedup_reduced(deep);
    reduced.retain(|c| *c != zero && !taken.contains(c));
    reduced.truncate(capacity);
    reduced.resize(capacity, zero);
    reduced.into_iter().map(|c| c.normalize()).collect()
}
