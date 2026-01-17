#!/bin/bash
# Real-time Reth vs Nethermind payload comparison TUI
# Run: ./scripts/payload-tui.sh

API="https://lab.ethpandaops.io/api/v1/mainnet/int_engine_new_payload"
INTERVAL=3

# Colors
CYAN='\033[0;36m'
MAGENTA='\033[0;35m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
BOLD='\033[1m'
DIM='\033[2m'
RESET='\033[0m'

color_duration() {
    local ms=$1
    if (( ms < 50 )); then echo -e "${GREEN}${ms}ms${RESET}"
    elif (( ms < 150 )); then echo -e "${YELLOW}${ms}ms${RESET}"
    else echo -e "${RED}${ms}ms${RESET}"
    fi
}

fetch_stats() {
    local client=$1
    local since=$(($(date +%s) - 3600))  # last hour
    curl -s "${API}?slot_start_date_time_gte=${since}&meta_execution_implementation_eq=${client}&page_size=100&order_by=slot%20desc" 2>/dev/null
}

fetch_recent() {
    local client=$1
    local since=$(($(date +%s) - 300))  # last 5 min
    curl -s "${API}?slot_start_date_time_gte=${since}&meta_execution_implementation_eq=${client}&page_size=10&order_by=slot%20desc" 2>/dev/null
}

calc_stats() {
    local json=$1
    echo "$json" | jq -r '
        .int_engine_new_payload | 
        if length == 0 then "0 0 0 0 0 0"
        else
            [.[].duration_ms] | 
            sort |
            "\(length) \(add/length | floor) \(.[length/2 | floor]) \(.[((length * 0.95) | floor)]) \(min) \(max)"
        end
    '
}

print_recent() {
    local json=$1
    local color=$2
    echo "$json" | jq -r '.int_engine_new_payload[:8][] | "\(.slot) \(.block_number) \(.duration_ms) \(.gas_used)"' | \
    while read slot block dur gas; do
        gas_m=$(echo "scale=1; $gas / 1000000" | bc 2>/dev/null || echo "0")
        dur_colored=$(color_duration $dur)
        printf "${color}%10s %10s${RESET} %8s %6sM\n" "$slot" "$block" "$dur_colored" "$gas_m"
    done
}

draw_bar() {
    local val=$1
    local max=$2
    local width=30
    local filled=$(echo "$val * $width / $max" | bc 2>/dev/null || echo 0)
    filled=${filled:-0}
    if (( filled > width )); then filled=$width; fi
    if (( filled < 0 )); then filled=0; fi
    printf '%*s' "$filled" '' | tr ' ' '█'
    printf '%*s' "$((width - filled))" '' | tr ' ' '░'
}

while true; do
    clear
    
    # Fetch data
    reth_data=$(fetch_stats "Reth")
    neth_data=$(fetch_stats "Nethermind")
    reth_recent=$(fetch_recent "Reth")
    neth_recent=$(fetch_recent "Nethermind")
    
    # Parse stats
    read reth_cnt reth_avg reth_p50 reth_p95 reth_min reth_max <<< $(calc_stats "$reth_data")
    read neth_cnt neth_avg neth_p50 neth_p95 neth_min neth_max <<< $(calc_stats "$neth_data")
    
    # Header
    echo -e "${BOLD}════════════════════════════════════════════════════════════════════════════${RESET}"
    echo -e "${BOLD}              ⚡ RETH vs NETHERMIND - Payload Execution Monitor${RESET}"
    echo -e "${DIM}              Updated: $(date '+%H:%M:%S') | Polling every ${INTERVAL}s | Last hour stats${RESET}"
    echo -e "${BOLD}════════════════════════════════════════════════════════════════════════════${RESET}"
    echo
    
    # Stats table
    echo -e "${BOLD}STATISTICS          ${CYAN}Reth${RESET}           ${MAGENTA}Nethermind${RESET}       Winner${RESET}"
    echo -e "─────────────────────────────────────────────────────────────────"
    
    printf "Samples:            ${CYAN}%-14s${RESET} ${MAGENTA}%-14s${RESET}\n" "$reth_cnt" "$neth_cnt"
    
    # Average with winner
    if (( reth_avg < neth_avg )); then
        diff=$((neth_avg - reth_avg))
        pct=$(echo "scale=1; $diff * 100 / $neth_avg" | bc 2>/dev/null || echo "0")
        winner="${CYAN}Reth +${pct}%${RESET}"
    else
        diff=$((reth_avg - neth_avg))
        pct=$(echo "scale=1; $diff * 100 / $reth_avg" | bc 2>/dev/null || echo "0")
        winner="${MAGENTA}Neth +${pct}%${RESET}"
    fi
    printf "Average:            ${CYAN}%-14s${RESET} ${MAGENTA}%-14s${RESET} %b\n" "${reth_avg}ms" "${neth_avg}ms" "$winner"
    
    # P50 with winner
    if (( reth_p50 < neth_p50 )); then
        diff=$((neth_p50 - reth_p50))
        pct=$(echo "scale=1; $diff * 100 / $neth_p50" | bc 2>/dev/null || echo "0")
        winner="${CYAN}Reth +${pct}%${RESET}"
    else
        diff=$((reth_p50 - neth_p50))
        pct=$(echo "scale=1; $diff * 100 / $reth_p50" | bc 2>/dev/null || echo "0")
        winner="${MAGENTA}Neth +${pct}%${RESET}"
    fi
    printf "Median (P50):       ${CYAN}%-14s${RESET} ${MAGENTA}%-14s${RESET} %b\n" "${reth_p50}ms" "${neth_p50}ms" "$winner"
    
    # P95 with winner
    if (( reth_p95 < neth_p95 )); then
        diff=$((neth_p95 - reth_p95))
        pct=$(echo "scale=1; $diff * 100 / $neth_p95" | bc 2>/dev/null || echo "0")
        winner="${CYAN}Reth +${pct}%${RESET}"
    else
        diff=$((reth_p95 - neth_p95))
        pct=$(echo "scale=1; $diff * 100 / $reth_p95" | bc 2>/dev/null || echo "0")
        winner="${MAGENTA}Neth +${pct}%${RESET}"
    fi
    printf "P95:                ${CYAN}%-14s${RESET} ${MAGENTA}%-14s${RESET} %b\n" "${reth_p95}ms" "${neth_p95}ms" "$winner"
    
    printf "Min:                ${CYAN}%-14s${RESET} ${MAGENTA}%-14s${RESET}\n" "${reth_min}ms" "${neth_min}ms"
    printf "Max:                ${CYAN}%-14s${RESET} ${MAGENTA}%-14s${RESET}\n" "${reth_max}ms" "${neth_max}ms"
    
    echo
    
    # Visual bars
    echo -e "${BOLD}LATENCY COMPARISON${RESET}"
    echo -e "─────────────────────────────────────────────────────────────────"
    max_val=$((reth_avg > neth_avg ? reth_avg : neth_avg))
    max_val=$((max_val + 20))
    printf "${CYAN}Reth Avg:  ${RESET}"
    draw_bar $reth_avg $max_val
    echo -e " ${reth_avg}ms"
    printf "${MAGENTA}Neth Avg:  ${RESET}"
    draw_bar $neth_avg $max_val
    echo -e " ${neth_avg}ms"
    
    echo
    
    # Recent payloads side by side
    echo -e "${BOLD}RECENT PAYLOADS${RESET}"
    echo -e "${CYAN}Reth                                    ${MAGENTA}Nethermind${RESET}"
    echo -e "Slot       Block      Duration    Gas     Slot       Block      Duration    Gas"
    echo -e "─────────────────────────────────────────────────────────────────────────────────"
    
    # Create temp files for side-by-side
    reth_lines=$(echo "$reth_recent" | jq -r '.int_engine_new_payload[:8][] | "\(.slot) \(.block_number) \(.duration_ms) \(.gas_used)"' 2>/dev/null)
    neth_lines=$(echo "$neth_recent" | jq -r '.int_engine_new_payload[:8][] | "\(.slot) \(.block_number) \(.duration_ms) \(.gas_used)"' 2>/dev/null)
    
    paste <(echo "$reth_lines") <(echo "$neth_lines") | while IFS=$'\t' read reth_line neth_line; do
        if [[ -n "$reth_line" ]]; then
            read slot block dur gas <<< "$reth_line"
            gas_m=$(echo "scale=1; $gas / 1000000" | bc 2>/dev/null || echo "0")
            dur_col=$(color_duration $dur)
            printf "${CYAN}%10s %10s${RESET} %8s %6sM  " "$slot" "$block" "$dur_col" "$gas_m"
        else
            printf "%40s  " ""
        fi
        
        if [[ -n "$neth_line" ]]; then
            read slot block dur gas <<< "$neth_line"
            gas_m=$(echo "scale=1; $gas / 1000000" | bc 2>/dev/null || echo "0")
            dur_col=$(color_duration $dur)
            printf "${MAGENTA}%10s %10s${RESET} %8s %6sM" "$slot" "$block" "$dur_col" "$gas_m"
        fi
        echo
    done
    
    echo
    echo -e "${DIM}Press Ctrl+C to quit${RESET}"
    
    sleep $INTERVAL
done
