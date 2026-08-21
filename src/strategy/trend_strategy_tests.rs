use super::*;
use chrono::{TimeZone, Utc};

fn snapshot_with_candles(symbol: &str, ts: i64, closes: &[f64]) -> MarketSnapshot {
    let mut snap = MarketSnapshot::default();
    let last = *closes.last().unwrap();
    snap.order_books.insert(
        symbol.to_string(),
        OrderBook {
            symbol: symbol.to_string(),
            market_id: 1,
            bids: vec![PriceLevel {
                price: last * 0.9995,
                quantity: 1.0,
            }],
            asks: vec![PriceLevel {
                price: last * 1.0005,
                quantity: 1.0,
            }],
            timestamp: Utc.timestamp_opt(ts, 0).unwrap(),
        },
    );
    let candles: Vec<Candlestick> = closes
        .iter()
        .enumerate()
        .map(|(i, &c)| Candlestick {
            timestamp: Utc
                .timestamp_opt(ts - (closes.len() - i) as i64 * 3600, 0)
                .unwrap(),
            open: c,
            high: c,
            low: c,
            close: c,
            volume: 1.0,
            symbol: symbol.to_string(),
        })
        .collect();
    snap.candles.insert(symbol.to_string(), candles);
    snap
}

#[test]
fn test_ema_last_two() {
    let prices = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let (prev, curr) = ema_last_two(&prices, 3).unwrap();
    assert!(curr > prev, "上升序列中 EMA 应递增");
    assert!(ema_last_two(&prices[..3], 3).is_none());
}

#[test]
fn test_check_exit_stop_loss_and_take_profit() {
    let strategy = TrendStrategy::new(10, 30, 0.05, 0.1);
    let pos = PositionState {
        side: Side::Buy,
        entry_price: 100.0,
        quantity: 1.0,
        best_price: 100.0,
    };
    assert!(strategy.check_exit(&pos, 94.0).is_some(), "跌超5%应止损");
    assert!(strategy.check_exit(&pos, 96.0).is_none());
    assert!(strategy.check_exit(&pos, 111.0).is_some(), "涨超10%应止盈");
    assert!(strategy.check_exit(&pos, 108.0).is_none());
}

#[test]
fn test_trailing_stop() {
    let strategy = TrendStrategy::with_options(10, 30, 0.05, 0.20, 0.02, 1000.0);
    let pos = PositionState {
        side: Side::Buy,
        entry_price: 100.0,
        quantity: 1.0,
        best_price: 105.0, // 已盈利 5% > 2% 启动线
    };
    // 从最优价 105 回撤 2% (=102.9) 触发移动止损
    assert!(strategy.check_exit(&pos, 102.8).is_some());
    assert!(strategy.check_exit(&pos, 103.5).is_none());
}

#[tokio::test]
async fn test_golden_cross_generates_buy_and_exit_tracks_position() {
    let strategy = TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 1000.0);

    // 下跌后 V 型反转，制造金叉；逐根推进评估，捕获交叉发生的那一根
    let mut closes: Vec<f64> = (0..12).map(|i| 110.0 - i as f64).collect(); // 110 -> 99
    closes.extend((0..6).map(|i| 100.0 + i as f64 * 3.0)); // 反弹到 115

    let mut buy_signal = None;
    for end in 8..=closes.len() {
        let snap = snapshot_with_candles("BTC", 1_700_000_000 + end as i64 * 3600, &closes[..end]);
        if let Some(sigs) = strategy.evaluate(&snap).await.unwrap() {
            buy_signal = Some(sigs[0].clone());
            break;
        }
    }
    let sig = buy_signal.expect("金叉应产生买入信号");
    assert_eq!(sig.side, Side::Buy);
    assert!(sig.quantity > 0.0);
    assert_eq!(sig.expected_edge_bps, Some(1000.0));
    assert!(!sig.risk_reducing);

    // 价格暴跌超过止损线 → 应产生平仓卖出信号
    let last = *closes.last().unwrap();
    let crash = [last; 8].iter().map(|&p| p * 0.80).collect::<Vec<f64>>();
    let mut closes2 = closes.clone();
    closes2.extend(crash);
    let snap2 = snapshot_with_candles("BTC", 1_700_010_000, &closes2);
    let signals2 = strategy.evaluate(&snap2).await.unwrap();
    assert!(signals2.is_some(), "跌破止损应产生平仓信号");
    let signals2 = signals2.unwrap();
    assert_eq!(signals2[0].side, Side::Sell);
    assert!(signals2[0].risk_reducing);
}

#[tokio::test]
async fn exchange_position_is_adopted_after_strategy_recreation() {
    let strategy = TrendStrategy::with_options(3, 6, 0.05, 0.10, 0.0, 50.0);
    let closes = vec![107.0; 12];
    let mut snapshot = snapshot_with_candles("BTC", 1_700_020_000, &closes);
    snapshot.positions_authoritative = true;
    snapshot.positions.insert("BTC".to_string(), -0.5);
    snapshot
        .position_entry_prices
        .insert("BTC".to_string(), 100.0);

    let signals = strategy
        .evaluate(&snapshot)
        .await
        .unwrap()
        .expect("existing short beyond its stop loss must be managed after recreation");
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].side, Side::Buy);
    assert_eq!(signals[0].quantity, 0.5);
    assert!(signals[0].risk_reducing);
}

/// Deterministic weak-then-confirm bull fixture (fast=3, slow=6, min_sep=0.05%).
/// Extra 14: weak gold cross (sep < threshold). Extra 15: same-regime sep confirms.
fn weak_bull_cross_series() -> Vec<f64> {
    let mut closes: Vec<f64> = (0..12).map(|i| 110.0 - i as f64).collect();
    closes.extend((1..=14).map(|i| 99.0 + i as f64 * 0.02));
    closes
}

fn weak_bull_confirm_series() -> Vec<f64> {
    let mut closes = weak_bull_cross_series();
    closes.push(99.78);
    closes
}

#[tokio::test]
async fn pending_weak_cross_emits_once_when_separation_confirms() {
    // Given: weak gold cross below min_separation, then same-direction separation confirms
    let strategy = TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 1000.0);
    let weak = weak_bull_cross_series();
    let confirm = weak_bull_confirm_series();

    // When: evaluate at weak cross bar
    let weak_sigs = strategy
        .evaluate(&snapshot_with_candles("BTC", 1_700_000_000, &weak))
        .await
        .unwrap();
    // Then: no entry yet (separation too small)
    assert!(weak_sigs.is_none(), "weak cross must not enter immediately");

    // When: later bar reaches min_separation in same regime
    let confirm_sigs = strategy
        .evaluate(&snapshot_with_candles("BTC", 1_700_003_600, &confirm))
        .await
        .unwrap()
        .expect("confirmed separation should emit one buy");
    assert_eq!(confirm_sigs.len(), 1);
    assert_eq!(confirm_sigs[0].side, Side::Buy);

    // When: evaluate again on same confirmed series
    let again = strategy
        .evaluate(&snapshot_with_candles("BTC", 1_700_007_200, &confirm))
        .await
        .unwrap();
    // Then: no duplicate entry
    assert!(again.is_none(), "confirmed pending must fire only once");
}

#[tokio::test]
async fn pending_invalidated_by_reverse_cross_does_not_fire() {
    // Given: weak gold pending, then death cross before separation confirms
    let strategy = TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 1000.0);
    let mut series = weak_bull_cross_series();
    let _ = strategy
        .evaluate(&snapshot_with_candles("BTC", 1_700_000_000, &series))
        .await
        .unwrap();

    // When: reverse cross replaces pending with sell (sep already sufficient)
    series.push(98.28);
    let after_reverse = strategy
        .evaluate(&snapshot_with_candles("BTC", 1_700_003_600, &series))
        .await
        .unwrap()
        .expect("strong death cross should emit sell");
    assert_eq!(after_reverse[0].side, Side::Sell);

    // Then: continue in bear without a new gold cross — no bull entry from old pending
    for i in 0..5 {
        series.push(series.last().copied().unwrap() - 0.2);
        let late = strategy
            .evaluate(&snapshot_with_candles(
                "BTC",
                1_700_007_200 + i * 3600,
                &series,
            ))
            .await
            .unwrap();
        if let Some(sigs) = late {
            assert_ne!(
                sigs[0].side,
                Side::Buy,
                "invalidated bull pending must not fire"
            );
        }
    }
}

#[tokio::test]
async fn opposite_cross_replaces_pending_direction() {
    // Given: weak gold pending
    let strategy = TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 1000.0);
    let mut series = weak_bull_cross_series();
    assert!(strategy
        .evaluate(&snapshot_with_candles("BTC", 1_700_000_000, &series))
        .await
        .unwrap()
        .is_none());

    // When: opposite death cross replaces pending (this fixture has sep >= threshold)
    series.push(98.28);
    let sigs = strategy
        .evaluate(&snapshot_with_candles("BTC", 1_700_003_600, &series))
        .await
        .unwrap()
        .expect("strong death cross should emit sell");
    assert_eq!(sigs[0].side, Side::Sell);
}

#[tokio::test]
async fn exit_in_ongoing_regime_does_not_reenter_without_new_cross() {
    // Given: weak pending then confirmed entry, then stop-loss exit
    let strategy = TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 1000.0);
    let weak = weak_bull_cross_series();
    assert!(strategy
        .evaluate(&snapshot_with_candles("BTC", 1_700_000_000, &weak))
        .await
        .unwrap()
        .is_none());
    let mut series = weak_bull_confirm_series();
    let entry = strategy
        .evaluate(&snapshot_with_candles("BTC", 1_700_003_600, &series))
        .await
        .unwrap()
        .expect("confirm bar should enter");
    assert_eq!(entry[0].side, Side::Buy);

    // When: hard stop exit
    let crash = series.last().copied().unwrap() * 0.80;
    series.extend(std::iter::repeat_n(crash, 3));
    let exit = strategy
        .evaluate(&snapshot_with_candles("BTC", 1_700_010_000, &series))
        .await
        .unwrap()
        .expect("stop loss should exit");
    assert_eq!(exit[0].side, Side::Sell);

    // Then: continue without a new crossover — no immediate re-entry
    for i in 0..7 {
        series.push(series.last().copied().unwrap() + 0.3);
        let sigs = strategy
            .evaluate(&snapshot_with_candles(
                "BTC",
                1_700_020_000 + i * 3600,
                &series,
            ))
            .await
            .unwrap();
        assert!(
            sigs.is_none(),
            "bar {i}: established regime after exit must not re-enter without new cross"
        );
    }
}

#[tokio::test]
async fn weak_death_cross_pending_confirms_sell() {
    // Given: rise then weak death cross (sep < threshold), then confirm
    let strategy = TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 1000.0);
    let mut closes: Vec<f64> = (0..12).map(|i| 99.0 + i as f64).collect(); // 99 -> 110
    closes.extend((1..=14).map(|i| 110.0 - i as f64 * 0.02));

    assert!(strategy
        .evaluate(&snapshot_with_candles("BTC", 1_700_000_000, &closes))
        .await
        .unwrap()
        .is_none());

    closes.push(closes.last().copied().unwrap() - 0.5);
    let sigs = strategy
        .evaluate(&snapshot_with_candles("BTC", 1_700_003_600, &closes))
        .await
        .unwrap()
        .expect("confirmed death cross should sell");
    assert_eq!(sigs[0].side, Side::Sell);
}

#[tokio::test]
async fn adx_gate_off_golden_cross_opens() {
    // Gate 关闭(阈值 0)时，boilerplate golden cross 应照常开仓（回归：默认行为不变）
    let strategy =
        TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 1000.0).with_adx_filter(0.0, 14);
    let mut closes: Vec<f64> = (0..14).map(|i| 110.0 - i as f64).collect();
    closes.extend((0..8).map(|i| 98.0 + i as f64 * 3.0));

    let mut opened = None;
    for end in 10..=closes.len() {
        let snap = snapshot_with_candles("BTC", 1_700_100_000 + end as i64 * 3600, &closes[..end]);
        if let Some(sigs) = strategy.evaluate(&snap).await.unwrap() {
            opened = Some(sigs[0].clone());
            break;
        }
    }
    assert!(opened.is_some(), "ADX 关闭时金叉应开仓");
    assert_eq!(opened.unwrap().side, Side::Buy);
}

#[tokio::test]
async fn adx_gate_on_impossible_high_blocks_open() {
    // 阈值设为不可能达到的 200（远超 ADX 0-100 上限），必定拦下——验证门禁接线
    // 其余 fixture 与 adx_gate_off_golden_cross_opens 完全一致，唯一变量是门槛。
    let strategy =
        TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 1000.0).with_adx_filter(200.0, 14);
    let mut closes: Vec<f64> = (0..14).map(|i| 110.0 - i as f64).collect();
    closes.extend((0..8).map(|i| 98.0 + i as f64 * 3.0));

    let mut opened = false;
    for end in 10..=closes.len() {
        let snap = snapshot_with_candles("BTC", 1_700_200_000 + end as i64 * 3600, &closes[..end]);
        if strategy.evaluate(&snap).await.unwrap().is_some() {
            opened = true;
            break;
        }
    }
    assert!(!opened, "ADX 阈值极高时应拦住金叉开仓");
}

#[tokio::test]
async fn slope_confirm_allows_rising_cross() {
    let strategy =
        TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 1000.0).with_slope_confirm(0.01, 5);
    // 先跌（制造死叉/熊市态），后加速上涨，形成金叉且 slow 斜率显著为正
    let mut closes: Vec<f64> = (0..14).map(|i| 110.0 - i as f64).collect();
    closes.extend((0..10).map(|i| 96.0 + i as f64 * 3.0));

    let mut opened = false;
    for end in 16..=closes.len() {
        let snap = snapshot_with_candles("BTC", 1_700_300_000 + end as i64 * 3600, &closes[..end]);
        if strategy.evaluate(&snap).await.unwrap().is_some() {
            opened = true;
            break;
        }
    }
    assert!(opened, "下跌后快速上涨形成的金叉应通过斜率确认");
}

#[tokio::test]
async fn live_restart_does_not_seed_entry_from_current_ema_regime() {
    // Same down-then-up fixture that produces a live-looking bull regime.
    let mut closes: Vec<f64> = (0..12).map(|i| 110.0 - i as f64).collect();
    closes.extend((0..8).map(|i| 98.0 + i as f64 * 3.0));
    let last = *closes.last().unwrap();

    let strategy = TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 60.0);
    let mut open = snapshot_with_candles("BTC", 1_700_800_000, &closes);
    open.positions_authoritative = true;
    open.positions.insert("BTC".to_string(), 0.000866);
    open.position_entry_prices.insert("BTC".to_string(), last);

    let first = strategy.evaluate(&open).await.unwrap();
    assert!(
        first
            .as_ref()
            .map(|sigs| sigs.iter().any(|sig| !sig.risk_reducing))
            .unwrap_or(false)
            == false,
        "if an open position already exists, never seed a new live entry"
    );

    let strategy_flat = TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 60.0);
    let mut flat = snapshot_with_candles("BTC", 1_700_800_000, &closes);
    flat.positions_authoritative = true;
    assert!(
        strategy_flat.evaluate(&flat).await.unwrap().is_none(),
        "still-crossed EMA regime after process start must not seed pending Buy"
    );
    assert!(
        strategy_flat.evaluate(&flat).await.unwrap().is_none(),
        "the same live snapshot must not enter until a fresh post-start cross"
    );
}

#[tokio::test]
async fn live_process_may_enter_only_after_a_fresh_cross() {
    let strategy = TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 60.0);
    let mut closes: Vec<f64> = (0..12).map(|i| 110.0 - i as f64).collect();
    closes.extend((0..8).map(|i| 98.0 + i as f64 * 3.0));
    let mut start = snapshot_with_candles("BTC", 1_700_900_000, &closes);
    start.positions_authoritative = true;
    assert!(
        strategy.evaluate(&start).await.unwrap().is_none(),
        "startup observation arms the live regime and must not enter"
    );

    let mut opened = false;
    for i in 0..16 {
        closes.push(closes.last().copied().unwrap() - 3.0);
        let mut snap = snapshot_with_candles("BTC", 1_700_910_000 + i * 3600, &closes);
        snap.positions_authoritative = true;
        let _ = strategy.evaluate(&snap).await.unwrap();
    }
    for i in 0..16 {
        closes.push(closes.last().copied().unwrap() + 3.5);
        let mut snap = snapshot_with_candles("BTC", 1_700_930_000 + i * 3600, &closes);
        snap.positions_authoritative = true;
        if let Some(sigs) = strategy.evaluate(&snap).await.unwrap() {
            if sigs
                .iter()
                .any(|sig| sig.side == Side::Buy && !sig.risk_reducing)
            {
                opened = true;
                break;
            }
        }
    }
    assert!(
        opened,
        "a genuine EMA cross after the process is up may still enter"
    );
}

#[test]
fn live_trend_source_never_seeds_pending_from_regime() {
    let src = include_str!("trend_strategy.rs");
    assert!(
        !src.contains("seeded live pending"),
        "restart must not seed a pending entry from the current EMA regime"
    );
    assert!(
        src.contains("not seeding an entry until a fresh cross after process start"),
        "live path must wait for a post-start cross"
    );
}

#[tokio::test]
async fn slope_confirm_blocks_flat_cross() {
    let strategy =
        TrendStrategy::with_options(3, 6, 0.05, 0.1, 0.0, 1000.0).with_slope_confirm(0.05, 5);
    let flat: Vec<f64> = (0..60).map(|i| 50.0 + (i % 2) as f64 * 0.02).collect();
    let mut opened = false;
    for end in 20..=flat.len() {
        let snap = snapshot_with_candles("BTC", 1_700_500_000 + end as i64 * 3600, &flat[..end]);
        if strategy.evaluate(&snap).await.unwrap().is_some() {
            opened = true;
            break;
        }
    }
    assert!(!opened, "横盘窄幅波动不应通过斜率确认");
}
