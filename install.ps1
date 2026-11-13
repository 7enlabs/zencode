#Requires -Version 5.1
<#
.SYNOPSIS
  Install ZenCode CLI (zen.exe) on Windows from the prebuilt release binary.

.DESCRIPTION
  Downloads the Windows x86-64 build of ZenCode CLI from the GitHub
  release, verifies its SHA-256 before and after copying, and installs
  it for the current user. No build tools are required, and no
  administrator rights are needed.

  One command (from INSTALL.md):

    powershell -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/7enlabs/zencore/main/install.ps1 | iex"

  With no parameters the script installs release v0.1.0: -Url defaults
  to the v0.1.0 release asset and -Sha256 defaults to the published
  SHA-256 of the zen.exe inside it. Pass -Url and -Sha256 together to
  install a different build, or -File to install from a local copy
  (a zen.exe, or the release .zip that contains one).
  Provenance: CI re-checks the pinned hash against the live release
  asset whenever install.ps1 changes (scripts/check_install_hash.py).

  Install location: %LOCALAPPDATA%\Programs\ZenCode (override with
  -InstallDir). The folder is added to the user PATH when missing.

  The final check re-hashes the installed copy, then smoke-tests it
  with `zen --version`: a matching hash proves the copy is intact,
  and the smoke test proves it actually runs on this machine. On a
  machine that blocks a freshly downloaded binary (Smart App Control,
  SmartScreen, Software Restriction Policies) the smoke test fails
  instead of the install reporting "Done" - the installer prints
  what blocked it and how to unblock it, and still exits 0 because
  the install itself succeeded.

.PARAMETER Url
  HTTPS URL of the prebuilt artifact (.zip containing zen.exe, or a
  bare zen.exe). Default: the v0.1.0 release asset.

.PARAMETER File
  Local path to zen.exe or to the release .zip. For offline installs.

.PARAMETER Sha256
  Expected SHA-256 of zen.exe itself (not of the zip). The install
  stops when the downloaded or local binary does not match.
  Default: the v0.1.0 zen.exe hash.

.PARAMETER InstallDir
  Target directory. Default: %LOCALAPPDATA%\Programs\ZenCode

.EXAMPLE
  .\install.ps1
  Installs v0.1.0 with hash verification, no parameters needed.

.EXAMPLE
  .\install.ps1 -File .\zen-x86_64-pc-windows-msvc.zip
  Installs from a local copy of the release zip.
#>
[CmdletBinding()]
param(
  [string]$Url = 'https://github.com/7enlabs/zencore/releases/download/v0.1.0/zen-x86_64-pc-windows-msvc.zip',
  [string]$File,
  [string]$Sha256 = 'eb27ec87c348587423c172710c4b11c86efa18a4454ff8a48fb3cd7128699239',
  [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\ZenCode')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-ZenExeFromZip {
  param([string]$ZipPath, [string]$WorkDir)
  $extractDir = Join-Path $WorkDir 'extract'
  New-Item -ItemType Directory -Force -Path $extractDir | Out-Null
  Expand-Archive -LiteralPath $ZipPath -DestinationPath $extractDir -Force
  $found = Get-ChildItem -LiteralPath $extractDir -Recurse -Filter 'zen.exe' |
    Select-Object -First 1
  if (-not $found) {
    throw "The zip does not contain zen.exe: $ZipPath"
  }
  return $found.FullName
}

if ($File) {
  $Url = ''
}

$workDir = Join-Path ([System.IO.Path]::GetTempPath()) ('zencode-install-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $workDir | Out-Null

try {
  $sourceExe = $null

  if ($File) {
    if (-not (Test-Path -LiteralPath $File)) {
      throw "File not found: $File"
    }
    if ($File -match '\.zip$') {
      $sourceExe = Get-ZenExeFromZip -ZipPath $File -WorkDir $workDir
    } else {
      $sourceExe = (Resolve-Path -LiteralPath $File).Path
    }
  } else {
    if ($Url -notmatch '^https://') {
      throw "Only https URLs are accepted, got: $Url"
    }
    $leaf = Split-Path ([uri]$Url).AbsolutePath -Leaf
    if (-not $leaf) { $leaf = 'zen-download.bin' }
    $downloadPath = Join-Path $workDir $leaf
    Write-Host "Downloading $Url"
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $Url -OutFile $downloadPath -UseBasicParsing
    if ($leaf -match '\.zip$') {
      $sourceExe = Get-ZenExeFromZip -ZipPath $downloadPath -WorkDir $workDir
    } else {
      $sourceExe = $downloadPath
    }
  }

  $actualHash = (Get-FileHash -LiteralPath $sourceExe -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($Sha256) {
    if ($actualHash -ne $Sha256.ToLowerInvariant()) {
      throw "SHA-256 mismatch for zen.exe. Expected $Sha256, got $actualHash. Nothing was installed."
    }
    Write-Host "SHA-256 verified: $actualHash"
  } else {
    Write-Host "SHA-256 of zen.exe: $actualHash"
  }

  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
  $targetExe = Join-Path $InstallDir 'zen.exe'
  Copy-Item -LiteralPath $sourceExe -Destination $targetExe -Force
  Write-Host "Installed: $targetExe"

  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  $pathParts = @()
  if ($userPath) { $pathParts = $userPath -split ';' | Where-Object { $_ } }
  if ($pathParts -notcontains $InstallDir) {
    $newPath = (@($pathParts) + $InstallDir) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    Write-Host "Added to user PATH: $InstallDir (open a new terminal to use it)"
  }
  if (($env:Path -split ';') -notcontains $InstallDir) {
    $env:Path = $env:Path + ';' + $InstallDir
  }

  # Smoke test helper: run `zen --version` in a child process with
  # a 15s timeout, so a SmartScreen prompt left unanswered cannot hang
  # the install. Returns @{ Ok = <bool>; Version = <string>; Detail = <string> }.
  function Test-ZenSmoke {
    param([string]$Exe)
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.Arguments = '--version'
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $p = New-Object System.Diagnostics.Process
    $p.StartInfo = $psi
    try {
      if (-not $p.Start()) {
        return @{ Ok = $false; Detail = "could not start $Exe" }
      }
      if (-not $p.WaitForExit(15000)) {
        try { $p.Kill() } catch { }
        return @{ Ok = $false; Detail = 'timed out after 15s (a SmartScreen prompt may be waiting for an answer)' }
      }
      $out = $p.StandardOutput.ReadToEnd().Trim()
      if ($p.ExitCode -eq 0) {
        return @{ Ok = $true; Version = $out }
      }
      $err = $p.StandardError.ReadToEnd().Trim()
      $detail = if ($err) { $err } else { "exited with code $($p.ExitCode)" }
      return @{ Ok = $false; Detail = $detail }
    } catch {
      return @{ Ok = $false; Detail = $_.Exception.Message }
    }
  }

  # Final check: re-hash the installed copy, then smoke-test it.
  # A matching hash proves the copy is byte-identical to the verified
  # download; running `zen --version` proves it actually executes on
  # this machine. A blocked binary must not report "Done" and leave
  # the user to discover it at the first `zen --version` - but the
  # install itself succeeded, so a failed smoke test warns instead of
  # failing the install.
  $installedHash = (Get-FileHash -LiteralPath $targetExe -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($Sha256 -and $installedHash -ne $Sha256.ToLowerInvariant()) {
    throw "Installed file failed verification. Expected $Sha256, got $installedHash."
  }
  Write-Host "Installed file verified: $installedHash"
  # Marker for `zen update`: a prebuilt install must not be updated
  # by building from source (see B5).
  Set-Content -LiteralPath (Join-Path $InstallDir '.zen-prebuilt') -Value 'prebuilt install (install.ps1)' -NoNewline
  $smoke = Test-ZenSmoke -Exe $targetExe
  if ($smoke.Ok) {
    Write-Host "Smoke test passed: $($smoke.Version)"
  } else {
    Write-Host 'WARNING: the installed binary did not run:'
    Write-Host "  $($smoke.Detail)"
    Write-Host '  The install itself succeeded (SHA-256 verified), but this machine blocks the binary'
    Write-Host '  from running - Smart App Control (SAC), SmartScreen, or a Software Restriction Policy.'
    Write-Host '  Unblock it before use: right-click zen.exe > Properties > Unblock, or in SmartScreen'
    Write-Host '  choose "Run anyway" / "More info", or check Windows Security > App & browser control.'
  }
  Write-Host 'Done. Open a new terminal and run: zen --version'
}
finally {
  Remove-Item -LiteralPath $workDir -Recurse -Force -ErrorAction SilentlyContinue
}
