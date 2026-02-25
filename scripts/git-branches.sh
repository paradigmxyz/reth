#!/usr/bin/env bash
# For every local and remote branch, outputs one of:
#   merged    <branch>   Changes are in main (true merge or squash merge)
#   pr<N>     <branch>   Has an open/draft PR (and not merged)
#   expired   <branch>   Not merged, no PR, no commits in --expire-days
#   active    <branch>   Not merged, no PR, has recent activity
set -euo pipefail

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Classifies all local and remote git branches as merged, pr, expired, or unmerged.
Detects both true merges and squash merges into main.

Output (one line per branch):
  merged    <branch>   Changes are in main (true merge or squash merge)
  pr<N>     <branch>   Has an open/draft PR #N (not yet merged)
  expired   <branch>   Not merged, no PR, no commits in --expire-days
  active    <branch>   Not merged, no PR, has recent activity

Options:
  --expire-days N   Days of inactivity before an unmerged branch is
                    considered expired (default: 90)
  -h, --help        Show this help message
EOF
}

expire_days=90

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        --expire-days)
            expire_days="$2"
            shift 2
            ;;
        --expire-days=*)
            expire_days="${1#*=}"
            shift
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

main_ref="main"
cutoff=$(date -d "$expire_days days ago" +%s 2>/dev/null) \
  || cutoff=$(date -v-"${expire_days}"d +%s)  # macOS fallback

# Fetch all open/draft PRs upfront in one API call, build branch→PR# map
declare -A pr_map
while IFS=$'\t' read -r number head_ref; do
    pr_map["$head_ref"]="$number"
done < <(gh pr list --state open --json number,headRefName --limit 1000 \
           | jq -r '.[] | [.number, .headRefName] | @tsv')

# Look up a branch name (strip origin/ prefix) in the PR map.
# Prints "pr<N>" if found, empty string otherwise.
lookup_pr() {
    local branch="$1"
    local ref="${branch#origin/}"
    local pr="${pr_map[$ref]:-}"
    if [[ -n "$pr" ]]; then
        echo "pr${pr}"
    fi
}

classify_unmerged() {
    local branch="$1"
    local pr
    pr=$(lookup_pr "$branch")
    if [[ -n "$pr" ]]; then
        echo "$pr $branch"
        return
    fi
    local last_commit_epoch
    last_commit_epoch=$(git log -1 --format='%ct' "$branch" 2>/dev/null) || { echo "active $branch"; return; }
    if [[ "$last_commit_epoch" -lt "$cutoff" ]]; then
        echo "expired $branch"
    else
        echo "active $branch"
    fi
}

# True merges (branch tip is ancestor of main)
git branch -a --merged "$main_ref" --format='%(refname:short)' \
  | grep -v -E '^(main|origin/main|origin/HEAD)$' \
  | while read -r branch; do echo "merged $branch"; done

# Remaining branches: check for squash merges via patch-id comparison
git branch -a --no-merged "$main_ref" --format='%(refname:short)' \
  | grep -v -E '^(main|origin/main|origin/HEAD)$' \
  | while read -r branch; do
    merge_base=$(git merge-base "$main_ref" "$branch" 2>/dev/null) || { classify_unmerged "$branch"; continue; }
    tree=$(git rev-parse "$branch^{tree}" 2>/dev/null) || { classify_unmerged "$branch"; continue; }
    dangling=$(git commit-tree "$tree" -p "$merge_base" -m temp 2>/dev/null) || { classify_unmerged "$branch"; continue; }
    if git cherry "$main_ref" "$dangling" 2>/dev/null | grep -q '^-'; then
        echo "merged $branch"
    else
        classify_unmerged "$branch"
    fi
done
