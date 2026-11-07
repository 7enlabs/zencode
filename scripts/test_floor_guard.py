"""Unit tests for scripts/floor_guard.py (stdlib unittest)."""
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import floor_guard as fg  # noqa: E402

DIFF = """diff --git a/crates/x/src/a.rs b/crates/x/src/a.rs
--- a/crates/x/src/a.rs
+++ b/crates/x/src/a.rs
@@ -10,0 +11,6 @@
+#[ignore]
+#[allow(dead_code)] // floor-guard: generated table, used by build.rs only
+// floor-guard: parked until Z Core lands the cut (tracked in #60)
+#[ignore]
+fn f() { todo!() }
+let s = "#[allow(x)]"; // a string, still flagged by design
diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -20,2 +20,2 @@
-      - run: cargo clippy -- -D warnings
-      - run: cargo test
+      - run: cargo clippy
+      - run: cargo test || true
"""


class ParseAndRules(unittest.TestCase):
    def setUp(self):
        self.files = {f.path: f for f in fg.parse_diff(DIFF)}

    def test_parse_line_numbers(self):
        rs = self.files["crates/x/src/a.rs"]
        self.assertEqual(rs.added[0][0], 11)
        self.assertEqual(len(rs.added), 6)

    def test_rust_marker_same_line_and_line_above(self):
        found = fg.check_rust(self.files["crates/x/src/a.rs"])
        lines = sorted({f.line for f in found})
        # 11 bare ignore, 15 todo!, 16 allow in code. 12 and 14 carry a marker.
        self.assertEqual(lines, [11, 15, 16])

    def test_workflow_loosening(self):
        found = fg.check_workflow(self.files[".github/workflows/ci.yml"])
        msgs = " ".join(f.msg for f in found)
        self.assertIn("|| true", msgs)
        self.assertIn("-D warnings", msgs)

    def test_workflow_moving_d_warnings_is_ok(self):
        fd = fg.FileDiff(".github/workflows/ci.yml", [(5, "run: cargo clippy -- -D warnings")],
                         [(9, "run: cargo clippy -- -D warnings")])
        self.assertEqual(fg.check_workflow(fd), [])

    def test_commented_workflow_line_ignored(self):
        fd = fg.FileDiff(".github/workflows/x.yml", [(1, "# never use continue-on-error: true")], [])
        self.assertEqual(fg.check_workflow(fd), [])


class ConstraintsAndBaseline(unittest.TestCase):
    BASE = "# C\n## Floor\n- no stubs\n- no skipped tests\n## Ratchet\n- tests\n"

    def test_floor_rule_removed(self):
        head = "# C\n## Floor\n- no stubs\n## Ratchet\n- tests\n"
        found = fg.check_constraints(self.BASE, head)
        self.assertEqual(len(found), 1)
        self.assertIn("no skipped tests", found[0].msg)

    def test_floor_rule_added_or_other_section_edit_ok(self):
        head = self.BASE.replace("- tests", "- tests, now 1300") + ""
        head = head.replace("- no stubs\n", "- no stubs\n- no new unsafe\n")
        self.assertEqual(fg.check_constraints(self.BASE, head), [])

    def test_new_file_has_no_base(self):
        self.assertEqual(fg.check_constraints(None, self.BASE), [])

    def test_baseline_lowered(self):
        found = fg.check_baseline('{"min_rust_tests": 100}', '{"min_rust_tests": 90}')
        self.assertEqual(len(found), 1)
        self.assertEqual(fg.check_baseline('{"min_rust_tests": 100}', '{"min_rust_tests": 120}'), [])


class Ratchet(unittest.TestCase):
    def make(self, tests, floor):
        d = Path(tempfile.mkdtemp())
        (d / "crates/a/src").mkdir(parents=True)
        (d / "crates/a/src/lib.rs").write_text("#[test]\nfn t(){}\n" * tests, encoding="utf-8")
        (d / "ci").mkdir()
        (d / "ci/floor-baseline.json").write_text(json.dumps({"min_rust_tests": floor}), encoding="utf-8")
        return d

    def test_drop_fails(self):
        found, _ = fg.check_ratchet(self.make(2, 3))
        self.assertEqual(found[0].rule, "R1")

    def test_gain_notes(self):
        found, note = fg.check_ratchet(self.make(4, 3))
        self.assertEqual(found, [])
        self.assertIn("Raise min_rust_tests to 4", note)

    def test_tokio_test_counted(self):
        self.assertEqual(len(fg.TEST_FN.findall("#[tokio::test]\n#[test]\n#[testing]")), 2)


class Labels(unittest.TestCase):
    def test_label_read(self):
        p = Path(tempfile.mkdtemp()) / "ev.json"
        p.write_text(json.dumps({"pull_request": {"labels": [{"name": fg.APPROVE_LABEL}]}}), encoding="utf-8")
        self.assertIn(fg.APPROVE_LABEL, fg.labels_from_event(str(p)))
        self.assertEqual(fg.labels_from_event(None), set())



class TaskGuard(unittest.TestCase):
    def test_extract_branch_code(self):
        self.assertEqual(fg.extract_task_codes("dev/p03-revive"), set())
        self.assertEqual(fg.extract_task_codes("dev/b61-retry-tool"), set())
        self.assertIn("B-4", fg.extract_task_codes("fix/B-4-flaky-test"))
        self.assertIn("VIỆC-6", fg.extract_task_codes("Việc 6: multi-model review"))

    def test_extract_step_supersedes_project(self):
        self.assertEqual(fg.extract_task_codes("P0-3 step 6 - revive dead-lettered tasks"),
                         {"P0-3-STEP-6"})
        self.assertEqual(fg.extract_task_codes("p03-s6 revive"), {"P0-3-STEP-6"})
        self.assertEqual(fg.extract_task_codes("P0-3-STEP-6"), {"P0-3-STEP-6"})

    def test_extract_no_false_positive_on_dep_name(self):
        self.assertEqual(fg.extract_task_codes("dev/dep-security-rustls-webpki-2"), set())
        self.assertEqual(fg.extract_task_codes("dev/secret-scan-ci"), set())

    def test_no_false_positive_on_non_task_codes(self):
        # Regression: the old open-ended prefix matched encodings, protocols
        # and standards (UTF-8, HTTP-2) as if they were task codes.
        for text in ("dev/fix-utf-8", "UTF-8 encoding", "UTF-16 output",
                     "fix HTTP-2 retry", "HTTP-3 support", "TLS-1.3 handshake",
                     "SHA-256 hash", "RFC-9110", "ISO-8859-1", "ECMA-262"):
            self.assertEqual(fg.extract_task_codes(text), set(), text)

    def test_known_prefixes_still_extracted(self):
        for code in ("B-4", "STU-3", "APP-1", "DV-2", "DS-1", "C-2"):
            self.assertIn(code, fg.extract_task_codes(f"work on {code}"), code)

    def test_t1_pass_from_body(self):
        self.assertEqual(fg.check_task_code("dev/secret-scan-ci", "Secret-scan CI (B-2)"), [])
        self.assertEqual(fg.check_task_code("dev/p03-revive", "P0-3 step 6"), [])

    def test_t1_fails_without_code(self):
        found = fg.check_task_code("dev/secret-scan-ci", "Secret-scan CI, no code here")
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].rule, "T1")

    def test_t1_dependabot_exempt(self):
        self.assertEqual(fg.check_task_code("dependabot/cargo/serde-1.0.0", ""), [])

    def test_t2_duplicate_found(self):
        prs = [
            {"number": 61, "headRefName": "dev/p03-sweep", "body": "P0-3 step 3"},
            {"number": 62, "headRefName": "dev/secret-scan-ci", "body": "secret scan (B-2)"},
        ]
        found = fg.check_task_duplicates({"P0-3-STEP-3"}, prs, 65)
        self.assertEqual(len(found), 1)
        self.assertIn("#61", found[0].msg)

    def test_t2_other_step_is_not_a_duplicate(self):
        prs = [{"number": 61, "headRefName": "dev/p03-sweep", "body": "P0-3 step 3"}]
        self.assertEqual(fg.check_task_duplicates({"P0-3-STEP-5"}, prs, 65), [])

    def test_t2_skips_self(self):
        prs = [{"number": 65, "headRefName": "dev/p03-revive", "body": "P0-3 step 6"}]
        self.assertEqual(fg.check_task_duplicates({"P0-3-STEP-6"}, prs, 65), [])

    def test_warn_only_window(self):
        import datetime as _dt
        self.assertTrue(fg.task_warn_only("2026-11-14", _dt.date(2026, 11, 14)))
        self.assertTrue(fg.task_warn_only("2026-11-14", _dt.date(2026, 11, 7)))
        self.assertFalse(fg.task_warn_only("2026-11-14", _dt.date(2026, 11, 15)))
        self.assertFalse(fg.task_warn_only(None, _dt.date(2026, 11, 7)))
        self.assertFalse(fg.task_warn_only("not-a-date", _dt.date(2026, 11, 7)))

    def test_task_guard_section_protected(self):
        base = "# C\n## Task guard\n- one task one PR\n## Floor\n- no stubs\n"
        head = "# C\n## Task guard\n## Floor\n- no stubs\n"
        found = fg.check_constraints(base, head)
        self.assertEqual(len(found), 1)
        self.assertIn("one task one PR", found[0].msg)


if __name__ == "__main__":
    unittest.main()
