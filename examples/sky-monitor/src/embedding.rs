//! Deterministic feature embeddings (ADR-199 §13).
//!
//! These are engineered (not learned) embeddings: every dimension is a
//! documented, normalized feature so that euclidean distance in embedding
//! space is interpretable. Per the ADR table, an **aircraft-track** embedding
//! encodes path shape, speed profile, altitude profile, time of day, and route
//! class; a **weather-window** embedding encodes the precip/wind state.
//!
//! Track and weather embeddings have different dimensions and live in
//! **separate** RuVector collections (one `VectorDB` per modality) — never mix
//! them in one index.
//!
//! Normalization targets `[0, 1]` per dimension (a few can mildly exceed 1 for
//! out-of-envelope inputs, which is fine for distance purposes):
//!
//! | dim | feature | scale |
//! |-----|---------|-------|
//! | 0–2 | mean / min / max altitude | / 12 000 m |
//! | 3   | mean ground speed | / 300 m/s |
//! | 4–5 | dominant heading sin/cos | mapped to `[0,1]` |
//! | 6–7 | time-of-day sin/cos | `0.5 ± 0.25` (half weight vs heading) |
//! | 8   | min slant range | / 50 km |
//! | 9–10| max / mean elevation | / 90° |
//! | 11  | duration | / 1 800 s |
//! | 12–13 | climb / descent point ratio | already 0–1 |
//! | 14  | path straightness | already 0–1 |
//! | 15  | mean abs vertical rate | / 15 m/s |
//! | 16–23 | azimuth-bucket occupancy (8 × 45° buckets) | fractions |
//! | 24  | mean signal | (dBFS + 40) / 40 |
//! | 25  | speed std | / 50 m/s |
//! | 26  | altitude std | / 3 000 m |
//! | 27–28 | start / end slant range | / 50 km |
//! | 29  | sample count | / 600 |
//! | 30  | overhead-candidate flag | 0 or 1 |
//! | 31  | reserved | 0 |

use crate::track::Track;
use crate::weather::{WeatherCondition, WeatherWindow};
use chrono::Timelike;

/// Dimension of aircraft-track embeddings.
pub const TRACK_EMBEDDING_DIM: usize = 32;
/// Dimension of weather-window embeddings (separate collection!).
pub const WEATHER_EMBEDDING_DIM: usize = 8;

fn clamp01(v: f64) -> f32 {
    v.clamp(0.0, 1.0) as f32
}

/// Compute the 32-dimensional track embedding described in the module docs.
/// Fully deterministic in the track contents.
pub fn track_embedding(track: &Track) -> Vec<f32> {
    let mut e = vec![0.0f32; TRACK_EMBEDDING_DIM];
    // Single pass over the points: accumulate every per-point statistic at
    // once instead of one full iteration per feature (the Track stat methods
    // would walk the point list ~14 times and allocate a temp Vec).
    let pts = &track.points;
    let count = pts.len();
    let nf = count as f64;
    let mut alt_sum = 0.0f64;
    let mut alt_sq = 0.0f64;
    let mut min_alt = f64::INFINITY;
    let mut max_alt = f64::NEG_INFINITY;
    let mut speed_sum = 0.0f64;
    let mut speed_sq = 0.0f64;
    let mut sig_sum = 0.0f64;
    let mut elev_sum = 0.0f64;
    let mut vr_abs_sum = 0.0f64;
    let mut climb = 0usize;
    let mut descent = 0usize;
    let mut head_s = 0.0f64;
    let mut head_c = 0.0f64;
    let mut buckets = [0u32; 8];
    let mut path_m = 0.0f64;
    let mut prev_lat = 0.0f64;
    let mut prev_lon = 0.0f64;
    for (i, p) in pts.iter().enumerate() {
        alt_sum += p.alt_m;
        alt_sq += p.alt_m * p.alt_m;
        min_alt = min_alt.min(p.alt_m);
        max_alt = max_alt.max(p.alt_m);
        speed_sum += p.speed_mps;
        speed_sq += p.speed_mps * p.speed_mps;
        sig_sum += p.signal_dbfs;
        elev_sum += p.frame.elevation_deg;
        vr_abs_sum += p.vertical_rate_mps.abs();
        if p.vertical_rate_mps > 2.0 {
            climb += 1;
        }
        if p.vertical_rate_mps < -2.0 {
            descent += 1;
        }
        let r = p.track_deg.to_radians();
        head_s += r.sin();
        head_c += r.cos();
        let b = ((p.frame.azimuth_deg.rem_euclid(360.0)) / 45.0) as usize % 8;
        buckets[b] += 1;
        if i > 0 {
            // Equirectangular step, identical to Track::path_length_m.
            let dy = (p.lat - prev_lat) * 111_132.0;
            let dx = (p.lon - prev_lon) * 111_320.0 * ((prev_lat + p.lat) / 2.0).to_radians().cos();
            path_m += dx.hypot(dy);
        }
        prev_lat = p.lat;
        prev_lon = p.lon;
    }
    let mean = |sum: f64| if count == 0 { 0.0 } else { sum / nf };
    let std = |sq: f64, sum: f64| {
        if count == 0 {
            0.0
        } else {
            let m = sum / nf;
            (sq / nf - m * m).max(0.0).sqrt()
        }
    };

    e[0] = clamp01(mean(alt_sum) / 12_000.0);
    e[1] = clamp01(min_alt / 12_000.0);
    e[2] = clamp01(max_alt / 12_000.0);
    e[3] = clamp01(mean(speed_sum) / 300.0);

    // Circular-mean ground track, as Track::dominant_heading_deg.
    let h = crate::coords::normalize_deg(head_s.atan2(head_c).to_degrees()).to_radians();
    e[4] = ((h.sin() + 1.0) / 2.0) as f32;
    e[5] = ((h.cos() + 1.0) / 2.0) as f32;

    // Time of day on the unit circle (UTC), half-amplitude so route class
    // dominates time of day in distance terms.
    let frac = (track.started.hour() as f64 + track.started.minute() as f64 / 60.0) / 24.0;
    let a = frac * std::f64::consts::TAU;
    e[6] = (0.5 + 0.25 * a.sin()) as f32;
    e[7] = (0.5 + 0.25 * a.cos()) as f32;

    e[8] = clamp01(track.min_range_m / 50_000.0);
    e[9] = clamp01(track.max_elevation_deg / 90.0);
    e[10] = clamp01(mean(elev_sum) / 90.0);
    e[11] = clamp01(track.duration_secs() / 1_800.0);
    e[12] = clamp01(if count == 0 { 0.0 } else { climb as f64 / nf });
    e[13] = clamp01(if count == 0 { 0.0 } else { descent as f64 / nf });
    // Net displacement / path length, as Track::straightness.
    let straightness = if path_m < 1.0 {
        1.0
    } else {
        let (a, b) = (pts.first().unwrap(), pts.last().unwrap());
        let dy = (b.lat - a.lat) * 111_132.0;
        let dx = (b.lon - a.lon) * 111_320.0 * ((a.lat + b.lat) / 2.0).to_radians().cos();
        (dx.hypot(dy) / path_m).clamp(0.0, 1.0)
    };
    e[14] = clamp01(straightness);
    e[15] = clamp01(mean(vr_abs_sum) / 15.0);

    // Coarse azimuth occupancy: fraction of samples seen in each 45° sky
    // sector around the observer (route-class signature).
    let n = count.max(1) as f64;
    for (b, &hits) in buckets.iter().enumerate() {
        e[16 + b] = (f64::from(hits) / n) as f32;
    }

    e[24] = clamp01((mean(sig_sum) + 40.0) / 40.0);
    e[25] = clamp01(std(speed_sq, speed_sum) / 50.0);
    e[26] = clamp01(std(alt_sq, alt_sum) / 3_000.0);
    e[27] = clamp01(pts.first().map(|p| p.frame.range_m).unwrap_or(0.0) / 50_000.0);
    e[28] = clamp01(pts.last().map(|p| p.frame.range_m).unwrap_or(0.0) / 50_000.0);
    e[29] = clamp01(count as f64 / 600.0);
    e[30] = if track.is_overhead_candidate {
        1.0
    } else {
        0.0
    };
    e[31] = 0.0; // reserved
    e
}

/// Weather-window embedding (8 dims, separate collection):
/// `[condition one-hot-ish severity, precip/5, wind/20, alert flag,
///   hour sin/cos (half weight), reserved, reserved]`.
pub fn weather_window_embedding(w: &WeatherWindow) -> Vec<f32> {
    let severity = match w.condition {
        WeatherCondition::Clear => 0.0,
        WeatherCondition::Cloudy => 0.25,
        WeatherCondition::Fog => 0.5,
        WeatherCondition::Rain => 0.6,
        WeatherCondition::Snow => 0.7,
        WeatherCondition::Thunderstorm => 1.0,
    };
    let frac = w.start.hour() as f64 / 24.0;
    let a = frac * std::f64::consts::TAU;
    vec![
        severity as f32,
        clamp01(w.precip_mm_hr / 5.0),
        clamp01(w.wind_mps / 20.0),
        if w.alert.is_some() { 1.0 } else { 0.0 },
        (0.5 + 0.25 * a.sin()) as f32,
        (0.5 + 0.25 * a.cos()) as f32,
        0.0,
        0.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adsb::{default_day_start, generate_scenario, ANOMALOUS_ICAO24};
    use crate::config::ObserverConfig;
    use crate::observation::{EntityType, GeoPosition, Motion, Observation};
    use crate::track::{stitch_tracks, TRACK_GAP_SECS};

    fn tracks() -> Vec<crate::track::Track> {
        let cfg = ObserverConfig::default();
        let obs: Vec<Observation> = generate_scenario(cfg.lat, cfg.lon, 42, default_day_start())
            .iter()
            .map(|s| {
                Observation::new(
                    &cfg,
                    "adsb_synthetic",
                    EntityType::Aircraft,
                    s.icao24.clone(),
                    s.ts,
                    GeoPosition {
                        lat: s.lat,
                        lon: s.lon,
                        alt_m: s.alt_m,
                    },
                    Motion {
                        speed_mps: s.speed_mps,
                        track_deg: s.track_deg,
                        vertical_rate_mps: s.vertical_rate_mps,
                    },
                    serde_json::json!({ "callsign": s.callsign, "signal_dbfs": s.signal_dbfs }),
                    0.95,
                )
            })
            .collect();
        stitch_tracks(&cfg, &obs, TRACK_GAP_SECS)
    }

    fn dist(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y) * (x - y))
            .sum::<f32>()
            .sqrt()
    }

    #[test]
    fn embeddings_are_fixed_dim_and_separate_route_classes() {
        let ts = tracks();
        let embs: Vec<(String, Vec<f32>)> = ts
            .iter()
            .map(|t| (t.icao24.clone(), track_embedding(t)))
            .collect();
        for (_, e) in &embs {
            assert_eq!(e.len(), TRACK_EMBEDDING_DIM);
            assert!(e.iter().all(|v| (0.0..=1.0001).contains(v)));
        }
        // Two eastbound corridor flights must be closer to each other than
        // either is to the anomalous track.
        let east1 = &embs.iter().find(|(i, _)| i == "c01a01").unwrap().1;
        let east2 = &embs.iter().find(|(i, _)| i == "a02b02").unwrap().1;
        let anom = &embs.iter().find(|(i, _)| i == ANOMALOUS_ICAO24).unwrap().1;
        assert!(dist(east1, east2) < dist(east1, anom));
        assert!(dist(east1, east2) < dist(east2, anom));
    }

    #[test]
    fn weather_embedding_dim_and_alert_flag() {
        let w = crate::weather::generate_weather(42, default_day_start());
        let rain = weather_window_embedding(&w[14]);
        assert_eq!(rain.len(), WEATHER_EMBEDDING_DIM);
        assert_eq!(rain[3], 1.0, "rain window carries an alert");
        let clear = weather_window_embedding(&w[2]);
        assert_eq!(clear[3], 0.0);
    }
}
