#!/usr/bin/env python3
"""Deterministic agency-agents ↔ App zh-TW resource parity gate."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


EXPECTED_BASELINE = "3c9588880b7cafaec325a104899fd8bbe27e7d72"
MIRROR = Path("src-tauri/resources/corpus-baseline/scripts/i18n")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--app-root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--source-root", type=Path)
    args = parser.parse_args()
    app_root = args.app_root.resolve()
    mirror_dir = app_root / MIRROR
    manifest_path = mirror_dir / "source-association.json"
    manifest_bytes = manifest_path.read_bytes()
    manifest = json.loads(manifest_bytes)
    if manifest.get("schemaVersion") != 1:
        raise SystemExit("parity manifest schemaVersion must be 1")
    if manifest.get("sourceBaselineCommit") != EXPECTED_BASELINE:
        raise SystemExit("parity manifest baseline commit does not match the approved pin")
    if manifest.get("sourceRepository") != "keyrc0816/agency-agents":
        raise SystemExit("parity manifest sourceRepository is not the controlled fork")
    if manifest.get("sourceDefaultRef") != "main":
        raise SystemExit("parity manifest sourceDefaultRef must be the durable main ref")
    if "sourceBranch" in manifest:
        raise SystemExit("parity manifest must not depend on a permanent feature branch")
    resource_path = manifest.get("resourcePath")
    if resource_path != "scripts/i18n/agent-metadata-zh-TW.json":
        raise SystemExit("parity manifest resourcePath is invalid")

    mirror_resource = mirror_dir / "agent-metadata-zh-TW.json"
    mirror_bytes = mirror_resource.read_bytes()
    if sha256(mirror_bytes) != manifest.get("resourceSha256"):
        raise SystemExit("App mirror SHA-256 differs from the pinned source resource hash")
    data = json.loads(mirror_bytes)
    if data.get("schemaVersion") != 1 or data.get("locale") != "zh-TW":
        raise SystemExit("App mirror schema/locale is invalid")
    if not isinstance(data.get("agents"), dict) or not data["agents"]:
        raise SystemExit("App mirror has no serialized Agent mapping")

    if args.source_root:
        source_root = args.source_root.resolve()
        source_bytes = (source_root / resource_path).read_bytes()
        source_manifest = (
            source_root / "scripts/i18n/source-association.json"
        ).read_bytes()
        if source_bytes != mirror_bytes:
            raise SystemExit("App mirror is not byte-identical to agency-agents source")
        if source_manifest != manifest_bytes:
            raise SystemExit("App/source association manifests are not byte-identical")

    print(
        "PASS: zh-TW mirror parity "
        f"(agents={len(data['agents'])}, sha256={sha256(mirror_bytes)}, "
        f"baseline={EXPECTED_BASELINE})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
