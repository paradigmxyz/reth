#!/usr/bin/env python3

import argparse
import json
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path


TRUE = "true"
FALSE = "false"


def command(*args: str) -> str:
    return subprocess.check_output(args, text=True)


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


def is_main_branch_ref(ref: str | None, ref_name: str | None) -> bool:
    return ref == "refs/heads/main" or ref_name == "main"


def is_main_branch() -> bool:
    return is_main_branch_ref(os.environ.get("GITHUB_REF"), os.environ.get("GITHUB_REF_NAME"))


def resolve_refs() -> tuple[str | None, str | None, str | None]:
    base = os.environ.get("CI_CLASSIFY_BASE")
    head = os.environ.get("CI_CLASSIFY_HEAD")
    if base and head:
        return base, head, None

    if is_main_branch():
        return None, None, "main-branch"

    if os.environ.get("GITHUB_EVENT_NAME", "") != "pull_request":
        return None, None, "non-pr-event"

    event = read_event()
    base = event.get("pull_request", {}).get("base", {}).get("sha")
    head = os.environ.get("GITHUB_SHA", "HEAD")
    return base, head, None


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


def docs_points_to_version(docs_config: str, version: str) -> bool:
    return re.search(rf"text:\s*['\"]v{re.escape(version)}['\"]", docs_config) is not None


def is_workspace_version_bump(base: str, head: str, changes: list[dict[str, str]]) -> bool:
    allowed_files = {"Cargo.toml", "Cargo.lock", "docs/vocs/vocs.config.ts"}
    files = [change["path"] for change in changes]

    if "Cargo.toml" not in files or "Cargo.lock" not in files:
        return False
    if any(file not in allowed_files for file in files):
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
        or re.search(r"\.(md|mdx|png|jpg|jpeg|gif|webp|svg)$", path, re.I) is not None
    )


def is_grafana_path(path: str) -> bool:
    return path.startswith("etc/grafana/")


def is_passive_github_path(path: str) -> bool:
    return (
        path == ".github/CODEOWNERS"
        or path == ".github/dependabot.yml"
        or path.startswith(".github/ISSUE_TEMPLATE/")
    )


def is_cargo_path(path: str) -> bool:
    return path in {"Cargo.lock", "Cargo.toml"} or (
        path.endswith("/Cargo.toml") and not path.startswith("docs/")
    )


def full_outputs(reason: str) -> dict[str, str]:
    return {
        "reason": reason,
        "workspace_version_bump": FALSE,
        "run_heavy_rust": TRUE,
        "run_dependency_checks": TRUE,
        "run_version_sync_check": TRUE,
        "run_docs_site": TRUE,
        "run_cli_docs": TRUE,
        "run_grafana": TRUE,
        "lint_allowed_skips": "",
        "unit_allowed_skips": "",
        "integration_allowed_skips": "",
        "e2e_allowed_skips": "",
        "compact_allowed_skips": "",
    }


def classify_outputs(files: list[str], workspace_bump: bool) -> dict[str, str]:
    non_behavioral_only = (
        workspace_bump
        or not files
        or all(
            is_docs_path(file) or is_grafana_path(file) or is_passive_github_path(file)
            for file in files
        )
    )
    has_cargo_change = any(is_cargo_path(file) for file in files)
    dependency_checks = has_cargo_change and not workspace_bump
    run_heavy_rust = not non_behavioral_only
    run_docs_site = any(file.startswith("docs/vocs/") for file in files)
    run_grafana = any(is_grafana_path(file) for file in files)
    run_version_sync_check = workspace_bump
    run_cli_docs = run_heavy_rust or any(
        file.startswith("docs/cli/") or file.startswith("docs/vocs/docs/pages/cli/")
        for file in files
    )

    lint_allowed_skips = []
    if not run_heavy_rust:
        lint_allowed_skips.extend(["clippy", "wasm", "docs", "udeps"])
    if not dependency_checks:
        lint_allowed_skips.extend(["deny", "no-test-deps", "feature-propagation"])
    if not run_version_sync_check:
        lint_allowed_skips.append("version-sync")
    if not run_cli_docs:
        lint_allowed_skips.append("book")
    if not run_grafana:
        lint_allowed_skips.append("grafana")

    reason = "no-changes"
    if workspace_bump:
        reason = "workspace-version-bump"
    elif run_heavy_rust:
        reason = "behavioral-change"
    elif files:
        reason = "non-behavioral-paths"

    return {
        "reason": reason,
        "workspace_version_bump": as_bool(workspace_bump),
        "run_heavy_rust": as_bool(run_heavy_rust),
        "run_dependency_checks": as_bool(dependency_checks),
        "run_version_sync_check": as_bool(run_version_sync_check),
        "run_docs_site": as_bool(run_docs_site),
        "run_cli_docs": as_bool(run_cli_docs),
        "run_grafana": as_bool(run_grafana),
        "lint_allowed_skips": ",".join(lint_allowed_skips),
        "unit_allowed_skips": "" if run_heavy_rust else "test,state,doc",
        "integration_allowed_skips": "" if run_heavy_rust else "test",
        "e2e_allowed_skips": "" if run_heavy_rust else "test,rocksdb",
        "compact_allowed_skips": "" if run_heavy_rust else "compact-codec",
    }


def classify() -> int:
    base, head, force_full_reason = resolve_refs()
    if force_full_reason:
        set_outputs(full_outputs(force_full_reason))
        return 0
    if not base or not head or not ensure_ref(base) or not ensure_ref(head):
        set_outputs(full_outputs("missing-diff-ref"))
        return 0

    changes = parse_changes(base, head)
    files = [change["path"] for change in changes]
    workspace_bump = is_workspace_version_bump(base, head, changes)

    set_outputs(classify_outputs(files, workspace_bump))
    return 0


def manifest_uses_workspace_version(path: str) -> bool:
    manifest = Path(path).read_text(encoding="utf-8")
    return any(line.strip() == "version.workspace = true" for line in manifest.splitlines())


def check_version_sync() -> int:
    version = workspace_version(Path("Cargo.toml").read_text(encoding="utf-8"))
    if not version:
        raise RuntimeError("workspace.package.version is missing from Cargo.toml")

    docs_config = Path("docs/vocs/vocs.config.ts").read_text(encoding="utf-8")
    if not docs_points_to_version(docs_config, version):
        raise RuntimeError(f"docs/vocs/vocs.config.ts does not point at v{version}")

    metadata = json.loads(command("cargo", "metadata", "--locked", "--format-version=1"))
    workspace_members = set(metadata["workspace_members"])
    packages_by_id = {package["id"]: package for package in metadata["packages"]}
    mismatches = []

    for package_id in workspace_members:
        package = packages_by_id.get(package_id)
        if not package:
            continue
        manifest_path = package["manifest_path"]
        if manifest_path.endswith("/reth/Cargo.toml") or manifest_uses_workspace_version(manifest_path):
            if package["version"] != version:
                mismatches.append(f"{package['name']}: {package['version']}")

    if mismatches:
        raise RuntimeError(
            f"workspace package versions do not match {version}: {', '.join(mismatches)}"
        )

    print(f"workspace version {version} is consistent")
    return 0


def expect_classification(files: list[str], workspace_bump: bool, expected: dict[str, str]) -> None:
    outputs = classify_outputs(files, workspace_bump)
    for key, value in expected.items():
        actual = outputs.get(key)
        if actual != value:
            raise AssertionError(
                f"{files}: expected {key}={value}, got {actual}; outputs={outputs}"
            )


def self_test() -> int:
    if not is_main_branch_ref("refs/heads/main", None):
        raise AssertionError("refs/heads/main should force full CI")
    if not is_main_branch_ref(None, "main"):
        raise AssertionError("GITHUB_REF_NAME=main should force full CI")
    if is_main_branch_ref("refs/pull/1/merge", "1/merge"):
        raise AssertionError("pull request refs should not be treated as main")
    if full_outputs("main-branch")["run_version_sync_check"] != TRUE:
        raise AssertionError("full CI paths should include version sync")
    if full_outputs("main-branch")["lint_allowed_skips"]:
        raise AssertionError("full CI paths should not allow skipped lint jobs")
    if not docs_points_to_version("text: 'v1.2.3'", "1.2.3"):
        raise AssertionError("single-quoted docs version should be accepted")
    if not docs_points_to_version('text: "v1.2.3"', "1.2.3"):
        raise AssertionError("double-quoted docs version should be accepted")

    expect_classification(
        ["Cargo.toml", "Cargo.lock", "docs/vocs/vocs.config.ts"],
        True,
        {
            "reason": "workspace-version-bump",
            "run_heavy_rust": FALSE,
            "run_dependency_checks": FALSE,
            "run_version_sync_check": TRUE,
            "run_cli_docs": FALSE,
        },
    )
    expect_classification(
        ["crates/storage/Cargo.toml"],
        False,
        {
            "reason": "behavioral-change",
            "run_heavy_rust": TRUE,
            "run_dependency_checks": TRUE,
            "run_cli_docs": TRUE,
        },
    )
    expect_classification(
        ["crates/era-downloader/tests/res/era1/checksums.txt"],
        False,
        {
            "reason": "behavioral-change",
            "run_heavy_rust": TRUE,
            "run_cli_docs": TRUE,
        },
    )
    expect_classification(
        ["docs/vocs/docs/pages/cli/reth.mdx"],
        False,
        {
            "reason": "non-behavioral-paths",
            "run_heavy_rust": FALSE,
            "run_docs_site": TRUE,
            "run_cli_docs": TRUE,
        },
    )
    expect_classification(
        [".github/workflows/lint.yml"],
        False,
        {
            "reason": "behavioral-change",
            "run_heavy_rust": TRUE,
            "run_cli_docs": TRUE,
        },
    )
    expect_classification(
        [".github/ISSUE_TEMPLATE/bug.yml"],
        False,
        {
            "reason": "non-behavioral-paths",
            "run_heavy_rust": FALSE,
            "run_cli_docs": FALSE,
        },
    )
    expect_classification(
        ["etc/grafana/dashboards/overview.json"],
        False,
        {
            "reason": "non-behavioral-paths",
            "run_heavy_rust": FALSE,
            "run_grafana": TRUE,
        },
    )
    print("ci-rules self-test passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Evaluate CI rules for PR changes")
    subcommands = parser.add_subparsers(dest="command", required=True)
    subcommands.add_parser("classify", help="classify a PR diff and emit GitHub Actions outputs")
    subcommands.add_parser("check-version-sync", help="validate workspace version metadata")
    subcommands.add_parser("self-test", help="run classifier fixture tests")
    args = parser.parse_args()

    if args.command == "classify":
        return classify()
    if args.command == "check-version-sync":
        return check_version_sync()
    if args.command == "self-test":
        return self_test()

    raise RuntimeError(f"unknown command: {args.command}")


if __name__ == "__main__":
    sys.exit(main())
