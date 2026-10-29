//! Synthetic test screen: a dark-theme code editor drawn from the frame number alone.
//!
//! The viewer regenerates the exact frame the sharer sent (the frame number travels in the
//! second marker slot) and compares it with what it decoded, which measures text quality
//! objectively and on any OS. The content is chosen to be hard for screen codecs: tiny 8 px
//! text in the left pane (like small fonts after Retina downscaling), 16 px text on the
//! right, coloured syntax, a pane that scrolls, a line being typed and a blinking cursor.

use font8x8::legacy::BASIC_LEGACY;
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::I420Buffer;

use crate::marker::{self, BLOCK, COLS, FRAME_SLOT_X, MARKED_AREA};

const BG: u32 = 0x001e_1e1e;
const BAR: u32 = 0x0025_2526;
const GUTTER: u32 = 0x0085_8585;
const TEXT: u32 = 0x00d4_d4d4;
const KEYWORD: u32 = 0x0056_9cd6;
const STRING: u32 = 0x00ce_9178;
const COMMENT: u32 = 0x006a_9955;
const NUMBER: u32 = 0x00b5_cea8;
const CURSOR: u32 = 0x00ae_afad;
const DIVIDER: u32 = 0x0040_4040;

/// The left pane alternates gentle scrolling (reading) with fast scrolling (one line per frame,
/// the hardest case for a screen codec), [`PHASE`] frames each: 10 s at 60 fps.
const PHASE: u32 = 600;
/// Frames per line while scrolling gently (4 lines a second at 60 fps).
const SCROLL_EVERY: u32 = 15;
/// Frames per character typed on the right.
const TYPE_EVERY: u32 = 4;
/// Frames per cursor blink phase.
const BLINK_EVERY: u32 = 30;

const KEYWORDS: &[&str] = &[
    "fn", "let", "pub", "use", "struct", "impl", "match", "if", "else", "return", "for", "in",
    "mut", "const", "Some", "None", "Ok", "self", "Self", "while", "loop", "enum", "mod",
];

/// Our own sample code, so the pattern has realistic token shapes.
const CODE: &[&str] = &[
    "// Sharer: publishes captured frames and stamps latency markers.",
    "use std::time::{Duration, Instant};",
    "",
    "pub struct Session {",
    "    room: Room,",
    "    started: Instant,",
    "    frames_sent: u64,",
    "}",
    "",
    "impl Session {",
    "    pub fn new(room: Room) -> Self {",
    "        Self { room, started: Instant::now(), frames_sent: 0 }",
    "    }",
    "",
    "    /// Sends one frame, unless the encoder is still busy.",
    "    pub fn send(&mut self, frame: &Frame) -> Result<(), Error> {",
    "        if frame.width % 16 != 0 {",
    "            return Err(Error::Size(frame.width, frame.height));",
    "        }",
    "        let elapsed = self.started.elapsed().as_micros() as i64;",
    "        self.room.publish(frame, elapsed)?;",
    "        self.frames_sent += 1;",
    "        Ok(())",
    "    }",
    "",
    "    pub fn fps(&self) -> f64 {",
    "        let secs = self.started.elapsed().as_secs_f64().max(0.001);",
    "        self.frames_sent as f64 / secs",
    "    }",
    "}",
    "",
    "fn grade(rtt_ms: f64, loss_pct: f64) -> &'static str {",
    "    match (rtt_ms, loss_pct) {",
    "        (r, l) if r < 80.0 && l < 1.0 => \"good\",",
    "        (r, l) if r < 200.0 && l < 5.0 => \"fair\",",
    "        _ => \"poor\",",
    "    }",
    "}",
    "",
    "const RETRIES: [u32; 4] = [50, 100, 200, 400];",
    "",
    "for (attempt, delay) in RETRIES.iter().enumerate() {",
    "    if let Ok(room) = connect(\"wss://pair.example/room/42\") {",
    "        log(\"connected after {} attempts\", attempt + 1);",
    "        break;",
    "    }",
    "    sleep(Duration::from_millis(*delay as u64));",
    "}",
];

/// Renders frames of a given size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pattern {
    pub width: u32,
    pub height: u32,
}

impl Pattern {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Renders frame `n` as 0RGB pixels (`width * height`).
    pub fn render_rgb(&self, n: u32, px: &mut [u32]) {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut c = Canvas { px, w, h };
        c.fill(0, 0, w, h, BG);

        // Top bar with the frame number in the second marker slot. The sharer stamps the
        // latency marker into the first slot later.
        let bar_h = MARKED_AREA.1;
        c.fill(0, 0, w, bar_h, BAR);
        for (i, bit) in marker::bits(n).enumerate() {
            let level = u32::from(if bit {
                marker::LEVELS.0
            } else {
                marker::LEVELS.1
            });
            // Limited-range luma 16..235 corresponds to RGB 0..255 for grey.
            let v = ((level - 16) * 255 / 219).min(255);
            let grey = v << 16 | v << 8 | v;
            c.fill(
                FRAME_SLOT_X + (i % COLS) * BLOCK,
                (i / COLS) * BLOCK,
                BLOCK,
                BLOCK,
                grey,
            );
        }
        c.text(MARKED_AREA.0 + 16, 24, "twincursor test pattern", TEXT, 2);

        // Left pane: 8 px code that scrolls.
        let top = bar_h + 8;
        let split = w * 3 / 5;
        let small_line = 10;
        let first = scroll_offset(n) as usize;
        let rows = h.saturating_sub(top) / small_line;
        for row in 0..rows {
            let idx = (first + row) % CODE.len();
            let y = top + row * small_line;
            c.text(4, y, &format!("{:>4}", idx + 1), GUTTER, 1);
            c.code(48, y, CODE[idx], 1, split - 56);
        }
        c.fill(split, bar_h, 2, h.saturating_sub(bar_h), DIVIDER);

        // Right pane: 16 px code that stays put, a line being typed and a cursor.
        let x = split + 16;
        let big_line = 20;
        let max_w = w.saturating_sub(x + 8);
        let rows = h.saturating_sub(top) / big_line;
        let fixed = rows.saturating_sub(1).min(CODE.len());
        for (row, line) in CODE.iter().take(fixed).enumerate() {
            c.code(x, top + row * big_line, line, 2, max_w);
        }
        if rows > 0 {
            let typed_line = CODE[(n / (TYPE_EVERY * 64)) as usize % CODE.len()];
            let typed = ((n / TYPE_EVERY) % 64) as usize;
            let shown: String = typed_line.chars().take(typed).collect();
            let y = top + (rows - 1) * big_line;
            c.code(x, y, &shown, 2, max_w);
            if (n / BLINK_EVERY).is_multiple_of(2) {
                let cx = x + shown.len() * 16;
                if cx + 2 < w {
                    c.fill(cx, y, 2, 16, CURSOR);
                }
            }
        }
    }

    /// Renders frame `n` and converts it to I420 the same way the capture path does.
    pub fn render_i420(&self, n: u32) -> I420Buffer {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut px = vec![0u32; w * h];
        self.render_rgb(n, &mut px);
        // SAFETY: reinterpreting initialized u32s as bytes; alignment only gets looser.
        let bytes = unsafe { std::slice::from_raw_parts(px.as_ptr().cast::<u8>(), px.len() * 4) };
        let mut out = I420Buffer::new(self.width, self.height);
        let (sy, su, sv) = out.strides();
        let (y, u, v) = out.data_mut();
        // 0RGB u32s are B,G,R,X bytes on little-endian: what libyuv calls ARGB.
        yuv_helper::argb_to_i420(
            bytes,
            self.width * 4,
            y,
            sy,
            u,
            su,
            v,
            sv,
            self.width as i32,
            self.height as i32,
        );
        out
    }
}

/// Lines the left pane has scrolled by frame `n`.
fn scroll_offset(n: u32) -> u32 {
    let per_cycle = PHASE / SCROLL_EVERY + PHASE;
    let (cycles, r) = (n / (2 * PHASE), n % (2 * PHASE));
    let within = if r < PHASE {
        r / SCROLL_EVERY
    } else {
        PHASE / SCROLL_EVERY + (r - PHASE)
    };
    cycles.wrapping_mul(per_cycle).wrapping_add(within)
}

/// Whether frame `n` is in a fast-scrolling phase.
pub fn fast_scrolling(n: u32) -> bool {
    n % (2 * PHASE) >= PHASE
}

struct Canvas<'a> {
    px: &'a mut [u32],
    w: usize,
    h: usize,
}

impl Canvas<'_> {
    fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let (x1, y1) = ((x + w).min(self.w), (y + h).min(self.h));
        for row in y.min(y1)..y1 {
            self.px[row * self.w + x.min(x1)..row * self.w + x1].fill(color);
        }
    }

    fn glyph(&mut self, x: usize, y: usize, ch: u8, color: u32, scale: usize) {
        let glyph = BASIC_LEGACY[usize::from(ch).min(127)];
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..8 {
                if bits >> col & 1 == 1 {
                    self.fill(x + col * scale, y + row * scale, scale, scale, color);
                }
            }
        }
    }

    fn text(&mut self, x: usize, y: usize, text: &str, color: u32, scale: usize) {
        for (i, ch) in text.bytes().enumerate() {
            self.glyph(x + i * 8 * scale, y, ch, color, scale);
        }
    }

    /// Draws a line of code with simple syntax colours, clipped to `max_w` pixels.
    fn code(&mut self, x: usize, y: usize, line: &str, scale: usize, max_w: usize) {
        let max_chars = max_w / (8 * scale);
        let mut col = 0;
        for (token, color) in tokens(line) {
            for ch in token.bytes() {
                if col >= max_chars {
                    return;
                }
                if ch != b' ' {
                    self.glyph(x + col * 8 * scale, y, ch, color, scale);
                }
                col += 1;
            }
        }
    }
}

/// Splits a line into coloured tokens: comments, strings, numbers, keywords, the rest.
fn tokens(line: &str) -> Vec<(&str, u32)> {
    let mut out = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let (len, color) = if rest.starts_with("//") {
            (rest.len(), COMMENT)
        } else if let Some(body) = rest.strip_prefix('"') {
            (body.find('"').map_or(rest.len(), |e| e + 2), STRING)
        } else if rest.starts_with(|c: char| c.is_ascii_digit()) {
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '_')
                .unwrap_or(rest.len());
            (end, NUMBER)
        } else if rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            let color = if KEYWORDS.contains(&&rest[..end]) {
                KEYWORD
            } else {
                TEXT
            };
            (end, color)
        } else {
            (1, TEXT)
        };
        out.push((&rest[..len], color));
        rest = &rest[len..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use livekit::webrtc::video_frame::VideoBuffer;

    fn rgb(p: Pattern, n: u32) -> Vec<u32> {
        let mut px = vec![0; (p.width * p.height) as usize];
        p.render_rgb(n, &mut px);
        px
    }

    #[test]
    fn same_frame_renders_identically() {
        let p = Pattern::new(1664, 1080);
        assert_eq!(rgb(p, 1234), rgb(p, 1234));
    }

    #[test]
    fn frames_change_over_time() {
        let p = Pattern::new(1664, 1080);
        assert_ne!(rgb(p, 0), rgb(p, SCROLL_EVERY), "left pane scrolls");
    }

    #[test]
    fn frame_number_survives_conversion() {
        let p = Pattern::new(1104, 720);
        for n in [0, 1, 777, marker::MAX_ID] {
            let frame = p.render_i420(n);
            let (sy, _, _) = frame.strides();
            let read = marker::read_luma_at(
                frame.data().0,
                sy as usize,
                frame.width() as usize,
                frame.height() as usize,
                FRAME_SLOT_X,
            );
            assert_eq!(read, Some(n));
        }
    }

    #[test]
    fn has_lots_of_small_text() {
        // Roughly: a busy editor is ~10-25% "ink" in the text area.
        let p = Pattern::new(1664, 1080);
        let px = rgb(p, 0);
        let ink = px.iter().filter(|&&c| c != BG && c != BAR).count();
        let share = ink as f64 / px.len() as f64;
        assert!((0.05..0.5).contains(&share), "ink share {share}");
    }

    #[test]
    fn scrolling_alternates_and_never_jumps() {
        assert_eq!(scroll_offset(0), 0);
        assert_eq!(scroll_offset(SCROLL_EVERY), 1);
        assert!(!fast_scrolling(PHASE - 1) && fast_scrolling(PHASE));
        for n in 0..5 * PHASE {
            let step = scroll_offset(n + 1) - scroll_offset(n);
            assert!(step <= 1, "frame {n} jumps {step} lines");
            if fast_scrolling(n) && fast_scrolling(n + 1) {
                assert_eq!(step, 1);
            }
        }
    }

    #[test]
    fn tokenizer_colours() {
        let t = tokens("let x = \"hi\"; // done 42");
        assert_eq!(t[0], ("let", KEYWORD));
        assert!(t.contains(&("\"hi\"", STRING)));
        assert_eq!(t.last(), Some(&("// done 42", COMMENT)));
        let t = tokens("[50, 100]");
        assert!(t.contains(&("50", NUMBER)));
    }

    #[test]
    fn tiny_frames_do_not_panic() {
        let p = Pattern::new(16, 2);
        let _ = rgb(p, 5);
    }
}
