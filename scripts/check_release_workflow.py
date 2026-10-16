#!/usr/bin/env python3
"""Check the release gating of .github/workflows/{verify,release}.yml.

The contract (a release is the verified build of one commit):
- verify.yml can be called by other workflows (`workflow_call`), runs the
  full tier of scripts/verify_local.sh, cargo-deny and the
  minimum-Rust-version build, and none of its jobs or steps can be skipped
  or allowed to fail;
- release.yml calls verify.yml as its `verify` job; packaging needs it, the
  .deb install test needs packaging, and the publishing job needs all
  three directly;
- no job or step in either workflow can turn a failed or skipped gate into
  a pass: no `continue-on-error`, no `always()`/`failure()`/`cancelled()`
  conditions, no step-level `if`, no errexit switched off or other shell,
  no OR list whose alternative does not fail the step, and only the
  publishing job carries a job-level `if` (the preflight switch);
- the packaging job verifies the build identity, smokes the artifacts,
  checks SHA256SUMS and the complete non-empty artifact set before its
  upload; the install test and the publish script re-verify the digests of
  the downloaded artifacts;
- permissions are explicit and minimal: read-only at the workflow level,
  every job declares a mapping, and `contents: write` exists only on the
  publishing job, which is also the only job that sees a GitHub token;
- no checkout pins another `ref` or `repository`, every action is pinned
  to a full commit SHA from a short allow-list;
- release mutations go through scripts/publish_draft_release.sh only
  (bound to `$GITHUB_SHA`), never through `gh release`, a mutating
  `gh api` call, raw API requests or a release action; publishing is
  serialized by a fixed, ref-independent concurrency group;
- a manual run defaults to preflight (no release mutation).

`--self-test` additionally checks that broken variants of the real
workflows are rejected.

Exit codes: 0 = contract holds, 1 = violations found, 2 = cannot check.
"""

from __future__ import annotations

import copy
import re
import sys
from pathlib import Path

try:
    import yaml
except ImportError:  # pragma: no cover - environment problem, not a pass
    print("check_release_workflow: PyYAML is required (python3-yaml)", file=sys.stderr)
    raise SystemExit(2)

REPO_ROOT = Path(__file__).resolve().parent.parent
WORKFLOWS = REPO_ROOT / ".github" / "workflows"

PUBLISH_JOB = "publish-draft"
PACKAGE_JOB = "package-linux-release"
INSTALL_JOB = "install-test-deb"
PUBLISH_IF = (
    "github.event_name == 'push' || "
    "(github.event_name == 'workflow_dispatch' && inputs.preflight == false)"
)
ALLOWED_ACTIONS = {
    "actions/checkout",
    "actions/upload-artifact",
    "actions/download-artifact",
    "dtolnay/rust-toolchain",
    "taiki-e/install-action",
}
PINNED_USES = re.compile(r"^([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)@[0-9a-f]{40}$")
LOCAL_WORKFLOW = "./.github/workflows/verify.yml"
CHECKSUMS = "./scripts/verify_release_checksums.sh"

GH_RELEASE_MUTATION = re.compile(r"\bgh\s+release\s+(upload|edit|create|delete)\b")
# `gh api` writes with an explicit method or implicitly with -f/-F/--input.
GH_API_WRITE = re.compile(
    r"\bgh\s+api\b[^\n]*?(\s-X\s*(?!GET\b)\w+|\s--method[=\s]+(?!GET\b)\w+"
    r"|\s-[fF]\s|\s--(raw-)?field[=\s]|\s--input[=\s])"
)
RAW_GITHUB_API = re.compile(r"(api|uploads)\.github\.com")
# Turning errexit off would let a failed gate pass.
# Gating steps only ever set shell options; any `set` that clears one
# (`+e`, `+eu`, `+o errexit`, quoted or not) is refused. Fail-closed: a
# message that merely contains such text is refused too.
ERREXIT_OFF = re.compile(r"\bset\b[^;&|#]*\s[\"']?\+")
# The only accepted OR-list alternative: report and fail the step with
# `exit 1`, nothing after it. Anything looser (a word "false" in a message,
# "exit 1" in a comment, `exit 256`, which wraps to 0) could hide a failure.
FAILING_ALTERNATIVE = re.compile(
    r'^\s*\(\s*echo\s+"[^"`$]*"\s*(>&2\s*)?;\s*exit\s+1\s*\)\s*$'
)
# The shell every gating step runs in (`bash -eo pipefail` on GitHub).
GATE_SHELL = "bash"
EXPECTED_ARTIFACTS = "./scripts/check_expected_artifacts.sh"
# Informational commands whose failure is genuinely irrelevant.
ALLOWED_SWALLOW_LINES = {'"$HOME/.local/bin/appimagetool" --version || true'}
BYPASS_CONDITION = re.compile(r"\b(always|failure|cancelled)\s*\(")
TOKEN_REFERENCE = re.compile(r"\bsecrets\.|\bgithub\.token\b|\bGITHUB_TOKEN\b")


def triggers(workflow: dict) -> dict:
    # YAML 1.1 reads the bare key `on` as boolean true.
    value = workflow.get("on", workflow.get(True))
    if isinstance(value, dict):
        return value
    if isinstance(value, list):
        return {name: None for name in value}
    return {value: None} if isinstance(value, str) else {}


def needs(job: dict) -> list[str]:
    value = job.get("needs", [])
    return [value] if isinstance(value, str) else list(value)


def steps(workflow: dict):
    for job_name, job in workflow.get("jobs", {}).items():
        for step in job.get("steps", []) or []:
            yield job_name, step


def job_steps(job: dict) -> list[dict]:
    return [step for step in job.get("steps", []) or [] if isinstance(step, dict)]


def run_lines(run: str) -> list[str]:
    return [line.strip() for line in run.splitlines() if line.strip()]


def step_index(job: dict, predicate) -> int | None:
    for index, step in enumerate(job_steps(job)):
        if predicate(step):
            return index
    return None


def runs_command(command: str):
    """A step whose run has `command` as one of its own command lines."""

    def predicate(step: dict) -> bool:
        run = step.get("run")
        return isinstance(run, str) and any(
            line == command or line.startswith(command + " ") for line in run_lines(run)
        )

    return predicate


def mentions(text: str):
    def predicate(step: dict) -> bool:
        run = step.get("run")
        return isinstance(run, str) and text in run

    return predicate


def check_permissions(name: str, scope: str, permissions, problems: list[str]) -> dict:
    if permissions is None:
        problems.append(f"{name}: {scope} declares no permissions (the default may be write)")
        return {}
    if not isinstance(permissions, dict):
        problems.append(f"{name}: {scope} uses blanket permissions {permissions!r}")
        return {}
    for key, value in permissions.items():
        if value not in ("read", "none"):
            problems.append(f"{name}: {scope} grants {key}: {value}")
    return permissions


def swallows(line: str) -> bool:
    """A command line that can turn a failure into success.

    An OR list passes only when its alternative fails the step (`|| (echo
    ...; exit 1)`) or it is the condition of an `if`/`elif`/`while`.
    """
    if line in ALLOWED_SWALLOW_LINES:
        return False
    if ERREXIT_OFF.search(line):
        return True
    shell = re.sub(r"\$\{\{.*?\}\}", "", line)
    if "||" not in shell:
        return False
    if re.match(r"(if|elif|while|until)\s", shell) and shell.rstrip().endswith("then"):
        return False
    alternatives = shell.split("||")[1:]
    return not all(FAILING_ALTERNATIVE.match(alternative) for alternative in alternatives)


def check_shell(name: str, scope: str, defaults, problems: list[str]) -> None:
    shell = ((defaults or {}).get("run") or {}).get("shell", GATE_SHELL)
    if shell != GATE_SHELL:
        problems.append(f"{name}: {scope} runs its steps in shell {shell!r}")


def check_common(name: str, workflow: dict, problems: list[str]) -> None:
    """Rules that hold for every job and step of a gating workflow."""
    jobs = workflow.get("jobs", {})
    check_shell(name, "the workflow", workflow.get("defaults"), problems)
    for job_name, job in jobs.items():
        check_shell(name, f"job {job_name}", job.get("defaults"), problems)
        if job.get("continue-on-error") not in (None, False):
            problems.append(f"{name} job {job_name} sets continue-on-error")
        condition = job.get("if")
        if condition is not None and BYPASS_CONDITION.search(str(condition)):
            problems.append(f"{name} job {job_name} runs regardless of failed needs: {condition}")
        for step in job_steps(job):
            label = step.get("name") or step.get("uses") or step.get("run", "")[:40]
            if step.get("continue-on-error") not in (None, False):
                problems.append(f"{name} job {job_name} step {label!r} sets continue-on-error")
            if "if" in step:
                problems.append(f"{name} job {job_name} step {label!r} is conditional")
            if step.get("shell", GATE_SHELL) != GATE_SHELL:
                problems.append(f"{name} job {job_name} step {label!r} runs in shell {step['shell']!r}")
            uses = step.get("uses")
            if uses is not None:
                match = PINNED_USES.match(str(uses))
                if not match:
                    problems.append(f"{name} job {job_name} uses unpinned action {uses}")
                elif match.group(1) not in ALLOWED_ACTIONS:
                    problems.append(f"{name} job {job_name} uses unexpected action {uses}")
                if match and match.group(1) == "actions/checkout":
                    with_block = step.get("with") or {}
                    for key in ("ref", "repository"):
                        if key in with_block:
                            problems.append(f"{name} job {job_name} checks out a pinned {key}")
            run = step.get("run")
            if isinstance(run, str):
                for line in run_lines(run):
                    if swallows(line):
                        problems.append(
                            f"{name} job {job_name} swallows a failure: {line!r}"
                        )
                if GH_RELEASE_MUTATION.search(run) or GH_API_WRITE.search(run):
                    problems.append(
                        f"{name} job {job_name} mutates GitHub state outside "
                        "scripts/publish_draft_release.sh"
                    )
                if RAW_GITHUB_API.search(run):
                    problems.append(f"{name} job {job_name} calls the GitHub API directly")


def check_verify(verify: dict) -> list[str]:
    problems: list[str] = []
    name = "verify.yml"
    if "workflow_call" not in triggers(verify):
        problems.append("verify.yml has no workflow_call trigger")
    check_permissions(name, "the workflow", verify.get("permissions"), problems)
    jobs = verify.get("jobs", {})
    for job_name, job in jobs.items():
        check_permissions(name, f"job {job_name}", job.get("permissions"), problems)
        if "if" in job:
            problems.append(f"verify.yml job {job_name} is conditional")
    check_common(name, verify, problems)

    verify_job = jobs.get("verify")
    if verify_job is None:
        problems.append("verify.yml has no verify job")
    else:
        verification = step_index(verify_job, runs_command("./scripts/verify_local.sh"))
        if verification is None:
            problems.append("verify.yml does not run ./scripts/verify_local.sh")
        desktop_utils = step_index(
            verify_job,
            lambda step: runs_command("sudo apt-get install")(step)
            and "desktop-file-utils" in step["run"].split(),
        )
        if desktop_utils is None or (verification is not None and desktop_utils >= verification):
            problems.append("verify.yml must install desktop-file-utils before local verification")
        for step in job_steps(verify_job):
            run = step.get("run")
            if isinstance(run, str):
                for line in run_lines(run):
                    if line.startswith("./scripts/verify_local.sh") and line != (
                        "./scripts/verify_local.sh"
                    ):
                        problems.append(
                            f"verify.yml does not run the full tier of verify_local.sh: {line!r}"
                        )
        if step_index(verify_job, runs_command("cargo deny check")) is None:
            problems.append("verify.yml does not run cargo deny check")
    msrv = jobs.get("msrv")
    if msrv is None:
        problems.append("verify.yml has no msrv job")
    elif step_index(msrv, runs_command("cargo check --locked --workspace --all-targets")) is None:
        problems.append("verify.yml msrv job does not build the workspace")
    return problems


def check_release(release: dict) -> list[str]:
    problems: list[str] = []
    name = "release.yml"
    on = triggers(release)
    tags = ((on.get("push") or {}).get("tags")) if isinstance(on.get("push"), dict) else None
    if not tags:
        problems.append("release.yml is not triggered by version tags")
    dispatch = on.get("workflow_dispatch") or {}
    preflight = (dispatch.get("inputs") or {}).get("preflight") if isinstance(dispatch, dict) else None
    if "workflow_dispatch" in on and (not preflight or preflight.get("default") is not True):
        problems.append("a manual release run does not default to preflight")

    workflow_permissions = release.get("permissions")
    check_permissions(name, "the workflow", workflow_permissions, problems)
    jobs = release.get("jobs", {})
    writers = []
    for job_name, job in jobs.items():
        permissions = job.get("permissions")
        if permissions is None:
            problems.append(f"release.yml job {job_name} declares no permissions")
            continue
        if not isinstance(permissions, dict):
            problems.append(f"release.yml job {job_name} uses blanket permissions {permissions!r}")
            writers.append(job_name)
            continue
        for key, value in permissions.items():
            if value in ("read", "none"):
                continue
            if job_name == PUBLISH_JOB and key == "contents" and value == "write":
                writers.append(job_name)
            else:
                problems.append(f"release.yml job {job_name} grants {key}: {value}")
        if job_name != PUBLISH_JOB and "if" in job:
            problems.append(f"release.yml job {job_name} is conditional")
        if job_name != PUBLISH_JOB and TOKEN_REFERENCE.search(yaml.safe_dump(job)):
            problems.append(f"release.yml job {job_name} receives a GitHub token")
    check_common(name, release, problems)

    verify_job = jobs.get("verify")
    if not verify_job or verify_job.get("uses") != LOCAL_WORKFLOW:
        problems.append("release.yml has no `verify` job calling ./.github/workflows/verify.yml")
    package = jobs.get(PACKAGE_JOB)
    install = jobs.get(INSTALL_JOB)
    publish = jobs.get(PUBLISH_JOB)
    if package is None:
        problems.append("release.yml has no package-linux-release job")
    elif "verify" not in needs(package):
        problems.append("package-linux-release does not need verify")
    if install is None:
        problems.append("release.yml has no install-test-deb job")
    elif PACKAGE_JOB not in needs(install):
        problems.append("install-test-deb does not need package-linux-release")

    if package is not None:
        order = [
            ("the packaging script", mentions("./scripts/package_linux_release.sh")),
            ("the build identity check", mentions('!= "$GITHUB_SHA"')),
            ("the packaged-artifact smoke", runs_command("./scripts/smoke_release_artifacts.sh")),
            ("the SHA256SUMS check", runs_command(CHECKSUMS)),
            ("the expected-artifact check", runs_command(EXPECTED_ARTIFACTS)),
        ]
        upload = step_index(package, lambda s: str(s.get("uses", "")).startswith("actions/upload-artifact@"))
        previous = -1
        for label, predicate in order:
            index = step_index(package, predicate)
            if index is None:
                problems.append(f"package-linux-release does not run {label}")
                continue
            if index < previous:
                problems.append(f"package-linux-release runs {label} out of order")
            if upload is not None and index > upload:
                problems.append(f"package-linux-release uploads before {label}")
            previous = index
        if upload is None:
            problems.append("package-linux-release uploads no artifacts")
    if install is not None:
        if step_index(install, runs_command(CHECKSUMS)) is None:
            problems.append("install-test-deb does not verify the downloaded digests")
        if step_index(install, runs_command("./scripts/smoke_deb_install.sh")) is None:
            problems.append("install-test-deb does not run scripts/smoke_deb_install.sh")

    if writers != [PUBLISH_JOB]:
        problems.append(
            f"exactly the {PUBLISH_JOB} job may write contents, found {writers or 'none'}"
        )
    if publish is not None:
        missing = {"verify", PACKAGE_JOB, INSTALL_JOB} - set(needs(publish))
        if missing:
            problems.append(f"{PUBLISH_JOB} does not directly need {sorted(missing)}")
        if publish.get("if") != PUBLISH_IF:
            problems.append(f"{PUBLISH_JOB} is not gated by the preflight switch: {publish.get('if')!r}")
        concurrency = publish.get("concurrency")
        group = concurrency.get("group") if isinstance(concurrency, dict) else concurrency
        if not isinstance(group, str) or "${{" in group:
            problems.append(f"{PUBLISH_JOB} has no fixed, ref-independent concurrency group")
        elif isinstance(concurrency, dict) and concurrency.get("cancel-in-progress") not in (None, False):
            problems.append(f"{PUBLISH_JOB} may cancel a publish in progress")
        publish_runs = [
            step.get("run", "")
            for step in job_steps(publish)
            if isinstance(step.get("run"), str) and "publish_draft_release.sh" in step["run"]
        ]
        if not publish_runs:
            problems.append("release.yml never runs scripts/publish_draft_release.sh")
        elif not all('--sha "$GITHUB_SHA"' in run for run in publish_runs):
            problems.append("publish_draft_release.sh is not bound to the run commit ($GITHUB_SHA)")
    for job_name, job in jobs.items():
        if job_name == PUBLISH_JOB:
            continue
        for step in job_steps(job):
            if "publish_draft_release.sh" in str(step.get("run", "")):
                problems.append(f"release.yml job {job_name} publishes outside {PUBLISH_JOB}")
    return problems


def check(verify: dict, release: dict) -> list[str]:
    return check_verify(verify) + check_release(release)


def load(name: str) -> dict:
    with open(WORKFLOWS / name, encoding="utf-8") as handle:
        return yaml.safe_load(handle)


def first_step(workflow: dict, predicate):
    for _, step in steps(workflow):
        if predicate(step):
            return step
    raise LookupError("self-test: the real workflow lacks the step this mutant edits")


def append_to_run(workflow: dict, text: str, suffix: str) -> None:
    step = first_step(workflow, mentions(text))
    step["run"] = step["run"].rstrip("\n") + suffix


def mutants():
    """Broken variants of the real workflows; every one must be rejected."""

    def pub(r):
        return r["jobs"][PUBLISH_JOB]

    def pkg(r):
        return r["jobs"][PACKAGE_JOB]

    def drop_verify_need(_v, r):
        pkg(r).pop("needs", None)
        pub(r)["needs"] = [INSTALL_JOB]

    def drop_call_trigger(v, _r):
        triggers(v).pop("workflow_call", None)

    def pin_ref(_v, r):
        first_step(r, lambda s: str(s.get("uses", "")).startswith("actions/checkout@")).setdefault(
            "with", {}
        )["ref"] = "main"

    def other_repository(_v, r):
        first_step(r, lambda s: str(s.get("uses", "")).startswith("actions/checkout@")).setdefault(
            "with", {}
        )["repository"] = "someone/else"

    def raw_clobber(_v, r):
        pub(r)["steps"].append({"run": 'gh release upload "$TAG" a.tar.gz --clobber'})

    def unbound_publish(_v, r):
        step = first_step(r, mentions("publish_draft_release.sh"))
        step["run"] = step["run"].replace('--sha "$GITHUB_SHA"', '--sha "$SOME_SHA"')

    def fast_tier(v, _r):
        step = first_step(v, mentions("verify_local.sh"))
        step["run"] = step["run"].replace("verify_local.sh", "verify_local.sh --fast")

    def echoed_tier(v, _r):
        step = first_step(v, mentions("verify_local.sh"))
        step["run"] = "echo ./scripts/verify_local.sh"

    def drop_desktop_utils(v, _r):
        step = first_step(v, mentions("desktop-file-utils"))
        step["run"] = step["run"].replace("desktop-file-utils", "")

    def late_desktop_utils(v, _r):
        items = v["jobs"]["verify"]["steps"]
        step = first_step(v, mentions("desktop-file-utils"))
        items.remove(step)
        items.append(step)

    def drop_msrv(v, _r):
        v["jobs"].pop("msrv", None)

    def drop_deny(v, _r):
        v["jobs"]["verify"]["steps"] = [
            s for s in job_steps(v["jobs"]["verify"]) if "cargo deny" not in str(s.get("run", ""))
        ]

    def publish_always(_v, r):
        pub(r)["if"] = "always()"

    def publish_not_cancelled(_v, r):
        pub(r)["if"] = "${{ !cancelled() }}"

    def publish_without_preflight(_v, r):
        pub(r).pop("if")

    def package_continue(_v, r):
        pkg(r)["continue-on-error"] = True

    def verify_step_continue(v, _r):
        first_step(v, mentions("verify_local.sh"))["continue-on-error"] = True

    def verify_or_true(v, _r):
        first_step(v, mentions("verify_local.sh"))["run"] = "./scripts/verify_local.sh || true"

    def verify_set_plus_e(v, _r):
        step = first_step(v, mentions("verify_local.sh"))
        step["run"] = "set +e\n" + step["run"]

    def smoke_or_true(_v, r):
        append_to_run(r, "smoke_release_artifacts.sh", " || true")

    def deb_smoke_or_exit0(_v, r):
        append_to_run(r, "smoke_deb_install.sh", " || exit 0")

    def conditional_smoke(_v, r):
        first_step(r, mentions("smoke_release_artifacts.sh"))["if"] = "github.event_name == 'push'"

    def drop_smoke(_v, r):
        pkg(r)["steps"] = [
            s for s in job_steps(pkg(r)) if "smoke_release_artifacts" not in str(s.get("run", ""))
        ]

    def drop_identity(_v, r):
        pkg(r)["steps"] = [s for s in job_steps(pkg(r)) if "GITHUB_SHA" not in str(s.get("run", ""))]

    def drop_sums_check(_v, r):
        for job in (pkg(r), r["jobs"][INSTALL_JOB]):
            job["steps"] = [
                s for s in job_steps(job) if CHECKSUMS not in str(s.get("run", ""))
            ]

    def drop_expected_assets(_v, r):
        pkg(r)["steps"] = [
            s for s in job_steps(pkg(r)) if EXPECTED_ARTIFACTS not in str(s.get("run", ""))
        ]

    def echoed_expected_assets(_v, r):
        step = first_step(r, mentions(EXPECTED_ARTIFACTS))
        step["run"] = step["run"].replace(EXPECTED_ARTIFACTS, "echo " + EXPECTED_ARTIFACTS)

    def checksum_or_warning(_v, r):
        append_to_run(r, CHECKSUMS, ' || echo "::warning::ignoring checksum failure"')

    def expected_or_colon_list(_v, r):
        append_to_run(r, EXPECTED_ARTIFACTS, " || { echo skipped; }")

    def checksum_or_false_word(_v, r):
        append_to_run(r, CHECKSUMS, ' || echo "checksum false"')

    def checksum_or_commented_exit(_v, r):
        append_to_run(r, CHECKSUMS, ' || echo "warning" # exit 1')

    def combined_set_plus_eu(_v, r):
        step = first_step(r, mentions(CHECKSUMS))
        step["run"] = "set +eu\n" + step["run"] + "\necho done"

    def checksum_or_fail_then_true(_v, r):
        append_to_run(r, CHECKSUMS, ' || (echo "x" >&2; exit 1) || true')

    def checksum_or_exit_256(_v, r):
        append_to_run(r, CHECKSUMS, ' || (echo "failed"; exit 256)')

    def quoted_errexit_off(_v, r):
        step = first_step(r, mentions(CHECKSUMS))
        step["run"] = "set +o 'errexit'\n" + step["run"] + "\necho done"

    def errexit_off(_v, r):
        step = first_step(r, mentions("smoke_release_artifacts.sh"))
        step["run"] = "set +o errexit\n" + step["run"]

    def step_shell_without_errexit(_v, r):
        first_step(r, mentions("smoke_release_artifacts.sh"))["shell"] = "bash {0}"

    def job_shell_without_errexit(_v, r):
        pkg(r)["defaults"] = {"run": {"shell": "sh {0}"}}

    def upload_before_smoke(_v, r):
        items = job_steps(pkg(r))
        upload = next(s for s in items if str(s.get("uses", "")).startswith("actions/upload-artifact@"))
        items.remove(upload)
        smoke = next(i for i, s in enumerate(items) if "smoke_release_artifacts" in str(s.get("run", "")))
        items.insert(smoke, upload)
        pkg(r)["steps"] = items

    def publish_write_all_without_needs(_v, r):
        pub(r)["permissions"] = "write-all"
        pub(r)["needs"] = []

    def top_level_write_all(_v, r):
        r["permissions"] = "write-all"
        r["jobs"][INSTALL_JOB].pop("permissions")

    def top_level_missing(_v, r):
        r.pop("permissions", None)

    def job_without_permissions(_v, r):
        r["jobs"][INSTALL_JOB].pop("permissions")

    def package_writes(_v, r):
        pkg(r)["permissions"] = {"contents": "write"}

    def extra_scope(_v, r):
        pub(r)["permissions"] = {"contents": "write", "actions": "write"}

    def api_post_release(_v, r):
        pkg(r)["steps"].append({"run": "gh api -X POST repos/$GH_REPO/releases -f tag_name=v1"})

    def api_implicit_post(_v, r):
        pkg(r)["steps"].append({"run": "gh api repos/$GH_REPO/releases -f tag_name=v1"})

    def api_patch_in_publish(_v, r):
        pub(r)["steps"].append({"run": "gh api --method PATCH repos/o/r/releases/1 -F draft=false"})

    def curl_api(_v, r):
        pub(r)["steps"].append(
            {"run": 'curl -X POST -H "Authorization: token $GH_TOKEN" https://api.github.com/repos/o/r/releases'}
        )

    def release_action(_v, r):
        pkg(r)["steps"].append({"uses": "softprops/action-gh-release@v2"})

    def pinned_release_action(_v, r):
        pkg(r)["steps"].append({"uses": "softprops/action-gh-release@" + "a" * 40})

    def unpinned_checkout(_v, r):
        first_step(r, lambda s: str(s.get("uses", "")).startswith("actions/checkout@"))["uses"] = (
            "actions/checkout@v4"
        )

    def rogue_writer(_v, r):
        r["jobs"]["rogue"] = {
            "runs-on": "x",
            "permissions": "write-all",
            "steps": [{"run": "gh api -X PATCH repos/o/r/releases/1"}],
        }

    def rogue_publisher(_v, r):
        r["jobs"]["rogue"] = {
            "runs-on": "x",
            "permissions": {"contents": "read"},
            "needs": ["verify", PACKAGE_JOB, INSTALL_JOB],
            "if": "always()",
            "steps": [{"run": './scripts/publish_draft_release.sh --sha "$GITHUB_SHA"'}],
        }

    def token_in_package(_v, r):
        pkg(r).setdefault("env", {})["GH_TOKEN"] = "${{ github.token }}"

    def ref_concurrency(_v, r):
        pub(r)["concurrency"] = {"group": "release-${{ github.ref }}", "cancel-in-progress": False}

    def no_concurrency(_v, r):
        pub(r).pop("concurrency", None)

    def dispatch_publishes_by_default(_v, r):
        triggers(r)["workflow_dispatch"]["inputs"]["preflight"]["default"] = False

    def conditional_verify_job(v, _r):
        v["jobs"]["verify"]["if"] = "github.event_name != 'workflow_call'"

    def skipped_install(_v, r):
        r["jobs"][INSTALL_JOB]["if"] = "false"

    def publish_drops_install(_v, r):
        pub(r)["needs"] = ["verify", PACKAGE_JOB]

    return [
        ("packaging without verify", drop_verify_need),
        ("verify.yml without workflow_call", drop_call_trigger),
        ("checkout of a pinned ref", pin_ref),
        ("checkout of another repository", other_repository),
        ("a raw `gh release upload --clobber`", raw_clobber),
        ("publishing without the run commit", unbound_publish),
        ("the fast verify tier", fast_tier),
        ("verify_local.sh only echoed", echoed_tier),
        ("verify without desktop-file-utils", drop_desktop_utils),
        ("desktop-file-utils installed after verification", late_desktop_utils),
        ("verify without the msrv job", drop_msrv),
        ("verify without cargo deny", drop_deny),
        ("publish if: always()", publish_always),
        ("publish if: !cancelled()", publish_not_cancelled),
        ("publish without the preflight gate", publish_without_preflight),
        ("package job continue-on-error", package_continue),
        ("verify step continue-on-error", verify_step_continue),
        ("verify_local.sh || true", verify_or_true),
        ("verify under set +e", verify_set_plus_e),
        ("artifact smoke || true", smoke_or_true),
        ("deb smoke || exit 0", deb_smoke_or_exit0),
        ("conditional artifact smoke", conditional_smoke),
        ("no artifact smoke", drop_smoke),
        ("no build identity check", drop_identity),
        ("no SHA256SUMS check", drop_sums_check),
        ("no expected-artifact check", drop_expected_assets),
        ("expected-artifact check only echoed", echoed_expected_assets),
        ("checksum check || echo warning", checksum_or_warning),
        ("expected-artifact check || { echo; }", expected_or_colon_list),
        ("errexit turned off", errexit_off),
        ("checksum || echo with the word false", checksum_or_false_word),
        ("checksum || echo with exit 1 in a comment", checksum_or_commented_exit),
        ("combined set +eu", combined_set_plus_eu),
        ("checksum || exit 256 (wraps to 0)", checksum_or_exit_256),
        ("quoted set +o 'errexit'", quoted_errexit_off),
        ("checksum || failing echo; then success", checksum_or_fail_then_true),
        ("step shell without errexit", step_shell_without_errexit),
        ("job shell without errexit", job_shell_without_errexit),
        ("upload before the smoke", upload_before_smoke),
        ("publish write-all without needs", publish_write_all_without_needs),
        ("top-level write-all", top_level_write_all),
        ("no top-level permissions", top_level_missing),
        ("job without permissions", job_without_permissions),
        ("packaging job with contents: write", package_writes),
        ("publish job with an extra write scope", extra_scope),
        ("gh api -X POST releases", api_post_release),
        ("gh api -f (implicit POST)", api_implicit_post),
        ("gh api --method PATCH in publish", api_patch_in_publish),
        ("curl against api.github.com", curl_api),
        ("third-party release action", release_action),
        ("pinned third-party release action", pinned_release_action),
        ("unpinned checkout", unpinned_checkout),
        ("extra write-all job", rogue_writer),
        ("extra publishing job with if: always()", rogue_publisher),
        ("token in the packaging job", token_in_package),
        ("ref-dependent publish concurrency", ref_concurrency),
        ("publish without concurrency", no_concurrency),
        ("manual run publishes by default", dispatch_publishes_by_default),
        ("conditional verify job", conditional_verify_job),
        ("skipped install test", skipped_install),
        ("publish without the install test", publish_drops_install),
    ]


def self_test(verify: dict, release: dict) -> list[str]:
    failures: list[str] = []
    for label, mutate in mutants():
        broken_verify, broken_release = copy.deepcopy(verify), copy.deepcopy(release)
        try:
            mutate(broken_verify, broken_release)
        except (LookupError, KeyError, StopIteration) as error:
            failures.append(f"self-test: cannot build mutant {label!r}: {error!r}")
            continue
        if not check(broken_verify, broken_release):
            failures.append(f"self-test: {label} was not rejected")
    return failures


def main() -> int:
    try:
        verify, release = load("verify.yml"), load("release.yml")
    except (OSError, yaml.YAMLError) as error:
        print(f"check_release_workflow: cannot read workflows: {error}", file=sys.stderr)
        return 2
    problems = check(verify, release)
    if "--self-test" in sys.argv[1:]:
        problems += self_test(verify, release)
    for problem in problems:
        print(f"check_release_workflow: {problem}", file=sys.stderr)
    if problems:
        return 1
    count = f" ({len(mutants())} mutants rejected)" if "--self-test" in sys.argv[1:] else ""
    print(f"check_release_workflow: ok{count}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
