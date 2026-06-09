# SkyGraph all-sky dashboard (ADR-199 presentation plane)

Vanilla JS + Canvas, no build tooling. Renders the embedded deterministic
scenario (`sky-demo-data.js`) on a polar all-sky plot: zenith at the centre,
horizon at the edge, azimuth 0° = North = up. Aircraft are dots with callsign
+ altitude labels and fading trails; overhead candidates get a blue highlight
ring; anomaly badges are colored by band (normal / mildly unusual /
interesting / strong anomaly / rare, gray = unscored baseline). The side panel
lists tracks (click to select + jump the replay) and shows the §15 anomaly
reasons; the footer scrubber replays the synthetic day at 60×.

## Serve

```bash
# from this directory (ES modules need http://, not file://)
python3 -m http.server 8000
# open http://localhost:8000/
```

## Regenerate the demo data

```bash
# from the repository root
cargo run -p sky-monitor --release -- --emit-json examples/sky-monitor/ui/dashboard/sky-demo-data.js
```

## Optional: wasm projection engine

`sky.js` does the WGS-84 → az/el/range projection in plain JS (mirroring
`src/coords.rs`). If the wasm-pack output exists at `./pkg/`, it is detected
and preferred automatically (`SkyProjector.project_batch`):

```bash
# from the repository root
wasm-pack build examples/sky-monitor/wasm --target web --out-dir ../ui/dashboard/pkg
```

The header shows which engine is active (`projection: wasm …` vs
`projection: JS fallback`). Without `./pkg` the dashboard is fully functional
on the JS fallback.
