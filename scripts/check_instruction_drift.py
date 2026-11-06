#!/usr/bin/env python3
"""Check agent instruction files against the code they describe.

Instruction files (AGENTS.md, CLAUDE.md, ZEN.md, SKILL.md) reach a coding
agent as instructions, while the code reaches it as tool output. When the two
disagree, agents tend to follow the stale file (arXiv 2610.30571). This check
flags claims in those files that the repository can disprove mechanically.

Severity:
  error    the file is clearly wrong about the repo (a path, crate or zen
           command it names does not exist). The job fails.
  warning  the claim could not be confirmed but may be legitimate (a runtime
           path, a tool from an MCP server, a word that may be a prompt).
           Reported only; never fails the job.

Silence one line with an HTML comment on that line: <!-- drift-ignore -->

Standard library only, so CI needs no install step. Run from the repo root:
  python3 scripts/check_instruction_drift.py            # scan tracked files
  python3 scripts/check_instruction_drift.py FILE...    # scan given files
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass, asdict
from pathlib import Path

INSTRUCTION_NAMES = {"AGENTS.md", "CLAUDE.md", "ZEN.md", "SKILL.md"}
PATH_EXTS = {
    "md", "rs", "toml", "yml", "yaml", "json", "jsonl", "sh", "ps1", "py",
    "lock", "txt", "html",
}
IGNORE_MARK = "drift-ignore"
# A line that says the thing does not exist yet is not drift.
FUTURE_RE = re.compile(r"\b(planned|not yet|roadmap|todo|proposed|future)\b", re.I)
NESTED_ENUMS = {"mcp": "McpSubcommand", "skill": "SkillSubcommand", "sessions": "SessionSubcommand"}


@dataclass
class Finding:
    severity: str  # "error" or "warning"
    file: str
    line: int
    kind: str
    message: str


# ---------------------------------------------------------------- repo facts

def kebab(name: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "-", name).lower()


def enum_variants(src: str, enum: str) -> set[str]:
    """Command names of a clap Subcommand enum, honoring name/alias attrs."""
    m = re.search(r"pub enum " + re.escape(enum) + r"\s*\{", src)
    if not m:
        return set()
    names: set[str] = set()
    pending: list[str] = []
    body_lines = []
    depth = 1
    for line in src[m.end():].splitlines():
        body_lines.append((depth, line))
        depth += line.count("{") - line.count("}")
        if depth <= 0:
            break
    for depth_at, line in body_lines:
        if depth_at != 1:
            continue
        s = line.strip()
        if s.startswith("#[command("):
            pending.append(s)
            continue
        v = re.match(r"([A-Z]\w*)\s*[({,]?", s)
        if v and not s.startswith("//"):
            explicit = None
            for attr in pending:
                n = re.search(r'name\s*=\s*"([^"]+)"', attr)
                if n:
                    explicit = n.group(1)
                for a in re.findall(r'(?:visible_)?alias(?:es)?\s*=\s*"([^"]+)"', attr):
                    names.add(a)
            names.add(explicit or kebab(v.group(1)))
            pending = []
    return names


@dataclass
class Repo:
    root: Path
    packages: set[str]
    commands: set[str]
    nested: dict[str, set[str]]
    tools: set[str]
    ci_jobs: set[str]

    @classmethod
    def load(cls, root: Path) -> "Repo":
        packages: set[str] = set()
        ws = (root / "Cargo.toml").read_text(encoding="utf-8") if (root / "Cargo.toml").is_file() else ""
        mem = re.search(r"members\s*=\s*\[([^\]]*)\]", ws)
        for member in re.findall(r'"([^"]+)"', mem.group(1) if mem else ""):
            toml = root / member / "Cargo.toml"
            if toml.is_file():
                n = re.search(r'^name\s*=\s*"([^"]+)"', toml.read_text(encoding="utf-8"), re.M)
                if n:
                    packages.add(n.group(1))
        cli_rs = root / "crates/zen-cli/src/cli.rs"
        cli_src = cli_rs.read_text(encoding="utf-8") if cli_rs.is_file() else ""
        commands = enum_variants(cli_src, "Commands") | {"help"}
        nested = {k: enum_variants(cli_src, e) | {"help"} for k, e in NESTED_ENUMS.items()}
        tools: set[str] = set()
        for rs in (root / "crates/zen-tools/src").glob("*.rs"):
            for n in re.findall(r'fn name\(&self\)[^{]*\{\s*"([^"]+)"', rs.read_text(encoding="utf-8")):
                tools.add(n.lower())
        ci_jobs: set[str] = set()
        for wf in list((root / ".github/workflows").glob("*.yml")) + list((root / ".github/workflows").glob("*.yaml")):
            text = wf.read_text(encoding="utf-8")
            jm = re.search(r"^jobs:\s*$", text, re.M)
            if jm:
                ci_jobs.update(re.findall(r"^  ([A-Za-z0-9_-]+):\s*$", text[jm.end():], re.M))
        return cls(root, packages, commands, nested, tools, ci_jobs)


# ------------------------------------------------------------------ checks

LINK_RE = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
CODE_RE = re.compile(r"`([^`\n]+)`")
ZEN_RE = re.compile(r"(?:^|\s|\$\s*)(?:zen|cargo run\b[^\n]*?\s--)\s+(.*)$")
PKG_RE = re.compile(r"(?:\s-p|\s--package)[\s=]+([A-Za-z0-9_][A-Za-z0-9_-]*)")


def looks_like_path(tok: str) -> bool:
    if " " in tok or tok.startswith(("~", "/", "$", "-", "http://", "https://", "<")):
        return False
    if any(ch in tok for ch in "*<>{}|=\\\"'()") or re.match(r"^[A-Za-z]:", tok):
        return False
    if not re.fullmatch(r"[A-Za-z0-9_./-]+", tok) or tok.startswith(".."):
        return False
    last = tok.rstrip("/").rsplit("/", 1)[-1]
    ext = last.rsplit(".", 1)[-1].lower() if "." in last.lstrip(".") else ""
    return "/" in tok.strip("/") or ext in PATH_EXTS


class Checker:
    def __init__(self, repo: Repo):
        self.repo = repo
        self.findings: list[Finding] = []
        self._names: set[str] | None = None
        self._line = ""

    def add(self, sev, file, line, kind, msg):
        if sev == "error" and FUTURE_RE.search(self._line):
            sev, msg = "warning", msg + " (line marks it as planned)"
        self.findings.append(Finding(sev, file, line, kind, msg))

    def tracked_basenames(self) -> set[str]:
        if self._names is None:
            self._names = {p.name for p in self.repo.root.rglob("*") if ".git" not in p.parts and "target" not in p.parts}
        return self._names

    def exists(self, rel: str, base: Path) -> bool:
        rel = re.sub(r":\d+(-\d+)?$", "", rel).rstrip("/")
        return (self.repo.root / rel).exists() or (base / rel).exists()

    def check_path(self, rel_file: str, lineno: int, tok: str, base: Path, from_link: bool):
        clean = re.sub(r":\d+(-\d+)?$", "", tok.split("#", 1)[0])
        if not clean or self.exists(clean, base):
            return
        first = clean.strip("/").split("/", 1)[0]
        last = clean.rstrip("/").rsplit("/", 1)[-1]
        has_ext = "." in last.lstrip(".") and last.rsplit(".", 1)[-1].lower() in PATH_EXTS
        in_repo_tree = ("/" in clean.strip("/") and (self.repo.root / first).is_dir()
                        and (has_ext or tok.endswith("/")))
        if from_link or in_repo_tree:
            self.add("error", rel_file, lineno, "path", f"`{tok}` does not exist in the repo")
        elif "/" not in clean and clean in self.tracked_basenames():
            return
        else:
            self.add("warning", rel_file, lineno, "path",
                     f"`{tok}` not found in the repo (fine if it is a runtime or user path)")

    def check_zen(self, rel_file: str, lineno: int, rest: str, in_block: bool):
        rest = rest.split("#", 1)[0] if in_block else rest
        toks = rest.split()
        if not toks or any(t in ("-p", "--print") for t in toks):
            return
        i, after_flag = 0, False
        while i < len(toks) and toks[i].startswith("-"):
            after_flag = "=" not in toks[i]
            i += 1
        if i >= len(toks):
            return
        word = toks[i]
        if word[0] in "\"'<$[" or not re.fullmatch(r"[a-z][a-z0-9-]*", word):
            return
        if word in self.repo.commands:
            sub = self.repo.nested.get(word)
            nxt = next((t for t in toks[i + 1:] if not t.startswith("-")), None)
            if sub and nxt and re.fullmatch(r"[a-z][a-z0-9-]*", nxt) and nxt not in sub:
                self.add("error", rel_file, lineno, "zen-command",
                         f"`zen {word} {nxt}` is not a subcommand (known: {', '.join(sorted(sub - {'help'}))})")
            return
        lone = len([t for t in toks[i:] if not t.startswith("-")]) == 1
        if lone and not after_flag and in_block:
            self.add("error", rel_file, lineno, "zen-command",
                     f"`zen {word}` is not a zen command; a lone unknown word exits 4")
        else:
            self.add("warning", rel_file, lineno, "zen-command",
                     f"`{word}` after `zen` is not a known command (may be a flag value or a prompt)")

    def check_line_commands(self, rel_file, lineno, text, in_block):
        for pkg in (PKG_RE.findall(" " + text) if re.search(r"\bcargo\b", text) else []):
            if self.repo.packages and pkg not in self.repo.packages:
                self.add("error", rel_file, lineno, "cargo-package",
                         f"`-p {pkg}` is not a workspace package (known: {', '.join(sorted(self.repo.packages))})")
        m = ZEN_RE.search(text)
        if m and (in_block or text.lstrip().startswith(("zen ", "cargo "))):
            self.check_zen(rel_file, lineno, m.group(1), in_block)

    def check_frontmatter_tools(self, rel_file, lines):
        if not lines or lines[0].strip() != "---" or not self.repo.tools:
            return
        for idx, line in enumerate(lines[1:], start=2):
            if line.strip() == "---":
                return
            m = re.match(r"\s*(allowed[-_]tools|tools)\s*:\s*(.*)$", line, re.I)
            if not m or IGNORE_MARK in line:
                continue
            for raw in re.split(r",(?![^()]*\))", m.group(2)):
                name = re.sub(r"\(.*\)$", "", raw.strip().strip("[]\"' ")).lower()
                if name and not name.startswith("mcp__") and name not in self.repo.tools:
                    self.add("warning", rel_file, idx, "tool",
                             f"tool `{raw.strip()}` is not a built-in zen tool (fine if an MCP server provides it)")

    def check_file(self, path: Path):
        try:
            rel_file = path.relative_to(self.repo.root).as_posix()
        except ValueError:
            rel_file = path.as_posix()
        lines = path.read_text(encoding="utf-8").splitlines()
        base = path.parent
        self.check_frontmatter_tools(rel_file, lines)
        in_block = False
        for lineno, line in enumerate(lines, start=1):
            if line.lstrip().startswith("```"):
                in_block = not in_block
                continue
            if IGNORE_MARK in line:
                continue
            self._line = line
            if in_block:
                self.check_line_commands(rel_file, lineno, line, True)
                continue
            for target in LINK_RE.findall(line):
                if not re.match(r"^[a-z]+:|^#", target):
                    self.check_path(rel_file, lineno, target, base, True)
            for span in CODE_RE.findall(line):
                if looks_like_path(span):
                    self.check_path(rel_file, lineno, span, base, False)
                else:
                    self.check_line_commands(rel_file, lineno, span, False)
                if re.fullmatch(r"[a-z][a-z0-9-]+", span) and re.search(r"\bjob\b", line, re.I) \
                        and self.repo.ci_jobs and span not in self.repo.ci_jobs and "-" in span:
                    self.add("warning", rel_file, lineno, "ci-job",
                             f"`{span}` reads like a CI job but no workflow defines it")


def tracked_instruction_files(root: Path) -> list[Path]:
    try:
        out = subprocess.run(["git", "ls-files"], cwd=root, capture_output=True, text=True, check=True).stdout
        files = out.splitlines()
    except (OSError, subprocess.CalledProcessError):
        files = [p.relative_to(root).as_posix() for p in root.rglob("*.md")]
    return sorted(Path(f) for f in files if Path(f).name in INSTRUCTION_NAMES)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("files", nargs="*", type=Path)
    ap.add_argument("--root", type=Path, default=Path.cwd())
    ap.add_argument("--json", action="store_true", help="print findings as JSON")
    ap.add_argument("--no-fail", action="store_true", help="exit 0 even with errors")
    args = ap.parse_args(argv)
    root = args.root.resolve()
    repo = Repo.load(root)
    checker = Checker(repo)
    files = args.files or tracked_instruction_files(root)
    for f in files:
        checker.check_file(f if f.is_absolute() else (Path.cwd() / f if args.files else root / f).resolve())
    errors = [f for f in checker.findings if f.severity == "error"]
    warnings = [f for f in checker.findings if f.severity == "warning"]
    if args.json:
        print(json.dumps({"files": [str(f) for f in files], "findings": [asdict(f) for f in checker.findings]}, indent=2))
    else:
        for f in checker.findings:
            print(f"{f.file}:{f.line}: {f.severity}: [{f.kind}] {f.message}")
        print(f"checked {len(files)} instruction file(s): {len(errors)} error(s), {len(warnings)} warning(s)")
    if os.environ.get("GITHUB_ACTIONS") == "true":
        for f in checker.findings:
            print(f"::{f.severity} file={f.file},line={f.line},title=instruction drift ({f.kind})::{f.message}")
        summary = os.environ.get("GITHUB_STEP_SUMMARY")
        if summary:
            with open(summary, "a", encoding="utf-8") as fh:
                fh.write(f"## Instruction drift\n\n{len(files)} file(s), {len(errors)} error(s), {len(warnings)} warning(s)\n\n")
                if checker.findings:
                    fh.write("| Severity | File | Line | Kind | Finding |\n|---|---|---|---|---|\n")
                    for f in checker.findings:
                        fh.write(f"| {f.severity} | {f.file} | {f.line} | {f.kind} | {f.message.replace('|', '/')} |\n")
    return 1 if errors and not args.no_fail else 0


if __name__ == "__main__":
    sys.exit(main())
