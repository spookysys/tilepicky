// SPDX-License-Identifier: GPL-3.0-only
//! Local grid-edge detection. Island order follows the first cell in row order.

use image::{Rgba, RgbaImage};

/// Geometry of one island. Future labels can refer to its index in `Islands`.
#[derive(Debug, PartialEq)]
pub struct Island {
    pub cells: Vec<(u32, u32)>,
}

#[derive(Debug, PartialEq)]
pub struct Islands {
    pub islands: Vec<Island>,
    cells: Vec<Option<usize>>,
    cols: u32,
}

type PixelRect = (u32, u32, u32, u32);

impl Islands {
    pub fn at(&self, x: u32, y: u32) -> Option<&Island> {
        if x >= self.cols { return None; }
        self.cells.get((y * self.cols + x) as usize).copied().flatten().map(|id| &self.islands[id])
    }
}

/// Rectangles come from the sheet's grid geometry, including clipped cells and gaps.
pub fn detect(img: &RgbaImage, cols: u32, rows: u32, rect: impl Fn(u32, u32) -> PixelRect) -> Islands {
    let rects: Vec<_> = (0..rows).flat_map(|y| (0..cols).map(move |x| (x, y))).map(|(x, y)| {
        let (x0, y0, x1, y1) = rect(x, y);
        (x0.min(img.width()), y0.min(img.height()), x1.min(img.width()), y1.min(img.height()))
    }).collect();
    let occupied: Vec<_> = rects.iter().map(|&(x0, y0, x1, y1)| {
        (y0..y1).any(|y| (x0..x1).any(|x| img.get_pixel(x, y)[3] != 0))
    }).collect();
    let mut result = Islands { islands: Vec::new(), cells: vec![None; rects.len()], cols };
    for start in 0..rects.len() {
        if !occupied[start] || result.cells[start].is_some() { continue; }
        let id = result.islands.len();
        let mut island = Island { cells: Vec::new() };
        let mut pending = vec![start];
        result.cells[start] = Some(id);
        while let Some(i) = pending.pop() {
            let (x, y) = (i as u32 % cols, i as u32 / cols);
            island.cells.push((x, y));
            let neighbors = [
                (x.checked_sub(1).map(|x| (x, y)), true),
                ((x + 1 < cols).then_some((x + 1, y)), true),
                (y.checked_sub(1).map(|y| (x, y)), false),
                ((y + 1 < rows).then_some((x, y + 1)), false),
            ];
            for (cell, horizontal) in neighbors {
                let Some((nx, ny)) = cell else { continue; };
                let j = (ny * cols + nx) as usize;
                if occupied[j] && result.cells[j].is_none()
                    && joins(img, rects[i.min(j)], rects[i.max(j)], horizontal) {
                    result.cells[j] = Some(id);
                    pending.push(j);
                }
            }
        }
        result.islands.push(island);
    }
    result
}

fn difference(a: &Rgba<u8>, b: &Rgba<u8>) -> u64 {
    (0..3).map(|c| a[c].abs_diff(b[c]) as u64).sum()
}

fn joins(img: &RgbaImage, a: PixelRect, b: PixelRect, horizontal: bool) -> bool {
    let (a0, a1, b0, b1, start, end) = if horizontal {
        (a.0, a.2, b.0, b.2, a.1.max(b.1), a.3.min(b.3))
    } else {
        (a.1, a.3, b.1, b.3, a.0.max(b.0), a.2.min(b.2))
    };
    if a0 >= a1 || b0 >= b1 { return false; }
    let mut pairs = 0;
    let mut edge = 0;
    let mut inside = 0;
    let mut samples = 0;
    for t in start..end {
        let pixel = |n| if horizontal { img.get_pixel(n, t) } else { img.get_pixel(t, n) };
        let (left, right) = (pixel(a1 - 1), pixel(b0));
        // Any visible alpha counts. Hidden RGB must never bridge transparent space.
        if left[3] == 0 || right[3] == 0 { continue; }
        pairs += 1;
        edge += difference(left, right);
        for (outer, inner) in [(left, (a1 - a0 > 1).then(|| pixel(a1 - 2))),
                               (right, (b1 - b0 > 1).then(|| pixel(b0 + 1)))] {
            if let Some(inner) = inner.filter(|p| p[3] != 0) {
                inside += difference(outer, inner);
                samples += 1;
            }
        }
    }
    // Cut when the mean RGB jump exceeds twice the local variation plus 16
    // levels per channel. Integer sums keep the decision deterministic.
    pairs > 0 && edge * samples.max(1) <= pairs * (48 * samples.max(1) + 2 * inside)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(img: &RgbaImage, tile: [u32; 2]) -> Islands {
        detect(img, img.width() / tile[0], img.height() / tile[1], |x, y| {
            (x * tile[0], y * tile[1], (x + 1) * tile[0], (y + 1) * tile[1])
        })
    }

    #[test]
    fn transparent_sprite_crosses_cells() {
        let mut img = RgbaImage::new(12, 8);
        for y in 2..7 { for x in 1..7 { img.put_pixel(x, y, Rgba([80, 90, 100, 255])); } }
        let found = grid(&img, [4, 4]);
        assert_eq!(found.islands.len(), 1);
        assert_eq!(found.islands[0].cells.len(), 4);
        assert!(found.at(2, 0).is_none());
        assert_eq!(found, grid(&img, [4, 4]));
    }

    #[test]
    fn transparency_cuts_even_between_occupied_cells() {
        let mut img = RgbaImage::new(8, 4);
        img.put_pixel(2, 1, Rgba([255; 4]));
        img.put_pixel(4, 1, Rgba([255; 4]));
        assert_eq!(grid(&img, [4, 4]).islands.len(), 2);
        assert!(grid(&RgbaImage::new(8, 4), [4, 4]).islands.is_empty());
    }

    #[test]
    fn opaque_gradient_joins_but_color_jump_cuts() {
        let mut img = RgbaImage::new(8, 8);
        for (x, y, p) in img.enumerate_pixels_mut() { *p = Rgba([(x * 20 + y * 5) as u8, 80, 90, 255]); }
        assert_eq!(grid(&img, [4, 4]).islands.len(), 1);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 4 { Rgba([200, 20, 20, 255]) } else { Rgba([20, 20, 200, 255]) };
        }
        assert_eq!(grid(&img, [4, 4]).islands.len(), 2);
    }

    #[test]
    fn diagonal_contact_does_not_join() {
        let mut img = RgbaImage::new(8, 8);
        img.put_pixel(3, 3, Rgba([255; 4]));
        img.put_pixel(4, 4, Rgba([255; 4]));
        assert_eq!(grid(&img, [4, 4]).islands.len(), 2);
    }
}
