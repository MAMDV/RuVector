# sky-monitor — RuView SkyGraph Appliance core (ADR-199, Phases 1–4)

> **See the sky. Remember the sky. Explain the sky.**

A local sky-monitoring appliance core that observes, projects, records, and
explains activity above a fixed observer (the reference *Oakville node*,
43.4675 N / −79.6877 E / 100 m). The sky is treated as a continuously changing
spatial graph, not a dashboard.

Everything runs on a **deterministic synthetic ADS-B + weather scenario** —
no network, no SDR hardware — while a `dump1090` `aircraft.json` parser keeps
the door open for real RTL-SDR data. Vectors live in
[`ruvector-core`](../../crates/ruvector-core) (`VectorDB`), the SkyGraph in
[`ruvector-graph`](../../crates/ruvector-graph) (`GraphDB`).

## Module map → ADR-199 sections

| Module | ADR-199 | What it does |
|--------|---------|--------------|
| `config` | §30 | `ObserverConfig` (Oakville node defaults), `AnomalyConfig` (§15 weights, 0.76 alert threshold, baseline `min_history`) |
| `coords` | §10 | WGS-84 → ECEF → ENU → azimuth/elevation/range/bearing (`ObserverFrame`), pure `f64` |
| `observation` | §11 | Canonical observation schema (uuid, UTC time, entity, location, observer frame, motion, attributes, confidence, `raw_ref`/`embedding_ref`) |
| `adsb` | §9.1, Phase 1 | Seeded synthetic scenario (corridor / arrivals / GA overhead / one anomalous night track) + `parse_dump1090` for real data |
| `track` | §19 skygraph-builder | Gap-based track stitching, summary stats, §14 rule 1 overhead candidates |
| `weather` | §9.3, Phase 2 | Synthetic hourly `WeatherWindow`s aligned to the timeline (incl. the sample-brief rain band) |
| `embedding` | §13 | Deterministic 32-dim track embeddings + 8-dim weather embeddings (separate collections), every dimension documented |
| `indexer` | §19 ruvector-indexer, Phase 4 | `VectorDB` wrapper: similarity search + calibrated novelty score |
| `skygraph` | §12, Phase 3 | `GraphDB` nodes (Observer/Aircraft/Track/Observation/WeatherCell/TimeWindow/Anomaly) + §12 edge vocabulary; time-window / overhead queries; citeable `explain()` |
| `anomaly` | §15 | Exact composite formula, interpretation bands, mandatory reasons (§27 rule 2) |
| `brief` | §21.3 | Daily sky brief with `Display` text block |
| `pipeline` | §22 | One `Pipeline::run()` shared by demo, tests, and benches |

## Run

```bash
# demo (from the repository root)
cargo run -p sky-monitor --release

# acceptance + unit tests (mapped to ADR-199 §31 / §22)
cargo test -p sky-monitor

# criterion benches (projection, embedding, VectorDB, anomaly, end-to-end)
cargo bench -p sky-monitor            # full run
cargo bench -p sky-monitor -- --test  # smoke mode
```

## Sample output (trimmed)

```text
RuView SkyGraph Appliance — synthetic demo (ADR-199 Phases 1-4)
Observer: oakville_node (43.4675, -79.6877, 100 m) | seed 42 | 2820 observations

== Tracks (observer-relative at closest approach) ==
track            call     range_km  az_deg  el_deg    alt_m     hdg speed_mps  overhead
track-c01a01-0   ACA101       13.2     162    52.6    10600      72       236
track-c07e07-0   JZA707        7.0     121    30.6     3679      32       145  yes
track-c0a9a9-0   CGSKY         1.1     187    67.8     1100      88        62  yes
track-deadbf-0   -             2.0     254     9.9      450     165        48  yes
...

== SkyGraph ==
nodes: 109   edges: 111
overhead candidates: ["track-c07e07-0", "track-c08f08-0", "track-c0a9a9-0", "track-deadbf-0"]

== Top similar-track pairs (RuVector, euclidean) ==
  track-a03c03-0 <-> track-c01a01-0   distance 0.378
  track-c01a01-0 <-> track-c04d04-0   distance 0.448

== Anomaly scores (ADR-199 §15) ==
track            call     score  band             reasons
track-c04d04-0   WJA404   0.165  normal           within normal envelope: heading 73°, ...
track-c0a9a9-0   CGSKY    0.570  interesting      mean altitude 1100 m deviates 2.9σ ...
track-deadbf-0   -        0.860  strong anomaly   heading 165° is 77° off the nearest known
                                                  corridor | mean altitude 450 m deviates
                                                  2.0σ | start time 03:xx UTC has 0 prior
                                                  tracks within ±2 h | signal -3.0 dBFS is
                                                  3.3σ | vector novelty 1.00 | no callsign

== Explain track-deadbf-0 (strong anomaly, action: local alert) ==
  - track track-deadbf-0 stitched from 420 observations; evidence observation ids:
    first 1964af06-..., closest approach 6a29bba3-..., last 46473de0-...
  - geometry: closest approach 2034 m at azimuth 254°, max elevation 10.2°, ...
  - near observer:oakville_node (closest approach inside 10 km)
  - during window:2026-06-09T03
  - anomalous_relative_to baseline track-c0a9a9-0
  - correlated_with weather:2026-06-09T03 (clear, wind 2.8 m/s)

== Daily sky brief (ADR-199 §21.3) ==
Sky brief — oakville_node, 2026-06-08. 10 aircraft observed; 4 overhead candidates;
2 unusual tracks. Weather: rain 14:00–16:00 UTC. Most unusual event: low-altitude
pass by icao24 deadbf heading 165° at 03:13 UTC (450 m): heading 165° is 77° off
the nearest known corridor (confidence 0.86).
```

## Synthetic scenario

One day over the observer, seed-deterministic (`Pipeline::default()`, seed 42):

* 4 eastbound + 2 westbound **en-route corridor** flights (~072°/252°,
  10.5–11.2 km, ~230 m/s),
* 2 **arrivals** descending through the area (~032°, 4.8 km → 2.6 km),
* 1 low **general-aviation overhead pass** (1.1 km, within 1.1 km slant range),
* 1 **anomalous track**: 450 m, 48 m/s, heading 165° (off-corridor), 03:10 UTC
  the following night, unusually strong signal, no callsign — scores **0.86 →
  strong anomaly → local alert**, while scored corridor flights stay ≤ 0.23.

The first `min_history` (5) tracks form the unscored baseline; later tracks are
scored against strictly prior tracks (ADR §26: baseline before alerting).

## What is deliberately out of scope here

Phase 5 sensors (audio/RF/camera — `cross_sensor_confirmation` is a documented
placeholder at 0), live dump1090/OpenSky ingestion, the WASM projection engine
and Canvas dashboard (separate `examples/sky-monitor/wasm` work), retention /
hash-chained raw archive, and the NL assistant service.
