//! The ordered dither patterns the incremental quantizer supports.
//!
//! Error-diffusion dithers (Atkinson, Floyd-Steinberg) are not part of the
//! tile-aware method; larger Bayer patterns are handled elsewhere. These are
//! the patterns that bias a candidate before matching.

/// Weight applied to the accumulated error when biasing a candidate.
pub(crate) const DITHER_WEIGHT: f32 = 0.5;

/// Most candidates tested per pixel.
pub(crate) const MAX_CANDIDATES: usize = 4;

type Matrix = [[usize; 2]; 2];

#[rustfmt::skip]
const BAYER_2X2: Matrix = [[0, 2], [3, 1]];
#[rustfmt::skip]
const CHECKER: Matrix = [[0, 1], [1, 0]];
#[rustfmt::skip]
const STIPPLE_V: Matrix = [[0, 1], [3, 2]];
#[rustfmt::skip]
const STIPPLE_H: Matrix = [[0, 3], [1, 2]];
#[rustfmt::skip]
const LINE_V: Matrix = [[0, 0], [1, 1]];
#[rustfmt::skip]
const LINE_H: Matrix = [[0, 1], [0, 1]];

/// A dithered matching pattern.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dither {
    Bayer2x2,
    Checker,
    StippleV,
    StippleH,
    LineV,
    LineH,
}

impl Dither {
    fn table(self) -> Matrix {
        match self {
            Dither::Bayer2x2 => BAYER_2X2,
            Dither::Checker => CHECKER,
            Dither::StippleV => STIPPLE_V,
            Dither::StippleH => STIPPLE_H,
            Dither::LineV => LINE_V,
            Dither::LineH => LINE_H,
        }
    }

    /// Number of candidates tested per pixel.
    pub(crate) fn candidates(self) -> usize {
        match self {
            Dither::Bayer2x2 | Dither::StippleH | Dither::StippleV => 4,
            Dither::Checker | Dither::LineH | Dither::LineV => 2,
        }
    }

    /// Rank of the candidate to pick at `(x, y)`.
    pub(crate) fn rank(self, x: u32, y: u32) -> usize {
        self.table()[(x & 1) as usize][(y & 1) as usize]
    }
}
