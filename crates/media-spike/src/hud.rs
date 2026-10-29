//! Heads-up display: call quality and network stats drawn over the shared screen, in the
//! top-right corner so it never covers the latency marker (top-left).

use font8x8::legacy::BASIC_LEGACY;
use media::netstats::{Board, Limit, NetSnapshot, Quality};
use media::stats::Summary;

/// Glyphs are 8x8; drawn at 2x so they stay readable on Retina and 1440p frames.
const SCALE: usize = 2;
const GLYPH: usize = 8 * SCALE;
const LINE_GAP: usize = 4;
const PAD: usize = 8;

const WHITE: u32 = 0x00e8_e8e8;
const GREEN: u32 = 0x0040_d060;
const AMBER: u32 = 0x00f0_b030;
const RED: u32 = 0x00f0_4040;
const GREY: u32 = 0x0090_9090;

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub color: u32,
}

impl Line {
    fn new(text: String, color: u32) -> Self {
        Self { text, color }
    }
}

/// Text for the HUD. `rtt` is the tick round trip over the last report window.
pub fn lines(board: &Board, rtt: Option<Summary>) -> Vec<Line> {
    let mut out = Vec::new();
    let (badge, color) = match board.quality() {
        Some(Quality::Good) => ("GOOD", GREEN),
        Some(Quality::Fair) => ("FAIR", AMBER),
        Some(Quality::Poor) => ("POOR", RED),
        None => ("....", GREY),
    };
    let link = |s: &Option<NetSnapshot>| {
        s.as_ref()
            .and_then(|s| s.rtt_ms)
            .map_or("-".to_owned(), |ms| format!("{ms:.0} ms"))
    };
    let path = board
        .local
        .as_ref()
        .and_then(|s| s.path.clone())
        .unwrap_or_default();
    out.push(Line::new(
        format!(
            "{badge}  link RTT you {} / sharer {}  {path}",
            link(&board.local),
            link(&board.remote)
        ),
        color,
    ));
    out.push(Line::new(
        rtt.map_or("round trip: waiting for ticks".to_owned(), |s| {
            format!(
                "round trip p50 {:.0} ms  p95 {:.0} ms  max {:.0} ms",
                s.p50, s.p95, s.max
            )
        }),
        WHITE,
    ));

    match board.local.as_ref().and_then(|s| s.recv.as_ref()) {
        Some(r) => {
            out.push(Line::new(
                format!(
                    "recv {}x{} {:.0} fps {}  loss {:.1}%  jitter {:.0} ms",
                    r.width,
                    r.height,
                    r.fps,
                    mbps(r.kbps),
                    r.loss_pct,
                    r.jitter_ms
                ),
                severity(r.loss_pct, r.jitter_ms),
            ));
            out.push(Line::new(
                format!(
                    "     freezes {} ({:.0} ms)  dropped {}  nack {}  pli {}  {}",
                    r.freezes, r.freeze_ms, r.dropped, r.nacks, r.plis, r.decoder
                ),
                if r.freezes > 0 { AMBER } else { GREY },
            ));
        }
        None => out.push(Line::new("recv: no video yet".to_owned(), GREY)),
    }

    match board
        .remote
        .as_ref()
        .and_then(|s| s.send.as_ref().map(|t| (s, t)))
    {
        Some((s, t)) => {
            let loss = t.loss_pct.unwrap_or(0.0);
            out.push(Line::new(
                format!(
                    "send {}x{} {:.0} fps {} of {}  loss {:.1}%  rtx {:.1}%",
                    t.width,
                    t.height,
                    t.fps,
                    mbps(t.kbps),
                    mbps(t.target_kbps),
                    loss,
                    t.retransmit_pct
                ),
                severity(loss, 0.0),
            ));
            let limit = match t.limit {
                Limit::None => "none",
                Limit::Cpu => "CPU",
                Limit::Bandwidth => "bandwidth",
                Limit::Other => "other",
            };
            out.push(Line::new(
                format!(
                    "     limit {limit}  bandwidth est {}  {}",
                    s.bwe_kbps.map_or("-".to_owned(), mbps),
                    t.encoder
                ),
                if t.limit == Limit::None { GREY } else { AMBER },
            ));
        }
        None => out.push(Line::new("send: waiting for sharer stats".to_owned(), GREY)),
    }
    out
}

fn mbps(kbps: f64) -> String {
    format!("{:.1} Mbps", kbps / 1000.0)
}

fn severity(loss_pct: f64, jitter_ms: f64) -> u32 {
    if loss_pct >= 5.0 || jitter_ms >= 80.0 {
        RED
    } else if loss_pct >= 1.0 || jitter_ms >= 30.0 {
        AMBER
    } else {
        WHITE
    }
}

/// Draws `lines` over a dimmed box at the top-right of a 0RGB pixel buffer.
pub fn draw(pixels: &mut [u32], width: usize, height: usize, lines: &[Line]) {
    let cols = lines.iter().map(|l| l.text.len()).max().unwrap_or(0);
    if cols == 0 || pixels.len() < width * height {
        return;
    }
    let box_w = cols * GLYPH + 2 * PAD;
    let box_h = lines.len() * (GLYPH + LINE_GAP) - LINE_GAP + 2 * PAD;
    let x0 = width.saturating_sub(box_w + PAD);
    let y0 = PAD;
    let (x1, y1) = ((x0 + box_w).min(width), (y0 + box_h).min(height));

    for y in y0..y1 {
        for p in &mut pixels[y * width + x0..y * width + x1] {
            // Quarter brightness keeps the screen faintly visible behind the text.
            *p = (*p >> 2) & 0x003f_3f3f;
        }
    }
    for (i, line) in lines.iter().enumerate() {
        let top = y0 + PAD + i * (GLYPH + LINE_GAP);
        for (j, ch) in line.text.bytes().enumerate() {
            let glyph = BASIC_LEGACY[usize::from(ch).min(127)];
            let left = x0 + PAD + j * GLYPH;
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..8 {
                    // Bit 0 is the leftmost pixel of the row.
                    if bits >> col & 1 == 1 {
                        fill(
                            pixels,
                            width,
                            (x1, y1),
                            left + col * SCALE,
                            top + row * SCALE,
                            line.color,
                        );
                    }
                }
            }
        }
    }
}

fn fill(pixels: &mut [u32], width: usize, (x1, y1): (usize, usize), x: usize, y: usize, c: u32) {
    for yy in y..(y + SCALE).min(y1) {
        for xx in x..(x + SCALE).min(x1) {
            pixels[yy * width + xx] = c;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use media::netstats::{RecvStats, SendStats};

    #[test]
    fn draws_in_top_right_and_leaves_rest_untouched() {
        let (w, h) = (400, 100);
        let mut px = vec![0x00ff_ffff; w * h];
        draw(&mut px, w, h, &[Line::new("L".into(), 0x0000_00ff)]);
        // Top-left corner (marker area) is untouched.
        assert_eq!(px[0], 0x00ff_ffff);
        // The box is dimmed white, and 'L' has its vertical stroke at its left edge.
        let x0 = w - (GLYPH + 2 * PAD) - PAD;
        assert_eq!(px[PAD * w + x0], 0x003f_3f3f);
        let stroke = (PAD + PAD) * w + x0 + PAD;
        assert_eq!(px[stroke], 0x0000_00ff);
    }

    #[test]
    fn survives_tiny_frames() {
        let mut px = vec![0; 10 * 10];
        draw(
            &mut px,
            10,
            10,
            &[Line::new("long line of text".into(), WHITE)],
        );
    }

    #[test]
    fn lines_show_both_sides() {
        let board = Board {
            local: Some(NetSnapshot {
                rtt_ms: Some(3.0),
                path: Some("host udp".into()),
                recv: Some(RecvStats {
                    width: 1660,
                    height: 1080,
                    fps: 58.0,
                    kbps: 2100.0,
                    loss_pct: 2.0,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            remote: Some(NetSnapshot {
                rtt_ms: Some(2.0),
                bwe_kbps: Some(8100.0),
                send: Some(SendStats {
                    width: 1660,
                    height: 1080,
                    fps: 58.0,
                    kbps: 2300.0,
                    target_kbps: 4000.0,
                    limit: Limit::Bandwidth,
                    encoder: "libvpx".into(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            history: Vec::new(),
        };
        let l = lines(&board, None);
        assert!(
            l[0].text.starts_with("FAIR"),
            "2% loss is fair: {}",
            l[0].text
        );
        assert!(l[0].text.contains("you 3 ms / sharer 2 ms  host udp"));
        assert!(l[2].text.contains("1660x1080 58 fps 2.1 Mbps  loss 2.0%"));
        assert_eq!(l[2].color, AMBER);
        assert!(l[4].text.contains("2.3 Mbps of 4.0 Mbps"));
        assert!(l[5]
            .text
            .contains("limit bandwidth  bandwidth est 8.1 Mbps  libvpx"));
        assert!(l.iter().all(|l| l.text.is_ascii()));
    }

    #[test]
    fn empty_board_says_waiting() {
        let l = lines(&Board::default(), None);
        assert!(l[0].text.starts_with("...."));
        assert!(l.iter().any(|l| l.text.contains("waiting for sharer")));
    }
}
