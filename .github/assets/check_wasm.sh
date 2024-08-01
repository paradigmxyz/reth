#!/usr/bin/env bash
set +e  # Disable immediate exit on error

# Array of crates 
no_std_packages=(
  # The following were confirmed not working in the past, but could be enabled if issues have been resolved
  reth-db
  reth-primitives
  reth-revm
  reth-evm
  reth-evm-ethereum
  reth-consensus
  # The following are confirmed working
  reth-errors
  reth-ethereum-forks
  reth-network-peers
  reth-primitives-traits
  reth-codecs
)

# Dictionary to hold the results
declare -A results
# Flag to track if any command fails
any_failed=0

for package in "${no_std_packages[@]}"; do
  cmd="cargo +stable build -p $package --target wasm32-wasip1 --no-default-features"

  if [ -n "$CI" ]; then
    echo "::group::$cmd"
  else
    printf "\n%s:\n  %s\n" "$package" "$cmd"
  fi

  # Run the command and capture the return code
  $cmd
  ret_code=$?

  # Store the result in the dictionary
  if [ $ret_code -eq 0 ]; then
    results["$package"]="✅"
  else
    results["$package"]="❌"
    any_failed=1
  fi

  if [ -n "$CI" ]; then
    echo "::endgroup::"
  fi
done

# Print summary
echo -e "\nSummary of build results:"
for package in "${!results[@]}"; do
  echo "$package: ${results[$package]}"
done

# Exit with a non-zero status if any command fails
exit $any_failed
