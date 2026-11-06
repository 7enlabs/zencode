"""Tests for check_instruction_drift.py. Run: python3 -m unittest scripts/test_check_instruction_drift.py"""

import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_instruction_drift as drift  # noqa: E402

CLI_RS = textwrap.dedent('''
    #[derive(Subcommand, Debug, Clone)]
    pub enum Commands {
        /// Manage MCP
        Mcp(McpArgs),
        Doctor,
        Eval(EvalArgs),
        #[command(name = "tool-help")]
        ToolHelp {
            name: String,
        },
        PrComments {
            pr: u64,
        },
    }

    #[derive(Subcommand, Debug, Clone)]
    pub enum McpSubcommand {
        List,
        Add {
            name: String,
        },
    }
''')

TOOLS_RS = 'impl Tool for R { fn name(&self) -> &str { "Read" } }\nimpl Tool for B { fn name(&self) -> &str {\n "Bash"\n } }\n'

WORKFLOW = "name: CI\non: [pull_request]\njobs:\n  check:\n    runs-on: x\n  windows-check:\n    runs-on: y\n"


def make_repo(tmp: Path, agents: str, skill: str = "") -> Path:
    (tmp / "crates/zen-cli/src").mkdir(parents=True)
    (tmp / "crates/zen-tools/src").mkdir(parents=True)
    (tmp / ".github/workflows").mkdir(parents=True)
    (tmp / "docs").mkdir()
    (tmp / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/zen-cli", "crates/zen-tools"]\n')
    (tmp / "crates/zen-cli/Cargo.toml").write_text('[package]\nname = "zen-cli"\n')
    (tmp / "crates/zen-tools/Cargo.toml").write_text('[package]\nname = "zen-tools"\n')
    (tmp / "crates/zen-cli/src/cli.rs").write_text(CLI_RS)
    (tmp / "crates/zen-tools/src/builtin.rs").write_text(TOOLS_RS)
    (tmp / ".github/workflows/ci.yml").write_text(WORKFLOW)
    (tmp / "docs/real.md").write_text("x")
    (tmp / "AGENTS.md").write_text(textwrap.dedent(agents))
    if skill:
        (tmp / "skills/demo").mkdir(parents=True)
        (tmp / "skills/demo/SKILL.md").write_text(textwrap.dedent(skill))
    return tmp


def run(tmp: Path):
    repo = drift.Repo.load(tmp)
    c = drift.Checker(repo)
    for f in drift.tracked_instruction_files(tmp):
        c.check_file((tmp / f).resolve())
    return c.findings


class DriftTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name).resolve()

    def tearDown(self):
        self._tmp.cleanup()

    def kinds(self, findings, severity):
        return sorted((f.kind, f.line) for f in findings if f.severity == severity)

    def test_clean_file_has_no_findings(self):
        make_repo(self.tmp, """\
            See [docs](docs/real.md) and `docs/real.md:3`.
            ```sh
            cargo test -p zen-cli
            zen doctor
            zen --json eval --mock
            zen mcp add foo
            zen tool-help Read
            ```
            The Windows job (`windows-check`) runs on PRs.
        """)
        self.assertEqual(run(self.tmp), [])

    def test_clear_drift_is_error(self):
        make_repo(self.tmp, """\
            See [gone](docs/gone.md).
            Edit `crates/zen-cli/src/missing.rs`.
            ```sh
            cargo run -p zen-nope -- doctor
            zen doktor
            zen mcp remove foo
            ```
        """)
        errs = self.kinds(run(self.tmp), "error")
        self.assertEqual(errs, [("cargo-package", 4), ("path", 1), ("path", 2), ("zen-command", 5), ("zen-command", 6)])

    def test_unclear_drift_is_warning_only(self):
        make_repo(self.tmp, """\
            User settings live in `.zen/settings.json`.
            The Linux job (`linux-check`) is gone.
            ```sh
            zen --model fast doctor
            zen fix the failing test
            ```
        """)
        findings = run(self.tmp)
        self.assertEqual(self.kinds(findings, "error"), [])
        self.assertEqual(self.kinds(findings, "warning"),
                         [("ci-job", 2), ("path", 1), ("zen-command", 4), ("zen-command", 5)])

    def test_skill_unknown_tool_is_warning(self):
        make_repo(self.tmp, "ok\n", skill="""\
            ---
            name: demo
            allowed-tools: read, Bash(git *), Teleport
            ---
            body
        """)
        findings = run(self.tmp)
        self.assertEqual([(f.severity, f.file, f.line) for f in findings],
                         [("warning", "skills/demo/SKILL.md", 3)])
        self.assertIn("Teleport", findings[0].message)

    def test_planned_and_extensionless_are_not_errors(self):
        (self.tmp / "tools").mkdir()
        make_repo(self.tmp, """\
            `zen mcp sync` (planned) will show a diff.
            MCP methods `docs/list` and `tools/call` are not files.
        """)
        findings = run(self.tmp)
        self.assertEqual(self.kinds(findings, "error"), [])
        self.assertIn("planned", findings[0].message)

    def test_ignore_marker(self):
        make_repo(self.tmp, "Old path `crates/zen-cli/src/old.rs` <!-- drift-ignore -->\n")
        self.assertEqual(run(self.tmp), [])

    def test_exit_code_fails_only_on_errors(self):
        make_repo(self.tmp, "Runtime `.zen/settings.json`.\n")
        self.assertEqual(drift.main(["--root", str(self.tmp)]), 0)
        (self.tmp / "AGENTS.md").write_text("Edit `crates/zen-cli/src/missing.rs`.\n")
        self.assertEqual(drift.main(["--root", str(self.tmp)]), 1)
        self.assertEqual(drift.main(["--root", str(self.tmp), "--no-fail"]), 0)


if __name__ == "__main__":
    unittest.main()
