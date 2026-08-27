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
//! Then fold that signal onto itself. For a candidate pitch, gather every
//! sample that falls in the same slot of the fold. If the pitch is the true
//! one, the samples in a slot agree with each other, because they are the
//! same place in the tile seen again and again.
//!
//! Do not ask whether one slot is loud. A tileset parts on a loud line,
//! where one colour meets another. A sheet of sprites parts on an empty
//! one, where nothing is drawn at all and the difference falls to zero.
//! Loud and quiet are the same fact: the sheet repeats. So ask instead how
//! much of the whole signal the fold accounts for, against how much it
//! leaves over. That one question answers both kinds of sheet.
//!
//! Asking it that way also pays for itself. A fold of five hundred slots
//! can account for almost anything, so the account is divided by the slots
//! it used and the leftover by the samples that remain. The ratio of the
//! two is near one when a pitch explains nothing, whatever its size, and it
//! climbs only when a pitch is real.
//!
//! Only the pitch is read. A gap and an offset can be read off the same
//! fold, and an earlier version did, but they were guessed from the same
//! evidence that had already been spent on the pitch, and they were wrong
//! often enough to move every tile on the screen. A sheet is read as
//! starting at its corner with its tiles touching until they can be found
//! as surely as the pitch.
//!
//! The two axes then help each other. A sheet is one picture, and a pitch
//! that runs across it usually runs down it as well, so a pitch both axes
//! support is worth more than a pitch one of them merely prefers. Where an
//! axis cannot make up its mind, and a tileset of soft edges often cannot,
//! the other axis breaks the tie. An axis that is sure keeps its own
//! answer, because a sprite is often taller than it is wide.
//!
//! Last, the sheet must convince. A mockup or a splash screen holds no
//! grid, and a search always returns something. A pitch that does not stand
//! clear of the noise is dropped, and the sheet is read as one whole tile,
//! which is what a picture with nothing to divide is.
//!
//! Three other measurements were tried and dropped, so that nobody spends
//! the day again.
//!
//! How much ink each line holds finds the pitch of a font sheet, which the
//! difference misses entirely, and costs more sheets elsewhere than it
//! saves. How much that ink varies along a line does the same, smaller.
//!
//! The third is the interesting one. Every place in a tile leaves a
//! fingerprint: a seam looks the same in every tile, a middle looks
//! different in each, so how far the samples of a slot stand apart should
//! name the pitch by itself. It does, and strongly. At the true pitch of
//! cave.png it carries five times the evidence the difference does.
//!
//! It needs one guard. The spread of a slot is measured against that
//! slot's own mean, and where a slot holds two samples that mean fits them
//! exactly, the spread collapses, and the ratio divides by nothing. Half
//! the sheet then scores six hundred against thirteen for the truth.
//! Refusing a pitch whose slots hold fewer than about twelve samples fixes
//! it, and leaving one sample out of its own mean does not, because with
//! two samples the two residuals come out equal whatever you do.
//!
//! Guarded, it finds 6 of the 17 sheets on its own. Added to the
//! difference, taken as the louder of the two, multiplied, or pooled, it
//! reaches 10, which is what the difference reaches alone. So the code
//! keeps the difference alone, and this paragraph keeps the reason.

use image::{Rgba, RgbaImage};

/// The grid of one axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub tile: u32,
    pub gap: u32,
    pub offset: i32,
}

/// Below this, a tile is not a tile.
const MIN_TILE: u32 = 4;
/// A tile larger than this is one picture, not a sheet.
const MAX_TILE: u32 = 512;
/// How much better than nothing a pitch must account for on its own. A
/// pitch that explains nothing scores about one, whatever its size, so this
/// asks for several times that.
const SURE: f32 = 5.0;
/// What a pitch must account for when the other axis names it too. Two
/// axes that agree are two measurements, so each needs to say less.
const AGREE: f32 = 2.0;

/// Reads the grid off the pixels, and off nothing else.
pub fn grid(img: &RgbaImage) -> (Axis, Axis) {
    let (w, h) = (img.width(), img.height());
    let dx: Vec<f32> = (0..w.saturating_sub(1))
        .map(|x| (0..h).map(|y| diff(img.get_pixel(x, y), img.get_pixel(x + 1, y))).sum::<f32>() / h as f32)
        .collect();
    let dy: Vec<f32> = (0..h.saturating_sub(1))
        .map(|y| (0..w).map(|x| diff(img.get_pixel(x, y), img.get_pixel(x, y + 1))).sum::<f32>() / w as f32)
        .collect();
    let (fx, fy) = (profile(&dx, w), profile(&dy, h));
    // The pitch both axes support: the one whose weaker showing is the
    // strongest. Taking the weaker of the two makes this a test of
    // agreement, not of one loud axis carrying a quiet one.
    let both = (0..fx.len().min(fy.len()))
        .max_by(|&a, &b| fx[a].min(fy[a]).total_cmp(&fx[b].min(fy[b])))
        .filter(|&i| fx[i].min(fy[i]) >= AGREE)
        .map(|i| i as u32 + MIN_TILE);
    (axis(&fx, w, both), axis(&fy, h, both))
}

/// How much two pixels differ. Colour under a transparent pixel is never
/// drawn, so it counts for as much as the two pixels are opaque.
fn diff(a: &Rgba<u8>, b: &Rgba<u8>) -> f32 {
    let seen = (a[3] as f32 + b[3] as f32) / 510.0;
    let colour: f32 = (0..3).map(|i| (a[i] as f32 - b[i] as f32).abs()).sum();
    (a[3] as f32 - b[3] as f32).abs() + colour * seen
}

/// Folds the signal onto one pitch and leaves the mean of each slot in
/// `slot`. Returns how much of the signal that fold accounts for, against
/// what it leaves over, with both divided by what they cost.
fn fold(d: &[f32], pitch: u32, mean: f32, total: f32, slot: &mut Vec<f32>) -> f32 {
    let (p, n) = (pitch as usize, d.len());
    slot.clear();
    slot.resize(p, 0.0);
    for (i, v) in d.iter().enumerate() {
        slot[i % p] += v;
    }
    // The last turn is short, so the first slots hold one sample more.
    let over = n % p;
    let mut held = 0.0;
    for (k, v) in slot.iter_mut().enumerate() {
        let turns = (n / p + usize::from(k < over)).max(1);
        *v /= turns as f32;
        held += turns as f32 * (*v - mean) * (*v - mean);
    }
    if p < 2 || n <= p {
        return 0.0;
    }
    let left = (total - held).max(f32::EPSILON);
    (held / (p - 1) as f32) / (left / (n - p) as f32)
}

/// How much each pitch from `MIN_TILE` upwards accounts for, on one axis.
fn profile(d: &[f32], len: u32) -> Vec<f32> {
    let high = (len / 2).min(MAX_TILE);
    if d.is_empty() || high < MIN_TILE {
        return Vec::new();
    }
    let mean = d.iter().sum::<f32>() / d.len() as f32;
    let total: f32 = d.iter().map(|v| (v - mean) * (v - mean)).sum();
    if total <= f32::EPSILON {
        return Vec::new();
    }
    let mut slot = Vec::new();
    (MIN_TILE..=high).map(|p| fold(d, p, mean, total, &mut slot)).collect()
}

/// The grid of one axis. `len` is the length of the axis, which is the
/// answer when no pitch convinces: a sheet that does not divide is one
/// tile. `both` is the pitch the two axes agree on, if there is one.
fn axis(f: &[f32], len: u32, both: Option<u32>) -> Axis {
    let whole = Axis { tile: len.max(1), gap: 0, offset: 0 };
    let Some(at) = (0..f.len()).max_by(|&a, &b| f[a].total_cmp(&f[b])) else {
        return whole;
    };
    // An axis speaks for itself when it is sure. When it is not, a pitch
    // the other axis names as well needs to say less to be believed.
    let pitch = if f[at] >= SURE {
        at as u32 + MIN_TILE
    } else {
        match both {
            Some(p) if f.get((p - MIN_TILE) as usize).is_some_and(|&v| v >= AGREE) => p,
            _ => return whole,
        }
    };

    // The gap and the offset are not read yet. A pitch is hard enough to
    // find on its own, and a wrong offset moves every tile on the screen,
    // so until they are as sure as the pitch is, a sheet starts at its
    // corner with its tiles touching.
    Axis { tile: pitch, gap: 0, offset: 0 }
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
        let (x, y) = grid(&flat(16, 0, 0, 8));
        assert_eq!(x, Axis { tile: 16, gap: 0, offset: 0 });
        assert_eq!(y, Axis { tile: 16, gap: 0, offset: 0 });
    }

    /// A gap is part of the pitch, and the pitch is all we read for now,
    /// so a sheet of 16 with a gap of 2 reads as 18.
    #[test]
    fn a_gap_counts_into_the_pitch() {
        let (x, y) = grid(&flat(16, 2, 3, 8));
        assert_eq!(x.tile, 18);
        assert_eq!(y.tile, 18);
    }

    /// A sheet need not end on a whole tile. Slack at the far edge used to
    /// move to the near edge and take every tile with it.
    #[test]
    fn slack_at_the_far_edge_moves_nothing() {
        let tiles = flat(16, 0, 0, 8);
        let img = RgbaImage::from_fn(tiles.width() + 9, tiles.height() + 9, |x, y| {
            if x < tiles.width() && y < tiles.height() { *tiles.get_pixel(x, y) } else { Rgba([0, 0, 0, 0]) }
        });
        assert_eq!(grid(&img).0, Axis { tile: 16, gap: 0, offset: 0 });
    }

    /// The pitch holds even when the first tile is cut by the edge. Where
    /// the grid then starts is a question for the offset, which is not read
    /// yet.
    #[test]
    fn a_cut_first_tile_keeps_the_pitch() {
        let whole = flat(16, 0, 0, 9);
        let img = RgbaImage::from_fn(whole.width() - 6, whole.height(), |x, y| *whole.get_pixel(x + 6, y));
        assert_eq!(grid(&img).0.tile, 16);
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
        assert_eq!(grid(&img).0.tile, 32);
    }

    /// A checkerboard of 16. A 32 grid holds a loud line inside every tile,
    /// so 32 must lose.
    #[test]
    fn it_does_not_double_a_tile() {
        let img = RgbaImage::from_fn(128, 128, |x, y| {
            let c = if (x / 16 + y / 16) % 2 == 0 { 240 } else { 30 };
            Rgba([c, c, c, 255])
        });
        assert_eq!(grid(&img).0.tile, 16);
    }

    /// The sheets of the real packs, from `tools/grid-cases.json`. The packs
    /// are not in the repository, so a case whose file is missing is skipped
    /// and this passes quietly on a machine without them.
    ///
    /// A case marked `pass` must keep passing. A case marked `fail` is a
    /// known weakness: it only prints, so that turning one into a pass is
    /// visible work rather than a broken build.
    #[test]
    fn the_real_packs_read_as_they_did() {
        let Ok(text) = std::fs::read_to_string("tools/grid-cases.json") else { return };
        let book: serde_json::Value = serde_json::from_str(&text).expect("grid-cases.json");
        let cases = book["cases"].as_array().expect("cases");
        let (mut broke, mut read) = (Vec::new(), 0);
        for case in cases {
            let file = case["file"].as_str().unwrap_or_default();
            let Ok(img) = image::open(file) else { continue };
            read += 1;
            let two = |name: &str| -> [u32; 2] {
                let v = &case[name];
                [0, 1].map(|i| v[i].as_u64().unwrap_or(0) as u32)
            };
            let (tile, gap) = (two("tile"), two("gap"));
            let want = [tile[0] + gap[0], tile[1] + gap[1]];
            let (x, y) = grid(&img.to_rgba8());
            let got = [x.tile, y.tile];
            match (case["expect"].as_str() == Some("pass"), got == want) {
                (true, false) => broke.push(format!("{file}: wanted pitch {want:?}, read {got:?}")),
                (false, true) => println!("now right, mark it pass: {file}"),
                _ => {}
            }
        }
        println!("read {read} of {} cases; the rest are not on this machine", cases.len());
        assert!(broke.is_empty(), "these read differently than before:\n  {}", broke.join("\n  "));
    }

    /// Nothing to read. A picture with nothing to divide is one tile.
    #[test]
    fn an_empty_sheet_is_one_tile() {
        let img = RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 0]));
        assert_eq!(grid(&img).0, Axis { tile: 64, gap: 0, offset: 0 });
    }
}
