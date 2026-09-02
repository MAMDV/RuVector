//! Glue between `neural_trader_strategies::Intent` and the Kalshi V2
//! `V2CreateOrderRequest` payload. Kept in this crate (not the strategies
//! crate) so strategies remain venue-agnostic.
//!
//! # The YES-book mapping (real-money-correctness landmine)
//!
//! Kalshi's V2 order book is single-book: every order is a `bid` or `ask`
//! quoted on the YES leg, at a fixed-point **dollar** price. A strategy
//! expresses a binary-market intent as an `action` (Buy/Sell) on a `side`
//! (Yes/No) at a price in **cents** `p`. The two map as:
//!
//! | intent (action, side) @ p cents | V2 `side` | V2 `price` (YES book) |
//! |---------------------------------|-----------|-----------------------|
//! | Buy  YES @ p                    | `bid`     | p/100                 |
//! | Sell YES @ p                    | `ask`     | p/100                 |
//! | Buy  NO  @ q                    | `ask`     | (100-q)/100           |
//! | Sell NO  @ q                    | `bid`     | (100-q)/100           |
//!
//! A NO order is the complementary YES order: buying NO @ q is economically
//! selling YES @ (100-q) (a YES+NO pair settles at $1), so it rests on the YES
//! book as an `ask` at the complementary price. A flipped mapping is a
//! wrong-direction live order, so all four combos are unit-tested below.
//!
//! The wire `price` (the YES-book price) is distinct from the order's
//! **notional** (cents actually outlaid == the original leg price p): the SONA
//! adapter computes notional from the intent's `limit_price_cents`, NOT from
//! this inverted wire price.

use neural_trader_strategies::{Action, Intent, Side};

use crate::models::{
    cents_to_dollars_str, contracts_to_count_fp, SelfTradePreventionType, TimeInForce,
    V2CreateOrderRequest, V2Side,
};

/// Convert a strategy `Intent` into a Kalshi V2 limit [`V2CreateOrderRequest`].
/// The caller must supply a ticker — the `Intent` holds a canonical
/// `symbol_id` (FNV-1a of the ticker) and recovering the original string is
/// not possible, so the caller, who knows which ticker it's submitting for,
/// provides it directly.
///
/// `time_in_force` is `good_till_canceled` (a resting limit order, preserving
/// the legacy `OrderType::Limit` semantics) and `self_trade_prevention_type`
/// is `taker_at_cross` (issue #63 D2 venue-native self-cross block). A future
/// scalping wave that needs IOC/FOK can thread those through explicitly.
pub fn intent_to_order(
    ticker: impl Into<String>,
    intent: &Intent,
    client_order_id: impl Into<String>,
) -> V2CreateOrderRequest {
    // Map (action, side) -> (V2 single-book side, YES-book price in cents). See
    // the module doc table; a flipped arm = wrong-direction live order.
    let p = intent.limit_price_cents;
    let (side, yes_book_price_cents) = match (intent.action, intent.side) {
        (Action::Buy, Side::Yes) => (V2Side::Bid, p),
        (Action::Sell, Side::Yes) => (V2Side::Ask, p),
        // NO leg -> complementary YES price (a contract pair settles at 100c).
        (Action::Buy, Side::No) => (V2Side::Ask, 100 - p),
        (Action::Sell, Side::No) => (V2Side::Bid, 100 - p),
    };

    V2CreateOrderRequest {
        ticker: ticker.into(),
        side,
        count: contracts_to_count_fp(intent.quantity),
        price: cents_to_dollars_str(yes_book_price_cents),
        time_in_force: TimeInForce::GoodTillCanceled,
        self_trade_prevention_type: SelfTradePreventionType::TakerAtCross,
        client_order_id: client_order_id.into(),
        // SONA #700 — the shard is a property of the MARKET, not of the intent,
        // so it is not resolvable here (this fn takes no venue client). The
        // caller reads `Market.exchange_index` and sets it on the returned
        // request before submitting. `None` serializes to nothing, leaving the
        // body byte-identical to the pre-#700 request.
        exchange_index: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neural_trader_strategies::{Action, Intent, Side};

    fn intent(action: Action, side: Side, price_cents: i64, qty: i64) -> Intent {
        Intent {
            symbol_id: 42,
            side,
            action,
            limit_price_cents: price_cents,
            quantity: qty,
            edge_bps: 0,
            confidence: 0.0,
            strategy: "t",
        }
    }

    #[test]
    fn buy_yes_maps_to_bid_at_face_price() {
        let o = intent_to_order(
            "FED-DEC23",
            &intent(Action::Buy, Side::Yes, 24, 10),
            "cli-1",
        );
        assert_eq!(o.ticker, "FED-DEC23");
        assert_eq!(o.client_order_id, "cli-1");
        assert_eq!(o.side, V2Side::Bid);
        assert_eq!(o.price, "0.24");
        assert_eq!(o.count, "10.00");
        assert_eq!(o.time_in_force, TimeInForce::GoodTillCanceled);
        assert_eq!(
            o.self_trade_prevention_type,
            SelfTradePreventionType::TakerAtCross
        );
    }

    #[test]
    fn sell_yes_maps_to_ask_at_face_price() {
        let o = intent_to_order("X", &intent(Action::Sell, Side::Yes, 30, 7), "c");
        assert_eq!(o.side, V2Side::Ask);
        assert_eq!(o.price, "0.30");
        assert_eq!(o.count, "7.00");
    }

    #[test]
    fn buy_no_maps_to_ask_at_complementary_price() {
        // Buy NO @ 76c == sell YES @ 24c -> ask @ 0.24.
        let o = intent_to_order("X", &intent(Action::Buy, Side::No, 76, 5), "c");
        assert_eq!(o.side, V2Side::Ask);
        assert_eq!(o.price, "0.24");
        assert_eq!(o.count, "5.00");
    }

    #[test]
    fn sell_no_maps_to_bid_at_complementary_price() {
        // Sell NO @ 76c == buy YES @ 24c -> bid @ 0.24.
        let o = intent_to_order("X", &intent(Action::Sell, Side::No, 76, 5), "c");
        assert_eq!(o.side, V2Side::Bid);
        assert_eq!(o.price, "0.24");
    }
}
