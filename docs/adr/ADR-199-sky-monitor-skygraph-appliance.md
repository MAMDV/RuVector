---
adr: 199
title: "Sky Monitor and SkyGraph Appliance"
status: proposed
date: 2026-06-09
authors: [Reuven Cohen]
project: "RuView Sky Monitor"
related: [ADR-196, ADR-197, ADR-198]
tags: [ruview, skygraph, adsb, weather, sensing, edge-ai, anomaly, worldgraph, ruvector-core, ruvector-graph, projection, appliance]
---

# ADR-199 — Sky Monitor and SkyGraph Appliance

## Status

**Proposed.** Owner: Reuven Cohen. Project: RuView Sky Monitor.
Date: 2026-06-09.

A reference implementation of Phases 1–4 (with synthetic ADS-B data) lives at
`examples/sky-monitor/` in this repository, built on `crates/ruvector-core` and
`crates/ruvector-graph` (see §13 and §29).

## 1. Decision

Build a **local sky monitoring appliance** that observes, projects, records, and
explains activity above a fixed physical location.

The appliance starts with ADS-B aircraft tracking, weather overlays, and a
projector display. It then evolves into a multi-sensor **SkyGraph** that
correlates aircraft, weather, RF activity, satellites, acoustic events, camera
observations, and environmental signals.

The core decision: **treat the sky as a continuously changing spatial graph, not
a dashboard.** The appliance ingests live signals, normalizes them into common
spatial and temporal coordinates, stores observations in RuVector plus graph
storage, and exposes a local assistant that can answer questions like:

- "What aircraft crossed overhead today?"
- "Was that sound correlated with an aircraft?"
- "What changed versus normal Tuesday patterns?"

## 2. Context

The required upstream signals and decoders already exist and are publicly
accessible:

- **ADS-B / Mode S state vectors** carry position, velocity, and identity for
  aircraft broadcasting in the clear at 1090 MHz.
- **OpenSky Network API** provides live state vectors over HTTP, usable for
  bootstrap, comparison, and gap filling.
- **dump1090** is a mature Mode S decoder for cheap RTL-SDR receivers, exposing
  decoded aircraft state as local JSON.
- **MSC GeoMet APIs** publish Canadian weather (ECCC): radar, precipitation,
  wind, visibility, and alerts.

The value here is *not* another flight tracker. It is **local ambient
intelligence** that learns the sky above a specific property, building, event,
or city zone — a persistent, queryable, observer-centric model of "above us".

## 3. Problem

Current tools show individual streams: flight trackers show aircraft, weather
apps show radar, satellite apps show passes, RF tools show spectrum, cameras
show frames. None of them create a **persistent local model of the sky** that
explains relationships across time, space, signals, and events.

The missing layer is a **governed sensing harness** that converts raw
observations into a queryable intelligence substrate — with provenance,
retention policy, and explanation, not just pixels.

## 4. Goals

1. **Live visual sky display** — observer-relative aircraft and weather, on a
   dashboard or projector.
2. **Local history of sky events** — replayable, indexed, durable.
3. **Anomaly detection** — unusual paths, low-altitude passes, RF patterns,
   rare weather, acoustic signatures.
4. **Cross-signal correlation** — aircraft ↔ audio ↔ weather ↔ RF ↔ camera.
5. **Local-first** — fully functional without cloud; cloud is optional.
6. **Field appliance for RuView / RuVector** — a concrete testbed for ambient
   sensing, vector memory, WorldGraph, and edge intelligence.

## 5. Non-Goals

- Not air traffic control.
- Not safety-critical.
- Not a replacement for certified aviation, weather, or emergency systems.
- Not personal surveillance.
- **Never transmits** — receive-only sensors plus licensed public APIs only.

## 6. Architecture Decision

The appliance is an **event-driven local system**. Every observation is indexed
three ways:

1. **Time index** — enables replay, sliding windows, and trails.
2. **Spatial index** — local observer coordinates: azimuth, elevation, range.
3. **Semantic vector index** — enables similarity, anomaly scoring,
   explanation, and long-term memory.

Four planes structure the system:

| Plane | Responsibility |
|-------|----------------|
| **Sensor plane** | Receive raw signals (ADS-B, weather API, camera, audio, RF, …) |
| **Normalization plane** | Align time, convert coordinates, score confidence, resolve entities |
| **Intelligence plane** | Track stitching, anomaly detection, correlation, pattern learning, NL query |
| **Presentation plane** | Dashboard, projector, replay, timeline, alerts, daily brief |

## 7. Reference Architecture

```
Sensors
├── ADS-B receiver (RTL-SDR + dump1090)
├── OpenSky API (fallback / comparison)
├── Weather API (MSC GeoMet)
├── Camera (optional)
├── Microphone / mic array (optional)
├── SDR spectrum scan (optional)
├── BLE / WiFi environmental sensing (optional)
└── Satellite pass prediction (optional, TLE)
        │
        ▼
Collectors
├── adsb-collector
├── weather-collector
├── camera-collector
├── audio-collector
├── rf-collector
└── satellite-collector
        │
        ▼
Normalization
├── timestamp alignment (UTC, monotonic drift correction)
├── coordinate conversion (WGS-84 → ECEF → ENU → az/el/range)
├── confidence scoring
├── entity resolution (icao24, track ids, weather cells)
└── sensor calibration profiles
        │
        ▼
Storage
├── time-series store (metrics, state vectors)
├── object store (raw frames, audio clips, IQ snippets)
├── graph store (SkyGraph nodes + edges)
├── RuVector index (embeddings)
└── raw archive (hash-chained)
        │
        ▼
Intelligence
├── track stitching
├── anomaly detection
├── causal correlation
├── pattern learning
├── natural-language query
└── local assistant
        │
        ▼
Presentation
├── web dashboard
├── HDMI projector output
├── sky overlay (observer-relative)
├── replay
├── timeline
└── alerts + daily brief
```

## 8. Deployment Target

| Component | Choice | Notes |
|-----------|--------|-------|
| Compute | Orange Pi 5 Plus or Raspberry Pi 5 | ARM SBC, low power, NVMe-capable |
| SDR | RTL-SDR v4 or Airspy Mini | 1090 MHz reception |
| Antenna | 1090 MHz outdoor antenna | Roofline/mast mount, short low-loss feed |
| Storage | 1 TB NVMe | Tiered retention (see §18) |
| Display | HDMI projector or monitor | Browser kiosk mode |
| Camera | USB or CSI camera | Optional, disabled by default (see §16) |
| Audio | USB mic array | Optional |
| Network | Ethernet preferred | Local-only admin network |
| Optional | Intel BE200 (WiFi sensing), LoRa module | Later phases |

**v1 prioritizes stable ADS-B reception + weather over everything else.**

## 9. Data Sources

### 9.1 ADS-B local (primary)

dump1090 JSON output from local Mode S decode; state vectors yield: identity
(icao24, callsign, squawk), lat/lon, barometric altitude, ground speed, track
angle, vertical rate, signal strength, last-seen timestamp.

### 9.2 OpenSky API (fallback)

Used for: bootstrap before the antenna is installed, comparison against local
decode (coverage QA), gap filling when local reception drops, and historical
enrichment.

### 9.3 MSC GeoMet (Canadian weather, ECCC)

Radar overlay, precipitation type, wind, visibility, official alerts, and
storm-event correlation against aircraft/audio observations.

### 9.4 Optional sources (later phases)

TLE-based satellite pass prediction, camera observations, audio events, RF
spectrum events, and WiFi/BLE **environmental sensing only — never personal
identification** (see §16).

## 10. Coordinate Model

Inputs: observer lat/lon/alt, aircraft lat/lon/alt, timestamp.
Outputs: range, bearing, elevation angle, azimuth, apparent screen position,
confidence.

Pipeline: **WGS-84 → ECEF → ENU → azimuth/elevation/range** (observer-centric
local tangent plane).

### Projector calibration

| Level | Requirements |
|-------|--------------|
| **Minimum** | North reference, horizon line, field of view, one alignment point, observer position |
| **Better** | Star or known-aircraft anchor points, continuous refinement, persisted per-setup calibration profile |

## 11. Canonical Observation Schema

Every normalized observation, from any sensor, conforms to one schema:

```json
{
  "observation_id": "uuid",
  "timestamp_utc": "2026-06-09T19:00:00Z",
  "source": "adsb_local",
  "sensor_id": "sky_node_001",
  "entity_type": "aircraft",
  "entity_id": "icao24_or_internal_id",
  "location": { "lat": 43.4675, "lon": -79.6877, "alt_m": 1200 },
  "observer_frame": { "range_m": 8500, "azimuth_deg": 72.4, "elevation_deg": 8.2, "bearing_deg": 72.4 },
  "motion": { "speed_mps": 210, "track_deg": 247, "vertical_rate_mps": -3.1 },
  "attributes": { "callsign": "ACA123", "squawk": "1234", "signal_dbfs": -18.4 },
  "confidence": 0.92,
  "raw_ref": "object_store_key",
  "embedding_ref": "ruvector_key"
}
```

`raw_ref` links every insight back to evidence; `embedding_ref` links every
observation into vector memory.

## 12. SkyGraph Model

### Nodes

| Node type | Meaning |
|-----------|---------|
| `Aircraft` | A resolved physical aircraft (icao24 or internal id) |
| `Track` | A stitched flight path segment through local airspace |
| `WeatherCell` | A radar/precipitation/wind cell over a time window |
| `RFEvent` | A detected RF spectrum event |
| `AudioEvent` | A detected acoustic event |
| `CameraEvent` | A detected visual event |
| `Satellite` | A predicted or observed satellite pass |
| `Observer` | A physical sensor node (location + calibration) |
| `TimeWindow` | A bounded interval used for grouping and replay |
| `Anomaly` | A scored deviation from baseline |

### Edges

| Edge type | Meaning |
|-----------|---------|
| `observed_by` | Entity ← sensor/observer that produced the observation |
| `near` | Spatial proximity in the observer frame |
| `during` | Membership in a TimeWindow |
| `correlated_with` | Cross-signal correlation (e.g. audio ↔ aircraft) |
| `caused_candidate` | Hypothesized causal link (never asserted as fact) |
| `part_of_track` | Observation → stitched Track |
| `similar_to` | Vector-similarity link between embeddings |
| `anomalous_relative_to` | Anomaly → the baseline it deviates from |

## 13. RuVector Usage

RuVector provides the semantic memory layer:

- **Similarity search** — "find tracks like this one."
- **Anomaly detection** — distance from local historical neighborhoods.
- **Explanatory retrieval** — pull the precedents the assistant cites.
- **Compression** — condense time windows into semantic memory summaries.
- **Contrastive separation** — keep normal vs unusual well-separated in
  embedding space.

Example embeddings:

| Embedding | Encodes |
|-----------|---------|
| Aircraft-track | Path shape, speed profile, altitude profile, time of day, route class |
| Weather-window | Radar/precip/wind state over a window |
| Audio-event | Spectral signature of an acoustic event |
| RF-event | Spectrum shape, power, recurrence |
| Scene | Fused snapshot of the sky state in a window |

### Mapping to actual crates in this repository

| Role | Crate | Notes |
|------|-------|-------|
| Vector store + ANN search | `crates/ruvector-core` | `VectorDB`, HNSW index, `DistanceMetric` |
| SkyGraph storage | `crates/ruvector-graph` | `GraphDB` property graph with a Cypher subset for node/edge queries |
| Browser-side search | `crates/ruvector-wasm`, `crates/micro-hnsw-wasm` | Dashboard-local similarity without round trips |
| Future intelligence plane | `crates/ruvector-attention`, `crates/ruvector-gnn` | Attention over windows; GNN reasoning over the SkyGraph |

## 14. Rule Layer

Rules give **auditability**; pattern learning gives **flexibility**. Use both —
rules gate actions and produce explainable triggers; learned similarity ranks
and contextualizes. Example rules:

1. **Overhead candidate**: range < 10 km AND elevation > 5° → mark
   `overhead_candidate`.
2. **Audio ↔ aircraft correlation**: audio event within 30 s of an aircraft
   closest approach AND aircraft altitude < 3000 m → create `correlated_with`
   edge.
3. **Weather suppression**: active weather alert → suppress weak audio
   anomalies (storm noise floor).
4. **Track deviation**: track deviates > 3σ from its historical corridor →
   create `Anomaly` candidate.
5. **Recurring RF**: an uncorrelated RF event that recurs → escalate for
   review.

## 15. Anomaly Scoring

Composite score:

```
anomaly_score = 0.30 * route_deviation
              + 0.20 * altitude_deviation
              + 0.15 * time_of_day_rarity
              + 0.15 * signal_unusualness
              + 0.10 * cross_sensor_confirmation
              + 0.10 * novelty_score
```

Interpretation bands:

| Score | Interpretation | Action |
|-------|----------------|--------|
| 0.00–0.30 | Normal | Store |
| 0.31–0.55 | Mildly unusual | Timeline marker |
| 0.56–0.75 | Interesting | Include in summary |
| 0.76–0.90 | Strong anomaly | Local alert |
| 0.91–1.00 | Rare | Preserve raw data + generate report |

## 16. Privacy and Governance

- No face recognition. No person identification. No license-plate reading.
- Local storage by default; configurable retention per data class.
- Camera redaction (when camera is enabled at all — disabled by default).
- Maintained **sensor inventory** — every active sensor is declared.
- Query and export **audit log**.
- **Location generalization** on any shared output.
- **Synthetic or delayed data** for public demos.

## 17. Security

### Threats

API key leakage; poisoned external data (OpenSky/weather); sensor spoofing
(fake ADS-B); untrusted network access; location-history inference; event-history
tampering; malicious dashboard access.

### Controls

- Local-only admin network.
- Read-only sensor containers.
- Signed event batches.
- Hash-chained raw archives (tamper-evident, cf. the witness chain in ADR-198).
- Public vs admin dashboard separation.
- No cloud sync by default.
- Secrets kept outside the repository.
- Assistant rate limits.
- RBAC for export and delete operations.
- Daily integrity check over the hash chains.

## 18. Storage Design

| Store | Contents | Retention |
|-------|----------|-----------|
| Raw archive | dump1090 frames, audio clips, IQ snippets | 7–30 d normal; 180 d interesting; manual hold for rare events |
| Time series | Aircraft count, message rate, signal strength, wind, precipitation, RF power, audio amplitude | Long-lived, compact |
| Graph store | SkyGraph nodes + edges | Long-lived |
| RuVector | Embeddings + semantic summaries | Long-lived |
| Reports | Daily briefs, anomaly reports | Long-lived |

## 19. Services

| Service | Responsibilities |
|---------|------------------|
| **adsb-collector** | Poll dump1090 JSON; decode state vectors; emit raw + normalized observations; fall back to OpenSky; tag source and confidence |
| **weather-collector** | Poll MSC GeoMet; normalize radar/precip/wind/alerts into WeatherCell observations; align frames to local time windows |
| **projection-engine** | WGS-84 → ECEF → ENU → az/el/range conversion; calibration profiles; apparent screen positions for dashboard/projector |
| **skygraph-builder** | Track stitching; entity resolution; node/edge creation; rule-layer evaluation (§14); TimeWindow management |
| **ruvector-indexer** | Compute embeddings; insert into `ruvector-core` `VectorDB`; maintain similarity/novelty links; window compression into semantic memory |
| **assistant-service** | NL query over SkyGraph + RuVector; answer with cited observation ids; enforce governance rules (§27); rate-limited |

## 20. APIs

```
GET  /v1/sky/events?start={iso}&end={iso}     # observations in a window
GET  /v1/sky/aircraft/overhead                # current overhead candidates
GET  /v1/sky/anomalies                        # scored anomalies
GET  /v1/sky/replay/{window_id}               # replay a stored window
POST /v1/sky/query                            # natural-language assistant
```

Example assistant exchange:

```json
// POST /v1/sky/query
{ "question": "What flew over the house around 9 pm?" }
```

```json
{
  "answer": "One aircraft passed overhead at 21:14 local: an eastbound track at unusually low altitude (1180 m). A loud audio event 11 seconds after closest approach is correlated with this track. Weather was calm with no precipitation.",
  "cited_observations": ["aircraft_track_123", "audio_event_456", "weather_window_789"],
  "confidence": 0.84
}
```

## 21. UX

### Live mode

Aircraft dots with callsigns, altitude, and direction; motion trails; weather
overlay; alert badge; RF activity indicator; audio event markers.

### Replay mode

Scrub any stored TimeWindow; trails and correlations replay in observer frame.

### Daily brief (sample — Oakville node)

> **Sky brief — Oakville, 2026-06-09.** 812 aircraft observed; 37 overhead
> candidates; 4 unusual tracks. Light rain 14:10–15:30. 2 RF anomalies. Most
> unusual event: low-altitude eastbound pass at 21:14 (confidence 0.78).

## 22. Build Plan

| Phase | Scope | Inputs | Outputs | Acceptance |
|-------|-------|--------|---------|------------|
| **1** | ADS-B sky display | dump1090 JSON (OpenSky fallback) | Live observer-relative display | Aircraft appear within 5 s of decode; azimuth within 10°; 24 h uptime |
| **2** | Weather context | MSC GeoMet | Weather overlay + alert suppression context | Radar overlay aligned in time and frame with aircraft |
| **3** | SkyGraph | Normalized observations | Graph store + rule layer | Query by time window; query "near observer"; explain an unusual track |
| **4** | RuVector memory | SkyGraph + embeddings | Similarity, novelty, assistant | Similar tracks plausible; novelty separates commercial routes from unusual paths; assistant cites event ids |
| **5** | Audio / RF / camera | Optional sensors | Cross-signal correlation | Correlated_with edges with confidence; governance controls active |

## 23. Technology Choices

| Concern | Choice | Notes |
|---------|--------|-------|
| Collectors | Rust | Reliability, low footprint on SBC |
| UI rendering | WebGL / Canvas | Browser-native sky overlay |
| Local API | Axum | Rust HTTP service |
| Time series | SQLite / DuckDB | Embedded, zero-ops |
| Graph | SQLite tables first, then graph engine | In this repo: `crates/ruvector-graph` (`GraphDB`) |
| Vectors | RuVector | `crates/ruvector-core` (`VectorDB`, HNSW) |
| Messaging | NATS or local channels | Local channels for v1 |
| Containers | Podman / Docker | Read-only sensor containers |
| Display | Browser kiosk over HDMI | Projector and monitor both |

## 24. Decision Matrix

Scores 1 = weak … 5 = strong.

| Option | Cost | Latency | Control | Complexity | Strategic fit |
|--------|------|---------|---------|------------|---------------|
| OpenSky only | 5 | 3 | 2 | 5 | 3 |
| Local ADS-B only | 4 | 5 | 5 | 3 | 4 |
| Local ADS-B + OpenSky | 4 | 5 | 5 | 4 | 5 |
| Full sensor fusion day one | 2 | 4 | 5 | 1 | 2 |
| **Phased SkyGraph appliance** | **5** | **5** | **5** | **4** | **5** |

**Decision: phased SkyGraph appliance — local ADS-B first, OpenSky as
fallback.**

## 25. Key Tradeoffs

| Tradeoff | Resolution |
|----------|------------|
| Local receiver vs API | Both; **local is the source of truth**, API is bootstrap/fallback/QA |
| Graph-first vs vector-first | Graph for facts and provenance; RuVector for similarity, novelty, and memory |
| Projector vs dashboard | **Dashboard first**; projector is a calibrated presentation mode on top |
| Edge-only vs cloud-assisted | **Edge first**; cloud strictly optional |

## 26. Failure Modes

| Failure mode | Mitigation |
|--------------|------------|
| Wrong sky position on display | Calibration wizard (north, horizon, FoV, anchor point) |
| Missing aircraft | Antenna placement review + OpenSky comparison for coverage QA |
| False anomalies | Require ≥ 14 days of baseline before alerting |
| Weather overlay mismatch | Normalize time and coordinate frames across sources |
| Audio false positives | Weather and time-of-day filters |
| RF noise overload | Event compression + thresholds |
| Privacy concern raised | Disable person-related analysis; redact; camera off by default |
| Storage growth | Tiered retention (§18) |

## 27. Governance Rules

1. Every insight links back to raw and normalized observations.
2. Every anomaly includes a stated reason.
3. Assistant answers distinguish **fact**, **inference**, and **uncertainty**.
4. Every sensor appears in the sensor inventory.
5. Every external API call is logged.
6. Every retained camera/audio event has an explicit retention policy.
7. Exported reports remove precise location unless explicitly enabled.

## 28. Example Event Explanation

The 21:14 loud-aircraft event, as the assistant should explain it:

> An aircraft was 7.8 km east at 1180 m altitude (8.4° elevation). The audio
> event occurred 11 seconds after closest approach. Weather was calm — no
> thunder. No RF anomaly in the window. **Conclusion: likely aircraft related
> (confidence 0.82).** Uncertainty: aircraft type unknown; no camera
> confirmation available.

This is the product bar: cited evidence, stated mechanism, explicit
uncertainty — not a dot on a map.

## 29. Repository Layout

A reference implementation exists in this repository at
`examples/sky-monitor/`:

- a SkyGraph **core pipeline** crate (collectors → normalization → graph →
  anomaly scoring),
- a **WASM projection engine** (WGS-84 → ECEF → ENU → az/el/range),
- a **Canvas dashboard** (observer-relative live + replay views),
- **Criterion benches** for the projection and indexing hot paths.

It implements **Phases 1–4 with synthetic ADS-B data**, storing vectors in
`crates/ruvector-core` and the SkyGraph in `crates/ruvector-graph`. Phase 5
sensors (audio/RF/camera) and live dump1090 ingestion are the appliance
deployment steps on top of it.

## 30. Configuration Sketch

```toml
[observer]
name = "oakville_node"
latitude = 43.4675
longitude = -79.6877
altitude_m = 100
[adsb]
mode = "local_plus_opensky"
dump1090_url = "http://localhost:8080/data/aircraft.json"
opensky_enabled = true
[weather]
provider = "msc_geomet"
country = "CA"
[projection]
mode = "dashboard_first"
field_of_view_deg = 90
north_offset_deg = 0
[privacy]
camera_enabled = false
audio_retention_days = 7
raw_retention_days = 14
precise_location_export = false
[anomaly]
min_history_days = 14
alert_threshold = 0.76
```

## 31. Acceptance Tests

### System acceptance

1. Receive local ADS-B messages.
2. Convert state vectors to azimuth/elevation/range.
3. Display aircraft observer-relative.
4. Store raw + normalized observations.
5. Replay a 30-minute window.
6. Answer "what flew overhead" for a given period.
7. Generate a daily brief.
8. Flag unusual tracks after the baseline period.
9. Cite observations in every assistant answer.
10. Run fully without cloud connectivity.

### Business value acceptance

1. A demo is understandable to a non-technical viewer in under 60 seconds.
2. It feels different from a flight tracker.
3. The assistant **explains**, not merely displays.
4. The architecture extends to elder care, event safety, municipal sensing,
   and Arista-style edge deployments.
5. The primitives are reusable for RuView ambient intelligence.

## 32. Open Questions (with recommendations)

| Question | Recommendation |
|----------|----------------|
| Browser-first UI? | **Yes** — kiosk browser covers monitor and projector |
| Camera/audio in v1? | **No** — Phase 5; privacy posture first |
| Cognitum One edge demo? | **Yes** — natural showcase deployment |
| Is RuVector required in v1? | **Yes** for summaries/similarity; **not** for basic display |
| Arista integration? | **Separate ADR** |

## 33. Consequences

### Positive

- A concrete RuView field appliance — hardware + software + governance.
- Local intelligence that goes beyond chat: it perceives, remembers, explains.
- RuVector exercised on real-world temporal sensing data.
- A strong ambient-AI demonstration.
- Reusable primitives: observation schema, projection engine, SkyGraph,
  anomaly scoring, governed assistant.

### Negative

- Sensor fusion complexity grows with each phase.
- Projector calibration friction.
- Raw data growth pressure on edge storage.
- False anomalies during the baseline period.
- External API coverage limits (OpenSky rate limits, GeoMet geography).

### Mitigation (phased path)

Aircraft + weather → graph → RuVector → projector → audio/RF/camera. Each phase
is independently useful and independently demonstrable.

## 34. Final Recommendation

Build this as the **RuView SkyGraph Appliance**. Do not position it as a flight
tracker; position it as **local intelligence for the atmosphere above you**:

> **See the sky. Remember the sky. Explain the sky.**

It is a real-world testbed for edge AI, vector memory, sensor fusion, anomaly
detection, and governed local agents.
