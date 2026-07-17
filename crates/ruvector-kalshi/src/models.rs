//! Kalshi REST + WebSocket DTOs. Only the fields we actually consume are
//! declared; everything else is ignored by `#[serde(deny_unknown_fields)]`
//! being *off* — we intentionally accept forward-compatible payloads.

use serde::{Deserialize, Deserializer, Serialize};

// ---------------------------------------------------------------------------
// 2026-01-28 fixed-point-migration parse helpers.
//
// Kalshi migrated wire prices from integer cents (`yes_bid: i64`) to dollar
// strings (`yes_bid_dollars: "0.5200"`) and contract counts from integers to
// fixed-point strings (`initial_count_fp: "1.00"`). SONA's internal types stay
// integer-cents / integer-contracts (ADR-019 fixed-point canon), so the crate
// boundary converts the strings here and downstream code is unchanged.
// ---------------------------------------------------------------------------

/// Parse a Kalshi dollar-string price (e.g. `"0.5200"`) into integer cents.
/// Whole-cent prices are exact; the extra fractional digits are formatting.
/// Returns `None` for empty/malformed input.
pub fn dollars_str_to_cents(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let neg = s.starts_with('-');
    let body = s.trim_start_matches(['-', '+']);
    let (whole, frac) = body.split_once('.').unwrap_or((body, ""));
    let whole: i64 = whole.parse().ok()?;
    let mut digits = frac.chars();
    let d0 = digits.next().and_then(|c| c.to_digit(10)).unwrap_or(0) as i64;
    let d1 = digits.next().and_then(|c| c.to_digit(10)).unwrap_or(0) as i64;
    let d2 = digits.next().and_then(|c| c.to_digit(10)).unwrap_or(0) as i64;
    let mut cents = whole * 100 + d0 * 10 + d1;
    if d2 >= 5 {
        cents += 1; // round half-up on the third fractional digit
    }
    Some(if neg { -cents } else { cents })
}

/// Parse a Kalshi fixed-point count string (e.g. `"1.00"`, `"10.00"`) into an
/// integer contract count. Contracts are whole; the `.00` is formatting.
pub fn count_fp_str_to_contracts(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    s.split_once('.').map(|(w, _)| w).unwrap_or(s).parse().ok()
}

/// Encode integer cents (0..=100 for a Kalshi binary contract) as a V2
/// fixed-point US-dollar price string: `56 -> "0.56"`, `100 -> "1.00"`. Clamps
/// to the binary-contract range so a malformed input can never serialize a
/// negative or out-of-band price onto the money path.
pub fn cents_to_dollars_str(cents: i64) -> String {
    let c = cents.clamp(0, 100);
    format!("{}.{:02}", c / 100, c % 100)
}

/// Encode an integer contract count as a V2 fixed-point count string:
/// `1 -> "1.00"`, `10 -> "10.00"`.
pub fn contracts_to_count_fp(count: i64) -> String {
    format!("{}.00", count.max(0))
}

/// Deserialize a Kalshi orderbook side — `[[priceDollarsStr, countFpStr], …]`
/// — into `Vec<[i64; 2]>` of `[price_cents, contracts]`, preserving the
/// internal `[i64; 2]` shape `normalize::orderbook_to_events` consumes.
fn de_price_count_pairs<'de, D>(d: D) -> Result<Vec<[i64; 2]>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Vec<[String; 2]> = Vec::deserialize(d)?;
    let mut out = Vec::with_capacity(raw.len());
    for [p, c] in raw {
        let price = dollars_str_to_cents(&p).ok_or_else(|| {
            <D::Error as serde::de::Error>::custom(format!("unparseable price {p:?}"))
        })?;
        let count = count_fp_str_to_contracts(&c).ok_or_else(|| {
            <D::Error as serde::de::Error>::custom(format!("unparseable count {c:?}"))
        })?;
        out.push([price, count]);
    }
    Ok(out)
}

/// Deserialize an OPTIONAL Kalshi dollar-string price (e.g. `"0.750"`) into
/// `Option<i64>` integer cents via [`dollars_str_to_cents`]. A JSON `null` →
/// `None`; a present string → `Some(cents)`; an unparseable string is a HARD
/// decode error — a malformed price must never silently become `None` on the
/// money path. Pair the field with `#[serde(default)]` so an ABSENT key is
/// `None` without invoking this fn (serde only calls a `deserialize_with` for a
/// present field).
fn de_opt_dollars_cents<'de, D>(d: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(d)?;
    match opt {
        None => Ok(None),
        Some(s) => dollars_str_to_cents(&s).map(Some).ok_or_else(|| {
            <D::Error as serde::de::Error>::custom(format!("unparseable price {s:?}"))
        }),
    }
}

/// Deserialize a REQUIRED Kalshi dollar-string price into integer cents via
/// [`dollars_str_to_cents`]. Unparseable input is a hard decode error.
fn de_dollars_cents<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    dollars_str_to_cents(&s)
        .ok_or_else(|| <D::Error as serde::de::Error>::custom(format!("unparseable price {s:?}")))
}

/// Deserialize a Kalshi fixed-point count string (`"278.00"`, or a SIGNED
/// `"-54.00"` for an orderbook delta) into an integer contract count via
/// [`count_fp_str_to_contracts`] (which retains the leading sign). Unparseable
/// input is a hard decode error.
fn de_count_fp_i64<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    count_fp_str_to_contracts(&s)
        .ok_or_else(|| <D::Error as serde::de::Error>::custom(format!("unparseable count {s:?}")))
}

/// Market metadata (GET /markets, /markets/{ticker}).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Market {
    pub ticker: String,
    pub event_ticker: Option<String>,
    pub title: Option<String>,
    pub status: Option<String>,
    // Legacy integer-cent price fields. Post the 2026-01-28 fixed-point
    // migration these are present-but-`null` on the live API; the real values
    // live in the `*_dollars` strings below. Read via the `*_cents()` helpers,
    // which prefer the dollar string and fall back to these.
    pub yes_bid: Option<i64>,
    pub yes_ask: Option<i64>,
    pub no_bid: Option<i64>,
    pub no_ask: Option<i64>,
    pub last_price: Option<i64>,
    pub volume: Option<i64>,
    pub open_interest: Option<i64>,
    pub close_time: Option<String>,
    pub expiration_time: Option<String>,
    // Post-migration dollar-string prices (the live source of truth).
    pub yes_bid_dollars: Option<String>,
    pub yes_ask_dollars: Option<String>,
    pub no_bid_dollars: Option<String>,
    pub no_ask_dollars: Option<String>,
    pub last_price_dollars: Option<String>,
}

impl Market {
    /// Yes-bid in integer cents, preferring the post-migration dollar string
    /// and falling back to the legacy integer field.
    pub fn yes_bid_cents(&self) -> Option<i64> {
        self.yes_bid_dollars
            .as_deref()
            .and_then(dollars_str_to_cents)
            .or(self.yes_bid)
    }
    /// Yes-ask in integer cents (see [`Market::yes_bid_cents`]).
    pub fn yes_ask_cents(&self) -> Option<i64> {
        self.yes_ask_dollars
            .as_deref()
            .and_then(dollars_str_to_cents)
            .or(self.yes_ask)
    }
    /// No-bid in integer cents (see [`Market::yes_bid_cents`]).
    pub fn no_bid_cents(&self) -> Option<i64> {
        self.no_bid_dollars
            .as_deref()
            .and_then(dollars_str_to_cents)
            .or(self.no_bid)
    }
    /// No-ask in integer cents (see [`Market::yes_bid_cents`]).
    pub fn no_ask_cents(&self) -> Option<i64> {
        self.no_ask_dollars
            .as_deref()
            .and_then(dollars_str_to_cents)
            .or(self.no_ask)
    }
    /// Last-trade price in integer cents (see [`Market::yes_bid_cents`]).
    pub fn last_price_cents(&self) -> Option<i64> {
        self.last_price_dollars
            .as_deref()
            .and_then(dollars_str_to_cents)
            .or(self.last_price)
    }
}

/// Envelope for list-markets response.
#[derive(Debug, Clone, Deserialize)]
pub struct MarketsResponse {
    pub markets: Vec<Market>,
    pub cursor: Option<String>,
}

/// Single trade print (GET /markets/trades).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KalshiTrade {
    pub ticker: String,
    pub trade_id: String,
    pub yes_price: Option<i64>,
    pub no_price: Option<i64>,
    pub count: i64,
    pub taker_side: Option<String>, // "yes" | "no"
    pub created_time: String,       // ISO-8601
}

/// Orderbook snapshot (GET /markets/{ticker}/orderbook).
///
/// Post the 2026-01-28 fixed-point migration the live wire keys are
/// `yes_dollars`/`no_dollars` and each entry is `[priceDollarsStr, countFpStr]`
/// (e.g. `["0.5200", "6.00"]`). The custom deserializer converts to the
/// internal `[price_cents, contracts]` `[i64; 2]` shape so downstream
/// normalizers are unchanged.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrderbookSnapshot {
    /// Each entry is `[price_cents, contracts]`.
    #[serde(
        rename = "yes_dollars",
        deserialize_with = "de_price_count_pairs",
        default
    )]
    pub yes: Vec<[i64; 2]>,
    #[serde(
        rename = "no_dollars",
        deserialize_with = "de_price_count_pairs",
        default
    )]
    pub no: Vec<[i64; 2]>,
}

/// Wrapper for orderbook GET envelope. The live wrapper key migrated from
/// `orderbook` to `orderbook_fp`.
#[derive(Debug, Clone, Deserialize)]
pub struct OrderbookResponse {
    #[serde(rename = "orderbook_fp")]
    pub orderbook: OrderbookSnapshot,
}

// ---------------------------------------------------------------------------
// V2 order-mutation DTOs (ADR-018 §Amendment 2026-06-19).
//
// Kalshi deprecated the legacy `/portfolio/orders*` mutation endpoints (June
// 18-25 2026) for V2 `/portfolio/events/orders*`. The V2 request is a single-
// book `side: bid|ask` quoted off the YES leg, with fixed-point STRING `count`
// + dollar `price`, a required `time_in_force`, and a required
// `self_trade_prevention_type` (SONA sends `taker_at_cross` -- issue #63 D2,
// venue-native self-cross block). V2 create returns a LIGHTWEIGHT flat ack
// (`order_id` + fill/remaining counts), NOT the legacy `{"order":{…}}`; the
// full order is read back via GET `/portfolio/orders/{id}` ([`OrderAck`]).
// ---------------------------------------------------------------------------

/// Single-book order side (V2). Quoted off the YES leg: `bid` = buy YES,
/// `ask` = sell YES. A NO order is expressed on the YES book at the
/// complementary price: Buy NO @ q -> `ask` @ (1-q); Sell NO @ q -> `bid` @ (1-q).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum V2Side {
    Bid,
    Ask,
}

/// V2 `time_in_force` -- serializes to the exact Kalshi strings
/// (`fill_or_kill` / `good_till_canceled` / `immediate_or_cancel`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimeInForce {
    FillOrKill,
    GoodTillCanceled,
    ImmediateOrCancel,
}

/// V2 `self_trade_prevention_type`. `taker_at_cross` cancels the incoming
/// (taker) order if it would cross our own resting order (issue #63 D2);
/// `maker` cancels the resting maker order instead.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelfTradePreventionType {
    TakerAtCross,
    Maker,
}

/// V2 create-order request body (POST /portfolio/events/orders). `count` and
/// `price` are fixed-point STRINGS per the 2026-01-28 migration; there is no
/// `type` field and no `outcome_side` (the YES/NO leg is encoded in `side`
/// plus the complementary `price`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct V2CreateOrderRequest {
    pub ticker: String,
    pub side: V2Side,
    /// Fixed-point contract-count string, e.g. `"10.00"`.
    pub count: String,
    /// Fixed-point US-dollar price string, e.g. `"0.5600"`.
    pub price: String,
    pub time_in_force: TimeInForce,
    pub self_trade_prevention_type: SelfTradePreventionType,
    /// REQUIRED by the V2 contract — Kalshi's idempotency / dedup key. Never
    /// omit it (an omitted key risks a duplicate live order).
    pub client_order_id: String,
}

/// V2 create-order response -- a LIGHTWEIGHT flat ack (NOT the legacy
/// `{"order":{…}}`). Only `order_id` is guaranteed; the rest track fill state.
/// SONA hashes `order_id` into the witness receipt; the full order is read
/// back via [`crate::rest::RestClient::get_order`] when the resting state is
/// needed.
#[derive(Debug, Clone, Deserialize)]
pub struct V2OrderResponse {
    pub order_id: String,
    pub client_order_id: Option<String>,
    pub fill_count: Option<String>,
    pub remaining_count: Option<String>,
    pub average_fill_price: Option<String>,
    pub average_fee_paid: Option<String>,
    pub ts_ms: Option<i64>,
}

/// Kalshi binary-market contract side (YES/NO) -- the *leg* a SONA strategy
/// trades, orthogonal to the buy/sell action. Drives the [`V2Side`] + price
/// mapping in [`crate::strategy_adapter::intent_to_order`]; not itself a V2
/// wire field.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OrderSide {
    Yes,
    No,
}

/// Full order object wrapped as `{"order": {…}}` -- the response of the GET
/// `/portfolio/orders/{order_id}` read-back (V2 cancel/amend acks no longer
/// echo the order), and the shape the paper simulator synthesizes.
#[derive(Debug, Clone, Deserialize)]
pub struct OrderAck {
    pub order: OrderRecord,
}

/// V2 amend-order request body (POST /portfolio/events/orders/{id}/amend).
/// V2 amend is a full re-specification (ticker + side + price + count), not a
/// delta. Not yet wired into the SONA order path (fork-level only).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct V2AmendOrder {
    pub ticker: String,
    pub side: V2Side,
    pub price: String,
    pub count: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_order_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_client_order_id: Option<String>,
}

/// V2 cancel ack (DELETE /portfolio/events/orders/{id}) -- lightweight; does
/// NOT echo the order (SONA reads the resulting state back via `get_order`).
/// All fields optional for forward-compatible decode.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct V2CancelAck {
    pub order_id: Option<String>,
    pub client_order_id: Option<String>,
    pub reduced_by: Option<String>,
    pub ts_ms: Option<i64>,
}

/// V2 amend ack (POST /portfolio/events/orders/{id}/amend) -- lightweight;
/// does NOT echo the order. All fields optional for forward-compatible decode.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct V2AmendAck {
    pub order_id: Option<String>,
    pub client_order_id: Option<String>,
    pub remaining_count: Option<String>,
    pub fill_count: Option<String>,
    pub average_fill_price: Option<String>,
    pub average_fee_paid: Option<String>,
    pub ts_ms: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrderRecord {
    pub order_id: String,
    pub client_order_id: Option<String>,
    pub status: String,
    pub ticker: String,
    /// Pre-2026-01-28 integer contract count. The fixed-point migration removed
    /// this field from order objects (counts now live in the `*_count_fp`
    /// strings below), so it deserializes to `None` against the live API. It
    /// was previously a required `i64`, which is exactly what broke the order
    /// response decode (the field is gone → "missing field `count`"). SONA
    /// reads only `order_id`; this stays optional for backward-compat decoding.
    pub count: Option<i64>,
    pub filled_count: Option<i64>,
    pub remaining_count: Option<i64>,
    // Post-migration fixed-point count + dollar-price strings. Captured for
    // conformance + future consumers; not read by SONA today.
    pub initial_count_fp: Option<String>,
    pub remaining_count_fp: Option<String>,
    pub fill_count_fp: Option<String>,
    pub yes_price_dollars: Option<String>,
    pub no_price_dollars: Option<String>,
}

impl OrderRecord {
    /// The order's contract count, sourced from the legacy integer `count`
    /// (pre-migration) or parsed from `initial_count_fp` (post-2026-01-28
    /// fixed-point migration, where the integer `count` is absent). Returns 0
    /// if neither is present.
    pub fn contract_count(&self) -> i64 {
        self.count
            .or_else(|| {
                self.initial_count_fp
                    .as_deref()
                    .and_then(count_fp_str_to_contracts)
            })
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Portfolio read DTOs (issue #55 items 7-8). Context7-verified against the live
// Kalshi V2 OpenAPI spec (docs.kalshi.com/openapi.yaml, retrieved 2026-06-23):
//   * GET /portfolio/balance — `balance` + `portfolio_value` are INTEGER CENTS
//     (int64), with a `balance_dollars` fixed-point mirror.
//   * GET /portfolio/positions — `market_positions[]` with a fixed-point
//     `position_fp` count string and `*_dollars` money strings.
//   * GET /portfolio/fills — `fills[]` whose timestamp field is `ts` (int64),
//     NOT `ts_ms`, and whose direction is the canonical `outcome_side`/
//     `book_side` (the legacy `side`/`action` are deprecated). Counts are
//     fixed-point strings, money is dollar strings.
// These are READS — not gated on `KALSHI_ENABLE_LIVE`. SONA converts at the
// crate boundary via the helpers above (`dollars_str_to_cents` /
// `count_fp_str_to_contracts`); only the fields SONA consumes are declared
// (deny_unknown_fields is OFF, so extra payload — `event_positions`,
// `balance_breakdown`, the deprecated mirror fields — is ignored, not an error).
// ---------------------------------------------------------------------------

/// GET /portfolio/balance. `balance` and `portfolio_value` are integer cents
/// (Kalshi keeps these int64-cents even post the 2026-01-28 fixed-point
/// migration); `balance_dollars` is the fixed-point mirror. SONA's pre-flight
/// gate reads `balance` (cents) directly — no string parse on the hot path.
#[derive(Debug, Clone, Deserialize)]
pub struct BalanceResponse {
    /// Member's available balance in integer cents.
    pub balance: i64,
    /// Fixed-point dollar mirror of `balance` (e.g. `"1000.00"`); informational.
    pub balance_dollars: Option<String>,
    /// Portfolio value (all positions) in integer cents.
    pub portfolio_value: i64,
    /// Unix timestamp of the last balance update.
    pub updated_ts: Option<i64>,
}

/// A single market position (GET /portfolio/positions → `market_positions[]`).
/// `position_fp` is a SIGNED fixed-point contract count (`"10.00"` long,
/// `"-3.00"` short); money fields are dollar strings. Parse at the SONA
/// boundary via [`count_fp_str_to_contracts`] / [`dollars_str_to_cents`].
#[derive(Debug, Clone, Deserialize)]
pub struct MarketPosition {
    pub ticker: String,
    /// Signed fixed-point net contract count, e.g. `"10.00"` / `"-3.00"`.
    pub position_fp: Option<String>,
    pub market_exposure_dollars: Option<String>,
    pub total_traded_dollars: Option<String>,
    pub realized_pnl_dollars: Option<String>,
    pub fees_paid_dollars: Option<String>,
    pub last_updated_ts: Option<String>,
}

impl MarketPosition {
    /// Net signed contract count parsed from `position_fp` (whole contracts;
    /// the fractional `.NN` is truncated, matching [`count_fp_str_to_contracts`]).
    /// Returns 0 when `position_fp` is absent or unparseable.
    pub fn position_contracts(&self) -> i64 {
        self.position_fp
            .as_deref()
            .and_then(count_fp_str_to_contracts)
            .unwrap_or(0)
    }
}

/// GET /portfolio/positions envelope. SONA consumes `market_positions` for
/// ramp-state reconciliation; the spec's `event_positions` array is ignored
/// (forward-compat decode). `cursor` is empty-string when there is no next page.
#[derive(Debug, Clone, Deserialize)]
pub struct PositionsResponse {
    pub market_positions: Vec<MarketPosition>,
    pub cursor: Option<String>,
}

/// A single fill (GET /portfolio/fills → `fills[]`). Context7-verified: the
/// timestamp field is `ts` (int64 Unix seconds), NOT `ts_ms` (the issue draft
/// was stale); direction is the canonical `outcome_side` (yes|no) /
/// `book_side` (bid|ask) — the legacy `side`/`action` are deprecated. Counts
/// are fixed-point strings, prices/fees are dollar strings.
#[derive(Debug, Clone, Deserialize)]
pub struct RestFill {
    pub fill_id: String,
    pub order_id: String,
    pub ticker: String,
    /// Fixed-point contract count for this fill, e.g. `"1.00"`.
    pub count_fp: Option<String>,
    pub yes_price_dollars: Option<String>,
    pub no_price_dollars: Option<String>,
    /// Canonical fill direction (`"yes"`|`"no"`) — replaces the deprecated `side`.
    pub outcome_side: Option<String>,
    /// Canonical book side (`"bid"`|`"ask"`).
    pub book_side: Option<String>,
    pub is_taker: Option<bool>,
    pub fee_cost: Option<String>,
    pub created_time: Option<String>,
    /// Unix timestamp (legacy field name `ts`, int64 seconds) — NOT `ts_ms`.
    pub ts: Option<i64>,
}

/// GET /portfolio/fills envelope. `cursor` is empty-string when there is no
/// next page.
#[derive(Debug, Clone, Deserialize)]
pub struct FillsResponse {
    pub fills: Vec<RestFill>,
    pub cursor: Option<String>,
}

/// GET /portfolio/orders envelope (SONA ADR-042 §1 — the halt cancel-sweep's
/// resting-order inventory read). Context7-verified against the live Kalshi V2
/// OpenAPI spec (`docs.kalshi.com/openapi.yaml`, retrieved 2026-07-06):
/// `orders[]` is the same `Order` object the `GET /portfolio/orders/{id}`
/// read-back wraps (decoded here with the existing forward-compat
/// [`OrderRecord`]); `cursor` is required on the wire and empty-string when
/// there is no next page. Resting orders are ALWAYS available on this
/// endpoint (never historical-only), per the spec's historical-cutoff note.
#[derive(Debug, Clone, Deserialize)]
pub struct OrdersResponse {
    pub orders: Vec<OrderRecord>,
    pub cursor: Option<String>,
}

/// Raw Kalshi WS envelope. `{"type": "...", "msg": {...}}`. `msg` is
/// kept as a `Value` so unknown `type` tags don't fail the parse — the
/// decoder routes on `msg_type`.
#[derive(Debug, Clone, Deserialize)]
pub struct WsEnvelope {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub msg: serde_json::Value,
}

/// Typed WS messages routed from [`WsEnvelope`] by the decoder.
#[derive(Debug, Clone)]
pub enum WsMessage {
    Ticker(WsTicker),
    Trade(WsTrade),
    OrderbookSnapshot(WsOrderbook),
    OrderbookDelta(WsOrderbookDelta),
    Fill(WsFill),
    /// Any non-data frame (heartbeat, ack, etc.) or unknown `type` tag.
    Other,
}

impl WsMessage {
    /// Decode an envelope into a typed message. Unknown type tags produce
    /// [`WsMessage::Other`] rather than an error so forward-compatible
    /// payloads don't kill the feed.
    pub fn from_envelope(env: WsEnvelope) -> serde_json::Result<Self> {
        Ok(match env.msg_type.as_str() {
            "ticker" => Self::Ticker(serde_json::from_value(env.msg)?),
            "trade" => Self::Trade(serde_json::from_value(env.msg)?),
            "orderbook_snapshot" => Self::OrderbookSnapshot(serde_json::from_value(env.msg)?),
            "orderbook_delta" => Self::OrderbookDelta(serde_json::from_value(env.msg)?),
            "fill" => Self::Fill(serde_json::from_value(env.msg)?),
            _ => Self::Other,
        })
    }
}

/// WS `ticker` frame. Post the 2026-01-28 fixed-point migration the live wire
/// carries `*_dollars` price strings + a millisecond `ts_ms`; the integer-cent
/// Rust fields are filled by the boundary deserializers so `normalize.rs` reads
/// unchanged. Context7-verified against `/websites/kalshi_websockets`
/// (market-ticker, 2026-06-28).
#[derive(Debug, Clone, Deserialize)]
pub struct WsTicker {
    pub market_ticker: String,
    /// Best YES bid in integer cents, from the live `yes_bid_dollars` string.
    #[serde(
        rename = "yes_bid_dollars",
        deserialize_with = "de_opt_dollars_cents",
        default
    )]
    pub yes_bid: Option<i64>,
    /// Best YES ask in integer cents, from the live `yes_ask_dollars` string.
    #[serde(
        rename = "yes_ask_dollars",
        deserialize_with = "de_opt_dollars_cents",
        default
    )]
    pub yes_ask: Option<i64>,
    /// Last-traded price in integer cents, from the live `price_dollars` string.
    #[serde(
        rename = "price_dollars",
        deserialize_with = "de_opt_dollars_cents",
        default
    )]
    pub price: Option<i64>,
    /// DEPRECATED Unix timestamp in SECONDS (legacy `ts`). Prefer [`Self::ts_ms`];
    /// treating it as milliseconds is a 1000× trap. (The live ticker frame sends
    /// only `ts_ms`; `ts` is retained for forward/backward-compat decode.)
    pub ts: Option<i64>,
    /// Unix timestamp in MILLISECONDS (`ts_ms`, the non-deprecated field).
    pub ts_ms: Option<i64>,
}

/// WS `trade` (public prints) frame. Live wire carries `*_dollars` prices +
/// fixed-point `count_fp` + `ts`(sec)/`ts_ms`(ms). Context7-verified against
/// `/websites/kalshi_websockets` (public-trades, 2026-06-28).
#[derive(Debug, Clone, Deserialize)]
pub struct WsTrade {
    pub market_ticker: String,
    /// YES print price in integer cents, from the live `yes_price_dollars` string.
    #[serde(
        rename = "yes_price_dollars",
        deserialize_with = "de_opt_dollars_cents",
        default
    )]
    pub yes_price: Option<i64>,
    /// NO print price in integer cents, from the live `no_price_dollars` string.
    #[serde(
        rename = "no_price_dollars",
        deserialize_with = "de_opt_dollars_cents",
        default
    )]
    pub no_price: Option<i64>,
    /// Contracts traded, from the fixed-point `count_fp` string.
    #[serde(rename = "count_fp", deserialize_with = "de_count_fp_i64")]
    pub count: i64,
    pub taker_side: Option<String>,
    /// DEPRECATED Unix timestamp in SECONDS. Prefer [`Self::ts_ms`].
    pub ts: Option<i64>,
    /// Unix timestamp in MILLISECONDS (`ts_ms`).
    pub ts_ms: Option<i64>,
}

/// WS `orderbook_snapshot` frame. Live wire keys are `yes_dollars_fp`/
/// `no_dollars_fp`, each `[priceDollarsStr, countFpStr]`, decoded to the
/// internal `[price_cents, contracts]` `[i64; 2]` shape. Context7-verified
/// against `/websites/kalshi_websockets` (orderbook-updates, 2026-06-28).
#[derive(Debug, Clone, Deserialize)]
pub struct WsOrderbook {
    pub market_ticker: String,
    /// YES side `[price_cents, contracts]`, from `yes_dollars_fp`. Absent ⇒ empty.
    #[serde(
        rename = "yes_dollars_fp",
        deserialize_with = "de_price_count_pairs",
        default
    )]
    pub yes: Vec<[i64; 2]>,
    /// NO side `[price_cents, contracts]`, from `no_dollars_fp`. Absent ⇒ empty.
    #[serde(
        rename = "no_dollars_fp",
        deserialize_with = "de_price_count_pairs",
        default
    )]
    pub no: Vec<[i64; 2]>,
    /// DEPRECATED Unix timestamp in SECONDS. Prefer [`Self::ts_ms`].
    pub ts: Option<i64>,
    /// Unix timestamp in MILLISECONDS (`ts_ms`).
    pub ts_ms: Option<i64>,
}

/// WS `orderbook_delta` frame. Live wire carries `price_dollars` + a SIGNED
/// fixed-point `delta_fp` (e.g. `"-54.00"`). NOTE: the live `ts` field is now an
/// RFC3339 STRING (`"2022-11-22T20:44:01Z"`), so it is intentionally NOT modeled
/// as `Option<i64>` (that broke the decode) — the unknown `ts` string is ignored
/// and the millisecond `ts_ms` is the sole timestamp. Context7-verified against
/// `/websites/kalshi_websockets` (orderbook-updates, 2026-06-28).
#[derive(Debug, Clone, Deserialize)]
pub struct WsOrderbookDelta {
    pub market_ticker: String,
    pub side: String, // "yes" | "no"
    /// Level price in integer cents, from the live `price_dollars` string.
    #[serde(rename = "price_dollars", deserialize_with = "de_dollars_cents")]
    pub price: i64,
    /// SIGNED contract-count change, from the fixed-point `delta_fp` string.
    #[serde(rename = "delta_fp", deserialize_with = "de_count_fp_i64")]
    pub delta: i64,
    /// Unix timestamp in MILLISECONDS (`ts_ms`); the live `ts` RFC3339 string is
    /// ignored (see the struct note).
    pub ts_ms: Option<i64>,
}

/// WS `fill` (own-fill) frame — the highest-stakes WS datum (realized-P&L /
/// position reconcile against it). Live wire carries `yes_price_dollars` +
/// fixed-point `count_fp`; prices route through the SAME `dollars_str_to_cents`
/// the REST fill path uses. Context7-verified against `/websites/kalshi_websockets`
/// (user-fills, 2026-06-28).
#[derive(Debug, Clone, Deserialize)]
pub struct WsFill {
    pub market_ticker: String,
    pub order_id: String,
    /// YES fill price in integer cents, from the live `yes_price_dollars` string
    /// (`"0.750"` → 75¢ — the money-path conformance anchor).
    #[serde(
        rename = "yes_price_dollars",
        deserialize_with = "de_opt_dollars_cents",
        default
    )]
    pub yes_price: Option<i64>,
    /// NO fill price in integer cents, from the live `no_price_dollars` string
    /// (optional — the live fill frame usually carries only the YES leg).
    #[serde(
        rename = "no_price_dollars",
        deserialize_with = "de_opt_dollars_cents",
        default
    )]
    pub no_price: Option<i64>,
    /// Contracts filled, from the fixed-point `count_fp` string.
    #[serde(rename = "count_fp", deserialize_with = "de_count_fp_i64")]
    pub count: i64,
    pub side: String,
    /// DEPRECATED Unix timestamp in SECONDS (Kalshi `ts`). Prefer [`Self::ts_ms`];
    /// kept for backward-compat. Treating this as milliseconds is a 1000× trap.
    pub ts: Option<i64>,
    /// True per-fill identity — the Kalshi WS `trade_id` (a UUID). The SONA fill
    /// consumer maps this to `TradeFill.venue_fill_id`, the dedup key (SONA
    /// migration 0019, issue #55 item 9 Prereq A). NOT `order_id`: two partial
    /// fills of one order share an `order_id` but carry distinct `trade_id`s, so
    /// keying dedup on `order_id` would wrongly collapse legitimate partial
    /// fills. Context7-verified against the Kalshi WS user-fills schema 2026-06-24.
    pub trade_id: Option<String>,
    /// Unix timestamp in MILLISECONDS (Kalshi `ts_ms`, the non-deprecated field).
    /// issue #55 item 9 Prereq B.
    pub ts_ms: Option<i64>,
    /// Post-fill net position as a fixed-point string (Kalshi `post_position_fp`)
    /// — the accumulation-error cross-check the (deferred) fill consumer
    /// reconciles its running position against. issue #55 item 9 Prereq B.
    pub post_position_fp: Option<String>,
    /// Exchange fee paid for THIS fill, as a fixed-point US-dollar string
    /// (Kalshi `fee_cost`; Required on the wire per the user-fills schema,
    /// Context7-verified `/websites/kalshi_websockets` 2026-07-17 — the TOTAL
    /// fee for the fill, not a per-contract average). Kept `Option` +
    /// `default` for serde-backward-compatible decode of older captures. The
    /// SONA WS fill listener (issue #55 phase 1, canary phase-2 prerequisite)
    /// maps this to `TradeFill.fees_cents` via `dollars_str_to_cents`; without
    /// it a WS-first record books `fees_cents = 0` and corrupts the canary's
    /// fees-actual-vs-model measurement.
    #[serde(default)]
    pub fee_cost: Option<String>,
    // NOTE (Context7 2026-06-28, SONA #55 Phase B; fee_cost added 2026-07-17):
    // full WS wire conformance has landed — `yes_price`/`no_price`/`count`
    // deserialize from the live
    // `yes_price_dollars`/`no_price_dollars`/`count_fp` fixed-point strings via
    // the boundary helpers, so the integer-cent Rust fields downstream
    // `normalize.rs` consumes are unchanged. `fee_cost` is modeled as the raw
    // dollar string (SONA parses it at its own boundary).
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_deserializes_with_optional_fields() {
        let json = r#"{
            "ticker": "FED-23DEC-T3.00",
            "title": "Fed raises rates",
            "status": "active",
            "yes_bid": 24,
            "yes_ask": 26,
            "volume": 1200
        }"#;
        let m: Market = serde_json::from_str(json).unwrap();
        assert_eq!(m.ticker, "FED-23DEC-T3.00");
        assert_eq!(m.yes_bid, Some(24));
        assert!(m.no_bid.is_none());
    }

    #[test]
    fn ws_message_dispatch() {
        // Live wire shape: `*_dollars` price strings (post the 2026-01-28
        // fixed-point migration), decoded into the internal integer-cent fields.
        let json = r#"{"type":"ticker","msg":{"market_ticker":"X","yes_bid_dollars":"0.10","yes_ask_dollars":"0.12"}}"#;
        let env: WsEnvelope = serde_json::from_str(json).unwrap();
        let msg = WsMessage::from_envelope(env).unwrap();
        assert!(
            matches!(msg, WsMessage::Ticker(ref t) if t.market_ticker == "X" && t.yes_bid == Some(10) && t.yes_ask == Some(12))
        );
    }

    #[test]
    fn ws_message_unknown_kind_does_not_error() {
        let json = r#"{"type":"heartbeat","msg":{}}"#;
        let env: WsEnvelope = serde_json::from_str(json).unwrap();
        let msg = WsMessage::from_envelope(env).unwrap();
        assert!(matches!(msg, WsMessage::Other));
    }

    #[test]
    fn ws_fill_parses_trade_id_ts_ms_and_post_position_fp() {
        // SONA issue #55 item 9 Prereq B + Phase B conformance: the fill carries
        // `trade_id` (the per-fill dedup identity), `ts_ms` (milliseconds — the
        // non-deprecated timestamp), `post_position_fp` (post-fill net position),
        // and the live `yes_price_dollars`/`count_fp` fixed-point strings now
        // decoded into the integer-cent `yes_price` + integer `count` fields.
        // Decoded through the real WsEnvelope → from_envelope path. Field values
        // mirror the Context7-verified Kalshi user-fills example (2026-06-28).
        let json = r#"{
            "type":"fill",
            "msg":{
                "market_ticker":"HIGHNY-22DEC23-B53.5",
                "order_id":"ee587a1c-8b87-4dcf-b721-9f6f790619fa",
                "yes_price_dollars":"0.750",
                "count_fp":"278.00",
                "side":"yes",
                "trade_id":"d91bc706-ee49-470d-82d8-11418bda6fed",
                "ts":1671899397,
                "ts_ms":1671899397000,
                "post_position_fp":"500.00"
            }
        }"#;
        let env: WsEnvelope = serde_json::from_str(json).unwrap();
        let WsMessage::Fill(f) = WsMessage::from_envelope(env).unwrap() else {
            panic!("expected WsMessage::Fill");
        };
        // Money-path: the dollar string parses to integer cents.
        assert_eq!(f.yes_price, Some(75), "yes_price_dollars 0.750 → 75¢");
        assert_eq!(f.count, 278, "count_fp 278.00 → 278 contracts");
        assert_eq!(
            f.trade_id.as_deref(),
            Some("d91bc706-ee49-470d-82d8-11418bda6fed"),
            "trade_id is the per-fill dedup identity (NOT order_id)"
        );
        assert_eq!(f.ts_ms, Some(1_671_899_397_000), "ts_ms is milliseconds");
        assert_eq!(
            f.ts,
            Some(1_671_899_397),
            "ts is retained for backward-compat (deprecated seconds)"
        );
        assert_eq!(f.post_position_fp.as_deref(), Some("500.00"));
    }

    #[test]
    fn ws_fill_legacy_frame_without_new_fields_still_decodes() {
        // Backward-compat: a frame carrying only the required wire fields
        // (`count_fp` + `side`) and none of the new Optional fields still decodes
        // (serde defaults absent Option fields to None) — so the addition is
        // non-breaking for any existing producer/test.
        let json = r#"{"type":"fill","msg":{"market_ticker":"X","order_id":"o","count_fp":"1.00","side":"yes"}}"#;
        let env: WsEnvelope = serde_json::from_str(json).unwrap();
        let WsMessage::Fill(f) = WsMessage::from_envelope(env).unwrap() else {
            panic!("expected WsMessage::Fill");
        };
        assert_eq!(f.count, 1);
        assert!(f.yes_price.is_none());
        assert!(f.trade_id.is_none());
        assert!(f.ts_ms.is_none());
        assert!(f.post_position_fp.is_none());
        assert!(f.fee_cost.is_none(), "absent fee_cost decodes to None (backward-compat)");
    }

    #[test]
    fn v2_create_order_request_serializes_to_the_wire_shape() {
        let o = V2CreateOrderRequest {
            ticker: "KXMLBGAME-X".into(),
            side: V2Side::Bid,
            count: contracts_to_count_fp(10),
            price: cents_to_dollars_str(56),
            time_in_force: TimeInForce::GoodTillCanceled,
            self_trade_prevention_type: SelfTradePreventionType::TakerAtCross,
            client_order_id: "abc".into(),
        };
        let s = serde_json::to_string(&o).unwrap();
        assert!(s.contains("\"side\":\"bid\""), "{s}");
        assert!(s.contains("\"count\":\"10.00\""), "{s}");
        assert!(s.contains("\"price\":\"0.56\""), "{s}");
        assert!(s.contains("\"time_in_force\":\"good_till_canceled\""), "{s}");
        assert!(
            s.contains("\"self_trade_prevention_type\":\"taker_at_cross\""),
            "{s}"
        );
        assert!(s.contains("\"client_order_id\":\"abc\""), "{s}");
        // No legacy fields leak onto the V2 wire.
        assert!(!s.contains("\"action\""), "{s}");
        assert!(!s.contains("\"type\""), "{s}");
        assert!(!s.contains("yes_price"), "{s}");
    }

    #[test]
    fn v2_create_order_request_ask_ioc_serializes() {
        let o = V2CreateOrderRequest {
            ticker: "T".into(),
            side: V2Side::Ask,
            count: contracts_to_count_fp(1),
            price: cents_to_dollars_str(4),
            time_in_force: TimeInForce::ImmediateOrCancel,
            self_trade_prevention_type: SelfTradePreventionType::TakerAtCross,
            client_order_id: "cli-2".into(),
        };
        let s = serde_json::to_string(&o).unwrap();
        assert!(s.contains("\"side\":\"ask\""), "{s}");
        assert!(s.contains("\"price\":\"0.04\""), "{s}");
        assert!(s.contains("\"time_in_force\":\"immediate_or_cancel\""), "{s}");
        // client_order_id is REQUIRED by the V2 contract — always serialized.
        assert!(s.contains("\"client_order_id\":\"cli-2\""), "{s}");
    }

    #[test]
    fn fixed_point_encoders_match_kalshi_examples_and_round_trip() {
        assert_eq!(cents_to_dollars_str(56), "0.56");
        assert_eq!(cents_to_dollars_str(4), "0.04");
        assert_eq!(cents_to_dollars_str(100), "1.00");
        assert_eq!(cents_to_dollars_str(0), "0.00");
        // out-of-band cents clamp to the binary-contract range.
        assert_eq!(cents_to_dollars_str(-5), "0.00");
        assert_eq!(cents_to_dollars_str(250), "1.00");
        // round-trip through the existing parsers.
        assert_eq!(dollars_str_to_cents(&cents_to_dollars_str(52)), Some(52));
        assert_eq!(contracts_to_count_fp(1), "1.00");
        assert_eq!(contracts_to_count_fp(10), "10.00");
        assert_eq!(count_fp_str_to_contracts(&contracts_to_count_fp(7)), Some(7));
    }

    #[test]
    fn v2_order_response_decodes_lightweight_create_ack() {
        // The V2 create response is FLAT — no `order` wrapper, unlike legacy.
        let json = r#"{"order_id":"abc-123","fill_count":"0.00","remaining_count":"1.00","ts_ms":1750000000000}"#;
        let resp: V2OrderResponse = serde_json::from_str(json).expect("v2 create ack decodes");
        assert_eq!(resp.order_id, "abc-123");
        assert_eq!(resp.remaining_count.as_deref(), Some("1.00"));
    }

    #[test]
    fn v2_cancel_ack_decodes_lightweight_and_empty() {
        let full: V2CancelAck =
            serde_json::from_str(r#"{"order_id":"o-1","reduced_by":"1.00","ts_ms":1}"#).unwrap();
        assert_eq!(full.order_id.as_deref(), Some("o-1"));
        // A forward-compat / empty body still decodes (all fields optional).
        let empty: V2CancelAck = serde_json::from_str("{}").unwrap();
        assert!(empty.order_id.is_none());
    }
}

/// Wire-conformance fixtures captured from the live demo API
/// (`external-api.demo.kalshi.co`, 2026-06-14) after the 2026-01-28 fixed-point
/// migration. These guard against the next silent wire drift — a future
/// migration that breaks a decode now fails CI here instead of at a live rung.
#[cfg(test)]
mod conformance_2026_01_28 {
    use super::*;

    #[test]
    fn dollars_str_to_cents_parses_whole_cents() {
        assert_eq!(dollars_str_to_cents("0.5200"), Some(52));
        assert_eq!(dollars_str_to_cents("0.0400"), Some(4));
        assert_eq!(dollars_str_to_cents("0.4300"), Some(43));
        assert_eq!(dollars_str_to_cents("1.0000"), Some(100));
        assert_eq!(dollars_str_to_cents("0.0000"), Some(0));
        assert_eq!(dollars_str_to_cents(""), None);
        assert_eq!(dollars_str_to_cents("  0.9900 "), Some(99));
    }

    #[test]
    fn count_fp_str_to_contracts_parses_whole_contracts() {
        assert_eq!(count_fp_str_to_contracts("1.00"), Some(1));
        assert_eq!(count_fp_str_to_contracts("10.00"), Some(10));
        assert_eq!(count_fp_str_to_contracts("0.00"), Some(0));
        assert_eq!(count_fp_str_to_contracts(""), None);
    }

    /// The exact POST /portfolio/orders 201 body that broke the runner decode:
    /// no `count` integer, counts in `*_count_fp`, prices in `*_dollars`.
    #[test]
    fn order_record_decodes_post_migration_order_ack() {
        let json = r#"{"order":{
            "action":"buy","book_side":"bid",
            "client_order_id":"demo-rung3-x","created_time":"2026-06-14T18:00:00Z",
            "fill_count_fp":"0.00","initial_count_fp":"1.00","remaining_count_fp":"1.00",
            "no_price_dollars":"0.7000","yes_price_dollars":"0.3000",
            "order_id":"abc-123","outcome_side":"yes","side":"yes","status":"resting",
            "subaccount_number":0,"ticker":"KXMLBGAME-X","type":"limit","user_id":"u-1"
        }}"#;
        let ack: OrderAck = serde_json::from_str(json).expect("order ack must decode");
        // The field SONA actually consumes:
        assert_eq!(ack.order.order_id, "abc-123");
        assert_eq!(ack.order.status, "resting");
        // `count` is absent on the live wire → None (was a required i64 → the bug).
        assert_eq!(ack.order.count, None);
        // New fixed-point / dollar strings are captured.
        assert_eq!(ack.order.initial_count_fp.as_deref(), Some("1.00"));
        assert_eq!(ack.order.yes_price_dollars.as_deref(), Some("0.3000"));
        // accepted-quantity is sourced from initial_count_fp when `count` is gone.
        assert_eq!(ack.order.contract_count(), 1);
    }

    #[test]
    fn contract_count_prefers_legacy_then_fixed_point() {
        let legacy = OrderRecord {
            order_id: "o".into(),
            client_order_id: None,
            status: "resting".into(),
            ticker: "T".into(),
            count: Some(5),
            filled_count: None,
            remaining_count: None,
            initial_count_fp: Some("9.00".into()), // ignored: legacy `count` wins
            remaining_count_fp: None,
            fill_count_fp: None,
            yes_price_dollars: None,
            no_price_dollars: None,
        };
        assert_eq!(legacy.contract_count(), 5);
        let migrated = OrderRecord {
            count: None,
            initial_count_fp: Some("3.00".into()),
            ..legacy.clone()
        };
        assert_eq!(migrated.contract_count(), 3);
        let neither = OrderRecord {
            count: None,
            initial_count_fp: None,
            ..legacy
        };
        assert_eq!(neither.contract_count(), 0);
    }

    /// GET /markets/{ticker}/orderbook — `orderbook_fp` wrapper, `*_dollars`
    /// keys, `[priceStr, countFpStr]` entries → internal `[cents, contracts]`.
    #[test]
    fn orderbook_decodes_post_migration_fp_wrapper() {
        let json = r#"{"orderbook_fp":{
            "no_dollars":[["0.0400","10.00"],["0.4300","3.00"]],
            "yes_dollars":[["0.4400","3.00"],["0.4800","3.00"],["0.5200","6.00"]]
        }}"#;
        let resp: OrderbookResponse = serde_json::from_str(json).expect("orderbook must decode");
        assert_eq!(resp.orderbook.yes, vec![[44, 3], [48, 3], [52, 6]]);
        assert_eq!(resp.orderbook.no, vec![[4, 10], [43, 3]]);
    }

    /// GET /markets/{ticker} — integer price fields now `null`; real values in
    /// the `*_dollars` strings, read via the `*_cents()` helpers.
    #[test]
    fn market_decodes_post_migration_dollar_prices() {
        let json = r#"{
            "ticker":"KXMLBGAME-26JUN171410DETHOU-HOU","status":"active",
            "yes_bid":null,"yes_ask":null,"no_bid":null,"last_price":null,
            "yes_bid_dollars":"0.5200","last_price_dollars":"0.0000","volume":null
        }"#;
        let m: Market = serde_json::from_str(json).expect("market must decode");
        assert_eq!(m.yes_bid, None); // legacy integer field is null
        assert_eq!(m.yes_bid_cents(), Some(52)); // real value via dollar string
        assert_eq!(m.last_price_cents(), Some(0));
        assert_eq!(m.no_bid_cents(), None); // neither field present
    }
}

/// Portfolio-read wire-conformance fixtures (issue #55 items 7-8). Shapes
/// Context7-verified against the live Kalshi V2 OpenAPI spec
/// (`docs.kalshi.com/openapi.yaml`, retrieved 2026-06-23) — NOT hand-drafted
/// from the issue body (whose portfolio shapes predated the fixed-point
/// migration and used `ts_ms`/`side` rather than the live `ts`/`outcome_side`).
/// These guard the next silent wire drift on the portfolio read path.
#[cfg(test)]
mod conformance_portfolio_2026_06_23 {
    use super::*;

    /// GET /portfolio/balance — `balance`/`portfolio_value` are integer cents;
    /// `balance_dollars` is the fixed-point mirror; `balance_breakdown` is
    /// extra payload ignored by the forward-compat decode.
    #[test]
    fn balance_response_decodes_integer_cents() {
        let json = r#"{
            "balance": 100000,
            "balance_dollars": "1000.00",
            "portfolio_value": 150000,
            "updated_ts": 1678886400,
            "balance_breakdown": [{"exchange_index": 0, "balance": "500.00"}]
        }"#;
        let r: BalanceResponse = serde_json::from_str(json).expect("balance must decode");
        // SONA's pre-flight gate reads `balance` (cents) directly.
        assert_eq!(r.balance, 100000);
        assert_eq!(r.portfolio_value, 150000);
        assert_eq!(r.balance_dollars.as_deref(), Some("1000.00"));
        assert_eq!(r.updated_ts, Some(1678886400));
    }

    /// GET /portfolio/positions — fixed-point `position_fp` + `*_dollars`
    /// money; the spec's `event_positions` array is ignored; empty `cursor`
    /// means no next page.
    #[test]
    fn positions_response_decodes_fixed_point_and_ignores_event_positions() {
        let json = r#"{
            "market_positions": [{
                "ticker": "KXMLBGAME-26JUN23DETHOU-HOU",
                "position_fp": "10.00",
                "market_exposure_dollars": "5.2000",
                "total_traded_dollars": "5.2000",
                "realized_pnl_dollars": "0.0000",
                "resting_orders_count": 0,
                "fees_paid_dollars": "0.0700",
                "last_updated_ts": "2026-06-23T18:00:00Z"
            }],
            "event_positions": [],
            "cursor": ""
        }"#;
        let r: PositionsResponse = serde_json::from_str(json).expect("positions must decode");
        assert_eq!(r.market_positions.len(), 1);
        let p = &r.market_positions[0];
        assert_eq!(p.ticker, "KXMLBGAME-26JUN23DETHOU-HOU");
        // Net contract count parsed from the fixed-point string.
        assert_eq!(p.position_contracts(), 10);
        assert_eq!(p.total_traded_dollars.as_deref(), Some("5.2000"));
        // Empty cursor → no next page (decodes to Some("")).
        assert_eq!(r.cursor.as_deref(), Some(""));
    }

    /// A short (negative) position decodes with a signed `position_fp`.
    #[test]
    fn market_position_short_is_signed() {
        let json = r#"{"ticker":"T","position_fp":"-3.00"}"#;
        let p: MarketPosition = serde_json::from_str(json).expect("short position decodes");
        assert_eq!(p.position_contracts(), -3);
        // Absent position_fp → 0 (forward-compat).
        let empty: MarketPosition = serde_json::from_str(r#"{"ticker":"T"}"#).unwrap();
        assert_eq!(empty.position_contracts(), 0);
    }

    /// GET /portfolio/fills — the timestamp field is `ts` (int64), NOT `ts_ms`;
    /// direction is the canonical `outcome_side`/`book_side`. This is the exact
    /// drift the issue's hand-drafted `RestFill` would have introduced.
    #[test]
    fn fills_response_decodes_with_ts_not_ts_ms_and_canonical_direction() {
        let json = r#"{
            "fills": [{
                "fill_id": "f-1",
                "trade_id": "f-1",
                "order_id": "o-1",
                "ticker": "KXMLBGAME-26JUN23DETHOU-HOU",
                "market_ticker": "KXMLBGAME-26JUN23DETHOU-HOU",
                "side": "yes",
                "action": "buy",
                "outcome_side": "yes",
                "book_side": "bid",
                "count_fp": "1.00",
                "yes_price_dollars": "0.5200",
                "no_price_dollars": "0.4800",
                "is_taker": true,
                "fee_cost": "0.0100",
                "created_time": "2026-06-23T18:00:00Z",
                "ts": 1678886400
            }],
            "cursor": ""
        }"#;
        let r: FillsResponse = serde_json::from_str(json).expect("fills must decode");
        assert_eq!(r.fills.len(), 1);
        let f = &r.fills[0];
        assert_eq!(f.fill_id, "f-1");
        assert_eq!(f.order_id, "o-1");
        // Count via the fixed-point parser.
        assert_eq!(count_fp_str_to_contracts(f.count_fp.as_deref().unwrap()), Some(1));
        // Price via the dollar-string parser.
        assert_eq!(
            dollars_str_to_cents(f.yes_price_dollars.as_deref().unwrap()),
            Some(52)
        );
        // The canonical direction fields are populated (NOT the deprecated ones).
        assert_eq!(f.outcome_side.as_deref(), Some("yes"));
        assert_eq!(f.book_side.as_deref(), Some("bid"));
        assert_eq!(f.is_taker, Some(true));
        // The timestamp is `ts` (NOT `ts_ms`) — the field name the issue got wrong.
        assert_eq!(f.ts, Some(1678886400));
    }
}

/// Orders-list wire-conformance fixtures (SONA ADR-042 §1 — the halt
/// cancel-sweep's resting-order inventory). Shape Context7-verified against
/// the live Kalshi V2 OpenAPI spec (`docs.kalshi.com/openapi.yaml`, retrieved
/// 2026-07-06): post-fixed-point-migration `Order` objects (`*_count_fp` /
/// `*_dollars` strings, no integer `count`), required `cursor`, and the
/// deprecated-but-present `side`/`action` fields ignored by the
/// forward-compat decode. These guard the next silent wire drift on the
/// order-inventory read path.
#[cfg(test)]
mod conformance_orders_2026_07_06 {
    use super::*;

    /// GET /portfolio/orders?status=resting — the live post-migration shape:
    /// fp-count/dollar strings only (integer `count` absent), extra canonical
    /// fields (`outcome_side`/`book_side`) ignored, non-empty cursor = more
    /// pages.
    #[test]
    fn orders_response_decodes_live_resting_page() {
        let json = r#"{
            "orders": [{
                "order_id": "a1b2c3d4-0001",
                "user_id": "u-1",
                "client_order_id": "sona-exit-42",
                "ticker": "KXMLBGAME-26JUL06TBKC-TB",
                "side": "yes",
                "action": "buy",
                "outcome_side": "yes",
                "book_side": "bid",
                "type": "limit",
                "status": "resting",
                "yes_price_dollars": "0.4200",
                "no_price_dollars": "0.5800",
                "fill_count_fp": "0.00",
                "remaining_count_fp": "1.00",
                "initial_count_fp": "1.00",
                "taker_fill_cost_dollars": "0.0000",
                "maker_fill_cost_dollars": "0.0000",
                "taker_fees_dollars": "0.0000",
                "maker_fees_dollars": "0.0000",
                "created_time": "2026-07-06T19:00:00Z",
                "last_update_time": "2026-07-06T19:00:00Z"
            }],
            "cursor": "next-page-token"
        }"#;
        let r: OrdersResponse = serde_json::from_str(json).expect("orders page must decode");
        assert_eq!(r.orders.len(), 1);
        let o = &r.orders[0];
        assert_eq!(o.order_id, "a1b2c3d4-0001");
        assert_eq!(o.status, "resting");
        assert_eq!(o.ticker, "KXMLBGAME-26JUL06TBKC-TB");
        // Post-migration: integer `count` is absent; the fp string carries it.
        assert_eq!(o.count, None);
        assert_eq!(o.contract_count(), 1);
        assert_eq!(o.remaining_count_fp.as_deref(), Some("1.00"));
        // Non-empty cursor → caller must walk the next page.
        assert_eq!(r.cursor.as_deref(), Some("next-page-token"));
    }

    /// The empty page (no resting orders): `orders` empty, empty-string
    /// cursor = no next page — the sweep's clean-account fast path.
    #[test]
    fn orders_response_empty_page_decodes() {
        let r: OrdersResponse =
            serde_json::from_str(r#"{"orders": [], "cursor": ""}"#).expect("empty page decodes");
        assert!(r.orders.is_empty());
        assert_eq!(r.cursor.as_deref(), Some(""));
    }
}

/// Multi-page wire-conformance fixtures for the cursor-walked positions/fills
/// reads (`RestClient::get_all_positions` / `get_all_fills`). Shapes
/// Context7-verified against the live Kalshi V2 OpenAPI spec
/// (`docs.kalshi.com/openapi.yaml`, retrieved 2026-07-10): both endpoints are
/// cursor-paginated; `GET /portfolio/positions` returns `market_positions`
/// (fixed-point `position_fp` + `*_dollars` money) plus a separate
/// `event_positions` array the forward-compat [`PositionsResponse`] ignores;
/// `GET /portfolio/fills` returns `fills` (`fill_id`/`order_id`/`count_fp`/
/// `fee_cost`, canonical `outcome_side`/`book_side`) with a required `cursor`.
///
/// These are DECODE-LEVEL fixtures: each covers a page-1 payload with a
/// non-empty cursor (the sentinel that makes the walk fetch another page) and
/// a page-2 payload with an empty/absent cursor (the sentinel that stops it).
/// The `get_all_*` HTTP walk loop itself is NOT unit-tested here — the crate
/// ships no mock transport (`RestClient::send` holds a live `reqwest::Client`
/// with no injectable seam), the exact same limitation under which the
/// pre-existing `get_resting_orders` walk is also untested. The
/// `*_walk_over_decoded_pages_*` tests below exercise the page-merge +
/// stop-on-empty-cursor SEMANTICS against the decoded fixtures, which is what
/// is testable at this layer.
#[cfg(test)]
mod conformance_walked_positions_fills_2026_07_10 {
    use super::*;

    // --- positions --------------------------------------------------------

    /// GET /portfolio/positions page 1 — realistic fixed-point shapes, the
    /// spec's `event_positions` array present-but-ignored, and a NON-EMPTY
    /// cursor: the walk must fetch the next page.
    #[test]
    fn positions_page_one_has_next_cursor() {
        let json = r#"{
            "market_positions": [{
                "ticker": "KXMLBGAME-26JUL10NYYBOS-NYY",
                "position_fp": "10.00",
                "market_exposure_dollars": "5.2000",
                "total_traded_dollars": "5.2000",
                "realized_pnl_dollars": "0.0000",
                "resting_orders_count": 0,
                "fees_paid_dollars": "0.0700",
                "last_updated_ts": "2026-07-10T18:00:00Z"
            }],
            "event_positions": [{
                "event_ticker": "KXMLBGAME-26JUL10NYYBOS",
                "total_cost_dollars": "5.2000",
                "total_cost_shares_fp": "10.00",
                "event_exposure_dollars": "5.2000",
                "realized_pnl_dollars": "0.0000",
                "fees_paid_dollars": "0.0700"
            }],
            "cursor": "pos-page-2-token"
        }"#;
        let r: PositionsResponse = serde_json::from_str(json).expect("positions page 1 decodes");
        assert_eq!(r.market_positions.len(), 1);
        let p = &r.market_positions[0];
        assert_eq!(p.ticker, "KXMLBGAME-26JUL10NYYBOS-NYY");
        assert_eq!(p.position_contracts(), 10);
        assert_eq!(p.total_traded_dollars.as_deref(), Some("5.2000"));
        // Non-empty cursor → the walk continues to page 2.
        assert_eq!(r.cursor.as_deref(), Some("pos-page-2-token"));
    }

    /// GET /portfolio/positions page 2 — a second (short) position and an
    /// EMPTY cursor: the walk stops here. Also asserts the absent-`cursor`
    /// shape terminates identically (decodes to `None`).
    #[test]
    fn positions_page_two_terminates_walk() {
        let json = r#"{
            "market_positions": [{
                "ticker": "KXMLBGAME-26JUL10LADSF-SF",
                "position_fp": "-3.00",
                "market_exposure_dollars": "1.4400",
                "total_traded_dollars": "1.4400",
                "realized_pnl_dollars": "0.0000",
                "fees_paid_dollars": "0.0200",
                "last_updated_ts": "2026-07-10T18:05:00Z"
            }],
            "event_positions": [],
            "cursor": ""
        }"#;
        let r: PositionsResponse = serde_json::from_str(json).expect("positions page 2 decodes");
        assert_eq!(r.market_positions.len(), 1);
        // Signed short position decodes.
        assert_eq!(r.market_positions[0].position_contracts(), -3);
        // Empty cursor → the walk stops.
        assert_eq!(r.cursor.as_deref(), Some(""));

        // An absent `cursor` is the other terminal sentinel (`None`).
        let no_cursor: PositionsResponse =
            serde_json::from_str(r#"{"market_positions": []}"#).expect("absent cursor decodes");
        assert!(no_cursor.market_positions.is_empty());
        assert_eq!(no_cursor.cursor.as_deref(), None);
    }

    /// The page-merge + stop-on-empty-cursor semantics of `get_all_positions`,
    /// exercised over the two decoded wire pages (the HTTP loop is not
    /// unit-testable without a mock transport — see the module doc). Mirrors
    /// the production walk's `Some("") | None => stop` predicate.
    #[test]
    fn positions_walk_over_decoded_pages_concatenates_and_stops() {
        let page1 = r#"{
            "market_positions": [{"ticker": "A", "position_fp": "10.00"}],
            "event_positions": [],
            "cursor": "pos-page-2-token"
        }"#;
        let page2 = r#"{
            "market_positions": [{"ticker": "B", "position_fp": "-3.00"}],
            "event_positions": [],
            "cursor": ""
        }"#;
        let pages = [page1, page2];

        let mut all: Vec<MarketPosition> = Vec::new();
        let mut consumed = 0usize;
        for raw in pages {
            let page: PositionsResponse = serde_json::from_str(raw).expect("page decodes");
            all.extend(page.market_positions);
            consumed += 1;
            // Same termination decision as RestClient::get_all_positions.
            if matches!(page.cursor.as_deref(), Some("") | None) {
                break;
            }
        }
        // Both pages consumed, both positions concatenated, stopped on page 2.
        assert_eq!(consumed, 2);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].ticker, "A");
        assert_eq!(all[1].ticker, "B");
        assert_eq!(all[1].position_contracts(), -3);
    }

    // --- fills ------------------------------------------------------------

    /// GET /portfolio/fills page 1 — `fill_id`/`order_id`/`count_fp`/`fee_cost`
    /// + canonical `outcome_side`/`book_side` + `ts` (int64), and a NON-EMPTY
    /// cursor: the walk must fetch the next page.
    #[test]
    fn fills_page_one_has_next_cursor() {
        let json = r#"{
            "fills": [{
                "fill_id": "f-1",
                "trade_id": "f-1",
                "order_id": "o-1",
                "ticker": "KXMLBGAME-26JUL10NYYBOS-NYY",
                "market_ticker": "KXMLBGAME-26JUL10NYYBOS-NYY",
                "side": "yes",
                "action": "buy",
                "outcome_side": "yes",
                "book_side": "bid",
                "count_fp": "1.00",
                "yes_price_dollars": "0.5200",
                "no_price_dollars": "0.4800",
                "is_taker": true,
                "fee_cost": "0.0100",
                "created_time": "2026-07-10T18:00:00Z",
                "ts": 1752170400
            }],
            "cursor": "fill-page-2-token"
        }"#;
        let r: FillsResponse = serde_json::from_str(json).expect("fills page 1 decodes");
        assert_eq!(r.fills.len(), 1);
        let f = &r.fills[0];
        assert_eq!(f.fill_id, "f-1");
        assert_eq!(f.order_id, "o-1");
        assert_eq!(count_fp_str_to_contracts(f.count_fp.as_deref().unwrap()), Some(1));
        assert_eq!(f.fee_cost.as_deref(), Some("0.0100"));
        assert_eq!(f.outcome_side.as_deref(), Some("yes"));
        assert_eq!(f.book_side.as_deref(), Some("bid"));
        // Non-empty cursor → the walk continues to page 2.
        assert_eq!(r.cursor.as_deref(), Some("fill-page-2-token"));
    }

    /// GET /portfolio/fills page 2 — a second fill and an EMPTY cursor: the
    /// walk stops. Also asserts the absent-`cursor` shape terminates (`None`).
    #[test]
    fn fills_page_two_terminates_walk() {
        let json = r#"{
            "fills": [{
                "fill_id": "f-2",
                "trade_id": "f-2",
                "order_id": "o-2",
                "ticker": "KXMLBGAME-26JUL10LADSF-SF",
                "market_ticker": "KXMLBGAME-26JUL10LADSF-SF",
                "side": "no",
                "action": "sell",
                "outcome_side": "no",
                "book_side": "ask",
                "count_fp": "2.00",
                "yes_price_dollars": "0.6100",
                "no_price_dollars": "0.3900",
                "is_taker": false,
                "fee_cost": "0.0200",
                "created_time": "2026-07-10T18:05:00Z",
                "ts": 1752170700
            }],
            "cursor": ""
        }"#;
        let r: FillsResponse = serde_json::from_str(json).expect("fills page 2 decodes");
        assert_eq!(r.fills.len(), 1);
        assert_eq!(r.fills[0].fill_id, "f-2");
        assert_eq!(r.fills[0].outcome_side.as_deref(), Some("no"));
        // Empty cursor → the walk stops.
        assert_eq!(r.cursor.as_deref(), Some(""));

        // An absent `cursor` is the other terminal sentinel (`None`).
        let no_cursor: FillsResponse =
            serde_json::from_str(r#"{"fills": []}"#).expect("absent cursor decodes");
        assert!(no_cursor.fills.is_empty());
        assert_eq!(no_cursor.cursor.as_deref(), None);
    }

    /// The page-merge + stop-on-empty-cursor semantics of `get_all_fills`,
    /// exercised over the two decoded wire pages (the HTTP loop is not
    /// unit-testable without a mock transport — see the module doc). Mirrors
    /// the production walk's `Some("") | None => stop` predicate.
    #[test]
    fn fills_walk_over_decoded_pages_concatenates_and_stops() {
        let page1 = r#"{
            "fills": [{"fill_id": "f-1", "order_id": "o-1", "ticker": "A", "count_fp": "1.00"}],
            "cursor": "fill-page-2-token"
        }"#;
        let page2 = r#"{
            "fills": [{"fill_id": "f-2", "order_id": "o-2", "ticker": "B", "count_fp": "2.00"}],
            "cursor": ""
        }"#;
        let pages = [page1, page2];

        let mut all: Vec<RestFill> = Vec::new();
        let mut consumed = 0usize;
        for raw in pages {
            let page: FillsResponse = serde_json::from_str(raw).expect("page decodes");
            all.extend(page.fills);
            consumed += 1;
            // Same termination decision as RestClient::get_all_fills.
            if matches!(page.cursor.as_deref(), Some("") | None) {
                break;
            }
        }
        // Both pages consumed, both fills concatenated, stopped on page 2.
        assert_eq!(consumed, 2);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].fill_id, "f-1");
        assert_eq!(all[1].fill_id, "f-2");
    }
}

/// WebSocket wire-conformance fixtures (SONA issue #55 Phase B). The 5 live WS
/// channel frames decoded through the real `WsEnvelope → WsMessage::from_envelope`
/// path, asserting the integer-cent / integer-contract values the boundary
/// deserializers fill from the live `*_dollars` / `*_fp` strings. Shapes
/// Context7-verified against `/websites/kalshi_websockets` (ticker / public-trades
/// / orderbook-updates / user-fills, retrieved 2026-06-28) — the recon that
/// corrected the original audit (integer-cent DTOs would fail to deserialize the
/// live dollar/fp strings). These guard the next silent WS wire drift.
#[cfg(test)]
mod conformance_ws_2026_06_28 {
    use super::*;

    fn decode(json: &str) -> WsMessage {
        let env: WsEnvelope = serde_json::from_str(json).expect("envelope must parse");
        WsMessage::from_envelope(env).expect("frame must decode")
    }

    /// `ticker` — `*_dollars` price strings → integer cents (with half-up
    /// rounding on the 3rd fractional digit), `ts_ms` milliseconds.
    #[test]
    fn ticker_frame_decodes_dollar_prices_to_cents() {
        let json = r#"{
            "type":"ticker","sid":12345,
            "msg":{
                "market_ticker":"FED-23DEC-T3.00",
                "market_id":"a1b2c3d4-e5f6-7890-1234-567890abcdef",
                "price_dollars":"0.500000",
                "yes_bid_dollars":"0.495000",
                "yes_ask_dollars":"0.505000",
                "volume_fp":"1500.00",
                "ts_ms":1678886400000
            }
        }"#;
        let WsMessage::Ticker(t) = decode(json) else {
            panic!("expected Ticker");
        };
        assert_eq!(t.market_ticker, "FED-23DEC-T3.00");
        assert_eq!(t.yes_bid, Some(50), "0.495000 → 50¢ (half-up)");
        assert_eq!(t.yes_ask, Some(51), "0.505000 → 51¢ (half-up)");
        assert_eq!(t.price, Some(50), "0.500000 → 50¢");
        assert_eq!(t.ts_ms, Some(1678886400000));
    }

    /// `trade` — `yes_price_dollars`/`no_price_dollars` → cents, `count_fp` →
    /// contracts, both `ts` (sec) and `ts_ms` (ms) present.
    #[test]
    fn trade_frame_decodes_dollar_prices_and_count_fp() {
        let json = r#"{
            "type":"trade","sid":11,
            "msg":{
                "trade_id":"d91bc706-ee49-470d-82d8-11418bda6fed",
                "market_ticker":"HIGHNY-22DEC23-B53.5",
                "yes_price_dollars":"0.360",
                "no_price_dollars":"0.640",
                "count_fp":"136.00",
                "taker_side":"no",
                "ts":1669149841,
                "ts_ms":1669149841000
            }
        }"#;
        let WsMessage::Trade(t) = decode(json) else {
            panic!("expected Trade");
        };
        assert_eq!(t.yes_price, Some(36), "0.360 → 36¢");
        assert_eq!(t.no_price, Some(64), "0.640 → 64¢");
        assert_eq!(t.count, 136, "count_fp 136.00 → 136");
        assert_eq!(t.taker_side.as_deref(), Some("no"));
        assert_eq!(t.ts, Some(1669149841));
        assert_eq!(t.ts_ms, Some(1669149841000));
    }

    /// `orderbook_snapshot` — `yes_dollars_fp`/`no_dollars_fp` arrays of
    /// `[priceDollarsStr, countFpStr]` → internal `[price_cents, contracts]`.
    #[test]
    fn orderbook_snapshot_frame_decodes_dollar_fp_levels() {
        let json = r#"{
            "type":"orderbook_snapshot","sid":2,"seq":2,
            "msg":{
                "market_ticker":"FED-23DEC-T3.00",
                "market_id":"9b0f6b43-5b68-4f9f-9f02-9a2d1b8ac1a1",
                "yes_dollars_fp":[["0.0800","300.00"],["0.2200","333.00"]],
                "no_dollars_fp":[["0.5400","20.00"],["0.5600","146.00"]]
            }
        }"#;
        let WsMessage::OrderbookSnapshot(ob) = decode(json) else {
            panic!("expected OrderbookSnapshot");
        };
        assert_eq!(ob.market_ticker, "FED-23DEC-T3.00");
        assert_eq!(ob.yes, vec![[8, 300], [22, 333]]);
        assert_eq!(ob.no, vec![[54, 20], [56, 146]]);
        // The snapshot frame carries no timestamp — both fall back to None.
        assert!(ob.ts_ms.is_none());
        assert!(ob.ts.is_none());
    }

    /// `orderbook_delta` — `price_dollars` → cents, SIGNED `delta_fp` → signed
    /// contract delta, AND the live `ts` is an RFC3339 STRING that must NOT break
    /// the decode (the `ts` i64 field was dropped; `ts_ms` is the timestamp).
    #[test]
    fn orderbook_delta_frame_decodes_and_rfc3339_ts_does_not_break() {
        let json = r#"{
            "type":"orderbook_delta","sid":2,"seq":3,
            "msg":{
                "market_ticker":"FED-23DEC-T3.00",
                "market_id":"9b0f6b43-5b68-4f9f-9f02-9a2d1b8ac1a1",
                "price_dollars":"0.960",
                "delta_fp":"-54.00",
                "side":"yes",
                "ts":"2022-11-22T20:44:01Z",
                "ts_ms":1669149841000
            }
        }"#;
        // Decoding succeeding at all is the assertion that the RFC3339 `ts`
        // STRING does not get parsed as `Option<i64>` (it is an ignored unknown
        // field now that the `ts` i64 field is dropped).
        let WsMessage::OrderbookDelta(d) = decode(json) else {
            panic!("expected OrderbookDelta");
        };
        assert_eq!(d.price, 96, "0.960 → 96¢");
        assert_eq!(d.delta, -54, "delta_fp -54.00 → -54 (signed)");
        assert_eq!(d.side, "yes");
        assert_eq!(d.ts_ms, Some(1669149841000));
    }

    /// `fill` — THE money-path anchor: `yes_price_dollars:"0.750"` → 75¢, routed
    /// through the SAME `dollars_str_to_cents` the REST fill path uses. A wrong
    /// price unit here corrupts realized-P&L / position the scalp round-trip +
    /// ramp counter reconcile against.
    #[test]
    fn fill_frame_decodes_yes_price_dollars_to_75_cents() {
        let json = r#"{
            "type":"fill","sid":13,
            "msg":{
                "trade_id":"d91bc706-ee49-470d-82d8-11418bda6fed",
                "order_id":"ee587a1c-8b87-4dcf-b721-9f6f790619fa",
                "market_ticker":"HIGHNY-22DEC23-B53.5",
                "is_taker":true,
                "side":"yes",
                "yes_price_dollars":"0.750",
                "count_fp":"278.00",
                "fee_cost":"2.7800",
                "action":"buy",
                "ts":1671899397,
                "ts_ms":1671899397000,
                "post_position_fp":"500.00",
                "purchased_side":"yes",
                "subaccount":3
            }
        }"#;
        let WsMessage::Fill(f) = decode(json) else {
            panic!("expected Fill");
        };
        // THE highest-stakes money-path datum.
        assert_eq!(f.yes_price, Some(75), "yes_price_dollars 0.750 → 75¢");
        assert_eq!(f.count, 278, "count_fp 278.00 → 278 contracts");
        assert_eq!(f.side, "yes");
        assert_eq!(f.order_id, "ee587a1c-8b87-4dcf-b721-9f6f790619fa");
        assert_eq!(
            f.trade_id.as_deref(),
            Some("d91bc706-ee49-470d-82d8-11418bda6fed")
        );
        assert_eq!(f.ts, Some(1671899397));
        assert_eq!(f.ts_ms, Some(1671899397000));
        assert_eq!(f.post_position_fp.as_deref(), Some("500.00"));
        // fee_cost (2026-07-17, SONA #55 phase 1): the TOTAL exchange fee for
        // this fill as the raw fixed-point dollar string — SONA parses it at
        // its own boundary (dollars_str_to_cents → fees_cents).
        assert_eq!(f.fee_cost.as_deref(), Some("2.7800"));
    }

    /// Backward-compat: frames missing the optional fields still decode —
    /// a ticker with no price keys yields `None` cents; a one-sided snapshot
    /// (only `yes_dollars_fp`, count without a decimal) yields an empty `no`.
    #[test]
    fn missing_optional_fields_still_decode() {
        let WsMessage::Ticker(t) = decode(r#"{"type":"ticker","msg":{"market_ticker":"X"}}"#)
        else {
            panic!("expected Ticker");
        };
        assert_eq!(t.market_ticker, "X");
        assert!(t.yes_bid.is_none());
        assert!(t.yes_ask.is_none());
        assert!(t.price.is_none());
        assert!(t.ts.is_none());
        assert!(t.ts_ms.is_none());

        let WsMessage::OrderbookSnapshot(ob) = decode(
            r#"{"type":"orderbook_snapshot","msg":{"market_ticker":"X","yes_dollars_fp":[["0.99","100"]]}}"#,
        ) else {
            panic!("expected OrderbookSnapshot");
        };
        assert_eq!(ob.yes, vec![[99, 100]], "count without a decimal → 100");
        assert!(ob.no.is_empty(), "absent no_dollars_fp → empty");
    }
}
