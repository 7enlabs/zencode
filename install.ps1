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

    powershell -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/7enlabs/zencode/main/install.ps1 | iex"

  With no parameters the script installs release v0.1.0: -Url defaults
  to the v0.1.0 release asset and -Sha256 defaults to the published
  SHA-256 of the zen.exe inside it. Pass -Url and -Sha256 together to
  install a different build, or -File to install from a local copy
  (a zen.exe, or the release .zip that contains one).

  Install location: %LOCALAPPDATA%\Programs\ZenCode (override with
  -InstallDir). The folder is added to the user PATH when missing.

  The final check re-hashes the installed file instead of executing
  it: some machines block a freshly downloaded binary from running
  (for example through Software Restriction Policies), and a matching
  hash already proves the installed copy is intact. Run
  `zen --version` yourself once the install finishes.

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
  [string]$Url = 'https://github.com/7enlabs/zencode/releases/download/v0.1.0/zen-x86_64-pc-windows-msvc.zip',
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

  # Final check: re-hash the installed copy instead of executing it.
  # A matching hash proves the copy is byte-identical to the verified
  # download, and this stays reliable on machines that block a fresh
  # binary from running (Software Restriction Policies and similar).
  $installedHash = (Get-FileHash -LiteralPath $targetExe -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($Sha256 -and $installedHash -ne $Sha256.ToLowerInvariant()) {
    throw "Installed file failed verification. Expected $Sha256, got $installedHash."
  }
  Write-Host "Installed file verified: $installedHash"
  Write-Host 'Done. Open a new terminal and run: zen --version'
}
finally {
  Remove-Item -LiteralPath $workDir -Recurse -Force -ErrorAction SilentlyContinue
}
