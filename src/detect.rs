// SPDX-License-Identifier: GPL-3.0-only
//! Reads the grid of a sheet that the book says nothing about.
//!
//! A grid is square to the image, so the columns and the rows are two
//! separate problems of one dimension. Turn the image on its side and the
//! same reasoning answers both.
//!
//! One signal carries everything. For each column, add up how much it
//! differs from the column to its right, down the whole height. A boundary
//! between tiles stands where that number is large.
//!
//! A column either holds a boundary or it does not, so the signal becomes
//! yes or no: loud enough to stand out of the sheet, or not. This keeps one
//! very loud edge, the kind an empty margin makes against a solid tile,
//! from speaking for a whole slot.
//!
//! Then fold that onto itself. For a candidate pitch, gather every sample
//! that falls in the same slot of the fold, and ask what share of them are
//! boundaries. The true pitch drops every boundary into one slot, so its
//! best slot answers yes nearly every time.
//!
//! Share alone is not enough, because how often a slot was asked matters as
//! much as how often it said yes. A pitch of twice the truth answers yes
//! every time too, on half as many turns. A pitch of half the truth is
//! asked twice as often and says yes half the time. Weighing the share by
//! the root of the turns separates all three, and it is the same weighing
//! that a proportion always needs: an answer from two turns is a guess, and
//! an answer from fifty is a measurement.
//!
//! The rest reads off the fold. A gap makes two slots tall instead of one:
//! the tile ends against the gap, and the gap ends against the next tile.
//! The way round from one to the other is the gap, the pitch less the gap
//! is the tile, and a tile opens one place after the slot that closes the
//! gap.
//!
//! One thing the fold cannot tell is whether the pixels before the first
//! tile are a margin or a tile that the edge cut. The near columns of the
//! image answer it: flat colour is a margin, and anything else is a tile
//! that starts before the edge, which is an offset below zero.
//!
//! Last, the sheet must convince. A mockup or a splash screen holds no
//! grid, and a search always returns something, so a grid that does not
//! stand clear of the noise gives way to the size the folder used last.

use image::{Rgba, RgbaImage};

/// The grid of one axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub tile: u32,
    pub gap: u32,
    pub offset: i32,
}

/// The most a gap is ever likely to be.
const MAX_GAP: u32 = 8;
/// Below this, a tile is not a tile.
const MIN_TILE: u32 = 4;
/// A tile larger than this is one picture, not a sheet.
const MAX_TILE: u32 = 512;
/// How tall a second slot must stand to be the far side of a gap.
const PEAK: f32 = 0.5;
/// How many standard errors the grid needs before the pixels decide. Below
/// this they have not made their case.
const SURE: f32 = 8.0;

/// Reads the grid off the pixels. `hint` is the tile size the folder used
/// last, which decides a sheet that the pixels cannot.
pub fn grid(img: &RgbaImage, hint: [u32; 2]) -> (Axis, Axis) {
    let (w, h) = (img.width(), img.height());
    let dx: Vec<f32> = (0..w.saturating_sub(1))
        .map(|x| (0..h).map(|y| diff(img.get_pixel(x, y), img.get_pixel(x + 1, y))).sum::<f32>() / h as f32)
        .collect();
    let dy: Vec<f32> = (0..h.saturating_sub(1))
        .map(|y| (0..w).map(|x| diff(img.get_pixel(x, y), img.get_pixel(x, y + 1))).sum::<f32>() / w as f32)
        .collect();
    // How many lines at the near edge hold one colour.
    let flat_x = (0..w).take_while(|&x| (1..h).all(|y| img.get_pixel(x, y) == img.get_pixel(x, 0))).count() as u32;
    let flat_y = (0..h).take_while(|&y| (1..w).all(|x| img.get_pixel(x, y) == img.get_pixel(0, y))).count() as u32;
    (axis(&dx, flat_x, hint[0]), axis(&dy, flat_y, hint[1]))
}

/// How much two pixels differ. Colour under a transparent pixel is never
/// drawn, so it counts for as much as the two pixels are opaque.
fn diff(a: &Rgba<u8>, b: &Rgba<u8>) -> f32 {
    let seen = (a[3] as f32 + b[3] as f32) / 510.0;
    let colour: f32 = (0..3).map(|i| (a[i] as f32 - b[i] as f32).abs()).sum();
    (a[3] as f32 - b[3] as f32).abs() + colour * seen
}

/// Folds the signal onto one pitch and leaves the share of each slot in
/// `slot`. Returns how many whole turns the fold made.
fn fold(d: &[f32], pitch: u32, slot: &mut Vec<f32>) -> u32 {
    let p = pitch as usize;
    slot.clear();
    slot.resize(p, 0.0);
    for (i, v) in d.iter().enumerate() {
        slot[i % p] += v;
    }
    // The last turn is short, so the first slots hold one sample more.
    let (turns, over) = (d.len() / p, d.len() % p);
    for (k, v) in slot.iter_mut().enumerate() {
        *v /= (turns + usize::from(k < over)).max(1) as f32;
    }
    turns as u32
}

/// The tallest slot, and the tallest one somewhere else.
fn two_peaks(slot: &[f32]) -> (usize, usize) {
    let top = (0..slot.len()).max_by(|&a, &b| slot[a].total_cmp(&slot[b])).unwrap_or(0);
    let next = (0..slot.len()).filter(|&k| k != top).max_by(|&a, &b| slot[a].total_cmp(&slot[b])).unwrap_or(top);
    (top, next)
}

/// The best grid for one axis. `flat` is how many lines at the near edge
/// hold one colour, which says whether an offset runs forward or back.
fn axis(d: &[f32], flat: u32, hint: u32) -> Axis {
    let len = d.len() as u32 + 1;
    let fallback = Axis { tile: hint.clamp(MIN_TILE, len.max(MIN_TILE)), gap: 0, offset: 0 };
    let high = (len / 2).min(MAX_TILE + MAX_GAP);
    if d.is_empty() || high < MIN_TILE {
        return fallback;
    }
    let mean = d.iter().sum::<f32>() / d.len() as f32;
    let sd = (d.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / d.len() as f32).sqrt();
    if sd <= f32::EPSILON {
        return fallback;
    }
    // Loud enough to be a boundary, or not. What share of a sheet answers
    // yes is what a slot has to beat.
    let hot: Vec<f32> = d.iter().map(|v| f32::from(*v >= mean + sd)).collect();
    let share = hot.iter().sum::<f32>() / hot.len() as f32;
    if share <= 0.0 || share >= 1.0 {
        return fallback;
    }
    let error = (share * (1.0 - share)).sqrt();

    // How far each pitch stands above chance, in standard errors.
    let mut slot = Vec::new();
    let (mut pitch, mut top) = (0, 0.0);
    for p in MIN_TILE..=high {
        let turns = fold(&hot, p, &mut slot);
        let z = (slot[two_peaks(&slot).0] - share) * (turns as f32).sqrt() / error;
        if z > top {
            (pitch, top) = (p, z);
        }
    }
    // The pixels decide only when they stand clear of the noise.
    if top < SURE {
        return fallback;
    }

    fold(&hot, pitch, &mut slot);
    let (top, next) = two_peaks(&slot);
    let p = slot.len();
    let (ahead, back) = ((next + p - top) % p, (top + p - next) % p);
    // The way round to the second slot is the gap, and the way back is the
    // tile, so the gap is the shorter of the two.
    let paired = next != top && slot[next] >= slot[top] * PEAK && ahead.min(back) <= MAX_GAP as usize;
    let (gap, closes) = match (paired, ahead <= back) {
        (true, true) => (ahead as u32, next),
        (true, false) => (back as u32, top),
        (false, _) => (0, top),
    };
    if pitch <= gap || pitch - gap < MIN_TILE {
        return fallback;
    }
    // A tile opens one place after the slot that closes the gap.
    let offset = (closes as u32 + 1) % pitch;
    // Flat colour before the first tile is a margin. Anything else is a
    // tile that the edge cut, and that offset runs back, not forward.
    let back = offset > 0 && flat < offset;
    let offset = if back { offset as i32 - pitch as i32 } else { offset as i32 };
    Axis { tile: pitch - gap, gap, offset }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sheet of flat tiles, each one its own colour. Eight tiles across,
    /// because a sheet of four gives only three lines to judge by, and
    /// three lines are not enough to be sure of anything.
    fn flat(tile: u32, gap: u32, offset: u32, count: u32) -> RgbaImage {
        let len = offset + count * (tile + gap) - gap;
        RgbaImage::from_fn(len, len, |x, y| {
            if x < offset || y < offset {
                return Rgba([0, 0, 0, 0]);
            }
            let (cx, cy) = ((x - offset) % (tile + gap), (y - offset) % (tile + gap));
            if cx >= tile || cy >= tile {
                return Rgba([0, 0, 0, 0]);
            }
            let (tx, ty) = ((x - offset) / (tile + gap), (y - offset) / (tile + gap));
            let v = (tx * 7 + ty * 53) as u8;
            Rgba([v.wrapping_mul(9), 255 - v, v.wrapping_add(90), 255])
        })
    }

    #[test]
    fn it_reads_a_plain_grid() {
        let (x, y) = grid(&flat(16, 0, 0, 8), [8, 8]);
        assert_eq!(x, Axis { tile: 16, gap: 0, offset: 0 });
        assert_eq!(y, Axis { tile: 16, gap: 0, offset: 0 });
    }

    #[test]
    fn it_reads_the_gap_and_the_offset() {
        let (x, y) = grid(&flat(16, 2, 3, 8), [8, 8]);
        assert_eq!(x, Axis { tile: 16, gap: 2, offset: 3 });
        assert_eq!(y, Axis { tile: 16, gap: 2, offset: 3 });
    }

    /// A sheet need not end on a whole tile. Slack at the far edge used to
    /// move to the near edge and take every tile with it.
    #[test]
    fn slack_at_the_far_edge_moves_nothing() {
        let tiles = flat(16, 0, 0, 8);
        let img = RgbaImage::from_fn(tiles.width() + 9, tiles.height() + 9, |x, y| {
            if x < tiles.width() && y < tiles.height() { *tiles.get_pixel(x, y) } else { Rgba([0, 0, 0, 0]) }
        });
        assert_eq!(grid(&img, [8, 8]).0, Axis { tile: 16, gap: 0, offset: 0 });
    }

    /// A tile that the edge cut sits at an offset below zero.
    #[test]
    fn a_cut_first_tile_offsets_backwards() {
        let whole = flat(16, 0, 0, 9);
        let img = RgbaImage::from_fn(whole.width() - 6, whole.height(), |x, y| *whole.get_pixel(x + 6, y));
        assert_eq!(grid(&img, [8, 8]).0, Axis { tile: 16, gap: 0, offset: -6 });
    }

    /// Tiles drawn at 32 with no seam down the middle. Half of a 16 grid
    /// would stand on quiet pixels, so 16 must lose.
    #[test]
    fn it_does_not_halve_a_tile() {
        let img = RgbaImage::from_fn(256, 256, |x, y| {
            let (tx, ty) = (x / 32, y / 32);
            let ramp = ((x % 32) + (y % 32)) as u8 * 3;
            let base = ((tx * 4 + ty) as u8).wrapping_mul(37);
            Rgba([base.wrapping_add(ramp), 120, 200, 255])
        });
        assert_eq!(grid(&img, [8, 8]).0.tile, 32);
    }

    /// A checkerboard of 16. A 32 grid holds a loud line inside every tile,
    /// so 32 must lose.
    #[test]
    fn it_does_not_double_a_tile() {
        let img = RgbaImage::from_fn(128, 128, |x, y| {
            let c = if (x / 16 + y / 16) % 2 == 0 { 240 } else { 30 };
            Rgba([c, c, c, 255])
        });
        assert_eq!(grid(&img, [64, 64]).0.tile, 16);
    }

    /// Nothing to read: the pixels say nothing, so the hint stands.
    #[test]
    fn an_empty_sheet_keeps_the_hint() {
        let img = RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 0]));
        assert_eq!(grid(&img, [24, 24]).0.tile, 24);
    }
}
