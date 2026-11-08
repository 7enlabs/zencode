"""Unit tests for scripts/arch_lint.py (stdlib unittest)."""
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import arch_lint as al  # noqa: E402


def _crate(root: Path, name: str, deps: list[str]) -> None:
    d = root / "crates" / name / "src"
    d.mkdir(parents=True, exist_ok=True)
    lines = [f'{dep}.workspace = true' for dep in deps]
    body = "[package]\nname = \"{}\"\n\n[dependencies]\n{}\n".format(name, "\n".join(lines))
    (root / "crates" / name / "Cargo.toml").write_text(body, encoding="utf-8")
    (d / "lib.rs").write_text("pub fn f() {}\n", encoding="utf-8")


def _tree(root: Path, deps: dict[str, list[str]] | None = None) -> None:
    deps = deps or {
        "zen-core": [],
        "zen-agent": ["zen-core"],
        "zen-tools": ["zen-core"],
        "zen-ext": ["zen-core"],
        "zen-mcp": ["zen-core", "zen-tools"],
        "zen-cli": ["zen-core", "zen-agent", "zen-tools", "zen-ext", "zen-mcp"],
    }
    for name, ds in deps.items():
        _crate(root, name, ds)
    (root / "ci").mkdir(exist_ok=True)
    (root / "ci" / "floor-baseline.json").write_text(
        json.dumps({"min_rust_tests": 1, "max_runtime_allows": 0}), encoding="utf-8")


class Layers(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        _tree(self.root)

    def tearDown(self):
        self.tmp.cleanup()

    def test_layers_ok(self):
        self.assertEqual(al.check_layers(self.root), [])

    def test_layer_violation(self):
        _crate(self.root, "zen-agent", ["zen-core", "zen-tools"])
        found = al.check_layers(self.root)
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].rule, "A1")
        self.assertIn("zen-agent depends on zen-tools", found[0].msg)
        self.assertIn("Fix:", found[0].msg)

    def test_layer_zen_cli_may_depend_on_all(self):
        _crate(self.root, "zen-cli",
               ["zen-core", "zen-agent", "zen-tools", "zen-ext", "zen-mcp"])
        self.assertEqual(al.check_layers(self.root), [])

    def test_layer_nothing_may_depend_on_zen_cli(self):
        _crate(self.root, "zen-tools", ["zen-core", "zen-cli"])
        found = al.check_layers(self.root)
        self.assertEqual(len(found), 1)
        self.assertIn("zen-tools depends on zen-cli", found[0].msg)

    def test_layer_unknown_crate_fails_closed(self):
        _crate(self.root, "zen-new", ["zen-core"])
        found = al.check_layers(self.root)
        self.assertEqual(len(found), 1)
        self.assertIn("not in the layer map", found[0].msg)
        self.assertIn("Fix:", found[0].msg)

    def test_layer_dev_dependencies_are_exempt(self):
        toml = self.root / "crates" / "zen-agent" / "Cargo.toml"
        toml.write_text(toml.read_text(encoding="utf-8")
                        + "\n[dev-dependencies]\nzen-tools.workspace = true\n",
                        encoding="utf-8")
        self.assertEqual(al.check_layers(self.root), [])


class RuntimeAllows(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        _tree(self.root)
        self.src = self.root / "crates" / "zen-agent" / "src"

    def tearDown(self):
        self.tmp.cleanup()

    def _scan(self, rel: str, text: str):
        p = self.root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf-8")
        return al.scan_file(p, self.root)

    def test_unmarked_allow_flagged_with_fix(self):
        found, marked = self._scan("crates/zen-agent/src/agent.rs",
                                   "#[allow(clippy::too_many_arguments)]\nfn f() {}\n")
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].rule, "A2")
        self.assertIn("Fix:", found[0].msg)
        self.assertIn("group the parameters into a struct", found[0].msg)
        self.assertEqual(marked, 0)

    def test_inner_allow_flagged(self):
        found, _ = self._scan("crates/zen-agent/src/lib.rs",
                              "#![allow(clippy::module_inception)]\npub mod x;\n")
        self.assertEqual(len(found), 1)
        self.assertIn("module_inception", found[0].msg)
        self.assertIn("rename the module", found[0].msg)

    def test_cfg_attr_allow_flagged(self):
        found, _ = self._scan("crates/zen-agent/src/agent.rs",
                              "#[cfg_attr(test, allow(dead_code))]\nfn f() {}\n")
        # cfg_attr(test, ...) is still an inline-disable in runtime source
        self.assertEqual(len(found), 1)

    def test_marked_same_line_ok(self):
        found, marked = self._scan(
            "crates/zen-agent/src/agent.rs",
            "#[allow(dead_code)] // floor-guard: kept intentionally, unused for now\n"
            "fn f() {}\n")
        self.assertEqual(found, [])
        self.assertEqual(marked, 1)

    def test_marked_line_above_ok(self):
        found, marked = self._scan(
            "crates/zen-agent/src/agent.rs",
            "// floor-guard: params-struct refactor deferred\n"
            "#[allow(clippy::too_many_arguments)]\nfn f() {}\n")
        self.assertEqual(found, [])
        self.assertEqual(marked, 1)

    def test_short_marker_does_not_count(self):
        found, marked = self._scan(
            "crates/zen-agent/src/agent.rs",
            "#[allow(dead_code)] // floor-guard: x\nfn f() {}\n")
        self.assertEqual(len(found), 1)
        self.assertEqual(marked, 0)

    def test_allow_in_string_not_flagged(self):
        found, marked = self._scan(
            "crates/zen-agent/src/agent.rs",
            "let s = \"#[allow(dead_code)]\";\nfn f() {}\n")
        self.assertEqual(found, [])
        self.assertEqual(marked, 0)

    def test_test_files_exempt(self):
        for rel in ("crates/zen-agent/src/lsp_tests.rs",
                    "crates/zen-agent/src/testutil.rs",
                    "crates/zen-agent/tests/it.rs"):
            found, marked = self._scan(rel, "#[allow(dead_code)]\nfn f() {}\n")
            self.assertEqual(found, [], rel)
            self.assertEqual(marked, 0, rel)

    def test_cfg_test_module_exempt(self):
        text = ("fn real() {}\n"
                "#[cfg(test)]\n"
                "mod tests {\n"
                "    #[allow(dead_code)]\n"
                "    fn helper() {\n"
                "        let s = \"}\";\n"
                "        let _ = s;\n"
                "    }\n"
                "}\n"
                "#[allow(dead_code)]\n"
                "fn runtime_after() {}\n")
        found, marked = self._scan("crates/zen-agent/src/agent.rs", text)
        self.assertEqual([f.line for f in found], [10])
        self.assertEqual(marked, 0)

    def test_build_rs_not_scanned(self):
        p = self.root / "crates" / "zen-agent" / "build.rs"
        p.write_text("#[allow(dead_code)]\nfn main() {}\n", encoding="utf-8")
        found, marked, scanned = al.check_runtime_allows(self.root)
        self.assertEqual(found, [])
        self.assertEqual(marked, 0)


class Ratchet(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        _tree(self.root)

    def tearDown(self):
        self.tmp.cleanup()

    def _set_baseline(self, obj):
        (self.root / "ci" / "floor-baseline.json").write_text(
            json.dumps(obj), encoding="utf-8")

    def test_over_max_flagged_with_fix(self):
        found = al.check_ratchet(self.root, 1)
        self.assertEqual(len(found), 1)
        self.assertIn("max_runtime_allows 0", found[0].msg)
        self.assertIn("Fix:", found[0].msg)

    def test_at_max_ok(self):
        self.assertEqual(al.check_ratchet(self.root, 0), [])

    def test_missing_key_flagged(self):
        self._set_baseline({"min_rust_tests": 1})
        found = al.check_ratchet(self.root, 0)
        self.assertEqual(len(found), 1)
        self.assertIn("max_runtime_allows is missing", found[0].msg)


if __name__ == "__main__":
    unittest.main()
