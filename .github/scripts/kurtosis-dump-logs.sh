#!/usr/bin/env bash

# 
# 
# This is a log script used mostly in the CI pipeline
# 
# It will search for all OP clients in a specified kurtosis enclave
# and will dump their logs in a CI-friendly manner
# 
# Usage:
# 
# ./kurtosis-dump-logs.sh <ENCLAVE_NAME>
# 
# 

# Just so that we don't repeat ourselves
ENCLAVES_API_URL=http://127.0.0.1:9779/api/enclaves

# We pass the enclave name and the target block as script arguments
ENCLAVE_NAME=$1
if [ -z "$ENCLAVE_NAME" ]; then
    echo "Please specify the enclave name as the first parameter"
    exit 1
fi

echo "Dumping logs of the OP services for enclave $ENCLAVE_NAME"

# Get the enclave UUID from the API
# 
# The | values at the end of the j1 transformation will convert null to an empty string
ENCLAVE_ID=$(curl -s $ENCLAVES_API_URL | jq -r 'to_entries | map(select(.value.name == "'$ENCLAVE_NAME'")) | .[0].key | values')

# Make sure we got something
if [ -z "$ENCLAVE_ID" ]; then
    echo "No enclave found for enclave $ENCLAVE_NAME"
    exit 1
fi

echo "Got enclave UUID: $ENCLAVE_ID"

# Now get all the service names
ENCLAVE_SERVICES=$(curl -s $ENCLAVES_API_URL/$ENCLAVE_ID/services)
ENCLAVE_SERVICES_NAMES=$(echo $ENCLAVE_SERVICES | jq -r 'keys_unsorted | join(" ")')

# Make sure we got something
if [ -z "$ENCLAVE_SERVICES_NAMES" ]; then
    echo "No clients found for enclave $ENCLAVE_NAME"
    exit 0
fi

echo "Got enclave services: $ENCLAVE_SERVICES_NAMES"

# Convert the list into a bash array
ENCLAVE_SERVICE_NAMES_ARRAY=($ENCLAVE_SERVICES_NAMES)

# Iterate over the names/RPC ports arrays
for SERVICE_NAME in "${ENCLAVE_SERVICE_NAMES_ARRAY[@]}"; do
    # We use the github actions grouping to get a nicer output
    echo "::group::$SERVICE_NAME"
    kurtosis service logs -a $ENCLAVE_NAME $SERVICE_NAME
    echo "::endgroup::"
done