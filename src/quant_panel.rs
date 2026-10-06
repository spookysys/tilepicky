// SPDX-License-Identifier: GPL-3.0-only
//! Color quantization preview: the side panel of the project half, the run
//! that turns a group's setting into pixels, and the export.

use crate::quant::{self, Dither, Job, Oklab, Rgb};
use crate::sidecar::{
    General, GeneralFormat, GeneralKind, Quantize, QuantizeDither, QuantizeGroup, QuantizeMode, QuantizeTarget, Snes, SnesDepth,
};
use crate::App;
use eframe::egui;
use image::RgbaImage;

/// The paint tool of the panel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Paint,
    Erase,
}

/// What an export writes.
pub enum Export {
    /// An indexed PNG: a palette, one index per pixel, and index 0 transparent.
    Indexed { palette: Vec<Rgb>, indices: Vec<u8>, key: bool },
    /// The preview image itself, for the formats without a palette.
    Image,
}

/// The result of one run, ready for the preview and the export.
pub struct Outcome {
    pub preview: RgbaImage,
    pub export: Export,
    pub deep_tiles: usize,
}

/// A pending conflict: the sheets that already carry a setting.
pub struct Conflict {
    pub covered: Vec<String>,
}

/// The transparency sentinel. A pixel with alpha below half becomes this
/// color, which the quantizer reserves as index 0.
const KEY: Rgb = Rgb::new(255, 0, 255);
/// Alpha at or above this is opaque.
const OPAQUE: u8 = 128;

fn dither(which: QuantizeDither) -> Option<Dither> {
    Some(match which {
        QuantizeDither::None => return None,
        QuantizeDither::Bayer2 => Dither::Bayer2x2,
        QuantizeDither::Checker => Dither::Checker,
        QuantizeDither::StippleV => Dither::StippleV,
        QuantizeDither::StippleH => Dither::StippleH,
        QuantizeDither::LineV => Dither::LineV,
        QuantizeDither::LineH => Dither::LineH,
        // The general path handles Floyd-Steinberg on its own.
        QuantizeDither::FloydSteinberg => return None,
    })
}

/// Runs one sheet through the SNES quantizer. `pins` is one flag per tile;
/// it is empty when the depth is not `4bpp+8bpp`.
pub fn run_snes(setting: &Snes, img: &RgbaImage, tile: [u32; 2], pins: &[bool], seed: u64) -> Outcome {
    let (w, h) = img.dimensions();
    let (tw, th) = (tile[0].max(1), tile[1].max(1));
    let bpp = match setting.depth {
        SnesDepth::Bpp2 => 2,
        SnesDepth::Bpp4 | SnesDepth::Bpp4Plus8 => 4,
        SnesDepth::Bpp8 => 8,
    };
    let stride = 1u32 << bpp;
    let palettes = setting.palettes.clamp(1, (256 / stride) as usize);
    let colors = setting.colors.clamp(1, stride as usize - 1);

    let pixels: Vec<Rgb> = img
        .pixels()
        .map(|p| if p.0[3] < OPAQUE { KEY } else { Rgb::new(p.0[0], p.0[1], p.0[2]) })
        .collect();

    let mut job = Job::new(bpp, palettes, colors);
    job.tile_width = tw;
    job.tile_height = th;
    job.dither = dither(setting.dither);
    job.color_zero = Some(KEY);
    job.seed = seed;

    if matches!(setting.depth, SnesDepth::Bpp4Plus8) {
        let count = (w.div_ceil(tw) * h.div_ceil(th)) as usize;
        let pinned: Vec<bool> = (0..count).map(|i| pins.get(i).copied().unwrap_or(false)).collect();
        let start = palettes as u32 * stride;
        let extra = setting.extra_8bpp.min((256u32.saturating_sub(start)) as usize);
        job.deep_entries = (0..extra as u32).map(|i| (start + i) as u8).collect();
        let want = (setting.auto_pct as usize * count + 50) / 100;
        let have = pinned.iter().filter(|&&d| d).count();
        job.deep_tiles = if want > have { quant::choose_deep_tiles(&pixels, w, h, &job, want - have) } else { pinned };
    }

    let result = quant::quantize(&pixels, w, h, &job);
    let table = quant::cgram(&result, &job);
    let indices = quant::index_image(&result, &job, w, h);
    let deep_tiles = result.tile_deep.iter().filter(|&&d| d).count();
    let preview = paint_preview(img, &table, &indices);
    Outcome { preview, export: Export::Indexed { palette: table, indices, key: true }, deep_tiles }
}

/// Runs one sheet through the general path.
pub fn run_general(setting: &General, img: &RgbaImage) -> Outcome {
    match setting.kind {
        GeneralKind::Downsample => run_downsample(setting, img),
        GeneralKind::Indexed => run_indexed(setting, img),
    }
}

/// Reduces each pixel to the chosen channel format, with no palette.
fn run_downsample(setting: &General, img: &RgbaImage) -> Outcome {
    let (w, h) = img.dimensions();
    let raw = img.as_raw();
    let n = (w * h) as usize;
    let ordered = pattern(setting.dither);
    let fs = is_floyd(setting.dither);
    let mut buf: Vec<[f32; 3]> =
        (0..n).map(|i| [raw[i * 4] as f32, raw[i * 4 + 1] as f32, raw[i * 4 + 2] as f32]).collect();
    let mut preview = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let (alpha, keyed) = alpha(setting, raw[i * 4 + 3]);
            if keyed {
                preview.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
                continue;
            }
            let src = if fs { buf[i] } else { [raw[i * 4] as f32, raw[i * 4 + 1] as f32, raw[i * 4 + 2] as f32] };
            let rank = ordered.map(|(m, c)| (m[(x & 1) as usize][(y & 1) as usize], c));
            let [r, g, b] = reduce_float(setting.format, src, rank);
            preview.put_pixel(x, y, image::Rgba([r, g, b, alpha]));
            if fs {
                diffuse(&mut buf, w, h, x, y, src, [r as f32, g as f32, b as f32]);
            }
        }
    }
    Outcome { preview, export: Export::Image, deep_tiles: 0 }
}

/// Quantizes to a palette, then writes each pixel as an index. `train` is
/// every opaque pixel of the group; `img` is the sheet the preview shows.
fn run_indexed_with(setting: &General, train: &[quantette::deps::palette::Srgb<u8>], img: &RgbaImage) -> Outcome {
    let (w, h) = img.dimensions();
    let raw = img.as_raw();
    let colors = setting.colors.clamp(2, 256);
    let want = colors - usize::from(setting.key);
    let palette = if train.is_empty() {
        vec![Rgb::new(0, 0, 0)]
    } else {
        match quantette::Pipeline::new()
            .palette_size(quantette::PaletteSize::try_from(want).expect("2..=256"))
            .quantize_method(quantette::QuantizeMethod::kmeans())
            .input_slice(train)
        {
            Ok(p) => p.output_srgb8_palette().iter().map(|c| Rgb::new(c.red, c.green, c.blue)).collect(),
            Err(_) => vec![Rgb::new(0, 0, 0)],
        }
    };
    let mut table = palette;
    if setting.key {
        table.insert(0, Rgb::new(0, 0, 0));
    }
    let formatted: Vec<Rgb> = table
        .iter()
        .map(|c| {
            let [r, g, b] = reduce(setting.format, [c.r, c.g, c.b]);
            Rgb::new(r, g, b)
        })
        .collect();
    let oklab: Vec<Oklab> = formatted.iter().map(|&c| quant::srgb_to_oklab(c)).collect();
    let start = usize::from(setting.key);
    let ordered = pattern(setting.dither);
    let fs = is_floyd(setting.dither);
    let mut buf: Vec<[f32; 3]> =
        (0..(w * h) as usize).map(|i| [raw[i * 4] as f32, raw[i * 4 + 1] as f32, raw[i * 4 + 2] as f32]).collect();
    let mut indices = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let a = raw[i * 4 + 3];
            if setting.key && a < OPAQUE {
                continue;
            }
            let src = if fs { buf[i] } else { [raw[i * 4] as f32, raw[i * 4 + 1] as f32, raw[i * 4 + 2] as f32] };
            let source = Rgb::new(
                src[0].clamp(0.0, 255.0) as u8,
                src[1].clamp(0.0, 255.0) as u8,
                src[2].clamp(0.0, 255.0) as u8,
            );
            let pick = match ordered {
                Some((m, c)) => {
                    let (n0, n1, d0, d1) = nearest_two(&oklab, start, source);
                    let t = if d0 + d1 > 0.0 { d0 / (d0 + d1) } else { 0.0 };
                    let threshold = (m[(x & 1) as usize][(y & 1) as usize] as f32 + 0.5) / c as f32;
                    if t <= threshold { n0 } else { n1 }
                }
                None => nearest(&oklab, start, source),
            };
            indices[i] = pick as u8;
            if fs {
                let pal = formatted[pick];
                diffuse(&mut buf, w, h, x, y, src, [pal.r as f32, pal.g as f32, pal.b as f32]);
            }
        }
    }
    let mut preview = RgbaImage::new(w, h);
    for (i, p) in preview.pixels_mut().enumerate() {
        let a = raw[i * 4 + 3];
        if setting.key && a < OPAQUE {
            *p = image::Rgba([0, 0, 0, 0]);
        } else {
            let c = formatted[indices[i] as usize];
            let alpha = if setting.format.has_alpha() { a } else { 255 };
            *p = image::Rgba([c.r, c.g, c.b, alpha]);
        }
    }
    let export = if setting.format.has_alpha() {
        Export::Image
    } else {
        Export::Indexed { palette: formatted, indices, key: setting.key }
    };
    Outcome { preview, export, deep_tiles: 0 }
}

/// The training pixels of one sheet: its opaque colors.
fn train_pixels(setting: &General, img: &RgbaImage) -> Vec<quantette::deps::palette::Srgb<u8>> {
    let raw = img.as_raw();
    let n = (img.width() * img.height()) as usize;
    (0..n)
        .filter(|&i| !setting.key || raw[i * 4 + 3] >= OPAQUE)
        .map(|i| quantette::deps::palette::Srgb::new(raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2]))
        .collect()
}

/// Quantizes to a palette, then writes each pixel as an index.
fn run_indexed(setting: &General, img: &RgbaImage) -> Outcome {
    let train = train_pixels(setting, img);
    run_indexed_with(setting, &train, img)
}

/// The nearest palette entry at or after `start`, in Oklab.
fn nearest(palette: &[Oklab], start: usize, color: Rgb) -> usize {
    let c = quant::srgb_to_oklab(color);
    (start..palette.len())
        .min_by(|&a, &b| quant::oklab_sqdist(c, palette[a]).total_cmp(&quant::oklab_sqdist(c, palette[b])))
        .unwrap_or(0)
}

/// Reduces a color to the channel format.
fn reduce(format: GeneralFormat, [r, g, b]: [u8; 3]) -> [u8; 3] {
    let bits = |v: u8, n: u32| -> u8 { let step = 8 - n; ((v as u32 >> step) << step) as u8 };
    match format {
        GeneralFormat::Rgb565 => [bits(r, 5), bits(g, 6), bits(b, 5)],
        GeneralFormat::Rgba5658 => [bits(r, 5), bits(g, 6), bits(b, 5)],
        GeneralFormat::Rgb888 | GeneralFormat::Rgba8888 => [r, g, b],
        GeneralFormat::Rgb233 => [bits(r, 2), bits(g, 3), bits(b, 3)],
    }
}

/// The ordered dither matrices, with the number of candidates per pixel.
fn pattern(which: QuantizeDither) -> Option<([[usize; 2]; 2], usize)> {
    Some(match which {
        QuantizeDither::None | QuantizeDither::FloydSteinberg => return None,
        QuantizeDither::Bayer2 => ([[0, 2], [3, 1]], 4),
        QuantizeDither::Checker => ([[0, 1], [1, 0]], 2),
        QuantizeDither::StippleV => ([[0, 1], [3, 2]], 4),
        QuantizeDither::StippleH => ([[0, 3], [1, 2]], 4),
        QuantizeDither::LineV => ([[0, 0], [1, 1]], 2),
        QuantizeDither::LineH => ([[0, 1], [0, 1]], 2),
    })
}

fn is_floyd(which: QuantizeDither) -> bool {
    matches!(which, QuantizeDither::FloydSteinberg)
}

/// The bits per channel of a format.
fn format_bits(format: GeneralFormat) -> (u32, u32, u32) {
    match format {
        GeneralFormat::Rgb565 | GeneralFormat::Rgba5658 => (5, 6, 5),
        GeneralFormat::Rgb233 => (2, 3, 3),
        GeneralFormat::Rgb888 | GeneralFormat::Rgba8888 => (8, 8, 8),
    }
}

/// Reduces one channel, with the ordered bias added first.
fn reduce_channel(v: f32, bits: u32, rank: Option<(usize, usize)>) -> u8 {
    let v = match rank {
        Some((r, c)) => v + ((r as f32 + 0.5) / c as f32 - 0.5) * (256u32 >> bits) as f32,
        None => v,
    };
    let v = v.clamp(0.0, 255.0) as u32;
    ((v >> (8 - bits)) << (8 - bits)) as u8
}

/// Reduces a float color to the format.
fn reduce_float(format: GeneralFormat, [r, g, b]: [f32; 3], rank: Option<(usize, usize)>) -> [u8; 3] {
    let (br, bg, bb) = format_bits(format);
    [reduce_channel(r, br, rank), reduce_channel(g, bg, rank), reduce_channel(b, bb, rank)]
}

/// The two nearest palette entries at or after `start`, with their distances.
fn nearest_two(palette: &[Oklab], start: usize, color: Rgb) -> (usize, usize, f32, f32) {
    let c = quant::srgb_to_oklab(color);
    let (mut best, mut second) = ((0usize, f32::INFINITY), (0usize, f32::INFINITY));
    for (i, p) in palette.iter().enumerate().skip(start) {
        let d = quant::oklab_sqdist(c, *p);
        if d < best.1 {
            second = best;
            best = (i, d);
        } else if d < second.1 {
            second = (i, d);
        }
    }
    (best.0, second.0, best.1, second.1)
}

/// Spreads the Floyd-Steinberg error of one pixel to its neighbors.
fn diffuse(buf: &mut [[f32; 3]], w: u32, h: u32, x: u32, y: u32, src: [f32; 3], quant: [f32; 3]) {
    let e = [src[0] - quant[0], src[1] - quant[1], src[2] - quant[2]];
    let mut add = |dx: i64, dy: i64, f: f32| {
        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
        if nx >= 0 && ny >= 0 && (nx as u32) < w && (ny as u32) < h {
            let j = (ny as u32 * w + nx as u32) as usize;
            for c in 0..3 {
                buf[j][c] += e[c] * f;
            }
        }
    };
    add(1, 0, 7.0 / 16.0);
    add(-1, 1, 3.0 / 16.0);
    add(0, 1, 5.0 / 16.0);
    add(1, 1, 1.0 / 16.0);
}

/// The alpha to write, and whether the pixel is keyed out.
fn alpha(setting: &General, a: u8) -> (u8, bool) {
    if setting.key && a < OPAQUE {
        (0, true)
    } else if setting.format.has_alpha() {
        (a, false)
    } else {
        (255, false)
    }
}

/// The preview pixels: the quantized colors, with the source transparency.
fn paint_preview(img: &RgbaImage, table: &[Rgb], indices: &[u8]) -> RgbaImage {
    let (w, h) = img.dimensions();
    let raw = img.as_raw();
    let mut out = RgbaImage::new(w, h);
    for (i, p) in out.pixels_mut().enumerate() {
        if raw[i * 4 + 3] < OPAQUE {
            *p = image::Rgba([0, 0, 0, 0]);
        } else {
            let c = table[usize::from(indices[i])];
            *p = image::Rgba([c.r, c.g, c.b, 255]);
        }
    }
    out
}

/// One member of a group, for a run that shares one palette.
pub struct Member<'a> {
    pub image: &'a RgbaImage,
    pub pins: Vec<bool>,
    pub open: bool,
}

/// Runs a whole group in one pass, so its palettes are shared. The preview
/// shows the open member.
pub fn run_group(setting: &Quantize, members: &[Member], tile: [u32; 2]) -> Result<Outcome, String> {
    if members.is_empty() {
        return Err("the group covers no sheets".into());
    }
    if members.len() == 1 {
        let m = &members[0];
        return run(setting, m.image, tile, &m.pins);
    }
    match setting.mode {
        QuantizeMode::Snes => match &setting.snes {
            Some(s) => Ok(run_snes_group(s, members, tile, 0)),
            None => Err("a SNES group has no SNES setting".into()),
        },
        QuantizeMode::General => match &setting.general {
            Some(g) if matches!(g.kind, GeneralKind::Downsample) => {
                let open = members.iter().find(|m| m.open).ok_or("the open sheet is not in the group")?;
                Ok(run_downsample(g, open.image))
            }
            Some(g) => {
                let train: Vec<_> = members.iter().flat_map(|m| train_pixels(g, m.image)).collect();
                let open = members.iter().find(|m| m.open).ok_or("the open sheet is not in the group")?;
                Ok(run_indexed_with(g, &train, open.image))
            }
            None => Err("a general group has no general setting".into()),
        },
    }
}

/// Stacks the members into one image and quantizes it once, so they share
/// the palettes and the CGRAM.
fn run_snes_group(setting: &Snes, members: &[Member], tile: [u32; 2], seed: u64) -> Outcome {
    let (tw, th) = (tile[0].max(1), tile[1].max(1));
    let width = members.iter().map(|m| m.image.width().div_ceil(tw) * tw).max().unwrap_or(tw);
    let bpp = match setting.depth {
        SnesDepth::Bpp2 => 2,
        SnesDepth::Bpp4 | SnesDepth::Bpp4Plus8 => 4,
        SnesDepth::Bpp8 => 8,
    };
    let stride = 1u32 << bpp;
    let palettes = setting.palettes.clamp(1, (256 / stride) as usize);
    let colors = setting.colors.clamp(1, stride as usize - 1);

    let mut pixels: Vec<Rgb> = Vec::new();
    let mut pins: Vec<bool> = Vec::new();
    let mut open_at = (0u32, 0u32, 0u32, 0u32);
    let mut y = 0u32;
    for m in members {
        let (w, h) = m.image.dimensions();
        let ph = h.div_ceil(th) * th;
        for py in 0..ph {
            for px in 0..width {
                if px < w && py < h {
                    let p = m.image.get_pixel(px, py).0;
                    pixels.push(if p[3] < OPAQUE { KEY } else { Rgb::new(p[0], p[1], p[2]) });
                } else {
                    pixels.push(KEY);
                }
            }
        }
        let across = width / tw;
        let mcols = w.div_ceil(tw);
        let mrows = h.div_ceil(th);
        for ty in 0..(ph / th) {
            for tx in 0..across {
                let inside = tx < mcols && ty < mrows;
                pins.push(inside && m.pins.get((ty * mcols + tx) as usize).copied().unwrap_or(false));
            }
        }
        if m.open {
            open_at = (0, y, w, h);
        }
        y += ph;
    }
    let height = y;

    let mut job = Job::new(bpp, palettes, colors);
    job.tile_width = tw;
    job.tile_height = th;
    job.dither = dither(setting.dither);
    job.color_zero = Some(KEY);
    job.seed = seed;
    if matches!(setting.depth, SnesDepth::Bpp4Plus8) {
        let start = palettes as u32 * stride;
        let extra = setting.extra_8bpp.min((256u32.saturating_sub(start)) as usize);
        job.deep_entries = (0..extra as u32).map(|i| (start + i) as u8).collect();
        let want = (setting.auto_pct as usize * pins.len() + 50) / 100;
        let have = pins.iter().filter(|&&d| d).count();
        job.deep_tiles = if want > have { quant::choose_deep_tiles(&pixels, width, height, &job, want - have) } else { pins.clone() };
    }
    let result = quant::quantize(&pixels, width, height, &job);
    let table = quant::cgram(&result, &job);
    let indices = quant::index_image(&result, &job, width, height);

    let (ox, oy, ow, oh) = open_at;
    let open_img = members.iter().find(|m| m.open).map(|m| m.image);
    let mut preview = RgbaImage::new(ow, oh);
    let mut out_indices = vec![0u8; (ow * oh) as usize];
    for py in 0..oh {
        for px in 0..ow {
            let ci = ((oy + py) * width + ox + px) as usize;
            let i = (py * ow + px) as usize;
            out_indices[i] = indices[ci];
            let a = open_img.map_or(255, |im| im.get_pixel(px, py).0[3]);
            if a < OPAQUE {
                preview.put_pixel(px, py, image::Rgba([0, 0, 0, 0]));
            } else {
                let c = table[usize::from(indices[ci])];
                preview.put_pixel(px, py, image::Rgba([c.r, c.g, c.b, 255]));
            }
        }
    }
    let across = width / tw;
    let mut deep_tiles = 0;
    for ty in (oy / th)..((oy + oh).div_ceil(th)) {
        for tx in 0..ow.div_ceil(tw) {
            if result.tile_deep[(ty * across + tx) as usize] {
                deep_tiles += 1;
            }
        }
    }
    Outcome { preview, export: Export::Indexed { palette: table, indices: out_indices, key: true }, deep_tiles }
}

/// Runs the open sheet through its setting. `pins` is one flag per tile.
pub fn run(setting: &Quantize, img: &RgbaImage, tile: [u32; 2], pins: &[bool]) -> Result<Outcome, String> {
    match setting.mode {
        QuantizeMode::Snes => match &setting.snes {
            Some(s) => Ok(run_snes(s, img, tile, pins, 0)),
            None => Err("a SNES group has no SNES setting".into()),
        },
        QuantizeMode::General => match &setting.general {
            Some(g) => Ok(run_general(g, img)),
            None => Err("a general group has no general setting".into()),
        },
    }
}

/// Writes an outcome to `path`.
pub fn write(outcome: &Outcome, path: &std::path::Path, width: u32, height: u32) -> Result<(), String> {
    match &outcome.export {
        Export::Indexed { palette, indices, key } => quant::export_indexed_png(path, palette, indices, width, height, *key),
        Export::Image => outcome.preview.save(path).map_err(|e| e.to_string()),
    }
}

/// A default setting for the open sheet.
pub fn default_setting() -> Quantize {
    Quantize {
        mode: QuantizeMode::Snes,
        general: Some(General {
            kind: GeneralKind::Indexed,
            format: GeneralFormat::Rgb888,
            colors: 256,
            key: true,
            dither: QuantizeDither::None,
        }),
        snes: Some(Snes {
            depth: SnesDepth::Bpp4,
            palettes: 8,
            colors: 15,
            dither: QuantizeDither::None,
            auto_pct: 0,
            extra_8bpp: 0,
        }),
    }
}

/// The names of the dither patterns, in the order the dropdown shows them.
const DITHERS: [(QuantizeDither, &str); 8] = [
    (QuantizeDither::None, "none"),
    (QuantizeDither::Bayer2, "bayer 2x2"),
    (QuantizeDither::Checker, "checker"),
    (QuantizeDither::StippleV, "stipple v"),
    (QuantizeDither::StippleH, "stipple h"),
    (QuantizeDither::LineV, "line v"),
    (QuantizeDither::LineH, "line h"),
    (QuantizeDither::FloydSteinberg, "floyd-steinberg"),
];

fn dither_name(which: QuantizeDither) -> &'static str {
    DITHERS.iter().find(|(d, _)| *d == which).map_or("none", |(_, n)| n)
}

fn dither_combo(ui: &mut egui::Ui, which: &mut QuantizeDither) -> bool {
    let mut changed = false;
    egui::ComboBox::from_label("dither").selected_text(dither_name(*which)).show_ui(ui, |ui| {
        for (value, name) in DITHERS {
            if ui.selectable_value(which, value, name).changed() {
                changed = true;
            }
        }
    });
    changed
}

/// The controls of a SNES setting. Returns whether anything changed.
fn snes_ui(ui: &mut egui::Ui, s: &mut Snes) -> bool {
    let mut changed = false;
    let depth_name = |d: SnesDepth| match d {
        SnesDepth::Bpp2 => "2bpp",
        SnesDepth::Bpp4 => "4bpp",
        SnesDepth::Bpp8 => "8bpp",
        SnesDepth::Bpp4Plus8 => "4bpp + 8bpp",
    };
    egui::ComboBox::from_label("depth").selected_text(depth_name(s.depth)).show_ui(ui, |ui| {
        for (value, name) in [
            (SnesDepth::Bpp2, "2bpp"),
            (SnesDepth::Bpp4, "4bpp"),
            (SnesDepth::Bpp8, "8bpp"),
            (SnesDepth::Bpp4Plus8, "4bpp + 8bpp"),
        ] {
            if ui.selectable_value(&mut s.depth, value, name).changed() {
                changed = true;
            }
        }
    });
    let bpp = match s.depth {
        SnesDepth::Bpp2 => 2,
        SnesDepth::Bpp4 | SnesDepth::Bpp4Plus8 => 4,
        SnesDepth::Bpp8 => 8,
    };
    let stride = 1usize << bpp;
    if bpp != 8 {
        changed |= ui.add(egui::Slider::new(&mut s.palettes, 1..=(256 / stride)).text("palettes")).changed();
    } else {
        s.palettes = 1;
    }
    changed |= ui.add(egui::Slider::new(&mut s.colors, 1..=(stride - 1)).text("colors per palette")).changed();
    changed |= dither_combo(ui, &mut s.dither);
    if matches!(s.depth, SnesDepth::Bpp4Plus8) {
        changed |= ui.add(egui::Slider::new(&mut s.auto_pct, 0..=100).text("auto 8bpp %")).changed();
        changed |= ui.add(egui::Slider::new(&mut s.extra_8bpp, 0..=128).text("extra 8bpp colors")).changed();
    }
    ui.weak("index 0 of every palette is transparent");
    changed
}

/// The controls of a general setting. Returns whether anything changed.
fn general_ui(ui: &mut egui::Ui, g: &mut General) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= ui.selectable_value(&mut g.kind, GeneralKind::Downsample, "downsample").changed();
        changed |= ui.selectable_value(&mut g.kind, GeneralKind::Indexed, "indexed").changed();
    });
    let formats: &[(GeneralFormat, &str)] = match g.kind {
        GeneralKind::Downsample => &[
            (GeneralFormat::Rgb565, "rgb565"),
            (GeneralFormat::Rgba5658, "rgba5658"),
            (GeneralFormat::Rgb233, "rgb233"),
        ],
        GeneralKind::Indexed => &[
            (GeneralFormat::Rgb565, "rgb565"),
            (GeneralFormat::Rgba5658, "rgba5658"),
            (GeneralFormat::Rgb888, "rgb888"),
            (GeneralFormat::Rgba8888, "rgba8888"),
        ],
    };
    let name = |f: GeneralFormat| match f {
        GeneralFormat::Rgb565 => "rgb565",
        GeneralFormat::Rgba5658 => "rgba5658",
        GeneralFormat::Rgb233 => "rgb233",
        GeneralFormat::Rgb888 => "rgb888",
        GeneralFormat::Rgba8888 => "rgba8888",
    };
    egui::ComboBox::from_label("format").selected_text(name(g.format)).show_ui(ui, |ui| {
        for (value, text) in formats {
            if ui.selectable_value(&mut g.format, *value, *text).changed() {
                changed = true;
            }
        }
    });
    if !formats.iter().any(|(f, _)| *f == g.format) {
        g.format = formats[0].0;
        changed = true;
    }
    if matches!(g.kind, GeneralKind::Indexed) {
        changed |= ui.add(egui::Slider::new(&mut g.colors, 2..=256).text("palette colors")).changed();
        if !g.format.has_alpha() {
            changed |= ui.checkbox(&mut g.key, "color 0 is transparent").changed();
        } else {
            g.key = false;
        }
    }
    changed |= dither_combo(ui, &mut g.dither);
    changed
}

impl App {
    /// Pins or unpins the tile under a paint click.
    pub fn quant_paint(&mut self, cell: (u32, u32)) {
        let Some(sheet) = self.project.sheet.as_ref() else { return };
        let count = (sheet.cols() * sheet.rows()) as usize;
        let index = (cell.1 * sheet.cols() + cell.0) as usize;
        if index >= count {
            return;
        }
        let set = self.quant_tool == Some(Tool::Paint);
        if self.quant_pins.len() != count {
            self.quant_pins.resize(count, false);
        }
        if self.quant_pins[index] != set {
            self.quant_pins[index] = set;
            self.quant_dirty = true;
        }
    }

    /// Chooses a target from the tree and opens the panel on it.
    pub fn quant_set_target(&mut self, target: QuantizeTarget) {
        let members = self.project.index.quantize_members(&target);
        let group = self
            .project
            .index
            .quantize
            .iter()
            .find(|g| self.project.index.quantize_members(&g.target).iter().any(|r| members.contains(r)))
            .cloned();
        match group {
            Some(g) => {
                self.quant = g.setting;
                self.quant_pins = match (&target, members.first()) {
                    (QuantizeTarget::Files(rels), Some(rel)) if rels.len() == 1 => g.pins.get(rel).cloned().unwrap_or_default(),
                    _ => Vec::new(),
                };
            }
            None => {
                self.quant = default_setting();
                self.quant_pins.clear();
            }
        }
        self.quant_target = Some(target);
        self.quant_panel = true;
        self.quant_open_rel = self.project.sheet.as_ref().map(|s| s.rel.clone());
        self.quant_load_members();
        self.quant_dirty = true;
    }

    /// Loads the open sheet's group into the panel, or a fresh setting.
    pub fn quant_open_sheet(&mut self) {
        let rel = self.project.sheet.as_ref().map(|s| s.rel.clone()).unwrap_or_default();
        let found = self
            .project
            .index
            .quantize
            .iter()
            .find(|g| self.project.index.quantize_members(&g.target).contains(&rel))
            .cloned();
        match found {
            Some(g) => {
                self.quant = g.setting;
                self.quant_pins = g.pins.get(&rel).cloned().unwrap_or_default();
                self.quant_target = Some(g.target);
            }
            None => {
                self.quant = default_setting();
                self.quant_target = Some(QuantizeTarget::Files(vec![rel]));
                self.quant_pins.clear();
            }
        }
        self.quant_load_members();
        self.quant_dirty = true;
    }

    /// Loads the images of the current target, once, for the shared run.
    pub fn quant_load_members(&mut self) {
        let Some(target) = self.quant_target.clone() else {
            self.quant_members.clear();
            return;
        };
        let open_rel = self.project.sheet.as_ref().map(|s| s.rel.clone());
        let root = self.project.index.root.clone();
        let rels = self.project.index.quantize_members(&target);
        let mut members = Vec::new();
        for rel in rels {
            if Some(&rel) == open_rel.as_ref()
                && let Some(sheet) = self.project.sheet.as_ref()
            {
                members.push((rel, sheet.img.clone()));
                continue;
            }
            if let Ok(img) = image::open(root.join(&rel)) {
                members.push((rel, img.to_rgba8()));
            }
        }
        self.quant_members = members;
    }

    /// Recomputes the preview from the whole target and the working setting.
    pub fn quant_rebuild(&mut self) {
        if self.quant_members.is_empty() {
            self.quant_load_members();
        }
        let Some(tile) = self.project.sheet.as_ref().map(|s| s.tile) else { return };
        let outcome = {
            let open_rel = self.project.sheet.as_ref().map(|s| s.rel.clone());
            let pins = self.quant_pins.clone();
            let group_pins = self
                .project
                .index
                .quantize
                .iter()
                .find(|g| {
                    let target = self.quant_target.as_ref();
                    target.is_some_and(|t| self.project.index.quantize_members(&g.target) == self.project.index.quantize_members(t))
                })
                .map(|g| g.pins.clone())
                .unwrap_or_default();
            let members: Vec<Member> = self
                .quant_members
                .iter()
                .map(|(rel, img)| Member {
                    image: img,
                    pins: if Some(rel) == open_rel.as_ref() { pins.clone() } else { group_pins.get(rel).cloned().unwrap_or_default() },
                    open: Some(rel) == open_rel.as_ref(),
                })
                .collect();
            run_group(&self.quant, &members, tile)
        };
        match outcome {
            Ok(o) => {
                self.quant_preview = Some(o);
                self.quant_error.clear();
            }
            Err(e) => {
                self.quant_error = e;
                self.quant_preview = None;
            }
        }
    }

    /// Writes the working setting as a group. With `delete_existing`, the
    /// sheets that already carry a setting give theirs up first.
    pub fn quant_apply(&mut self, delete_existing: bool) {
        let Some(target) = self.quant_target.clone() else { return };
        let members = self.project.index.quantize_members(&target);
        if members.is_empty() {
            self.quant_error = "the target covers no sheets".into();
            return;
        }
        let covered: Vec<String> = self
            .project
            .index
            .quantize
            .iter()
            .flat_map(|g| self.project.index.quantize_members(&g.target))
            .filter(|r| members.contains(r))
            .collect();
        if !covered.is_empty() && !delete_existing {
            self.quant_conflict = Some(Conflict { covered });
            return;
        }
        // Drop the covered sheets from every group, and the groups that empty.
        let mut groups = self.project.index.quantize.clone();
        for g in &mut groups {
            let gmembers = self.project.index.quantize_members(&g.target);
            let keep = !gmembers.iter().any(|r| members.contains(r));
            if !keep {
                g.pins.retain(|rel, _| !members.contains(rel));
                g.target = QuantizeTarget::Files(gmembers.into_iter().filter(|r| !members.contains(r)).collect());
            }
        }
        groups.retain(|g| match &g.target {
            QuantizeTarget::Files(rels) => !rels.is_empty(),
            QuantizeTarget::Dir(_) => true,
        });
        let pins: std::collections::BTreeMap<String, Vec<bool>> = {
            let rel = self.project.sheet.as_ref().map(|s| s.rel.clone()).unwrap_or_default();
            if matches!(self.quant.mode, QuantizeMode::Snes) && !self.quant_pins.is_empty() && members.contains(&rel) {
                [(rel, self.quant_pins.clone())].into_iter().collect()
            } else {
                Default::default()
            }
        };
        groups.push(QuantizeGroup { target: target.clone(), setting: self.quant.clone(), pins });
        match crate::sidecar::store_quantize(&self.project.index.root.clone(), &groups) {
            Ok(()) => {
                self.project.index.quantize = groups;
                self.quant_conflict = None;
                self.quant_dirty = true;
                self.status = "color quantization saved".into();
            }
            Err(e) => self.quant_error = e,
        }
    }

    /// Removes the settings that cover the open sheet.
    pub fn quant_clear(&mut self) {
        let rel = self.project.sheet.as_ref().map(|s| s.rel.clone()).unwrap_or_default();
        let mut groups = self.project.index.quantize.clone();
        for g in &mut groups {
            let members = self.project.index.quantize_members(&g.target);
            if !members.contains(&rel) {
                continue;
            }
            g.pins.remove(&rel);
            g.target = QuantizeTarget::Files(members.into_iter().filter(|r| *r != rel).collect());
        }
        groups.retain(|g| match &g.target {
            QuantizeTarget::Files(rels) => !rels.is_empty(),
            QuantizeTarget::Dir(_) => true,
        });
        match crate::sidecar::store_quantize(&self.project.index.root.clone(), &groups) {
            Ok(()) => {
                self.project.index.quantize = groups;
                self.quant_preview = None;
                self.quant_dirty = false;
                if let Some(sheet) = self.project.sheet.as_mut() {
                    sheet.clear_quant_preview();
                }
                self.status = "color quantization cleared".into();
            }
            Err(e) => self.quant_error = e,
        }
    }

    /// Asks where to save the quantized PNG, then writes it.
    pub fn quant_save(&mut self) {
        let name = self
            .project
            .sheet
            .as_ref()
            .map(|s| {
                let stem = std::path::Path::new(&s.rel).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| "sheet".into());
                format!("{stem}-quantized.png")
            })
            .unwrap_or_else(|| "sheet-quantized.png".into());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let path = rfd::FileDialog::new().set_file_name(name).save_file();
            let _ = tx.send(path);
        });
        self.quant_picking = Some(rx);
    }

    fn quant_poll_save(&mut self) {
        let Some(rx) = &self.quant_picking else { return };
        match rx.try_recv() {
            Ok(answer) => {
                self.quant_picking = None;
                if let (Some(path), Some(outcome), Some(sheet)) = (answer, &self.quant_preview, self.project.sheet.as_ref()) {
                    let (w, h) = sheet.img.dimensions();
                    match write(outcome, &path, w, h) {
                        Ok(()) => self.status = format!("saved {}", path.display()),
                        Err(e) => self.quant_error = e,
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.quant_picking = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    /// The right side panel of the project half.
    pub fn quant_side_panel(&mut self, ui: &mut egui::Ui, keys: bool) {
        self.quant_poll_save();
        ui.heading(crate::title_text("Color quantization", keys));
        let target = self.quant_target.clone();
        match &target {
            Some(t) => {
                ui.label(format!("target: {}", self.project.index.target_name(t)));
                if ui.selectable_label(self.quant_paths, "paths").clicked() {
                    self.quant_paths = !self.quant_paths;
                }
                if self.quant_paths {
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        for rel in self.project.index.quantize_members(t) {
                            ui.weak(rel);
                        }
                    });
                }
            }
            None => {
                ui.weak("no sheet");
            }
        }
        ui.separator();
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui.selectable_value(&mut self.quant.mode, QuantizeMode::Snes, "SNES").changed();
            changed |= ui.selectable_value(&mut self.quant.mode, QuantizeMode::General, "general").changed();
        });
        match self.quant.mode {
            QuantizeMode::Snes => {
                if let Some(s) = self.quant.snes.as_mut() {
                    changed |= snes_ui(ui, s);
                }
            }
            QuantizeMode::General => {
                if let Some(g) = self.quant.general.as_mut() {
                    changed |= general_ui(ui, g);
                }
            }
        }
        ui.separator();
        if matches!(self.quant.mode, QuantizeMode::Snes)
            && self.quant.snes.as_ref().is_some_and(|s| matches!(s.depth, SnesDepth::Bpp4Plus8))
        {
            ui.label("draw on tiles");
            ui.horizontal(|ui| {
                let tool = self.quant_tool;
                if ui.selectable_label(tool == Some(Tool::Paint), "paint 8bpp").clicked() {
                    self.quant_tool = if tool == Some(Tool::Paint) { None } else { Some(Tool::Paint) };
                }
                if ui.selectable_label(tool == Some(Tool::Erase), "erase").clicked() {
                    self.quant_tool = if tool == Some(Tool::Erase) { None } else { Some(Tool::Erase) };
                }
            });
        } else {
            self.quant_tool = None;
        }
        if let Some(o) = &self.quant_preview {
            ui.weak(format!("{} tiles are 8bpp", o.deep_tiles));
        }
        if !self.quant_error.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), &self.quant_error);
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() {
                self.quant_apply(false);
            }
            if ui.button("Clear").clicked() {
                self.quant_clear();
            }
            if ui.add_enabled(self.quant_preview.is_some(), egui::Button::new("Save Quantized...")).clicked() {
                self.quant_save();
            }
        });
        if changed {
            self.quant_dirty = true;
        }
        if self.quant_dirty {
            self.quant_dirty = false;
            self.quant_rebuild();
            if let (Some(sheet), Some(o)) = (self.project.sheet.as_mut(), &self.quant_preview) {
                sheet.set_quant_preview(ui.ctx(), &o.preview);
            }
        }
        if let Some(sheet) = self.project.sheet.as_mut() {
            sheet.quant_tool = self.quant_tool;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16x8 sheet: a busy red tile on the left, a flat green one on the right.
    fn sheet() -> RgbaImage {
        let mut img = RgbaImage::new(16, 8);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = if x < 8 {
                image::Rgba([(x * 30) as u8, (y * 30) as u8, 40, 255])
            } else {
                image::Rgba([0, 200, 0, 255])
            };
        }
        img
    }

    #[test]
    fn a_snes_run_makes_a_preview_and_an_indexed_export() {
        let setting = Snes { depth: SnesDepth::Bpp4, palettes: 1, colors: 15, dither: QuantizeDither::None, auto_pct: 0, extra_8bpp: 0 };
        let out = run_snes(&setting, &sheet(), [8, 8], &[], 0);
        assert_eq!(out.preview.dimensions(), (16, 8));
        match out.export {
            Export::Indexed { palette, indices, key } => {
                assert_eq!(palette.len(), 256, "the CGRAM is 256 entries");
                assert_eq!(indices.len(), 16 * 8);
                assert!(key, "SNES index 0 is transparent");
            }
            Export::Image => panic!("SNES exports an indexed image"),
        }
    }

    /// A transparent pixel is index 0 and shows as transparent.
    #[test]
    fn a_transparent_pixel_is_keyed() {
        let mut img = RgbaImage::new(8, 8);
        for p in img.pixels_mut() {
            *p = image::Rgba([200, 100, 50, 0]);
        }
        let setting = Snes { depth: SnesDepth::Bpp4, palettes: 1, colors: 15, dither: QuantizeDither::None, auto_pct: 0, extra_8bpp: 0 };
        let out = run_snes(&setting, &img, [8, 8], &[], 0);
        assert_eq!(out.preview.get_pixel(0, 0).0[3], 0);
        match out.export {
            Export::Indexed { indices, .. } => assert!(indices.iter().all(|&i| i == 0)),
            Export::Image => panic!(),
        }
    }

    /// A downsample keeps no palette; the indexed kind does.
    #[test]
    fn the_general_kinds_differ() {
        let mut g = General { kind: GeneralKind::Downsample, format: GeneralFormat::Rgb565, colors: 16, key: false, dither: QuantizeDither::None };
        assert!(matches!(run_general(&g, &sheet()).export, Export::Image));
        g.kind = GeneralKind::Indexed;
        g.format = GeneralFormat::Rgb888;
        match run_general(&g, &sheet()).export {
            Export::Indexed { palette, .. } => assert!(palette.len() <= 16),
            Export::Image => panic!("the indexed kind exports a palette"),
        }
    }

    /// Two sheets run together share one palette; the preview shows the open one.
    #[test]
    fn a_group_runs_as_one() {
        let a = sheet();
        let mut b = RgbaImage::new(16, 8);
        for (x, _, p) in b.enumerate_pixels_mut() {
            *p = image::Rgba([(x * 10) as u8, 0, 200, 255]);
        }
        let members = vec![
            Member { image: &a, pins: Vec::new(), open: true },
            Member { image: &b, pins: Vec::new(), open: false },
        ];
        let snes = Snes { depth: SnesDepth::Bpp4, palettes: 1, colors: 15, dither: QuantizeDither::None, auto_pct: 0, extra_8bpp: 0 };
        let setting = Quantize { mode: QuantizeMode::Snes, general: None, snes: Some(snes) };
        let out = run_group(&setting, &members, [8, 8]).unwrap();
        assert_eq!(out.preview.dimensions(), (16, 8));
        match out.export {
            Export::Indexed { palette, indices, .. } => {
                assert_eq!(palette.len(), 256);
                assert_eq!(indices.len(), 16 * 8);
            }
            Export::Image => panic!(),
        }
    }

    /// A dither changes the general index image.
    #[test]
    fn a_general_dither_changes_the_indices() {
        let mut img = RgbaImage::new(8, 8);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x * 30) as u8, (x * 30) as u8, (x * 30) as u8, 255]);
        }
        let base = General { kind: GeneralKind::Indexed, format: GeneralFormat::Rgb888, colors: 2, key: false, dither: QuantizeDither::None };
        let dithered = General { dither: QuantizeDither::Checker, ..base.clone() };
        let indices = |g: &General| match run_general(g, &img).export {
            Export::Indexed { indices, .. } => indices,
            Export::Image => panic!(),
        };
        assert_ne!(indices(&base), indices(&dithered), "checker dither changes the index image");
    }
}
