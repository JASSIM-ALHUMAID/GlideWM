# Builds a signed x64 MSI installer of GlideWM, with UIAccess, for this
# machine.
#
# Unlike `install-local.ps1`, the MSI registers GlideWM like a regular app:
# an "Apps & features" entry (with uninstall + upgrade support), a Start Menu
# shortcut, an optional desktop shortcut, and the CLI on the `PATH`.
#
# The binaries are signed with the same machine-wide self-signed certificate
# as `install-local.ps1` (see `local-signing.ps1`), which is also what grants
# UIAccess. The certificate is only trusted on machines that ran this script
# or `install-local.ps1`, so the MSI isn't meant to be installed elsewhere.
#
# This script:
#   1. Builds the release binaries.
#   2. Elevates (one UAC prompt), then signs the binaries in an admin-only
#      staging directory, trusts the certificate, builds the MSI with WiX,
#      and signs the MSI.
#
# Requires the WiX CLI and its extensions:
#   dotnet tool install --global wix --version 5.0.2
#   wix extension add --global WixToolset.UI.wixext/5.0.2
#   wix extension add --global WixToolset.Util.wixext/5.0.1
#
# Usage:
#   ./resources/scripts/build-msi.ps1 -VersionNumber 1.0.0
param(
  # Version of the MSI (`major.minor.patch`), also shown in the CLI and tray.
  [string]$VersionNumber = "0.0.0",

  [string]$SourceDir = (Join-Path $PSScriptRoot "..\..\target\release"),
  [string]$OutDir = (Join-Path $PSScriptRoot "..\..\out"),

  # Skips the build, and packages the binaries that are already in
  # `$SourceDir`.
  [switch]$SkipBuild,

  # Internal: runs the steps that need administrator rights.
  [switch]$Elevated,
  [string]$StagingDir
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "local-signing.ps1")

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path

# Source file name -> file name expected by `standalone.wxs`.
$Binaries = [ordered]@{
  "glidewm.exe"         = "glidewm.exe"
  "glidewm-watcher.exe" = "glidewm-watcher.exe"
  "glidewm-cli.exe"     = "glidewm-cli.exe"
}

# Signs the staged binaries, builds the MSI from them, and signs it. The MSI
# is written to `$OutDir`. Runs elevated.
function Build-Installer() {
  $certificate = Get-SigningCertificate

  # Binaries are copied out of the user-writable staging directory before
  # they're signed, so that they can't be swapped after signing. The
  # private directory inherits the admin-only ACL of Program Files.
  $programFiles = if ($env:ProgramW6432) { $env:ProgramW6432 } else { $env:ProgramFiles }
  $privateStaging = Join-Path $programFiles "GlideWM.msi-staging"
  Remove-Item $privateStaging -Recurse -Force -ErrorAction SilentlyContinue

  try {
    $exeDir = Join-Path $privateStaging "x64"
    New-Item -ItemType Directory -Force $exeDir | Out-Null

    foreach ($target in $Binaries.Values) {
      $staged = Join-Path $exeDir $target
      Copy-Item (Join-Path $StagingDir $target) $staged
      Sign-File $staged $certificate
    }

    Install-SigningTrust $certificate $privateStaging

    $msi = Join-Path $privateStaging "GlideWM-$VersionNumber-x64.msi"

    # Paths in the `.wxs` files are relative to the repository root.
    Push-Location $RepoRoot

    try {
      wix build -arch x64 -ext WixToolset.UI.wixext -ext WixToolset.Util.wixext `
        -out $msi "resources/wix/standalone.wxs" "resources/wix/standalone-ui.wxs" `
        -d VERSION_NUMBER="$VersionNumber" `
        -d EXE_DIR="$exeDir"

      if ($LASTEXITCODE -ne 0) {
        throw "Failed to build the MSI. Is the WiX CLI installed? See the header of this script."
      }
    }
    finally {
      Pop-Location
    }

    Sign-File $msi $certificate

    New-Item -ItemType Directory -Force $OutDir | Out-Null
    Copy-Item $msi $OutDir -Force
  }
  finally {
    Remove-Item $privateStaging -Recurse -Force -ErrorAction SilentlyContinue
  }
}

if ($Elevated) {
  # The elevated console closes on exit, so errors are passed back to the
  # calling script through a file.
  try {
    Build-Installer
    Exit 0
  }
  catch {
    $_ | Out-String | Set-Content (Join-Path $StagingDir "error.log")
    Exit 1
  }
}

if ($VersionNumber -notmatch '^\d+\.\d+\.\d+$') {
  throw "Invalid version '$VersionNumber'. Expected 'major.minor.patch'."
}

if (!(Get-Command "wix" -ErrorAction SilentlyContinue)) {
  throw "The WiX CLI isn't installed. See the header of this script."
}

if (!$SkipBuild) {
  # Compiled into the binaries for the CLI and tray.
  $env:VERSION_NUMBER = $VersionNumber
  Build-Binaries
}

if (!(Test-Path $SourceDir)) {
  throw "Source directory '$SourceDir' doesn't exist. It's produced by the release build; omit -SkipBuild, or pass -SourceDir."
}

$SourceDir = (Resolve-Path $SourceDir).Path

foreach ($source in $Binaries.Keys) {
  if (!(Test-Path (Join-Path $SourceDir $source))) {
    throw "Missing '$source' in '$SourceDir'. The build didn't produce it."
  }
}

# UIAccess is only granted if requested in the manifest.
$manifestMatch = Select-String -Path (Join-Path $SourceDir "glidewm.exe") `
  -Pattern 'uiAccess="true"' -SimpleMatch -Quiet

if (!$manifestMatch) {
  throw "glidewm.exe wasn't built with the 'ui_access' feature."
}

$staging = Join-Path ([IO.Path]::GetTempPath()) "glidewm-msi-$PID"

try {
  New-Item -ItemType Directory -Force $staging | Out-Null

  foreach ($entry in $Binaries.GetEnumerator()) {
    Copy-Item (Join-Path $SourceDir $entry.Key) (Join-Path $staging $entry.Value)
  }

  Write-Host "Signing and packaging (requires administrator rights)."

  $shell = (Get-Process -Id $PID).Path
  $arguments = @(
    "-NoProfile", "-ExecutionPolicy", "Bypass",
    "-File", "`"$PSCommandPath`"",
    "-Elevated",
    "-VersionNumber", $VersionNumber,
    "-StagingDir", "`"$staging`"",
    "-OutDir", "`"$((New-Item -ItemType Directory -Force $OutDir).FullName)`""
  )

  $process = Start-Process $shell -ArgumentList $arguments `
    -Verb RunAs -Wait -PassThru

  if ($process.ExitCode -ne 0) {
    $errorLog = Join-Path $staging "error.log"
    $details = Get-Content $errorLog -Raw -ErrorAction SilentlyContinue
    throw "Packaging failed (exit code $($process.ExitCode)). $details"
  }
}
finally {
  Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "Built '$(Join-Path (Resolve-Path $OutDir).Path "GlideWM-$VersionNumber-x64.msi")'."
