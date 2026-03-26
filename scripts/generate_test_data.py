#!/usr/bin/env python3
"""
生成用于回测的模拟历史数据
"""
import csv
import math
import os
import random
import sys
from datetime import datetime, timedelta


def generate_ohlc_data(symbol, start_date, end_date, interval='1h', volatility=0.02):
    """生成OHLC模拟数据"""
    start = datetime.strptime(start_date, "%Y-%m-%d")
    end = datetime.strptime(end_date, "%Y-%m-%d")

    # 计算时间步长
    if interval == '1h':
        delta = timedelta(hours=1)
    elif interval == '4h':
        delta = timedelta(hours=4)
    elif interval == '1d':
        delta = timedelta(days=1)
    else:
        delta = timedelta(hours=1)

    random.seed(42)
    rows = []
    price = 10000.0
    current = start

    while current <= end:
        # 随机游走
        ret = random.gauss(0, volatility)
        price *= math.exp(ret)

        open_p = price * (1 + random.uniform(-0.001, 0.001))
        close_p = price
        high_p = max(open_p, close_p) * (1 + random.uniform(0, 0.02))
        low_p = min(open_p, close_p) * (1 - random.uniform(0, 0.02))
        volume = math.exp(random.gauss(10, 1))

        rows.append({
            'timestamp': current.isoformat(),
            'open': round(open_p, 2),
            'high': round(high_p, 2),
            'low': round(low_p, 2),
            'close': round(close_p, 2),
            'volume': round(volume, 4),
            'symbol': symbol
        })

        current += delta

    return rows


if __name__ == "__main__":
    symbol = sys.argv[1] if len(sys.argv) > 1 else "BTCUSDT"
    start_date = sys.argv[2] if len(sys.argv) > 2 else "2024-01-01"
    end_date = sys.argv[3] if len(sys.argv) > 3 else "2024-06-01"

    print(f"生成 {symbol} 数据从 {start_date} 到 {end_date}")

    rows = generate_ohlc_data(symbol, start_date, end_date, '1h')

    os.makedirs("backtests/data", exist_ok=True)
    output_file = f"backtests/data/{symbol}-1h.csv"

    with open(output_file, 'w', newline='') as f:
        writer = csv.DictWriter(f, fieldnames=['timestamp', 'open', 'high', 'low', 'close', 'volume', 'symbol'])
        writer.writeheader()
        writer.writerows(rows)

    prices = [r['close'] for r in rows]
    print(f"✅ 数据已生成: {output_file}")
    print(f"   {len(rows)} 条记录")
    print(f"   价格范围: ${min(prices):.2f} - ${max(prices):.2f}")
