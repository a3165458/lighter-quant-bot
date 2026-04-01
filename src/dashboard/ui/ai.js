// AI Strategy Lab - Frontend Logic
(function() {
    'use strict';

    var lastBacktestResult = null;

    // ── Provider presets: url + model defaults ──
    var PRESETS = {
        openai:   { url: 'https://api.openai.com/v1/chat/completions',          model: 'gpt-4o' },
        zhipu:    { url: 'https://open.bigmodel.cn/api/paas/v4/chat/completions', model: 'glm-4-plus' },
        deepseek: { url: 'https://api.deepseek.com/v1/chat/completions',        model: 'deepseek-chat' },
        claude:   { url: 'https://api.anthropic.com/v1/messages',               model: 'claude-sonnet-4-20250514' },
        groq:     { url: 'https://api.groq.com/openai/v1/chat/completions',     model: 'llama-3.3-70b-versatile' },
        ollama:   { url: 'http://localhost:11434/v1/chat/completions',           model: 'llama3' },
        custom:   { url: '',                                                     model: '' }
    };

    // ── LocalStorage persistence ──
    var STORAGE_KEY = 'lighter-ai-settings';

    function saveSettings() {
        var settings = {
            provider: document.getElementById('ai-provider').value,
            url: document.getElementById('ai-url').value,
            model: document.getElementById('ai-model').value,
            key: document.getElementById('ai-key').value,
            goal: document.getElementById('ai-goal').value,
            maxTokens: document.getElementById('ai-max-tokens').value
        };
        try { localStorage.setItem(STORAGE_KEY, JSON.stringify(settings)); } catch(e) {}
    }

    function loadSettings() {
        try {
            var raw = localStorage.getItem(STORAGE_KEY);
            if (!raw) return;
            var s = JSON.parse(raw);
            if (s.provider) document.getElementById('ai-provider').value = s.provider;
            if (s.url) document.getElementById('ai-url').value = s.url;
            if (s.model) document.getElementById('ai-model').value = s.model;
            if (s.key) document.getElementById('ai-key').value = s.key;
            if (s.goal) document.getElementById('ai-goal').value = s.goal;
            if (s.maxTokens) document.getElementById('ai-max-tokens').value = s.maxTokens;
        } catch(e) {}
    }

    // Load saved settings on startup
    loadSettings();

    // If URL/model are empty (first visit), apply preset
    if (!document.getElementById('ai-url').value) {
        var p = PRESETS[document.getElementById('ai-provider').value] || PRESETS.openai;
        document.getElementById('ai-url').value = p.url;
        document.getElementById('ai-model').value = p.model;
    }

    // Auto-save on any input change
    ['ai-provider','ai-url','ai-model','ai-key','ai-goal','ai-max-tokens'].forEach(function(id) {
        var el = document.getElementById(id);
        if (el) el.addEventListener('change', saveSettings);
        if (el) el.addEventListener('input', saveSettings);
    });

    // Provider dropdown → fill URL + model from preset
    document.getElementById('ai-provider').addEventListener('change', function() {
        var preset = PRESETS[this.value];
        if (preset) {
            document.getElementById('ai-url').value = preset.url;
            document.getElementById('ai-model').value = preset.model;
            saveSettings();
        }
    });

    // Toggle API key visibility
    document.getElementById('toggle-key-vis').addEventListener('click', function() {
        var inp = document.getElementById('ai-key');
        var isPassword = inp.type === 'password';
        inp.type = isPassword ? 'text' : 'password';
        var icon = this.querySelector('[data-lucide]');
        if (icon) {
            icon.setAttribute('data-lucide', isPassword ? 'eye-off' : 'eye');
            lucide.createIcons();
        }
    });

    // ── Test AI connection ──
    window.aiTestConnection = function() {
        var btn = document.getElementById('btn-ai-test');
        var url = document.getElementById('ai-url').value;
        var model = document.getElementById('ai-model').value;
        var apiKey = document.getElementById('ai-key').value;

        if (!url) { alert('Please enter an API Base URL'); return; }
        if (!model) { alert('Please enter a Model ID'); return; }

        btn.disabled = true;
        btn.textContent = '⏳...';

        var provider = document.getElementById('ai-provider').value;
        var isAnthropic = provider === 'claude' || url.includes('anthropic.com');

        var headers, body;
        if (isAnthropic) {
            headers = {
                'x-api-key': apiKey,
                'anthropic-version': '2023-06-01',
                'Content-Type': 'application/json',
                'anthropic-dangerous-direct-browser-access': 'true'
            };
            body = { model: model, max_tokens: 20, messages: [{role:'user',content:'Hi, respond with just "OK"'}] };
        } else {
            headers = { 'Authorization': 'Bearer ' + apiKey, 'Content-Type': 'application/json' };
            body = { model: model, messages: [{role:'user',content:'Hi, respond with just "OK"'}], max_tokens: 20 };
        }

        fetch(url, { method: 'POST', headers: headers, body: JSON.stringify(body) })
        .then(function(r) {
            if (!r.ok) return r.text().then(function(t) { throw new Error('HTTP ' + r.status + ': ' + t.substring(0, 200)); });
            return r.json();
        })
        .then(function(d) {
            btn.disabled = false;
            btn.textContent = '🔗 Test';
            var reply = '';
            if (isAnthropic && d.content && d.content[0]) {
                reply = d.content[0].text || '';
            } else if (d.choices && d.choices[0]) {
                reply = (d.choices[0].message || {}).content || '';
            }
            alert('✅ Connection OK!\nModel: ' + model + '\nResponse: ' + reply.substring(0, 100));
        })
        .catch(function(e) {
            btn.disabled = false;
            btn.textContent = '🔗 Test';
            alert('❌ Connection failed:\n' + e.message);
        });
    };

    // ── Run backtest via server API ──
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
            if (data.error) { showError(data.error); return; }
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
        content.innerHTML = '<div class="result-card"><p class="negative" style="padding:12px">❌ ' + msg + '</p></div>';
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
            '<span class="result-badge ' + badgeClass + '">' + badgeText + '</span></div>' +
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

        var eqCurve = (data.equity_curve || []).map(function(p) { return p.v || p; });
        if (eqCurve.length > 1) {
            html += '<div class="chart-container"><canvas id="bt-chart" class="chart-canvas"></canvas></div>';
        }

        if (data.trades && data.trades.length > 0) {
            html += '<div style="margin-top:12px"><table><thead><tr>' +
                '<th>#</th><th>Time</th><th>Side</th><th>Price</th><th>Size</th><th>PnL</th></tr></thead><tbody>';
            var trades = data.trades.slice(-30);
            for (var i = 0; i < trades.length; i++) {
                var t = trades[i];
                var pnlCls = (t.pnl || 0) >= 0 ? 'positive' : 'negative';
                html += '<tr><td>' + (i+1) + '</td>' +
                    '<td>' + (t.timestamp || '-') + '</td>' +
                    '<td>' + (t.side || '-') + '</td>' +
                    '<td>$' + Number(t.price||0).toFixed(2) + '</td>' +
                    '<td>' + Number(t.quantity||0).toFixed(6) + '</td>' +
                    '<td class="' + pnlCls + '">$' + Number(t.pnl||0).toFixed(2) + '</td></tr>';
            }
            html += '</tbody></table></div>';
        }

        html += '</div>';
        content.innerHTML = html;

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

        ctx.fillStyle = isDark ? '#56607B' : '#A3AED0';
        ctx.font = '10px sans-serif';
        ctx.textAlign = 'right';
        ctx.fillText('$' + maxV.toFixed(0), w - 4, h * 0.05 + 12);
        ctx.fillText('$' + minV.toFixed(0), w - 4, h * 0.95);
    }

    // ── AI Optimization ──
    window.aiOptimize = function() {
        var apiKey = document.getElementById('ai-key').value;
        var apiUrl = document.getElementById('ai-url').value;
        var modelId = document.getElementById('ai-model').value;
        var maxTokens = parseInt(document.getElementById('ai-max-tokens').value) || 800;

        if (!apiUrl) { alert('Please enter an API Base URL'); return; }
        if (!modelId) { alert('Please enter a Model ID'); return; }
        if (!apiKey && document.getElementById('ai-provider').value !== 'ollama') {
            alert('Please enter an API Key'); return;
        }

        var btn = document.getElementById('btn-ai-optimize');
        var log = document.getElementById('ai-log');
        btn.disabled = true;
        btn.innerHTML = '<span class="spinner"></span>Analyzing...';
        log.style.display = 'block';
        log.textContent = '';

        function addLog(msg) {
            log.textContent += '> ' + msg + '\n';
            log.scrollTop = log.scrollHeight;
        }

        var provider = document.getElementById('ai-provider').value;
        var isAnthropic = provider === 'claude' || apiUrl.includes('anthropic.com');

        addLog('Provider: ' + provider + ' | Model: ' + modelId);
        addLog('Starting AI optimization...');

        var baseParams = document.getElementById('bt-params').value || 'grid_count=10,investment=8,deviation=0.012';
        addLog('Running baseline backtest: ' + baseParams);

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
            addLog('Baseline: Return=' + (baseResult.total_return_pct||0).toFixed(2) + '%, Sharpe=' + (baseResult.sharpe_ratio||0).toFixed(2));
            addLog('Consulting AI (' + modelId + ') for suggestions...');

            var goal = document.getElementById('ai-goal').value;
            var goalText = {sharpe:'Sharpe Ratio','return':'Total Return',drawdown:'Min Max Drawdown',balanced:'Balanced Risk/Return'}[goal];
            var prompt = buildAIPrompt(baseResult, baseParams, goalText);

            return callAI(apiUrl, modelId, apiKey, prompt, maxTokens, isAnthropic)
            .then(function(suggestion) {
                addLog('AI response received');
                addLog(suggestion.substring(0, 300) + (suggestion.length > 300 ? '...' : ''));

                var suggestedParams = parseSuggestedParams(suggestion);
                if (suggestedParams) {
                    addLog('Suggested params: ' + suggestedParams);
                    document.getElementById('bt-params').value = suggestedParams;
                    addLog('Running backtest with AI params...');

                    payload.params = suggestedParams;
                    return fetch('/api/backtest', {
                        method: 'POST',
                        headers: {'Content-Type': 'application/json'},
                        body: JSON.stringify(payload)
                    }).then(function(r) { return r.json(); })
                    .then(function(newResult) {
                        return { base: baseResult, optimized: newResult };
                    });
                } else {
                    addLog('⚠ Could not parse params from AI response');
                    return { base: baseResult, optimized: null };
                }
            });
        })
        .then(function(results) {
            btn.disabled = false;
            btn.textContent = '🤖 AI Optimize Parameters';
            if (results && results.optimized) {
                addLog('AI Optimized: Return=' + (results.optimized.total_return_pct||0).toFixed(2) + '%, Sharpe=' + (results.optimized.sharpe_ratio||0).toFixed(2));
                var improve = (results.optimized.total_return_pct||0) - (results.base.total_return_pct||0);
                addLog((improve >= 0 ? '📈' : '📉') + ' Improvement: ' + (improve >= 0 ? '+' : '') + improve.toFixed(2) + '%');
                lastBacktestResult = results.optimized;
                renderResults(results.optimized);
            }
        })
        .catch(function(e) {
            btn.disabled = false;
            btn.textContent = '🤖 AI Optimize Parameters';
            addLog('❌ Error: ' + e.message);
        });
    };

    function buildAIPrompt(result, params, goalText) {
        return 'You are a quantitative trading strategy optimizer. I run a grid trading strategy on crypto (BTC/ETH perpetuals).\n\n' +
            'Current parameters: ' + params + '\n' +
            'Backtest results:\n' +
            '- Total return: ' + (result.total_return_pct||0).toFixed(2) + '%\n' +
            '- Sharpe ratio: ' + (result.sharpe_ratio||0).toFixed(2) + '\n' +
            '- Max drawdown: ' + (result.max_drawdown_pct||0).toFixed(2) + '%\n' +
            '- Win rate: ' + (result.win_rate_pct||0).toFixed(1) + '%\n' +
            '- Total trades: ' + (result.total_trades||0) + '\n\n' +
            'Optimization goal: ' + goalText + '\n\n' +
            'Available parameters:\n' +
            '- grid_count (integer 4-20): number of grid levels each side\n' +
            '- investment (float 3-30): USD per grid level\n' +
            '- deviation (float 0.005-0.03): price deviation between levels\n\n' +
            'Please suggest improved parameters. Respond with EXACTLY one line in this format:\n' +
            'PARAMS: grid_count=X,investment=Y,deviation=Z\n' +
            'Then explain your reasoning briefly.';
    }

    function callAI(url, model, apiKey, prompt, maxTokens, isAnthropic) {
        var headers, body;

        if (isAnthropic) {
            headers = {
                'x-api-key': apiKey,
                'anthropic-version': '2023-06-01',
                'Content-Type': 'application/json',
                'anthropic-dangerous-direct-browser-access': 'true'
            };
            body = { model: model, max_tokens: maxTokens, messages: [{role:'user',content:prompt}] };
        } else {
            headers = { 'Content-Type': 'application/json' };
            if (apiKey) headers['Authorization'] = 'Bearer ' + apiKey;
            body = { model: model, messages: [{role:'user',content:prompt}], max_tokens: maxTokens };
        }

        return fetch(url, { method: 'POST', headers: headers, body: JSON.stringify(body) })
            .then(function(r) {
                if (!r.ok) return r.text().then(function(t) { throw new Error('HTTP ' + r.status + ': ' + t.substring(0, 300)); });
                return r.json();
            })
            .then(function(d) {
                // Anthropic format
                if (isAnthropic && d.content && d.content[0]) {
                    return d.content[0].text || '';
                }
                // OpenAI-compatible format
                if (d.choices && d.choices[0] && d.choices[0].message) {
                    return d.choices[0].message.content;
                }
                throw new Error('Unexpected response format: ' + JSON.stringify(d).substring(0, 300));
            });
    }

    function parseSuggestedParams(text) {
        var match = text.match(/PARAMS:\s*([\w=.,]+)/i);
        if (match) return match[1].trim();
        var gc = text.match(/grid_count\s*=\s*(\d+)/);
        var inv = text.match(/investment\s*=\s*([\d.]+)/);
        var dev = text.match(/deviation\s*=\s*([\d.]+)/);
        if (gc && inv && dev) {
            return 'grid_count=' + gc[1] + ',investment=' + inv[1] + ',deviation=' + dev[1];
        }
        return null;
    }

})();
