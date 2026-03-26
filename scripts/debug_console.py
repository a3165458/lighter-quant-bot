#!/usr/bin/env python3
"""
交互式调试控制台
"""
import asyncio
import json
import sys

try:
    import websockets
except ImportError:
    print("请安装 websockets: pip install websockets")
    sys.exit(1)


async def debug_console():
    uri = "ws://localhost:3000/ws"

    try:
        async with websockets.connect(uri) as websocket:
            print("✅ 连接到交易机器人调试控制台")
            print("可用命令: status, positions, trades, symbols, help, exit")
            print()

            while True:
                command = input("> ").strip().lower()

                if command == "exit":
                    break
                elif command == "help":
                    print("命令列表:")
                    print("  status - 获取系统状态")
                    print("  positions - 获取当前持仓")
                    print("  trades - 获取最近交易")
                    print("  symbols - 获取监控的交易对")
                    print("  subscribe <symbol> - 订阅交易对")
                    print("  unsubscribe <symbol> - 取消订阅")
                    print("  help - 显示此帮助")
                    print("  exit - 退出控制台")
                elif command == "status":
                    await websocket.send(json.dumps({"type": "status"}))
                elif command == "positions":
                    await websocket.send(json.dumps({"type": "positions"}))
                elif command == "trades":
                    await websocket.send(json.dumps({"type": "recent_trades"}))
                elif command == "symbols":
                    await websocket.send(json.dumps({"type": "symbols"}))
                elif command.startswith("subscribe "):
                    symbol = command.split(" ", 1)[1]
                    await websocket.send(json.dumps({
                        "type": "subscribe",
                        "symbol": symbol
                    }))
                elif command.startswith("unsubscribe "):
                    symbol = command.split(" ", 1)[1]
                    await websocket.send(json.dumps({
                        "type": "unsubscribe",
                        "symbol": symbol
                    }))
                else:
                    print("未知命令，输入 'help' 查看可用命令")
                    continue

                # 接收响应
                try:
                    response = await asyncio.wait_for(websocket.recv(), timeout=2.0)
                    data = json.loads(response)
                    print(json.dumps(data, indent=2, ensure_ascii=False))
                except asyncio.TimeoutError:
                    print("⏰ 响应超时")

    except ConnectionRefusedError:
        print("❌ 无法连接到交易机器人，请确保机器人正在运行")
    except Exception as e:
        print(f"❌ 错误: {e}")


if __name__ == "__main__":
    asyncio.run(debug_console())
