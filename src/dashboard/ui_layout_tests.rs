const DASHBOARD_HTML: &str = include_str!("ui/index.html");
const DASHBOARD_APP_JS: &str = include_str!("ui/app.js");
const DASHBOARD_AI_JS: &str = include_str!("ui/ai.js");

fn css_rule(selector: &str) -> &str {
    let selector_start = DASHBOARD_HTML
        .find(selector)
        .unwrap_or_else(|| panic!("missing CSS selector: {selector}"));
    let rule_start = DASHBOARD_HTML[selector_start..]
        .find('{')
        .map(|offset| selector_start + offset + 1)
        .expect("CSS rule must have an opening brace");
    let rule_end = DASHBOARD_HTML[rule_start..]
        .find('}')
        .map(|offset| rule_start + offset)
        .expect("CSS rule must have a closing brace");

    &DASHBOARD_HTML[rule_start..rule_end]
}

#[test]
fn event_stream_fills_its_desktop_card_and_stays_bounded_on_narrow_screens() {
    let event_title = DASHBOARD_HTML
        .find(r#"data-i18n="eventLog""#)
        .expect("event stream title must exist");
    let event_card = DASHBOARD_HTML[..event_title]
        .rfind("\n      <div class=\"sec")
        .map(|start| &DASHBOARD_HTML[start + 7..event_title])
        .expect("event stream card must exist");
    assert!(
        event_card.starts_with(r#"<div class="sec event-stream-section">"#),
        "the card containing the event stream needs a dedicated layout class"
    );

    let section_rule = css_rule(".event-stream-section {");
    assert!(section_rule.contains("display:flex"));
    assert!(section_rule.contains("flex-direction:column"));
    assert!(section_rule.contains("min-height:0"));

    let body_rule = css_rule(".event-stream-section > .sec-body {");
    assert!(body_rule.contains("flex:1"));
    assert!(body_rule.contains("min-height:270px"));
    // Full-bleed log rows: padding lives on each chip, not on the rail inset.
    assert!(body_rule.contains("padding:0"));

    let rail_rule = css_rule(".event-stream-section .rail {");
    assert!(rail_rule.contains("position:absolute"));
    assert!(rail_rule.contains("inset:0"));
    assert!(rail_rule.contains("max-height:none"));

    // Four-column log rows (time / kind / detail / age) fill the card width.
    let chip_rule = css_rule(".rail-chip {");
    assert!(chip_rule.contains("display:grid"));
    assert!(chip_rule.contains("grid-template-columns:auto auto 1fr auto"));
    assert!(chip_rule.contains("padding:8px 16px"));

    assert!(
        DASHBOARD_HTML.contains(
            ".event-stream-section .rail { position:static; max-height:360px; min-height:0; }"
        ),
        "stacked layouts must cap a long event stream"
    );
}

#[test]
fn event_stream_renders_server_persisted_history_instead_of_page_local_deltas() {
    assert!(DASHBOARD_HTML.contains("function renderEvents(events)"));
    assert!(DASHBOARD_HTML.contains("msg.type === 'events'"));
    assert!(DASHBOARD_HTML.contains("fetch('/api/events')"));
    assert!(!DASHBOARD_HTML.contains("rail('risk'"));
    assert!(!DASHBOARD_HTML.contains("rail('order'"));
    assert!(!DASHBOARD_HTML.contains("rail('fill'"));
}

#[test]
fn control_loop_lights_only_real_actions_at_their_orbit_contact_time() {
    assert!(
        DASHBOARD_HTML.contains(r#"id="loop-path" d="M 450,24"#),
        "the orbit must begin at PUSH so a state frame and the dot share one clock"
    );
    assert!(DASHBOARD_HTML.contains(
        r#"<animateMotion id="loop-motion" dur="3s" begin="indefinite" repeatCount="indefinite">"#
    ));

    for (node, delay) in [("risk", 750), ("order", 1500), ("fill", 2250)] {
        assert!(
            DASHBOARD_HTML.contains(&format!("scheduleLoopContact('{node}', {delay})")),
            "{node} must be checked when the dot reaches it"
        );
    }

    assert!(DASHBOARD_HTML.contains("queueLoopAction('risk')"));
    assert!(DASHBOARD_HTML.contains("queueLoopAction('order')"));
    assert!(DASHBOARD_HTML.contains("queueLoopAction('fill')"));
    assert!(DASHBOARD_HTML.contains("restartLoopMotion()"));
}

#[test]
fn dashboard_escapes_exchange_data_before_using_inner_html() {
    assert!(DASHBOARD_APP_JS.contains("const escapeHtml = value =>"));
    assert!(DASHBOARD_APP_JS.contains("replace(/[&<>\"']/g"));

    for escaped_value in [
        "escapeHtml(n.message)",
        "escapeHtml(p.symbol)",
        "escapeHtml(p.side)",
        "escapeHtml(p.size)",
        "escapeHtml(String(o.id).slice(-6))",
        "escapeHtml(o.symbol || 'BTC')",
        "escapeHtml(o.side)",
        "escapeHtml(total)",
        "escapeHtml(fill)",
        "escapeHtml(o.status || 'Open')",
        "escapeHtml(t.symbol || t.market)",
        "escapeHtml(t.side)",
        "escapeHtml(action)",
        "escapeHtml(asset)",
        "escapeHtml(shortDate)",
        "escapeHtml(l.msg)",
    ] {
        assert!(
            DASHBOARD_APP_JS.contains(escaped_value),
            "server-derived value must be HTML-escaped: {escaped_value}"
        );
    }

    for unsafe_interpolation in [
        ">${n.message}<",
        ">${p.symbol}<",
        ">${p.side}<",
        ">${o.side}<",
        ">${total}<",
        ">${fill}<",
        ">${action}<",
        ">${t.side}<",
        ">${asset}<",
        ">${l.msg}<",
    ] {
        assert!(
            !DASHBOARD_APP_JS.contains(unsafe_interpolation),
            "raw server-derived HTML interpolation remains: {unsafe_interpolation}"
        );
    }
}

#[test]
fn dynamic_market_order_and_fill_strings_are_escaped_before_inner_html_rendering() {
    assert!(
        DASHBOARD_HTML.contains("var safeSym = esc(sym);")
            && DASHBOARD_HTML.contains("var safeSide = esc(pos && pos.side || '');"),
        "the market tape must escape server-provided symbols and position sides"
    );

    for escaped_fill_field in [
        "esc(t.side || '')",
        "esc(t.symbol || t.market || '')",
        "esc(t.quantity || '')",
    ] {
        assert!(
            DASHBOARD_HTML.contains(escaped_fill_field),
            "fill toasts must escape {escaped_fill_field} before assigning innerHTML"
        );
    }

    assert!(
        DASHBOARD_HTML.contains("var safeOrderSymbol = esc(o.symbol || '—');")
            && DASHBOARD_HTML.contains("var safeOrderSide = esc(side);"),
        "the order swarm must escape server-provided symbols and sides"
    );

    for unsafe_fragment in [
        "'<span class=\"tape-sym\">' + sym + '</span>'",
        "'<div class=\"toast-main\">' + (t.side || '')",
        "'<span class=\"swarm-name\">' + (o.symbol || '—')",
    ] {
        assert!(
            !DASHBOARD_HTML.contains(unsafe_fragment),
            "raw server data must not reach innerHTML: {unsafe_fragment}"
        );
    }
}

#[test]
fn ai_api_key_is_persisted_in_browser_local_storage() {
    // Product decision: users asked to keep the key across refreshes.
    // Stored only in browser localStorage (not on the trading server).
    assert!(
        DASHBOARD_AI_JS.contains("key: document.getElementById('ai-key').value"),
        "API key must be serialized into localStorage settings"
    );
    assert!(
        DASHBOARD_AI_JS.contains("document.getElementById('ai-key').value = s.key"),
        "API key must be restored from localStorage"
    );
    assert!(
        DASHBOARD_AI_JS.contains("'ai-key'") || DASHBOARD_AI_JS.contains("\"ai-key\""),
        "API-key input must trigger settings persistence"
    );
}

#[test]
fn settings_exposes_restart_safe_network_selection() {
    for id in [
        "network-mainnet",
        "network-robinhood",
        "network-rest-url",
        "network-ws-url",
        "network-chain-id",
        "btn-save-network",
        "network-msg",
    ] {
        assert!(
            DASHBOARD_HTML.contains(&format!(r#"id="{id}""#)),
            "missing network setting control: {id}"
        );
    }
    assert!(DASHBOARD_HTML.contains("/api/network"));
    assert!(DASHBOARD_HTML.contains("requires_restart"));
    assert!(
        DASHBOARD_HTML.contains("credentials") || DASHBOARD_HTML.contains("凭据"),
        "network settings must warn that network credentials are isolated"
    );
}

#[test]
fn ai_lab_loads_datasets_and_aligns_dates() {
    assert!(
        DASHBOARD_AI_JS.contains("/api/backtest/datasets"),
        "AI Lab must load dataset catalog from the server"
    );
    assert!(
        DASHBOARD_AI_JS.contains("function applyDatasetDates")
            || DASHBOARD_AI_JS.contains("applyDatasetDates("),
        "selecting a dataset must realign start/end dates"
    );
}

#[test]
fn ai_lab_is_tool_using_quant_agent() {
    const DASHBOARD_AI_HTML: &str = include_str!("ui/ai.html");
    const QUANT_AGENT_JS: &str = include_str!("ui/quant_agent.js");
    assert!(
        DASHBOARD_AI_HTML.contains(r#"id="agent-chat""#),
        "AI Lab must expose an agent chat surface"
    );
    assert!(
        DASHBOARD_AI_HTML.contains("quant_agent.js"),
        "AI Lab must load the quant agent script"
    );
    assert!(
        QUANT_AGENT_JS.contains("run_backtest") && QUANT_AGENT_JS.contains("run_param_sweep"),
        "agent must expose real backtest tools"
    );
    assert!(
        QUANT_AGENT_JS.contains("tool_calls") || QUANT_AGENT_JS.contains("TOOLS"),
        "agent must use tool-calling protocol"
    );
    assert!(
        QUANT_AGENT_JS.contains("agentLoop") || QUANT_AGENT_JS.contains("function agentLoop"),
        "agent multi-step loop required"
    );
}

#[test]
fn ai_backtest_html_escapes_dynamic_text() {
    assert!(
        DASHBOARD_AI_JS.contains("escapeHtml(t('errPrefix') + msg)")
            || DASHBOARD_AI_JS.contains("escapeHtml(msg)"),
        "error messages must be HTML-escaped"
    );
    // Title is built then escaped as one string (strategy + data file).
    assert!(DASHBOARD_AI_JS.contains("escapeHtml(t('backtestOn')"));
    assert!(DASHBOARD_AI_JS.contains("escapeHtml(currentParams)"));
    assert!(DASHBOARD_AI_JS.contains("escapeHtml(currentStrategy)"));
}

#[test]
fn ai_optimize_sends_start_end_not_start_date_aliases() {
    // Regression: AI path used start_date/end_date while /api/backtest only
    // read start/end → silent empty metrics shown as 0% return.
    assert!(
        DASHBOARD_AI_JS.contains("start: document.getElementById('bt-start').value"),
        "AI optimize payload must use start"
    );
    assert!(
        DASHBOARD_AI_JS.contains("end: document.getElementById('bt-end').value"),
        "AI optimize payload must use end"
    );
    assert!(
        !DASHBOARD_AI_JS.contains("start_date: document.getElementById('bt-start').value"),
        "AI optimize must not send the broken start_date field"
    );
    assert!(
        DASHBOARD_AI_JS.contains("function runServerBacktest")
            || DASHBOARD_AI_JS.contains("runServerBacktest")
            || DASHBOARD_AI_JS.contains("function assertBacktestOk"),
        "failed backtests must abort the AI loop instead of faking 0%"
    );
}

#[test]
fn ai_lab_clamps_max_tokens_to_safe_completion_range() {
    assert!(
        DASHBOARD_AI_JS.contains("function clampMaxTokens"),
        "max_tokens must be clamped before API calls"
    );
    assert!(
        DASHBOARD_AI_JS.contains("MAX_TOKENS_MAX = 131072")
            || DASHBOARD_AI_JS.contains("MAX_TOKENS_MAX=131072"),
        "completion ceiling allows long-context model outputs"
    );
    assert!(
        DASHBOARD_AI_JS.contains("readMaxTokensFromUi"),
        "UI reader must normalize max_tokens"
    );
}

#[test]
fn quant_agent_supports_million_context_and_compact() {
    const QUANT_AGENT_JS: &str = include_str!("ui/quant_agent.js");
    const DASHBOARD_AI_HTML: &str = include_str!("ui/ai.html");
    assert!(
        QUANT_AGENT_JS.contains("DEFAULT_CONTEXT_WINDOW = 1000000")
            || QUANT_AGENT_JS.contains("1000000"),
        "agent default context should be 1M tokens"
    );
    assert!(
        QUANT_AGENT_JS.contains("function compactHistory")
            || QUANT_AGENT_JS.contains("compactHistory"),
        "agent must support history compact"
    );
    assert!(
        DASHBOARD_AI_HTML.contains(r#"id="ai-context-window""#),
        "UI must expose context window setting"
    );
    assert!(
        DASHBOARD_AI_HTML.contains(r#"id="agent-compact""#),
        "UI must expose manual Compact control"
    );
}

#[test]
fn ai_lab_right_panel_streams_process_and_thinking() {
    const DASHBOARD_AI_HTML: &str = include_str!("ui/ai.html");
    assert!(
        DASHBOARD_AI_HTML.contains(r#"id="process-stream""#),
        "right panel needs a live process stream"
    );
    assert!(
        DASHBOARD_AI_HTML.contains(r#"id="think-body""#),
        "right panel needs an AI thinking body"
    );
    assert!(
        DASHBOARD_AI_JS.contains("function addProcess")
            || DASHBOARD_AI_JS.contains("function renderThought"),
        "AI Lab must render process + thought into the right panel"
    );
}

#[test]
fn ai_lab_shares_dashboard_language_and_has_chinese_copy() {
    const DASHBOARD_AI_HTML: &str = include_str!("ui/ai.html");
    assert!(
        DASHBOARD_AI_JS.contains("lighter-lang"),
        "AI Lab must reuse the main dashboard language key"
    );
    assert!(
        DASHBOARD_AI_JS.contains("cn: {"),
        "AI Lab must ship a Chinese string pack"
    );
    assert!(
        DASHBOARD_AI_JS.contains("function applyI18n"),
        "AI Lab must apply i18n to data-i18n nodes"
    );
    assert!(
        DASHBOARD_AI_HTML.contains(r#"data-i18n="apiKey""#)
            || DASHBOARD_AI_HTML.contains(r#"data-i18n="btConfig""#),
        "AI Lab markup must mark translatable nodes"
    );
    assert!(
        DASHBOARD_AI_HTML.contains(r#"id="ai-lang-btn""#),
        "AI Lab must expose a language toggle"
    );
    assert!(
        DASHBOARD_AI_HTML.contains("Quant Bot Agent") || DASHBOARD_AI_HTML.contains("agent-send"),
        "Chinese/agent primary surface must exist"
    );
}

#[test]
fn ai_lab_layout_keeps_agent_primary_and_responsive_rules_authoritative() {
    const DASHBOARD_AI_HTML: &str = include_str!("ui/ai.html");
    assert!(DASHBOARD_AI_HTML.contains(r#"class="config-sidebar""#));
    assert!(DASHBOARD_AI_HTML.contains("grid-template-areas:\"config agent rail\""));
    assert!(DASHBOARD_AI_HTML.contains("grid-area:agent"));
    assert!(DASHBOARD_AI_HTML.contains("grid-area:config"));
    assert!(DASHBOARD_AI_HTML.contains("grid-area:rail"));
    assert!(
        DASHBOARD_AI_HTML.contains("grid-template-areas:\"agent\" \"config\" \"rail\""),
        "mobile must put the task surface before lengthy configuration"
    );

    let base_rail = DASHBOARD_AI_HTML
        .find(".agent-rail {")
        .expect("agent rail base rule");
    let responsive = DASHBOARD_AI_HTML
        .rfind("@media (max-width:1200px)")
        .expect("tablet responsive rule");
    assert!(
        responsive > base_rail,
        "responsive rules must come after base rules so they are not overridden"
    );
}

#[test]
fn ai_lab_runs_a_structured_strategy_research_mission() {
    const DASHBOARD_AI_HTML: &str = include_str!("ui/ai.html");
    const QUANT_AGENT_JS: &str = include_str!("ui/quant_agent.js");

    for control in [
        r#"id="strategy-mission""#,
        r#"id="mission-goal""#,
        r#"id="mission-risk""#,
        r#"id="mission-universe""#,
        r#"id="mission-run""#,
        r#"id="mission-stages""#,
        r#"id="strategy-candidates""#,
    ] {
        assert!(DASHBOARD_AI_HTML.contains(control), "missing {control}");
    }

    assert!(QUANT_AGENT_JS.contains("name: 'research_strategies'"));
    assert!(QUANT_AGENT_JS.contains("function scoreStrategyCandidate"));
    assert!(QUANT_AGENT_JS.contains("async function startStrategyMission"));
    assert!(QUANT_AGENT_JS.contains("fetch('/api/status')"));
    assert!(QUANT_AGENT_JS.contains("fetch('/api/positions')"));
    assert!(QUANT_AGENT_JS.contains("toolSweep"));
    assert!(QUANT_AGENT_JS.contains("runAdaptiveAiResearch"));
    assert!(QUANT_AGENT_JS.contains("MAX_AI_RESEARCH_ROUNDS"));
    assert!(QUANT_AGENT_JS.contains("var MODEL_RESEARCH_TIMEOUT_MS = 120000"));
    assert!(QUANT_AGENT_JS.contains("timeoutMs: MODEL_RESEARCH_TIMEOUT_MS"));
    assert!(QUANT_AGENT_JS.contains("maxTokens: 1600"));
    assert!(QUANT_AGENT_JS.contains("opts.timeoutMs || 45000"));
    assert!(QUANT_AGENT_JS.contains("max_notional_usd"));
    assert!(QUANT_AGENT_JS.contains("livePolicy: research.live_policy"));
    assert!(QUANT_AGENT_JS.contains("live_eligible"));
    assert!(DASHBOARD_AI_HTML.contains(r#"data-stage="ai""#));
    assert!(
        !QUANT_AGENT_JS.contains("startStrategyMission")
            || !QUANT_AGENT_JS.contains("startStrategyMission();\n        toolApplyLive"),
        "research mission must never auto-apply a live strategy"
    );
}

#[test]
fn strategies_page_exposes_market_making_and_universe_controls() {
    assert!(
        DASHBOARD_HTML.contains(r#"id="btn-activate-mm""#),
        "Strategies page must have an Activate Market Making control"
    );
    assert!(
        DASHBOARD_HTML.contains(r#"id="cfg-mm-bid""#)
            && DASHBOARD_HTML.contains(r#"id="cfg-mm-ask""#)
            && DASHBOARD_HTML.contains(r#"id="cfg-mm-notional""#),
        "MM card must expose bid/ask spread and notional"
    );
    assert!(
        DASHBOARD_HTML.contains(r#"id="tc-universe-mode""#),
        "Trading controls must expose universe mode"
    );
    assert!(
        DASHBOARD_APP_JS.contains("strategy: 'market_making'"),
        "Apply MM must POST the shipped factory name"
    );
    assert!(
        DASHBOARD_APP_JS.contains("universe_mode"),
        "Save markets must send universe_mode to /api/trading/markets"
    );
    assert!(
        DASHBOARD_HTML.contains(r#"id="btn-open-mm""#),
        "home dashboard must link to MM settings"
    );
    let mm_btn = DASHBOARD_HTML.find(r#"id="btn-activate-mm""#).unwrap();
    let dca_btn = DASHBOARD_HTML.find(r#"id="btn-activate-dca""#).unwrap();
    assert!(
        mm_btn < dca_btn,
        "MM card must appear before DCA so it is visible without scrolling past other strategies"
    );
}

#[test]
fn total_pnl_display_prefers_equity_identity_over_accumulated_realized() {
    assert!(
        DASHBOARD_APP_JS.contains("inception_pnl"),
        "headline PnL must use equity - initial, not drifted realized"
    );
    assert!(
        DASHBOARD_APP_JS.contains("d.equity - d.initial_equity")
            || DASHBOARD_APP_JS.contains("data.equity - data.initial_equity"),
        "fallback must still be mark-to-market, not raw total_realized_pnl first"
    );
}

#[test]
fn history_volume_counts_fills_not_quote_placements() {
    assert!(
        DASHBOARD_APP_JS.contains("const isFillTrade = t =>"),
        "local history fallback must ignore unfilled quote placements"
    );
    assert!(
        DASHBOARD_APP_JS.contains("if (!isFillTrade(t)) return;"),
        "volume fallback must skip non-fills"
    );
    assert!(
        DASHBOARD_APP_JS
            .contains("'Full Close', 'Partial Close', 'Emergency Close', 'Liquidation'"),
        "close counting must use exact fill actions, not /stop/ substrings"
    );
    assert!(
        !DASHBOARD_APP_JS.contains("/Close|Stop|Emergency|Liquidat/i"),
        "loose close regex treated every placement reason containing stop as a close"
    );
}
