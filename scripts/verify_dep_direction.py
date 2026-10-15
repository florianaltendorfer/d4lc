#!/usr/bin/env python3
"""Assert workspace crate dependency direction from the real cargo graph.

Runs `cargo metadata --format-version 1` and walks the workspace-internal
dependency graph of the default-feature host resolve. Normal, dev, and build
edges all compose transitively: a rule fails when a source crate can reach a
forbidden crate through any chain of workspace-internal dependencies.

Exit codes: 0 = clean, 1 = forbidden edges found, 2 = the graph could not be
checked (cargo metadata failed or the table below names an unknown crate).
"""

from __future__ import annotations

import json
import subprocess
import sys
from collections import deque
from pathlib import Path

# ---------------------------------------------------------------------------
# Forbidden-edge table. Edit this table to add or tighten rules; no logic
# changes are needed. Each rule is (sources, forbidden targets, description).
# ---------------------------------------------------------------------------
PIPELINE_CORE_CRATES = [
    "d4lc-capture",
    "d4lc-vision",
    "d4lc-detect",
    "d4lc-ocr",
    "d4lc-fuzzy",
    "d4lc-parse",
    "d4lc-rules",
    "d4lc-config",
    "d4lc-assets",
    "d4lc-core",
]
UI_CRATES = ["d4lc-gui", "d4lc-app", "d4lc-cli"]

FORBIDDEN_EDGES = [
    (
        PIPELINE_CORE_CRATES,
        UI_CRATES,
        "pipeline/core crates must not depend on gui/app/cli",
    ),
    (
        ["d4lc-runtime"],
        UI_CRATES,
        "runtime must not depend on gui/app/cli",
    ),
    (
        PIPELINE_CORE_CRATES + ["d4lc-runtime"],
        ["d4lc-overlay-x11", "d4lc-overlay-wayland"],
        "pipeline/runtime crates must not depend on an overlay backend "
        "(the frame contract is d4lc-rules::OverlayFrame)",
    ),
]
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parent.parent


def load_workspace_graph() -> tuple[dict[str, str], dict[str, dict[str, set[str]]]]:
    """Return (member names by package id, internal edges name -> name -> kinds)."""
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = (result.stderr or "").strip().splitlines()
        message = detail[-1] if detail else "no error output"
        print(
            f"dependency direction check could not run: cargo metadata failed: {message}",
            file=sys.stderr,
        )
        raise SystemExit(2)

    metadata = json.loads(result.stdout)
    members = set(metadata["workspace_members"])
    id_to_name = {
        package["id"]: package["name"]
        for package in metadata["packages"]
        if package["id"] in members
    }
    edges: dict[str, dict[str, set[str]]] = {}
    for node in metadata["resolve"]["nodes"]:
        if node["id"] not in members:
            continue
        source = id_to_name[node["id"]]
        for dep in node["deps"]:
            if dep["pkg"] not in members:
                continue
            target = id_to_name[dep["pkg"]]
            kinds = {(kind.get("kind") or "normal") for kind in dep["dep_kinds"]}
            edges.setdefault(source, {}).setdefault(target, set()).update(kinds or {"normal"})
    return id_to_name, edges


def find_reach_paths(
    source: str, edges: dict[str, dict[str, set[str]]]
) -> dict[str, tuple[list[str], list[set[str]]]]:
    """BFS over internal edges; return reachable name -> (path, edge kinds per hop)."""
    first: dict[str, tuple[list[str], list[set[str]]]] = {source: ([source], [])}
    queue = deque([source])
    while queue:
        current = queue.popleft()
        path, kinds = first[current]
        for nxt in sorted(edges.get(current, {})):
            if nxt in first:
                continue
            first[nxt] = (path + [nxt], kinds + [edges[current][nxt]])
            queue.append(nxt)
    return first


def render_path(path: list[str], kinds: list[set[str]]) -> str:
    rendered = path[0]
    for node, edge_kinds in zip(path[1:], kinds):
        rendered += f" -> {node} [{','.join(sorted(edge_kinds))}]"
    return rendered


def main() -> int:
    id_to_name, edges = load_workspace_graph()
    names = set(id_to_name.values())

    unknown = sorted(
        {crate for sources, targets, _ in FORBIDDEN_EDGES for crate in sources + targets}
        - names
    )
    if unknown:
        for crate in unknown:
            print(f"forbidden-edge table names unknown workspace crate: {crate}", file=sys.stderr)
        return 2

    violations: list[tuple[str, tuple[list[str], list[set[str]]]]] = []
    for sources, targets, description in FORBIDDEN_EDGES:
        for source in sources:
            reach = find_reach_paths(source, edges)
            for target in targets:
                if target in reach:
                    violations.append((description, reach[target]))

    if violations:
        print(f"dependency direction check FAILED: {len(violations)} forbidden edge(s)")
        for description, (path, kinds) in violations:
            print(f"  FORBIDDEN {render_path(path, kinds)}")
            print(f"    rule: {description}")
        return 1

    edge_count = sum(len(targets) for targets in edges.values())
    print(
        f"dependency direction OK: {len(FORBIDDEN_EDGES)} rule(s), "
        f"{len(names)} workspace crates, {edge_count} internal edge(s)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
