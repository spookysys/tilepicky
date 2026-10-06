// SPDX-License-Identifier: GPL-3.0-only
//! Reading a quantization back into pixels and indices, and writing an
//! indexed PNG. The crate's own tests rebuild the image this way; this is
//! the same walk, exposed for the preview and the export.

use crate::quant::{Job, Quantized, Rgb};
use std::path::Path;

/// The 256-entry CGRAM a result fills: the color zero at 0, each subpalette
/// at its stride, and the deep-only entries where they belong.
pub fn cgram(result: &Quantized, job: &Job) -> Vec<Rgb> {
    let stride = 1usize << job.bpp;
    let zero = job.color_zero.map_or(Rgb::new(0, 0, 0), |c| c.reduce().normalize());
    let mut table = vec![zero; 256];
    for (k, palette) in result.palettes.iter().enumerate() {
        for (i, &color) in palette.iter().enumerate() {
            let at = k * stride + i;
            if at < 256 {
                table[at] = color;
            }
        }
    }
    for (&entry, &color) in job.deep_entries.iter().zip(&result.deep_colors) {
        table[usize::from(entry)] = color;
    }
    table[0] = zero;
    table
}

/// One index per pixel, row-major, into `cgram`. A shallow pixel indexes its
/// subpalette; a deep pixel indexes CGRAM. A transparent pixel, or a mixed
/// tile's pixel that the other layer draws, is 0.
pub fn index_image(result: &Quantized, job: &Job, width: u32, height: u32) -> Vec<u8> {
    let (tw, th) = (job.tile_width.max(1), job.tile_height.max(1));
    let stride = 1u32 << job.bpp;
    let across = width.div_ceil(tw);
    let mut out = vec![0u8; (width * height) as usize];
    for (tile, indices) in result.tiles.iter().enumerate() {
        let (tx, ty) = (tile as u32 % across * tw, tile as u32 / across * th);
        let deep = result.tile_deep[tile];
        let palette = result.tile_palette[tile] as u32;
        let shallow_part = result.shallow_parts[tile].as_deref();
        for (i, &index) in indices.iter().enumerate() {
            let (x, y) = (tx + i as u32 % tw, ty + i as u32 / tw);
            if x >= width || y >= height {
                continue;
            }
            let shallow = job.shallow_pixels.get((y * width + x) as usize).copied().unwrap_or(false);
            let value = match shallow_part {
                Some(part) if shallow => palette * stride + u32::from(part[i]),
                _ if deep => u32::from(index),
                _ => palette * stride + u32::from(index),
            };
            out[(y * width + x) as usize] = value as u8;
        }
    }
    out
}

/// The image a result rebuilds, at the palettes' native depth.
pub fn rebuild(result: &Quantized, job: &Job, width: u32, height: u32) -> Vec<Rgb> {
    let table = cgram(result, job);
    index_image(result, job, width, height).into_iter().map(|i| table[usize::from(i)]).collect()
}

/// The smallest PNG index depth that holds `colors` entries.
fn index_depth(colors: usize) -> png::BitDepth {
    match colors {
        0..=2 => png::BitDepth::One,
        3..=4 => png::BitDepth::Two,
        5..=16 => png::BitDepth::Four,
        _ => png::BitDepth::Eight,
    }
}

/// Packs one-byte indices into PNG's MSB-first rows for a depth below eight.
fn pack_rows(indices: &[u8], width: u32, height: u32, bits: u8) -> Vec<u8> {
    if bits == 8 {
        return indices.to_vec();
    }
    let per_byte = (8 / bits) as usize;
    let row_bytes = (width as usize).div_ceil(per_byte);
    let mut out = vec![0u8; row_bytes * height as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let v = indices[y * width as usize + x] & ((1 << bits) - 1);
            let shift = 8 - (bits as usize) * (x % per_byte + 1);
            out[y * row_bytes + x / per_byte] |= v << shift;
        }
    }
    out
}

/// Writes an indexed PNG: a palette, one index per pixel, and index 0 made
/// transparent when `key` is set.
pub fn export_indexed_png(
    path: &Path,
    palette: &[Rgb],
    indices: &[u8],
    width: u32,
    height: u32,
    key: bool,
) -> Result<(), String> {
    let depth = index_depth(palette.len());
    let bits = depth as u8;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(depth);
    let rgb: Vec<u8> = palette.iter().flat_map(|c| [c.r, c.g, c.b]).collect();
    encoder.set_palette(rgb);
    if key && !palette.is_empty() {
        let mut trns = vec![255u8; palette.len()];
        trns[0] = 0;
        encoder.set_trns(trns);
    }
    let packed = pack_rows(indices, width, height, bits);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&packed).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_pack_from_the_high_bit() {
        // Four pixels at depth two: 0b00, 0b01, 0b10, 0b11 -> 0b00011011.
        assert_eq!(pack_rows(&[0, 1, 2, 3], 4, 1, 2), vec![0b0001_1011]);
        // Nine pixels at depth one: eight in the first byte, one in the next.
        assert_eq!(pack_rows(&[1, 0, 1, 0, 0, 0, 0, 1, 1], 9, 1, 1), vec![0b1010_0001, 0b1000_0000]);
    }

    #[test]
    fn depth_follows_the_palette_size() {
        assert_eq!(index_depth(2), png::BitDepth::One);
        assert_eq!(index_depth(4), png::BitDepth::Two);
        assert_eq!(index_depth(16), png::BitDepth::Four);
        assert_eq!(index_depth(256), png::BitDepth::Eight);
    }
}

#[cfg(test)]
mod write_tests {
    use super::*;

    /// The written file reads back as an indexed PNG with the palette and a
    /// transparent index 0.
    #[test]
    fn an_indexed_png_reads_back_with_its_palette() {
        let dir = crate::storage::tests::Folder::new();
        let path = dir.0.join("out.png");
        let palette = vec![Rgb::new(0, 0, 0), Rgb::new(255, 0, 0), Rgb::new(0, 255, 0), Rgb::new(0, 0, 255)];
        let indices = vec![0u8, 1, 2, 3, 3, 2, 1, 0];
        export_indexed_png(&path, &palette, &indices, 4, 2, true).unwrap();
        let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let reader = decoder.read_info().unwrap();
        let info = reader.info();
        assert_eq!(info.color_type, png::ColorType::Indexed);
        assert_eq!(info.bit_depth, png::BitDepth::Two);
        assert_eq!(info.palette.as_ref().unwrap().len(), 12);
        assert_eq!(info.trns.as_ref().unwrap()[0], 0);
    }
}
