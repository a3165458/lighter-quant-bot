// AI Strategy Lab - Frontend Logic
(function() {
    'use strict';

    var lastBacktestResult = null;

    // Show/hide custom URL field
    document.getElementById('ai-provider').addEventListener('change', function() {
        document.getElementById('ai-custom-url-group').style.display =
            this.value === 'custom' ? 'block' : 'none';
    });

    // Run backtest via server API
    window.runBacktest = function() {
        var btn = document.getElementById('btn-run-backtest');
        btn.disabled = true;
        btn.innerHTML = '<span class="spinner"></span>Running...';

        var payload = {
            strategy: document.getElementById('bt-strategy').value,
            data_file: document.getElementById('bt-data').value,
            start: document.getElementById('bt-start').value,
            end: document.getElementById('bt-end').value,
            capital: parseFloat(document.getElementById('bt-capital').value),
            params: document.getElementById('bt-params').value || ''
        };

        fetch('/api/backtest', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify(payload)
        })
        .then(function(r) { return r.json(); })
        .then(function(data) {
            btn.disabled = false;
            btn.textContent = '▶ Run Backtest';
            if (data.error) {
                showError(data.error);
                return;
            }
            lastBacktestResult = data;
            renderResults(data);
        })
        .catch(function(e) {
            btn.disabled = false;
            btn.textContent = '▶ Run Backtest';
            showError('Request failed: ' + e.message);
        });
    };

    function showError(msg) {
        document.getElementById('results-empty').style.display = 'none';
        var content = document.getElementById('results-content');
        content.style.display = 'block';
        content.innerHTML = '<div class="result-card"><p class="negative" style="padding:12px">' +
            '❌ ' + msg + '</p></div>';
    }

    function renderResults(data) {
        document.getElementById('results-empty').style.display = 'none';
        var content = document.getElementById('results-content');
        content.style.display = 'block';

        var totalReturn = data.total_return_pct || 0;
        var badgeClass = totalReturn >= 0 ? 'badge-profit' : 'badge-loss';
        var badgeText = totalReturn >= 0 ? 'PROFIT' : 'LOSS';

        var html = '<div class="result-card">' +
            '<div class="result-header">' +
            '<div class="result-title">Backtest: ' + (data.strategy || 'grid') + ' on ' + (data.data_file || '-') + '</div>' +
            '<span class="result-badge ' + badgeClass + '">' + badgeText + '</span>' +
            '</div>' +
            '<div class="metrics-grid">' +
            metric('Total Return', fmtPct(totalReturn), totalReturn >= 0) +
            metric('Sharpe Ratio', (data.sharpe_ratio || 0).toFixed(2), data.sharpe_ratio >= 1) +
            metric('Max Drawdown', fmtPct(data.max_drawdown_pct || 0), false) +
            metric('Win Rate', fmtPct(data.win_rate_pct || 0), data.win_rate_pct >= 50) +
            '</div>' +
            '<div class="metrics-grid">' +
            metric('Total Trades', data.total_trades || 0, true) +
            metric('Final Equity', '$' + (data.final_capital || 0).toFixed(2), true) +
            metric('Avg Win', '$' + (data.avg_profit || 0).toFixed(2), true) +
            metric('Avg Loss', '$' + (data.avg_loss || 0).toFixed(2), false) +
            '</div>';

        // Equity chart
        var eqCurve = (data.equity_curve || []).map(function(p) { return p.v || p; });
        if (eqCurve.length > 1) {
            html += '<div class="chart-container"><canvas id="bt-chart" class="chart-canvas"></canvas></div>';
        }

        // Trade log
        if (data.trades && data.trades.length > 0) {
            html += '<div style="margin-top:12px"><table><thead><tr>' +
                '<th>#</th><th>Time</th><th>Side</th><th>Price</th><th>Size</th><th>PnL</th>' +
                '</tr></thead><tbody>';
            var trades = data.trades.slice(-30);
            for (var i = 0; i < trades.length; i++) {
                var t = trades[i];
                var pnlCls = (t.pnl || 0) >= 0 ? 'positive' : 'negative';
                html += '<tr><td>' + (i + 1) + '</td>' +
                    '<td>' + (t.timestamp || '-') + '</td>' +
                    '<td>' + (t.side || '-') + '</td>' +
                    '<td>$' + Number(t.price || 0).toFixed(2) + '</td>' +
                    '<td>' + Number(t.quantity || 0).toFixed(6) + '</td>' +
                    '<td class="' + pnlCls + '">$' + Number(t.pnl || 0).toFixed(2) + '</td></tr>';
            }
            html += '</tbody></table></div>';
        }

        html += '</div>';
        content.innerHTML = html;

        // Draw equity chart
        if (eqCurve.length > 1) {
            setTimeout(function() { drawChart('bt-chart', eqCurve); }, 50);
        }
    }

    function metric(label, value, isGood) {
        var cls = isGood ? 'positive' : 'negative';
        return '<div class="metric"><div class="metric-value ' + cls + '">' + value + '</div>' +
            '<div class="metric-label">' + label + '</div></div>';
    }

    function fmtPct(v) { return (v >= 0 ? '+' : '') + v.toFixed(2) + '%'; }

    function drawChart(canvasId, equityCurve) {
        var canvas = document.getElementById(canvasId);
        if (!canvas || equityCurve.length < 2) return;
        var ctx = canvas.getContext('2d');
        var dpr = window.devicePixelRatio || 1;
        var rect = canvas.getBoundingClientRect();
        canvas.width = rect.width * dpr;
        canvas.height = rect.height * dpr;
        ctx.scale(dpr, dpr);
        var w = rect.width, h = rect.height;
        ctx.clearRect(0, 0, w, h);

        var vals = equityCurve;
        var minV = Math.min.apply(null, vals) * 0.998;
        var maxV = Math.max.apply(null, vals) * 1.002;
        var range = maxV - minV || 1;

        // Grid
        var isDark = document.documentElement.getAttribute('data-theme') === 'dark';
        ctx.strokeStyle = isDark ? '#1B254B' : '#E9EDF7';
        ctx.lineWidth = 1;
        for (var g = 0; g < 4; g++) {
            var gy = h * 0.05 + (h * 0.9 / 3) * g;
            ctx.beginPath(); ctx.moveTo(0, gy); ctx.lineTo(w, gy); ctx.stroke();
        }

        var isProfit = vals[vals.length - 1] >= vals[0];
        var lineColor = isProfit ? '#26a65b' : '#ea3943';
        var fillColor = isProfit ? 'rgba(38,166,91,0.1)' : 'rgba(234,57,67,0.1)';

        ctx.beginPath();
        ctx.strokeStyle = lineColor;
        ctx.lineWidth = 2;
        for (var i = 0; i < vals.length; i++) {
            var x = (i / (vals.length - 1)) * w;
            var y = h * 0.05 + (1 - (vals[i] - minV) / range) * h * 0.9;
            if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
        }
        ctx.stroke();
        ctx.lineTo(w, h); ctx.lineTo(0, h); ctx.closePath();
        ctx.fillStyle = fillColor;
        ctx.fill();

        // Labels
        ctx.fillStyle = isDark ? '#56607B' : '#A3AED0';
        ctx.font = '10px sans-serif';
        ctx.textAlign = 'right';
        ctx.fillText('$' + maxV.toFixed(0), w - 4, h * 0.05 + 12);
        ctx.fillText('$' + minV.toFixed(0), w - 4, h * 0.95);
    }

    // AI Optimization
    window.aiOptimize = function() {
        var apiKey = document.getElementById('ai-key').value;
        if (!apiKey) {
            alert('Please enter an AI API key');
            return;
        }

        var btn = document.getElementById('btn-ai-optimize');
        var log = document.getElementById('ai-log');
        btn.disabled = true;
        btn.innerHTML = '<span class="spinner"></span>Analyzing...';
        log.style.display = 'block';
        log.textContent = '';

        function addAiLog(msg) {
            log.textContent += '> ' + msg + '\n';
            log.scrollTop = log.scrollHeight;
        }

        addAiLog('Starting AI optimization...');

        // Step 1: run baseline backtest
        var baseParams = document.getElementById('bt-params').value || 'grid_count=10,investment=8,deviation=0.012';
        addAiLog('Running baseline backtest with: ' + baseParams);

        var payload = {
            strategy: document.getElementById('bt-strategy').value,
            data_file: document.getElementById('bt-data').value,
            start_date: document.getElementById('bt-start').value,
            end_date: document.getElementById('bt-end').value,
            capital: parseFloat(document.getElementById('bt-capital').value),
            params: baseParams
        };

        fetch('/api/backtest', {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            body: JSON.stringify(payload)
        })
        .then(function(r) { return r.json(); })
        .then(function(baseResult) {
            addAiLog('Baseline: Return=' + (baseResult.total_return_pct || 0).toFixed(2) + '%, Sharpe=' + (baseResult.sharpe_ratio || 0).toFixed(2));
            addAiLog('Consulting AI for parameter suggestions...');

            // Step 2: Call AI API
            var provider = document.getElementById('ai-provider').value;
            var goal = document.getElementById('ai-goal').value;
            var goalText = {sharpe: 'Sharpe Ratio', 'return': 'Total Return', drawdown: 'Min Max Drawdown', balanced: 'Balanced Risk/Return'}[goal];

            var prompt = buildAIPrompt(baseResult, baseParams, goalText);
            callAI(provider, apiKey, prompt)
            .then(function(suggestion) {
                addAiLog('AI response received');
                addAiLog(suggestion.substring(0, 200) + '...');

                // Parse suggested params from AI response
                var suggestedParams = parseSuggestedParams(suggestion);
                if (suggestedParams) {
                    addAiLog('Suggested params: ' + suggestedParams);
                    document.getElementById('bt-params').value = suggestedParams;
                    addAiLog('Running backtest with AI-suggested params...');

                    payload.params = suggestedParams;
                    return fetch('/api/backtest', {
                        method: 'POST',
                        headers: {'Content-Type': 'application/json'},
                        body: JSON.stringify(payload)
                    }).then(function(r) { return r.json(); });
                } else {
                    addAiLog('Could not parse params from AI response');
                    return null;
                }
            })
            .then(function(newResult) {
                btn.disabled = false;
                btn.textContent = '🤖 AI Optimize Parameters';
                if (newResult) {
                    addAiLog('AI Optimized: Return=' + (newResult.total_return_pct || 0).toFixed(2) + '%, Sharpe=' + (newResult.sharpe_ratio || 0).toFixed(2));
                    var improve = (newResult.total_return_pct || 0) - (baseResult.total_return_pct || 0);
                    addAiLog('Improvement: ' + (improve >= 0 ? '+' : '') + improve.toFixed(2) + '%');
                    lastBacktestResult = newResult;
                    renderResults(newResult);
                }
            })
            .catch(function(e) {
                btn.disabled = false;
                btn.textContent = '🤖 AI Optimize Parameters';
                addAiLog('Error: ' + e.message);
            });
        })
        .catch(function(e) {
            btn.disabled = false;
            btn.textContent = '🤖 AI Optimize Parameters';
            addAiLog('Backtest error: ' + e.message);
        });
    };

    function buildAIPrompt(result, params, goalText) {
        return 'You are a quantitative trading strategy optimizer. I run a grid trading strategy on crypto (BTC/ETH perpetuals).\n\n' +
            'Current parameters: ' + params + '\n' +
            'Backtest results:\n' +
            '- Total return: ' + (result.total_return_pct || 0).toFixed(2) + '%\n' +
            '- Sharpe ratio: ' + (result.sharpe_ratio || 0).toFixed(2) + '\n' +
            '- Max drawdown: ' + (result.max_drawdown_pct || 0).toFixed(2) + '%\n' +
            '- Win rate: ' + (result.win_rate_pct || 0).toFixed(1) + '%\n' +
            '- Total trades: ' + (result.total_trades || 0) + '\n\n' +
            'Optimization goal: ' + goalText + '\n\n' +
            'Available parameters:\n' +
            '- grid_count (integer 4-20): number of grid levels each side\n' +
            '- investment (float 3-30): USD per grid level\n' +
            '- deviation (float 0.005-0.03): price deviation between levels\n\n' +
            'Please suggest improved parameters. Respond with EXACTLY one line in this format:\n' +
            'PARAMS: grid_count=X,investment=Y,deviation=Z\n' +
            'Then explain your reasoning briefly.';
    }

    function callAI(provider, apiKey, prompt) {
        var url, body, headers;

        if (provider === 'openai') {
            url = 'https://api.openai.com/v1/chat/completions';
            body = {model: 'gpt-4', messages: [{role: 'user', content: prompt}], max_tokens: 500};
            headers = {'Authorization': 'Bearer ' + apiKey, 'Content-Type': 'application/json'};
        } else if (provider === 'zhipu') {
            url = 'https://open.bigmodel.cn/api/paas/v4/chat/completions';
            body = {model: 'glm-4', messages: [{role: 'user', content: prompt}], max_tokens: 500};
            headers = {'Authorization': 'Bearer ' + apiKey, 'Content-Type': 'application/json'};
        } else {
            url = document.getElementById('ai-url').value;
            body = {model: 'default', messages: [{role: 'user', content: prompt}], max_tokens: 500};
            headers = {'Authorization': 'Bearer ' + apiKey, 'Content-Type': 'application/json'};
        }

        return fetch(url, {method: 'POST', headers: headers, body: JSON.stringify(body)})
            .then(function(r) { return r.json(); })
            .then(function(d) {
                if (d.choices && d.choices[0] && d.choices[0].message) {
                    return d.choices[0].message.content;
                }
                throw new Error('Unexpected AI response format: ' + JSON.stringify(d).substring(0, 200));
            });
    }

    function parseSuggestedParams(text) {
        // Look for PARAMS: grid_count=X,investment=Y,deviation=Z
        var match = text.match(/PARAMS:\s*([\w=.,]+)/i);
        if (match) return match[1].trim();
        // Fallback: try to find key=value patterns
        var gc = text.match(/grid_count\s*=\s*(\d+)/);
        var inv = text.match(/investment\s*=\s*([\d.]+)/);
        var dev = text.match(/deviation\s*=\s*([\d.]+)/);
        if (gc && inv && dev) {
            return 'grid_count=' + gc[1] + ',investment=' + inv[1] + ',deviation=' + dev[1];
        }
        return null;
    }

})();
