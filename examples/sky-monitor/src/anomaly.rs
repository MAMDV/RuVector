//! Composite anomaly scoring (ADR-199 §15).
//!
//! ```text
//! anomaly_score = 0.30 * route_deviation
//!               + 0.20 * altitude_deviation
//!               + 0.15 * time_of_day_rarity
//!               + 0.15 * signal_unusualness
//!               + 0.10 * cross_sensor_confirmation
//!               + 0.10 * novelty_score
//! ```
//!
//! Every component is `[0, 1]`, computed against [`BaselineStats`] built from
//! the tracks observed **before** the one being scored. Governance rule 2
//! (ADR §27): every anomaly report carries human-readable reasons.

use crate::config::AnomalyConfig;
use crate::track::Track;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Divisor squashing |z|-scores into `[0, 1]` (saturates at 2.5 σ).
const Z_SQUASH: f64 = 2.5;
/// Heading deviation (degrees from nearest baseline corridor) that saturates
/// the route component.
const ROUTE_SATURATION_DEG: f64 = 60.0;
/// ±hours window and saturation count for time-of-day rarity.
const HOUR_WINDOW: i64 = 2;
const HOUR_SATURATION: f64 = 3.0;

/// Baseline statistics over prior tracks.
#[derive(Debug, Clone, Default)]
pub struct BaselineStats {
    /// Dominant headings of prior tracks (the known corridors), degrees.
    pub corridor_headings: Vec<f64>,
    pub altitude_mean_m: f64,
    pub altitude_std_m: f64,
    pub signal_mean_dbfs: f64,
    pub signal_std_dbfs: f64,
    /// Start-hour histogram (UTC) of prior tracks.
    pub hour_counts: [u32; 24],
    pub n_tracks: usize,
}

impl BaselineStats {
    /// Build baseline statistics from prior tracks.
    pub fn from_tracks(prior: &[Track]) -> Self {
        let n = prior.len();
        if n == 0 {
            return Self::default();
        }
        let alts: Vec<f64> = prior.iter().map(|t| t.mean_altitude_m()).collect();
        let sigs: Vec<f64> = prior.iter().map(|t| t.mean_signal_dbfs()).collect();
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let std = |v: &[f64], m: f64| {
            (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / v.len() as f64).sqrt()
        };
        let am = mean(&alts);
        let sm = mean(&sigs);
        let mut hours = [0u32; 24];
        for t in prior {
            hours[t.start_hour_utc() as usize] += 1;
        }
        Self {
            corridor_headings: prior.iter().map(|t| t.dominant_heading_deg()).collect(),
            altitude_mean_m: am,
            altitude_std_m: std(&alts, am).max(500.0), // floor: avoid div-by-~0
            signal_mean_dbfs: sm,
            signal_std_dbfs: std(&sigs, sm).max(1.0),
            hour_counts: hours,
            n_tracks: n,
        }
    }
}

/// Smallest circular difference between two headings, degrees in `[0, 180]`.
pub fn circular_diff_deg(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// The six §15 components, each in `[0, 1]`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AnomalyComponents {
    pub route_deviation: f64,
    pub altitude_deviation: f64,
    pub time_of_day_rarity: f64,
    pub signal_unusualness: f64,
    pub cross_sensor_confirmation: f64,
    pub novelty_score: f64,
}

/// ADR-199 §15 interpretation bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Interpretation {
    /// 0.00–0.30 — store.
    Normal,
    /// 0.31–0.55 — timeline marker.
    MildlyUnusual,
    /// 0.56–0.75 — include in summary.
    Interesting,
    /// 0.76–0.90 — local alert.
    StrongAnomaly,
    /// 0.91–1.00 — preserve raw data + generate report.
    Rare,
}

impl Interpretation {
    /// Band for a composite score.
    pub fn band(score: f64) -> Self {
        match score {
            s if s <= 0.30 => Interpretation::Normal,
            s if s <= 0.55 => Interpretation::MildlyUnusual,
            s if s <= 0.75 => Interpretation::Interesting,
            s if s <= 0.90 => Interpretation::StrongAnomaly,
            _ => Interpretation::Rare,
        }
    }

    /// Action column of the §15 table.
    pub fn action(&self) -> &'static str {
        match self {
            Interpretation::Normal => "store",
            Interpretation::MildlyUnusual => "timeline marker",
            Interpretation::Interesting => "include in summary",
            Interpretation::StrongAnomaly => "local alert",
            Interpretation::Rare => "preserve raw data + report",
        }
    }
}

impl fmt::Display for Interpretation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Interpretation::Normal => "normal",
            Interpretation::MildlyUnusual => "mildly unusual",
            Interpretation::Interesting => "interesting",
            Interpretation::StrongAnomaly => "strong anomaly",
            Interpretation::Rare => "rare",
        };
        f.write_str(s)
    }
}

/// Scored anomaly for one track, with mandatory reasons (governance rule 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyReport {
    pub track_id: String,
    pub icao24: String,
    pub callsign: String,
    pub score: f64,
    pub components: AnomalyComponents,
    pub band: Interpretation,
    /// Human-readable reasons — never empty.
    pub reasons: Vec<String>,
}

/// Score one track against the baseline (§15). `novelty` comes from the
/// RuVector indexer; `cross_sensor` is the Phase 5 corroboration placeholder
/// (0 when no second sensor confirms the entity).
pub fn score_track(
    cfg: &AnomalyConfig,
    track: &Track,
    baseline: &BaselineStats,
    novelty: f64,
    cross_sensor: f64,
) -> AnomalyReport {
    let heading = track.dominant_heading_deg();
    let nearest_corridor = baseline
        .corridor_headings
        .iter()
        .map(|c| circular_diff_deg(heading, *c))
        .fold(f64::INFINITY, f64::min);
    let route_deviation = if nearest_corridor.is_finite() {
        (nearest_corridor / ROUTE_SATURATION_DEG).min(1.0)
    } else {
        0.5 // no baseline corridors yet
    };

    let alt_z = (track.mean_altitude_m() - baseline.altitude_mean_m).abs() / baseline.altitude_std_m.max(1.0);
    let altitude_deviation = (alt_z / Z_SQUASH).min(1.0);

    // Rarity of the start hour: how many prior tracks started within ±2 h
    // (circular over the day); 3+ neighbours = fully ordinary.
    let hour = track.start_hour_utc() as i64;
    let mut window_count = 0u32;
    for dh in -HOUR_WINDOW..=HOUR_WINDOW {
        window_count += baseline.hour_counts[((hour + dh).rem_euclid(24)) as usize];
    }
    let time_of_day_rarity = (1.0 - f64::from(window_count) / HOUR_SATURATION).max(0.0);

    let sig_z = (track.mean_signal_dbfs() - baseline.signal_mean_dbfs).abs() / baseline.signal_std_dbfs;
    let signal_unusualness = (sig_z / Z_SQUASH).min(1.0);

    let components = AnomalyComponents {
        route_deviation,
        altitude_deviation,
        time_of_day_rarity,
        signal_unusualness,
        cross_sensor_confirmation: cross_sensor.clamp(0.0, 1.0),
        novelty_score: novelty.clamp(0.0, 1.0),
    };
    let score = cfg.w_route_deviation * components.route_deviation
        + cfg.w_altitude_deviation * components.altitude_deviation
        + cfg.w_time_of_day_rarity * components.time_of_day_rarity
        + cfg.w_signal_unusualness * components.signal_unusualness
        + cfg.w_cross_sensor_confirmation * components.cross_sensor_confirmation
        + cfg.w_novelty_score * components.novelty_score;

    let mut reasons = Vec::new();
    if components.route_deviation > 0.5 {
        reasons.push(format!(
            "heading {heading:.0}° is {nearest_corridor:.0}° off the nearest known corridor"
        ));
    }
    if components.altitude_deviation > 0.5 {
        reasons.push(format!(
            "mean altitude {:.0} m deviates {alt_z:.1}σ from the local baseline ({:.0} m)",
            track.mean_altitude_m(),
            baseline.altitude_mean_m
        ));
    }
    if components.time_of_day_rarity > 0.5 {
        reasons.push(format!(
            "start time {:02}:xx UTC has {window_count} prior tracks within ±{HOUR_WINDOW} h",
            track.start_hour_utc()
        ));
    }
    if components.signal_unusualness > 0.5 {
        reasons.push(format!(
            "signal {:.1} dBFS is {sig_z:.1}σ from baseline {:.1} dBFS (unusually close/strong)",
            track.mean_signal_dbfs(),
            baseline.signal_mean_dbfs
        ));
    }
    if components.cross_sensor_confirmation > 0.5 {
        reasons.push("corroborated by a second sensor modality".to_string());
    }
    if components.novelty_score > 0.5 {
        reasons.push(format!(
            "vector novelty {:.2}: no similar track in RuVector memory",
            components.novelty_score
        ));
    }
    if track.callsign.is_empty() && score > 0.30 {
        reasons.push("no callsign broadcast".to_string());
    }
    if reasons.is_empty() {
        reasons.push(format!(
            "within normal envelope: heading {heading:.0}°, altitude {:.0} m, score {score:.2}",
            track.mean_altitude_m()
        ));
    }

    AnomalyReport {
        track_id: track.track_id.clone(),
        icao24: track.icao24.clone(),
        callsign: track.callsign.clone(),
        score,
        components,
        band: Interpretation::band(score),
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_match_adr_table() {
        assert_eq!(Interpretation::band(0.10), Interpretation::Normal);
        assert_eq!(Interpretation::band(0.30), Interpretation::Normal);
        assert_eq!(Interpretation::band(0.42), Interpretation::MildlyUnusual);
        assert_eq!(Interpretation::band(0.60), Interpretation::Interesting);
        assert_eq!(Interpretation::band(0.76), Interpretation::StrongAnomaly);
        assert_eq!(Interpretation::band(0.95), Interpretation::Rare);
        assert_eq!(Interpretation::band(0.80).action(), "local alert");
    }

    #[test]
    fn circular_diff_wraps() {
        assert!((circular_diff_deg(350.0, 10.0) - 20.0).abs() < 1e-9);
        assert!((circular_diff_deg(72.0, 252.0) - 180.0).abs() < 1e-9);
    }
}
