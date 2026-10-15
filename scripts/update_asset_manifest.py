#!/usr/bin/env python3
from pathlib import Path
import hashlib
import json
import re

UPSTREAM_LICENSE = "MIT"


def sha256_hex(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_upstream_lock(root: Path) -> dict:
    lock_path = root / "upstream.lock"
    values: dict[str, str] = {}
    for line in lock_path.read_text().splitlines():
        match = re.match(r'^([a-z_]+)\s*=\s*"(.*)"\s*$', line.strip())
        if match:
            values[match.group(1)] = match.group(2)
    return values


def source_path_for(destination: str) -> str:
    prefix = "assets/original/"
    assert destination.startswith(prefix), destination
    rel = destination[len(prefix) :]

    if rel.startswith("data/"):
        return f"D4Companion/Data/{rel[len('data/'):]}"

    if rel.startswith("images/"):
        return f"D4Companion/Images/{rel[len('images/'):]}"

    if rel == "systempresets/systempresets.json":
        return "downloads/systempresets/systempresets.json"

    if rel.startswith("systempresets/zips/") and rel.endswith(".zip"):
        return f"downloads/systempresets/{Path(rel).name}"

    if rel.startswith("systempresets/"):
        parts = Path(rel).parts
        if len(parts) >= 3:
            preset = parts[1]
            member = Path(*parts[1:]).as_posix()
            return f"downloads/systempresets/{preset}.zip::{member}"

    raise ValueError(f"unsupported asset path for provenance mapping: {destination}")


def main() -> None:
    root = Path.cwd()
    upstream = read_upstream_lock(root)
    upstream_commit = upstream["last_reviewed_commit"]

    asset_root = root / "assets" / "original"
    out = root / "assets" / "provenance" / "asset-manifest.json"

    items = []
    for path in sorted(asset_root.rglob("*")):
        if not path.is_file() or path.name == ".gitkeep":
            continue
        rel = path.relative_to(root).as_posix()
        if rel == "assets/original/asset-manifest.json":
            continue
        items.append(
            {
                "destination_path": rel,
                "source_path": source_path_for(rel),
                "upstream_commit": upstream_commit,
                "sha256": sha256_hex(path),
                "license": UPSTREAM_LICENSE,
            }
        )

    payload = {
        "schema_version": 2,
        "upstream": {
            "url": upstream["url"],
            "branch": upstream["branch"],
            "last_reviewed_commit": upstream["last_reviewed_commit"],
            "last_reviewed_release": upstream["last_reviewed_release"],
        },
        "items": items,
    }
    if out.exists():
        existing = json.loads(out.read_text())
        for key, value in existing.items():
            if key not in payload:
                payload[key] = value
    out.write_text(json.dumps(payload, indent=2) + "\n")
    print(out)


if __name__ == "__main__":
    main()
