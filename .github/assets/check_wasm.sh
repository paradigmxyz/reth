#!/usr/bin/env bash
set +e  # Disable immediate exit on error

# Array of crates 
wasm_crates=(
  # The following were confirmed not working in the past, but could be enabled if issues have been resolved
  # reth-consensus
  # reth-db
  # reth-evm
  # reth-evm-ethereum
  # The following are confirmed working
  reth-codecs
  reth-errors
  reth-ethereum-forks
  reth-network-peers
  reth-primitives
  reth-primitives-traits
  reth-revm
)

# Array to hold the results
results=()
# Flag to track if any command fails
any_failed=0

for crate in "${wasm_crates[@]}"; do
  # TODO: use `wasm32-wasip1` once https://github.com/gyscos/zstd-rs/commit/fc3a73100b6d3f2ff6dd3da0ff14dd9185ba02f0 is released
  cmd="cargo +stable build -p $crate --target wasm32-wasi --no-default-features"

  if [ -n "$CI" ]; then
    echo "::group::$cmd"
  else
    printf "\n%s:\n  %s\n" "$crate" "$cmd"
  fi

  # Run the command and capture the return code
  $cmd
  ret_code=$?

  # Store the result in the dictionary
  if [ $ret_code -eq 0 ]; then
    results+=("✅:$crate")
  else
    results+=("❌:$crate")
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
  status="${result%%:*}"
  crate="${result##*:}"
  echo "$status $crate"
done

# Exit with a non-zero status if any command fails
exit $any_failed
