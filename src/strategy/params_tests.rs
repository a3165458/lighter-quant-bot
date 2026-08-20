//! `create_strategy*` 的库存政策参数解析与实盘拒绝规则。

use super::*;
use crate::strategy::grid_strategy::tests::snapshot;
use serial_test::serial;

fn live_config(inventory_mode: Option<&str>) -> Config {
    let mut b = Config::builder()
        .set_override("trading.strategies.grid_trading.enabled", true)
        .unwrap()
        .set_override("trading.strategies.grid_trading.grid_count", 12)
        .unwrap()
        .set_override("trading.strategies.grid_trading.investment_per_grid", 30.0)
        .unwrap()
        .set_override("trading.strategies.grid_trading.price_deviation", 0.004)
        .unwrap();
    if let Some(mode) = inventory_mode {
        b = b
            .set_override("trading.strategies.grid_trading.inventory_mode", mode)
            .unwrap();
    }
    b.build().unwrap()
}

#[test]
fn params_default_to_hard_when_unspecified() {
    let s =
        create_strategy_with_params("grid", Some("grid_count=12,investment=30,deviation=0.004"))
            .expect("默认构造");
    assert_eq!(s.name(), "grid_trading");
}

#[test]
fn params_accept_soft_mode() {
    assert!(create_strategy_with_params(
        "grid",
        Some(
            "grid_count=12,investment=30,deviation=0.004,inventory_mode=soft,soft_cap=5,hard_cap=8"
        )
    )
    .is_ok());
}

#[test]
fn params_reject_unknown_mode_and_bad_caps() {
    assert!(create_strategy_with_params("grid", Some("inventory_mode=loose")).is_err());
    assert!(
        create_strategy_with_params("grid", Some("inventory_mode=soft,soft_cap=8,hard_cap=5"))
            .is_err(),
        "hard 必须大于 soft"
    );
    assert!(
        create_strategy_with_params("grid", Some("inventory_mode=soft,soft_cap=abc")).is_err(),
        "非数字上限必须报错而不是静默取默认值"
    );
}

#[tokio::test]
async fn soft_params_actually_scale_signals() {
    // 通过 trait 驱动，证明参数确实生效（而不仅仅是构造成功）
    let s = create_strategy_with_params(
        "grid",
        Some(
            "grid_count=6,investment=100,deviation=0.03,inventory_mode=soft,soft_cap=5,hard_cap=8",
        ),
    )
    .unwrap();
    s.evaluate(&snapshot("BTC", 1_700_000_000, 100.0))
        .await
        .unwrap();
    let mut snap = snapshot("BTC", 1_700_000_100, 98.5);
    snap.positions.insert("BTC".to_string(), 6.5 * 100.0 / 98.5); // 6.5 格
    let sigs = s.evaluate(&snap).await.unwrap().expect("缩量信号");
    assert!((sigs[0].quantity * sigs[0].price - 50.0).abs() < 1e-9);
}

#[test]
#[serial]
fn research_nocap_requires_explicit_research_flag() {
    std::env::remove_var("SOFT_CAP_RESEARCH");
    assert!(
        create_strategy_with_params("grid", Some("inventory_mode=research_nocap")).is_err(),
        "缺开关时必须拒绝"
    );

    std::env::set_var("SOFT_CAP_RESEARCH", "1");
    let ok = create_strategy_with_params("grid", Some("inventory_mode=research_nocap"));
    std::env::remove_var("SOFT_CAP_RESEARCH");
    assert!(ok.is_ok(), "带研究开关时回测路径应放行");
}

#[test]
#[serial]
fn live_path_rejects_research_nocap_even_with_flag() {
    std::env::set_var("SOFT_CAP_RESEARCH", "1");
    let res = create_strategy(&live_config(Some("research_nocap")));
    std::env::remove_var("SOFT_CAP_RESEARCH");
    assert!(res.is_err(), "实盘 yaml 路径必须无条件拒绝 research_nocap");
}

#[test]
fn live_path_defaults_to_hard_and_accepts_soft() {
    assert!(create_strategy(&live_config(None)).is_ok());
    assert!(create_strategy(&live_config(Some("soft"))).is_ok());
    assert!(create_strategy(&live_config(Some("nonsense"))).is_err());
}

fn mm_live_config(inventory_mode: Option<&str>) -> Config {
    let mut b = Config::builder()
        .set_override("trading.strategies.grid_trading.enabled", false)
        .unwrap()
        .set_override("trading.strategies.trend_following.enabled", false)
        .unwrap()
        .set_override("trading.strategies.market_making.enabled", true)
        .unwrap()
        .set_override("trading.strategies.market_making.bid_spread", 0.001)
        .unwrap()
        .set_override("trading.strategies.market_making.ask_spread", 0.001)
        .unwrap();
    if let Some(mode) = inventory_mode {
        b = b
            .set_override("trading.strategies.market_making.inventory_mode", mode)
            .unwrap();
    }
    b.build().unwrap()
}

#[test]
fn live_mm_defaults_to_vol_obi_engine() {
    let strat = create_strategy(&mm_live_config(None)).unwrap();
    assert_eq!(strat.name(), "market_making");
}

#[test]
fn factory_rejects_unknown_names_and_still_constructs_grid_trend_mm() {
    assert!(create_strategy_with_params("not_a_strategy", None).is_err());
    assert_eq!(
        create_strategy_with_params("grid", None).unwrap().name(),
        "grid_trading"
    );
    assert_eq!(
        create_strategy_with_params("trend", None).unwrap().name(),
        "trend_following"
    );
    assert_eq!(
        create_strategy_with_params("mm", Some("bid_spread=0.001,ask_spread=0.001"))
            .unwrap()
            .name(),
        "market_making"
    );
    assert_eq!(
        create_strategy_with_params("market_making", None)
            .unwrap()
            .name(),
        "market_making"
    );
}

#[test]
#[serial]
fn live_mm_path_rejects_research_nocap_even_with_flag() {
    std::env::set_var("SOFT_CAP_RESEARCH", "1");
    let res = create_strategy(&mm_live_config(Some("research_nocap")));
    std::env::remove_var("SOFT_CAP_RESEARCH");
    assert!(res.is_err(), "实盘 yaml 路径必须无条件拒绝 research_nocap");
}

#[tokio::test]
async fn factory_mm_emits_two_sided_maker_quotes() {
    let s = create_strategy_with_params(
        "mm",
        Some("quote_engine=simple,bid_spread=0.001,ask_spread=0.001"),
    )
    .expect("mm factory");
    let snap = snapshot("BTC", 1_700_000_000, 100.0);
    let sigs = s.evaluate(&snap).await.unwrap().expect("quotes");
    assert_eq!(sigs.len(), 2);
    assert!(sigs
        .iter()
        .all(|sig| sig.order_type == crate::lighter::types::OrderType::Limit));
    let mid = 100.0;
    let buy = sigs
        .iter()
        .find(|s| s.side == crate::lighter::types::Side::Buy)
        .unwrap();
    let sell = sigs
        .iter()
        .find(|s| s.side == crate::lighter::types::Side::Sell)
        .unwrap();
    assert!(buy.price < mid && mid < sell.price);
    assert!(buy.expected_edge_bps.unwrap_or(0.0) > 0.0);
}

#[test]
fn live_loop_applies_open_order_and_profitability_gates_to_all_signals() {
    let src = include_str!("../main.rs");
    assert!(
        src.contains("if current_open >= max_open_orders"),
        "live loop must still enforce max_open_orders"
    );
    assert!(
        src.contains("check_signal"),
        "live loop must still run the profitability/risk gate"
    );
    assert!(
        !src.contains("bypass max_open_orders"),
        "MM must not carve out an open-order bypass"
    );
}

#[test]
fn live_loop_reconciles_empty_exchange_open_orders_instead_of_ignoring() {
    let src = include_str!("../main.rs");
    assert!(
        src.contains("reconcile_open_order_count"),
        "live path must call the shipped open-order reconciler"
    );
    assert!(
        src.contains("ReconcileToExchange"),
        "live path must reset local ghosts when exchange is confirmed empty"
    );
    assert!(
        !src.contains("Ignoring open-order sync of 0"),
        "must not keep a ghost working order by ignoring exchange 0 forever"
    );
}

#[test]
fn live_loop_re_resolves_universe_from_collected_bbos() {
    let src = include_str!("../main.rs");
    assert!(
        src.contains("resolve_live_universe"),
        "live path must call the shipped universe resolver"
    );
    assert!(
        src.contains("latest_bbos"),
        "live path must collect BBO updates"
    );
    assert!(
        src.contains("Auto universe re-ranked from live BBO"),
        "live path must re-rank after books exist"
    );
    assert!(
        !src.contains("resolve_quoting_market_ids(catalog, &[], &params)"),
        "empty-BBO first-N fallback must not remain on the live path"
    );
}
