# Installs a locally built GlideWM for the current user, without administrator
# rights (no UAC prompt).
#
# Unlike `install-local.ps1`, the binaries aren't built with UIAccess or
# signed, so GlideWM can't manage windows of elevated processes (e.g. Task
# Manager). Use `install-local.ps1` if that's needed.
#
# This script:
#   1. Builds the release binaries (without the `ui_access` feature).
#   2. Exits GlideWM if it's running.
#   3. Copies the binaries to the install directory.
#   4. Registers GlideWM to run on login, adds the CLI to the user's `PATH`,
#      and starts GlideWM.
#
# Usage:
#   git clone https://github.com/JASSIM-ALHUMAID/glazewm.git
#   cd glazewm
#   ./resources/scripts/install-local-user.ps1
#
# To uninstall, delete the install directory, the `GlideWM` value under
# `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, and the install
# directory's `cli` subdirectory from the user's `PATH`.
param(
  [string]$SourceDir = (Join-Path $PSScriptRoot "..\..\target\release"),
  [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Programs\GlideWM"),

  # Skips the build, and installs the binaries that are already in
  # `$SourceDir`.
  [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"

# Source file name -> path relative to the install directory. Matches the
# layout of the official installer.
$Binaries = [ordered]@{
  "glidewm.exe"         = "glidewm.exe"
  "glidewm-watcher.exe" = "glidewm-watcher.exe"
  "glidewm-cli.exe"     = "cli\glidewm.exe"
}

# Builds the release binaries. The build is incremental, so it's quick when
# nothing has changed since the last one.
function Build-Binaries() {
  Write-Host "Building release binaries. This can take a while on the first run."

  $repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path

  Push-Location $repoRoot

  # Restored afterwards, since a script's environment changes outlive it in
  # the calling shell.
  $previousRustc = $env:RUSTC

  try {
    $cargoExe = "cargo"

    # Prefer rustup, so that the nightly toolchain pinned by
    # `rust-toolchain.toml` is used even when a standalone (e.g. stable)
    # install comes first on the PATH.
    if (Get-Command "rustup" -ErrorAction SilentlyContinue) {
      # `rustup which` resolves the toolchain of the current directory.
      $cargoExe = (& rustup which cargo | Out-String).Trim()
      $rustcExe = (& rustup which rustc | Out-String).Trim()

      if ($LASTEXITCODE -ne 0 -or !$cargoExe -or !$rustcExe) {
        throw "Failed to resolve the toolchain pinned by 'rust-toolchain.toml'. Install it with 'rustup toolchain install nightly'."
      }

      # Cargo looks `rustc` up on the PATH, where a standalone install can
      # shadow the toolchain's compiler.
      $env:RUSTC = $rustcExe
    }

    & $cargoExe build --release -p wm -p wm-watcher -p wm-cli
    if ($LASTEXITCODE -ne 0) {
      throw "Failed to build the binaries. The build needs a nightly Rust toolchain (see https://rustup.rs)."
    }
  }
  finally {
    $env:RUSTC = $previousRustc
    Pop-Location
  }
}

# Exits GlideWM if it's running, and waits for it to clean up.
function Stop-GlideWM() {
  if (!(Get-Process glidewm -ErrorAction SilentlyContinue)) {
    return
  }

  Write-Host "Exiting running GlideWM."
  & (Join-Path $SourceDir "glidewm-cli.exe") command wm-exit | Out-Null

  # The watcher restores windows after the main process exits. Polled,
  # since `Wait-Process` can't open 64-bit processes from a 32-bit shell.
  $deadline = (Get-Date).AddSeconds(30)

  while (Get-Process glidewm, glidewm-watcher -ErrorAction SilentlyContinue) {
    if ((Get-Date) -gt $deadline) {
      throw "GlideWM didn't exit. Exit it manually and run this script again."
    }

    Start-Sleep -Milliseconds 250
  }
}

# Appends a directory to the user's `PATH` if it's missing. Only affects
# shells started afterwards.
function Add-UserPath([string]$Directory) {
  $current = [Environment]::GetEnvironmentVariable("Path", "User")
  $entries = @($current -split ";" | Where-Object { $_ })

  $normalized = $Directory.TrimEnd("\")
  if ($entries | Where-Object { $_.TrimEnd("\") -ieq $normalized }) {
    return
  }

  [Environment]::SetEnvironmentVariable(
    "Path", (($entries + $Directory) -join ";"), "User")
}

$SourceDir = (Resolve-Path $SourceDir).Path

if (!$SkipBuild) {
  Build-Binaries
}

foreach ($source in $Binaries.Keys) {
  if (!(Test-Path (Join-Path $SourceDir $source))) {
    throw "Missing '$source' in '$SourceDir'. The build didn't produce it."
  }
}

# An unsigned executable that requests UIAccess fails to launch.
$manifestMatch = Select-String -Path (Join-Path $SourceDir "glidewm.exe") `
  -Pattern 'uiAccess="true"' -SimpleMatch -Quiet

if ($manifestMatch) {
  throw "glidewm.exe was built with the 'ui_access' feature, and can't run unsigned. Rebuild without '-SkipBuild'."
}

# Both installs register the same `Run` value and process name.
$signedInstall = Join-Path $(
  if ($env:ProgramW6432) { $env:ProgramW6432 } else { $env:ProgramFiles }
) "GlideWM\glidewm.exe"

if (Test-Path $signedInstall) {
  Write-Warning "A signed install exists at '$signedInstall'. This install replaces its login entry; remove the signed install to avoid running both."
}

Stop-GlideWM

Write-Host "Installing to '$InstallDir'."

foreach ($entry in $Binaries.GetEnumerator()) {
  $destination = Join-Path $InstallDir $entry.Value
  New-Item -ItemType Directory -Force (Split-Path $destination) | Out-Null
  Copy-Item (Join-Path $SourceDir $entry.Key) $destination -Force
}

Add-UserPath (Join-Path $InstallDir "cli")

$executable = Join-Path $InstallDir "glidewm.exe"
Set-ItemProperty $RunKey -Name "GlideWM" -Value "`"$executable`""

Start-Process $executable

Write-Host "Installed and started '$executable'."
