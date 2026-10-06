//! sRGB, native 5-bit RGB, and Oklab color handling.
//!
//! The Oklab conversion follows Björn Ottosson's reference, the same math the
//! `quantette` crate uses. Distances weigh chroma against lightness by the
//! same factor as the reference (`CHROMA_WEIGHT`), because the palette choice
//! depends on that weight.

/// A raw 8-bit RGB color.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Rgb { r, g, b }
    }

    /// The SNES's native depth: the top five bits of each channel.
    pub fn reduce(self) -> Self {
        Rgb::new(self.r >> 3, self.g >> 3, self.b >> 3)
    }

    /// Scales a 5-bit color back to 8-bit range by bit replication, which
    /// maps 31 to 255 rather than to 248.
    pub fn normalize(self) -> Self {
        Rgb::new(scale_up(self.r), scale_up(self.g), scale_up(self.b))
    }
}

/// A perceptual color in Oklab.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Oklab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

impl Oklab {
    pub fn new(l: f32, a: f32, b: f32) -> Self {
        Oklab { l, a, b }
    }

    /// The native 5-bit color this value is closest to, as raw 5-bit channels.
    pub fn to_reduced(self) -> Rgb {
        oklab_to_rgb8(self).reduce()
    }

    /// The native 5-bit color this value is closest to, scaled back to 8-bit.
    pub fn to_rgb5(self) -> Rgb {
        self.to_reduced().normalize()
    }
}

/// Scales a 5-bit channel to 8 bits by replicating its bits.
fn scale_up(value: u8) -> u8 {
    let v = value as u32;
    (((v << 3) | (v >> 2)) & 0xff) as u8
}

/// Chroma mismatch penalty relative to lightness difference.
const CHROMA_WEIGHT: f32 = 2.15;

/// Squared Oklab distance, penalizing chroma mismatch.
pub fn oklab_sqdist(a: Oklab, b: Oklab) -> f32 {
    let dl = a.l - b.l;
    let da = a.a - b.a;
    let db = a.b - b.b;
    dl * dl + CHROMA_WEIGHT * (da * da + db * db)
}

/// Component-wise mean of `colors` in Oklab, or `None` if empty.
pub fn mean_oklab(colors: impl IntoIterator<Item = Oklab>) -> Option<Oklab> {
    let (mut l, mut a, mut b) = (0.0f32, 0.0f32, 0.0f32);
    let mut count = 0u32;
    for c in colors {
        l += c.l;
        a += c.a;
        b += c.b;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    let n = count as f32;
    Some(Oklab::new(l / n, a / n, b / n))
}

/// Converts an 8-bit sRGB color to Oklab.
///
/// This uses `quantette`'s conversion, the same one the reference uses, so the
/// result is bit-identical. A hand-rolled Oklab differs in the last bits, and
/// the palette choice can turn on that.
pub fn srgb_to_oklab(color: Rgb) -> Oklab {
    use quantette::deps::palette::Srgb;
    let converted = quantette::color_space::srgb8_to_oklab(&[Srgb::new(color.r, color.g, color.b)])[0];
    Oklab::new(converted.l, converted.a, converted.b)
}

/// Converts Oklab back to an 8-bit sRGB color, using the same conversion the
/// reference uses.
pub(crate) fn oklab_to_rgb8(c: Oklab) -> Rgb {
    use quantette::deps::palette::Oklab as QuantetteOklab;
    let srgb = quantette::color_space::oklab_to_srgb8(&[QuantetteOklab::new(c.l, c.a, c.b)])[0];
    Rgb::new(srgb.red, srgb.green, srgb.blue)
}


