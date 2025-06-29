#!/bin/bash
# Hoodi benchmark for PGO - sends execution payloads via engine API

set -e

# Configuration
ENGINE_URL="${1:-http://localhost:8551}"
JWT_SECRET="${2:-}"
PAYLOADS_DIR="${3:-./scripts/hoodi-exec-payloads}"

# Usage
if [ $# -lt 2 ]; then
    echo "Usage: $0 <engine_url> <jwt_secret_file> [payloads_dir]"
    echo "Example: $0 http://localhost:8551 /tmp/hoodi-pgo/jwt.hex"
    exit 1
fi

# Check dependencies
command -v jq >/dev/null 2>&1 || { echo "Error: jq not found"; exit 1; }
command -v curl >/dev/null 2>&1 || { echo "Error: curl not found"; exit 1; }

# Read JWT secret
if [ ! -f "$JWT_SECRET" ]; then
    echo "Error: JWT secret file not found: $JWT_SECRET"
    exit 1
fi
JWT_SECRET_HEX=$(cat "$JWT_SECRET" | tr -d '\n' | tr -d ' ')

# Create JWT token for Engine API authentication
# The Engine API uses a simplified JWT with HS256 algorithm
create_jwt_token() {
    local secret_hex="$1"
    
    # JWT header: {"alg":"HS256","typ":"JWT"}
    local header='{"alg":"HS256","typ":"JWT"}'
    
    # JWT payload with current timestamp
    local iat=$(date +%s)
    local payload="{\"iat\":$iat}"
    
    # Base64url encode (no padding, URL-safe)
    base64url() {
        openssl base64 -e -A | tr '+/' '-_' | tr -d '='
    }
    
    # Encode header and payload
    local header_b64=$(echo -n "$header" | base64url)
    local payload_b64=$(echo -n "$payload" | base64url)
    
    # Create signature: HMAC-SHA256(base64url(header).base64url(payload), secret)
    local data_to_sign="$header_b64.$payload_b64"
    
    # Convert hex secret to binary for HMAC
    local signature=$(echo -n "$data_to_sign" | openssl dgst -sha256 -hex -mac HMAC -macopt hexkey:$secret_hex | cut -d' ' -f2)
    # Convert hex signature to binary and then base64url
    local signature_b64=$(echo -n "$signature" | xxd -r -p | base64url)
    
    # Combine to create JWT
    echo "$header_b64.$payload_b64.$signature_b64"
}

# Check if required tools are available
if ! command -v openssl >/dev/null 2>&1; then
    echo "Error: openssl not found. Cannot create JWT token."
    exit 1
fi

if ! command -v xxd >/dev/null 2>&1; then
    echo "Error: xxd not found. Cannot create JWT token."
    exit 1
fi

# Create JWT token
JWT_TOKEN=$(create_jwt_token "$JWT_SECRET_HEX")

# Process blocks 1-3
for i in 1 2 3; do
    PAYLOAD_FILE="$PAYLOADS_DIR/hoodi-exec-payload-$i.json"
    
    if [ ! -f "$PAYLOAD_FILE" ]; then
        echo "Error: Payload file not found: $PAYLOAD_FILE"
        exit 1
    fi
    
    echo "Processing block $i..."
    
    # Read the entire params array from the file
    PARAMS=$(cat "$PAYLOAD_FILE")
    
    # Extract the payload (first element) and other params
    PAYLOAD=$(echo "$PARAMS" | jq '.[0]')
    VERSIONED_HASHES=$(echo "$PARAMS" | jq '.[1] // []')
    PARENT_BEACON_ROOT=$(echo "$PARAMS" | jq -r '.[2] // "0x0000000000000000000000000000000000000000000000000000000000000000"')
    
    # Extract block hash for FCU
    BLOCK_HASH=$(echo "$PAYLOAD" | jq -r '.blockHash')
    PARENT_HASH=$(echo "$PAYLOAD" | jq -r '.parentHash')
    
    # Send newPayload
    echo "Calling engine_newPayloadV3..."
    START_TIME=$(date +%s%N)
    RESPONSE=$(curl -s -X POST "$ENGINE_URL" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer $JWT_TOKEN" \
        -d "{\"jsonrpc\":\"2.0\",\"method\":\"engine_newPayloadV3\",\"params\":$PARAMS,\"id\":1}")
    NEW_PAYLOAD_TIME=$(( ($(date +%s%N) - START_TIME) / 1000000 ))
    
    # Check response
    STATUS=$(echo "$RESPONSE" | jq -r '.result.status // .error.message // "unknown"')
    if [ "$STATUS" != "VALID" ] && [ "$STATUS" != "ACCEPTED" ]; then
        echo "Error: newPayload failed with status: $STATUS"
        echo "Response: $RESPONSE"
        exit 1
    fi
    
    # Send forkchoiceUpdated
    echo "Calling engine_forkchoiceUpdatedV3..."
    START_TIME=$(date +%s%N)
    RESPONSE=$(curl -s -X POST "$ENGINE_URL" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer $JWT_TOKEN" \
        -d "{\"jsonrpc\":\"2.0\",\"method\":\"engine_forkchoiceUpdatedV3\",\"params\":[{\"headBlockHash\":\"$BLOCK_HASH\",\"safeBlockHash\":\"$PARENT_HASH\",\"finalizedBlockHash\":\"$PARENT_HASH\"},null],\"id\":2}")
    FCU_TIME=$(( ($(date +%s%N) - START_TIME) / 1000000 ))
    
    # Check response
    FCU_STATUS=$(echo "$RESPONSE" | jq -r '.result.payloadStatus.status // .error.message // "unknown"')
    if [ "$FCU_STATUS" != "VALID" ] && [ "$FCU_STATUS" != "SYNCING" ]; then
        echo "Error: forkchoiceUpdated failed with status: $FCU_STATUS"
        echo "Response: $RESPONSE"
        exit 1
    fi
    
    TOTAL_TIME=$((NEW_PAYLOAD_TIME + FCU_TIME))
    echo "Block $i complete: newPayload ${NEW_PAYLOAD_TIME}ms, forkchoiceUpdated ${FCU_TIME}ms, total ${TOTAL_TIME}ms"
done

echo "Benchmark complete!"