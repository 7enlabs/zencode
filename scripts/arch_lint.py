#!/usr/bin/env python3
"""Architectural lint for the ZenCode CLI workspace (D2 of the harness 7-point package).

Runs outside clippy: it checks the crate layering from docs/ARCHITECTURE.md
and the ban on inline lint silencing in runtime Rust code. Stdlib only
(Python 3.11+ for tomllib).

Rules:

* A1  layering: a crate may only depend on the crates its layer allows
  (LAYER_DEPS). zen-cli is the composition root and may depend on
  everything; nothing except itself may depend on zen-cli. A crate the
  map does not know fails closed: add it to the map first.
  Only [dependencies] (and [target.*.dependencies]) count: dev- and
  build-dependencies are test/build-time only and cannot leak into
  runtime code, so they are exempt.
* A2  no inline-disable in runtime code: no `#[allow(...)]`,
  `#![allow(...)]` or `allow(...)` inside `#[cfg_attr(...)]` in
  `crates/*/src/**/*.rs`, except test code (files under `tests/`,
  `benches/`, `examples/`, files named `*_tests.rs` or `testutil.rs`,
  code inside `#[cfg(test)]` modules) and except a line carrying
  `// floor-guard: <reason>` on the same line or the line above it.
  Build scripts (`build.rs`) are build-time, not runtime, and are not
  scanned. The count of justified runtime allows must not exceed
  `max_runtime_allows` in ci/floor-baseline.json (a ratchet: it only
  shrinks, enforced by scripts/floor_guard.py refusing a raised number).
  Single-line attributes only: a multi-line `#[allow(...)]` is caught by
  its opening line.
* Every finding carries a Fix: line: the message says how to fix it, not
  just what is wrong (piece 2 of D2).

Usage: arch_lint.py [--root PATH]
Exit: 0 pass, 1 violation, 2 usage error.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

ROOT_DEFAULT = Path(__file__).resolve().parent.parent
BASELINE = "ci/floor-baseline.json"
MARKER = re.compile(r"floor-guard:\s*\S.{4,}")

# Layer map from docs/ARCHITECTURE.md, verified against the code: zen-mcp
# uses zen-tools for the shared output-spill limit (BASH_MAX_CHARS,
# spill_output); zen-cli uses all five crates; the rest use zen-core only.
LAYER_DEPS: dict[str, frozenset[str]] = {
    "zen-core": frozenset(),
    "zen-agent": frozenset({"zen-core"}),
    "zen-tools": frozenset({"zen-core"}),
    "zen-ext": frozenset({"zen-core"}),
    "zen-mcp": frozenset({"zen-core", "zen-tools"}),
    "zen-cli": frozenset({"zen-core", "zen-agent", "zen-tools", "zen-ext", "zen-mcp"}),
}

ALLOW_RE = re.compile(r"#\s*!?\s*\[\s*allow\s*\(\s*([^)\s,]+)?")
CFG_ATTR_ALLOW_RE = re.compile(r"#\s*\[\s*cfg_attr\s*\([^)]*?\ballow\s*\(\s*([^)\s,]+)?")
CFG_TEST_RE = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
MOD_OPEN_RE = re.compile(r"\bmod\s+\w+\s*\{")
TEST_PATH_RE = re.compile(r"(^|/)(tests|benches|examples)/|_tests\.rs$|testutil\.rs$")

FIX_BY_LINT = {
    "clippy::too_many_arguments": "group the parameters into a struct",
    "dead_code": "delete the unused item (or make it used)",
    "clippy::module_inception": "rename the module so it no longer matches the crate name",
    "clippy::type_complexity": "introduce a type alias for the complex type",
}


@dataclass
class Finding:
    rule: str
    path: str
    line: int
    msg: str


def crate_deps(cargo_toml: Path) -> set[str]:
    """Inter-crate names in [dependencies] / [target.*.dependencies]."""
    try:
        data = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as e:
        raise ValueError(f"cannot parse {cargo_toml}: {e}") from e
    deps: set[str] = set(data.get("dependencies", {}))
    target = data.get("target", {})
    if isinstance(target, dict):
        for cfg in target.values():
            if isinstance(cfg, dict):
                deps |= set(cfg.get("dependencies", {}))
    return {d for d in deps if d.startswith("zen-")}


def check_layers(root: Path) -> list[Finding]:
    """A1: every crate's [dependencies] must stay inside its layer."""
    out = []
    for toml in sorted((root / "crates").glob("*/Cargo.toml")):
        crate = toml.parent.name
        rel = str(toml.relative_to(root))
        allowed = LAYER_DEPS.get(crate)
        if allowed is None:
            out.append(Finding("A1", rel, 0,
                               f"crate {crate} is not in the layer map in "
                               f"scripts/arch_lint.py. Fix: add it to LAYER_DEPS with the "
                               f"crates its layer may depend on, update "
                               f"docs/ARCHITECTURE.md in the same PR, and get the Leader "
                               f"to approve the layer change."))
            continue
        for dep in sorted(crate_deps(toml)):
            if dep not in allowed:
                out.append(Finding("A1", rel, 0,
                                   f"{crate} depends on {dep}, but its layer only allows "
                                   f"{', '.join(sorted(allowed)) or 'nothing'}. Fix: move "
                                   f"the shared type or trait into zen-core (or the lower "
                                   f"layer) so {crate} no longer needs {dep}, or invert "
                                   f"the dependency; if the layering itself must change, "
                                   f"update LAYER_DEPS in scripts/arch_lint.py and "
                                   f"docs/ARCHITECTURE.md in the same PR and get the "
                                   f"Leader to approve it."))
    return out


def _code_part(line: str) -> str:
    """Line with string/char literals blanked and // comments cut.

    Used for brace counting and for deciding whether an allow is real code
    (one inside a string literal does not silence anything). Handles
    regular strings (with escapes), raw strings (r"..." / r#"..."#) and
    char literals; lifetimes such as &'a are left alone. A brace inside a
    /* */ block comment would miscount, which only matters inside
    #[cfg(test)] modules.
    """
    out: list[str] = []
    i, n = 0, len(line)
    while i < n:
        c = line[i]
        if c == "/" and i + 1 < n and line[i + 1] == "/":
            break
        if c == '"':
            j = i + 1
            while j < n:
                if line[j] == "\\":
                    j += 2
                    continue
                if line[j] == '"':
                    break
                j += 1
            i = j + 1
            continue
        if c == "r" and (i == 0 or not (line[i - 1].isalnum() or line[i - 1] == "_")) \
                and i + 1 < n and (line[i + 1] == '"' or line[i + 1] == "#"):
            j = i + 1
            hashes = 0
            while j < n and line[j] == "#":
                hashes += 1
                j += 1
            if j < n and line[j] == '"':
                close = '"' + "#" * hashes
                k = line.find(close, j + 1)
                i = n if k < 0 else k + len(close)
                continue
        if c == "'":
            j = i + 1
            if j < n and line[j] == "\\":
                j += 2
            else:
                j += 1
            if j < n and line[j] == "'":
                i = j + 1
                continue
        out.append(c)
        i += 1
    return "".join(out)


def _allow_on_line(line: str) -> str | None:
    """Lint name silenced by this line, or None.

    Matches `#[allow(...)]`, `#![allow(...)]` and `allow(...)` inside
    `#[cfg_attr(...)]`; only the opening line of a multi-line attribute is
    needed. An allow inside a string literal does not count.
    """
    code = _code_part(line)
    m = ALLOW_RE.search(code) or CFG_ATTR_ALLOW_RE.search(code)
    if not m:
        return None
    return m.group(1) or "?"


def _is_blank_or_comment_or_attr(code: str) -> bool:
    s = code.strip()
    return not s or s.startswith("//") or s.startswith("#[") or s.startswith("#!")


def is_runtime_file(path: Path, root: Path) -> bool:
    """True for Rust files the runtime ban applies to.

    Build scripts are build-time, not runtime; test files (under tests/,
    benches/ or examples/, *_tests.rs, testutil.rs) are exempt.
    """
    if path.name == "build.rs":
        return False
    return not TEST_PATH_RE.search(str(path.relative_to(root)))


def scan_file(path: Path, root: Path) -> tuple[list[Finding], int]:
    """A2 for one runtime file. Returns (findings, justified_allow_count)."""
    if not is_runtime_file(path, root):
        return [], 0
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    rel = str(path.relative_to(root))
    findings: list[Finding] = []
    marked = 0
    in_test = 0  # brace depth inside a #[cfg(test)] mod; 0 = runtime code
    pending_test = False
    for idx, raw in enumerate(lines):
        lineno = idx + 1
        if CFG_TEST_RE.search(re.sub(r"\s+", "", raw)):
            pending_test = True
            code = _code_part(raw)
            if MOD_OPEN_RE.search(code):
                in_test = code.count("{") - code.count("}")
                pending_test = False
            continue
        code = _code_part(raw)
        if pending_test:
            if _is_blank_or_comment_or_attr(code):
                pass  # blank / comment / attribute lines do not end the wait
            elif MOD_OPEN_RE.search(code):
                in_test = code.count("{") - code.count("}")
                pending_test = False
            else:
                pending_test = False  # #[cfg(test)] guarded a non-mod item
        elif in_test:
            in_test += code.count("{") - code.count("}")
            if in_test <= 0:
                in_test = 0
        if in_test:
            continue
        lint = _allow_on_line(raw)
        if lint is None:
            continue
        if MARKER.search(raw) or (idx > 0 and MARKER.search(lines[idx - 1])):
            marked += 1
            continue
        fix = FIX_BY_LINT.get(lint, "fix the underlying lint instead of silencing it")
        findings.append(Finding("A2", rel, lineno,
                                f"runtime #[allow({lint})] without justification. "
                                f"Fix: remove the attribute and {fix}; if the lint "
                                f"genuinely cannot be fixed here, justify this exact "
                                f"line with `// floor-guard: <reason>` and ask a "
                                f"reviewer to accept it."))
    return findings, marked


def check_runtime_allows(root: Path) -> tuple[list[Finding], int, int]:
    """A2 over the whole tree. Returns (findings, justified_count, files_scanned)."""
    findings: list[Finding] = []
    marked = 0
    scanned = 0
    for path in sorted((root / "crates").rglob("*.rs")):
        if not is_runtime_file(path, root):
            continue
        f, m = scan_file(path, root)
        findings += f
        marked += m
        scanned += 1
    return findings, marked, scanned


def check_ratchet(root: Path, marked: int) -> list[Finding]:
    """A2 ratchet: justified runtime allows must not exceed the baseline."""
    path = root / BASELINE
    try:
        base = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        return [Finding("A2", BASELINE, 0,
                        f"cannot read the lint baseline: {e}. Fix: restore "
                        f"{BASELINE} from main.")]
    want = base.get("max_runtime_allows")
    if not isinstance(want, int):
        return [Finding("A2", BASELINE, 0,
                        f"max_runtime_allows is missing from {BASELINE}. Fix: add it "
                        f"with the current count of justified runtime allows (see the "
                        f"arch-lint summary) and get the Leader to approve it.")]
    if marked > want:
        return [Finding("A2", BASELINE, 0,
                        f"justified runtime #[allow] count is {marked}, above "
                        f"max_runtime_allows {want} in {BASELINE}. Fix: remove an "
                        f"inline-disable (fix the lint it silences) and lower "
                        f"max_runtime_allows to the new count, or ask the Leader for "
                        f"the floor-change-approved label before raising it.")]
    return []


def run(root: Path) -> int:
    if not (root / "crates").is_dir():
        print(f"arch-lint: no crates/ under {root}", file=sys.stderr)
        return 2
    findings: list[Finding] = []
    try:
        findings += check_layers(root)
    except ValueError as e:
        print(f"arch-lint: {e}", file=sys.stderr)
        return 2
    allow_findings, marked, scanned = check_runtime_allows(root)
    findings += allow_findings
    findings += check_ratchet(root, marked)
    for f in findings:
        loc = f"file={f.path},line={f.line}" if f.line else f"file={f.path}"
        print(f"::error {loc},title=arch-lint {f.rule}::{f.msg}")
    if findings:
        print(f"arch-lint: {len(findings)} finding(s); scanned {scanned} runtime files, "
              f"{marked} justified runtime #[allow]")
        return 1
    print(f"arch-lint: ok; scanned {scanned} runtime files, "
          f"{marked} justified runtime #[allow]")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--root", type=Path, default=ROOT_DEFAULT,
                    help="workspace root (default: parent of scripts/)")
    a = ap.parse_args(argv)
    return run(a.root)


if __name__ == "__main__":
    sys.exit(main())
