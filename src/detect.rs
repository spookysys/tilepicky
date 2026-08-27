// SPDX-License-Identifier: GPL-3.0-only
//! Reads the grid of a sheet that the book says nothing about.
//!
//! A grid is square to the image, so the columns and the rows are two
//! separate problems of one dimension. Each axis gets one signal: how much
//! a line of pixels differs from the line before it. A grid line stands
//! where that difference is large, and the inside of a tile is quiet.
//!
//! A candidate grid must divide the sheet exactly, which leaves few of
//! them. Each one scores by how far its lines stand above the ordinary
//! difference of the sheet, counted in standard errors. The best wins.
//!
//! The count of lines belongs in the score, and this is why. A tile of half
//! the sheet has one line, and one lucky line beats an honest average of
//! fifty every time. Dividing by the square root of the number of lines
//! asks for evidence as well as effect, and a big tile brings little.
//!
//! The measure answers both ways a guess goes wrong. A tile that is too
//! small puts quiet lines among the loud ones and halves the effect. A tile
//! that is too big keeps the effect but throws half of its evidence away.

use image::{Rgba, RgbaImage};

/// The grid of one axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub tile: u32,
    pub gap: u32,
    pub offset: u32,
}

/// The most a gap is ever likely to be.
const MAX_GAP: u32 = 8;
/// Below this, a tile is not a tile.
const MIN_TILE: u32 = 4;
/// A tile larger than this is one picture, not a sheet.
const MAX_TILE: u32 = 512;
/// How many standard errors the winner needs before the pixels decide.
/// Below this they have not made their case. On real packs a true grid
/// comes in above nine, and everything that is wrong stays under eight.
const SURE: f32 = 8.0;

/// Reads the grid off the pixels. `hint` is the tile size the folder used
/// last, which decides a sheet that the pixels cannot.
pub fn grid(img: &RgbaImage, hint: [u32; 2]) -> (Axis, Axis) {
    (axis(&steps_x(img), img.width(), hint[0]), axis(&steps_y(img), img.height(), hint[1]))
}

/// How much two pixels differ. Colour under a transparent pixel is never
/// drawn, so it counts for as much as the two pixels are opaque.
fn diff(a: &Rgba<u8>, b: &Rgba<u8>) -> f32 {
    let seen = (a[3] as f32 + b[3] as f32) / 510.0;
    let colour: f32 = (0..3).map(|i| (a[i] as f32 - b[i] as f32).abs()).sum();
    (a[3] as f32 - b[3] as f32).abs() + colour * seen
}

/// How much each column differs from the column before it.
fn steps_x(img: &RgbaImage) -> Vec<f32> {
    let (w, h) = (img.width(), img.height());
    let mut d = vec![0.0; w as usize];
    for x in 1..w {
        let sum: f32 = (0..h).map(|y| diff(img.get_pixel(x - 1, y), img.get_pixel(x, y))).sum();
        d[x as usize] = sum / h as f32;
    }
    d
}

/// How much each row differs from the row above it.
fn steps_y(img: &RgbaImage) -> Vec<f32> {
    let (w, h) = (img.width(), img.height());
    let mut d = vec![0.0; h as usize];
    for y in 1..h {
        let sum: f32 = (0..w).map(|x| diff(img.get_pixel(x, y - 1), img.get_pixel(x, y))).sum();
        d[y as usize] = sum / w as f32;
    }
    d
}

/// The offsets that let a grid of this tile and gap land on the sheet
/// exactly. The last tile ends at the edge, or one last gap follows it,
/// and either way the offset follows from the length.
fn offsets(len: u32, period: u32, gap: u32) -> [u32; 2] {
    [(len + gap) % period, len % period]
}

/// How far the lines of this grid stand above the ordinary difference of
/// the sheet, in standard errors.
fn score(sum: &[f32], sq: &[f32], d: &[f32], len: u32, a: Axis) -> f32 {
    let period = a.tile + a.gap;
    let n = (len + a.gap).saturating_sub(a.offset) / period;
    if n < 2 {
        return 0.0;
    }
    // Where the last tile ends. The edge of the image is not a line: no
    // pixel stands on the far side of it to differ from.
    let last = (a.offset + n * period - a.gap).min(len - 1);
    let first = a.offset.max(1);
    if first > last {
        return 0.0;
    }
    let (mut on, mut lines) = (0.0, 0u32);
    // The start of each tile, and the start of each gap, are lines.
    for start in [0, a.tile] {
        if start == a.tile && a.gap == 0 {
            continue;
        }
        let mut x = a.offset + start;
        while x <= last {
            if x >= first {
                on += d[x as usize];
                lines += 1;
            }
            x += period;
        }
    }
    let span = (last - first + 1) as f32;
    if lines == 0 || span <= lines as f32 {
        return 0.0;
    }
    let all = sum[last as usize + 1] - sum[first as usize];
    let all_sq = sq[last as usize + 1] - sq[first as usize];
    let mean = all / span;
    let var = (all_sq / span - mean * mean).max(0.0);
    // A sheet of one flat colour says nothing, whatever grid you lay on it.
    if var <= f32::EPSILON {
        return 0.0;
    }
    (on / lines as f32 - mean) * (lines as f32).sqrt() / var.sqrt()
}

/// The best grid for one axis.
fn axis(d: &[f32], len: u32, hint: u32) -> Axis {
    let (mut sum, mut sq) = (vec![0.0; d.len() + 1], vec![0.0; d.len() + 1]);
    for (i, v) in d.iter().enumerate() {
        sum[i + 1] = sum[i] + v;
        sq[i + 1] = sq[i] + v * v;
    }
    let fallback = Axis { tile: hint.clamp(MIN_TILE, len.max(MIN_TILE)), gap: 0, offset: 0 };
    let (mut best, mut top) = (fallback, 0.0);
    for tile in MIN_TILE..=(len / 2).min(MAX_TILE) {
        for gap in 0..=MAX_GAP {
            let period = tile + gap;
            let mut seen = None;
            for offset in offsets(len, period, gap) {
                if seen == Some(offset) {
                    continue;
                }
                seen = Some(offset);
                let a = Axis { tile, gap, offset };
                let s = score(&sum, &sq, d, len, a);
                if s > top {
                    (best, top) = (a, s);
                }
            }
        }
    }
    // The pixels decide when they speak clearly. When they do not, the size
    // the folder used last decides, because a pack draws to one size, and a
    // sheet with little on it cannot say otherwise. A gap and an offset are
    // not guessed at all then: the best of a set of weak answers is still a
    // weak answer, and a wrong offset moves every tile on the screen.
    if top >= SURE { best } else { fallback }
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

    /// Tiles drawn at 32 with no seam down their middle. Half of a 16 grid
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
