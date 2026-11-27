//! The menu bar icon: the two-chevron mark (`icons/icon.svg`, and the `Mark` component in the
//! React side), drawn here as a template image. macOS uses only its alpha and tints it for light
//! and dark menu bars, so it is black with coverage in the alpha. Drawn in code rather than
//! shipped as a PNG so it needs no image decoder and can't drift from the shapes below.
//!
//! Windows doesn't tint tray icons, and taskbars are light or dark, so there the mark is white
//! on a rounded tile of the app's indigo ([`badge_rgba`]): readable on both.

/// The mark's two strokes in its 20 × 20 drawing space, with their opacity: the trailing
/// chevron is fainter, as in the app icon.
const STROKES: [([(f64, f64); 3], f64); 2] = [
    ([(4.0, 5.0), (9.0, 10.0), (4.0, 15.0)], 0.45),
    ([(10.5, 5.0), (15.5, 10.0), (10.5, 15.0)], 1.0),
];
/// Stroke width in drawing units, with round caps and joins.
const STROKE: f64 = 2.6;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
/// The part of the drawing space the image shows: the strokes with a unit of room around them,
/// so the mark fills the menu bar's 18 pt icon height the way other menu bar icons do.
const VIEW_ORIGIN: f64 = 1.0;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const VIEW_SIZE: f64 = 18.0;
/// Samples per pixel along each axis, for smooth edges.
const SAMPLES: u32 = 4;

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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

/// The app's indigo (`--accent` in styles.css, the middle of the icon's gradient).
const INDIGO: [f64; 3] = [70.0, 72.0, 255.0];
/// The tile's corner radius, in drawing units (the tile is the whole 20 × 20 space).
const TILE_RADIUS: f64 = 4.5;

/// Whether a point of the 20 × 20 drawing space is on the rounded tile.
fn on_tile(x: f64, y: f64) -> bool {
    let r = TILE_RADIUS;
    let cx = x.clamp(r, 20.0 - r);
    let cy = y.clamp(r, 20.0 - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// The tray icon for Windows at `size` × `size` pixels, as RGBA (not premultiplied): the mark
/// in white on an indigo tile, the trailing chevron fainter, with smooth edges.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn badge_rgba(size: u32) -> Vec<u8> {
    let scale = 20.0 / f64::from(size);
    let n = f64::from(SAMPLES * SAMPLES);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (mut inside, mut rgb) = (0.0, [0.0; 3]);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (f64::from(px) + (f64::from(sx) + 0.5) / f64::from(SAMPLES)) * scale;
                    let y = (f64::from(py) + (f64::from(sy) + 0.5) / f64::from(SAMPLES)) * scale;
                    if !on_tile(x, y) {
                        continue;
                    }
                    inside += 1.0;
                    let white = alpha_at(x, y);
                    for (c, base) in rgb.iter_mut().zip(INDIGO) {
                        *c += base + (255.0 - base) * white;
                    }
                }
            }
            if inside == 0.0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let [r, g, b] = rgb.map(|c| (c / inside).round() as u8);
                out.extend_from_slice(&[r, g, b, (inside / n * 255.0).round() as u8]);
            }
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
    fn the_windows_icon_is_white_on_indigo() {
        let size = 32;
        let img = badge_rgba(size);
        assert_eq!(img.len(), (size * size * 4) as usize);
        let px = |x: u32, y: u32| &img[((y * size + x) * 4) as usize..][..4];
        // The rounded corner is clear; the tile's edge middle is solid indigo.
        assert_eq!(px(0, 0)[3], 0);
        assert_eq!(px(16, 1), [70, 72, 255, 255]);
        // The front chevron's tip is white; the trailing one's is a lighter indigo.
        let tip = px((15.5 / 20.0 * 32.0) as u32, 16);
        assert!(tip[0] > 240 && tip[1] > 240 && tip[3] == 255, "{tip:?}");
        let trailing = px((9.0 / 20.0 * 32.0) as u32, 16);
        assert!(trailing[0] > 120 && trailing[0] < 220, "{trailing:?}");
        // Opaque wherever the tile is: no see-through middle on a dark or light taskbar.
        assert!((4..28).all(|i| px(i, 16)[3] == 255));
    }

    #[test]
    fn edges_are_smooth() {
        let img = template_rgba(36);
        let partial = img.chunks(4).filter(|p| p[3] > 0 && p[3] < 110).count();
        assert!(partial > 20, "anti-aliased edge pixels: {partial}");
    }
}
