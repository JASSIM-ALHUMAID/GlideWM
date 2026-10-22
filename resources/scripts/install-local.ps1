# Installs a locally built GlideWM with UIAccess, so that it can manage
# elevated windows (e.g. Task Manager) without running as administrator.
#
# Windows only grants UIAccess to executables that are signed by a trusted
# certificate and installed in a secure location (e.g. Program Files). This
# script:
#   1. Builds the release binaries.
#   2. Elevates (one UAC prompt), then:
#      a. Creates a self-signed code signing certificate in the machine
#         store (once). Its private key can't be exported, and only
#         administrators can use it, so nothing running as the user can
#         sign code that the machine trusts.
#      b. Signs the binaries in an admin-only staging directory, so that
#         they can't be swapped between signing and installing.
#      c. Trusts the certificate machine-wide and moves the binaries to the
#         install directory.
#   3. Restarts GlideWM from the install directory and registers it to run
#      on login.
#
# Usage:
#   git clone https://github.com/JASSIM-ALHUMAID/glazewm.git
#   cd glazewm
#   ./resources/scripts/install-local.ps1
#
# To uninstall, delete the install directory, the `GlideWM` value under
# `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, and the certificate
# (subject `CN=GlideWM Local Code Signing`) from `Cert:\LocalMachine\My`
# (with its private key), `Cert:\LocalMachine\Root` and
# `Cert:\LocalMachine\TrustedPublisher`.
param(
  [string]$SourceDir = (Join-Path $PSScriptRoot "..\..\target\release"),
  # `ProgramW6432` is the 64-bit Program Files directory, also when this
  # script runs in a 32-bit shell (where `ProgramFiles` is the x86 one).
  [string]$InstallDir = (Join-Path $(
    if ($env:ProgramW6432) { $env:ProgramW6432 } else { $env:ProgramFiles }
  ) "GlideWM"),

  # Skips the build, and installs the binaries that are already in
  # `$SourceDir`.
  [switch]$SkipBuild,

  # Internal: runs the steps that need administrator rights.
  [switch]$Elevated,
  [string]$StagingDir
)

$ErrorActionPreference = "Stop"

$CertSubject = "CN=GlideWM Local Code Signing"
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

  # Prefer rustup, so that the nightly toolchain pinned by
  # `rust-toolchain.toml` is used even when a standalone `cargo` on the
  # PATH shadows it.
  if (Get-Command "rustup" -ErrorAction SilentlyContinue) {
    $cargoExe = "rustup"
    $cargoArgs = @("run", "nightly", "cargo")
  } else {
    $cargoExe = "cargo"
    $cargoArgs = @()
  }

  $repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path

  Push-Location $repoRoot

  try {
    & $cargoExe @cargoArgs build --release -p wm --features ui_access
    if ($LASTEXITCODE -ne 0) {
      throw "Failed to build the WM binary. The build needs a nightly Rust toolchain (see https://rustup.rs)."
    }

    & $cargoExe @cargoArgs build --release -p wm-watcher -p wm-cli
    if ($LASTEXITCODE -ne 0) {
      throw "Failed to build the watcher and CLI binaries."
    }
  }
  finally {
    Pop-Location
  }
}

# Returns the machine's code signing certificate, creating it if missing or
# about to expire. Runs elevated.
function Get-SigningCertificate() {
  $certificate = Get-ChildItem Cert:\LocalMachine\My -CodeSigningCert |
    Where-Object {
      $_.Subject -eq $CertSubject -and
      $_.HasPrivateKey -and
      $_.NotAfter -gt (Get-Date).AddDays(30)
    } |
    Sort-Object NotAfter -Descending |
    Select-Object -First 1

  if ($certificate) {
    return $certificate
  }

  Write-Host "Creating code signing certificate '$CertSubject'."

  # Machine keys of the software key storage provider are only accessible
  # to administrators and SYSTEM.
  return New-SelfSignedCertificate `
    -Type CodeSigningCert `
    -Subject $CertSubject `
    -KeyAlgorithm RSA `
    -KeyLength 3072 `
    -HashAlgorithm SHA256 `
    -KeyExportPolicy NonExportable `
    -Provider "Microsoft Software Key Storage Provider" `
    -NotAfter (Get-Date).AddYears(5) `
    -CertStoreLocation Cert:\LocalMachine\My
}

# Removes the certificates with the signing subject from the given store,
# except the one with the given thumbprint. Private keys are deleted along
# with their certificates.
function Remove-StaleCertificates([string]$StorePath, [string]$Thumbprint) {
  Get-ChildItem $StorePath |
    Where-Object {
      $_.Subject -eq $CertSubject -and $_.Thumbprint -ne $Thumbprint
    } |
    ForEach-Object {
      Write-Host "Removing stale certificate $($_.Thumbprint) from '$StorePath'."
      Remove-Item $_.PSPath -DeleteKey -ErrorAction SilentlyContinue
      if (Test-Path $_.PSPath) {
        Remove-Item $_.PSPath
      }
    }
}

# Signs the staged binaries, trusts the certificate machine-wide, and moves
# the binaries to the install directory. Runs elevated.
function Install-Elevated() {
  $certificate = Get-SigningCertificate

  # Binaries are copied out of the user-writable staging directory before
  # they're signed, so that they can't be swapped after signing. The
  # private directory inherits the admin-only ACL of Program Files.
  $privateStaging = "$InstallDir.staging"
  Remove-Item $privateStaging -Recurse -Force -ErrorAction SilentlyContinue

  try {
    foreach ($target in $Binaries.Values) {
      $staged = Join-Path $privateStaging $target
      New-Item -ItemType Directory -Force (Split-Path $staged) | Out-Null
      Copy-Item (Join-Path $StagingDir $target) $staged

      $signature = Set-AuthenticodeSignature -FilePath $staged `
        -Certificate $certificate -HashAlgorithm SHA256

      if (!$signature.SignerCertificate) {
        throw "Failed to sign '$target': $($signature.StatusMessage)"
      }
    }

    # The trusted stores only need the public certificate.
    $certPath = Join-Path $privateStaging "signing.cer"
    Export-Certificate -Cert $certificate -FilePath $certPath | Out-Null

    foreach ($store in @("Root", "TrustedPublisher")) {
      $storePath = "Cert:\LocalMachine\$store"

      if (!(Test-Path "$storePath\$($certificate.Thumbprint)")) {
        Import-Certificate -FilePath $certPath `
          -CertStoreLocation $storePath | Out-Null
      }

      Remove-StaleCertificates $storePath $certificate.Thumbprint
    }

    Remove-StaleCertificates "Cert:\LocalMachine\My" $certificate.Thumbprint

    foreach ($target in $Binaries.Values) {
      $destination = Join-Path $InstallDir $target
      New-Item -ItemType Directory -Force (Split-Path $destination) | Out-Null
      Copy-Item (Join-Path $privateStaging $target) $destination -Force
    }
  }
  finally {
    Remove-Item $privateStaging -Recurse -Force -ErrorAction SilentlyContinue
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

if ($Elevated) {
  # The elevated console closes on exit, so errors are passed back to the
  # calling script through a file.
  try {
    Install-Elevated
    Exit 0
  }
  catch {
    $_ | Out-String | Set-Content (Join-Path $StagingDir "error.log")
    Exit 1
  }
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

# UIAccess is only granted if requested in the manifest.
$manifestMatch = Select-String -Path (Join-Path $SourceDir "glidewm.exe") `
  -Pattern 'uiAccess="true"' -SimpleMatch -Quiet

if (!$manifestMatch) {
  throw "glidewm.exe wasn't built with the 'ui_access' feature."
}

$staging = Join-Path ([IO.Path]::GetTempPath()) "glidewm-install-$PID"

try {
  foreach ($entry in $Binaries.GetEnumerator()) {
    $staged = Join-Path $staging $entry.Value
    New-Item -ItemType Directory -Force (Split-Path $staged) | Out-Null
    Copy-Item (Join-Path $SourceDir $entry.Key) $staged
  }

  Stop-GlideWM

  Write-Host "Signing and installing to '$InstallDir' (requires administrator rights)."

  $shell = (Get-Process -Id $PID).Path
  $arguments = @(
    "-NoProfile", "-ExecutionPolicy", "Bypass",
    "-File", "`"$PSCommandPath`"",
    "-Elevated",
    "-StagingDir", "`"$staging`"",
    "-InstallDir", "`"$InstallDir`""
  )

  $process = Start-Process $shell -ArgumentList $arguments `
    -Verb RunAs -Wait -PassThru

  if ($process.ExitCode -ne 0) {
    $errorLog = Join-Path $staging "error.log"
    $details = Get-Content $errorLog -Raw -ErrorAction SilentlyContinue
    throw "Installation failed (exit code $($process.ExitCode)). $details"
  }
}
finally {
  Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
}

# Earlier versions of this script kept the signing key in the user's store,
# where anything running as the user could sign code with it.
Get-ChildItem Cert:\CurrentUser\My |
  Where-Object { $_.Subject -eq $CertSubject } |
  ForEach-Object {
    Write-Host "Removing legacy user signing certificate $($_.Thumbprint)."
    Remove-Item $_.PSPath -DeleteKey
  }

$executable = Join-Path $InstallDir "glidewm.exe"
$signature = Get-AuthenticodeSignature $executable

if ($signature.Status -ne "Valid") {
  throw "Installed glidewm.exe has an invalid signature: $($signature.StatusMessage)"
}

Set-ItemProperty $RunKey -Name "GlideWM" -Value "`"$executable`""

# Executables with UIAccess have to be launched through the shell (i.e.
# `ShellExecute`), which `Start-Process` does. GlideWM is launched without
# administrator rights, since this script isn't elevated.
Start-Process $executable

Write-Host "Installed and started '$executable'."
