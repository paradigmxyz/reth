#!/usr/bin/env bash
set -eo pipefail

# List of no_std packages
no_std_packages=(
    reth-db
    reth-network-peers
)

# Loop through each package and check it for no_std compliance
for package in "${no_std_packages[@]}"; do
  cmd="cargo +stable check -p $package --no-default-features"
  
  if [ -n "$CI" ]; then
    echo "::group::$cmd"
  else
    printf "\n%s:\n  %s\n" "$package" "$cmd"
  fi

  $cmd

  if [ -n "$CI" ]; then
    echo "::endgroup::"
  fi
done
