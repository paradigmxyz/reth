#!/bin/sh

set -e  # Exit on error

# ==============================================================================
# CONSTANTS AND DEFAULTS
# ==============================================================================
readonly DEFAULT_GENESIS_TEMPLATE="/root/reth/genesis.template.json"
readonly DEFAULT_GENESIS_JSON="/root/reth/genesis.json"
readonly DEFAULT_JWT_FILE="/root/reth/jwt.hex"
readonly DEFAULT_BASE_FEE="0x1"
readonly DEFAULT_DATA_DIR="/root/reth/data"

# ==============================================================================
# FUNCTIONS
# ==============================================================================

# Print a separator line
print_separator() {
    echo "========================================="
}

# Print an error message and exit
fatal_error() {
    local message="$1"
    local exit_code="${2:-1}"
    echo "FATAL: $message" >&2
    exit "$exit_code"
}

# Display all environment variables
display_env_vars() {
    print_separator
    echo "ENVIRONMENT VARIABLES:"
    print_separator
    echo "Required variables:"
    echo "  CHAIN_ID: ${CHAIN_ID}"
    echo "  ONE_TIME_ADDRESS: ${ONE_TIME_ADDRESS}"
    echo "  IGRA_LAUNCH_DAA_SCORE: ${IGRA_LAUNCH_DAA_SCORE}"
    echo "  L1_REFERENCE_DAA_SCORE: ${L1_REFERENCE_DAA_SCORE}"
    echo "  L1_REFERENCE_TIMESTAMP: ${L1_REFERENCE_TIMESTAMP}"
    echo ""
    echo "Optional variables:"
    echo "  GENESIS_TEMPLATE_JSON: ${GENESIS_TEMPLATE_JSON:-$DEFAULT_GENESIS_TEMPLATE}"
    echo "  GENESIS_JSON: ${GENESIS_JSON:-$DEFAULT_GENESIS_JSON}"
    echo "  JWT_FILE: ${JWT_FILE:-$DEFAULT_JWT_FILE}"
    echo "  BASE_FEE_PER_GAS: ${BASE_FEE_PER_GAS:-$DEFAULT_BASE_FEE}"
    echo "  DATA_DIR: ${DATA_DIR:-$DEFAULT_DATA_DIR}"
    print_separator
    echo ""
}

# Validate required environment variables
validate_required_vars() {
    local var_name
    local var_value

    for var_name in CHAIN_ID ONE_TIME_ADDRESS IGRA_LAUNCH_DAA_SCORE L1_REFERENCE_DAA_SCORE L1_REFERENCE_TIMESTAMP; do
        eval "var_value=\${$var_name}"
        if [ -z "$var_value" ]; then
            fatal_error "$var_name environment variable is not set" 4
        fi
    done
}

# Validate file exists
validate_file_exists() {
    local file_path="$1"
    local file_description="$2"
    local exit_code="${3:-1}"

    if [ ! -f "$file_path" ]; then
        fatal_error "$file_description not found at $file_path" "$exit_code"
    fi
}

# Calculate genesis timestamp
calculate_genesis_timestamp() {
    GENESIS_TIMESTAMP=$(((IGRA_LAUNCH_DAA_SCORE/10) - (L1_REFERENCE_DAA_SCORE/10) + L1_REFERENCE_TIMESTAMP - 1))
    export GENESIS_TIMESTAMP

    echo "Calculated values:"
    echo "  GENESIS_TIMESTAMP: ${GENESIS_TIMESTAMP} (calculated as (${IGRA_LAUNCH_DAA_SCORE}/10) - (${L1_REFERENCE_DAA_SCORE}/10) + ${L1_REFERENCE_TIMESTAMP} - 1)"
    echo ""
}

# Replace environment variables in template (pure shell implementation)
replace_env_vars() {
    local input_file="$1"
    local output_file="$2"

    # Read the template and replace variables
    while IFS= read -r line || [ -n "$line" ]; do
        # Replace each variable pattern ${VAR} with its value
        line=$(echo "$line" | sed \
            -e "s/\${CHAIN_ID}/${CHAIN_ID}/g" \
            -e "s/\${GENESIS_TIMESTAMP}/${GENESIS_TIMESTAMP}/g" \
            -e "s/\${ONE_TIME_ADDRESS}/${ONE_TIME_ADDRESS}/g" \
            -e "s/\${BASE_FEE_PER_GAS}/${BASE_FEE_PER_GAS}/g")
        echo "$line"
    done < "$input_file" > "$output_file"
}

# Generate genesis.json from template
generate_genesis_json() {
    local template="$1"
    local output="$2"
    local temp_file="${output}.tmp"

    echo "Generating genesis.json with the following configuration:"
    echo "  Chain ID: ${CHAIN_ID}"
    echo "  Genesis Timestamp: ${GENESIS_TIMESTAMP}"
    echo "  One-time Address: ${ONE_TIME_ADDRESS}"
    echo "  Base Fee Per Gas: ${BASE_FEE_PER_GAS}"
    print_separator

    # Generate from template using shell substitution
    replace_env_vars "$template" "$temp_file" || {
        fatal_error "Failed to generate genesis.json from template" 5
    }

    # Fix chainId to be a number instead of string
    sed 's/"chainId": "\([0-9]*\)"/"chainId": \1/' "$temp_file" > "$output" || {
        rm -f "$temp_file"
        fatal_error "Failed to fix chainId in genesis.json" 5
    }

    rm -f "$temp_file"
    echo "Successfully generated genesis.json"
    echo ""
}

# Display generated genesis.json content
display_genesis_content() {
    local genesis_file="$1"

    echo "Generated genesis.json content:"
    print_separator
    cat "$genesis_file"
    echo ""
    print_separator
    echo ""
}

# Extract value from genesis.json
extract_genesis_value() {
    local genesis_file="$1"
    local key="$2"

    grep -o "\"$key\": *\"[^\"]*\"" "$genesis_file" |
    awk -F':' '{gsub(/"| /, "", $2); print $2}'
}

# Extract genesis parameters
extract_genesis_params() {
    local genesis_file="$1"

    GENESIS_gasLimit=$(extract_genesis_value "$genesis_file" "gasLimit")

    # Convert gasLimit from hex to decimal if provided
    if [ -n "${GENESIS_gasLimit}" ]; then
        GENESIS_gasLimit=$(printf "%d\n" "${GENESIS_gasLimit}")
    fi

    echo "Extracted from genesis.json:"
    echo "  GENESIS_gasLimit: ${GENESIS_gasLimit}"
    print_separator
    echo ""
}

# Display reth node parameters
display_reth_params() {
    echo "Starting reth node with parameters:"
    print_separator
    echo "  Chain: ${genesis_json}"
    echo "  JWT Secret: ${jwt_file}"
    echo "  HTTP API: all"
    echo "  HTTP CORS: *"
    echo "  HTTP Address: 0.0.0.0"
    echo "  Auth RPC Address: 0.0.0.0"
    echo "  WebSocket API: all"
    echo "  Data Directory: ${data_dir}"
    echo "  Builder Extra Data: 'IGRA-NETWORK'"
    echo "  Builder Gas Limit: ${GENESIS_gasLimit}"
    echo "  RPC TX Fee Cap: 0"
    echo "  Logging: RUST_LOG=info,sync::stages=trace,downloaders=trace,debug"
    print_separator
    echo ""
}

# Start reth node
start_reth_node() {
    mkdir -p /root/reth/socket
    RUST_LOG=info,sync::stages=trace,downloaders=trace,debug,txpool=debug,engine::stream::reorg=debug,consensus::engine=debug \
    reth node \
        --chain "${genesis_json}" \
        --authrpc.jwtsecret "${jwt_file}" \
        --http \
        --http.api all \
        --http.corsdomain="*" \
        --http.addr 0.0.0.0 \
        --authrpc.addr 0.0.0.0 \
        --auth-ipc \
        --auth-ipc.path /root/reth/socket/auth.ipc \
        --ws \
        --ws.api all \
        --datadir "${data_dir}" \
        --trusted-only \
        --no-persist-peers \
        --disable-discovery \
        --builder.extradata "IGRA-NETWORK" \
        --builder.gaslimit "${GENESIS_gasLimit}" \
        --rpc.txfeecap 0 \
        --db.exclusive true \
        --engine.always-process-payload-attributes-on-canonical-head \
        --metrics 0.0.0.0:9001 \
        --engine.persistence-threshold 200 \
        --engine.memory-block-buffer-target 100 \
        --engine.state-root-fallback \
        --txpool.discard-reorged-transactions \
        --txpool.fifo-ordering \
        --txpool.clear-after-fcu \
        --engine.allow-unwind-canonical-header \
        -vv
}

# ==============================================================================
# MAIN EXECUTION
# ==============================================================================

# Display environment variables
display_env_vars

# Set file paths with defaults
genesis_template="${GENESIS_TEMPLATE_JSON:-$DEFAULT_GENESIS_TEMPLATE}"
genesis_json="${GENESIS_JSON:-$DEFAULT_GENESIS_JSON}"
jwt_file="${JWT_FILE:-$DEFAULT_JWT_FILE}"
data_dir="${DATA_DIR:-$DEFAULT_DATA_DIR}"

# Set and export BASE_FEE_PER_GAS with default
BASE_FEE_PER_GAS="${BASE_FEE_PER_GAS:-$DEFAULT_BASE_FEE}"
export BASE_FEE_PER_GAS

# Validate environment and files
validate_required_vars
validate_file_exists "$genesis_template" "genesis.template.json" 1

# Calculate genesis timestamp
calculate_genesis_timestamp

# Export variables for template substitution
export CHAIN_ID
export ONE_TIME_ADDRESS

# Generate genesis.json
generate_genesis_json "$genesis_template" "$genesis_json"

# Verify and display genesis.json
validate_file_exists "$genesis_json" "genesis.json" 1
display_genesis_content "$genesis_json"

# Extract genesis parameters
extract_genesis_params "$genesis_json"

# Validate JWT file
validate_file_exists "$jwt_file" "jwt.hex" 2

# Display reth parameters and start node
display_reth_params
start_reth_node