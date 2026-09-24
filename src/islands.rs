// SPDX-License-Identifier: GPL-3.0-only
//! Local grid-edge detection. Island order follows the first cell in row order.

use image::{Rgba, RgbaImage};

mod partition;

/// Geometry of one island. Future labels can refer to its index in `Islands`.
#[derive(Debug, PartialEq)]
pub struct Island {
    pub cells: Vec<(u32, u32)>,
}

#[derive(Debug, PartialEq)]
pub struct Islands {
    pub islands: Vec<Island>,
}

type PixelRect = (u32, u32, u32, u32);

/// Rectangles come from the sheet's grid geometry, including clipped cells and gaps.
pub fn detect(img: &RgbaImage, cols: u32, rows: u32, rect: impl Fn(u32, u32) -> PixelRect) -> Islands {
    partition::detect(img, cols, rows, rect)
}

#[cfg(test)]
fn detect_connected(img: &RgbaImage, cols: u32, rows: u32, rect: impl Fn(u32, u32) -> PixelRect) -> Islands {
    let rects: Vec<_> = (0..rows).flat_map(|y| (0..cols).map(move |x| (x, y))).map(|(x, y)| {
        let (x0, y0, x1, y1) = rect(x, y);
        (x0.min(img.width()), y0.min(img.height()), x1.min(img.width()), y1.min(img.height()))
    }).collect();
    let occupied: Vec<_> = rects.iter().map(|&(x0, y0, x1, y1)| {
        (y0..y1).any(|y| (x0..x1).any(|x| img.get_pixel(x, y)[3] != 0))
    }).collect();
    let mut result = Islands { islands: Vec::new() };
    let mut cells = vec![None; rects.len()];
    for start in 0..rects.len() {
        if !occupied[start] || cells[start].is_some() { continue; }
        let id = result.islands.len();
        let mut island = Island { cells: Vec::new() };
        let mut pending = vec![start];
        cells[start] = Some(id);
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
                if occupied[j] && cells[j].is_none()
                    && joins(img, rects[i.min(j)], rects[i.max(j)], horizontal) {
                    cells[j] = Some(id);
                    pending.push(j);
                }
            }
        }
        result.islands.push(island);
    }
    result
}

/// Read a missing grid using the same geometry as an opened sheet.
pub fn grid(img: &RgbaImage, side: &crate::sidecar::Sidecar, fallback: [u32; 2]) -> crate::Grid {
    use crate::sidecar::Pair;
    let read = side.tile.is_none().then(|| crate::detect::grid(img));
    let tile = side.tile.map(Pair::xy).unwrap_or_else(|| read.map_or(fallback, |(x, y)| [x.tile, y.tile]));
    let gap = side.gap.map(Pair::xy).unwrap_or_else(|| read.map_or([0, 0], |(x, y)| [x.gap, y.gap]));
    let found = read.map_or([0, 0], |(x, y)| [x.offset, y.offset]);
    let offset = crate::sheet::clamp_offset(side.offset.map(Pair::xy).unwrap_or(found), tile, gap);
    (tile, gap, offset)
}

pub fn pixel_rect(img: &RgbaImage, (tile, gap, offset): crate::Grid, x: u32, y: u32) -> PixelRect {
    let left = offset[0] as i64 + x as i64 * (tile[0] as i64 + gap[0] as i64);
    let top = offset[1] as i64 + y as i64 * (tile[1] as i64 + gap[1] as i64);
    (left.clamp(0, img.width() as i64) as u32, top.clamp(0, img.height() as i64) as u32,
        (left + tile[0] as i64).clamp(0, img.width() as i64) as u32,
        (top + tile[1] as i64).clamp(0, img.height() as i64) as u32)
}

pub fn region(img: &RgbaImage, grid: crate::Grid, island: &Island) -> crate::sidecar::StoredIsland {
    let rects = island.cells.iter().map(|&(x, y)| {
        let (left, top, right, bottom) = pixel_rect(img, grid, x, y);
        [left, top, right - left, bottom - top]
    }).collect();
    crate::sidecar::StoredIsland { rects, label: None }
}

pub fn regions(img: &RgbaImage, grid: crate::Grid) -> Vec<crate::sidecar::StoredIsland> {
    let (tile, gap, offset) = grid;
    let cols = crate::sheet::span(img.width(), offset[0], gap[0], tile[0] + gap[0]);
    let rows = crate::sheet::span(img.height(), offset[1], gap[1], tile[1] + gap[1]);
    detect(img, cols, rows, |x, y| pixel_rect(img, grid, x, y)).islands.iter().map(|i| region(img, grid, i)).collect()
}

fn difference(a: &Rgba<u8>, b: &Rgba<u8>) -> u64 {
    // Premultiply RGB so hidden colors do not affect the comparison.
    (0..3).map(|c| {
        let a = u64::from(a[c]) * u64::from(a[3]) / 255;
        let b = u64::from(b[c]) * u64::from(b[3]) / 255;
        a.abs_diff(b)
    }).sum::<u64>() + u64::from(a[3].abs_diff(b[3]))
}

fn joins(img: &RgbaImage, a: PixelRect, b: PixelRect, horizontal: bool) -> bool {
    let (a0, a1, b0, b1, start, end) = if horizontal {
        (a.0, a.2, b.0, b.2, a.1.max(b.1), a.3.min(b.3))
    } else {
        (a.1, a.3, b.1, b.3, a.0.max(b.0), a.2.min(b.2))
    };
    if a0 >= a1 || b0 >= b1 || start >= end { return false; }
    let pixel = |n, t| if horizontal { img.get_pixel(n, t) } else { img.get_pixel(t, n) };
    // Compare the actual tile edges, skipping the configured gap.
    if !(start..end).any(|t| pixel(a1 - 1, t)[3] != 0 && pixel(b0, t)[3] != 0) { return false; }
    // One-third-width strips include the interior behind a narrow outline.
    let width = (a1 - a0).min(b1 - b0).div_ceil(3);
    let average = |left: std::ops::Range<u32>, right: u32| {
        let count = u64::from(left.end - left.start) * u64::from(end - start);
        let mut sum = 0;
        for t in start..end { for x in left.clone() {
            sum += difference(pixel(x, t), pixel(right + x - left.start, t));
        } }
        sum / count.max(1)
    };
    let across = average(a1 - width..a1, b0);
    // Use the same pixel distance throughout both tiles. The more varied tile
    // sets the baseline, so a detailed part can continue into a plain part.
    let inside = average(a0..a1 - width, a0 + width).max(average(b0..b1 - width, b0 + width));
    // Allow two levels per RGBA channel for weak changes and integer rounding.
    // Prefer a merged group over cutting a coherent object into pieces.
    across <= 2 * inside + 8
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
    fn matching_borders_can_join_different_interiors() {
        let mut img = RgbaImage::new(32, 16);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if (14..18).contains(&x) { Rgba([40, 40, 40, 255]) }
                else if x < 16 { Rgba([160, 180, 80, 255]) } else { Rgba([160, 100, 180, 255]) };
        }
        let found = grid(&img, [8, 8]);
        assert_eq!(found.islands.len(), 1);
        assert_eq!(found.islands[0].cells.len(), 8);
        assert_eq!(found, grid(&img, [8, 8]));
        let larger = image::imageops::resize(&img, 64, 32, image::imageops::FilterType::Nearest);
        assert_eq!(found, grid(&larger, [16, 16]));
        assert_eq!(grid(&image::imageops::rotate90(&img), [8, 8]).islands.len(), 1);
    }

    #[test]
    fn hidden_colors_and_clipped_cells_do_not_change_continuity() {
        let mut img = RgbaImage::new(7, 3);
        for x in 0..7 { img.put_pixel(x, 1, Rgba([100, 120, 140, 255])); }
        let geometry = ([3, 2], [0, 0], [-1, 0]);
        let expected = regions(&img, geometry);
        assert_eq!(expected.len(), 1);
        for p in img.pixels_mut().filter(|p| p[3] == 0) { *p = Rgba([255, 30, 200, 0]); }
        assert_eq!(regions(&img, geometry), expected);
        assert_eq!(regions(&RgbaImage::from_pixel(2, 1, Rgba([80, 90, 100, 255])), ([1, 1], [0, 0], [0, 0])).len(), 1);
    }

    #[test]
    fn furniture_example_keeps_doorway_tables_and_beds_whole() {
        let path = std::path::Path::new("assets/armm1998_zelda-like/gfx/Inner.png");
        if !path.exists() { return; }
        let img = image::open(path).unwrap().into_rgba8();
        let found = grid(&img, [16, 16]);
        let owner = |x, y| found.islands.iter().position(|i| i.cells.contains(&(x, y))).unwrap();
        let table = owner(13, 1);
        let green = owner(17, 1);
        let purple = owner(20, 1);
        // Neighboring objects may merge. Splitting these objects is the worse error.
        for y in 1..4 {
            for x in 13..16 { assert_eq!(owner(x, y), table); }
            for x in 17..19 { assert_eq!(owner(x, y), green); }
            for x in 20..22 { assert_eq!(owner(x, y), purple); }
        }
        for (left, top, right, bottom) in [(8, 6, 10, 8), (10, 7, 13, 10), (6, 9, 9, 12), (14, 7, 17, 9), (14, 9, 17, 10)] {
            let island = owner(left, top);
            for y in top..bottom { for x in left..right { assert_eq!(owner(x, y), island, "split cell ({x}, {y})"); } }
        }
    }

    #[test]
    fn ambiguous_touching_outlines_and_thin_sprites_stay_whole() {
        let mut img = RgbaImage::new(16, 8);
        for y in 1..7 { for x in 0..16 { img.put_pixel(x, y, Rgba([30, 30, 30, 255])); } }
        assert_eq!(grid(&img, [8, 8]).islands.len(), 1);
        for y in 2..6 { for x in [4, 5, 10, 11] { img.put_pixel(x, y, Rgba([180, 100, 80, 255])); } }
        assert_eq!(grid(&img, [8, 8]).islands.len(), 1);
        let rotated = image::imageops::rotate90(&img);
        assert_eq!(grid(&rotated, [8, 8]).islands.len(), 1);
    }

    #[test]
    fn configured_gaps_never_change_edge_decisions() {
        let tile = [4, 3];
        let offset = [2, 1];
        let mut expected = None;
        for gap in [[0, 0], [1, 1], [4, 2]] {
            for background in [Rgba([0, 0, 0, 0]), Rgba([255, 0, 255, 255])] {
                let geometry = (tile, gap, offset);
                let mut img = RgbaImage::from_pixel(2 + 2 * tile[0] + gap[0], 1 + 2 * tile[1] + gap[1], background);
                for y in 0..2 { for x in 0..2 {
                    let (left, top, right, bottom) = pixel_rect(&img, geometry, x, y);
                    let color = if (x, y) == (1, 1) { Rgba([0, 0, 255, 255]) } else { Rgba([255, 0, 0, 255]) };
                    for py in top..bottom { for px in left..right { img.put_pixel(px, py, color); } }
                } }
                let found = detect(&img, 2, 2, |x, y| pixel_rect(&img, geometry, x, y));
                assert_eq!(found.islands.iter().map(|i| i.cells.len()).sum::<usize>(), 4);
                if let Some(expected) = &expected { assert_eq!(&found, expected); } else { expected = Some(found); }
                let regions = regions(&img, geometry);
                assert_eq!(regions.len(), expected.as_ref().unwrap().islands.len());
                assert_eq!(regions.iter().flat_map(|r| &r.rects).map(|r| r[2] * r[3]).sum::<u32>(), 4 * tile[0] * tile[1]);
            }
        }
    }

    #[test]
    fn transparent_sprite_crosses_cells() {
        let mut img = RgbaImage::new(12, 8);
        for y in 2..7 { for x in 1..7 { img.put_pixel(x, y, Rgba([80, 90, 100, 255])); } }
        let found = grid(&img, [4, 4]);
        assert_eq!(found.islands.len(), 1);
        assert_eq!(found.islands[0].cells.len(), 4);
        assert!(found.islands.iter().all(|i| !i.cells.contains(&(2, 0))));
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
