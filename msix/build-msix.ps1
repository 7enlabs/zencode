<#
.SYNOPSIS
  Pack ZenCode as an MSIX from a built zen.exe (manual MSIX flow,
  Microsoft Learn "Generating MSIX package components").

.DESCRIPTION
  Stages zen.exe + AppxManifest.xml + Assets into a staging folder,
  runs MakePri when qualifier assets are present, then
  makeappx.exe pack. Optionally signs the package for a sideload test.

  The manifest in this repo is a TEMPLATE: packing stops while the
  Publisher still holds the placeholder value, unless -NameOverride
  and -PublisherOverride are passed (sideload test builds, where the
  Publisher must equal the test certificate subject).

  Requires the Windows 10/11 SDK (makeappx.exe, signtool.exe).
  Store submissions are re-signed by Microsoft; -PfxPath signing is
  only for local sideload tests.

.EXAMPLE
  .\build-msix.ps1 -ExePath C:\builds\zen.exe

.EXAMPLE
  .\build-msix.ps1 -ExePath .\zen.exe -NameOverride Contoso.ZenCodeTest `
    -PublisherOverride "CN=ZenCodeTest" -PfxPath .\test-cert.pfx `
    -PfxPassword secret
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$ExePath,

  [string]$ManifestPath = "$PSScriptRoot\AppxManifest.xml",
  [string]$AssetsDir = "$PSScriptRoot\Assets",
  [string]$OutputDir = "$PSScriptRoot\build",

  # Four-part package version, e.g. 0.1.0.0. Defaults to the manifest.
  [string]$Version,

  # Identity overrides, applied to the staged copy only (the template
  # in the repo is never edited). For sideload tests the Publisher
  # must match the signing certificate subject.
  [string]$NameOverride,
  [string]$PublisherOverride,

  # Sideload-test signing. Never used for Store submissions.
  [string]$PfxPath,
  [string]$PfxPassword,

  [switch]$KeepStaging
)

$ErrorActionPreference = 'Stop'

function Find-SdkTool([string]$Name) {
  $cmd = Get-Command $Name -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }
  $kits = 'C:\Program Files (x86)\Windows Kits\10\bin'
  if (Test-Path $kits) {
    $hit = Get-ChildItem $kits -Directory |
      Sort-Object Name -Descending |
      ForEach-Object { Join-Path $_.FullName "x64\$Name" } |
      Where-Object { Test-Path $_ } |
      Select-Object -First 1
    if ($hit) { return $hit }
  }
  throw "$Name not found. Install the Windows 10/11 SDK first."
}

$placeholderPublisher = 'CN=00000000-0000-0000-0000-000000000000'

if (-not (Test-Path $ExePath)) { throw "zen.exe not found: $ExePath" }
if (-not (Test-Path $ManifestPath)) { throw "Manifest not found: $ManifestPath" }

$manifestText = Get-Content $ManifestPath -Raw

$identity = [regex]::Match($manifestText, '<Identity[^>]*Name="([^"]+)"[^>]*Version="([^"]+)"[^>]*Publisher="([^"]+)"')
if (-not $identity.Success) { throw 'Could not parse the <Identity> element of the manifest.' }
$pkgName = $identity.Groups[1].Value
$pkgVersion = $identity.Groups[2].Value
$pkgPublisher = $identity.Groups[3].Value

if ($NameOverride) { $pkgName = $NameOverride }
if ($PublisherOverride) { $pkgPublisher = $PublisherOverride }
if ($Version) { $pkgVersion = $Version }

if ($pkgPublisher -eq $placeholderPublisher) {
  throw 'Manifest Publisher is still the template placeholder. Fill in the exact Publisher from Partner Center, or pass -PublisherOverride for a sideload test build.'
}
if ($pkgVersion -notmatch '^\d+\.\d+\.\d+\.\d+$') {
  throw "Package version must have four parts, got: $pkgVersion"
}

# Assets the manifest references must exist, or makeappx fails later
# with a less clear error. Check up front.
$requiredAssets = @('Square150x150Logo.png', 'Square44x44Logo.png', 'StoreLogo.png')
$missing = $requiredAssets | Where-Object { -not (Test-Path (Join-Path $AssetsDir $_)) }
if ($missing) {
  throw "Missing package assets in $AssetsDir : $($missing -join ', '). See Assets\README.md - assets come from the brand owner."
}

$makeappx = Find-SdkTool 'makeappx.exe'
$signtool = $null
if ($PfxPath) { $signtool = Find-SdkTool 'signtool.exe' }

$staging = Join-Path ([System.IO.Path]::GetTempPath()) "zencode-msix-$pkgVersion"
if (Test-Path $staging) { Remove-Item $staging -Recurse -Force }
New-Item -ItemType Directory -Path $staging | Out-Null
New-Item -ItemType Directory -Path (Join-Path $staging 'Assets') | Out-Null

try {
  Copy-Item $ExePath (Join-Path $staging 'zen.exe')
  Get-ChildItem $AssetsDir -File |
    Where-Object { $_.Extension -in '.png', '.pri' } |
    Copy-Item -Destination (Join-Path $staging 'Assets')

  $stagedManifest = $manifestText
  $stagedManifest = $stagedManifest -replace 'Name="[^"]+"(?=[^>]*ProcessorArchitecture)', "Name=`"$pkgName`""
  $stagedManifest = $stagedManifest -replace 'Version="[^"]+"(?=[^>]*ProcessorArchitecture)', "Version=`"$pkgVersion`""
  $stagedManifest = $stagedManifest -replace 'Publisher="[^"]+"', "Publisher=`"$pkgPublisher`""
  Set-Content -Path (Join-Path $staging 'AppxManifest.xml') -Value $stagedManifest -Encoding UTF8

  # MakePri is only needed when assets use qualifiers (scale-,
  # targetsize-, ...). Plain fixed-size logos are packed as-is.
  $qualified = Get-ChildItem (Join-Path $staging 'Assets') -File |
    Where-Object { $_.Name -match 'scale-|targetsize-' }
  if ($qualified) {
    $makepri = Find-SdkTool 'makepri.exe'
    & $makepri createconfig /cf (Join-Path $staging 'priconfig.xml') /dq en-US | Out-Null
    & $makepri new /pr $staging /cf (Join-Path $staging 'priconfig.xml') | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "makepri failed with exit code $LASTEXITCODE" }
  }

  if (-not (Test-Path $OutputDir)) { New-Item -ItemType Directory -Path $OutputDir | Out-Null }
  $msixPath = Join-Path $OutputDir "ZenCode_${pkgVersion}_x64.msix"

  & $makeappx pack /d $staging /p $msixPath /o
  if ($LASTEXITCODE -ne 0) { throw "makeappx pack failed with exit code $LASTEXITCODE" }
  Write-Host "Packed: $msixPath"

  if ($PfxPath) {
    if (-not (Test-Path $PfxPath)) { throw "Certificate not found: $PfxPath" }
    $signArgs = @('sign', '/fd', 'SHA256', '/f', $PfxPath)
    if ($PfxPassword) { $signArgs += @('/p', $PfxPassword) }
    $signArgs += $msixPath
    & $signtool @signArgs
    if ($LASTEXITCODE -ne 0) { throw "signtool failed with exit code $LASTEXITCODE" }
    Write-Host 'Signed for sideload test. Store submissions are re-signed by Microsoft - do not submit this file.'
  }

  Write-Host "Identity: $pkgName, $pkgVersion, $pkgPublisher"
}
finally {
  if (-not $KeepStaging -and (Test-Path $staging)) {
    Remove-Item $staging -Recurse -Force
  } elseif ($KeepStaging) {
    Write-Host "Staging kept at: $staging"
  }
}
