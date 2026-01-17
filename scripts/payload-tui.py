#!/usr/bin/env python3
"""Real-time TUI comparing Reth vs Nethermind payload execution performance."""

import curses
import json
import time
import urllib.request
from collections import deque
from datetime import datetime
from statistics import mean, median

API_BASE = "https://lab.ethpandaops.io/api/v1/mainnet/int_engine_new_payload"
CLIENTS = ["Reth", "Nethermind"]
HISTORY_SIZE = 200
POLL_INTERVAL = 3  # seconds


def fetch_payloads(client: str, since_ts: int, limit: int = 50) -> list:
    """Fetch recent payloads for a client."""
    url = f"{API_BASE}?slot_start_date_time_gte={since_ts}&meta_execution_implementation_eq={client}&page_size={limit}&order_by=slot%20desc"
    try:
        with urllib.request.urlopen(url, timeout=5) as resp:
            data = json.loads(resp.read())
            return data.get("int_engine_new_payload", [])
    except Exception:
        return []


def calc_stats(durations: list) -> dict:
    """Calculate statistics from duration list."""
    if not durations:
        return {"avg": 0, "p50": 0, "p95": 0, "min": 0, "max": 0, "count": 0}
    sorted_d = sorted(durations)
    p95_idx = int(len(sorted_d) * 0.95)
    return {
        "avg": mean(durations),
        "p50": median(durations),
        "p95": sorted_d[p95_idx] if p95_idx < len(sorted_d) else sorted_d[-1],
        "min": min(durations),
        "max": max(durations),
        "count": len(durations),
    }


def format_duration(ms: float, width: int = 6) -> str:
    """Format duration with color hint."""
    return f"{ms:>{width}.0f}ms"


def draw_bar(value: float, max_val: float, width: int) -> str:
    """Draw a horizontal bar."""
    if max_val == 0:
        return " " * width
    filled = int((value / max_val) * width)
    return "█" * filled + "░" * (width - filled)


def main(stdscr):
    curses.curs_set(0)
    stdscr.timeout(100)
    curses.start_color()
    curses.use_default_colors()
    
    # Colors: 1=green (good), 2=yellow (warn), 3=red (bad), 4=cyan (reth), 5=magenta (nethermind)
    curses.init_pair(1, curses.COLOR_GREEN, -1)
    curses.init_pair(2, curses.COLOR_YELLOW, -1)
    curses.init_pair(3, curses.COLOR_RED, -1)
    curses.init_pair(4, curses.COLOR_CYAN, -1)
    curses.init_pair(5, curses.COLOR_MAGENTA, -1)
    curses.init_pair(6, curses.COLOR_WHITE, -1)

    # State
    history = {c: deque(maxlen=HISTORY_SIZE) for c in CLIENTS}
    recent = {c: [] for c in CLIENTS}
    seen_slots = {c: set() for c in CLIENTS}
    last_fetch = 0
    start_time = time.time()

    while True:
        now = time.time()
        
        # Poll API
        if now - last_fetch >= POLL_INTERVAL:
            since = int(now - 300)  # last 5 min
            for client in CLIENTS:
                payloads = fetch_payloads(client, since, limit=30)
                new_payloads = []
                for p in payloads:
                    slot = p.get("slot")
                    if slot and slot not in seen_slots[client]:
                        seen_slots[client].add(slot)
                        history[client].append(p)
                        new_payloads.append(p)
                recent[client] = payloads[:10]
            last_fetch = now
            # Trim seen_slots to prevent memory growth
            for c in CLIENTS:
                if len(seen_slots[c]) > 1000:
                    seen_slots[c] = set(list(seen_slots[c])[-500:])

        # Draw
        stdscr.clear()
        height, width = stdscr.getmaxyx()
        
        # Header
        header = "═" * (width - 1)
        stdscr.addstr(0, 0, header, curses.color_pair(6))
        title = " ⚡ RETH vs NETHERMIND - Payload Execution Monitor "
        stdscr.addstr(1, (width - len(title)) // 2, title, curses.A_BOLD)
        elapsed = int(now - start_time)
        elapsed_str = f"Running: {elapsed//60}m {elapsed%60}s | Updated: {datetime.now().strftime('%H:%M:%S')}"
        stdscr.addstr(2, (width - len(elapsed_str)) // 2, elapsed_str, curses.color_pair(6))
        stdscr.addstr(3, 0, header, curses.color_pair(6))

        # Stats comparison
        row = 5
        stats = {}
        for client in CLIENTS:
            durations = [p.get("duration_ms", 0) for p in history[client]]
            stats[client] = calc_stats(durations)

        # Stats header
        stdscr.addstr(row, 2, "STATISTICS", curses.A_BOLD | curses.A_UNDERLINE)
        stdscr.addstr(row, 25, "Reth", curses.color_pair(4) | curses.A_BOLD)
        stdscr.addstr(row, 45, "Nethermind", curses.color_pair(5) | curses.A_BOLD)
        stdscr.addstr(row, 65, "Δ Winner", curses.A_BOLD)
        row += 2

        metrics = [
            ("Samples", "count", False),
            ("Average", "avg", True),
            ("Median (P50)", "p50", True),
            ("P95", "p95", True),
            ("Min", "min", True),
            ("Max", "max", True),
        ]

        for label, key, lower_better in metrics:
            reth_val = stats["Reth"][key]
            neth_val = stats["Nethermind"][key]
            
            stdscr.addstr(row, 2, f"{label}:", curses.A_BOLD)
            
            if key == "count":
                stdscr.addstr(row, 25, f"{reth_val}", curses.color_pair(4))
                stdscr.addstr(row, 45, f"{neth_val}", curses.color_pair(5))
            else:
                stdscr.addstr(row, 25, f"{reth_val:>6.0f}ms", curses.color_pair(4))
                stdscr.addstr(row, 45, f"{neth_val:>6.0f}ms", curses.color_pair(5))
                
                # Winner
                if reth_val > 0 and neth_val > 0:
                    if lower_better:
                        diff = neth_val - reth_val
                        pct = (diff / neth_val) * 100 if neth_val else 0
                        if diff > 0:
                            winner = f"Reth +{pct:.1f}%"
                            color = curses.color_pair(4)
                        else:
                            winner = f"Neth +{-pct:.1f}%"
                            color = curses.color_pair(5)
                        stdscr.addstr(row, 65, winner, color | curses.A_BOLD)
            row += 1

        # Visual comparison bars
        row += 2
        stdscr.addstr(row, 2, "LATENCY DISTRIBUTION", curses.A_BOLD | curses.A_UNDERLINE)
        row += 2
        
        max_avg = max(stats["Reth"]["avg"], stats["Nethermind"]["avg"], 1)
        bar_width = min(40, width - 30)
        
        stdscr.addstr(row, 2, "Reth Avg:", curses.color_pair(4))
        bar = draw_bar(stats["Reth"]["avg"], max_avg * 1.2, bar_width)
        stdscr.addstr(row, 15, bar, curses.color_pair(4))
        stdscr.addstr(row, 16 + bar_width, f"{stats['Reth']['avg']:.0f}ms", curses.color_pair(4))
        row += 1
        
        stdscr.addstr(row, 2, "Neth Avg:", curses.color_pair(5))
        bar = draw_bar(stats["Nethermind"]["avg"], max_avg * 1.2, bar_width)
        stdscr.addstr(row, 15, bar, curses.color_pair(5))
        stdscr.addstr(row, 16 + bar_width, f"{stats['Nethermind']['avg']:.0f}ms", curses.color_pair(5))
        
        # Recent payloads
        row += 3
        stdscr.addstr(row, 2, "RECENT PAYLOADS", curses.A_BOLD | curses.A_UNDERLINE)
        row += 2
        
        col_reth = 2
        col_neth = width // 2 + 2
        
        stdscr.addstr(row, col_reth, "Reth", curses.color_pair(4) | curses.A_BOLD)
        stdscr.addstr(row, col_neth, "Nethermind", curses.color_pair(5) | curses.A_BOLD)
        row += 1
        stdscr.addstr(row, col_reth, "Slot       Block      Duration  Gas", curses.A_DIM)
        stdscr.addstr(row, col_neth, "Slot       Block      Duration  Gas", curses.A_DIM)
        row += 1
        
        max_rows = min(10, height - row - 2)
        for i in range(max_rows):
            # Reth
            if i < len(recent["Reth"]):
                p = recent["Reth"][i]
                slot = p.get("slot", 0)
                block = p.get("block_number", 0)
                dur = p.get("duration_ms", 0)
                gas = p.get("gas_used", 0) / 1e6
                
                dur_color = curses.color_pair(1) if dur < 50 else (curses.color_pair(2) if dur < 150 else curses.color_pair(3))
                line = f"{slot:<10} {block:<10} "
                stdscr.addstr(row + i, col_reth, line, curses.color_pair(4))
                stdscr.addstr(row + i, col_reth + len(line), f"{dur:>4}ms", dur_color)
                stdscr.addstr(row + i, col_reth + len(line) + 7, f"{gas:>5.1f}M", curses.A_DIM)
            
            # Nethermind
            if i < len(recent["Nethermind"]):
                p = recent["Nethermind"][i]
                slot = p.get("slot", 0)
                block = p.get("block_number", 0)
                dur = p.get("duration_ms", 0)
                gas = p.get("gas_used", 0) / 1e6
                
                dur_color = curses.color_pair(1) if dur < 50 else (curses.color_pair(2) if dur < 150 else curses.color_pair(3))
                line = f"{slot:<10} {block:<10} "
                stdscr.addstr(row + i, col_neth, line, curses.color_pair(5))
                stdscr.addstr(row + i, col_neth + len(line), f"{dur:>4}ms", dur_color)
                stdscr.addstr(row + i, col_neth + len(line) + 7, f"{gas:>5.1f}M", curses.A_DIM)

        # Footer
        footer_row = height - 1
        footer = " Press 'q' to quit | Polling every 3s "
        stdscr.addstr(footer_row, (width - len(footer)) // 2, footer, curses.A_DIM)

        stdscr.refresh()

        # Handle input
        try:
            key = stdscr.getch()
            if key == ord('q') or key == ord('Q'):
                break
        except:
            pass

        time.sleep(0.1)


if __name__ == "__main__":
    curses.wrapper(main)
