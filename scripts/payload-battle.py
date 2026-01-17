#!/usr/bin/env python3
"""Real-time TUI: Reth vs Nethermind payload execution battle."""

import json
import time
import urllib.request
from collections import deque
from datetime import datetime
from statistics import mean, median
from rich.console import Console
from rich.live import Live
from rich.table import Table
from rich.panel import Panel
from rich.layout import Layout
from rich.text import Text
from rich.progress import BarColumn, Progress, TextColumn
from rich import box

API = "https://lab.ethpandaops.io/api/v1/mainnet/int_engine_new_payload"
POLL_INTERVAL = 2

console = Console()

# Battle state
reth_wins = 0
neth_wins = 0
ties = 0
# Per-metric scores: {metric: {"reth": X, "neth": Y}}
metric_scores = {
    "avg": {"reth": 0, "neth": 0},
    "median": {"reth": 0, "neth": 0},
    "p95": {"reth": 0, "neth": 0},
    "max": {"reth": 0, "neth": 0},
}
history = {"Reth": deque(maxlen=500), "Nethermind": deque(maxlen=500)}
recent = {"Reth": [], "Nethermind": []}
seen_blocks = set()
battle_log = deque(maxlen=12)
last_stats = {"Reth": {}, "Nethermind": {}}


def fetch(client: str, since: int, limit: int = 50) -> list:
    url = f"{API}?slot_start_date_time_gte={since}&meta_execution_implementation_eq={client}&page_size={limit}&order_by=slot%20desc"
    try:
        with urllib.request.urlopen(url, timeout=5) as r:
            return json.loads(r.read()).get("int_engine_new_payload", [])
    except:
        return []


def stats(durations: list) -> dict:
    if not durations:
        return {"avg": 0, "p50": 0, "p95": 0, "min": 0, "max": 0, "n": 0}
    s = sorted(durations)
    return {
        "avg": mean(durations),
        "p50": median(durations),
        "p95": s[int(len(s) * 0.95)] if len(s) > 1 else s[-1],
        "min": min(durations),
        "max": max(durations),
        "n": len(durations),
    }


def color_ms(ms: float) -> Text:
    if ms < 50:
        return Text(f"{ms:.0f}ms", style="green bold")
    elif ms < 150:
        return Text(f"{ms:.0f}ms", style="yellow")
    return Text(f"{ms:.0f}ms", style="red bold")


def sparkline(reth_vals: list, neth_vals: list, width: int = 60) -> Text:
    """Create overlaid sparkline chart for both clients."""
    SPARKS = " ▁▂▃▄▅▆▇█"
    
    result = Text()
    result.append("CHART ", style="bold")
    result.append("🦀", style="cyan")
    result.append("/", style="dim")
    result.append("🔷 ", style="magenta")
    
    all_vals = reth_vals + neth_vals
    if not all_vals:
        return result
    
    max_val = max(all_vals) if all_vals else 1
    min_val = min(all_vals) if all_vals else 0
    val_range = max_val - min_val or 1
    
    # Normalize to width
    def resample(vals, target_len):
        if not vals:
            return [0] * target_len
        if len(vals) <= target_len:
            return vals + [vals[-1]] * (target_len - len(vals))
        step = len(vals) / target_len
        return [vals[int(i * step)] for i in range(target_len)]
    
    reth_resampled = resample(reth_vals, width)
    neth_resampled = resample(neth_vals, width)
    
    for i in range(width):
        r_val = reth_resampled[i] if i < len(reth_resampled) else 0
        n_val = neth_resampled[i] if i < len(neth_resampled) else 0
        
        r_idx = int((r_val - min_val) / val_range * (len(SPARKS) - 1))
        n_idx = int((n_val - min_val) / val_range * (len(SPARKS) - 1))
        
        # Show winner's spark, colored appropriately
        if r_val <= n_val:
            result.append(SPARKS[r_idx], style="cyan")
        else:
            result.append(SPARKS[n_idx], style="magenta")
    
    result.append(f" {min_val:.0f}-{max_val:.0f}ms", style="dim")
    return result


def battle_bar(reth_pct: float, reth_score: int = 0, neth_score: int = 0) -> Text:
    width = 40
    reth_fill = int(reth_pct * width)
    neth_fill = width - reth_fill
    
    # Create the bar with percentages embedded
    reth_pct_str = f" {reth_pct*100:.0f}% "
    neth_pct_str = f" {(1-reth_pct)*100:.0f}% "
    
    bar = Text()
    
    # Reth side
    if reth_fill >= len(reth_pct_str) + 2:
        padding = (reth_fill - len(reth_pct_str)) // 2
        bar.append("█" * padding, style="cyan bold")
        bar.append(reth_pct_str, style="bold white on cyan")
        bar.append("█" * (reth_fill - padding - len(reth_pct_str)), style="cyan bold")
    else:
        bar.append("█" * reth_fill, style="cyan bold")
    
    # Neth side
    if neth_fill >= len(neth_pct_str) + 2:
        padding = (neth_fill - len(neth_pct_str)) // 2
        bar.append("█" * padding, style="magenta bold")
        bar.append(neth_pct_str, style="bold white on magenta")
        bar.append("█" * (neth_fill - padding - len(neth_pct_str)), style="magenta bold")
    else:
        bar.append("█" * neth_fill, style="magenta bold")
    
    return bar


def make_layout() -> Layout:
    layout = Layout()
    layout.split_column(
        Layout(name="header", size=5),
        Layout(name="battle", size=13),
        Layout(name="stats", size=10),
        Layout(name="recent", size=11),
        Layout(name="log", size=6),
    )
    return layout


def render(start_time: float) -> Layout:
    global reth_wins, neth_wins, ties
    
    layout = make_layout()
    elapsed = int(time.time() - start_time)
    
    # Header
    header = Text()
    header.append("⚡ PAYLOAD EXECUTION BATTLE ⚡\n", style="bold white")
    header.append(f"Reth vs Nethermind | ", style="dim")
    header.append(f"Running {elapsed//60}m {elapsed%60}s | ", style="dim")
    header.append(f"Updated {datetime.now().strftime('%H:%M:%S')}", style="dim")
    layout["header"].update(Panel(header, box=box.DOUBLE))
    
    # Calculate stats
    reth_stats = stats([p["duration_ms"] for p in history["Reth"]])
    neth_stats = stats([p["duration_ms"] for p in history["Nethermind"]])
    
    # Battle scoreboard with per-metric bars
    total = reth_wins + neth_wins + ties
    reth_pct = reth_wins / total if total > 0 else 0.5
    
    battle = Text()
    battle.append("🦀 RETH ", style="cyan bold")
    battle.append(f"{reth_wins}", style="cyan bold")
    battle.append("  vs  ", style="dim")
    battle.append(f"{neth_wins}", style="magenta bold")
    battle.append(" NETHERMIND 🔷", style="magenta bold")
    battle.append(f"  (Ties: {ties})\n\n", style="dim")
    
    # Per-metric battle bars with individual scores
    metrics = [
        ("AVG   ", "avg", reth_stats["avg"], neth_stats["avg"]),
        ("MEDIAN", "median", reth_stats["p50"], neth_stats["p50"]),
        ("P95   ", "p95", reth_stats["p95"], neth_stats["p95"]),
        ("MAX   ", "max", reth_stats["max"], neth_stats["max"]),
    ]
    
    for label, key, reth_val, neth_val in metrics:
        r_score = metric_scores[key]["reth"]
        n_score = metric_scores[key]["neth"]
        total_score = r_score + n_score
        r_pct = r_score / total_score if total_score > 0 else 0.5
        
        # Score display
        battle.append(f"{label} ", style="bold")
        battle.append(f"{r_score:>3}", style="cyan bold")
        battle.append("-", style="dim")
        battle.append(f"{n_score:<3} ", style="magenta bold")
        battle.append_text(battle_bar(r_pct, r_score, n_score))
        
        # Show current values
        if reth_val < neth_val:
            battle.append(f" 🦀{reth_val:.0f}", style="cyan")
        elif neth_val < reth_val:
            battle.append(f" 🔷{neth_val:.0f}", style="magenta")
        else:
            battle.append(f" ={reth_val:.0f}", style="yellow")
        battle.append("\n")
    
    # Line chart of recent block times
    battle.append("\n", style="dim")
    reth_times = [p["duration_ms"] for p in list(history["Reth"])[-50:]]
    neth_times = [p["duration_ms"] for p in list(history["Nethermind"])[-50:]]
    
    if reth_times or neth_times:
        battle.append_text(sparkline(reth_times, neth_times))
    
    layout["battle"].update(Panel(battle, title="[bold]SCOREBOARD[/bold]", box=box.ROUNDED))
    
    # Stats comparison table
    stats_table = Table(box=box.SIMPLE, expand=True, show_header=True)
    stats_table.add_column("Metric", style="bold")
    stats_table.add_column("🦀 Reth", style="cyan", justify="right")
    stats_table.add_column("🔷 Neth", style="magenta", justify="right")
    stats_table.add_column("Winner", justify="center")
    
    def winner(r, n, lower_better=True):
        if r == 0 or n == 0:
            return Text("-", style="dim")
        if lower_better:
            if r < n:
                pct = (n - r) / n * 100
                return Text(f"🦀 +{pct:.0f}%", style="cyan bold")
            elif n < r:
                pct = (r - n) / r * 100
                return Text(f"🔷 +{pct:.0f}%", style="magenta bold")
        return Text("TIE", style="yellow")
    
    stats_table.add_row("Samples", str(reth_stats["n"]), str(neth_stats["n"]), Text("-", style="dim"))
    stats_table.add_row("Average", f"{reth_stats['avg']:.0f}ms", f"{neth_stats['avg']:.0f}ms", winner(reth_stats['avg'], neth_stats['avg']))
    stats_table.add_row("Median", f"{reth_stats['p50']:.0f}ms", f"{neth_stats['p50']:.0f}ms", winner(reth_stats['p50'], neth_stats['p50']))
    stats_table.add_row("P95", f"{reth_stats['p95']:.0f}ms", f"{neth_stats['p95']:.0f}ms", winner(reth_stats['p95'], neth_stats['p95']))
    stats_table.add_row("Max", f"{reth_stats['max']:.0f}ms", f"{neth_stats['max']:.0f}ms", winner(reth_stats['max'], neth_stats['max']))
    
    layout["stats"].update(Panel(stats_table, title="[bold]PERFORMANCE STATS[/bold]", box=box.ROUNDED))
    
    # Recent payloads side by side
    recent_table = Table(box=box.SIMPLE, expand=True, show_header=True)
    recent_table.add_column("Slot", style="dim")
    recent_table.add_column("🦀 Reth", justify="right")
    recent_table.add_column("Gas", style="dim", justify="right")
    recent_table.add_column("│", style="dim", justify="center", width=1)
    recent_table.add_column("🔷 Neth", justify="right")
    recent_table.add_column("Gas", style="dim", justify="right")
    recent_table.add_column("⚔️", justify="center")
    
    for i in range(min(8, max(len(recent["Reth"]), len(recent["Nethermind"])))):
        reth_p = recent["Reth"][i] if i < len(recent["Reth"]) else None
        neth_p = recent["Nethermind"][i] if i < len(recent["Nethermind"]) else None
        
        slot = str(reth_p["slot"] if reth_p else (neth_p["slot"] if neth_p else ""))
        reth_dur = color_ms(reth_p["duration_ms"]) if reth_p else Text("-", style="dim")
        reth_gas = f"{reth_p['gas_used']/1e6:.1f}M" if reth_p else "-"
        neth_dur = color_ms(neth_p["duration_ms"]) if neth_p else Text("-", style="dim")
        neth_gas = f"{neth_p['gas_used']/1e6:.1f}M" if neth_p else "-"
        
        # Who won this block?
        if reth_p and neth_p and reth_p["slot"] == neth_p["slot"]:
            if reth_p["duration_ms"] < neth_p["duration_ms"]:
                win = Text("🦀", style="cyan")
            elif neth_p["duration_ms"] < reth_p["duration_ms"]:
                win = Text("🔷", style="magenta")
            else:
                win = Text("=", style="yellow")
        else:
            win = Text("-", style="dim")
        
        recent_table.add_row(slot, reth_dur, reth_gas, "│", neth_dur, neth_gas, win)
    
    layout["recent"].update(Panel(recent_table, title="[bold]RECENT BLOCKS[/bold]", box=box.ROUNDED))
    
    # Battle log
    log_text = Text()
    for entry in list(battle_log)[-6:]:
        log_text.append(entry + "\n")
    layout["log"].update(Panel(log_text, title="[bold]BATTLE LOG[/bold]", box=box.ROUNDED))
    
    return layout


def main():
    global reth_wins, neth_wins, ties, recent, battle_log
    
    start_time = time.time()
    last_fetch = 0
    
    console.clear()
    
    with Live(render(start_time), console=console, refresh_per_second=4, screen=True) as live:
        while True:
            now = time.time()
            
            if now - last_fetch >= POLL_INTERVAL:
                since = int(now - 300)
                
                for client in ["Reth", "Nethermind"]:
                    payloads = fetch(client, since, limit=20)
                    recent[client] = payloads
                    
                    for p in payloads:
                        block = p.get("block_number")
                        if block and (client, block) not in seen_blocks:
                            seen_blocks.add((client, block))
                            history[client].append(p)
                
                # Determine winners for matching blocks
                reth_blocks = {p["block_number"]: p for p in recent["Reth"]}
                neth_blocks = {p["block_number"]: p for p in recent["Nethermind"]}
                
                for block in set(reth_blocks.keys()) & set(neth_blocks.keys()):
                    if ("battle", block) not in seen_blocks:
                        seen_blocks.add(("battle", block))
                        r = reth_blocks[block]["duration_ms"]
                        n = neth_blocks[block]["duration_ms"]
                        slot = reth_blocks[block]["slot"]
                        
                        if r < n:
                            reth_wins += 1
                            diff = n - r
                            battle_log.append(f"[cyan]🦀 RETH WINS[/cyan] slot {slot}: {r}ms vs {n}ms (+{diff}ms)")
                        elif n < r:
                            neth_wins += 1
                            diff = r - n
                            battle_log.append(f"[magenta]🔷 NETH WINS[/magenta] slot {slot}: {n}ms vs {r}ms (+{diff}ms)")
                        else:
                            ties += 1
                            battle_log.append(f"[yellow]⚖️ TIE[/yellow] slot {slot}: {r}ms each")
                
                # Update per-metric scores based on current stats
                reth_stats_now = stats([p["duration_ms"] for p in history["Reth"]])
                neth_stats_now = stats([p["duration_ms"] for p in history["Nethermind"]])
                
                metric_map = [("avg", "avg"), ("median", "p50"), ("p95", "p95"), ("max", "max")]
                for metric_key, stat_key in metric_map:
                    r_val = reth_stats_now.get(stat_key, 0)
                    n_val = neth_stats_now.get(stat_key, 0)
                    r_prev = last_stats.get("Reth", {}).get(stat_key, r_val)
                    n_prev = last_stats.get("Nethermind", {}).get(stat_key, n_val)
                    
                    # Only score if we have new data
                    if r_val > 0 and n_val > 0 and (r_val != r_prev or n_val != n_prev):
                        if r_val < n_val:
                            metric_scores[metric_key]["reth"] += 1
                        elif n_val < r_val:
                            metric_scores[metric_key]["neth"] += 1
                
                last_stats["Reth"] = reth_stats_now
                last_stats["Nethermind"] = neth_stats_now
                
                # Trim seen_blocks
                if len(seen_blocks) > 2000:
                    seen_blocks.clear()
                
                last_fetch = now
            
            live.update(render(start_time))
            time.sleep(0.25)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        console.print("\n[bold]Battle ended![/bold]")
        console.print(f"Final score: 🦀 Reth {reth_wins} - {neth_wins} Nethermind 🔷 (Ties: {ties})")
        console.print("\n[bold]Per-metric scores:[/bold]")
        for m, scores in metric_scores.items():
            console.print(f"  {m.upper():8} 🦀 {scores['reth']:3} - {scores['neth']:3} 🔷")
