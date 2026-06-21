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

#[derive(Debug, Clone, Deserialize)]
pub struct WsTicker {
    pub market_ticker: String,
    pub yes_bid: Option<i64>,
    pub yes_ask: Option<i64>,
    pub price: Option<i64>,
    pub ts: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WsTrade {
    pub market_ticker: String,
    pub yes_price: Option<i64>,
    pub no_price: Option<i64>,
    pub count: i64,
    pub taker_side: Option<String>,
    pub ts: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WsOrderbook {
    pub market_ticker: String,
    pub yes: Vec<[i64; 2]>,
    pub no: Vec<[i64; 2]>,
    pub ts: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WsOrderbookDelta {
    pub market_ticker: String,
    pub side: String, // "yes" | "no"
    pub price: i64,
    pub delta: i64,
    pub ts: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WsFill {
    pub market_ticker: String,
    pub order_id: String,
    pub yes_price: Option<i64>,
    pub no_price: Option<i64>,
    pub count: i64,
    pub side: String,
    pub ts: Option<i64>,
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
        let json = r#"{"type":"ticker","msg":{"market_ticker":"X","yes_bid":10,"yes_ask":12}}"#;
        let env: WsEnvelope = serde_json::from_str(json).unwrap();
        let msg = WsMessage::from_envelope(env).unwrap();
        assert!(matches!(msg, WsMessage::Ticker(ref t) if t.market_ticker == "X"));
    }

    #[test]
    fn ws_message_unknown_kind_does_not_error() {
        let json = r#"{"type":"heartbeat","msg":{}}"#;
        let env: WsEnvelope = serde_json::from_str(json).unwrap();
        let msg = WsMessage::from_envelope(env).unwrap();
        assert!(matches!(msg, WsMessage::Other));
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
