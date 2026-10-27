//! Latency marker: a tick id painted into the top-left corner of a video frame.
//!
//! The marker is a grid of large black/white blocks so it survives lossy encoding:
//! [`ROWS`] x [`COLS`] blocks of [`BLOCK`] px, read row by row. The first [`GUARD`] bits
//! are a fixed pattern that tells a real marker apart from screen content; the remaining
//! [`ID_BITS`] bits are the id, most significant bit first.

/// Edge length of one block in pixels. Even, so chroma planes line up.
pub const BLOCK: usize = 32;
pub const COLS: usize = 12;
pub const ROWS: usize = 2;
pub const GUARD: [bool; 4] = [true, false, true, false];
pub const ID_BITS: usize = ROWS * COLS - GUARD.len();
pub const MAX_ID: u32 = (1 << ID_BITS) - 1;

/// Size of the marked region in luma pixels: (width, height).
pub const REGION: (usize, usize) = (COLS * BLOCK, ROWS * BLOCK);

// Limited-range luma, which is what WebRTC's I420 pipeline assumes.
const WHITE: u8 = 235;
const BLACK: u8 = 16;
const THRESHOLD: u32 = (WHITE as u32 + BLACK as u32) / 2;
const NEUTRAL_CHROMA: u8 = 128;

fn bits(id: u32) -> impl Iterator<Item = bool> {
    let id = id & MAX_ID;
    GUARD
        .into_iter()
        .chain((0..ID_BITS).rev().map(move |i| id >> i & 1 == 1))
}

fn fits(width: usize, height: usize) -> bool {
    width >= REGION.0 && height >= REGION.1
}

/// Paints `id` into an 8-bit luma plane. Returns `false` if the frame is too small.
pub fn stamp_luma(y: &mut [u8], stride: usize, width: usize, height: usize, id: u32) -> bool {
    if !fits(width, height) {
        return false;
    }
    for (i, bit) in bits(id).enumerate() {
        let (bx, by) = ((i % COLS) * BLOCK, (i / COLS) * BLOCK);
        let value = if bit { WHITE } else { BLACK };
        for row in by..by + BLOCK {
            y[row * stride + bx..row * stride + bx + BLOCK].fill(value);
        }
    }
    true
}

/// Sets a half-resolution chroma plane (I420 U or V) to neutral under the marker.
pub fn neutral_chroma_planar(plane: &mut [u8], stride: usize) {
    let (w, h) = (REGION.0 / 2, REGION.1 / 2);
    for row in 0..h {
        plane[row * stride..row * stride + w].fill(NEUTRAL_CHROMA);
    }
}

/// Sets an interleaved NV12 UV plane to neutral under the marker.
pub fn neutral_chroma_nv12(uv: &mut [u8], stride: usize) {
    // REGION.0 / 2 chroma samples per row, two bytes (U, V) each.
    let (w, h) = (REGION.0, REGION.1 / 2);
    for row in 0..h {
        uv[row * stride..row * stride + w].fill(NEUTRAL_CHROMA);
    }
}

/// Reads a marker id from a luma plane, or `None` if no valid marker is present.
///
/// Each bit is the mean of the inner half of its block, which ignores ringing that the
/// encoder adds at block edges.
pub fn read_luma(y: &[u8], stride: usize, width: usize, height: usize) -> Option<u32> {
    if !fits(width, height) {
        return None;
    }
    let inset = BLOCK / 4;
    let inner = BLOCK - 2 * inset;
    let mut id = 0u32;
    for i in 0..ROWS * COLS {
        let (bx, by) = ((i % COLS) * BLOCK + inset, (i / COLS) * BLOCK + inset);
        let sum: u32 = (by..by + inner)
            .flat_map(|row| &y[row * stride + bx..row * stride + bx + inner])
            .map(|&v| u32::from(v))
            .sum();
        let bit = sum / (inner * inner) as u32 > THRESHOLD;
        if i < GUARD.len() {
            if bit != GUARD[i] {
                return None;
            }
        } else {
            id = id << 1 | u32::from(bit);
        }
    }
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 640;
    const H: usize = 360;

    fn frame(fill: u8) -> Vec<u8> {
        vec![fill; W * H]
    }

    #[test]
    fn round_trips_ids() {
        for id in [0, 1, 0b1010, 12345, MAX_ID] {
            let mut y = frame(90);
            assert!(stamp_luma(&mut y, W, W, H, id));
            assert_eq!(read_luma(&y, W, W, H), Some(id));
        }
    }

    #[test]
    fn ids_wrap_to_available_bits() {
        let mut y = frame(0);
        stamp_luma(&mut y, W, W, H, MAX_ID + 6);
        assert_eq!(read_luma(&y, W, W, H), Some(5));
    }

    #[test]
    fn rejects_frames_without_marker() {
        for fill in [0, 16, 128, 235, 255] {
            assert_eq!(read_luma(&frame(fill), W, W, H), None);
        }
    }

    #[test]
    fn rejects_frames_that_are_too_small() {
        let mut y = vec![0; 100 * 50];
        assert!(!stamp_luma(&mut y, 100, 100, 50, 1));
        assert_eq!(read_luma(&y, 100, 100, 50), None);
    }

    #[test]
    fn respects_stride() {
        let stride = W + 64;
        let mut y = vec![50; stride * H];
        stamp_luma(&mut y, stride, W, H, 777);
        assert_eq!(read_luma(&y, stride, W, H), Some(777));
    }

    #[test]
    fn survives_noise_and_blur() {
        let mut y = frame(128);
        stamp_luma(&mut y, W, W, H, 424_242 & MAX_ID);
        // Deterministic noise of +-40 plus a 3-tap horizontal blur, harsher than VP8 at 4 Mbps.
        let mut seed = 0x2545_f491_u32;
        for v in y.iter_mut() {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *v = (i32::from(*v) + (seed % 81) as i32 - 40).clamp(0, 255) as u8;
        }
        let blurred: Vec<u8> = (0..y.len())
            .map(|i| {
                let l = y[i.saturating_sub(1)] as u32;
                let r = y[(i + 1).min(y.len() - 1)] as u32;
                ((l + 2 * y[i] as u32 + r) / 4) as u8
            })
            .collect();
        assert_eq!(read_luma(&blurred, W, W, H), Some(424_242 & MAX_ID));
    }

    #[test]
    fn neutralizes_chroma() {
        let stride = W / 2;
        let mut u = vec![0; stride * H / 2];
        neutral_chroma_planar(&mut u, stride);
        assert_eq!(u[0], NEUTRAL_CHROMA);
        assert_eq!(u[(REGION.1 / 2 - 1) * stride + REGION.0 / 2 - 1], NEUTRAL_CHROMA);
        assert_eq!(u[REGION.0 / 2], 0);

        let mut uv = vec![0; W * H / 2];
        neutral_chroma_nv12(&mut uv, W);
        assert_eq!(uv[REGION.0 - 1], NEUTRAL_CHROMA);
        assert_eq!(uv[REGION.0], 0);
    }
}
