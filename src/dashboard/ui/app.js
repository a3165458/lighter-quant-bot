// Lighter Bot Dashboard - Frontend
(function() {
    'use strict';

    let ws = null;
    let reconnectTimer = null;
    const MAX_LOG_ENTRIES = 200;

    function connect() {
        const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
        const wsUrl = protocol + '//' + location.host + '/ws';

        ws = new WebSocket(wsUrl);

        ws.onopen = function() {
            updateStatus('running');
            addLog('info', '已连接到交易机器人');
            clearInterval(reconnectTimer);
            // 请求初始数据
            send({ type: 'status' });
            send({ type: 'positions' });
            send({ type: 'recent_trades' });
        };

        ws.onmessage = function(event) {
            try {
                const data = JSON.parse(event.data);
                handleMessage(data);
            } catch (e) {
                addLog('error', '解析消息失败: ' + e.message);
            }
        };

        ws.onclose = function() {
            updateStatus('stopped');
            addLog('warn', '连接断开，5秒后重连...');
            reconnectTimer = setTimeout(connect, 5000);
        };

        ws.onerror = function() {
            addLog('error', '连接错误');
        };
    }

    function send(msg) {
        if (ws && ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify(msg));
        }
    }

    function handleMessage(data) {
        switch (data.type) {
            case 'welcome':
                addLog('info', data.message);
                break;
            case 'status':
                updateDashboardStatus(data.data);
                break;
            case 'positions':
                updatePositions(data.data);
                break;
            case 'recent_trades':
                updateTrades(data.data);
                break;
            case 'error':
                addLog('error', data.message);
                break;
            default:
                addLog('info', 'Received: ' + data.type);
        }
    }

    function updateStatus(status) {
        const badge = document.getElementById('status-badge');
        if (status === 'running') {
            badge.textContent = 'Connected';
            badge.className = 'status-badge status-running';
        } else {
            badge.textContent = 'Disconnected';
            badge.className = 'status-badge status-stopped';
        }
    }

    function updateDashboardStatus(data) {
        if (!data) return;
        setText('active-strategy', data.active_strategies && data.active_strategies.length > 0
            ? data.active_strategies.join(', ') : '-');
        setText('signal-count', data.total_trades || 0);
    }

    function updatePositions(positions) {
        const tbody = document.getElementById('positions-table');
        if (!positions || positions.length === 0) {
            tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;color:#787b86;">暂无持仓</td></tr>';
            return;
        }
        tbody.innerHTML = positions.map(function(p) {
            const pnlClass = p.unrealized_pnl >= 0 ? 'positive' : 'negative';
            return '<tr>' +
                '<td>' + p.symbol + '</td>' +
                '<td>' + p.side + '</td>' +
                '<td>' + p.size + '</td>' +
                '<td>$' + p.entry_price.toFixed(2) + '</td>' +
                '<td>-</td>' +
                '<td class="' + pnlClass + '">$' + p.unrealized_pnl.toFixed(2) + '</td>' +
                '</tr>';
        }).join('');
    }

    function updateTrades(trades) {
        const tbody = document.getElementById('trades-table');
        if (!trades || trades.length === 0) {
            tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;color:#787b86;">暂无交易</td></tr>';
            return;
        }
        tbody.innerHTML = trades.slice(-20).reverse().map(function(t) {
            const pnlClass = (t.pnl || 0) >= 0 ? 'positive' : 'negative';
            return '<tr>' +
                '<td>' + new Date(t.timestamp).toLocaleString() + '</td>' +
                '<td>' + t.symbol + '</td>' +
                '<td>' + t.side + '</td>' +
                '<td>$' + t.price.toFixed(2) + '</td>' +
                '<td>' + t.quantity.toFixed(6) + '</td>' +
                '<td class="' + pnlClass + '">$' + (t.pnl || 0).toFixed(2) + '</td>' +
                '</tr>';
        }).join('');
    }

    function addLog(level, message) {
        const container = document.getElementById('log-container');
        const entry = document.createElement('div');
        entry.className = 'log-entry log-' + level;
        const time = new Date().toLocaleTimeString();
        entry.textContent = '[' + time + '] [' + level.toUpperCase() + '] ' + message;
        container.appendChild(entry);

        // 限制日志数量
        while (container.children.length > MAX_LOG_ENTRIES) {
            container.removeChild(container.firstChild);
        }

        container.scrollTop = container.scrollHeight;
    }

    function setText(id, value) {
        const el = document.getElementById(id);
        if (el) el.textContent = value;
    }

    // 定期刷新数据
    setInterval(function() {
        send({ type: 'status' });
        send({ type: 'positions' });
    }, 5000);

    // 启动连接
    connect();
})();
