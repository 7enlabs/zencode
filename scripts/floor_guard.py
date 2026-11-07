#!/usr/bin/env python3
"""Floor guard: block pull requests that loosen the checks instead of fixing the code.

Reads CONSTRAINTS.md's rules as code. Three kinds of check:

* Floor (diff-only): looks only at lines a change adds or removes.
  - F1  a new test skip or lint/warning silencer in Rust (`#[ignore`, `#[allow(`,
        `#![allow(`) or a new stub (`todo!(`, `unimplemented!(`), unless the same
        line or the added line right above it carries `floor-guard: <reason>`.
  - F2  a workflow made softer: `continue-on-error: true`, `|| true`, or a removed
        `-D warnings` that no added line puts back.
  - F3  a removed or changed line inside CONSTRAINTS.md's "## Floor" or
        "## Task guard" section, or a lowered number in ci/floor-baseline.json.
* Ratchet (whole tree): the number of Rust test functions must not fall below
  ci/floor-baseline.json's `min_rust_tests`.
* Task guard (P0-3 section 4.3 layer 2, section 6 step 5): one task = one
  branch = one pull request.
  - T1  the branch name or the PR body must name a task code (B-N, STU-N,
        APP-N, DV-N, DS-N, C-N, `Việc N`, `P0-N step M`). A step code
        (`P0-3 step 6`) supersedes the bare project code (`P0-3`) in the
        same text, so two steps of one project are not flagged as the same task.
  - T2  two open pull requests sharing a task code are flagged; the Leader
        decides which one goes on. Needs the open-PR list (`--prs`, JSON from
        `gh pr list --state open --json number,headRefName,body`); without it
        T2 is skipped with a notice.
  - Warn-only: until the date in `--warn-until` (or the
        FLOOR_GUARD_TASK_WARN_UNTIL environment variable), T findings are
        warnings and never fail the check. After that date they fail unless
        the pull request carries the `floor-change-approved` label.
  - Dependabot branches are exempt: machine-made, no task code applies.

A pull request labelled `floor-change-approved` (set by a human reviewer) turns every
finding into a warning; nothing else does. Stdlib only.

Usage: floor_guard.py [--base REF] [--event PATH] [--branch NAME]
       [--pr-body-file PATH] [--pr-number N] [--prs PATH] [--warn-until YYYY-MM-DD]
Exit: 0 pass, 1 floor, ratchet or (after warn-only) task-guard failure,
2 usage error.
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASELINE = "ci/floor-baseline.json"
APPROVE_LABEL = "floor-change-approved"
MARKER = re.compile(r"floor-guard:\s*\S.{4,}")

RUST_LOOSEN = [
    (re.compile(r"#\[\s*ignore\b"), "test skipped with #[ignore]"),
    (re.compile(r"#\!?\[\s*allow\s*\("), "lint or warning silenced with #[allow(...)]"),
    (re.compile(r"\btodo!\s*\("), "stub added with todo!()"),
    (re.compile(r"\bunimplemented!\s*\("), "stub added with unimplemented!()"),
]
CI_LOOSEN = [
    (re.compile(r"continue-on-error:\s*true\b"), "step or job set to continue-on-error: true"),
    (re.compile(r"\|\|\s*true\b"), "command failure swallowed with || true"),
]
TEST_FN = re.compile(r"#\[(?:test|tokio::test)\b")

# Task codes (P0-3 section 4.3 layer 2). Canonical form is upper-case, no spaces.
TASK_GENERIC = re.compile(r"\b[A-Z]{1,4}-\d+\b", re.IGNORECASE)
TASK_VIEC = re.compile(r"\bviệc\s*(\d+)\b", re.IGNORECASE)
TASK_STEP = re.compile(r"\bP0-(\d+)[-\s]*(?:step|bước)[-\s]*(\d+)\b", re.IGNORECASE)
TASK_PSTEP = re.compile(r"\bp0(\d)[-\s]?s(?:tep)?[-\s]*(\d+)\b", re.IGNORECASE)
EXEMPT_BRANCH_PREFIXES = ("dependabot/",)


@dataclass
class Finding:
    rule: str
    path: str
    line: int
    msg: str


@dataclass
class FileDiff:
    path: str
    added: list[tuple[int, str]]
    removed: list[tuple[int, str]]


def parse_diff(text: str) -> list[FileDiff]:
    """Parse `git diff -U0` output into added/removed lines with line numbers."""
    files: list[FileDiff] = []
    cur: FileDiff | None = None
    old_no = new_no = 0
    for raw in text.splitlines():
        if raw.startswith("diff --git "):
            cur = None
            continue
        if raw.startswith("+++ "):
            name = raw[4:].strip()
            path = name[2:] if name.startswith("b/") else name
            cur = FileDiff(path, [], [])
            files.append(cur)
            continue
        if raw.startswith("--- "):
            continue
        if raw.startswith("@@"):
            m = re.match(r"@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@", raw)
            if m:
                old_no, new_no = int(m.group(1)), int(m.group(2))
            continue
        if cur is None:
            continue
        if raw.startswith("+"):
            cur.added.append((new_no, raw[1:]))
            new_no += 1
        elif raw.startswith("-"):
            cur.removed.append((old_no, raw[1:]))
            old_no += 1
    return files


def check_rust(fd: FileDiff) -> list[Finding]:
    out = []
    prev_no, prev_text = -10, ""
    for no, text in fd.added:
        code = text.split("//", 1)[0]
        marked = MARKER.search(text) or (prev_no == no - 1 and MARKER.search(prev_text))
        for pat, msg in RUST_LOOSEN:
            if pat.search(code) and not marked:
                out.append(Finding("F1", fd.path, no, msg + " (add `// floor-guard: <reason>` if it is justified)"))
        prev_no, prev_text = no, text
    return out


def check_workflow(fd: FileDiff) -> list[Finding]:
    out = []
    for no, text in fd.added:
        if text.lstrip().startswith("#"):
            continue
        for pat, msg in CI_LOOSEN:
            if pat.search(text) and not MARKER.search(text):
                out.append(Finding("F2", fd.path, no, msg))
    added_dw = sum("-D warnings" in t for _, t in fd.added)
    removed_dw = [(n, t) for n, t in fd.removed if "-D warnings" in t]
    if len(removed_dw) > added_dw:
        n, _ = removed_dw[0]
        out.append(Finding("F2", fd.path, n, "`-D warnings` removed from a workflow"))
    return out


PROTECTED_SECTIONS = ("## floor", "## task guard")


def protected_section_lines(text: str) -> set[str]:
    lines, inside = set(), False
    for line in text.splitlines():
        if line.startswith("## "):
            inside = line.strip().lower().startswith(PROTECTED_SECTIONS)
            continue
        if inside and line.strip():
            lines.add(line.rstrip())
    return lines


def check_constraints(base_text: str | None, head_text: str | None) -> list[Finding]:
    if base_text is None:
        return []
    lost = protected_section_lines(base_text) - protected_section_lines(head_text or "")
    return [Finding("F3", "CONSTRAINTS.md", 0, f"Protected rule removed or changed: {line.strip()}")
            for line in sorted(lost)]


def check_baseline(base_text: str | None, head_text: str | None) -> list[Finding]:
    if base_text is None:
        return []
    try:
        old = json.loads(base_text)
        new = json.loads(head_text or "{}")
    except json.JSONDecodeError as e:
        return [Finding("F3", BASELINE, 0, f"baseline is not valid JSON: {e}")]
    out = []
    for key, val in old.items():
        if key.startswith("min_") and isinstance(val, int) and new.get(key, 0) < val:
            out.append(Finding("F3", BASELINE, 0, f"{key} lowered from {val} to {new.get(key)}"))
    return out


def count_rust_tests(root: Path) -> int:
    n = 0
    for p in (root / "crates").rglob("*.rs"):
        try:
            n += len(TEST_FN.findall(p.read_text(encoding="utf-8", errors="replace")))
        except OSError:
            pass
    return n


def check_ratchet(root: Path) -> tuple[list[Finding], str]:
    path = root / BASELINE
    if not path.exists():
        return [Finding("R1", BASELINE, 0, "baseline file missing")], ""
    base = json.loads(path.read_text(encoding="utf-8"))
    have = count_rust_tests(root)
    want = int(base.get("min_rust_tests", 0))
    if have < want:
        return [Finding("R1", "crates/", 0, f"Rust tests fell to {have}, floor is {want}")], ""
    note = ""
    if have > want:
        note = f"Rust tests: {have} (floor {want}). Raise min_rust_tests to {have} to lock the gain in."
    return [], note


def extract_task_codes(text: str) -> set[str]:
    """Task codes in a branch name or PR body, canonicalised.

    A step code (P0-3 step 6) supersedes the bare project code (P0-3) and the
    bare STEP fragment in the same text, so steps of one project are distinct
    tasks instead of all colliding on the project code.
    """
    codes = set()
    for m in TASK_STEP.finditer(text):
        codes.add(f"P0-{m.group(1)}-STEP-{m.group(2)}")
    for m in TASK_PSTEP.finditer(text):
        codes.add(f"P0-{m.group(1)}-STEP-{m.group(2)}")
    for m in TASK_GENERIC.finditer(text):
        codes.add(m.group(0).upper())
    for m in TASK_VIEC.finditer(text):
        codes.add(f"VIỆC-{m.group(1)}")
    for c in list(codes):
        m = re.fullmatch(r"P0-(\d+)-STEP-(\d+)", c)
        if m:
            codes.discard(f"P0-{m.group(1)}")
            codes.discard(f"STEP-{m.group(2)}")
    return codes


def check_task_code(branch: str, body: str) -> list[Finding]:
    """T1: the branch name or the PR body must name a task code."""
    if branch.startswith(EXEMPT_BRANCH_PREFIXES):
        return []
    codes = extract_task_codes(branch) | extract_task_codes(body or "")
    if codes:
        return []
    return [Finding("T1", branch or "PR", 0,
                     "no task code in branch name or PR body: name the task "
                     "(B-N, STU-N, APP-N, DV-N, DS-N, C-N, Việc N, P0-N step M) "
                     "in the branch or the PR body")]


def check_task_duplicates(codes: set[str], prs: list[dict], current: int | None) -> list[Finding]:
    """T2: another open PR sharing a task code with this one."""
    out = []
    for pr in prs:
        num = pr.get("number")
        if current is not None and num == current:
            continue
        other = extract_task_codes(pr.get("headRefName", "") or "")
        other |= extract_task_codes(pr.get("body", "") or "")
        for code in sorted(codes & other):
            out.append(Finding("T2", "PR", 0,
                               f"task code {code} is also open in PR #{num} "
                               f"(head {pr.get('headRefName', '')}); one task = one branch = one PR, "
                               f"ask the Leader to decide"))
    return out


def task_warn_only(warn_until: str | None, today: datetime.date | None = None) -> bool:
    """True while T findings are warnings instead of failures."""
    if not warn_until:
        return False
    try:
        until = datetime.date.fromisoformat(warn_until)
    except ValueError:
        return False
    return (today or datetime.date.today()) <= until


def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout


def show(ref: str, path: str) -> str | None:
    r = subprocess.run(["git", "show", f"{ref}:{path}"], cwd=ROOT, capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def read_event(path: str | None) -> dict:
    if not path or not os.path.exists(path):
        return {}
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}


def labels_from_event(path: str | None) -> set[str]:
    ev = read_event(path)
    return {lab.get("name", "") for lab in ev.get("pull_request", {}).get("labels", [])}


def pr_from_event(path: str | None) -> dict | None:
    """Branch, body and number of the pull request in the event, if any."""
    ev = read_event(path)
    pr = ev.get("pull_request")
    if not isinstance(pr, dict):
        return None
    return {
        "number": pr.get("number"),
        "branch": (pr.get("head") or {}).get("ref", ""),
        "body": pr.get("body") or "",
    }


def load_prs(path: str | None) -> list[dict] | None:
    if not path:
        return None
    try:
        data = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        print(f"floor-guard: cannot read --prs file {path}: {e}", file=sys.stderr)
        return None
    return data if isinstance(data, list) else None


def run(base: str | None, event: str | None, branch: str | None = None,
        pr_body_file: str | None = None, pr_number: int | None = None,
        prs_file: str | None = None, warn_until: str | None = None) -> int:
    findings: list[Finding] = []
    if base:
        diff = git("diff", "-U0", "--no-color", f"{base}...HEAD")
        for fd in parse_diff(diff):
            if fd.path.endswith(".rs"):
                findings += check_rust(fd)
            elif fd.path.startswith(".github/workflows/"):
                findings += check_workflow(fd)
        findings += check_constraints(show(base, "CONSTRAINTS.md"), show("HEAD", "CONSTRAINTS.md"))
        findings += check_baseline(show(base, BASELINE), show("HEAD", BASELINE))
    ratchet, note = check_ratchet(ROOT)
    findings += ratchet

    approved = APPROVE_LABEL in labels_from_event(event)
    level = "warning" if approved else "error"
    for f in findings:
        loc = f"file={f.path},line={f.line}" if f.line else f"file={f.path}"
        print(f"::{level} {loc},title=floor-guard {f.rule}::{f.msg}")
    if note:
        print(f"::notice title=floor-guard ratchet::{note}")
    if not base:
        print("floor-guard: no base ref, ran the ratchet only")

    # Task guard (T1/T2): needs pull-request context.
    task_findings: list[Finding] = []
    pr_ctx = pr_from_event(event)
    if branch is not None or pr_body_file is not None or pr_number is not None:
        pr_ctx = {
            "number": pr_number if pr_number is not None else (pr_ctx or {}).get("number"),
            "branch": branch if branch is not None else (pr_ctx or {}).get("branch", ""),
            "body": Path(pr_body_file).read_text(encoding="utf-8") if pr_body_file else (pr_ctx or {}).get("body", ""),
        }
    if pr_ctx is None:
        print("floor-guard: no pull request in this event, task guard skipped")
    else:
        task_findings += check_task_code(pr_ctx["branch"], pr_ctx["body"])
        prs = load_prs(prs_file)
        if prs is None:
            if prs_file:
                print("floor-guard: duplicate-task check skipped (no usable --prs file)")
            else:
                print("floor-guard: duplicate-task check skipped (no --prs file given)")
        else:
            codes = extract_task_codes(pr_ctx["branch"]) | extract_task_codes(pr_ctx["body"])
            task_findings += check_task_duplicates(codes, prs, pr_ctx["number"])
    task_level = "warning" if (approved or task_warn_only(warn_until)) else "error"
    for f in task_findings:
        loc = f"file={f.path},line={f.line}" if f.line else f"file={f.path}"
        print(f"::{task_level} {loc},title=floor-guard {f.rule}::{f.msg}")
    if task_findings and task_level == "warning":
        print(f"floor-guard: {len(task_findings)} task-guard finding(s) reported as warnings "
              f"(warn-only until {warn_until or 'n/a'} or label '{APPROVE_LABEL}')")

    if findings and approved:
        print(f"floor-guard: {len(findings)} finding(s) accepted by label '{APPROVE_LABEL}'")
        return 0 if not (task_findings and task_level == "error") else 1
    if findings:
        print(f"floor-guard: {len(findings)} finding(s). Fix the code, or justify each line with "
              f"`floor-guard: <reason>` and ask a human reviewer for the '{APPROVE_LABEL}' label.")
        return 1
    if task_findings and task_level == "error":
        print(f"floor-guard: {len(task_findings)} task-guard finding(s). Name the task code in the "
              f"branch or PR body, or ask the Leader to decide on the duplicate.")
        return 1
    print("floor-guard: ok")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--base", help="git ref to diff against (merge base is used)")
    ap.add_argument("--event", default=os.environ.get("GITHUB_EVENT_PATH"))
    ap.add_argument("--branch", help="head branch name (overrides the event)")
    ap.add_argument("--pr-body-file", help="file with the PR body (overrides the event)")
    ap.add_argument("--pr-number", type=int, help="PR number (overrides the event)")
    ap.add_argument("--prs", help="JSON file of open PRs (gh pr list output) for T2")
    ap.add_argument("--warn-until", default=os.environ.get("FLOOR_GUARD_TASK_WARN_UNTIL"),
                    help="date (YYYY-MM-DD) through which T findings are warnings")
    a = ap.parse_args(argv)
    try:
        return run(a.base, a.event, a.branch, a.pr_body_file, a.pr_number, a.prs, a.warn_until)
    except subprocess.CalledProcessError as e:
        print(f"floor-guard: git failed: {e.stderr.strip()}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
