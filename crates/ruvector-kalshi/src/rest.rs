//! Kalshi REST client. All authenticated endpoints sign the request using
//! [`crate::auth::Signer`] and propagate typed errors.
//!
//! # Live-trade gate
//!
//! [`RestClient::post_order`] is the only method that can move money and it
//! refuses to run unless the `KALSHI_ENABLE_LIVE` environment variable is
//! set to `1`. Any other value (including unset) returns an error without
//! making the HTTP call. This is a belt-and-braces backstop on top of any
//! strategy-level `RiskGate`.

use std::sync::Arc;

use crate::auth::Signer;
use crate::models::{
    BalanceResponse, FillsResponse, GetMarketResponse, Market, MarketPosition, MarketsResponse,
    OrderAck, OrderRecord, OrderbookResponse, OrderbookSnapshot, OrdersResponse, PositionsResponse,
    RestFill, V2AmendAck, V2AmendOrder, V2CancelAck, V2CreateOrderRequest, V2OrderResponse,
};
use crate::rate_limit::RateLimiter;
use crate::{KalshiError, Result};

#[derive(Clone)]
pub struct RestClient {
    /// Base URL string (for `reqwest` to consume). `Arc<str>` keeps clone O(1).
    base_url: Arc<str>,
    /// Pre-computed URL path component of `base_url` (e.g. `/trade-api/v2`)
    /// — prepended to the caller path to build the signature payload
    /// without re-parsing the URL on every request.
    base_path: Arc<str>,
    signer: Signer,
    http: reqwest::Client,
    limiter: Arc<RateLimiter>,
}

impl RestClient {
    pub fn new(base_url: impl Into<String>, signer: Signer) -> Result<Self> {
        // Kalshi's public rate limits are conservative; 10 req/s sustained
        // with a burst of 20 is well under any documented cap.
        Self::with_rate_limit(base_url, signer, 20, 10.0)
    }

    /// Construct with an explicit rate-limit (useful for tests and high-
    /// frequency read-only workloads).
    pub fn with_rate_limit(
        base_url: impl Into<String>,
        signer: Signer,
        burst: u32,
        per_sec: f64,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("ruvector-kalshi/0.1")
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        let base: String = base_url.into();
        // Parse once at construction. For malformed URLs fall back to the
        // caller-supplied string (same behavior as the old path lookup).
        let base_path: String = reqwest::Url::parse(&base)
            .map(|u| u.path().trim_end_matches('/').to_string())
            .unwrap_or_else(|_| "".to_string());
        Ok(Self {
            base_url: Arc::from(base.into_boxed_str()),
            base_path: Arc::from(base_path.into_boxed_str()),
            signer,
            http,
            limiter: Arc::new(RateLimiter::new(burst, per_sec)),
        })
    }

    /// Path used in the signature must be the full `/trade-api/v2/...` path,
    /// not the host-relative fragment, per Kalshi's spec.
    ///
    /// Uses the pre-computed `base_path` so there is no URL parse per call.
    fn sig_path_for(&self, path: &str) -> String {
        let p = if path.starts_with('/') {
            path
        } else {
            &format!("/{path}")[..]
        };
        // Strip any query string for the signature base — Kalshi signs only
        // the path component.
        let path_only = match p.find('?') {
            Some(i) => &p[..i],
            None => p,
        };
        format!("{}{}", self.base_path, path_only)
    }

    async fn send<R: for<'de> serde::Deserialize<'de>>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl serde::Serialize>,
    ) -> Result<R> {
        self.limiter.acquire().await;
        let url = url_join(&self.base_url, path);
        let sig_path = self.sig_path_for(path);
        let headers = self.signer.sign_now(method.as_str(), &sig_path);

        let mut rb = self.http.request(method, &url);
        rb = headers.apply(rb);
        if let Some(b) = body {
            rb = rb.json(b);
        }

        let resp = rb.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(KalshiError::Api {
                status: status.as_u16(),
                body,
            });
        }
        let parsed = resp.json::<R>().await?;
        Ok(parsed)
    }

    pub async fn list_markets(&self, status: Option<&str>) -> Result<Vec<Market>> {
        let path = match status {
            Some(s) => format!("/markets?status={s}"),
            None => "/markets".into(),
        };
        let resp: MarketsResponse = self.send(reqwest::Method::GET, &path, NO_BODY).await?;
        Ok(resp.markets)
    }

    /// Read ONE market by ticker (`GET /markets/{ticker}`). Signed READ — NOT
    /// gated on `KALSHI_ENABLE_LIVE` (it mutates nothing).
    ///
    /// SONA #700: this is the read that sources [`Market::exchange_index`], the
    /// shard an order-class request must carry. The crate previously bound only
    /// the LIST endpoint, whose first-page/no-cursor honest gap makes it unfit
    /// for a per-ticker pre-flight; this is one signed read with no pagination
    /// surface.
    ///
    /// Live `GET /markets/KXMLBGAME-26SEP042010TORKC-TOR` [2026-09-02T16:18:31Z,
    /// api.elections.kalshi.com]: HTTP 200, envelope `{"market": {...}}` with
    /// `exchange_index: 3`.
    pub async fn get_market(&self, ticker: &str) -> Result<Market> {
        let path = format!("/markets/{ticker}");
        let resp: GetMarketResponse = self.send(reqwest::Method::GET, &path, NO_BODY).await?;
        Ok(resp.market)
    }

    pub async fn orderbook(&self, ticker: &str) -> Result<OrderbookSnapshot> {
        let path = format!("/markets/{ticker}/orderbook");
        let resp: OrderbookResponse = self.send(reqwest::Method::GET, &path, NO_BODY).await?;
        Ok(resp.orderbook)
    }

    /// Place a new V2 order (POST /portfolio/events/orders). Refuses to run
    /// unless `KALSHI_ENABLE_LIVE=1`. The V2 response is a LIGHTWEIGHT flat ack
    /// ([`V2OrderResponse`]) — the full order is read back via [`Self::get_order`].
    pub async fn post_order(&self, order: &V2CreateOrderRequest) -> Result<V2OrderResponse> {
        require_live_flag()?;
        self.send(
            reqwest::Method::POST,
            "/portfolio/events/orders",
            Some(order),
        )
        .await
    }

    /// Cancel an open order (DELETE /portfolio/events/orders/{id}). Refuses
    /// unless `KALSHI_ENABLE_LIVE=1`. The V2 ack does NOT echo the order; read
    /// the resulting state back via [`Self::get_order`].
    ///
    /// SONA #700 — the sharding asymmetry this signature exists to respect:
    /// **create and amend take `exchange_index` in the JSON body; cancel takes
    /// it in the QUERY string.** Before this change `cancel_order` built no
    /// query at all, so a DELETE could only ever reach shard 0 — which silently
    /// broke the ADR-042 §1 halt cancel-sweep for any order resting on shards
    /// 1-3.
    ///
    /// Venue facts, Context7 `/openapi/kalshi_openapi_yaml` [2026-09-02]:
    /// `DELETE /portfolio/events/orders/{order_id}` parameters —
    /// "**exchange_index** (ExchangeIndex, query, optional)" and
    /// "**market_ticker** (string, query, optional): Market ticker. Required
    /// when exchange_index is -1 (auto)."
    ///
    /// `market_ticker` is plumbed because the spec makes it mandatory in the
    /// `-1` auto-routing case; SONA does not use `-1` (unverified in production
    /// on the order path), and passes the real ticker alongside the real shard
    /// so the request is unambiguous either way. Both are `Option` so an
    /// unsharded caller produces the byte-identical pre-#700 request.
    ///
    /// The signature payload is unaffected: [`Self::sig_path_for`] strips the
    /// query string before signing (Kalshi signs the PATH only), which is the
    /// same property `available_symbols_in_series` already relies on.
    pub async fn cancel_order(
        &self,
        order_id: &str,
        exchange_index: Option<i64>,
        market_ticker: Option<&str>,
    ) -> Result<V2CancelAck> {
        require_live_flag()?;
        let mut params: Vec<String> = Vec::new();
        if let Some(idx) = exchange_index {
            params.push(format!("exchange_index={idx}"));
        }
        if let Some(t) = market_ticker {
            if !t.is_empty() {
                params.push(format!("market_ticker={t}"));
            }
        }
        let path = if params.is_empty() {
            format!("/portfolio/events/orders/{order_id}")
        } else {
            format!("/portfolio/events/orders/{order_id}?{}", params.join("&"))
        };
        self.send(reqwest::Method::DELETE, &path, NO_BODY).await
    }

    /// Amend an existing open order (POST /portfolio/events/orders/{id}/amend).
    /// Refuses unless `KALSHI_ENABLE_LIVE=1`. The V2 ack does NOT echo the
    /// order; read the resulting state back via [`Self::get_order`].
    ///
    /// DO NOT change to PATCH — Kalshi returns 405 on PATCH for this endpoint.
    pub async fn amend_order(&self, order_id: &str, amend: &V2AmendOrder) -> Result<V2AmendAck> {
        require_live_flag()?;
        let path = format!("/portfolio/events/orders/{order_id}/amend");
        self.send(reqwest::Method::POST, &path, Some(amend)).await
    }

    /// Read back a single order by id (GET /portfolio/orders/{id}). This is a
    /// READ — NOT part of the V2 mutation deprecation, so it stays on its
    /// current path (ADR-018 §Amendment 2026-06-19) — and it is deliberately
    /// NOT gated on `KALSHI_ENABLE_LIVE`. It composes the witness receipt's
    /// resulting-state after a V2 cancel/amend, whose lightweight ack no longer
    /// echoes the order.
    pub async fn get_order(&self, order_id: &str) -> Result<OrderAck> {
        let path = format!("/portfolio/orders/{order_id}");
        self.send(reqwest::Method::GET, &path, NO_BODY).await
    }

    /// Read the member's balance + portfolio value (GET /portfolio/balance).
    /// READ — deliberately NOT gated on `KALSHI_ENABLE_LIVE` (only money-moving
    /// calls are). `balance` / `portfolio_value` are integer cents (issue #55
    /// item 7). Used by the SONA pre-flight balance gate.
    ///
    /// SONA #700 — the response now carries the modelled per-shard
    /// `balance_breakdown`; read one shard's cents via
    /// [`BalanceResponse::shard_balance_cents`]. The top-level `balance` is the
    /// AGGREGATE and is not a safe gate on a sharded exchange.
    ///
    /// `exchange_index` is the optional server-side filter. Context7
    /// `/openapi/kalshi_openapi_yaml` [2026-09-02] says it "has no effect" when
    /// `subaccount` is omitted or 0 — but the keyed live read of
    /// 2026-09-02T14:35Z shows the filter DOES take effect on the operator's
    /// primary account (`?exchange_index=3` returned top-level `balance: 0`).
    /// Spec and venue disagree, so SONA does not depend on the filter: the
    /// unfiltered read plus `shard_balance_cents` is the path the money code
    /// takes, and this parameter exists for probes and for parity with the spec.
    pub async fn get_balance(&self, exchange_index: Option<i64>) -> Result<BalanceResponse> {
        let path = match exchange_index {
            Some(idx) => format!("/portfolio/balance?exchange_index={idx}"),
            None => "/portfolio/balance".to_string(),
        };
        self.send(reqwest::Method::GET, &path, NO_BODY).await
    }

    /// One page of the member's market positions (GET /portfolio/positions).
    /// READ — not gated. Pass `ticker` to scope to one market (the ramp-
    /// reconciliation path scopes to the traded ticker); pass `cursor` to fetch
    /// a subsequent page. An empty-string / absent response `cursor` means no
    /// next page; use [`Self::get_all_positions`] for the walked read (issue #55
    /// item 7).
    pub async fn get_positions(
        &self,
        ticker: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<PositionsResponse> {
        let mut params: Vec<String> = Vec::new();
        if let Some(t) = ticker {
            params.push(format!("ticker={t}"));
        }
        if let Some(c) = cursor {
            if !c.is_empty() {
                params.push(format!("cursor={c}"));
            }
        }
        let path = if params.is_empty() {
            "/portfolio/positions".to_string()
        } else {
            format!("/portfolio/positions?{}", params.join("&"))
        };
        self.send(reqwest::Method::GET, &path, NO_BODY).await
    }

    /// One page of the member's fills (GET /portfolio/fills). READ — not gated.
    /// Scope by `ticker` and/or `order_id` (gap-fill on reconnect / audit
    /// reconciliation); pass `cursor` to fetch a subsequent page. An empty-
    /// string / absent response `cursor` means no next page; use
    /// [`Self::get_all_fills`] for the walked read (issue #55 item 7).
    pub async fn get_fills(
        &self,
        ticker: Option<&str>,
        order_id: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<FillsResponse> {
        let mut params: Vec<String> = Vec::new();
        if let Some(t) = ticker {
            params.push(format!("ticker={t}"));
        }
        if let Some(o) = order_id {
            params.push(format!("order_id={o}"));
        }
        if let Some(c) = cursor {
            if !c.is_empty() {
                params.push(format!("cursor={c}"));
            }
        }
        let path = if params.is_empty() {
            "/portfolio/fills".to_string()
        } else {
            format!("/portfolio/fills?{}", params.join("&"))
        };
        self.send(reqwest::Method::GET, &path, NO_BODY).await
    }

    /// One page of GET /portfolio/orders (SONA ADR-042 §1 — the halt
    /// cancel-sweep's order inventory). Signed read; NOT gated on
    /// `KALSHI_ENABLE_LIVE` (it mutates nothing). `status` filters server-side
    /// (`resting` | `canceled` | `executed`); resting orders are always
    /// available on this endpoint (never historical-only). An empty-string /
    /// absent response `cursor` means no next page; use
    /// [`Self::get_resting_orders`] for the walked read.
    pub async fn get_orders(
        &self,
        status: Option<&str>,
        ticker: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<OrdersResponse> {
        let mut params: Vec<String> = Vec::new();
        if let Some(s) = status {
            params.push(format!("status={s}"));
        }
        if let Some(t) = ticker {
            params.push(format!("ticker={t}"));
        }
        if let Some(c) = cursor {
            if !c.is_empty() {
                params.push(format!("cursor={c}"));
            }
        }
        let path = if params.is_empty() {
            "/portfolio/orders".to_string()
        } else {
            format!("/portfolio/orders?{}", params.join("&"))
        };
        self.send(reqwest::Method::GET, &path, NO_BODY).await
    }

    /// EVERY resting order, cursor-walked to exhaustion (SONA ADR-042 §1: the
    /// cancel sweep is only as good as its inventory — a single-page read
    /// would silently miss resting orders past the first page, the exact
    /// truncation trap the fork's `list_markets` is documented to have).
    /// Pages are capped at [`Self::ORDERS_WALK_MAX_PAGES`] as a runaway guard
    /// (default server page = 100 orders; the cap is far above any real SONA
    /// book); hitting the cap returns an error rather than a silently
    /// truncated inventory — fail loud, never fabricate completeness.
    pub async fn get_resting_orders(&self, ticker: Option<&str>) -> Result<Vec<OrderRecord>> {
        let mut all: Vec<OrderRecord> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..Self::ORDERS_WALK_MAX_PAGES {
            let page = self
                .get_orders(Some("resting"), ticker, cursor.as_deref())
                .await?;
            all.extend(page.orders);
            match page.cursor.as_deref() {
                Some("") | None => return Ok(all),
                Some(next) => cursor = Some(next.to_string()),
            }
        }
        Err(KalshiError::Api {
            status: 0,
            body: format!(
                "get_resting_orders: cursor walk exceeded {} pages — refusing to return a \
                 possibly-truncated inventory",
                Self::ORDERS_WALK_MAX_PAGES
            ),
        })
    }

    /// Runaway guard for [`Self::get_resting_orders`]'s cursor walk.
    pub const ORDERS_WALK_MAX_PAGES: usize = 50;

    /// EVERY market position, cursor-walked to exhaustion (mirrors
    /// [`Self::get_resting_orders`]: a single-page read would silently miss
    /// positions past the first page, and a reconciliation is only as good as
    /// its inventory). Pages are capped at [`Self::POSITIONS_WALK_MAX_PAGES`] as
    /// a runaway guard; hitting the cap returns an error rather than a silently
    /// truncated inventory — fail loud, never fabricate completeness.
    ///
    /// Return shape: `Vec<MarketPosition>` — the concatenation of every page's
    /// `market_positions`. The spec's separate `event_positions` array is NOT
    /// surfaced: [`PositionsResponse`] deliberately does not decode it (forward-
    /// compat), so the single-page [`Self::get_positions`] does not expose it
    /// either; the walk drops no field the single-page read carries.
    pub async fn get_all_positions(&self, ticker: Option<&str>) -> Result<Vec<MarketPosition>> {
        let mut all: Vec<MarketPosition> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..Self::POSITIONS_WALK_MAX_PAGES {
            let page = self.get_positions(ticker, cursor.as_deref()).await?;
            all.extend(page.market_positions);
            match page.cursor.as_deref() {
                Some("") | None => return Ok(all),
                Some(next) => cursor = Some(next.to_string()),
            }
        }
        Err(KalshiError::Api {
            status: 0,
            body: format!(
                "get_all_positions: cursor walk exceeded {} pages — refusing to return a \
                 possibly-truncated inventory",
                Self::POSITIONS_WALK_MAX_PAGES
            ),
        })
    }

    /// Runaway guard for [`Self::get_all_positions`]'s cursor walk.
    pub const POSITIONS_WALK_MAX_PAGES: usize = 50;

    /// EVERY fill, cursor-walked to exhaustion (mirrors
    /// [`Self::get_resting_orders`]: a single-page read would silently miss
    /// fills past the first page — the exact gap-fill/audit-reconciliation trap
    /// this endpoint feeds). Pages are capped at [`Self::FILLS_WALK_MAX_PAGES`]
    /// as a runaway guard; hitting the cap returns an error rather than a
    /// silently truncated inventory — fail loud, never fabricate completeness.
    ///
    /// Return shape: `Vec<RestFill>` — the concatenation of every page's
    /// `fills` ([`FillsResponse`] carries a single `fills` list, so no field is
    /// dropped). Scope with `ticker` / `order_id` before walking (a per-order
    /// gap-fill rarely spans pages, but the walk still guarantees completeness).
    pub async fn get_all_fills(
        &self,
        ticker: Option<&str>,
        order_id: Option<&str>,
    ) -> Result<Vec<RestFill>> {
        let mut all: Vec<RestFill> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..Self::FILLS_WALK_MAX_PAGES {
            let page = self.get_fills(ticker, order_id, cursor.as_deref()).await?;
            all.extend(page.fills);
            match page.cursor.as_deref() {
                Some("") | None => return Ok(all),
                Some(next) => cursor = Some(next.to_string()),
            }
        }
        Err(KalshiError::Api {
            status: 0,
            body: format!(
                "get_all_fills: cursor walk exceeded {} pages — refusing to return a \
                 possibly-truncated inventory",
                Self::FILLS_WALK_MAX_PAGES
            ),
        })
    }

    /// Runaway guard for [`Self::get_all_fills`]'s cursor walk.
    pub const FILLS_WALK_MAX_PAGES: usize = 50;
}

fn require_live_flag() -> Result<()> {
    if std::env::var("KALSHI_ENABLE_LIVE").ok().as_deref() == Some("1") {
        Ok(())
    } else {
        Err(KalshiError::Api {
            status: 0,
            body: "live trading disabled (set KALSHI_ENABLE_LIVE=1 to enable)".into(),
        })
    }
}

const NO_BODY: Option<&()> = None;

fn url_join(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    let b = base.trim_end_matches('/');
    let p = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("{b}{p}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Signer;
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::RsaPrivateKey;

    fn test_signer() -> Signer {
        let mut rng = rand::thread_rng();
        let key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let pem = key
            .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
            .unwrap()
            .to_string();
        Signer::from_pem("test-key", &pem).unwrap()
    }

    #[test]
    fn url_join_handles_trailing_and_leading_slashes() {
        assert_eq!(
            url_join("https://example.com/trade-api/v2/", "/markets"),
            "https://example.com/trade-api/v2/markets"
        );
        assert_eq!(
            url_join("https://example.com/trade-api/v2", "markets"),
            "https://example.com/trade-api/v2/markets"
        );
    }

    #[test]
    fn sig_path_uses_full_url_path() {
        let client =
            RestClient::new("https://trading-api.kalshi.com/trade-api/v2", test_signer()).unwrap();
        let p = client.sig_path_for("/markets");
        assert_eq!(p, "/trade-api/v2/markets");
    }

    #[tokio::test]
    async fn post_order_refuses_without_live_flag() {
        // Ensure the flag is not set.
        std::env::remove_var("KALSHI_ENABLE_LIVE");
        let client =
            RestClient::new("https://trading-api.kalshi.com/trade-api/v2", test_signer()).unwrap();
        let order = crate::models::V2CreateOrderRequest {
            ticker: "X".into(),
            side: crate::models::V2Side::Bid,
            count: "1.00".into(),
            price: "0.24".into(),
            time_in_force: crate::models::TimeInForce::GoodTillCanceled,
            self_trade_prevention_type: crate::models::SelfTradePreventionType::TakerAtCross,
            client_order_id: "t-1".into(),
            exchange_index: None,
        };
        let err = client.post_order(&order).await.unwrap_err();
        match err {
            KalshiError::Api { status: 0, body } => {
                assert!(body.contains("live trading disabled"));
            }
            other => panic!("expected Api status=0 error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancel_order_refuses_without_live_flag() {
        std::env::remove_var("KALSHI_ENABLE_LIVE");
        let client =
            RestClient::new("https://trading-api.kalshi.com/trade-api/v2", test_signer()).unwrap();
        let err = client
            .cancel_order("some-order-id", None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, KalshiError::Api { status: 0, .. }));
    }

    #[tokio::test]
    async fn amend_order_refuses_without_live_flag() {
        std::env::remove_var("KALSHI_ENABLE_LIVE");
        let client =
            RestClient::new("https://trading-api.kalshi.com/trade-api/v2", test_signer()).unwrap();
        let amend = crate::models::V2AmendOrder {
            ticker: "X".into(),
            side: crate::models::V2Side::Bid,
            price: "0.25".into(),
            count: "1.00".into(),
            client_order_id: None,
            updated_client_order_id: None,
            exchange_index: None,
        };
        let err = client
            .amend_order("some-order-id", &amend)
            .await
            .unwrap_err();
        assert!(matches!(err, KalshiError::Api { status: 0, .. }));
    }
}
