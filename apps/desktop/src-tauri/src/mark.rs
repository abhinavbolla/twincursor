//! The menu bar icon: the two-chevron mark (`icons/icon.svg`, and the `Mark` component in the
//! React side), drawn here as a template image. macOS uses only its alpha and tints it for light
//! and dark menu bars, so it is black with coverage in the alpha. Drawn in code rather than
//! shipped as a PNG so it needs no image decoder and can't drift from the shapes below.

/// The mark's two strokes in its 20 × 20 drawing space, with their opacity: the trailing
/// chevron is fainter, as in the app icon.
const STROKES: [([(f64, f64); 3], f64); 2] = [
    ([(4.0, 5.0), (9.0, 10.0), (4.0, 15.0)], 0.45),
    ([(10.5, 5.0), (15.5, 10.0), (10.5, 15.0)], 1.0),
];
/// Stroke width in drawing units, with round caps and joins.
const STROKE: f64 = 2.6;
/// The part of the drawing space the image shows: the strokes with a unit of room around them,
/// so the mark fills the menu bar's 18 pt icon height the way other menu bar icons do.
const VIEW_ORIGIN: f64 = 1.0;
const VIEW_SIZE: f64 = 18.0;
/// Samples per pixel along each axis, for smooth edges.
const SAMPLES: u32 = 4;

/// The menu bar icon at `size` × `size` pixels (36 for the 18 pt icon at 2×), as RGBA.
pub fn template_rgba(size: u32) -> Vec<u8> {
    let scale = VIEW_SIZE / f64::from(size);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut cover = 0.0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = VIEW_ORIGIN
                        + (f64::from(px) + (f64::from(sx) + 0.5) / f64::from(SAMPLES)) * scale;
                    let y = VIEW_ORIGIN
                        + (f64::from(py) + (f64::from(sy) + 0.5) / f64::from(SAMPLES)) * scale;
                    cover += alpha_at(x, y);
                }
            }
            let a = cover / f64::from(SAMPLES * SAMPLES);
            out.extend_from_slice(&[0, 0, 0, (a * 255.0).round() as u8]);
        }
    }
    out
}

/// How opaque the mark is at one point of the drawing space.
fn alpha_at(x: f64, y: f64) -> f64 {
    STROKES
        .iter()
        .filter(|(points, _)| {
            points
                .windows(2)
                .any(|seg| distance_to_segment((x, y), seg[0], seg[1]) <= STROKE / 2.0)
        })
        .map(|(_, opacity)| *opacity)
        .fold(0.0, f64::max)
}

fn distance_to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(img: &[u8], size: u32, x: u32, y: u32) -> u8 {
        img[((y * size + x) * 4 + 3) as usize]
    }

    #[test]
    fn it_is_black_with_the_shape_in_alpha() {
        let size = 36;
        let img = template_rgba(size);
        assert_eq!(img.len(), (size * size * 4) as usize);
        assert!(img.chunks(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0));
        // Pixel centre for a point of the drawing space.
        let at = |x: f64, y: f64| {
            let px = ((x - VIEW_ORIGIN) / VIEW_SIZE * f64::from(size)) as u32;
            let py = ((y - VIEW_ORIGIN) / VIEW_SIZE * f64::from(size)) as u32;
            alpha(&img, size, px, py)
        };
        // The front chevron's tip is solid, the trailing one's is faint, between them is clear.
        assert_eq!(at(15.5, 10.0), 255);
        let faint = at(9.0, 10.0);
        assert!((100..=125).contains(&faint), "{faint}");
        assert_eq!(at(10.0, 2.0), 0);
        assert_eq!(at(1.2, 1.2), 0);
        // Corners are empty, so the mark has room around it.
        assert_eq!(alpha(&img, size, 0, 0), 0);
        assert_eq!(alpha(&img, size, size - 1, size - 1), 0);
    }

    #[test]
    fn edges_are_smooth() {
        let img = template_rgba(36);
        let partial = img.chunks(4).filter(|p| p[3] > 0 && p[3] < 110).count();
        assert!(partial > 20, "anti-aliased edge pixels: {partial}");
    }
}
