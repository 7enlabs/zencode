# ZenCode MSIX packaging (Microsoft Store)

Packages the Windows build of `zen.exe` as an MSIX for the Microsoft
Store. A Store package is re-signed by Microsoft, so users install
without SmartScreen / Smart App Control warnings.

Status: **template, identity filled - ready to pack once the logo assets land.**

1. **Identity** - filled from the Partner Center reservation
   (team lead, 2026-11-10): Name 7ENLabs.ZenCode, Publisher
   CN=3B32146C-7F74-4349-BE62-4416DB18376F, Version 0.1.0.0,
   Store ID 9NK3CWNMJKM9, Package Family Name
   7ENLabs.ZenCode_meqb5mtvkpysg. Do not invent values, an upload
   with a mismatched Publisher is rejected. PublisherDisplayName
   7EN Labs is the plan proposal, pending confirmation by Zen.
2. **Logo assets** - the PNG files listed in `Assets/README.md`, from
   the brand owner. None exist yet; do not fabricate them.

## Layout

- `AppxManifest.xml` - package manifest template: `runFullTrust`,
  `packagedClassicApp`, and a `uap5:AppExecutionAlias` so the command
  `zen` resolves from any terminal after install (Windows creates the
  alias under `%LOCALAPPDATA%\Microsoft\WindowsApps`, already on the
  user PATH).
- `Assets/` - logo files the manifest references.
- `build-msix.ps1` - staging + `makeappx.exe pack` (+ optional signing
  for sideload tests).

## Build

Prerequisites: Windows 10 1809 or later, Windows 10/11 SDK
(`makeappx.exe`, `makepri.exe`, `signtool.exe`). The GitHub-hosted
`windows-latest` runners already used by CI carry the SDK, so packing
can move into a workflow later; for now the script runs on any Windows
machine with the SDK installed.

```powershell
# zen.exe from the release CI artifact (zen-x86_64-pc-windows-msvc)
.\build-msix.ps1 -ExePath C:\path\to\zen.exe
```

The script still refuses to pack if the manifest carries the
placeholder Publisher (guard against an unfilled template); with the
real identity in place, a plain run packs the Store package. For a
sideload test build, pass identity overrides:

```powershell
# Sideload test only: identity must match the test certificate subject.
.\build-msix.ps1 -ExePath .\zen.exe `
  -NameOverride Contoso.ZenCodeTest -PublisherOverride "CN=ZenCodeTest" `
  -PfxPath .\test-cert.pfx -PfxPassword <password>
```

Output: `build\ZenCode_<version>_x64.msix` next to this README.
Store submissions do not need self-signing - Microsoft re-signs the
package. Self-signing is only for the local sideload test below.

## Sideload test (before any Store submission)

1. Create a test code-signing certificate whose subject equals the
   manifest Publisher used for the test build
   (`New-SelfSignedCertificate -Type CodeSigningCert`).
2. Trust it on the test machine (`Trusted People` store), sign the
   package with `build-msix.ps1 -PfxPath/-PfxPassword`.
3. Install: `Add-AppxPackage -Path .\ZenCode_0.1.0.0_x64.msix`
   (or `Add-AppxPackage -Register AppxManifest.xml` from a staging
   folder to test the unpacked layout).
4. Verify in a **new** terminal: `zen --version`, `zen --help`;
   `Get-AppxPackage -Name *ZenCode*` lists the package; the alias
   disappears again after `Remove-AppxPackage <PackageFullName>`.
5. Record the result (machine, Windows build, pass/fail per step)
   with the submission notes.

Known risks: the `zen.exe` alias can lose to another app that already
registered the same alias on the user's machine; `runFullTrust`
packages get extra questions in Store certification, so the listing
description must state plainly that the app runs shell commands at the
user's request.

## Version metadata

`zen.exe` embeds a Windows VERSIONINFO resource (FileVersion,
ProductName, CompanyName, ...) from `crates/zen-cli/build.rs` - the
Store and SignPath both expect a versioned binary. The MSIX package
version in the manifest is separate and must be bumped on every
submission.
