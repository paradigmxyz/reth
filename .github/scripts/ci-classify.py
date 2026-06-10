#!/usr/bin/env python3

import json
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path


TRUE = "true"
FALSE = "false"


def git(*args: str) -> str:
    return subprocess.check_output(("git", *args), text=True, stderr=subprocess.DEVNULL)


def git_optional(*args: str) -> str | None:
    try:
        return git(*args)
    except subprocess.CalledProcessError:
        return None


def set_outputs(outputs: dict[str, str]) -> None:
    lines = [f"{key}={value}" for key, value in outputs.items()]
    for line in lines:
        print(line)

    output_path = os.environ.get("GITHUB_OUTPUT")
    if output_path:
        with Path(output_path).open("a", encoding="utf-8") as output:
            output.write("\n".join(lines))
            output.write("\n")


def as_bool(value: bool) -> str:
    return TRUE if value else FALSE


def read_event() -> dict:
    event_path = os.environ.get("GITHUB_EVENT_PATH")
    if not event_path:
        return {}
    return json.loads(Path(event_path).read_text(encoding="utf-8"))


def resolve_refs() -> tuple[str | None, str | None, bool]:
    base = os.environ.get("CI_CLASSIFY_BASE")
    head = os.environ.get("CI_CLASSIFY_HEAD")
    if base and head:
        return base, head, False

    if os.environ.get("GITHUB_EVENT_NAME", "") != "pull_request":
        return None, None, True

    event = read_event()
    base = event.get("pull_request", {}).get("base", {}).get("sha")
    head = os.environ.get("GITHUB_SHA", "HEAD")
    return base, head, not bool(base and head)


def ensure_ref(ref: str) -> bool:
    if git_optional("cat-file", "-e", f"{ref}^{{commit}}") is not None:
        return True
    return git_optional("fetch", "--no-tags", "--depth=1", "origin", ref) is not None


def parse_changes(base: str, head: str) -> list[dict[str, str]]:
    output = git("diff", "--name-status", "--find-renames", base, head).strip()
    if not output:
        return []

    changes = []
    for line in output.splitlines():
        parts = line.split("\t")
        changes.append({"status": parts[0], "path": parts[-1]})
    return changes


def read_path(ref: str, path: str) -> str | None:
    return git_optional("show", f"{ref}:{path}")


def workspace_version(cargo_toml: str) -> str | None:
    parsed = tomllib.loads(cargo_toml)
    return parsed.get("workspace", {}).get("package", {}).get("version")


def cargo_without_workspace_version(cargo_toml: str) -> dict:
    parsed = tomllib.loads(cargo_toml)
    parsed.setdefault("workspace", {}).setdefault("package", {})["version"] = "<workspace-version>"
    return parsed


def lock_without_workspace_versions(cargo_lock: str, version: str) -> dict:
    parsed = tomllib.loads(cargo_lock)
    for package in parsed.get("package", []):
        if "source" not in package and package.get("version") == version:
            package["version"] = "<workspace-version>"
    return parsed


def docs_without_version(docs_config: str, version: str) -> str:
    pattern = re.compile(rf"(text:\s*['\"])v{re.escape(version)}(['\"])")
    return pattern.sub(r"\1v<workspace-version>\2", docs_config)


def is_release_version_only(base: str, head: str, changes: list[dict[str, str]]) -> bool:
    release_files = {"Cargo.toml", "Cargo.lock", "docs/vocs/vocs.config.ts"}
    files = [change["path"] for change in changes]

    if "Cargo.toml" not in files or "Cargo.lock" not in files:
        return False
    if any(file not in release_files for file in files):
        return False
    if any(change["status"] != "M" for change in changes):
        return False

    old_cargo = read_path(base, "Cargo.toml")
    new_cargo = read_path(head, "Cargo.toml")
    old_lock = read_path(base, "Cargo.lock")
    new_lock = read_path(head, "Cargo.lock")
    if not old_cargo or not new_cargo or not old_lock or not new_lock:
        return False

    old_version = workspace_version(old_cargo)
    new_version = workspace_version(new_cargo)
    if not old_version or not new_version or old_version == new_version:
        return False
    if cargo_without_workspace_version(old_cargo) != cargo_without_workspace_version(new_cargo):
        return False
    if lock_without_workspace_versions(old_lock, old_version) != lock_without_workspace_versions(
        new_lock, new_version
    ):
        return False

    if "docs/vocs/vocs.config.ts" in files:
        old_docs = read_path(base, "docs/vocs/vocs.config.ts")
        new_docs = read_path(head, "docs/vocs/vocs.config.ts")
        if not old_docs or not new_docs:
            return False
        if docs_without_version(old_docs, old_version) != docs_without_version(new_docs, new_version):
            return False

    return True


def is_docs_path(path: str) -> bool:
    return (
        path.startswith("docs/")
        or re.search(r"\.(md|mdx|txt|png|jpg|jpeg|gif|webp|svg)$", path, re.I) is not None
    )


def is_grafana_path(path: str) -> bool:
    return path.startswith("etc/grafana/")


def is_ci_path(path: str) -> bool:
    return path.startswith(".github/")


def is_cargo_path(path: str) -> bool:
    return path in {"Cargo.lock", "Cargo.toml"} or (
        path.endswith("/Cargo.toml") and not path.startswith("docs/")
    )


def full_outputs(reason: str) -> dict[str, str]:
    return {
        "reason": reason,
        "version_only": FALSE,
        "run_heavy_rust": TRUE,
        "run_dependency_checks": TRUE,
        "run_release_version_check": FALSE,
        "run_cargo_validation": TRUE,
        "run_docs_site": TRUE,
        "run_cli_docs": TRUE,
        "run_grafana": TRUE,
        "lint_allowed_skips": "release-version",
        "unit_allowed_skips": "",
        "integration_allowed_skips": "",
        "e2e_allowed_skips": "",
        "compact_allowed_skips": "",
    }


def main() -> int:
    base, head, force_full = resolve_refs()
    if force_full:
        set_outputs(full_outputs("non-pr-event"))
        return 0
    if not base or not head or not ensure_ref(base) or not ensure_ref(head):
        set_outputs(full_outputs("missing-diff-ref"))
        return 0

    changes = parse_changes(base, head)
    files = [change["path"] for change in changes]
    version_only = is_release_version_only(base, head, changes)
    non_behavioral_only = (
        version_only
        or not files
        or all(is_docs_path(file) or is_grafana_path(file) or is_ci_path(file) for file in files)
    )
    has_cargo_change = any(is_cargo_path(file) for file in files)
    dependency_checks = has_cargo_change and not version_only
    run_heavy_rust = not non_behavioral_only
    run_docs_site = any(file.startswith("docs/vocs/") for file in files)
    run_grafana = any(is_grafana_path(file) for file in files)
    run_release_version_check = version_only
    run_cargo_validation = has_cargo_change or version_only
    run_cli_docs = run_heavy_rust

    lint_allowed_skips = []
    if not run_heavy_rust:
        lint_allowed_skips.extend(
            ["clippy-binaries", "clippy", "wasm", "crate-checks", "docs", "udeps", "book"]
        )
    if not dependency_checks:
        lint_allowed_skips.extend(["deny", "no-test-deps", "feature-propagation"])
    if not run_release_version_check:
        lint_allowed_skips.append("release-version")
    if not run_grafana:
        lint_allowed_skips.append("grafana")

    reason = "no-changes"
    if version_only:
        reason = "version-only"
    elif run_heavy_rust:
        reason = "behavioral-change"
    elif files:
        reason = "non-behavioral-paths"

    set_outputs(
        {
            "reason": reason,
            "version_only": as_bool(version_only),
            "run_heavy_rust": as_bool(run_heavy_rust),
            "run_dependency_checks": as_bool(dependency_checks),
            "run_release_version_check": as_bool(run_release_version_check),
            "run_cargo_validation": as_bool(run_cargo_validation),
            "run_docs_site": as_bool(run_docs_site),
            "run_cli_docs": as_bool(run_cli_docs),
            "run_grafana": as_bool(run_grafana),
            "lint_allowed_skips": ",".join(lint_allowed_skips),
            "unit_allowed_skips": "" if run_heavy_rust else "test,state,doc",
            "integration_allowed_skips": "" if run_heavy_rust else "test",
            "e2e_allowed_skips": "" if run_heavy_rust else "test,rocksdb",
            "compact_allowed_skips": "" if run_heavy_rust else "compact-codec",
        }
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
