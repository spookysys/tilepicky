//! Fills the free deep-only entries after the main run.
//!
//! The run grows the deep-only colors with the subpalettes, but with dither
//! it moves them so little that many round to the same 5-bit color, and
//! finalization drops the duplicates. This fill gives those entries a use.
//!
//! The subpalettes and the kept deep-only colors are fixed by then. Each deep pixel already has an error:
//! the distance to its nearest color among color zero and the subpalettes.
//! The deep-only colors go where that error is largest:
//! 1. Pick each color in turn, greedily: the deep color whose addition
//!    lowers the total error most.
//! 2. Refine with k-means passes that move only the new colors, each to the
//!    mean of the deep pixels that it serves.
//!
//! The fixed colors never move, so this cannot make the error larger than
//! the subpalettes alone give.

use std::collections::HashMap;

use crate::quant::color::{oklab_sqdist, srgb_to_oklab, Oklab};
use crate::quant::palette::TileData;

/// k-means passes after the greedy pick.
const REFINEMENT_ITERATIONS: usize = 10;

/// The deep pixels' unique colors, each with its total weight.
fn deep_colors(tiles: &[TileData]) -> Vec<(Oklab, f32)> {
    let mut weights: HashMap<[u32; 3], (Oklab, f32)> = HashMap::new();
    for tile in tiles.iter().filter(|t| t.deep) {
        for (&color, &count) in tile.colors.iter().zip(&tile.counts) {
            let key = [color.l.to_bits(), color.a.to_bits(), color.b.to_bits()];
            weights.entry(key).or_insert((color, 0.0)).1 += count as f32;
        }
    }
    let mut colors: Vec<(Oklab, f32)> = weights.into_values().collect();
    // A fixed order, so the result does not depend on the hash order.
    colors.sort_by(|a, b| {
        (a.0.l, a.0.a, a.0.b)
            .partial_cmp(&(b.0.l, b.0.a, b.0.b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    colors
}

/// Snaps an Oklab color to the 5-bit grid and back.
fn snap(color: Oklab) -> Oklab {
    srgb_to_oklab(color.to_rgb5())
}

/// Chooses up to `capacity` deep-only colors for the deep pixels of `tiles`,
/// with `fixed` as the colors they reach already.
pub(crate) fn fill_deep(tiles: &[TileData], fixed: &[Oklab], capacity: usize) -> Vec<Oklab> {
    let pixels = deep_colors(tiles);
    if pixels.is_empty() || capacity == 0 {
        return Vec::new();
    }
    let nearest = |color: Oklab, set: &[Oklab]| {
        set.iter().map(|&c| oklab_sqdist(color, c)).fold(f32::INFINITY, f32::min)
    };
    let fixed_error: Vec<f32> = pixels.iter().map(|&(c, _)| nearest(c, fixed)).collect();

    // 1. Greedy pick among the deep colors themselves.
    let mut error = fixed_error.clone();
    let mut chosen: Vec<Oklab> = Vec::new();
    while chosen.len() < capacity {
        let mut best = (0.0f32, None);
        for &(candidate, _) in &pixels {
            let gain: f32 = pixels
                .iter()
                .zip(&error)
                .map(|(&(c, w), &e)| (e - oklab_sqdist(c, candidate)).max(0.0) * w)
                .sum();
            if gain > best.0 {
                best = (gain, Some(candidate));
            }
        }
        let Some(candidate) = best.1 else { break };
        for ((c, _), e) in pixels.iter().zip(&mut error) {
            *e = e.min(oklab_sqdist(*c, candidate));
        }
        chosen.push(candidate);
    }

    // 2. k-means on the chosen colors only. A pixel that a fixed color
    //    serves better stays with it and does not pull.
    let total = |set: &[Oklab]| -> f32 {
        pixels
            .iter()
            .zip(&fixed_error)
            .map(|(&(c, w), &f)| f.min(nearest(c, set)) * w)
            .sum()
    };
    let mut best = (total(&chosen), chosen.clone());
    for _ in 0..REFINEMENT_ITERATIONS {
        let mut sums = vec![(0.0f32, 0.0f32, 0.0f32, 0.0f32); chosen.len()];
        for (&(c, w), &f) in pixels.iter().zip(&fixed_error) {
            let (index, d) = chosen
                .iter()
                .enumerate()
                .map(|(i, &k)| (i, oklab_sqdist(c, k)))
                .fold((0, f32::INFINITY), |a, b| if b.1 < a.1 { b } else { a });
            if d < f {
                let s = &mut sums[index];
                *s = (s.0 + c.l * w, s.1 + c.a * w, s.2 + c.b * w, s.3 + w);
            }
        }
        for (color, s) in chosen.iter_mut().zip(&sums) {
            if s.3 > 0.0 {
                *color = snap(Oklab::new(s.0 / s.3, s.1 / s.3, s.2 / s.3));
            }
        }
        let error = total(&chosen);
        if error < best.0 {
            best = (error, chosen.clone());
        } else {
            break;
        }
    }
    best.1
}
