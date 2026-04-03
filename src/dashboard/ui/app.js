// Lighter Bot — Dashboard Logic (Full-Featured)
(function() {
    'use strict';

    // ── Config ──
    const MAX_EQUITY_PTS = 5000;
    const EQUITY_THROTTLE = 15000;
    let ws = null;
    let reconnTimer = null;
    let activePage = 'dashboard';
    let equityData = [];
    let allTrades = [];
    let equityChart = null;
    let revenueChart = null;
    let lastOrderbook = {};
    let obMarket = '1';
    let notifications = [];
    let ordersData = [];

    const $ = id => document.getElementById(id);
    const fmtPnl = v => (v >= 0 ? '+$' : '-$') + Math.abs(v).toFixed(2);
    const fmtPct = v => (v >= 0 ? '+' : '') + v.toFixed(2) + '%';
    const pnlArrow = v => v > 0.001 ? '▲ ' : v < -0.001 ? '▼ ' : '— ';
    const pnlClass = v => v >= 0 ? 'c-up' : 'c-down';

    // ── i18n ──
    const i18nStrings = {
        en: {
            dashboard: 'Dashboard', strategies: 'Strategies', portfolio: 'Portfolio',
            history: 'History', settings: 'Settings', equity: 'Equity',
            unrealizedPnl: 'Unrealized P&L', dailyPnl: 'Daily P&L', totalPnl: 'Total P&L',
            equityCurve: 'Equity Curve', weeklyPnl: 'Weekly P&L',
            positions: 'Positions', orders: 'Orders', trades: 'Trades', log: 'Log',
            orderBook: 'Order Book', riskMonitor: 'Risk Monitor',
            maxDrawdown: 'Max Drawdown', dailyLoss: 'Daily Loss',
            available: 'Available', peakEquity: 'Peak Equity', openOrders: 'Open Orders',
            dailyPnlHistory: 'Daily P&L History',
            tradingControls: 'Trading Controls', manageMarkets: 'Manage markets and trading state',
            activeMarkets: 'Active Markets', quickActions: 'Quick Actions',
            saveMarketConfig: 'Save Market Config', pauseTrading: 'Pause Trading',
            resumeTrading: 'Resume Trading', cancelAllOrders: 'Cancel All Orders',
            performance: 'Performance', winRate: 'Win Rate', totalTrades: 'Total Trades',
            maxLeverage: 'Max Leverage',
            investPerGrid: 'Investment per Grid ($)', priceDeviation: 'Price Deviation (%)',
            dcaSub: 'Dollar-Cost Averaging', trendSub: 'EMA Crossover + RSI',
            buyInterval: 'Buy Interval (hours)', amountPerBuy: 'Amount per Buy ($)',
            dipThreshold: 'Dip Threshold (%)', fastEma: 'Fast EMA Period',
            slowEma: 'Slow EMA Period', rsiPeriod: 'RSI Period',
            tradeHistory: 'Trade History', fullAuditTrail: 'Full audit trail of all executed trades',
            exportCsv: 'Export CSV', volume: 'Volume', avgDuration: 'Avg Duration',
            systemStatus: 'System Status', botStatus: 'Bot Status',
            apiConnection: 'API Connection', riskLimits: 'Risk Limits',
            theme: 'Theme', themeDesc: 'Switch between light and dark mode for comfortable viewing.',
            toggleTheme: 'Toggle Theme', market: 'Market', side: 'Side', size: 'Size',
            entry: 'Entry', mark: 'Mark', price: 'Price', qty: 'Qty', filled: 'Filled',
            status: 'Status', time: 'Time', asset: 'Asset', pnl: 'PNL',
            noPositions: 'No active positions', noOrders: 'No open orders',
            noTrades: 'No trades yet', noHistory: 'No trade history',
            searchPlaceholder: 'Search...', searchByAsset: 'Search by asset, side...',
            connecting: 'Connecting...', liveTrading: 'Live Trading', disconnected: 'Disconnected',
            connectionLost: '⚡ Connection lost. Reconnecting...',
            confirmCancel: 'Cancel ALL open orders? This cannot be undone.',
        },
        cn: {
            dashboard: '仪表盘', strategies: '策略', portfolio: '投资组合',
            history: '历史记录', settings: '设置', equity: '净值',
            unrealizedPnl: '未实现盈亏', dailyPnl: '当日盈亏', totalPnl: '总盈亏',
            equityCurve: '净值曲线', weeklyPnl: '周盈亏',
            positions: '持仓', orders: '订单', trades: '交易', log: '日志',
            orderBook: '订单簿', riskMonitor: '风控监控',
            maxDrawdown: '最大回撤', dailyLoss: '日内亏损',
            available: '可用余额', peakEquity: '峰值净值', openOrders: '挂单数',
            dailyPnlHistory: '每日盈亏历史',
            tradingControls: '交易控制', manageMarkets: '管理交易对和交易状态',
            activeMarkets: '激活市场', quickActions: '快速操作',
            saveMarketConfig: '保存市场配置', pauseTrading: '暂停交易',
            resumeTrading: '恢复交易', cancelAllOrders: '取消所有订单',
            performance: '业绩表现', winRate: '胜率', totalTrades: '总交易数',
            maxLeverage: '最大杠杆',
            investPerGrid: '每格投资 ($)', priceDeviation: '价格偏差 (%)',
            dcaSub: '定投策略', trendSub: 'EMA交叉 + RSI',
            buyInterval: '买入间隔 (小时)', amountPerBuy: '每次买入 ($)',
            dipThreshold: '下跌阈值 (%)', fastEma: '快速EMA周期',
            slowEma: '慢速EMA周期', rsiPeriod: 'RSI周期',
            tradeHistory: '交易历史', fullAuditTrail: '所有已执行交易的完整记录',
            exportCsv: '导出CSV', volume: '成交量', avgDuration: '平均持仓时间',
            systemStatus: '系统状态', botStatus: '机器人状态',
            apiConnection: 'API连接', riskLimits: '风险限制',
            theme: '主题', themeDesc: '切换亮色和暗色模式以获得舒适的浏览体验。',
            toggleTheme: '切换主题', market: '市场', side: '方向', size: '数量',
            entry: '开仓价', mark: '标记价', price: '价格', qty: '数量', filled: '已成交',
            status: '状态', time: '时间', asset: '资产', pnl: '盈亏',
            noPositions: '暂无持仓', noOrders: '暂无挂单',
            noTrades: '暂无交易', noHistory: '暂无交易历史',
            searchPlaceholder: '搜索...', searchByAsset: '按资产、方向搜索...',
            connecting: '连接中...', liveTrading: '实盘交易中', disconnected: '已断开',
            connectionLost: '⚡ 连接断开，正在重连...',
            confirmCancel: '取消所有挂单？此操作不可撤销。',
        }
    };

    let currentLang = localStorage.getItem('lighter-lang') || 'en';

    function t(key) { return (i18nStrings[currentLang] || i18nStrings.en)[key] || (i18nStrings.en)[key] || key; }

    function applyI18n() {
        // Update all elements with data-i18n attribute
        document.querySelectorAll('[data-i18n]').forEach(el => {
            const key = el.getAttribute('data-i18n');
            el.textContent = t(key);
        });
        // Update lang button
        const langLabel = $('lang-label');
        if (langLabel) langLabel.textContent = currentLang === 'en' ? 'EN' : '中';
        // Update placeholders
        const gs = $('global-search');
        if (gs) gs.placeholder = t('searchPlaceholder');
        const hs = $('h-search');
        if (hs) hs.placeholder = t('searchByAsset');
        // Update WS offline text
        const wsOff = $('ws-offline');
        if (wsOff) wsOff.textContent = t('connectionLost');
    }

    function toggleLang() {
        currentLang = currentLang === 'en' ? 'cn' : 'en';
        localStorage.setItem('lighter-lang', currentLang);
        applyI18n();
    }

    if ($('btn-lang')) $('btn-lang').addEventListener('click', toggleLang);

    // ── Theme ──
    function getTheme() {
        const stored = localStorage.getItem('lighter-theme');
        if (stored) return stored;
        return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
    }

    function applyTheme(theme) {
        document.documentElement.setAttribute('data-theme', theme);
        localStorage.setItem('lighter-theme', theme);
        const icon = $('theme-icon');
        if (icon) {
            icon.setAttribute('data-lucide', theme === 'dark' ? 'sun' : 'moon');
            lucide.createIcons({ attrs: { id: 'theme-icon' } });
        }
        if (equityChart) updateChartTheme();
    }

    function toggleTheme() {
        const cur = document.documentElement.getAttribute('data-theme') || 'light';
        applyTheme(cur === 'dark' ? 'light' : 'dark');
    }

    applyTheme(getTheme());

    if ($('btn-theme')) $('btn-theme').addEventListener('click', toggleTheme);
    if ($('btn-theme-settings')) $('btn-theme-settings').addEventListener('click', toggleTheme);

    function updateChartTheme() {
        const isDark = document.documentElement.getAttribute('data-theme') === 'dark';
        const gridColor = isDark ? '#1B254B' : '#E9EDF7';
        const tickColor = isDark ? '#56607B' : '#A3AED0';
        [equityChart, revenueChart].forEach(c => {
            if (!c) return;
            if (c.options.scales.y) {
                c.options.scales.y.grid.color = gridColor;
                c.options.scales.y.ticks.color = tickColor;
            }
            if (c.options.scales.x) {
                c.options.scales.x.ticks.color = tickColor;
                if (c.options.scales.x.grid) c.options.scales.x.grid.color = gridColor;
            }
            c.update('none');
        });
    }

    // ── Clock ──
    function updateClock() {
        const now = new Date();
        const el = $('clock');
        if (el) el.textContent = now.toLocaleTimeString('en-GB', { hour12: false });
    }
    setInterval(updateClock, 1000);
    updateClock();

    // ── Notifications ──
    function addNotification(type, message) {
        notifications.unshift({ type, message, time: new Date() });
        if (notifications.length > 50) notifications.pop();
        const dot = $('notif-dot');
        if (dot) dot.style.display = '';
        renderNotifications();
    }

    function renderNotifications() {
        const list = $('notif-list');
        if (!list) return;
        if (notifications.length === 0) {
            list.innerHTML = '<div class="notif-empty">No notifications yet</div>';
            return;
        }
        list.innerHTML = notifications.slice(0, 20).map(n => {
            const iconClass = n.type === 'trade' ? 'trade' : n.type === 'warn' ? 'warn' : n.type === 'error' ? 'err' : 'trade';
            const iconChar = n.type === 'trade' ? '💹' : n.type === 'warn' ? '⚠️' : n.type === 'error' ? '❌' : '📌';
            const ago = timeAgo(n.time);
            return `<div class="notif-item"><div class="notif-icon ${iconClass}">${iconChar}</div><div class="notif-text"><div class="notif-msg">${n.message}</div><div class="notif-time">${ago}</div></div></div>`;
        }).join('');
    }

    function timeAgo(d) {
        const s = Math.floor((Date.now() - d.getTime()) / 1000);
        if (s < 60) return 'just now';
        if (s < 3600) return Math.floor(s/60) + 'm ago';
        if (s < 86400) return Math.floor(s/3600) + 'h ago';
        return Math.floor(s/86400) + 'd ago';
    }

    if ($('btn-notif')) {
        $('btn-notif').addEventListener('click', e => {
            e.stopPropagation();
            const panel = $('notif-panel');
            panel.classList.toggle('show');
            if (panel.classList.contains('show')) {
                $('notif-dot').style.display = 'none';
            }
        });
    }
    if ($('notif-clear')) {
        $('notif-clear').addEventListener('click', () => {
            notifications = [];
            renderNotifications();
        });
    }
    document.addEventListener('click', () => {
        const p = $('notif-panel');
        if (p) p.classList.remove('show');
    });

    // ── Page Navigation ──
    document.querySelectorAll('.nav-item[data-page]').forEach(link => {
        link.addEventListener('click', function(e) {
            e.preventDefault();
            const page = this.getAttribute('data-page');
            if (page === activePage) return;
            document.querySelectorAll('.nav-item').forEach(l => l.classList.remove('active'));
            this.classList.add('active');
            document.querySelectorAll('.page').forEach(p => p.classList.remove('active'));
            const target = $('page-' + page);
            if (target) target.classList.add('active');
            $('current-page-title').innerText = this.innerText.trim();
            activePage = page;
            if (page === 'dashboard') setTimeout(initCharts, 100);
            if (page === 'history') { renderHistory(); renderPositionSummary(); }
        });
    });

    // ── Bottom Tabs ──
    const btmTabs = $('btm-tabs');
    if (btmTabs) {
        btmTabs.querySelectorAll('.tab-btn').forEach(btn => {
            btn.addEventListener('click', function() {
                btmTabs.querySelectorAll('.tab-btn').forEach(b => b.classList.remove('active'));
                this.classList.add('active');
                const t = this.getAttribute('data-t');
                ['positions','orders','trades','log'].forEach(id => {
                    const panel = $('tp-' + id);
                    if (panel) panel.classList.toggle('active', id === t);
                });
            });
        });
    }

    // ── Orderbook Tabs ──
    const obTabs = $('ob-tabs');
    if (obTabs) {
        obTabs.querySelectorAll('.ob-tab').forEach(btn => {
            btn.addEventListener('click', function() {
                obTabs.querySelectorAll('.ob-tab').forEach(b => b.classList.remove('active'));
                this.classList.add('active');
                obMarket = this.getAttribute('data-m');
                renderOrderbook();
            });
        });
    }

    // ── Search ──
    const globalSearch = $('global-search');
    if (globalSearch) {
        globalSearch.addEventListener('input', function() {
            const q = this.value.toLowerCase();
            if (activePage === 'history') {
                renderHistory(q);
            } else if (activePage === 'dashboard') {
                filterTable('pos-tbody', q);
                filterTable('ord-tbody', q);
                filterTable('trd-tbody', q);
            }
        });
    }

    function filterTable(tbodyId, query) {
        const tb = $(tbodyId);
        if (!tb) return;
        const rows = tb.querySelectorAll('tr');
        rows.forEach(r => {
            if (!query) { r.style.display = ''; return; }
            r.style.display = r.textContent.toLowerCase().includes(query) ? '' : 'none';
        });
    }

    // ── History Filter ──
    let historyAssetFilter = 'all';
    document.querySelectorAll('.fpill[data-asset]').forEach(btn => {
        btn.addEventListener('click', function() {
            document.querySelectorAll('.fpill[data-asset]').forEach(b => b.classList.remove('active'));
            this.classList.add('active');
            historyAssetFilter = this.getAttribute('data-asset');
            renderHistory();
        });
    });

    const hSearch = $('h-search');
    if (hSearch) {
        hSearch.addEventListener('input', () => renderHistory());
    }

    // ── WebSocket ──
    function connect() {
        const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
        ws = new WebSocket(`${proto}//${location.host}/ws`);
        ws.onopen = () => {
            $('ws-offline').style.display = 'none';
            const dot = $('status-dot');
            const label = $('status-label');
            if (dot) dot.classList.add('live');
            if (label) label.textContent = 'Live Trading';
            if ($('set-conn')) $('set-conn').textContent = 'Connected';
            if (reconnTimer) { clearTimeout(reconnTimer); reconnTimer = null; }
            addLog('i', 'WebSocket connected');
            loadInitialData();
        };
        ws.onmessage = e => {
            try { handleMessage(JSON.parse(e.data)); }
            catch(err) { console.error('WS parse error:', err); }
        };
        ws.onclose = () => {
            $('ws-offline').style.display = 'block';
            const dot = $('status-dot');
            const label = $('status-label');
            if (dot) dot.classList.remove('live');
            if (label) label.textContent = 'Disconnected';
            if ($('set-conn')) { $('set-conn').textContent = 'Disconnected'; $('set-conn').style.color = 'var(--danger)'; }
            addLog('w', 'WebSocket disconnected, reconnecting...');
            reconnTimer = setTimeout(connect, 5000);
        };
    }

    function handleMessage(msg) {
        switch (msg.type) {
            case 'status': updateMetrics(msg.data); break;
            case 'positions': updatePositions(msg.data); break;
            case 'recent_trades': updateTrades(msg.data); break;
            case 'open_orders': updateOrdersPanel(msg.data); break;
            case 'orderbook': updateOrderbookData(msg.data); break;
            case 'risk': updateRisk(msg.data); break;
        }
    }

    // ── Data Loading ──
    function loadInitialData() {
        fetch('/api/pnl').then(r => r.json()).then(data => {
            if (data.equity_history) {
                equityData = data.equity_history.map(p => ({ t: p.t * 1000, v: p.v }));
                updateEquityChart();
                const pnlMap = Object.assign({}, data.daily_pnl_map || {});
                const todayKey = new Date().toISOString().split('T')[0];
                if (data.daily_realized_pnl !== undefined) pnlMap[todayKey] = data.daily_realized_pnl;
                updateRevenueChart(pnlMap);
            }
            if (data.trades) {
                allTrades = data.trades;
                renderHistory();
                computeHistoryStats();
                renderPositionSummary();
            }
            if (data.total_realized_pnl !== undefined) {
                const el = $('mc-total');
                if (el) { el.textContent = fmtPnl(data.total_realized_pnl); el.className = 'value ' + pnlClass(data.total_realized_pnl); }
                if ($('sp-pnl')) $('sp-pnl').textContent = fmtPnl(data.total_realized_pnl);
            }
        }).catch(e => addLog('e', 'Failed to load PnL data'));

        fetch('/api/strategy').then(r => r.json()).then(data => {
            if (data.params) {
                if ($('cfg-gc')) $('cfg-gc').value = data.params.grid_count || 6;
                if ($('cfg-inv')) $('cfg-inv').value = data.params.investment_per_grid || 8;
                if ($('cfg-dev')) $('cfg-dev').value = data.params.price_deviation || 0.012;
            }
            if (data.strategy && $('strat-name')) $('strat-name').textContent = data.strategy;
            updateStrategyBadges(data.strategy || 'grid_trading');
        }).catch(e => addLog('e', 'Failed to load strategy config'));
    }

    // Update strategy card badges based on active strategy
    function updateStrategyBadges(active) {
        const gridBadge = document.querySelector('#strat-name')?.closest('.card')?.querySelector('.badge');
        const dcaBadge = $('dca-status-badge');
        const trendBadge = $('trend-status-badge');
        if (gridBadge) gridBadge.className = 'badge ' + (active === 'grid_trading' || active === 'grid' ? 'badge-up' : 'badge-warn');
        if (gridBadge) gridBadge.textContent = (active === 'grid_trading' || active === 'grid') ? '● Active' : '○ Inactive';
        if (dcaBadge) { dcaBadge.className = 'badge ' + (active === 'dca' ? 'badge-up' : 'badge-warn'); dcaBadge.textContent = active === 'dca' ? '● Active' : '○ Inactive'; }
        if (trendBadge) { trendBadge.className = 'badge ' + (active === 'trend_following' || active === 'trend' ? 'badge-up' : 'badge-warn'); trendBadge.textContent = (active === 'trend_following' || active === 'trend') ? '● Active' : '○ Inactive'; }
    }

    // ── Strategy Apply (Grid) ──
    const applyBtn = $('btn-apply');
    if (applyBtn) {
        applyBtn.addEventListener('click', function() {
            const body = { strategy: 'grid_trading', params: {
                grid_count: parseFloat($('cfg-gc').value),
                investment_per_grid: parseFloat($('cfg-inv').value),
                price_deviation: parseFloat($('cfg-dev').value)
            }};
            this.disabled = true; this.innerText = 'Applying...';
            const msgEl = $('cfg-msg');
            fetch('/api/strategy', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
                .then(r => r.json())
                .then(d => {
                    msgEl.innerText = '✓ Grid strategy activated';
                    msgEl.style.color = 'var(--success)';
                    updateStrategyBadges('grid_trading');
                    addNotification('trade', 'Grid strategy activated');
                    setTimeout(() => msgEl.innerText = '', 3000);
                })
                .catch(() => { msgEl.innerText = '✗ Failed to apply'; msgEl.style.color = 'var(--danger)'; })
                .finally(() => { this.disabled = false; this.innerText = 'Apply Changes'; });
        });
    }

    // ── DCA Strategy Activate ──
    const dcaBtn = $('btn-activate-dca');
    if (dcaBtn) {
        dcaBtn.addEventListener('click', function() {
            const body = { strategy: 'dca', params: {
                interval: parseFloat($('cfg-dca-interval').value),
                amount: parseFloat($('cfg-dca-amount').value),
                dip_threshold: parseFloat($('cfg-dca-dip').value)
            }};
            this.disabled = true; this.innerText = 'Activating...';
            const msgEl = $('dca-msg');
            fetch('/api/strategy', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
                .then(r => r.json())
                .then(d => {
                    msgEl.innerText = '✓ DCA strategy activated';
                    msgEl.style.color = 'var(--success)';
                    updateStrategyBadges('dca');
                    addNotification('trade', 'DCA strategy activated');
                    setTimeout(() => msgEl.innerText = '', 3000);
                })
                .catch(() => { msgEl.innerText = '✗ Failed'; msgEl.style.color = 'var(--danger)'; })
                .finally(() => { this.disabled = false; this.innerText = 'Activate DCA Strategy'; });
        });
    }

    // ── Trend Following Activate ──
    const trendBtn = $('btn-activate-trend');
    if (trendBtn) {
        trendBtn.addEventListener('click', function() {
            const body = { strategy: 'trend_following', params: {
                fast_ma: parseInt($('cfg-trend-fast').value),
                slow_ma: parseInt($('cfg-trend-slow').value),
                stop_loss: parseFloat($('cfg-trend-sl').value) / 100.0,
                take_profit: parseFloat($('cfg-trend-tp').value) / 100.0
            }};
            this.disabled = true; this.innerText = 'Activating...';
            const msgEl = $('trend-msg');
            fetch('/api/strategy', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
                .then(r => r.json())
                .then(d => {
                    msgEl.innerText = '✓ Trend strategy activated';
                    msgEl.style.color = 'var(--success)';
                    updateStrategyBadges('trend_following');
                    addNotification('trade', 'Trend following strategy activated');
                    setTimeout(() => msgEl.innerText = '', 3000);
                })
                .catch(() => { msgEl.innerText = '✗ Failed'; msgEl.style.color = 'var(--danger)'; })
                .finally(() => { this.disabled = false; this.innerText = 'Activate Trend Strategy'; });
        });
    }

    // ── Metrics Update ──
    let lastEquity = 0, lastAvail = 0, lastPeak = 0;

    function updateMetrics(d) {
        if (!d) return;
        lastEquity = d.equity || 0;
        lastAvail = d.available_balance || 0;
        lastPeak = d.peak_equity || lastPeak;

        setVal('mc-equity', '$' + lastEquity.toFixed(2));
        setVal('pf-equity', '$' + lastEquity.toFixed(2));
        setVal('pf-avail', '$' + lastAvail.toFixed(2));
        setVal('s-avail', '$' + lastAvail.toFixed(2));
        setVal('s-peak', '$' + lastPeak.toFixed(2));

        const daily = d.daily_realized_pnl || 0;
        setPnl('mc-daily', daily);

        const upnl = d.unrealized_pnl || 0;
        setPnl('mc-upnl', upnl);

        const total = d.total_realized_pnl || 0;
        setPnl('mc-total', total);
        if ($('sp-pnl')) { $('sp-pnl').textContent = fmtPnl(total); $('sp-pnl').className = 'info-v ' + pnlClass(total); }

        setVal('s-orders', d.open_orders || 0);
        setVal('s-orders-label', (d.open_orders || 0) + ' open orders');
        setVal('pf-ord-count', d.open_orders || 0);

        if (d.version) setVal('set-version', d.version);
        if (d.strategy) setVal('strat-name', d.strategy);
        if (d.account_index) setVal('set-account', d.account_index);

        // Equity chart update
        const now = Date.now();
        const lastPt = equityData[equityData.length - 1];
        if (!lastPt || now - lastPt.t > EQUITY_THROTTLE) {
            equityData.push({ t: now, v: lastEquity });
            if (equityData.length > MAX_EQUITY_PTS) equityData.shift();
            updateEquityChart();
        }

        // Chart info
        if ($('chart-info') && equityData.length > 1) {
            const first = equityData[0].v;
            const changePct = ((lastEquity - first) / first * 100);
            $('chart-info').textContent = fmtPct(changePct) + ' since start';
            $('chart-info').style.color = changePct >= 0 ? 'var(--success)' : 'var(--danger)';
        }
    }

    function setVal(id, val) { const el = $(id); if (el) el.textContent = val; }
    function setPnl(id, val) {
        const el = $(id);
        if (!el) return;
        el.innerHTML = `<span class="pnl-arrow">${pnlArrow(val)}</span>${fmtPnl(val)}`;
        el.className = 'value ' + pnlClass(val);
        // Update parent stat-icon background tint
        const card = el.closest('.stat-card');
        if (card) {
            const icon = card.querySelector('.stat-icon');
            if (icon) {
                icon.style.color = val > 0.001 ? 'var(--success)' : val < -0.001 ? 'var(--danger)' : '';
                icon.style.background = val > 0.001 ? 'var(--success-bg)' : val < -0.001 ? 'var(--danger-bg)' : '';
            }
        }
    }

    // ── Positions ──
    function updatePositions(data) {
        const tb = $('pos-tbody');
        const pftb = $('pf-pos-tbody');
        if (!tb) return;
        const cnt = data ? data.length : 0;
        setVal('pf-pos-count', cnt);
        if (!data || cnt === 0) {
            const empty = '<tr><td colspan="6" class="empty-cell">No active positions</td></tr>';
            tb.innerHTML = empty;
            if (pftb) pftb.innerHTML = empty;
            return;
        }
        const html = data.map(p => {
            const pnl = p.unrealized_pnl || 0;
            return `<tr><td>${p.symbol}</td><td><span class="badge ${p.side==='Buy'?'badge-up':'badge-down'}">${p.side}</span></td><td>${p.size}</td><td>$${parseFloat(p.entry_price).toFixed(2)}</td><td>$${(p.mark_price||0).toFixed(2)}</td><td class="td-r ${pnlClass(pnl)}">${fmtPnl(pnl)}</td></tr>`;
        }).join('');
        tb.innerHTML = html;
        if (pftb) pftb.innerHTML = html;
    }

    // ── Orders ──
    function updateOrdersPanel(data) {
        if (Array.isArray(data)) {
            ordersData = data;
            const cnt = data.length;
            setVal('s-orders', cnt);
            setVal('s-orders-label', cnt + ' open orders');
            setVal('pf-ord-count', cnt);
            const tb = $('ord-tbody');
            const pftb = $('pf-ord-tbody');
            if (!tb) return;
            if (cnt === 0) {
                const empty = '<tr><td colspan="7" class="empty-cell">No open orders</td></tr>';
                tb.innerHTML = empty;
                if (pftb) pftb.innerHTML = empty;
                return;
            }
            const html = data.map(o => {
                const fill = o.filled_quantity || 0;
                const total = o.quantity || 1;
                const fillPct = (fill / total * 100).toFixed(0);
                return `<tr><td style="font-family:monospace;font-size:11px;">${String(o.id).slice(-6)}</td><td>${o.symbol||'BTC'}</td><td><span class="badge ${o.side==='Buy'?'badge-up':'badge-down'}">${o.side}</span></td><td>$${parseFloat(o.price).toFixed(2)}</td><td>${total}</td><td>${fill} (${fillPct}%)</td><td><span class="badge badge-info">${o.status||'Open'}</span></td></tr>`;
            }).join('');
            tb.innerHTML = html;
            if (pftb) pftb.innerHTML = html;
        } else if (typeof data === 'number') {
            setVal('s-orders', data);
            setVal('s-orders-label', data + ' open orders');
        }
    }

    // ── Trades ──
    let prevTradeCount = 0;
    function updateTrades(data) {
        if (!data) return;
        // Detect new trades for notifications
        if (data.length > prevTradeCount && prevTradeCount > 0) {
            const newTrades = data.slice(0, data.length - prevTradeCount);
            newTrades.forEach(t => {
                const msg = `${t.side} ${t.symbol||t.market} @ $${parseFloat(t.price).toFixed(2)} × ${t.quantity}`;
                addNotification('trade', msg);
                addLog('t', 'Trade: ' + msg + (t.pnl ? ' PnL=' + fmtPnl(t.pnl) : ''));
            });
        }
        prevTradeCount = data.length;

        // Merge into allTrades (dedup by timestamp+price)
        data.forEach(t => {
            const exists = allTrades.some(a => a.timestamp === t.timestamp && a.price === t.price && a.side === t.side);
            if (!exists) allTrades.unshift(t);
        });

        const tb = $('trd-tbody');
        if (!tb) return;
        const rows = data.slice(0, 15);
        if (rows.length === 0) { tb.innerHTML = '<tr><td colspan="7" class="empty-cell">No trades yet</td></tr>'; return; }
        tb.innerHTML = rows.map(t => {
            const ts = new Date(t.timestamp).toLocaleTimeString();
            const pnl = t.pnl || 0;
            const action = t.action || t.close_type || t.trade_type || 'Order';
            const isClose = action.includes('Close') || action.includes('Stop') || action.includes('Emergency') || action.includes('Liquidat');
            const actionBadge = isClose
                ? `<span class="badge ${pnl >= 0 ? 'badge-up' : 'badge-down'}">${action}</span>`
                : `<span class="badge badge-neutral">${action}</span>`;
            const pnlCell = isClose
                ? `<td class="td-r ${pnlClass(pnl)}">${fmtPnl(pnl)}</td>`
                : `<td class="td-r" style="color:var(--text-muted)">—</td>`;
            return `<tr><td>${ts}</td><td>${t.symbol||t.market}</td><td>${actionBadge}</td><td><span class="badge ${t.side==='Buy'?'badge-up':'badge-down'}">${t.side}</span></td><td>$${parseFloat(t.price).toFixed(2)}</td><td>${parseFloat(t.quantity).toFixed(6)}</td>${pnlCell}</tr>`;
        }).join('');
    }

    // ── Orderbook ──
    function updateOrderbookData(data) {
        if (!data) return;
        lastOrderbook[data.market_id] = data;
        if (String(data.market_id) === obMarket) renderOrderbook();
    }

    function renderOrderbook() {
        const data = lastOrderbook[obMarket];
        const bidsEl = $('ob-bids');
        const asksEl = $('ob-asks');
        const spreadEl = $('ob-spread');
        if (!bidsEl || !asksEl) return;
        if (!data || !data.bids || !data.asks) {
            bidsEl.innerHTML = '<div style="padding:16px;text-align:center;color:var(--text-sub);font-size:12px;">Waiting...</div>';
            asksEl.innerHTML = '';
            if (spreadEl) spreadEl.textContent = '—';
            return;
        }
        const bids = data.bids.slice(0, 8);
        const asks = data.asks.slice(0, 8).reverse();
        const maxBidSize = Math.max(...bids.map(b => parseFloat(b.size) || 0), 0.001);
        const maxAskSize = Math.max(...asks.map(a => parseFloat(a.size) || 0), 0.001);

        asksEl.innerHTML = asks.map(a => {
            const depth = (parseFloat(a.size) / maxAskSize * 100).toFixed(1);
            return `<div class="ob-row ask" style="--depth:${depth}%"><span class="ob-price" style="color:var(--danger);">$${parseFloat(a.price).toFixed(2)}</span><span class="ob-size">${parseFloat(a.size).toFixed(4)}</span></div>`;
        }).join('');

        bidsEl.innerHTML = bids.map(b => {
            const depth = (parseFloat(b.size) / maxBidSize * 100).toFixed(1);
            return `<div class="ob-row bid" style="--depth:${depth}%"><span class="ob-price" style="color:var(--success);">$${parseFloat(b.price).toFixed(2)}</span><span class="ob-size">${parseFloat(b.size).toFixed(4)}</span></div>`;
        }).join('');

        if (spreadEl && bids.length && asks.length) {
            const bestAsk = parseFloat(asks[asks.length - 1].price);
            const bestBid = parseFloat(bids[0].price);
            const mid = ((bestAsk + bestBid) / 2).toFixed(2);
            const spreadPct = ((bestAsk - bestBid) / mid * 100).toFixed(3);
            spreadEl.innerHTML = `$${mid} <span style="font-size:11px;color:var(--text-sub);font-weight:400;">spread ${spreadPct}%</span>`;
        }
    }

    // ── Risk ──
    function updateRisk(data) {
        if (!data) return;
        const dd = data.drawdown_pct || 0;
        const dl = data.daily_loss_pct || 0;
        const ddLimit = data.max_drawdown_limit || 10;
        const dlLimit = data.daily_loss_limit || 5;

        setVal('r-dd', dd.toFixed(1) + '%');
        setVal('r-dl', dl.toFixed(1) + '%');

        const ddBar = $('r-dd-bar');
        const dlBar = $('r-dl-bar');
        if (ddBar) {
            ddBar.style.width = Math.min(dd / ddLimit * 100, 100) + '%';
            ddBar.className = 'risk-fill ' + (dd < ddLimit * 0.5 ? 'ok' : dd < ddLimit * 0.8 ? 'warn' : 'danger');
        }
        if (dlBar) {
            dlBar.style.width = Math.min(dl / dlLimit * 100, 100) + '%';
            dlBar.className = 'risk-fill ' + (dl < dlLimit * 0.5 ? 'ok' : dl < dlLimit * 0.8 ? 'warn' : 'danger');
        }
    }

    // ── History ──
    function renderHistory(searchOverride) {
        const tb = $('history-tbody');
        if (!tb) return;
        const search = (typeof searchOverride === 'string') ? searchOverride : ($('h-search') ? $('h-search').value.toLowerCase() : '');
        let filtered = allTrades;
        if (historyAssetFilter !== 'all') {
            filtered = filtered.filter(t => (t.symbol || t.market || '').toUpperCase().includes(historyAssetFilter));
        }
        if (search) {
            filtered = filtered.filter(t => {
                const txt = [t.symbol, t.market, t.side, t.price, t.quantity, t.action].join(' ').toLowerCase();
                return txt.includes(search);
            });
        }
        if (filtered.length === 0) {
            tb.innerHTML = '<tr><td colspan="7" class="empty-cell">No matching trades</td></tr>';
            return;
        }
        tb.innerHTML = filtered.slice(0, 100).map(t => {
            const ts = new Date(t.timestamp).toLocaleString();
            const pnl = t.pnl || 0;
            const action = t.action || t.close_type || t.trade_type || 'Order';
            const isClose = action.includes('Close') || action.includes('Stop') || action.includes('Emergency') || action.includes('Liquidat');
            const actionBadge = isClose
                ? `<span class="badge ${pnl >= 0 ? 'badge-up' : 'badge-down'}">${action}</span>`
                : `<span class="badge badge-neutral">${action}</span>`;
            const pnlCell = isClose
                ? `<td class="td-r ${pnlClass(pnl)}">${fmtPnl(pnl)}</td>`
                : `<td class="td-r" style="color:var(--text-muted)">—</td>`;
            return `<tr><td>${ts}</td><td>${t.symbol||t.market}</td><td>${actionBadge}</td><td><span class="badge ${t.side==='Buy'?'badge-up':'badge-down'}">${t.side}</span></td><td>$${parseFloat(t.price).toFixed(2)}</td><td>${parseFloat(t.quantity).toFixed(6)}</td>${pnlCell}</tr>`;
        }).join('');
    }

    function computeHistoryStats() {
        if (!allTrades.length) return;
        let totalPnl = 0, wins = 0, closeTrades = 0, vol = 0;
        allTrades.forEach(t => {
            const action = t.action || t.close_type || t.trade_type || '';
            const isClose = action.includes('Close') || action.includes('Stop') || action.includes('Emergency') || action.includes('Liquidat');
            vol += Math.abs(parseFloat(t.price) * parseFloat(t.quantity));
            if (isClose) {
                const p = t.pnl || 0;
                totalPnl += p;
                closeTrades++;
                if (p > 0) wins++;
            }
        });
        const winRate = closeTrades > 0 ? (wins / closeTrades * 100).toFixed(1) : '0.0';
        setPnl('hc-pnl', totalPnl);
        setVal('hc-winrate', winRate + '%');
        setVal('hc-volume', '$' + vol.toFixed(0));
        setVal('sp-winrate', winRate + '%');
        setVal('sp-trades', allTrades.length);
    }

    function renderPositionSummary() {
        const tb = $('pos-summary-tbody');
        if (!tb) return;
        const closeTrades = allTrades.filter(t => {
            const action = t.action || t.close_type || '';
            return action.includes('Close') || action.includes('Stop') || action.includes('Emergency');
        });
        if (closeTrades.length === 0) {
            tb.innerHTML = '<tr><td colspan="5" class="empty-cell">No closed positions yet</td></tr>';
            return;
        }
        const groups = {};
        closeTrades.forEach(t => {
            const asset = t.symbol || t.market || 'Unknown';
            if (!groups[asset]) groups[asset] = { wins: 0, losses: 0, totalPnl: 0, count: 0 };
            const pnl = t.pnl || 0;
            groups[asset].count++;
            groups[asset].totalPnl += pnl;
            if (pnl > 0) groups[asset].wins++;
            else groups[asset].losses++;
        });
        let totalPnl = 0, totalCount = 0, totalWins = 0;
        const rows = Object.entries(groups).map(([asset, g]) => {
            const wr = g.count > 0 ? (g.wins / g.count * 100).toFixed(1) : '0.0';
            totalPnl += g.totalPnl;
            totalCount += g.count;
            totalWins += g.wins;
            return `<tr><td><b>${asset}</b></td><td>${g.count}</td><td><span class="c-up">${g.wins}W</span> / <span class="c-down">${g.losses}L</span></td><td>${wr}%</td><td class="td-r ${pnlClass(g.totalPnl)}">${fmtPnl(g.totalPnl)}</td></tr>`;
        });
        const totalWr = totalCount > 0 ? (totalWins / totalCount * 100).toFixed(1) : '0.0';
        rows.push(`<tr style="border-top:2px solid var(--border);font-weight:600;"><td>Total</td><td>${totalCount}</td><td><span class="c-up">${totalWins}W</span> / <span class="c-down">${totalCount - totalWins}L</span></td><td>${totalWr}%</td><td class="td-r ${pnlClass(totalPnl)}">${fmtPnl(totalPnl)}</td></tr>`);
        tb.innerHTML = rows.join('');
    }

    // ── Export CSV ──
    if ($('btn-export')) {
        $('btn-export').addEventListener('click', () => {
            let csv = 'Time,Asset,Side,Price,Quantity,PNL\n';
            allTrades.forEach(t => {
                csv += `"${new Date(t.timestamp).toISOString()}","${t.symbol||t.market}","${t.side}",${t.price},${t.quantity},${t.pnl||0}\n`;
            });
            const blob = new Blob([csv], { type: 'text/csv' });
            const url = URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = url; a.download = 'lighter_trades_' + new Date().toISOString().slice(0,10) + '.csv';
            document.body.appendChild(a); a.click(); document.body.removeChild(a);
            URL.revokeObjectURL(url);
            addNotification('trade', 'Trade history exported to CSV');
        });
    }

    // ── Daily PnL History Bars ──
    function renderPnlHistory(pnlMap) {
        const el = $('pnl-history');
        if (!el) return;
        const entries = Object.entries(pnlMap).sort((a, b) => b[0].localeCompare(a[0])).slice(0, 14);
        if (entries.length === 0) { el.innerHTML = '<div class="notif-empty">No daily data yet</div>'; return; }
        const maxAbs = Math.max(...entries.map(e => Math.abs(e[1])), 0.01);
        el.innerHTML = entries.map(([date, val]) => {
            const pct = (Math.abs(val) / maxAbs * 48).toFixed(1);
            const isPos = val >= 0;
            const shortDate = date.slice(5);
            return `<div class="pnl-bar-row"><span class="pnl-bar-date">${shortDate}</span><div class="pnl-bar-track"><div class="pnl-bar-center"></div><div class="pnl-bar-fill ${isPos?'pos':'neg'}" style="width:${pct}%;${isPos?'':'right:auto;left:calc(50% - '+pct+'%);'}"></div></div><span class="pnl-bar-val ${pnlClass(val)}">${fmtPnl(val)}</span></div>`;
        }).join('');
    }

    // ── Log ──
    let logLines = [];
    function addLog(level, msg) {
        const ts = new Date().toLocaleTimeString('en-GB', { hour12: false });
        logLines.push({ ts, level, msg });
        if (logLines.length > 200) logLines.shift();
        const box = $('log-box');
        if (!box) return;
        box.innerHTML = logLines.slice(-60).map(l => `<div class="log-line"><span class="log-ts">[${l.ts}]</span> <span class="log-${l.level}">${l.msg}</span></div>`).join('');
        box.scrollTop = box.scrollHeight;
    }

    // ── Charts ──
    function initCharts() {
        const isDark = document.documentElement.getAttribute('data-theme') === 'dark';
        const gridColor = isDark ? '#1B254B' : '#E9EDF7';
        const tickColor = isDark ? '#56607B' : '#A3AED0';
        const primaryColor = isDark ? '#7551FF' : '#4318FF';

        const ctxEq = $('equityChart');
        if (!ctxEq) return;
        if (equityChart) equityChart.destroy();

        equityChart = new Chart(ctxEq.getContext('2d'), {
            type: 'line',
            data: {
                labels: equityData.map(d => { const dt = new Date(d.t); return dt.toLocaleDateString(undefined, {month:'short', day:'numeric'}) + ' ' + dt.toLocaleTimeString(undefined, {hour:'2-digit', minute:'2-digit'}); }),
                datasets: [{
                    label: 'Equity',
                    data: equityData.map(d => d.v),
                    borderColor: primaryColor,
                    borderWidth: 2.5,
                    fill: true,
                    backgroundColor: (ctx) => {
                        const chart = ctx.chart;
                        const { ctx: c, chartArea } = chart;
                        if (!chartArea) return null;
                        const g = c.createLinearGradient(0, chartArea.top, 0, chartArea.bottom);
                        g.addColorStop(0, isDark ? 'rgba(117,81,255,0.25)' : 'rgba(67,24,255,0.15)');
                        g.addColorStop(1, isDark ? 'rgba(117,81,255,0.01)' : 'rgba(67,24,255,0.01)');
                        return g;
                    },
                    tension: 0.4,
                    pointRadius: 0,
                    pointHoverRadius: 4,
                    pointHoverBackgroundColor: primaryColor,
                }]
            },
            options: {
                responsive: true, maintainAspectRatio: false,
                interaction: { mode: 'index', intersect: false },
                plugins: {
                    legend: { display: false },
                    tooltip: {
                        backgroundColor: isDark ? '#1B254B' : '#2B3674',
                        titleColor: '#fff', bodyColor: '#fff',
                        padding: 10, cornerRadius: 8,
                        displayColors: false,
                        callbacks: { label: ctx => '$' + ctx.parsed.y.toFixed(2) }
                    }
                },
                scales: {
                    x: { display: true, grid: { display: false }, ticks: { color: tickColor, font: { size: 10 }, maxTicksLimit: 6, maxRotation: 0 } },
                    y: { grid: { color: gridColor, drawBorder: false }, ticks: { color: tickColor, font: { weight: '500' } } }
                }
            }
        });

        const ctxRev = $('revenueChart');
        if (!ctxRev) return;
        if (revenueChart) revenueChart.destroy();
        revenueChart = new Chart(ctxRev.getContext('2d'), {
            type: 'bar',
            data: {
                labels: ['Mon','Tue','Wed','Thu','Fri','Sat','Sun'],
                datasets: [{ label: 'P&L', data: [0,0,0,0,0,0,0], backgroundColor: primaryColor, borderRadius: 6 }]
            },
            options: {
                responsive: true, maintainAspectRatio: false,
                plugins: {
                    legend: { display: false },
                    tooltip: {
                        backgroundColor: isDark ? '#1B254B' : '#2B3674',
                        titleColor: '#fff', bodyColor: '#fff',
                        padding: 10, cornerRadius: 8,
                        callbacks: { label: ctx => fmtPnl(ctx.parsed.y) }
                    }
                },
                scales: {
                    x: { grid: { display: false }, ticks: { color: tickColor } },
                    y: { grid: { color: gridColor, drawBorder: false }, ticks: { display: false } }
                }
            }
        });
    }

    function updateEquityChart() {
        if (!equityChart) return;
        equityChart.data.labels = equityData.map(d => { const dt = new Date(d.t); return dt.toLocaleDateString(undefined, {month:'short', day:'numeric'}) + ' ' + dt.toLocaleTimeString(undefined, {hour:'2-digit', minute:'2-digit'}); });
        equityChart.data.datasets[0].data = equityData.map(d => d.v);
        equityChart.update('none');
    }

    function updateRevenueChart(pnlMap) {
        if (!revenueChart) return;
        const values = new Array(7).fill(0);
        const today = new Date();
        const dayOfWeek = (today.getDay() + 6) % 7;
        for (let i = 0; i < 7; i++) {
            const d = new Date();
            d.setDate(today.getDate() - dayOfWeek + i);
            const key = d.toISOString().split('T')[0];
            values[i] = pnlMap[key] || 0;
        }
        revenueChart.data.datasets[0].data = values;
        revenueChart.data.datasets[0].backgroundColor = values.map(v => v >= 0 ? (document.documentElement.getAttribute('data-theme') === 'dark' ? '#7551FF' : '#4318FF') : 'var(--danger)');
        revenueChart.update();
        renderPnlHistory(pnlMap);
    }

    // ── Trading Controls ──
    let tradingPaused = false;
    let activeMarketsSet = new Set([1]); // default BTC
    const marketNames = { 0: 'ETH', 1: 'BTC' };

    function loadTradingControls() {
        fetch('/api/trading/markets').then(r => r.json()).then(data => {
            if (data.active_markets) {
                activeMarketsSet = new Set(data.active_markets);
                data.active_markets.forEach(mid => {
                    const cb = $('tc-m-' + mid);
                    if (cb) cb.checked = true;
                });
                // Uncheck inactive
                document.querySelectorAll('#tc-markets input[data-market]').forEach(cb => {
                    const mid = parseInt(cb.getAttribute('data-market'));
                    cb.checked = activeMarketsSet.has(mid);
                });
                updateMarketsDisplay();
            }
            if (data.trading_paused !== undefined) {
                tradingPaused = data.trading_paused;
                updatePauseButton();
            }
        }).catch(() => {});
    }

    function updateMarketsDisplay() {
        const names = [...activeMarketsSet].map(m => marketNames[m] || 'M' + m).join(', ');
        setVal('sp-markets', names || 'None');
    }

    function updatePauseButton() {
        const btn = $('btn-pause-trading');
        const txt = $('btn-pause-text');
        const badge = $('tc-status-badge');
        if (!btn) return;
        if (tradingPaused) {
            btn.classList.add('paused');
            txt.textContent = 'Resume Trading';
            if (badge) { badge.textContent = '⏸ Paused'; badge.className = 'badge badge-warn'; }
        } else {
            btn.classList.remove('paused');
            txt.textContent = 'Pause Trading';
            if (badge) { badge.textContent = '● Active'; badge.className = 'badge badge-up'; }
        }
    }

    // Save markets
    if ($('btn-save-markets')) {
        $('btn-save-markets').addEventListener('click', () => {
            const markets = [];
            document.querySelectorAll('#tc-markets input[data-market]:checked').forEach(cb => {
                markets.push(parseInt(cb.getAttribute('data-market')));
            });
            const msgEl = $('tc-market-msg');
            fetch('/api/trading/markets', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ markets })
            }).then(r => r.json()).then(data => {
                activeMarketsSet = new Set(markets);
                updateMarketsDisplay();
                msgEl.textContent = '✓ ' + data.message;
                msgEl.style.color = 'var(--success)';
                addNotification('trade', 'Markets updated: ' + markets.map(m => marketNames[m] || m).join(', '));
                addLog('i', 'Active markets changed: ' + JSON.stringify(markets));
                setTimeout(() => msgEl.textContent = '', 3000);
            }).catch(() => {
                msgEl.textContent = '✗ Failed to update markets';
                msgEl.style.color = 'var(--danger)';
            });
        });
    }

    // Pause/Resume
    if ($('btn-pause-trading')) {
        $('btn-pause-trading').addEventListener('click', () => {
            const endpoint = tradingPaused ? '/api/trading/resume' : '/api/trading/pause';
            const msgEl = $('tc-action-msg');
            fetch(endpoint, { method: 'POST' }).then(r => r.json()).then(data => {
                tradingPaused = !tradingPaused;
                updatePauseButton();
                msgEl.textContent = '✓ ' + data.message;
                msgEl.style.color = 'var(--success)';
                addNotification(tradingPaused ? 'warn' : 'trade', tradingPaused ? 'Trading paused' : 'Trading resumed');
                addLog(tradingPaused ? 'w' : 'i', tradingPaused ? 'Trading PAUSED' : 'Trading RESUMED');
                setTimeout(() => msgEl.textContent = '', 3000);
            }).catch(() => {
                msgEl.textContent = '✗ Failed';
                msgEl.style.color = 'var(--danger)';
            });
        });
    }

    // Cancel All
    if ($('btn-cancel-all')) {
        $('btn-cancel-all').addEventListener('click', () => {
            if (!confirm('Cancel ALL open orders? This cannot be undone.')) return;
            const msgEl = $('tc-action-msg');
            fetch('/api/trading/cancel-all', { method: 'POST' }).then(r => r.json()).then(data => {
                msgEl.textContent = '✓ ' + data.message;
                msgEl.style.color = 'var(--success)';
                addNotification('warn', 'All orders cancelled');
                addLog('w', 'Cancel all orders requested');
                setTimeout(() => msgEl.textContent = '', 3000);
            }).catch(() => {
                msgEl.textContent = '✗ Failed to cancel orders';
                msgEl.style.color = 'var(--danger)';
            });
        });
    }

    // Update paused state from WS status messages
    const origUpdateMetrics = updateMetrics;
    updateMetrics = function(d) {
        origUpdateMetrics(d);
        if (d && d.trading_paused !== undefined && d.trading_paused !== tradingPaused) {
            tradingPaused = d.trading_paused;
            updatePauseButton();
        }
        if (d && d.active_markets) {
            activeMarketsSet = new Set(d.active_markets);
            updateMarketsDisplay();
        }
    };

    // ── Risk Config ──
    function loadRiskConfig() {
        fetch('/api/risk/config').then(r => r.json()).then(data => {
            if (data.leverage_limit !== undefined) setRcInput('rc-leverage-limit', data.leverage_limit);
            if (data.max_leverage !== undefined) setRcInput('rc-max-leverage', data.max_leverage);
            if (data.position_stop_loss_pct !== undefined) setRcInput('rc-stop-loss', data.position_stop_loss_pct);
            if (data.position_take_profit_pct !== undefined) setRcInput('rc-take-profit', data.position_take_profit_pct);
            if (data.max_drawdown_pct !== undefined) setRcInput('rc-max-drawdown', data.max_drawdown_pct);
            if (data.daily_loss_limit_pct !== undefined) setRcInput('rc-daily-loss', data.daily_loss_limit_pct);
        }).catch(() => {});
    }

    function setRcInput(id, val) {
        const el = $(id);
        if (el) el.value = val;
    }

    // Save risk config
    if ($('btn-save-risk')) {
        $('btn-save-risk').addEventListener('click', () => {
            const body = {
                leverage_limit: parseFloat($('rc-leverage-limit').value) || 3,
                max_leverage: parseFloat($('rc-max-leverage').value) || 5,
                position_stop_loss_pct: parseFloat($('rc-stop-loss').value) || 3,
                position_take_profit_pct: parseFloat($('rc-take-profit').value) || 5,
                max_drawdown_pct: parseFloat($('rc-max-drawdown').value) || 10,
                daily_loss_limit_pct: parseFloat($('rc-daily-loss').value) || 5,
            };
            const msgEl = $('rc-save-msg');
            fetch('/api/risk/config', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body)
            }).then(r => r.json()).then(data => {
                if (data.status === 'ok') {
                    msgEl.textContent = '✓ Risk settings saved successfully';
                    msgEl.style.color = 'var(--success)';
                    addNotification('trade', 'Risk settings updated');
                    addLog('i', 'Risk config updated: leverage=' + body.leverage_limit + 'x, SL=' + body.position_stop_loss_pct + '%, TP=' + body.position_take_profit_pct + '%');
                } else {
                    msgEl.textContent = '✗ ' + (data.message || 'Failed');
                    msgEl.style.color = 'var(--danger)';
                }
                setTimeout(() => msgEl.textContent = '', 4000);
            }).catch(e => {
                msgEl.textContent = '✗ Network error';
                msgEl.style.color = 'var(--danger)';
            });
        });
    }

    // ── Init ──
    addLog('i', 'Dashboard initializing...');
    applyI18n();
    connect();
    setTimeout(initCharts, 500);
    setTimeout(loadTradingControls, 1000);
    setTimeout(loadRiskConfig, 1200);

})();
