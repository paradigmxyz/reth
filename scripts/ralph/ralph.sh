#!/bin/bash
# Ralph - Autonomous Benchmark Execution Loop
# Runs Amp repeatedly to complete benchmark tasks from prd.json

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PRD_FILE="$SCRIPT_DIR/prd.json"
PROGRESS_FILE="$SCRIPT_DIR/progress.txt"
PROMPT_FILE="$SCRIPT_DIR/prompt.md"
LOCK_FILE="$SCRIPT_DIR/.ralph.lock"
MAX_ITERATIONS="${1:-15}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

timestamp() {
    date '+%H:%M:%S'
}

log_info() {
    echo -e "${BLUE}[$(timestamp) INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[$(timestamp) SUCCESS]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[$(timestamp) WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[$(timestamp) ERROR]${NC} $1"
}

log_debug() {
    echo -e "[$(timestamp) DEBUG] $1"
}

# Check prerequisites
if [[ ! -f "$PRD_FILE" ]]; then
    log_error "prd.json not found at $PRD_FILE"
    exit 1
fi

if [[ ! -f "$PROMPT_FILE" ]]; then
    log_error "prompt.md not found at $PROMPT_FILE"
    exit 1
fi

# Initialize progress file if needed
if [[ ! -f "$PROGRESS_FILE" ]]; then
    cat > "$PROGRESS_FILE" << 'EOF'
# RocksDB vs MDBX Benchmark Progress

This file tracks progress across Ralph iterations.

---

EOF
    log_info "Created progress.txt"
fi

# Check if amp is available
if ! command -v amp &> /dev/null; then
    log_error "amp CLI not found. Please install it first."
    exit 1
fi

# Concurrency protection with lockfile
if [[ -f "$LOCK_FILE" ]]; then
    existing_pid=$(cat "$LOCK_FILE" 2>/dev/null)
    if [[ -n "$existing_pid" ]] && kill -0 "$existing_pid" 2>/dev/null; then
        log_error "Another instance is already running (PID: $existing_pid)"
        exit 1
    else
        log_warn "Stale lockfile found, removing..."
        rm -f "$LOCK_FILE"
    fi
fi
echo $$ > "$LOCK_FILE"
trap 'rm -f "$LOCK_FILE"' EXIT

# Change to repo root for correct relative paths in commands
cd "$REPO_ROOT"

# Create benchmark results directory
mkdir -p "$REPO_ROOT/benchmark-results"

log_info "Starting Ralph benchmark execution loop"
log_info "Max iterations: $MAX_ITERATIONS"
log_info "PRD file: $PRD_FILE"
log_info "Repo root: $REPO_ROOT"
log_debug "Amp location: $(which amp)"
log_debug "Amp version: $(amp --version 2>/dev/null || echo 'unknown')"
echo ""

# Show current task status
show_status() {
    echo ""
    log_info "Current task status:"
    jq -r '.userStories[] | "  \(.id): \(.title) [passes: \(.passes)]"' "$PRD_FILE"
    echo ""
}

show_status

for iteration in $(seq 1 $MAX_ITERATIONS); do
    log_info "=== Iteration $iteration of $MAX_ITERATIONS ==="
    
    # Check if all tasks are complete
    incomplete=$(jq '[.userStories[] | select(.passes == false)] | length' "$PRD_FILE")
    if [[ "$incomplete" -eq 0 ]]; then
        log_success "All tasks complete!"
        exit 0
    fi
    
    # Find next task
    next_task=$(jq -r '.userStories | map(select(.passes == false)) | sort_by(.priority) | .[0] | "\(.id): \(.title)"' "$PRD_FILE")
    log_info "Next task: $next_task"
    
    # Run amp with the prompt
    log_info "Spawning Amp instance..."
    log_debug "Working directory: $(pwd)"
    log_debug "Prompt file: $PROMPT_FILE ($(wc -l < "$PROMPT_FILE") lines)"

    # Create temp file for amp output
    amp_output_file=$(mktemp)
    amp_start_time=$(date +%s)

    # Run amp in background so we can show progress
    cat "$PROMPT_FILE" | amp --dangerously-allow-all > "$amp_output_file" 2>&1 &
    amp_pid=$!
    log_debug "Amp started with PID: $amp_pid"

    # Monitor amp while it runs
    spin_chars='⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'
    spin_idx=0
    while kill -0 "$amp_pid" 2>/dev/null; do
        elapsed=$(($(date +%s) - amp_start_time))
        output_lines=$(wc -l < "$amp_output_file" 2>/dev/null || echo "0")
        spin_char="${spin_chars:$spin_idx:1}"
        printf "\r${BLUE}[$(timestamp)]${NC} %s Amp running... %dm%ds elapsed, %s lines output" "$spin_char" $((elapsed/60)) $((elapsed%60)) "$output_lines"
        spin_idx=$(( (spin_idx + 1) % ${#spin_chars} ))
        sleep 0.5
    done
    printf "\n"

    # Get exit code
    wait "$amp_pid"
    amp_exit_code=$?
    amp_end_time=$(date +%s)
    amp_duration=$((amp_end_time - amp_start_time))

    # Read output
    output=$(cat "$amp_output_file")
    rm -f "$amp_output_file"

    log_info "Amp finished in ${amp_duration}s with exit code $amp_exit_code"
    log_debug "Output size: $(echo "$output" | wc -c) bytes, $(echo "$output" | wc -l) lines"

    # Log if amp failed
    if [[ "$amp_exit_code" -ne 0 ]]; then
        log_error "Amp exited with code $amp_exit_code"
        echo "--- Last 50 lines of output ---"
        echo "$output" | tail -50
        echo "--- End of output ---"
        log_warn "Continuing to next iteration despite error..."
    fi

    # Strip ANSI escape codes for reliable pattern matching
    clean_output=$(echo "$output" | sed 's/\x1b\[[0-9;]*m//g' | sed 's/\x1b\[[0-9;]*[a-zA-Z]//g')

    # Check for completion signal
    if echo "$clean_output" | grep -q "<promise>COMPLETE</promise>"; then
        log_success "All benchmark tasks completed!"
        echo ""
        log_info "Final status:"
        show_status
        exit 0
    fi

    # Show abbreviated output
    echo ""
    log_info "Last 20 lines of amp output:"
    echo "$output" | tail -20

    # Check what changed in prd.json
    echo ""
    new_incomplete=$(jq '[.userStories[] | select(.passes == false)] | length' "$PRD_FILE")
    completed_this_iteration=$((incomplete - new_incomplete))
    if [[ "$completed_this_iteration" -gt 0 ]]; then
        log_success "Completed $completed_this_iteration task(s) this iteration!"
        show_status
    else
        log_warn "No tasks were marked complete this iteration"
        # Show notes for current task to see if there's progress info
        current_notes=$(jq -r '.userStories | map(select(.passes == false)) | sort_by(.priority) | .[0] | .notes // ""' "$PRD_FILE")
        if [[ -n "$current_notes" ]]; then
            log_info "Current task notes: $current_notes"
        fi
    fi

    # Wait before next iteration
    log_info "Waiting 2 seconds before next iteration..."
    sleep 2
done

log_warn "Reached maximum iterations ($MAX_ITERATIONS) without completing all tasks"
show_status
exit 1
