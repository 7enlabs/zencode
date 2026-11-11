#!/usr/bin/env python3
"""Verify install.ps1's pinned SHA-256 against the release asset.

install.ps1 pins the SHA-256 of the zen.exe inside the release zip it
downloads. A pin is only provenance if something re-checks it against the
published asset: this script parses the -Url / -Sha256 defaults out of
install.ps1, obtains the zip, hashes the zen.exe inside it, and fails on
any mismatch.

The zip is obtained with `gh release download` when the gh CLI is
available (the authenticated path, used by CI for this private repo) and
otherwise with a plain HTTPS GET of the pinned URL. `--zip PATH` checks a
local zip instead of downloading, which keeps the logic testable offline.

Usage:
  python3 scripts/check_install_hash.py [--install-ps1 PATH] [--zip PATH]

Exit code 0 = pin matches the asset, 1 = mismatch or unreadable input.
"""
import argparse
import hashlib
import re
import subprocess
import sys
import tempfile
import urllib.request
import zipfile
from pathlib import Path

URL_RE = re.compile(r"\[string\]\$Url\s*=\s*'([^']+)'")
SHA_RE = re.compile(r"\[string\]\$Sha256\s*=\s*'([0-9a-fA-F]{64})'")
RELEASE_RE = re.compile(r"/releases/download/([^/]+)/([^/]+)$")


def parse_defaults(text):
    """Return (url, sha256) parsed from install.ps1 parameter defaults."""
    url = URL_RE.search(text)
    sha = SHA_RE.search(text)
    if not url or not sha:
        raise ValueError("could not parse -Url / -Sha256 defaults from install.ps1")
    return url.group(1), sha.group(1).lower()


def sha256_of_zen_exe(zip_path):
    """SHA-256 of the zen.exe entry inside a release zip."""
    with zipfile.ZipFile(zip_path) as zf:
        names = [n for n in zf.namelist() if n.endswith("zen.exe")]
        if len(names) != 1:
            raise ValueError(f"expected exactly one zen.exe in zip, found {names}")
        return hashlib.sha256(zf.read(names[0])).hexdigest()


def download_zip(url, dest_dir):
    """Download the release zip for `url` into dest_dir; return its path."""
    m = RELEASE_RE.search(url)
    if not m:
        raise ValueError(f"URL is not a GitHub release download URL: {url}")
    tag, asset = m.group(1), m.group(2)
    dest = Path(dest_dir) / asset
    if subprocess.run(["which", "gh"], capture_output=True).returncode == 0:
        subprocess.run(
            ["gh", "release", "download", tag, "--pattern", asset, "--dir", dest_dir],
            check=True,
        )
    else:
        req = urllib.request.Request(url, headers={"User-Agent": "check-install-hash"})
        with urllib.request.urlopen(req, timeout=120) as resp, open(dest, "wb") as fh:
            fh.write(resp.read())
    if not dest.exists():
        raise ValueError(f"download did not produce {dest}")
    return dest


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--install-ps1", default="install.ps1")
    ap.add_argument("--zip", dest="zip_path", default=None,
                    help="check this local zip instead of downloading")
    args = ap.parse_args(argv)

    try:
        url, pinned = parse_defaults(Path(args.install_ps1).read_text(encoding="utf-8"))
        if args.zip_path:
            zip_path = Path(args.zip_path)
        else:
            tmp = tempfile.mkdtemp(prefix="install-hash-")
            zip_path = download_zip(url, tmp)
        actual = sha256_of_zen_exe(zip_path)
    except Exception as exc:  # unreadable input is a failed check, not a crash
        print(f"FAIL: could not verify install.ps1 pin: {exc}")
        return 1

    print(f"install.ps1 pin : {pinned}")
    print(f"release asset   : {actual}  ({url})")
    if actual != pinned:
        print("FAIL: pinned SHA-256 does not match the release asset")
        return 1
    print("OK: pinned SHA-256 matches the release asset")
    return 0


if __name__ == "__main__":
    sys.exit(main())
