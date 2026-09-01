#!/usr/bin/env bash
# Updates interconnected dependency repositories in topological order.
set -euo pipefail

readonly NIGHTLY_BRANCH="nightly"
readonly WORK_ROOT="${RUNNER_TEMP:-/tmp}/reth-nightly-dependencies"
readonly MAX_REPAIR_ATTEMPTS=5

declare -A REPO_DIRS
declare -A BUILD_COMMANDS

REPO_DIRS[revm]="$WORK_ROOT/revm"
REPO_DIRS[alloy-evm]="$WORK_ROOT/alloy-evm"
REPO_DIRS[reth-core]="$WORK_ROOT/reth-core"
REPO_DIRS[revm-inspectors]="$WORK_ROOT/revm-inspectors"
REPO_DIRS[revmc]="$WORK_ROOT/revmc"
REPO_DIRS[reth]="$WORK_ROOT/reth"

BUILD_COMMANDS[revm]='cargo check --workspace --all-targets --all-features'
BUILD_COMMANDS[alloy-evm]='cargo check --workspace --all-targets --all-features'
BUILD_COMMANDS[reth-core]='cargo check --workspace --all-targets --all-features'
BUILD_COMMANDS[revm-inspectors]='cargo check --workspace --all-targets --all-features'
BUILD_COMMANDS[revmc]='cargo check --workspace --all-targets --features llvm'
BUILD_COMMANDS[reth]='cargo check --workspace --all-targets --all-features'

require_environment() {
  : "${GH_TOKEN:?GH_TOKEN is required}"
  : "${ANTHROPIC_API_KEY:?ANTHROPIC_API_KEY is required}"
  command -v claude >/dev/null
  command -v gh >/dev/null
  command -v jq >/dev/null
}

configure_git() {
  local user_json name id login
  user_json=$(gh api /user)
  name=$(jq -r '.name // .login' <<< "$user_json")
  id=$(jq -r '.id' <<< "$user_json")
  login=$(jq -r '.login' <<< "$user_json")
  git config --global user.name "$name"
  git config --global user.email "${id}+${login}@users.noreply.github.com"
  gh auth setup-git
}

clone_repo() {
  local name=$1 repo=$2
  local dir=${REPO_DIRS[$name]}
  rm -rf "$dir"
  gh repo clone "$repo" "$dir"
  git -C "$dir" fetch origin main "+refs/heads/$NIGHTLY_BRANCH:refs/remotes/origin/$NIGHTLY_BRANCH" || true
}

rebase_in_progress() {
  local dir=$1
  [ -d "$dir/.git/rebase-merge" ] || [ -d "$dir/.git/rebase-apply" ]
}

run_claude() {
  local dir=$1 prompt=$2
  (
    cd "$dir"
    env -u GH_TOKEN -u GITHUB_TOKEN -u DEREK_TOKEN \
      claude --print --dangerously-skip-permissions --max-turns 40 "$prompt"
  )
}

repair_rebase() {
  local name=$1 attempt=0
  local dir=${REPO_DIRS[$name]}

  while rebase_in_progress "$dir"; do
    attempt=$((attempt + 1))
    if [ "$attempt" -gt "$MAX_REPAIR_ATTEMPTS" ]; then
      echo "::error::$name rebase still conflicts after $MAX_REPAIR_ATTEMPTS attempts"
      return 1
    fi

    run_claude "$dir" "Resolve the current git rebase conflicts in this Rust repository. Read and follow AGENTS.md and CONTRIBUTING.md when present. Preserve the nightly branch's dependency compatibility changes while incorporating main. Do not abort the rebase, commit, push, or change the requested git dependency revisions. Resolve files only, then stop."

    git -C "$dir" add -A
    if git -C "$dir" diff --cached --quiet; then
      GIT_EDITOR=true git -C "$dir" rebase --skip
    else
      GIT_EDITOR=true git -C "$dir" rebase --continue || true
    fi
  done
}

prepare_branch() {
  local name=$1
  local dir=${REPO_DIRS[$name]}

  if git -C "$dir" show-ref --verify --quiet "refs/remotes/origin/$NIGHTLY_BRANCH"; then
    git -C "$dir" checkout -B "$NIGHTLY_BRANCH" "origin/$NIGHTLY_BRANCH"
    git -C "$dir" rebase origin/main || repair_rebase "$name"
  else
    git -C "$dir" checkout -b "$NIGHTLY_BRANCH" origin/main
  fi

  if rebase_in_progress "$dir" || ! git -C "$dir" merge-base --is-ancestor origin/main HEAD; then
    echo "::error::$name nightly branch was not fully rebased onto main"
    return 1
  fi
}

setup_build() {
  local name=$1
  local dir=${REPO_DIRS[$name]}
  local marker="$WORK_ROOT/.$name-build-setup"
  if [ -e "$marker" ]; then
    return 0
  fi
  if [ "$name" = "revmc" ] || [ "$name" = "reth" ]; then
    "$dir/.github/scripts/install_llvm.sh" ubuntu
  fi
  touch "$marker"
}

run_build() {
  local name=$1
  local dir=${REPO_DIRS[$name]} command=${BUILD_COMMANDS[$name]}
  local log="$dir/target/nightly-build.log"
  local attempt=0

  mkdir -p "$dir/target"

  while true; do
    if (cd "$dir" && bash -o pipefail -c "$command") 2>&1 | tee "$log"; then
      return 0
    fi

    attempt=$((attempt + 1))
    if [ "$attempt" -gt "$MAX_REPAIR_ATTEMPTS" ]; then
      echo "::error::$name does not build after $MAX_REPAIR_ATTEMPTS repair attempts"
      return 1
    fi

    run_claude "$dir" "The nightly dependency branch must pass: $command

The latest failure is in $log. Read the log, repository instructions, and relevant upstream APIs. Fix the source and manifests, then run the build until it succeeds. Do not commit, push, weaken lints, suppress warnings, or change the requested git dependency revisions."
  done
}

format_repo() {
  local dir=$1
  (cd "$dir" && cargo +nightly fmt --all)
}

commit_repairs() {
  local name=$1
  local dir=${REPO_DIRS[$name]}
  if [ -z "$(git -C "$dir" status --porcelain)" ]; then
    return 0
  fi

  format_repo "$dir"
  git -C "$dir" add -A
  if [ "$(git -C "$dir" rev-list --count origin/main..HEAD)" -gt 0 ]; then
    git -C "$dir" commit --amend --no-edit
  else
    git -C "$dir" commit -m "chore(deps): maintain nightly compatibility"
  fi
}

push_nightly() {
  local name=$1
  local dir=${REPO_DIRS[$name]}
  git -C "$dir" push --force-with-lease origin "HEAD:$NIGHTLY_BRANCH"
  echo "$name nightly: $(git -C "$dir" rev-parse HEAD)"
}

pin_dependency() {
  local manifest=$1 dependency=$2 repo=$3 rev=$4
  local replacement="git = \"https://github.com/$repo\", rev = \"$rev\""

  if grep -q -E "^${dependency} = \\{" "$manifest"; then
    sed -i -E "/^${dependency} = \\{/ s#(version|git) = \"[^\"]+\"(, (rev|branch|tag) = \"[^\"]+\")?#${replacement}#" "$manifest"
  elif grep -q -E "^${dependency} = \"[^\"]+\"$" "$manifest"; then
    sed -i -E "s#^${dependency} = \"[^\"]+\"\$#${dependency} = { ${replacement} }#" "$manifest"
  else
    echo "::error::Could not find $dependency in $manifest"
    return 1
  fi
}

assert_dependency() {
  local manifest=$1 dependency=$2 repo=$3 rev=$4
  grep -E "^${dependency} = .*git = \"https://github.com/${repo}\", rev = \"${rev}\"" "$manifest" >/dev/null
}

amend_dependency_update() {
  local name=$1
  local dir=${REPO_DIRS[$name]}
  shift

  setup_build "$name"
  run_build "$name"
  format_repo "$dir"

  while [ "$#" -gt 0 ]; do
    assert_dependency "$dir/Cargo.toml" "$1" "$2" "$3"
    shift 3
  done

  if [ -z "$(git -C "$dir" status --porcelain)" ]; then
    push_nightly "$name"
    return 0
  fi

  git -C "$dir" add -A
  if [ "$(git -C "$dir" rev-list --count origin/main..HEAD)" -gt 0 ]; then
    git -C "$dir" commit --amend --no-edit
  else
    git -C "$dir" commit -m "chore(deps): update nightly dependencies"
  fi
  push_nightly "$name"
}

prepare_and_publish_base() {
  local name=$1
  prepare_branch "$name"
  setup_build "$name"
  run_build "$name"
  commit_repairs "$name"
  push_nightly "$name"
}

pin_revm_facade() {
  local name=$1 rev=$2
  local dir=${REPO_DIRS[$name]}
  pin_dependency "$dir/Cargo.toml" revm bluealloy/revm "$rev"
  amend_dependency_update "$name" revm bluealloy/revm "$rev"
}

pin_revmc_revm() {
  local rev=$1 dir=${REPO_DIRS[revmc]} package
  local packages=(
    revm-bytecode
    revm-context
    revm-context-interface
    revm-database
    revm-database-interface
    revm-handler
    revm-inspector
    revm-interpreter
    revm-primitives
    revm-state
    revm-statetest-types
  )

  for package in "${packages[@]}"; do
    pin_dependency "$dir/Cargo.toml" "$package" bluealloy/revm "$rev"
  done
}

update_reth_dependencies() {
  local revm_rev=$1 alloy_evm_rev=$2 reth_core_rev=$3
  local inspectors_rev=$4 revmc_rev=$5 dir=${REPO_DIRS[reth]} package
  local reth_core_packages=(
    reth-codecs
    reth-codecs-derive
    reth-primitives-traits
    reth-rpc-traits
    reth-zstd-compressors
  )

  pin_dependency "$dir/Cargo.toml" revm bluealloy/revm "$revm_rev"
  pin_dependency "$dir/Cargo.toml" alloy-evm alloy-rs/evm "$alloy_evm_rev"
  for package in "${reth_core_packages[@]}"; do
    pin_dependency "$dir/Cargo.toml" "$package" paradigmxyz/reth-core "$reth_core_rev"
  done
  pin_dependency "$dir/Cargo.toml" revm-inspectors paradigmxyz/revm-inspectors "$inspectors_rev"
  pin_dependency "$dir/Cargo.toml" revmc paradigmxyz/revmc "$revmc_rev"

  amend_dependency_update reth \
    revm bluealloy/revm "$revm_rev" \
    alloy-evm alloy-rs/evm "$alloy_evm_rev" \
    reth-codecs paradigmxyz/reth-core "$reth_core_rev" \
    reth-codecs-derive paradigmxyz/reth-core "$reth_core_rev" \
    reth-primitives-traits paradigmxyz/reth-core "$reth_core_rev" \
    reth-rpc-traits paradigmxyz/reth-core "$reth_core_rev" \
    reth-zstd-compressors paradigmxyz/reth-core "$reth_core_rev" \
    revm-inspectors paradigmxyz/revm-inspectors "$inspectors_rev" \
    revmc paradigmxyz/revmc "$revmc_rev"
}

main() {
  require_environment
  configure_git
  rm -rf "$WORK_ROOT"
  mkdir -p "$WORK_ROOT"

  clone_repo revm bluealloy/revm
  clone_repo alloy-evm alloy-rs/evm
  clone_repo reth-core paradigmxyz/reth-core
  clone_repo revm-inspectors paradigmxyz/revm-inspectors
  clone_repo revmc paradigmxyz/revmc
  clone_repo reth paradigmxyz/reth

  # revm has no upstream nightly pin while alloy-rs/core is excluded.
  prepare_and_publish_base revm
  local revm_rev
  revm_rev=$(git -C "${REPO_DIRS[revm]}" rev-parse HEAD)

  prepare_and_publish_base alloy-evm
  pin_revm_facade alloy-evm "$revm_rev"

  # reth-core does not depend on revm, so only advance and build its branch.
  prepare_and_publish_base reth-core

  prepare_and_publish_base revm-inspectors
  pin_revm_facade revm-inspectors "$revm_rev"

  local alloy_evm_rev reth_core_rev inspectors_rev
  alloy_evm_rev=$(git -C "${REPO_DIRS[alloy-evm]}" rev-parse HEAD)
  reth_core_rev=$(git -C "${REPO_DIRS[reth-core]}" rev-parse HEAD)
  inspectors_rev=$(git -C "${REPO_DIRS[revm-inspectors]}" rev-parse HEAD)

  prepare_and_publish_base revmc
  pin_revmc_revm "$revm_rev"
  pin_dependency "${REPO_DIRS[revmc]}/Cargo.toml" alloy-evm alloy-rs/evm "$alloy_evm_rev"
  amend_dependency_update revmc \
    revm-bytecode bluealloy/revm "$revm_rev" \
    revm-context bluealloy/revm "$revm_rev" \
    revm-context-interface bluealloy/revm "$revm_rev" \
    revm-database bluealloy/revm "$revm_rev" \
    revm-database-interface bluealloy/revm "$revm_rev" \
    revm-handler bluealloy/revm "$revm_rev" \
    revm-inspector bluealloy/revm "$revm_rev" \
    revm-interpreter bluealloy/revm "$revm_rev" \
    revm-primitives bluealloy/revm "$revm_rev" \
    revm-state bluealloy/revm "$revm_rev" \
    revm-statetest-types bluealloy/revm "$revm_rev" \
    alloy-evm alloy-rs/evm "$alloy_evm_rev"

  local revmc_rev
  revmc_rev=$(git -C "${REPO_DIRS[revmc]}" rev-parse HEAD)

  prepare_and_publish_base reth
  update_reth_dependencies \
    "$revm_rev" \
    "$alloy_evm_rev" \
    "$reth_core_rev" \
    "$inspectors_rev" \
    "$revmc_rev"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  main "$@"
fi
