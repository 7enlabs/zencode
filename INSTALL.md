# Install ZenCode CLI

## Windows: prebuilt binary (recommended)

Release v0.1.0 ships a prebuilt Windows binary, so there is nothing
to compile. One command in PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/7enlabs/zencode/main/install.ps1 | iex"
```

Safer alternative, download first and read the script before running:

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/7enlabs/zencode/main/install.ps1 -OutFile install.ps1
.\install.ps1
```

What the script does, and nothing else:

1. Downloads the v0.1.0 release asset
   `zen-x86_64-pc-windows-msvc.zip` from GitHub Releases.
2. Verifies the SHA-256 of the `zen.exe` inside it against the
   published hash `eb27ec87c348587423c172710c4b11c86efa18a4454ff8a48fb3cd7128699239`.
   If the hash does not match, the script stops and installs nothing.
3. Copies `zen.exe` to `%LOCALAPPDATA%\Programs\ZenCode` and adds
   that directory to the user PATH if it is not there yet.
4. Re-hashes the installed copy as a final check instead of running
   it, so the install still succeeds on machines that block a freshly
   downloaded binary from executing.

No administrator rights are needed, and no toolchain is installed.
Open a new terminal after installing, then run `zen --version`.

To install a different build, pass both values explicitly:

```powershell
.\install.ps1 -Url <asset-url> -Sha256 <sha256-of-zen.exe>
```

Or install offline from a local copy of the release zip:

```powershell
.\install.ps1 -File .\zen-x86_64-pc-windows-msvc.zip
```

Linux and macOS still install from source for now — there is no
prebuilt binary for those platforms in v0.1.0:

```sh
curl -fsSL https://raw.githubusercontent.com/7enlabs/zencode/main/install.sh | sh
```

## Where the binary goes

- Windows: `%LOCALAPPDATA%\Programs\ZenCode\zen.exe`. The script
  adds that directory to the user PATH if it is not there yet.
- Linux and macOS: `~/.local/bin/zen`. If that directory is not on
  your PATH, the script prints the export line to add.

Open a new terminal after installing.

## Update

How you update depends on how you installed.

**Prebuilt install (the one-command install above):** re-run the
same one-command install. It downloads the newest release build
and replaces the binary in place. `zen update` does not apply to
prebuilt installs - it rebuilds from source (see below).

**Source install:** run:

```sh
zen update
```

This fast-forwards the source checkout, rebuilds `zen-cli` in
release mode, and replaces the binary you are running. If no
checkout exists yet, it clones one first. It needs Git and a Rust
toolchain (`cargo`) - the prebuilt install deliberately has
neither. Use `zen update --check` to see whether an update is
available without building anything. If the checkout has
uncommitted changes, update stops instead of overwriting them.

## Build from source

If you prefer to build the CLI yourself, you need Git and a Rust
toolchain (`cargo`). The build can take a few minutes.

```sh
git clone https://github.com/7enlabs/zencode.git
cd zencode
cargo build --release -p zen-cli
cp target/release/zen ~/.local/bin/zen
```

On Windows, copy `target\release\zen.exe` instead of `zen`.

## Uninstall

Remove the binary and the checkout:

```sh
rm ~/.local/bin/zen
rm -rf ~/.local/share/zencode
```

On Windows, remove `%LOCALAPPDATA%\Programs\ZenCode\zen.exe` and
`%USERPROFILE%\.local\share\zencode`, and remove the install
directory from the user PATH if you no longer want it there.

## Troubleshooting

- The binary does not start right after installing: some machines
  block freshly downloaded executables (Software Restriction
  Policies, SmartScreen). The installer verifies the installed copy
  by hash, so a completed install is intact; check your machine's
  execution policy if `zen` still will not run.
- `cargo not found` (source builds only): install Rust first, for
  example from https://rustup.rs, then rerun the build.
- `git not found`: install Git first, then rerun.
- The build needs network access once, for the Rust crate registry.
  After that, the binary runs against any OpenAI-compatible endpoint,
  including fully offline local models. See the README for providers.
