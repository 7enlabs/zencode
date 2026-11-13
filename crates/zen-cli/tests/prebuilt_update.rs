//! `zen update` on a prebuilt install (B5): the binary refuses with the
//! installer path instead of trying to update a source checkout. The
//! copied binary carries a `.zen-prebuilt` marker, so
//! `running_prebuilt_install` is true and `run_update --check` prints the
//! prebuilt status line and exits 0.
//!
//! Kills three update.rs mutants at once: `running_prebuilt_install`
//! -> false (:118), `||` -> `&&` in it (:121), and the deleted-`!` on
//! the `!quiet` print guard in the check branch (:144) - all three would
//! print a different line or none at all. Under the :118/:121 mutants
//! the check falls through to the "no local checkout" branch, which
//! never touches the network because `--check` returns before cloning.

mod common;

use common::*;
use std::process::Command;

#[test]
fn update_check_reports_prebuilt_install() {
    let dir = TempDir::new("prebuilt-update");
    let bin_name = if cfg!(windows) { "zen.exe" } else { "zen" };
    let bin = dir.path().join(bin_name);
    std::fs::copy(zen_bin(), &bin).unwrap();
    std::fs::write(dir.path().join(".zen-prebuilt"), "").unwrap();
    let out = Command::new(&bin)
        .arg("update")
        .arg("--check")
        .env("HOME", dir.path())
        .env_remove("ZENCODE_SRC_DIR")
        .env_remove("ZENCODE_REPO")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "status: {}\n{text}", out.status);
    assert!(
        text.contains("prebuilt install: re-run the installer from INSTALL.md to update"),
        "{text}"
    );
}
