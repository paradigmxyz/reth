#!/usr/bin/env bash
set +e  # Disable immediate exit on error

# Array of crates to compile
crates=($(cargo metadata --format-version=1 --no-deps | jq -r '.packages[].name' | grep '^reth' | sort))
# Array of crates to exclude
exclude_crates=(
  # The following are not working yet, but known to be fixable
  reth-exex-types # https://github.com/paradigmxyz/reth/issues/9946
  # The following require investigation if they can be fixed
  reth-auto-seal-consensus
  reth-basic-payload-builder
  reth-beacon-consensus
  reth-bench
  reth-blockchain-tree
  reth-chain-state
  reth-cli
  reth-cli-commands
  reth-cli-runner
  reth-consensus-debug-client
  reth-db-common
  reth-discv4
  reth-discv5
  reth-dns-discovery
  reth-downloaders
  reth-e2e-test-utils
  reth-engine-primitives
  reth-engine-service
  reth-engine-tree
  reth-engine-util
  reth-eth-wire
  reth-ethereum-cli
  reth-ethereum-engine
  reth-ethereum-engine-primitives
  reth-ethereum-payload-builder
  reth-etl
  reth-evm-ethereum
  reth-execution-errors
  reth-exex
  reth-exex-test-utils
  reth-ipc
  reth-net-nat
  reth-network
  reth-node-api
  reth-node-types
  reth-node-builder
  reth-node-core
  reth-node-ethereum
  reth-node-events
  reth-node-metrics
  reth-optimism-cli
  reth-optimism-evm
  reth-optimism-node
  reth-optimism-payload-builder
  reth-optimism-rpc
  reth-payload-builder
  reth-payload-primitives
  reth-rpc
  reth-rpc-api
  reth-rpc-api-testing-util
  reth-rpc-builder
  reth-rpc-engine-api
  reth-rpc-eth-api
  reth-rpc-eth-types
  reth-rpc-layer
  reth-rpc-types
  reth-stages
  reth-storage-errors
  reth-engine-local
  # The following are not supposed to be working
  reth # all of the crates below
  reth-invalid-block-hooks # reth-provider
  reth-libmdbx # mdbx
  reth-mdbx-sys # mdbx
  reth-provider # tokio
  reth-prune # tokio
  reth-stages-api # reth-provider, reth-prune
  reth-static-file # tokio
  reth-transaction-pool # c-kzg
  reth-trie-parallel # tokio
)

# Array to hold the results
results=()
# Flag to track if any command fails
any_failed=0

# Function to check if a value exists in an array
contains() {
  local array="$1[@]"
  local seeking=$2
  local in=1
  for element in "${!array}"; do
    if [[ "$element" == "$seeking" ]]; then
      in=0
      break
    fi
  done
  return $in
}

for crate in "${crates[@]}"; do
  if contains exclude_crates "$crate"; then
    results+=("3:⏭️:$crate")
    continue
  fi

  cmd="cargo +stable build -p $crate --target wasm32-wasip1 --no-default-features"

  if [ -n "$CI" ]; then
    echo "::group::$cmd"
  else
    printf "\n%s:\n  %s\n" "$crate" "$cmd"
  fi

  set +e  # Disable immediate exit on error
  # Run the command and capture the return code
  $cmd
  ret_code=$?
  set -e  # Re-enable immediate exit on error

  # Store the result in the dictionary
  if [ $ret_code -eq 0 ]; then
    results+=("1:✅:$crate")
  else
    results+=("2:❌:$crate")
    any_failed=1
  fi

  if [ -n "$CI" ]; then
    echo "::endgroup::"
  fi
done

# Sort the results by status and then by crate name
IFS=$'\n' sorted_results=($(sort <<<"${results[*]}"))
unset IFS

# Print summary
echo -e "\nSummary of build results:"
for result in "${sorted_results[@]}"; do
  status="${result#*:}"
  status="${status%%:*}"
  crate="${result##*:}"
  echo "$status $crate"
done

# Exit with a non-zero status if any command fails
exit $any_failed
