// SPDX-License-Identifier: GPL-3.0-only
//! Rectangular partitioning followed by merges along continuous shared boundaries.

use image::RgbaImage;
use super::{Island, Islands, PixelRect};

#[derive(Clone, Copy, Debug)]
struct Parameters { island: i64, empty: i64, cut: i64 }

const PARAMETERS: Parameters = Parameters { island: 64, empty: 64, cut: 128 };

pub(super) fn detect(img: &RgbaImage, cols: u32, rows: u32, rect: impl Fn(u32, u32) -> PixelRect) -> Islands {
    detect_with(img, cols, rows, rect, PARAMETERS)
}

fn detect_with(img: &RgbaImage, cols: u32, rows: u32, rect: impl Fn(u32, u32) -> PixelRect, parameters: Parameters) -> Islands {
    let rects: Vec<_> = (0..rows).flat_map(|y| (0..cols).map(move |x| (x, y))).map(|(x, y)| {
        let (x0, y0, x1, y1) = rect(x, y);
        (x0.min(img.width()), y0.min(img.height()), x1.min(img.width()), y1.min(img.height()))
    }).collect();
    let occupied: Vec<_> = rects.iter().map(|&(x0, y0, x1, y1)| {
        (y0..y1).any(|y| (x0..x1).any(|x| img.get_pixel(x, y)[3] != 0))
    }).collect();
    let mut right = vec![0; rects.len()];
    let mut down = vec![0; rects.len()];
    for y in 0..rows { for x in 0..cols {
        let i = (y * cols + x) as usize;
        for (neighbor, horizontal) in [((x + 1 < cols).then_some(i + 1), true),
            ((y + 1 < rows).then_some(i + cols as usize), false)] {
            let Some(j) = neighbor else { continue; };
            let value = if !occupied[i] || !occupied[j] { 0 }
                else { edge_score(img, rects[i], rects[j], horizontal, parameters.cut) };
            if horizontal { right[i] = value; } else { down[i] = value; }
        }
    } }
    let regions = rectangles(cols, rows, &occupied, &right, &down, parameters);
    merge_regions(cols, rows, &occupied, &right, &down, regions)
}

fn merge_regions(cols: u32, rows: u32, occupied: &[bool], right: &[i64], down: &[i64], regions: Vec<Rect>) -> Islands {
    let mut owner = vec![usize::MAX; occupied.len()];
    let mut islands: Vec<_> = regions.into_iter().enumerate().map(|(id, [x0, y0, x1, y1])| {
        for y in y0..y1 { for x in x0..x1 { owner[(y * cols + x) as usize] = id; } }
        Island { cells: (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).collect() }
    }).collect();
    loop {
        let mut boundaries = std::collections::BTreeMap::<(usize, usize), i64>::new();
        for y in 0..rows { for x in 0..cols {
            let i = (y * cols + x) as usize;
            if !occupied[i] { continue; }
            for (j, value) in [((x + 1 < cols).then_some(i + 1), right[i]),
                ((y + 1 < rows).then_some(i + cols as usize), down[i])] {
                let Some(j) = j else { continue; };
                if !occupied[j] { continue; }
                let (a, b) = (owner[i], owner[j]);
                if a != b { *boundaries.entry((a.min(b), a.max(b))).or_default() += value; }
            }
        } }
        // Require net continuity across the whole shared boundary. A single
        // touching edge must not bypass stronger evidence of separation.
        let mut best = None;
        let mut gain = 0;
        for (pair, score) in boundaries {
            if score > gain { best = Some(pair); gain = score; }
        }
        let Some((a, b)) = best else { break; };
        let cells = std::mem::take(&mut islands[b].cells);
        for &(x, y) in &cells { owner[(y * cols + x) as usize] = a; }
        islands[a].cells.extend(cells);
    }
    islands.retain(|i| !i.cells.is_empty());
    for island in &mut islands { island.cells.sort_by_key(|&(x, y)| (y, x)); }
    islands.sort_by_key(|i| { let (x, y) = i.cells[0]; (y, x) });
    Islands { islands }
}

fn edge_score(img: &RgbaImage, a: PixelRect, b: PixelRect, horizontal: bool, cut: i64) -> i64 {
    if !super::joins(img, a, b, horizontal) { return -cut; }
    let (edge_a, edge_b, start, end) = if horizontal {
        (a.2 - 1, b.0, a.1.max(b.1), a.3.min(b.3))
    } else { (a.3 - 1, b.1, a.0.max(b.0), a.2.min(b.2)) };
    let pixel = |n, t| if horizontal { img.get_pixel(n, t) } else { img.get_pixel(t, n) };
    let pairs = (start..end).filter(|&t| pixel(edge_a, t)[3] != 0 && pixel(edge_b, t)[3] != 0).count() as i64;
    // Sparse contact gives weaker support, never evidence of separation.
    (64 * pairs / i64::from(end - start)).max(1)
}

type Rect = [u32; 4];

struct Sums {
    stride: usize,
    values: Vec<i64>,
}

impl Sums {
    fn new(cols: u32, rows: u32, values: &[i64]) -> Self {
        let stride = cols as usize + 1;
        let mut sums = vec![0; stride * (rows as usize + 1)];
        for y in 0..rows as usize { for x in 0..cols as usize {
            let i = (y + 1) * stride + x + 1;
            sums[i] = values[y * cols as usize + x] + sums[i - 1] + sums[i - stride] - sums[i - stride - 1];
        } }
        Self { stride, values: sums }
    }

    fn get(&self, [x0, y0, x1, y1]: Rect) -> i64 {
        let at = |x, y| self.values[y as usize * self.stride + x as usize];
        at(x1, y1) - at(x0, y1) - at(x1, y0) + at(x0, y0)
    }
}

struct Score {
    parameters: Parameters,
    occupied: Sums,
    right: Sums,
    down: Sums,
}

impl Score {
    fn cost(&self, r @ [x0, y0, x1, y1]: Rect) -> i64 {
        // Each region and its unused cells have a cost; internal continuity lowers it.
        let occupied = self.occupied.get(r);
        if occupied == 0 { return 0; }
        let empty = i64::from(x1 - x0) * i64::from(y1 - y0) - occupied;
        self.parameters.island + self.parameters.empty * empty - self.right.get([x0, y0, x1 - 1, y1]) - self.down.get([x0, y0, x1, y1 - 1])
    }
}

fn split(r: Rect, axis: usize, at: u32) -> [Rect; 2] {
    let (mut a, mut b) = (r, r);
    a[axis + 2] = at;
    b[axis] = at;
    [a, b]
}

/// Join the shared span and retain any protruding parts as separate rectangles.
/// This can undo a cut even when the neighboring rectangles have different sizes.
fn joined(a: Rect, b: Rect) -> Option<Vec<Rect>> {
    for axis in 0..2 {
        let (a, b) = if a[axis + 2] == b[axis] { (a, b) }
            else if b[axis + 2] == a[axis] { (b, a) } else { continue; };
        let other = 1 - axis;
        let lo = a[other].max(b[other]);
        let hi = a[other + 2].min(b[other + 2]);
        if lo >= hi { continue; }
        let mut merged = a;
        merged[axis + 2] = b[axis + 2];
        merged[other] = lo;
        merged[other + 2] = hi;
        let mut result = vec![merged];
        for r in [a, b] {
            if r[other] < lo { result.push(split(r, other, lo)[0]); }
            if hi < r[other + 2] { result.push(split(r, other, hi)[1]); }
        }
        return Some(result);
    }
    None
}

/// A rectangular object may need empty corners, or parts of several neighbors.
fn enclose(regions: &[Rect], owner: &[usize], cols: u32, bounds: Rect) -> (Vec<usize>, Vec<Rect>) {
    let mut remove = std::collections::BTreeSet::new();
    for y in bounds[1]..bounds[3] { for x in bounds[0]..bounds[2] { remove.insert(owner[(y * cols + x) as usize]); } }
    let mut add = vec![bounds];
    for &i in &remove {
        let r = regions[i];
        let y0 = r[1].max(bounds[1]);
        let y1 = r[3].min(bounds[3]);
        if r[1] < y0 { add.push([r[0], r[1], r[2], y0]); }
        if y1 < r[3] { add.push([r[0], y1, r[2], r[3]]); }
        if r[0] < bounds[0] { add.push([r[0], y0, bounds[0], y1]); }
        if bounds[2] < r[2] { add.push([bounds[2], y0, r[2], y1]); }
    }
    (remove.into_iter().collect(), add)
}

struct Change {
    gain: i64,
    remove: Vec<usize>,
    add: Vec<Rect>,
}

impl Change {
    fn consider(&mut self, score: &Score, regions: &[Rect], remove: Vec<usize>, add: Vec<Rect>) {
        let gain = remove.iter().map(|&i| score.cost(regions[i])).sum::<i64>() - add.iter().map(|&r| score.cost(r)).sum::<i64>();
        if gain > self.gain { *self = Self { gain, remove, add }; }
    }
}

fn improve(cols: u32, rows: u32, score: &Score, mut regions: Vec<Rect>) -> Vec<Rect> {
    loop {
        regions.sort_by_key(|r| (r[1], r[0], r[3], r[2]));
        let mut change = Change { gain: 0, remove: vec![], add: vec![] };
        let mut owner = vec![0; (cols * rows) as usize];
        for (i, &r) in regions.iter().enumerate() {
            for y in r[1]..r[3] { for x in r[0]..r[2] { owner[(y * cols + x) as usize] = i; } }
            for axis in 0..2 { for at in r[axis] + 1..r[axis + 2] {
                change.consider(score, &regions, vec![i], split(r, axis, at).to_vec());
            } }
        }
        let mut neighbors = std::collections::BTreeSet::new();
        for y in 0..rows { for x in 0..cols {
            let i = (y * cols + x) as usize;
            for j in [(x + 1 < cols).then_some(i + 1), (y + 1 < rows).then_some(i + cols as usize)].into_iter().flatten() {
                let (a, b) = (owner[i], owner[j]);
                if a != b { neighbors.insert((a.min(b), a.max(b))); }
            }
        } }
        for (a, b) in neighbors {
            if let Some(add) = joined(regions[a], regions[b]) {
                let partial = add.len() > 1;
                change.consider(score, &regions, vec![a, b], add);
                if partial {
                    let a = regions[a];
                    let b = regions[b];
                    let bounds = [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
                    let (remove, add) = enclose(&regions, &owner, cols, bounds);
                    change.consider(score, &regions, remove, add);
                }
            }
        }
        if change.gain == 0 { return regions; }
        for i in change.remove.into_iter().rev() { regions.remove(i); }
        regions.extend(change.add);
    }
}

fn rectangles(cols: u32, rows: u32, occupied: &[bool], right: &[i64], down: &[i64], parameters: Parameters) -> Vec<Rect> {
    if cols == 0 || rows == 0 { return vec![]; }
    let score = Score { parameters, occupied: Sums::new(cols, rows, &occupied.iter().map(|&o| i64::from(o)).collect::<Vec<_>>()),
        right: Sums::new(cols, rows, right), down: Sums::new(cols, rows, down) };
    // Start at changes in cell occupancy. Empty space must not need a temporary
    // extra island before it can separate two disconnected objects.
    let mut initial: Vec<Rect> = vec![];
    for y in 0..rows {
        let mut x = 0;
        while x < cols {
            let start = x;
            let filled = occupied[(y * cols + x) as usize];
            while x < cols && occupied[(y * cols + x) as usize] == filled { x += 1; }
            if let Some(r) = initial.iter_mut().rfind(|r| r[0] == start && r[2] == x && r[3] == y
                && occupied[(r[1] * cols + r[0]) as usize] == filled) { r[3] += 1; }
            else { initial.push([start, y, x, y + 1]); }
        }
    }
    improve(cols, rows, &score, initial).into_iter().filter(|&r| score.occupied.get(r) > 0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whole(found: &Islands, [x0, y0, x1, y1]: Rect, img: &RgbaImage) -> bool {
        let cells: Vec<_> = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| {
            (y*16..(y+1)*16).any(|py| (x*16..(x+1)*16).any(|px| img.get_pixel(px, py)[3] != 0))
        }).collect();
        found.islands.iter().any(|i| cells.iter().all(|cell| i.cells.contains(cell)))
    }

    fn annotations(name: &str, validation: bool) -> Vec<Rect> {
        match (name, validation) {
            ("Overworld", false) => vec![[6, 0, 11, 5], [22, 9, 25, 12]],
            ("Overworld", true) => vec![[11, 0, 16, 5], [25, 9, 28, 12], [28, 9, 31, 12], [23, 12, 30, 17]],
            ("Inner", false) => vec![[13, 1, 16, 4], [16, 1, 17, 4], [17, 1, 19, 4], [8, 6, 10, 8], [10, 7, 13, 9]],
            _ => vec![[10, 1, 13, 4], [19, 1, 20, 4], [20, 1, 22, 4], [6, 9, 9, 12]],
        }
    }

    fn loss(img: &RgbaImage, found: &Islands, objects: &[Rect]) -> usize {
        let visible = |x, y| (y*16..(y+1)*16).any(|py| (x*16..(x+1)*16).any(|px| img.get_pixel(px, py)[3] != 0));
        let contents: Vec<std::collections::BTreeSet<_>> = found.islands.iter().map(|i| {
            i.cells.iter().copied().filter(|&(x, y)| visible(x, y)).collect()
        }).collect();
        objects.iter().map(|&[x0, y0, x1, y1]| {
            let cells: std::collections::BTreeSet<_> = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .filter(|&(x, y)| visible(x, y)).collect();
            assert!(!cells.is_empty());
            let groups: Vec<_> = contents.iter().filter(|i| !i.is_disjoint(&cells)).collect();
            let extra: usize = groups.iter().map(|i| i.difference(&cells).count()).sum();
            // One extra fragment costs four times an extra object's worth of pixels.
            400 * groups.len().saturating_sub(1) + 100 * extra / cells.len()
        }).sum()
    }

    #[test]
    #[ignore = "parameter search uses local example sheets; run in release mode"]
    fn tune_parameters() {
        let sheets: Vec<_> = ["Overworld", "Inner"].into_iter().map(|name| {
            let path = std::path::Path::new("assets/armm1998_zelda-like/gfx").join(format!("{name}.png"));
            (name, image::open(path).expect("The example sheets are required for tuning").into_rgba8())
        }).collect();
        let evaluate = |parameters| {
            let mut losses = [0; 2];
            for (name, img) in &sheets {
                let found = detect_with(img, img.width()/16, img.height()/16,
                    |x, y| (x*16, y*16, x*16+16, y*16+16), parameters);
                for (i, validation) in [false, true].into_iter().enumerate() { losses[i] += loss(img, &found, &annotations(name, validation)); }
            }
            losses
        };
        let mut best = None;
        let mut count = 0;
        for island in [8, 16, 32, 64] { for empty in [8, 16, 32, 64] { for cut in [64, 128, 256] {
            if empty < island || cut <= island { continue; }
            let parameters = Parameters { island, empty, cut };
            let losses = evaluate(parameters);
            count += 1;
            println!("trial {count}: {parameters:?}, fit={}, validation={}", losses[0], losses[1]);
            // Validation is reported but never used to select parameters.
            if best.as_ref().is_none_or(|(_, score)| losses[0] < *score) { best = Some((parameters, losses[0])); }
        } } }
        let (parameters, fit) = best.unwrap();
        println!("BEST {parameters:?}, fit={fit}, validation={}", evaluate(parameters)[1]);
        for (name, img) in &sheets {
            let current = super::super::detect_connected(img, img.width()/16, img.height()/16, |x, y| (x*16, y*16, x*16+16, y*16+16));
            println!("BASELINE {name}: fit={}, validation={}", loss(img, &current, &annotations(name, false)), loss(img, &current, &annotations(name, true)));
        }
    }

    #[test]
    fn tilemap_examples() {
        let mut scores = [[0; 2]; 2];
        let mut loaded = 0;
        for name in ["Overworld", "Inner"] {
            let path = std::path::Path::new("assets/armm1998_zelda-like/gfx").join(format!("{name}.png"));
            if !path.exists() { continue; }
            let img = image::open(&path).unwrap().into_rgba8();
            let (cols, rows) = (img.width() / 16, img.height() / 16);
            let rect = |x, y| (x * 16, y * 16, (x + 1) * 16, (y + 1) * 16);
            let current = super::super::detect_connected(&img, cols, rows, rect);
            let started = std::time::Instant::now();
            let proposed = detect(&img, cols, rows, rect);
            loaded += 1;
            for (i, found) in [&current, &proposed].into_iter().enumerate() {
                for (j, validation) in [false, true].into_iter().enumerate() { scores[i][j] += loss(&img, found, &annotations(name, validation)); }
            }
            println!("{name}: previous {} regions; merged {} regions ({:?})", current.islands.len(), proposed.islands.len(), started.elapsed());
            let examples: &[(&str, Rect)] = if name == "Overworld" {
                &[("left house", [6, 0, 11, 5]), ("right house", [11, 0, 16, 5]), ("large roof", [23, 12, 30, 17]),
                    ("left fountain", [22, 9, 25, 12]), ("middle fountain", [25, 9, 28, 12]), ("right fountain", [28, 9, 31, 12])]
            } else { &[("doorway", [8, 6, 10, 8]), ("table", [10, 7, 13, 10]), ("lower table", [6, 9, 9, 12])] };
            for &(label, bounds) in examples {
                println!("  {label}: current whole={}, rectangular whole={}", whole(&current, bounds, &img), whole(&proposed, bounds, &img));
            }
            if name == "Inner" { for &(_, bounds) in examples { assert!(whole(&proposed, bounds, &img)); } }
            else {
                for &(_, bounds) in &examples[3..] { assert!(whole(&proposed, bounds, &img)); }
                for &(_, bounds) in &examples[..2] { assert!(whole(&proposed, bounds, &img)); }
            }
            if let Some(dir) = std::env::var_os("TILEPICKY_RECTANGLE_PREVIEW") {
                let dir = std::path::PathBuf::from(dir);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(format!("{name}.svg")), preview(&path, &img, &current, &proposed)).unwrap();
            }
        }
        if loaded == 2 {
            assert!(scores[1][0] < scores[0][0], "tuning error: {scores:?}");
            assert!(scores[1][1] < scores[0][1], "validation error: {scores:?}");
        }
    }

    fn preview(path: &std::path::Path, img: &RgbaImage, current: &Islands, proposed: &Islands) -> String {
        use base64::Engine;
        use std::fmt::Write;
        let data = base64::engine::general_purpose::STANDARD.encode(std::fs::read(path).unwrap());
        let (w, h) = (img.width(), img.height());
        let mut svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 {} {}">"#, 2*w+16, h+24);
        write!(svg, r##"<rect width="100%" height="100%" fill="#dedede"/>"##).unwrap();
        for (column, title, found) in [(0, "Previous detector", current), (1, "After final merges", proposed)] {
            write!(svg, r#"<g transform="translate({},24)"><text x="0" y="-7" font-family="DejaVu Sans" font-size="12">{}: {} regions</text>"#,
                column * (w + 16), title, found.islands.len()).unwrap();
            write!(svg, r#"<image width="{w}" height="{h}" xlink:href="data:image/png;base64,{data}"/>"#).unwrap();
            for island in &found.islands {
                let cells: std::collections::BTreeSet<_> = island.cells.iter().copied().collect();
                let mut lines = String::new();
                for &(x, y) in &cells {
                    let (px, py) = (x * 16, y * 16);
                    if !x.checked_sub(1).is_some_and(|x| cells.contains(&(x, y))) { write!(lines, "M{px},{py}v16 ").unwrap(); }
                    if !cells.contains(&(x + 1, y)) { write!(lines, "M{},{}v16 ", px + 16, py).unwrap(); }
                    if !y.checked_sub(1).is_some_and(|y| cells.contains(&(x, y))) { write!(lines, "M{px},{py}h16 ").unwrap(); }
                    if !cells.contains(&(x, y + 1)) { write!(lines, "M{},{}h16 ", px, py + 16).unwrap(); }
                }
                write!(svg, r##"<path d="{lines}" fill="none" stroke="#ff1671" stroke-width="0.6"/>"##).unwrap();
            }
            svg.push_str("</g>");
        }
        svg.push_str("</svg>");
        svg
    }

    #[test]
    fn hollow_outline_contact_is_positive_and_empty_separation_is_negative() {
        use image::Rgba;
        let mut img = RgbaImage::new(32, 32);
        for y in 2..30 { for x in 2..30 {
            if x == 2 || x == 29 || y == 2 || y == 29 { img.put_pixel(x, y, Rgba([120, 120, 120, 255])); }
        } }
        let rect = |x, y| (x*16, y*16, x*16+16, y*16+16);
        assert!(edge_score(&img, rect(0, 0), rect(1, 0), true, 128) > 0);
        assert_eq!(detect(&img, 2, 2, rect).islands.len(), 1);
        for y in 0..32 { img.put_pixel(15, y, Rgba([0; 4])); }
        assert_eq!(edge_score(&img, rect(0, 0), rect(1, 0), true, 128), -128);
    }

    #[test]
    fn final_merge_forms_an_l_without_filling_its_corner() {
        let regions = vec![[0, 0, 2, 1], [0, 1, 1, 2]];
        let detect = || merge_regions(2, 2, &[true, true, true, false], &[64, 0, 0, 0], &[64, 0, 0, 0], regions.clone());
        let found = detect();
        assert_eq!(found, detect());
        assert_eq!(found.islands.len(), 1);
        assert_eq!(found.islands[0].cells, vec![(0, 0), (1, 0), (0, 1)]);
    }

    #[test]
    fn final_merge_respects_the_complete_boundary() {
        let regions = vec![[0, 0, 1, 2], [1, 0, 2, 2]];
        for lower in [-128, -64] {
            let found = merge_regions(2, 2, &[true; 4], &[64, 0, lower, 0], &[64, 64, 0, 0], regions.clone());
            assert_eq!(found.islands.len(), 2);
        }
        let found = merge_regions(2, 2, &[true; 4], &[64, 0, 64, 0], &[64, 64, 0, 0], regions);
        assert_eq!(found.islands.len(), 1);
    }

    #[test]
    fn final_merge_keeps_growing_a_region() {
        let regions = vec![[0, 0, 1, 1], [1, 0, 2, 1], [2, 0, 3, 1]];
        let found = merge_regions(3, 1, &[true; 3], &[64, 32, 0], &[0; 3], regions);
        assert_eq!(found.islands.len(), 1);
        assert_eq!(found.islands[0].cells, vec![(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn final_merge_rechecks_boundaries_after_each_join() {
        let regions = vec![[0, 0, 2, 1], [0, 1, 1, 2], [1, 1, 2, 2]];
        let found = merge_regions(2, 2, &[true; 4], &[64, 0, -128, 0], &[64, 32, 0, 0], regions);
        assert_eq!(found.islands.len(), 2);
        assert_eq!(found.islands[0].cells, vec![(0, 0), (1, 0), (0, 1)]);
        assert_eq!(found.islands[1].cells, vec![(1, 1)]);
    }

    #[test]
    fn image_geometry_and_diagonal_separation() {
        use image::Rgba;
        let mut diagonal = RgbaImage::new(8, 8);
        diagonal.put_pixel(3, 3, Rgba([255; 4]));
        diagonal.put_pixel(4, 4, Rgba([255; 4]));
        assert_eq!(detect(&diagonal, 2, 2, |x, y| (x*4, y*4, x*4+4, y*4+4)).islands.len(), 2);
        let mut expected = None;
        for gap in [[0, 0], [1, 2], [3, 4]] {
            let geometry = ([4, 3], gap, [2, 1]);
            let mut img = RgbaImage::from_pixel(10 + gap[0], 7 + gap[1], Rgba([255, 0, 255, 255]));
            for y in 0..2 { for x in 0..2 {
                let (x0, y0, x1, y1) = super::super::pixel_rect(&img, geometry, x, y);
                let color = if (x, y) == (1, 1) { Rgba([0, 0, 255, 255]) } else { Rgba([255, 0, 0, 255]) };
                for py in y0..y1 { for px in x0..x1 { img.put_pixel(px, py, color); } }
            } }
            let found = detect(&img, 2, 2, |x, y| super::super::pixel_rect(&img, geometry, x, y));
            assert_eq!(found.islands.iter().map(|i| i.cells.len()).sum::<usize>(), 4);
            if let Some(expected) = &expected { assert_eq!(&found, expected); } else { expected = Some(found); }
        }
    }

    #[test]
    fn a_join_can_split_a_neighbor_and_cross_an_earlier_cut() {
        let occupied = Sums::new(3, 2, &[1; 6]);
        let right = Sums::new(3, 2, &[64, -128, 0, 64, -128, 0]);
        let down = Sums::new(3, 2, &[64, 64, 64, 0, 0, 0]);
        let score = Score { parameters: PARAMETERS, occupied, right, down };
        let initial = vec![[0, 0, 3, 1], [0, 1, 2, 2], [2, 1, 3, 2]];
        let joined = joined(initial[0], initial[1]).unwrap();
        assert_eq!(joined, vec![[0, 0, 2, 2], [2, 0, 3, 1]]);
        assert!(joined.iter().map(|&r| score.cost(r)).sum::<i64>() < score.cost(initial[0]) + score.cost(initial[1]));
        assert_eq!(improve(3, 2, &score, initial), vec![[0, 0, 2, 2], [2, 0, 3, 2]]);
    }

    #[test]
    fn rectangles_cover_occupied_cells_once_and_are_deterministic() {
        for mask in 0..64 {
            let occupied: Vec<_> = (0..6).map(|i| mask & (1 << i) != 0).collect();
            let right: Vec<_> = (0..6).map(|i| if i % 3 == 2 { 0 } else if occupied[i] == occupied[i + 1] { 64 } else { -128 }).collect();
            let down: Vec<_> = (0..6).map(|i| if i >= 3 { 0 } else if occupied[i] == occupied[i + 3] { 64 } else { -128 }).collect();
            let found = rectangles(3, 2, &occupied, &right, &down, PARAMETERS);
            assert_eq!(found, rectangles(3, 2, &occupied, &right, &down, PARAMETERS));
            let mut covered = [0; 6];
            for [x0, y0, x1, y1] in found {
                assert!(x0 < x1 && x1 <= 3 && y0 < y1 && y1 <= 2);
                let mut content = false;
                for y in y0..y1 { for x in x0..x1 {
                    let i = (y * 3 + x) as usize;
                    covered[i] += 1;
                    content |= occupied[i];
                } }
                assert!(content);
            }
            for i in 0..6 { assert!(covered[i] <= 1); if occupied[i] { assert_eq!(covered[i], 1); } }
        }
        assert!(rectangles(0, 0, &[], &[], &[], PARAMETERS).is_empty());
    }
}
