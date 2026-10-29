//! Latency sample collection and percentile summaries.

use serde::Serialize;

/// Summary of a set of latency samples, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Summary {
    pub count: usize,
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
    pub mean: f64,
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "p50 {:.1} ms  p95 {:.1} ms  max {:.1} ms  (n={})",
            self.p50, self.p95, self.max, self.count
        )
    }
}

/// Nearest-rank percentile of already sorted samples. `p` is in 0..=100.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Summarizes samples, or returns `None` when there are none.
pub fn summarize(samples: &[f64]) -> Option<Summary> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(Summary {
        count: sorted.len(),
        p50: percentile(&sorted, 50.0),
        p95: percentile(&sorted, 95.0),
        max: sorted[sorted.len() - 1],
        mean: sorted.iter().sum::<f64>() / sorted.len() as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_has_no_summary() {
        assert_eq!(summarize(&[]), None);
    }

    #[test]
    fn single_sample() {
        let s = summarize(&[42.0]).unwrap();
        assert_eq!((s.p50, s.p95, s.max, s.mean), (42.0, 42.0, 42.0, 42.0));
    }

    #[test]
    fn nearest_rank_percentiles() {
        let samples: Vec<f64> = (1..=100).rev().map(f64::from).collect();
        let s = summarize(&samples).unwrap();
        assert_eq!(s.count, 100);
        assert_eq!(s.p50, 50.0);
        assert_eq!(s.p95, 95.0);
        assert_eq!(s.max, 100.0);
        assert_eq!(s.mean, 50.5);
    }

    #[test]
    fn small_sets_round_up() {
        let s = summarize(&[10.0, 20.0, 30.0]).unwrap();
        assert_eq!(s.p50, 20.0);
        assert_eq!(s.p95, 30.0);
    }
}
