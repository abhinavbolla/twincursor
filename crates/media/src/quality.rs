//! Objective picture quality on the luma plane: SSIM (structure, what blurry or blocky text
//! loses) and PSNR (raw error). Used with the synthetic pattern and with snapshots.

use serde::Serialize;

/// SSIM window. Non-overlapping 8x8 blocks, as in libvpx's fast SSIM.
const WIN: usize = 8;
const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
/// PSNR reported for identical images, so results stay finite (and JSON-friendly).
pub const MAX_PSNR_DB: f64 = 100.0;

/// A luma plane: `stride` bytes per row.
#[derive(Debug, Clone, Copy)]
pub struct Plane<'a> {
    pub data: &'a [u8],
    pub stride: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Scores {
    pub ssim: f64,
    pub psnr_db: f64,
}

/// Compares two `width` x `height` planes, ignoring blocks that overlap the top-left `skip`
/// area (width, height), e.g. the markers. `None` if nothing is left to compare.
pub fn compare(
    a: Plane,
    b: Plane,
    width: usize,
    height: usize,
    skip: (usize, usize),
) -> Option<Scores> {
    let (mut ssim_sum, mut blocks) = (0.0, 0usize);
    let (mut sq_err, mut pixels) = (0.0, 0usize);
    for by in (0..height.saturating_sub(WIN - 1)).step_by(WIN) {
        for bx in (0..width.saturating_sub(WIN - 1)).step_by(WIN) {
            if bx < skip.0 && by < skip.1 {
                continue;
            }
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for row in by..by + WIN {
                let ra = &a.data[row * a.stride + bx..row * a.stride + bx + WIN];
                let rb = &b.data[row * b.stride + bx..row * b.stride + bx + WIN];
                for (&pa, &pb) in ra.iter().zip(rb) {
                    let (fa, fb) = (f64::from(pa), f64::from(pb));
                    sa += fa;
                    sb += fb;
                    saa += fa * fa;
                    sbb += fb * fb;
                    sab += fa * fb;
                    sq_err += (fa - fb) * (fa - fb);
                }
            }
            let n = (WIN * WIN) as f64;
            let (ma, mb) = (sa / n, sb / n);
            let (va, vb) = (saa / n - ma * ma, sbb / n - mb * mb);
            let cov = sab / n - ma * mb;
            ssim_sum += ((2.0 * ma * mb + C1) * (2.0 * cov + C2))
                / ((ma * ma + mb * mb + C1) * (va + vb + C2));
            blocks += 1;
            pixels += WIN * WIN;
        }
    }
    if blocks == 0 {
        return None;
    }
    let mse = sq_err / pixels as f64;
    let psnr_db = if mse == 0.0 {
        MAX_PSNR_DB
    } else {
        (10.0 * (255.0 * 255.0 / mse).log10()).min(MAX_PSNR_DB)
    };
    Some(Scores {
        ssim: ssim_sum / blocks as f64,
        psnr_db,
    })
}

/// Summary of quality samples, where the low end matters (the worst frames).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LowSummary {
    pub count: usize,
    pub mean: f64,
    pub p50: f64,
    /// 5th percentile: 95% of frames were at least this good.
    pub p5: f64,
    pub min: f64,
}

pub fn summarize_low(samples: &[f64]) -> Option<LowSummary> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |p: f64| {
        sorted[((p / 100.0 * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1]
    };
    Some(LowSummary {
        count: sorted.len(),
        mean: sorted.iter().sum::<f64>() / sorted.len() as f64,
        p50: at(50.0),
        p5: at(5.0),
        min: sorted[0],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 128;
    const H: usize = 64;

    /// Stripes 2 px wide, like thin text strokes.
    fn stripes() -> Vec<u8> {
        (0..W * H)
            .map(|i| {
                if (i % W / 2).is_multiple_of(2) {
                    30
                } else {
                    220
                }
            })
            .collect()
    }

    fn plane(v: &[u8]) -> Plane<'_> {
        Plane { data: v, stride: W }
    }

    #[test]
    fn identical_is_perfect() {
        let a = stripes();
        let s = compare(plane(&a), plane(&a), W, H, (0, 0)).unwrap();
        assert!((s.ssim - 1.0).abs() < 1e-12);
        assert_eq!(s.psnr_db, MAX_PSNR_DB);
    }

    #[test]
    fn blur_hurts_ssim_more_than_a_brightness_shift() {
        let a = stripes();
        let blurred: Vec<u8> = (0..a.len())
            .map(|i| {
                let l = u32::from(a[i.saturating_sub(1)]);
                let r = u32::from(a[(i + 1).min(a.len() - 1)]);
                ((l + 2 * u32::from(a[i]) + r) / 4) as u8
            })
            .collect();
        let brighter: Vec<u8> = a.iter().map(|&v| v.saturating_add(10)).collect();
        let blur = compare(plane(&a), plane(&blurred), W, H, (0, 0)).unwrap();
        let shift = compare(plane(&a), plane(&brighter), W, H, (0, 0)).unwrap();
        assert!(blur.ssim < 0.9, "blur {blur:?}");
        assert!(shift.ssim > 0.99, "shift {shift:?}");
        assert!(blur.ssim < shift.ssim);
    }

    #[test]
    fn skipped_area_is_ignored() {
        let a = stripes();
        let mut b = a.clone();
        for row in 0..16 {
            b[row * W..row * W + 32].fill(0);
        }
        let s = compare(plane(&a), plane(&b), W, H, (32, 16)).unwrap();
        assert!((s.ssim - 1.0).abs() < 1e-12);
        let s = compare(plane(&a), plane(&b), W, H, (0, 0)).unwrap();
        assert!(s.ssim < 1.0);
    }

    #[test]
    fn respects_stride() {
        let a = stripes();
        let mut padded = vec![0u8; (W + 16) * H];
        for row in 0..H {
            padded[row * (W + 16)..row * (W + 16) + W].copy_from_slice(&a[row * W..(row + 1) * W]);
        }
        let p = Plane {
            data: &padded,
            stride: W + 16,
        };
        let s = compare(plane(&a), p, W, H, (0, 0)).unwrap();
        assert!((s.ssim - 1.0).abs() < 1e-12);
    }

    #[test]
    fn too_small_or_all_skipped_is_none() {
        let a = vec![0u8; 4 * 4];
        let p = Plane {
            data: &a,
            stride: 4,
        };
        assert_eq!(compare(p, p, 4, 4, (0, 0)), None);
        let b = stripes();
        assert_eq!(compare(plane(&b), plane(&b), W, H, (W, H)), None);
    }

    #[test]
    fn psnr_of_known_error() {
        let a = vec![100u8; W * H];
        let b = vec![110u8; W * H];
        let s = compare(plane(&a), plane(&b), W, H, (0, 0)).unwrap();
        // MSE 100 → 10·log10(65025/100) ≈ 28.13 dB.
        assert!((s.psnr_db - 28.13).abs() < 0.01, "{}", s.psnr_db);
    }

    #[test]
    fn low_summary() {
        let samples: Vec<f64> = (1..=100).map(|i| f64::from(i) / 100.0).collect();
        let s = summarize_low(&samples).unwrap();
        assert_eq!((s.p5, s.p50, s.min), (0.05, 0.5, 0.01));
        assert_eq!(summarize_low(&[]), None);
    }
}

/// Periodic luma snapshots for comparing a real screen across the call: the sharer saves the
/// frame it stamps with tick `id`, the viewer the frame it decodes with that marker, both when
/// `id % every == 0`. Compare a pair with `tc-spike compare`.
#[derive(Debug, Clone)]
pub struct Snapshots {
    pub every: u32,
    pub dir: std::path::PathBuf,
}

impl Snapshots {
    pub fn wants(&self, id: u32) -> bool {
        self.every > 0 && id.is_multiple_of(self.every)
    }

    /// Saves a luma plane as `<dir>/<side>-<id>.pgm` on a blocking thread.
    pub fn save(&self, side: &'static str, id: u32, y: &[u8], stride: usize, w: usize, h: usize) {
        let mut tight = Vec::with_capacity(w * h);
        for row in 0..h {
            tight.extend_from_slice(&y[row * stride..row * stride + w]);
        }
        let path = self.dir.join(format!("{side}-{id}.pgm"));
        let dir = self.dir.clone();
        std::thread::spawn(move || {
            let result =
                std::fs::create_dir_all(&dir).and_then(|()| write_pgm(&path, &tight, w, h));
            if let Err(e) = result {
                tracing::warn!("snapshot {} not saved: {e}", path.display());
            }
        });
    }
}

/// Writes a tightly packed 8-bit plane as a binary PGM.
pub fn write_pgm(path: &std::path::Path, data: &[u8], w: usize, h: usize) -> std::io::Result<()> {
    let mut out = format!("P5\n{w} {h}\n255\n").into_bytes();
    out.extend_from_slice(&data[..w * h]);
    std::fs::write(path, out)
}

/// Reads a binary 8-bit PGM written by [`write_pgm`]: (width, height, pixels).
pub fn read_pgm(path: &std::path::Path) -> std::io::Result<(usize, usize, Vec<u8>)> {
    let bytes = std::fs::read(path)?;
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not an 8-bit binary PGM");
    let mut fields = Vec::new();
    let mut pos = 0;
    while fields.len() < 4 {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let start = pos;
        while pos < bytes.len() && !bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if start == pos {
            return Err(bad());
        }
        fields.push(
            std::str::from_utf8(&bytes[start..pos])
                .map_err(|_| bad())?
                .to_owned(),
        );
    }
    pos += 1;
    let num = |s: &str| s.parse::<usize>().map_err(|_| bad());
    let (w, h) = (num(&fields[1])?, num(&fields[2])?);
    if fields[0] != "P5" || fields[3] != "255" || bytes.len() < pos + w * h {
        return Err(bad());
    }
    Ok((w, h, bytes[pos..pos + w * h].to_vec()))
}

#[cfg(test)]
mod pgm_tests {
    use super::*;

    #[test]
    fn pgm_round_trip() {
        let dir = std::env::temp_dir().join(format!("tc-pgm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.pgm");
        let data: Vec<u8> = (0..=255).cycle().take(20 * 10).collect();
        write_pgm(&path, &data, 20, 10).unwrap();
        assert_eq!(read_pgm(&path).unwrap(), (20, 10, data));
        std::fs::write(&path, b"P6\n1 1\n255\nxyz").unwrap();
        assert!(read_pgm(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn snapshot_cadence() {
        let s = Snapshots {
            every: 40,
            dir: "x".into(),
        };
        assert!(s.wants(0) && s.wants(80) && !s.wants(41));
        assert!(!Snapshots {
            every: 0,
            dir: "x".into()
        }
        .wants(0));
    }
}
