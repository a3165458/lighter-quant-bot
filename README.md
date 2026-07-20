# Lighter Quant Bot 🤖

基于 Rust 的 [Lighter.xyz](https://lighter.xyz) DEX 自动化量化交易机器人。

## ✨ 功能

- **策略引擎**: Grid（网格）策略 + EMA 趋势过滤，可扩展 DCA / Trend Following
- **实时交易**: REST API + WebSocket 双通道，自动重连 + Keepalive
- **Web Dashboard**: 实时监控面板（亮色/暗色主题、中英双语、长周期净值查看）
- **交易控制**: 通过 Dashboard 实时切换交易对、暂停/恢复、一键撤单
- **AI Strategy Lab**: 内置回测引擎 + AI 参数优化（支持 OpenAI / ZhiPu / 自定义 API / OpenCode GLM5）
- **风控系统**: 止损、最大回撤、日亏损限制、杠杆限制
- **PnL 持久化**: 净值曲线、交易历史、每日盈亏自动保存和恢复
- **高性能**: Tokio 异步运行时，低延迟订单执行

## 🚀 快速开始

### 前置条件

- [Lighter.xyz](https://lighter.xyz) 账户（已创建 API Key）
- Docker（推荐）或 Rust 1.80+

### 方式一：Docker 部署（推荐）

```bash
# 1. 克隆项目
git clone https://github.com/your-username/lighter-quant-bot.git
cd lighter-quant-bot

# 2. 创建并编辑 .env
cp .env.example .env
nano .env   # 填入你的 API 凭证

# 3. 一键启动
docker compose up -d

# 4. 查看 Dashboard
# 浏览器打开 http://localhost:2028
```

### 方式二：本地编译

```bash
# 1. 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 2. 获取 lighter-signer.so
pip install lighter-sdk
cp $(python3 -c "import lighter,os; print(os.path.join(os.path.dirname(lighter.__file__), 'signers', 'lighter-signer-linux-amd64.so'))") ./lighter-signer.so

# 3. 配置环境变量
cp .env.example .env
nano .env

# 4. 编译并运行
cargo build --release
./target/release/lighter-bot live --config config/settings.yaml
```

### 方式三：PM2 守护进程

```bash
# 编译后
npm install -g pm2
pm2 start ecosystem.config.js
pm2 logs lighter-bot
```

## ⚙️ 配置说明

### .env 必填字段（仅三项）

| 变量 | 说明 | 对应 API Key 生成弹窗字段 |
|------|------|--------------------------|
| `LIGHTER_SECRET_KEY` | API 私钥 (hex)，注意不是钱包 L1 私钥 | 「私钥」（弹窗关闭后无法再查看） |
| `LIGHTER_ACCOUNT_INDEX` | 账户编号 | 「您的账户索引」 |
| `LIGHTER_API_KEY_INDEX` | API Key 槽位编号 | 「API 密钥索引」 |

「公钥」无需填写；网络选择由 `--config` 决定（主网 `settings.yaml`，Robinhood Chain `settings.robinhood.yaml`，两边账户凭据独立）。

### config/settings.yaml 关键配置

```yaml
trading:
  markets: [1]              # 0=ETH, 1=BTC
  strategies:
    grid_trading:
      grid_count: 6          # 网格层数
      investment_per_grid: 8  # 每格投资 ($)
      price_deviation: 0.012  # 价格偏差 (1.2%)

risk:
  stop_loss:
    max_drawdown_percent: 10
    daily_loss_limit_percent: 5
    position_stop_loss_percent: 3
    position_take_profit_percent: 5
```

## 🪶 Robinhood Chain 实例

机器人同时支持 [Lighter on Robinhood Chain](https://robinhoodchain.lighter.xyz/)（USDG 计价，含 BTC/ETH/SOL 等加密永续与 AAPL/TSLA/NVDA 等股票永续）：

```bash
# 实盘（.env 需使用 Robinhood Chain 实例的账户凭据）
cargo run --release -- live --config config/settings.robinhood.yaml

# 下载 RH 实例历史数据（--url 指定实例）
cargo run --release -- download --symbol TSLA --interval 1h \
  --start 2026-06-26 --end 2026-07-18 --url https://api.rh.lighter.xyz

# 回测
cargo run --release -- backtest --strategy grid \
  --data backtests/data/TSLA-rh-1h-20260626-20260718.csv \
  --start 2026-06-26 --end 2026-07-18 --params "grid_count=8,investment=30,deviation=0.003"
```

关键差异：

| 项目 | 主网 | Robinhood Chain |
|------|------|-----------------|
| REST | `https://mainnet.zklighter.elliot.ai` | `https://api.rh.lighter.xyz` |
| WebSocket | `wss://mainnet.zklighter.elliot.ai/stream` | `wss://api.rh.lighter.xyz/stream` |
| 签名 chain_id | 304 | 466324 |
| 计价资产 | USDC | USDG |
| 市场 | 加密永续 | 加密+股票永续、现货（股票永续周末休市） |

市场符号与 ID 在启动时从交易所动态拉取注册，无需手工维护映射。

## 📊 Dashboard

访问 `http://localhost:2028`（默认端口）

| 功能 | 说明 |
|------|------|
| 📈 Dashboard | 净值曲线、盈亏统计、持仓、风险监控 |
| 📋 Strategies | 策略参数配置、交易控制（开关市场、暂停） |
| 💼 Portfolio | 持仓和挂单详情 |
| 📜 History | 完整交易历史 + CSV 导出 + 平均持仓时间 |
| ⚙️ Settings | 系统状态、风控限制、主题切换（不暴露账户编号） |
| 🤖 AI Lab | 回测引擎 + AI 参数优化 + OpenCode GLM5 联合回测 |

## 📝 更新历史 / 回测记录

### 2026-07-19 (二)

- **RH BTC 与主网价格同源验证 + 多窗口调参**
  - 重叠时段逐时对比：07-09 之后 RH 与主网 BTC 收盘价差仅 0.03~0.07%（上线初期 07-04~07-08 偏离最高 4%，该段数据不可用）
  - 相同子窗口（07-09~07-18）两边数据回测结果一致（-0.10% vs -0.08%）→ 主网 6.5 个月历史可直接用于 RH BTC 调参
  - 多窗口稳健性检验（1-3月/3-5月/5-6月/6-7月）取代单一 train/OOS 切分：
    `grid_count=12, deviation=0.004` 四窗口全正、累计 +3.00%、MaxDD 1.78%（全周期 1046 笔、胜率 52.3%）
  - 主网与 RH 配置统一更新为 `grid_count=12, price_deviation=0.004`

### 2026-07-19

- **Robinhood Chain 实例支持**
  - 新增 `config/settings.robinhood.yaml`（REST/WS 端点、签名 chain_id=466324）
  - 市场符号注册表改为启动时从 `/api/v1/orderBookDetails` 动态拉取，移除全部 ETH/BTC 硬编码
  - `download` 命令支持 `--url` 指定实例并自动分页（API 单次上限 500 根）
- **策略修复与调参**
  - 趋势策略重写：仓位状态真正生效（原止损/止盈为死代码）、EMA 交叉、移动止损、名义金额仓位
  - 回测引擎夏普年化按数据间隔自动推断
  - 真实主网 1h 数据（2026-01-01~07-18）train/OOS 调参：网格更新为 `grid_count=8, deviation=0.003`
    （样本外 Sharpe ≈1.0；旧参数 dev=0.016 样本外为负）；趋势策略样本外不稳健，保持禁用
  - RH 实例仅 ~3 周历史（2026-06-26 上线），近期单边行情下网格为负——建议小仓位观察、数据积累后再独立调参

### 2026-04-19

- **Dashboard 升级**
  - Equity Curve 新增 `ALL / 30D / 7D / 24H` 视图切换，支持查看早期净值与交易阶段
  - 面板移除订单簿模块，降低前端与 Dashboard WebSocket 资源占用
  - 历史页恢复 `Avg Duration` 平均持仓时间，并移除胜率展示
  - Settings 页面移除 `Account Index` 展示，避免公开仓库/公开面板泄露账户标识
- **AI Lab**
  - 新增 OpenCode GLM5 本地优化入口
  - 已验证本机 `opencode --pure -m opencode-go/glm-5` 可返回策略参数并联动回测
- **OpenCode GLM5 回测记录**
  - 建议参数：`grid_count=10,investment=40,deviation=0.01`
  - 结果：`Return +6.83% | Sharpe 2.09 | MaxDD 7.91% | Trades 58 | Profit Factor 2.45`

### 2026-04-04

- **Grid 策略优化**
  - 引入多层 EMA 趋势过滤、trailing anchor、1.5x anchor reset、每侧最大累积层数限制
  - 持久化参数修复后，Dashboard 调整的 `investment_per_grid` / `price_deviation` 可正确生效
- **风险与配置**
  - 风控改为杠杆感知的单笔限额
  - Strategy / Risk 配置支持重启恢复
- **回测记录**
  - 参数：`grid_count=6,investment=60,deviation=0.016`
  - 结果：`Return +4.81% | Sharpe 1.79 | MaxDD 7.51% | Trades 18 | Profit Factor 2.78`

## 🏗️ 项目结构

```
src/
├── main.rs              # 入口: 交易循环、信号处理
├── lighter/             # Lighter.xyz API 封装
│   ├── client.rs        # REST API 客户端
│   ├── websocket.rs     # WebSocket 实时数据
│   ├── ffi.rs           # 签名库 FFI 绑定
│   └── types.rs         # 数据类型定义
├── strategy/            # 交易策略
│   ├── grid_strategy.rs # 网格策略 + EMA 过滤
│   ├── trend_strategy.rs# 趋势跟踪策略
│   └── mod.rs           # 策略 trait 定义
├── dashboard/           # Web 监控面板
│   ├── server.rs        # Axum HTTP/WS 服务
│   └── ui/              # 前端文件 (embedded)
├── backtest/            # 回测引擎
├── risk/                # 风控管理
└── data/                # 数据加载
```

## 🐳 Docker 命令参考

```bash
docker compose up -d      # 启动
docker compose logs -f     # 查看日志
docker compose restart     # 重启
docker compose down        # 停止
docker compose build       # 重新构建
```

## ⚠️ 风险提示

- 量化交易存在亏损风险，请勿投入超过你能承受的资金
- 建议先在 `testnet` 上测试，确认策略有效后再切换到 `mainnet`
- 定期检查 Dashboard 监控交易状态和盈亏情况
- 私钥和 API Key 请妥善保管，不要提交到公开仓库

## 📄 License

MIT
