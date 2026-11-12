//! Heads-up display: call quality and network stats drawn over the shared screen, in the
//! top-right corner so it never covers the latency marker (top-left).

use font8x8::legacy::BASIC_LEGACY;
use media::audio::VoiceStatus;
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

/// The voices, as the HUD shows them.
#[derive(Debug, Clone, Default)]
pub struct Voice {
    pub status: VoiceStatus,
    /// The sharer's voice, from its mic to our speakers, when a run measures it.
    pub one_way_ms: Option<f64>,
}

/// The voice line: our mic, the sharer's voice, and how it arrives. `None` without any voice.
fn voice_line(board: &Board, voice: &Voice) -> Option<Line> {
    let local = board.local.as_ref();
    let recv = local.and_then(|s| s.audio_recv.as_ref());
    let status = &voice.status;
    if !status.sending && status.remote_muted.is_none() && recv.is_none() {
        return None;
    }
    let level = |db: Option<f64>| db.map_or("on".to_owned(), |db| format!("{db:.0} dB"));
    let mic = if !status.sending {
        "off".to_owned()
    } else if status.muted {
        "MUTED".to_owned()
    } else {
        level(local.and_then(|s| s.audio_send_db))
    };
    let sharer = match status.remote_muted {
        None => "no voice".to_owned(),
        Some(true) => "muted".to_owned(),
        Some(false) => level(recv.and_then(|r| r.level_db)),
    };
    let mut text = format!("voice mic {mic}  sharer {sharer}");
    let mut color = if status.muted { AMBER } else { WHITE };
    // Nothing arrives from a muted sharer, so there is nothing to say about how it arrives.
    if let Some(r) = recv.filter(|_| status.remote_muted != Some(true)) {
        if let Some(ms) = r.jitter_buffer_ms {
            text += &format!("  buffer {ms:.0} ms");
        }
        text += &format!("  made up {:.1}%", r.concealed_pct);
        if r.concealed_pct >= 5.0 {
            color = RED;
        } else if r.concealed_pct >= 1.0 {
            color = AMBER;
        }
    }
    if let Some(ms) = voice.one_way_ms {
        text += &format!("  one-way {ms:.0} ms");
    }
    Some(Line::new(text, color))
}

/// Text for the HUD. `rtt` is the tick round trip over the last report window.
pub fn lines(board: &Board, rtt: Option<Summary>, voice: &Voice) -> Vec<Line> {
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
            if let Some(note) = &s.adapt {
                out.push(Line::new(format!("     adapted to {note}"), AMBER));
            }
        }
        None => out.push(Line::new("send: waiting for sharer stats".to_owned(), GREY)),
    }
    out.extend(voice_line(board, voice));
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
    use media::netstats::{AudioRecvStats, RecvStats, SendStats};

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
        let l = lines(&board, None, &Voice::default());
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
        assert!(l.iter().all(|l| !l.text.starts_with("voice")), "no voice");
    }

    fn voice_board(send_db: Option<f64>, recv: Option<AudioRecvStats>) -> Board {
        Board {
            local: Some(NetSnapshot {
                audio_send_db: send_db,
                audio_recv: recv,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn voice_line_shows_levels_and_how_the_voice_arrives() {
        let board = voice_board(
            Some(-31.6),
            Some(AudioRecvStats {
                concealed_pct: 0.26,
                jitter_buffer_ms: Some(44.4),
                level_db: Some(-27.8),
                ..Default::default()
            }),
        );
        let voice = Voice {
            status: VoiceStatus {
                sending: true,
                remote_muted: Some(false),
                ..Default::default()
            },
            one_way_ms: Some(48.2),
        };
        let line = lines(&board, None, &voice).pop().unwrap();
        assert_eq!(
            line.text,
            "voice mic -32 dB  sharer -28 dB  buffer 44 ms  made up 0.3%  one-way 48 ms"
        );
        assert_eq!(line.color, WHITE);
        assert!(line.text.is_ascii());
    }

    #[test]
    fn voice_line_says_who_is_muted() {
        let mut voice = Voice {
            status: VoiceStatus {
                sending: true,
                muted: true,
                remote_muted: Some(true),
                ..Default::default()
            },
            one_way_ms: None,
        };
        let board = voice_board(Some(-100.0), None);
        let line = voice_line(&board, &voice).unwrap();
        assert_eq!(line.text, "voice mic MUTED  sharer muted");
        assert_eq!(line.color, AMBER);

        // Listening only: no mic of our own, and the sharer hasn't sent a level yet.
        voice.status = VoiceStatus {
            remote_muted: Some(false),
            ..Default::default()
        };
        let line = voice_line(&voice_board(None, None), &voice).unwrap();
        assert_eq!(line.text, "voice mic off  sharer on");
    }

    #[test]
    fn voice_line_warns_when_sound_is_made_up() {
        let recv = |pct| {
            Some(AudioRecvStats {
                concealed_pct: pct,
                ..Default::default()
            })
        };
        let voice = Voice::default();
        let color = |pct| {
            voice_line(&voice_board(None, recv(pct)), &voice)
                .unwrap()
                .color
        };
        assert_eq!(color(0.5), WHITE);
        assert_eq!(color(2.0), AMBER);
        assert_eq!(color(7.0), RED);
    }

    #[test]
    fn empty_board_says_waiting() {
        let l = lines(&Board::default(), None, &Voice::default());
        assert!(l[0].text.starts_with("...."));
        assert!(l.iter().any(|l| l.text.contains("waiting for sharer")));
    }
}
