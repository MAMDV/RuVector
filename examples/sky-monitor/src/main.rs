//! Demo binary: run the full ADR-199 Phase 1–4 pipeline over the synthetic
//! Oakville-node scenario and print a live-style report.

use sky_monitor::{Interpretation, Pipeline};

fn main() -> sky_monitor::Result<()> {
    let pipeline = Pipeline::default();
    let report = pipeline.run()?;

    println!("RuView SkyGraph Appliance — synthetic demo (ADR-199 Phases 1-4)");
    println!(
        "Observer: {} ({:.4}, {:.4}, {:.0} m) | seed {} | {} observations",
        pipeline.observer.name,
        pipeline.observer.lat,
        pipeline.observer.lon,
        pipeline.observer.alt_m,
        pipeline.seed,
        report.observations.len()
    );

    // ---- Track table (observer frame at closest approach) -----------------
    println!("\n== Tracks (observer-relative at closest approach) ==");
    println!(
        "{:<16} {:<7} {:>9} {:>7} {:>7} {:>8} {:>7} {:>9}  overhead",
        "track", "call", "range_km", "az_deg", "el_deg", "alt_m", "hdg", "speed_mps"
    );
    for t in &report.tracks {
        let f = t.closest_frame();
        println!(
            "{:<16} {:<7} {:>9.1} {:>7.0} {:>7.1} {:>8.0} {:>7.0} {:>9.0}  {}",
            t.track_id,
            if t.callsign.is_empty() { "-" } else { &t.callsign },
            t.min_range_m / 1000.0,
            f.azimuth_deg,
            f.elevation_deg,
            t.mean_altitude_m(),
            t.dominant_heading_deg(),
            t.mean_speed_mps(),
            if t.is_overhead_candidate { "yes" } else { "" }
        );
    }

    // ---- SkyGraph ----------------------------------------------------------
    let (nodes, edges) = report.skygraph.stats();
    println!("\n== SkyGraph ==");
    println!("nodes: {nodes}   edges: {edges}");
    println!("overhead candidates: {:?}", report.skygraph.overhead_candidates());

    // ---- Similarity --------------------------------------------------------
    println!("\n== Top similar-track pairs (RuVector, euclidean) ==");
    for (a, b, d) in report.similar_pairs.iter().take(5) {
        println!("  {a} <-> {b}   distance {d:.3}");
    }

    // ---- Anomalies ---------------------------------------------------------
    println!("\n== Anomaly scores (ADR-199 §15) ==");
    println!("{:<16} {:<7} {:>6}  {:<16} reasons", "track", "call", "score", "band");
    for r in &report.reports {
        println!(
            "{:<16} {:<7} {:>6.3}  {:<16} {}",
            r.track_id,
            if r.callsign.is_empty() { "-" } else { &r.callsign },
            r.score,
            r.band.to_string(),
            r.reasons.join(" | ")
        );
    }

    // ---- Explanation of the most unusual track -----------------------------
    if let Some(worst) = report
        .reports
        .iter()
        .max_by(|a, b| a.score.total_cmp(&b.score))
        .filter(|r| r.band > Interpretation::MildlyUnusual)
    {
        println!(
            "\n== Explain {} ({}, action: {}) ==",
            worst.track_id, worst.band, worst.band.action()
        );
        if let Some(explanation) = report.skygraph.explain(&worst.track_id) {
            for line in &explanation.evidence {
                println!("  - {line}");
            }
        }
    }

    // ---- Daily brief --------------------------------------------------------
    println!("\n== Daily sky brief (ADR-199 §21.3) ==");
    println!("{}", report.brief);
    Ok(())
}
