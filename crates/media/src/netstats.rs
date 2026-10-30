//! Network and media statistics for the HUD: RTT, loss, jitter, bitrate, bandwidth estimate,
//! what limits quality, and freezes. Polled from WebRTC once a second.
//!
//! WebRTC reports cumulative counters. [`collect`] picks the counters we care about out of a
//! stats report, and [`snapshot`] turns two consecutive collections into per-interval rates.
//! Both are pure, so they're unit-tested without a connection.

use std::time::Instant;

use livekit::prelude::Room;
use livekit::webrtc::stats::{self as rtc, QualityLimitationReason, RtcStats};
use serde::{Deserialize, Serialize};

/// Cumulative counters from one stats report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Counters {
    pub inbound: Option<Inbound>,
    pub outbound: Option<Outbound>,
    pub path: Option<Path>,
}

/// Receiving side of the video stream.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inbound {
    pub packets_received: u64,
    pub packets_lost: i64,
    pub bytes_received: u64,
    pub jitter_s: f64,
    pub frames_decoded: u32,
    pub frames_dropped: u32,
    pub freeze_count: u32,
    pub total_freeze_s: f64,
    pub nack_count: u32,
    pub pli_count: u32,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub decoder: String,
}

/// Sending side of the video stream, plus what the SFU reports back about it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outbound {
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub retransmitted_packets_sent: u64,
    pub target_bitrate: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub limit: Limit,
    pub encoder: String,
    pub nack_count: u32,
    pub pli_count: u32,
    /// Fraction (0..1) of our packets the SFU lost, from its last receiver report.
    pub remote_fraction_lost: Option<f64>,
}

/// The ICE candidate pair in use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    pub rtt_s: Option<f64>,
    pub available_outgoing_bps: Option<f64>,
    /// Local candidate type and protocol, e.g. "host udp" or "relay tcp".
    pub kind: String,
}

/// Why the encoder isn't sending at full quality.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Limit {
    #[default]
    None,
    Cpu,
    Bandwidth,
    Other,
}

impl From<QualityLimitationReason> for Limit {
    fn from(reason: QualityLimitationReason) -> Self {
        match reason {
            QualityLimitationReason::None => Self::None,
            QualityLimitationReason::Cpu => Self::Cpu,
            QualityLimitationReason::Bandwidth => Self::Bandwidth,
            QualityLimitationReason::Other => Self::Other,
        }
    }
}

/// Per-interval view of one peer connection. Sent to the other side as JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NetSnapshot {
    pub rtt_ms: Option<f64>,
    /// Bandwidth estimate for what we send.
    pub bwe_kbps: Option<f64>,
    pub path: Option<String>,
    pub send: Option<SendStats>,
    pub recv: Option<RecvStats>,
    /// Sharer only: the auto-throttle's current tier and why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapt: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SendStats {
    pub kbps: f64,
    pub target_kbps: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub limit: Limit,
    pub encoder: String,
    pub loss_pct: Option<f64>,
    pub retransmit_pct: f64,
    pub nacks: u32,
    pub plis: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecvStats {
    pub kbps: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub loss_pct: f64,
    pub jitter_ms: f64,
    pub freezes: u32,
    /// Often 0 even when `freezes` > 0: libwebrtc doesn't fill `totalFreezesDuration` on
    /// the builds we use. Rely on `freezes`.
    pub freeze_ms: f64,
    pub dropped: u32,
    pub nacks: u32,
    pub plis: u32,
    pub decoder: String,
}

/// Picks the video stream and active candidate pair out of a stats report.
pub fn collect(stats: &[RtcStats]) -> Counters {
    let inbound = stats
        .iter()
        .filter_map(|s| match s {
            RtcStats::InboundRtp(s) if s.stream.kind == "video" => Some(s),
            _ => None,
        })
        .max_by_key(|s| s.inbound.bytes_received);
    let outbound = stats
        .iter()
        .filter_map(|s| match s {
            RtcStats::OutboundRtp(s) if s.stream.kind == "video" => Some(s),
            _ => None,
        })
        .max_by_key(|s| s.sent.bytes_sent);

    Counters {
        inbound: inbound.map(|s| Inbound {
            packets_received: s.received.packets_received,
            packets_lost: s.received.packets_lost,
            bytes_received: s.inbound.bytes_received,
            jitter_s: s.received.jitter,
            frames_decoded: s.inbound.frames_decoded,
            frames_dropped: s.inbound.frames_dropped,
            freeze_count: s.inbound.freeze_count,
            total_freeze_s: s.inbound.total_freeze_duration,
            nack_count: s.inbound.nack_count,
            pli_count: s.inbound.pli_count,
            width: s.inbound.frame_width,
            height: s.inbound.frame_height,
            fps: s.inbound.frames_per_second,
            decoder: s.inbound.decoder_implementation.clone(),
        }),
        outbound: outbound.map(|s| Outbound {
            packets_sent: s.sent.packets_sent,
            bytes_sent: s.sent.bytes_sent,
            retransmitted_packets_sent: s.outbound.retransmitted_packets_sent,
            target_bitrate: s.outbound.target_bitrate,
            width: s.outbound.frame_width,
            height: s.outbound.frame_height,
            fps: s.outbound.frames_per_second,
            limit: s.outbound.quality_limitation_reason.into(),
            encoder: s.outbound.encoder_implementation.clone(),
            nack_count: s.outbound.nack_count,
            pli_count: s.outbound.pli_count,
            remote_fraction_lost: stats.iter().find_map(|r| match r {
                RtcStats::RemoteInboundRtp(r) if r.remote_inbound.local_id == s.rtc.id => {
                    Some(r.remote_inbound.fraction_lost)
                }
                _ => None,
            }),
        }),
        path: active_pair(stats).map(|pair| Path {
            // On localhost the RTT rounds to 0, which is a real measurement once STUN
            // responses have come back.
            rtt_s: (pair.responses_received > 0).then_some(pair.current_round_trip_time),
            available_outgoing_bps: positive(pair.available_outgoing_bitrate),
            kind: local_candidate_kind(stats, &pair.local_candidate_id),
        }),
    }
}

/// The transport's selected pair, or else the nominated pair carrying the most traffic.
fn active_pair(stats: &[RtcStats]) -> Option<&rtc::dictionaries::CandidatePairStats> {
    let pairs = || {
        stats.iter().filter_map(|s| match s {
            RtcStats::CandidatePair(p) => Some(p),
            _ => None,
        })
    };
    let selected = stats.iter().find_map(|s| match s {
        RtcStats::Transport(t) if !t.transport.selected_candidate_pair_id.is_empty() => {
            pairs().find(|p| p.rtc.id == t.transport.selected_candidate_pair_id)
        }
        _ => None,
    });
    selected
        .or_else(|| {
            pairs()
                .filter(|p| p.candidate_pair.nominated)
                .max_by_key(|p| p.candidate_pair.bytes_sent + p.candidate_pair.bytes_received)
        })
        .map(|p| &p.candidate_pair)
}

fn local_candidate_kind(stats: &[RtcStats], id: &str) -> String {
    stats
        .iter()
        .find_map(|s| match s {
            RtcStats::LocalCandidate(c) if c.rtc.id == id => {
                let c = &c.local_candidate;
                let kind = c
                    .candidate_type
                    .map_or("?".to_owned(), |t| format!("{t:?}").to_lowercase());
                Some(format!("{kind} {}", c.protocol))
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn positive(v: f64) -> Option<f64> {
    (v > 0.0).then_some(v)
}

/// Rates over the interval between two collections taken `secs` apart.
pub fn snapshot(prev: &Counters, cur: &Counters, secs: f64) -> NetSnapshot {
    let secs = secs.max(1e-3);
    let kbps = |bytes: u64| bytes as f64 * 8.0 / secs / 1000.0;

    let recv = cur.inbound.as_ref().map(|c| {
        let p = prev.inbound.clone().unwrap_or_default();
        let received = c.packets_received.saturating_sub(p.packets_received);
        let lost = (c.packets_lost - p.packets_lost).max(0) as u64;
        RecvStats {
            kbps: kbps(c.bytes_received.saturating_sub(p.bytes_received)),
            fps: c.fps,
            width: c.width,
            height: c.height,
            loss_pct: percent(lost, received + lost),
            jitter_ms: c.jitter_s * 1000.0,
            freezes: c.freeze_count.saturating_sub(p.freeze_count),
            freeze_ms: ((c.total_freeze_s - p.total_freeze_s) * 1000.0).max(0.0),
            dropped: c.frames_dropped.saturating_sub(p.frames_dropped),
            nacks: c.nack_count.saturating_sub(p.nack_count),
            plis: c.pli_count.saturating_sub(p.pli_count),
            decoder: c.decoder.clone(),
        }
    });

    let send = cur.outbound.as_ref().map(|c| {
        let p = prev.outbound.clone().unwrap_or_default();
        let sent = c.packets_sent.saturating_sub(p.packets_sent);
        let resent = c
            .retransmitted_packets_sent
            .saturating_sub(p.retransmitted_packets_sent);
        SendStats {
            kbps: kbps(c.bytes_sent.saturating_sub(p.bytes_sent)),
            target_kbps: c.target_bitrate / 1000.0,
            fps: c.fps,
            width: c.width,
            height: c.height,
            limit: c.limit,
            encoder: c.encoder.clone(),
            loss_pct: c.remote_fraction_lost.map(|f| f * 100.0),
            retransmit_pct: percent(resent, sent),
            nacks: c.nack_count.saturating_sub(p.nack_count),
            plis: c.pli_count.saturating_sub(p.pli_count),
        }
    });

    NetSnapshot {
        rtt_ms: cur.path.as_ref().and_then(|p| p.rtt_s).map(|s| s * 1000.0),
        bwe_kbps: cur
            .path
            .as_ref()
            .and_then(|p| p.available_outgoing_bps)
            .map(|b| b / 1000.0),
        path: cur.path.as_ref().map(|p| p.kind.clone()),
        send,
        recv,
        adapt: None,
    }
}

fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

/// Overall connection grade, like the coloured dot in a call app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Quality {
    Good,
    Fair,
    Poor,
}

/// Thresholds, as (good below, fair below). Anything above "fair" is poor.
const RTT_MS: (f64, f64) = (80.0, 200.0);
const LOSS_PCT: (f64, f64) = (1.0, 5.0);
const JITTER_MS: (f64, f64) = (30.0, 80.0);
/// Frozen for this long within one interval is poor.
const FREEZE_POOR_MS: f64 = 500.0;

/// Grades one or both sides of a call: the worst signal wins.
pub fn connection_quality(snapshots: &[&NetSnapshot]) -> Quality {
    let grade = |v: f64, (good, fair): (f64, f64)| {
        if v < good {
            Quality::Good
        } else if v < fair {
            Quality::Fair
        } else {
            Quality::Poor
        }
    };
    let mut worst = Quality::Good;
    for s in snapshots {
        if let Some(rtt) = s.rtt_ms {
            worst = worst.max(grade(rtt, RTT_MS));
        }
        if let Some(loss) = s.send.as_ref().and_then(|t| t.loss_pct) {
            worst = worst.max(grade(loss, LOSS_PCT));
        }
        if let Some(r) = &s.recv {
            worst = worst.max(grade(r.loss_pct, LOSS_PCT));
            worst = worst.max(grade(r.jitter_ms, JITTER_MS));
            if r.freeze_ms >= FREEZE_POOR_MS {
                worst = Quality::Poor;
            } else if r.freezes > 0 {
                worst = worst.max(Quality::Fair);
            }
        }
    }
    worst
}

/// Both ends' latest stats as seen by the viewer, plus their history for the results file.
#[derive(Debug, Default)]
pub struct Board {
    pub local: Option<NetSnapshot>,
    pub remote: Option<NetSnapshot>,
    pub history: Vec<BoardEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoardEntry {
    pub t_s: f64,
    pub local: Option<NetSnapshot>,
    pub remote: Option<NetSnapshot>,
}

impl Board {
    /// Records our own snapshot, taken `t_s` seconds into the call.
    pub fn local_update(&mut self, t_s: f64, snap: NetSnapshot) {
        self.local = Some(snap);
        self.history.push(BoardEntry {
            t_s,
            local: self.local.clone(),
            remote: self.remote.clone(),
        });
    }

    pub fn quality(&self) -> Option<Quality> {
        let sides: Vec<&NetSnapshot> = [&self.local, &self.remote].into_iter().flatten().collect();
        (!sides.is_empty()).then(|| connection_quality(&sides))
    }
}

/// Polls one side's peer connection and turns successive reports into snapshots.
#[derive(Debug)]
pub struct Poller {
    side: Side,
    prev: Option<(Instant, Counters)>,
}

/// Which direction we care about. LiveKit may use separate publish and subscribe connections,
/// or one connection for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Publisher,
    Subscriber,
}

impl Poller {
    pub fn new(side: Side) -> Self {
        Self { side, prev: None }
    }

    /// Returns `None` on the first call (no interval yet) or if stats aren't available.
    pub async fn poll(&mut self, room: &Room) -> Option<NetSnapshot> {
        let stats = match room.get_stats().await {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("stats unavailable: {e}");
                return None;
            }
        };
        let report = match self.side {
            // In single peer connection mode (the default with current servers) one
            // connection carries both directions and the SDK reports it as the publisher.
            Side::Subscriber if !stats.subscriber_stats.is_empty() => &stats.subscriber_stats,
            _ => &stats.publisher_stats,
        };
        let now = Instant::now();
        let cur = collect(report);
        let snap = self
            .prev
            .as_ref()
            .map(|(at, prev)| snapshot(prev, &cur, now.duration_since(*at).as_secs_f64()));
        self.prev = Some((now, cur));
        snap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inbound(received: u64, lost: i64, bytes: u64, freezes: u32, freeze_s: f64) -> Inbound {
        Inbound {
            packets_received: received,
            packets_lost: lost,
            bytes_received: bytes,
            jitter_s: 0.004,
            freeze_count: freezes,
            total_freeze_s: freeze_s,
            width: 1660,
            height: 1080,
            fps: 58.0,
            ..Default::default()
        }
    }

    #[test]
    fn receive_rates_over_interval() {
        let prev = Counters {
            inbound: Some(inbound(1000, 10, 1_000_000, 1, 0.2)),
            ..Default::default()
        };
        let cur = Counters {
            inbound: Some(inbound(1990, 20, 1_250_000, 2, 0.5)),
            path: Some(Path {
                rtt_s: Some(0.022),
                available_outgoing_bps: Some(8_000_000.0),
                kind: "host udp".into(),
            }),
            ..Default::default()
        };
        let s = snapshot(&prev, &cur, 1.0);
        let r = s.recv.unwrap();
        assert_eq!(r.kbps, 2000.0);
        assert!((r.loss_pct - 1.0).abs() < 1e-9, "10 lost of 1000");
        assert_eq!(r.jitter_ms, 4.0);
        assert_eq!(r.freezes, 1);
        assert!((r.freeze_ms - 300.0).abs() < 1e-6);
        assert_eq!(s.rtt_ms, Some(22.0));
        assert_eq!(s.bwe_kbps, Some(8000.0));
        assert_eq!(s.path.as_deref(), Some("host udp"));
        assert!(s.send.is_none());
    }

    #[test]
    fn first_interval_counts_from_zero() {
        let cur = Counters {
            inbound: Some(inbound(100, 0, 125_000, 0, 0.0)),
            ..Default::default()
        };
        let r = snapshot(&Counters::default(), &cur, 0.5).recv.unwrap();
        assert_eq!(r.kbps, 2000.0);
        assert_eq!(r.loss_pct, 0.0);
    }

    #[test]
    fn counter_reset_does_not_go_negative() {
        let prev = Counters {
            inbound: Some(inbound(5000, 50, 9_000_000, 3, 1.0)),
            ..Default::default()
        };
        let cur = Counters {
            inbound: Some(inbound(10, 0, 1000, 0, 0.0)),
            ..Default::default()
        };
        let r = snapshot(&prev, &cur, 1.0).recv.unwrap();
        assert_eq!(
            (r.kbps, r.loss_pct, r.freezes, r.freeze_ms),
            (0.0, 0.0, 0, 0.0)
        );
    }

    #[test]
    fn send_rates_and_remote_loss() {
        let out = |sent, resent, bytes| Outbound {
            packets_sent: sent,
            retransmitted_packets_sent: resent,
            bytes_sent: bytes,
            target_bitrate: 4_000_000.0,
            limit: Limit::Bandwidth,
            remote_fraction_lost: Some(0.02),
            ..Default::default()
        };
        let prev = Counters {
            outbound: Some(out(1000, 0, 0)),
            ..Default::default()
        };
        let cur = Counters {
            outbound: Some(out(1200, 6, 500_000)),
            ..Default::default()
        };
        let t = snapshot(&prev, &cur, 2.0).send.unwrap();
        assert_eq!(t.kbps, 2000.0);
        assert_eq!(t.target_kbps, 4000.0);
        assert_eq!(t.loss_pct, Some(2.0));
        assert_eq!(t.retransmit_pct, 3.0);
        assert_eq!(t.limit, Limit::Bandwidth);
    }

    fn snap(rtt: f64, loss: f64, jitter: f64, freezes: u32, freeze_ms: f64) -> NetSnapshot {
        NetSnapshot {
            rtt_ms: Some(rtt),
            recv: Some(RecvStats {
                loss_pct: loss,
                jitter_ms: jitter,
                freezes,
                freeze_ms,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn quality_grades() {
        assert_eq!(
            connection_quality(&[&snap(20.0, 0.0, 3.0, 0, 0.0)]),
            Quality::Good
        );
        assert_eq!(
            connection_quality(&[&snap(120.0, 0.0, 3.0, 0, 0.0)]),
            Quality::Fair
        );
        assert_eq!(
            connection_quality(&[&snap(20.0, 2.0, 3.0, 0, 0.0)]),
            Quality::Fair
        );
        assert_eq!(
            connection_quality(&[&snap(20.0, 0.0, 90.0, 0, 0.0)]),
            Quality::Poor
        );
        assert_eq!(
            connection_quality(&[&snap(20.0, 0.0, 3.0, 1, 100.0)]),
            Quality::Fair
        );
        assert_eq!(
            connection_quality(&[&snap(20.0, 0.0, 3.0, 1, 600.0)]),
            Quality::Poor
        );
        assert_eq!(connection_quality(&[]), Quality::Good);
    }

    #[test]
    fn worst_side_wins() {
        let good = snap(20.0, 0.0, 3.0, 0, 0.0);
        let bad = snap(300.0, 0.0, 3.0, 0, 0.0);
        assert_eq!(connection_quality(&[&good, &bad]), Quality::Poor);
    }

    #[test]
    fn collect_picks_video_and_selected_pair() {
        use rtc::dictionaries as d;
        let mut video = rtc::InboundRtpStats::default();
        video.stream.kind = "video".into();
        video.inbound.bytes_received = 500;
        video.received.packets_lost = 3;
        let mut audio = rtc::InboundRtpStats::default();
        audio.stream.kind = "audio".into();
        audio.inbound.bytes_received = 9999;

        let pair = |id: &str, rtt: f64, nominated: bool| {
            let mut p = rtc::CandidatePairStats::default();
            p.rtc.id = id.into();
            p.candidate_pair.current_round_trip_time = rtt;
            p.candidate_pair.responses_received = 1;
            p.candidate_pair.nominated = nominated;
            p.candidate_pair.local_candidate_id = "L1".into();
            RtcStats::CandidatePair(p)
        };
        let mut transport = rtc::TransportStats::default();
        transport.transport.selected_candidate_pair_id = "P2".into();
        let local = rtc::LocalCandidateStats {
            rtc: d::RtcStats {
                id: "L1".into(),
                timestamp: 0,
            },
            local_candidate: d::IceCandidateStats {
                candidate_type: Some(rtc::IceCandidateType::Relay),
                protocol: "tcp".into(),
                ..Default::default()
            },
        };

        let c = collect(&[
            RtcStats::InboundRtp(audio),
            RtcStats::InboundRtp(video),
            pair("P1", 0.5, true),
            pair("P2", 0.03, false),
            RtcStats::Transport(transport),
            RtcStats::LocalCandidate(local),
        ]);
        let inbound = c.inbound.unwrap();
        assert_eq!((inbound.bytes_received, inbound.packets_lost), (500, 3));
        let path = c.path.unwrap();
        assert_eq!(path.rtt_s, Some(0.03), "selected pair wins over nominated");
        assert_eq!(path.kind, "relay tcp");
        assert!(c.outbound.is_none());
    }

    #[test]
    fn snapshot_survives_json() {
        let s = snap(20.0, 0.5, 3.0, 0, 0.0);
        let back: NetSnapshot = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
