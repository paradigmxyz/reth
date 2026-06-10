#!/usr/bin/env python3

import json
import subprocess
import sys
import tomllib
from pathlib import Path


def command(*args: str) -> str:
    return subprocess.check_output(args, text=True)


def workspace_version(cargo_toml: str) -> str:
    parsed = tomllib.loads(cargo_toml)
    version = parsed.get("workspace", {}).get("package", {}).get("version")
    if not version:
        raise RuntimeError("workspace.package.version is missing from Cargo.toml")
    return version


def manifest_uses_workspace_version(path: str) -> bool:
    manifest = Path(path).read_text(encoding="utf-8")
    return any(line.strip() == "version.workspace = true" for line in manifest.splitlines())


def main() -> int:
    version = workspace_version(Path("Cargo.toml").read_text(encoding="utf-8"))
    docs_config = Path("docs/vocs/vocs.config.ts").read_text(encoding="utf-8")
    if f"text: 'v{version}'" not in docs_config:
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

    print(f"release version {version} is consistent")
    return 0


if __name__ == "__main__":
    sys.exit(main())
