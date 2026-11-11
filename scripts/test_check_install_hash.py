"""Unit tests for scripts/check_install_hash.py (stdlib unittest, offline)."""
import hashlib
import importlib.util
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_install_hash", Path(__file__).resolve().parent / "check_install_hash.py"
)
cih = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cih)

EXE_BYTES = b"fake zen.exe bytes for hashing"
EXE_SHA = hashlib.sha256(EXE_BYTES).hexdigest()
PS1 = (
    "param(\n"
    "  [string]$Url = 'https://github.com/7enlabs/zencode/releases/download/v9.9.9/zen-x86_64-pc-windows-msvc.zip',\n"
    f"  [string]$Sha256 = '{EXE_SHA}'\n"
    ")\n"
)


def _make_zip(path, entries):
    with zipfile.ZipFile(path, "w") as zf:
        for name, data in entries.items():
            zf.writestr(name, data)


class ParseDefaultsTests(unittest.TestCase):
    def test_parses_url_and_sha(self):
        url, sha = cih.parse_defaults(PS1)
        self.assertTrue(url.endswith("/v9.9.9/zen-x86_64-pc-windows-msvc.zip"))
        self.assertEqual(sha, EXE_SHA)

    def test_missing_defaults_raise(self):
        with self.assertRaises(ValueError):
            cih.parse_defaults("param( [string]$Other = 'x' )")


class Sha256OfZenExeTests(unittest.TestCase):
    def test_hashes_single_zen_exe(self):
        with tempfile.TemporaryDirectory() as td:
            zp = Path(td) / "rel.zip"
            _make_zip(zp, {"zen.exe": EXE_BYTES, "README.txt": b"hi"})
            self.assertEqual(cih.sha256_of_zen_exe(zp), EXE_SHA)

    def test_no_zen_exe_raises(self):
        with tempfile.TemporaryDirectory() as td:
            zp = Path(td) / "rel.zip"
            _make_zip(zp, {"other.exe": b"x"})
            with self.assertRaises(ValueError):
                cih.sha256_of_zen_exe(zp)


class MainTests(unittest.TestCase):
    def _run(self, ps1_text, zip_entries):
        with tempfile.TemporaryDirectory() as td:
            ps1 = Path(td) / "install.ps1"
            ps1.write_text(ps1_text, encoding="utf-8")
            zp = Path(td) / "rel.zip"
            _make_zip(zp, zip_entries)
            return cih.main(["--install-ps1", str(ps1), "--zip", str(zp)])

    def test_matching_pin_passes(self):
        self.assertEqual(self._run(PS1, {"zen.exe": EXE_BYTES}), 0)

    def test_mismatched_pin_fails(self):
        bad = PS1.replace(EXE_SHA, "0" * 64)
        self.assertEqual(self._run(bad, {"zen.exe": EXE_BYTES}), 1)


if __name__ == "__main__":
    unittest.main()
