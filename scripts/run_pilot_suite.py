#!/usr/bin/env python3
"""Run the real-input pilot suite: one process per pilot, several in flight.

A pilot drives the app through the Slint testing backend with threading and
its embedded MCP server, which claims Slint's process-wide event-loop proxy:
only one pilot can run per process. Every pilot is therefore its own
`#[test] #[ignore]` function (see `crates/d4lc-app/src/testing_harness/`),
each calling `start_pilot()` once, and this runner gives every pilot a
private process and keeps `--jobs` of them in flight, so the suite uses all
cores instead of running the scenarios one after another.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
TEST_NAME_PREFIX = "testing_harness::tests::pilot_"
# A gate that silently runs zero pilots is worse than a red one: fail closed
# when the enumeration finds fewer pilots than this.
DEFAULT_MIN_PILOTS = 20
DEFAULT_LOG_DIR = Path("target/pilot-logs")


@dataclass(frozen=True)
class PilotRun:
    name: str
    ok: bool
    seconds: float
    log_path: Path


def build_test_binary() -> Path:
    """Compile the test binary and return its path (a warm build is ~0.3 s)."""
    result = subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "d4lc-app",
            "--bin",
            "d4lc-app",
            "--no-run",
            "--message-format=json",
        ],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        raise SystemExit("[pilot] cargo could not build the pilot test binary")

    executable: str | None = None
    for line in result.stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("reason") != "compiler-artifact":
            continue
        target = event.get("target") or {}
        profile = event.get("profile") or {}
        if (
            event.get("executable")
            and target.get("name") == "d4lc-app"
            and target.get("kind") == ["bin"]
            and profile.get("test")
        ):
            executable = event["executable"]

    if executable is None:
        raise SystemExit(
            "[pilot] cargo produced no test executable for the d4lc-app bin target"
        )
    return Path(executable)


def list_pilots(executable: Path, min_pilots: int) -> list[str]:
    result = subprocess.run(
        [str(executable), "--ignored", "--list"],
        capture_output=True,
        text=True,
    )
    names = sorted(
        line[: -len(": test")]
        for line in result.stdout.splitlines()
        if line.endswith(": test") and line.startswith(TEST_NAME_PREFIX)
    )
    if len(names) < min_pilots:
        raise SystemExit(
            f"[pilot] found {len(names)} pilots, expected at least {min_pilots}: "
            "the suite would pass while covering less than it claims"
        )
    return names


def run_pilot(executable: Path, name: str, log_dir: Path) -> PilotRun:
    log_path = log_dir / f"{name.rsplit('::', 1)[-1]}.log"
    start = time.monotonic()
    with log_path.open("w", encoding="utf-8") as log:
        # `--nocapture` keeps the harness phase markers in the log even on
        # success: they are what makes a slow or failed pilot diagnosable.
        completed = subprocess.run(
            [str(executable), "--exact", name, "--ignored", "--nocapture"],
            stdout=log,
            stderr=subprocess.STDOUT,
        )
    return PilotRun(
        name=name,
        ok=completed.returncode == 0,
        seconds=time.monotonic() - start,
        log_path=log_path,
    )


def report_failures(runs: list[PilotRun], tail_lines: int = 30) -> None:
    for run in runs:
        print(f"\n[pilot] FAILED {run.name} (log: {run.log_path})")
        try:
            lines = run.log_path.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError as error:
            print(f"[pilot]   log unavailable: {error}")
            continue
        for line in lines[-tail_lines:]:
            print(f"[pilot]   {line}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--jobs",
        type=int,
        default=int(os.environ.get("D4LC_PILOT_JOBS") or min(6, os.cpu_count() or 4)),
        help="pilots to keep in flight (default: min(6, cpus), env D4LC_PILOT_JOBS)",
    )
    parser.add_argument(
        "--min-tests",
        type=int,
        default=DEFAULT_MIN_PILOTS,
        help=f"fail when fewer pilots are enumerated (default: {DEFAULT_MIN_PILOTS})",
    )
    parser.add_argument(
        "--filter",
        default="",
        help="only run pilots whose name contains this substring",
    )
    parser.add_argument(
        "--skip-build",
        action="store_true",
        help="trust an already built test binary (fails when it is missing)",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="only print the pilots that would run",
    )
    args = parser.parse_args()

    if args.jobs < 1:
        parser.error("--jobs must be at least 1")

    log_dir = REPO_ROOT / DEFAULT_LOG_DIR
    log_dir.mkdir(parents=True, exist_ok=True)

    if args.skip_build:
        candidates = sorted((REPO_ROOT / "target/debug/deps").glob("d4lc_app-*"))
        executables = [path for path in candidates if path.is_file() and os.access(path, os.X_OK)]
        if not executables:
            raise SystemExit("[pilot] --skip-build found no built pilot test binary")
        executable = max(executables, key=lambda path: path.stat().st_mtime)
    else:
        executable = build_test_binary()

    pilots = list_pilots(executable, args.min_tests)
    if args.filter:
        pilots = [name for name in pilots if args.filter in name]
        if not pilots:
            raise SystemExit(f"[pilot] no pilot matches --filter {args.filter!r}")

    if args.list:
        for name in pilots:
            print(name)
        return 0

    print(
        f"[pilot] {len(pilots)} pilots, {args.jobs} in flight, "
        f"binary {executable.name}"
    )
    start = time.monotonic()
    runs: list[PilotRun] = []
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = [pool.submit(run_pilot, executable, name, log_dir) for name in pilots]
        for future in futures:
            run = future.result()
            runs.append(run)
            status = "ok  " if run.ok else "FAIL"
            short = run.name.rsplit("::", 1)[-1]
            print(f"[pilot] {status} {run.seconds:7.1f}s  {short}", flush=True)

    total = time.monotonic() - start
    failed = [run for run in runs if not run.ok]
    slowest = sorted(runs, key=lambda run: -run.seconds)[:5]

    print(
        f"\n[pilot] {len(runs) - len(failed)}/{len(runs)} passed in {total:.1f}s "
        f"wall clock ({args.jobs} in flight); slowest: "
        + ", ".join(f"{run.name.rsplit('::', 1)[-1]} {run.seconds:.1f}s" for run in slowest)
    )

    if failed:
        report_failures(failed)
        print(f"\n[pilot] {len(failed)} of {len(runs)} pilots failed")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
