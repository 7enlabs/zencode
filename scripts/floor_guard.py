#!/usr/bin/env python3
"""Floor guard: block pull requests that loosen the checks instead of fixing the code.

Reads CONSTRAINTS.md's rules as code. Two kinds of check:

* Floor (diff-only): looks only at lines a change adds or removes.
  - F1  a new test skip or lint/warning silencer in Rust (`#[ignore`, `#[allow(`,
        `#![allow(`) or a new stub (`todo!(`, `unimplemented!(`), unless the same
        line or the added line right above it carries `floor-guard: <reason>`.
  - F2  a workflow made softer: `continue-on-error: true`, `|| true`, or a removed
        `-D warnings` that no added line puts back.
  - F3  a removed or changed line inside CONSTRAINTS.md's "## Floor" section, or a
        lowered number in ci/floor-baseline.json.
* Ratchet (whole tree): the number of Rust test functions must not fall below
  ci/floor-baseline.json's `min_rust_tests`.

A pull request labelled `floor-change-approved` (set by a human reviewer) turns every
finding into a warning; nothing else does. Stdlib only.

Usage: floor_guard.py [--base REF] [--event PATH]
Exit: 0 pass, 1 floor or ratchet failure, 2 usage error.
"""
from __future__ import annotations

import argparse
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
    (re.compile(r"#!?\[\s*allow\s*\("), "lint or warning silenced with #[allow(...)]"),
    (re.compile(r"\btodo!\s*\("), "stub added with todo!()"),
    (re.compile(r"\bunimplemented!\s*\("), "stub added with unimplemented!()"),
]
CI_LOOSEN = [
    (re.compile(r"continue-on-error:\s*true\b"), "step or job set to continue-on-error: true"),
    (re.compile(r"\|\|\s*true\b"), "command failure swallowed with || true"),
]
TEST_FN = re.compile(r"#\[(?:test|tokio::test)\b")


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


def floor_section_lines(text: str) -> set[str]:
    lines, inside = set(), False
    for line in text.splitlines():
        if line.startswith("## "):
            inside = line.strip().lower().startswith("## floor")
            continue
        if inside and line.strip():
            lines.add(line.rstrip())
    return lines


def check_constraints(base_text: str | None, head_text: str | None) -> list[Finding]:
    if base_text is None:
        return []
    lost = floor_section_lines(base_text) - floor_section_lines(head_text or "")
    return [Finding("F3", "CONSTRAINTS.md", 0, f"Floor rule removed or changed: {line.strip()}")
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


def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout


def show(ref: str, path: str) -> str | None:
    r = subprocess.run(["git", "show", f"{ref}:{path}"], cwd=ROOT, capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def labels_from_event(path: str | None) -> set[str]:
    if not path or not os.path.exists(path):
        return set()
    try:
        ev = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return set()
    return {lab.get("name", "") for lab in ev.get("pull_request", {}).get("labels", [])}


def run(base: str | None, event: str | None) -> int:
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
    if findings and approved:
        print(f"floor-guard: {len(findings)} finding(s) accepted by label '{APPROVE_LABEL}'")
        return 0
    if findings:
        print(f"floor-guard: {len(findings)} finding(s). Fix the code, or justify each line with "
              f"`floor-guard: <reason>` and ask a human reviewer for the '{APPROVE_LABEL}' label.")
        return 1
    print("floor-guard: ok")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--base", help="git ref to diff against (merge base is used)")
    ap.add_argument("--event", default=os.environ.get("GITHUB_EVENT_PATH"))
    a = ap.parse_args(argv)
    try:
        return run(a.base, a.event)
    except subprocess.CalledProcessError as e:
        print(f"floor-guard: git failed: {e.stderr.strip()}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
