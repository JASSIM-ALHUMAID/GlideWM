# Installs a locally built GlazeWM with UIAccess, so that it can manage
# elevated windows (e.g. Task Manager) without running as administrator.
#
# Windows only grants UIAccess to executables that are signed by a trusted
# certificate and installed in a secure location (e.g. Program Files). This
# script:
#   1. Creates a self-signed code signing certificate for the current user
#      (once; its private key can't be exported).
#   2. Signs the binaries with it.
#   3. Trusts the certificate machine-wide and copies the binaries to the
#      install directory (one UAC prompt).
#   4. Restarts GlazeWM from the install directory and registers it to run
#      on login.
#
# Usage:
#   cargo build --release -p wm --features ui_access
#   cargo build --release -p wm-watcher -p wm-cli
#   ./resources/scripts/install-local.ps1
#
# To uninstall, delete the install directory, the `GlazeWM` value under
# `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, and the certificate
# (subject `CN=GlazeWM Local Code Signing`) from `Cert:\CurrentUser\My`,
# `Cert:\LocalMachine\Root` and `Cert:\LocalMachine\TrustedPublisher`.
param(
  [string]$SourceDir = (Join-Path $PSScriptRoot "..\..\target\release"),
  # `ProgramW6432` is the 64-bit Program Files directory, also when this
  # script runs in a 32-bit shell (where `ProgramFiles` is the x86 one).
  [string]$InstallDir = (Join-Path $(
    if ($env:ProgramW6432) { $env:ProgramW6432 } else { $env:ProgramFiles }
  ) "GlazeWM Fork"),

  # Internal: runs the steps that need administrator rights.
  [switch]$Elevated,
  [string]$StagingDir,
  [string]$CertPath
)

$ErrorActionPreference = "Stop"

$CertSubject = "CN=GlazeWM Local Code Signing"
$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"

# Source file name -> path relative to the install directory. Matches the
# layout of the official installer.
$Binaries = [ordered]@{
  "glazewm.exe"         = "glazewm.exe"
  "glazewm-watcher.exe" = "glazewm-watcher.exe"
  "glazewm-cli.exe"     = "cli\glazewm.exe"
}

# Trusts the certificate machine-wide and copies the staged binaries to the
# install directory. Runs elevated.
function Install-Elevated() {
  $certificate = Get-PfxCertificate -FilePath $CertPath

  foreach ($store in @("Root", "TrustedPublisher")) {
    $storePath = "Cert:\LocalMachine\$store\$($certificate.Thumbprint)"

    if (!(Test-Path $storePath)) {
      Import-Certificate -FilePath $CertPath `
        -CertStoreLocation "Cert:\LocalMachine\$store" | Out-Null
    }
  }

  foreach ($target in $Binaries.Values) {
    $destination = Join-Path $InstallDir $target
    New-Item -ItemType Directory -Force (Split-Path $destination) | Out-Null
    Copy-Item (Join-Path $StagingDir $target) $destination -Force
  }
}

# Returns the user's code signing certificate, creating it if missing or
# about to expire.
function Get-SigningCertificate() {
  $certificate = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
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

  return New-SelfSignedCertificate `
    -Type CodeSigningCert `
    -Subject $CertSubject `
    -KeyAlgorithm RSA `
    -KeyLength 3072 `
    -HashAlgorithm SHA256 `
    -KeyExportPolicy NonExportable `
    -NotAfter (Get-Date).AddYears(5) `
    -CertStoreLocation Cert:\CurrentUser\My
}

# Exits GlazeWM if it's running, and waits for it to clean up.
function Stop-GlazeWM() {
  if (!(Get-Process glazewm -ErrorAction SilentlyContinue)) {
    return
  }

  Write-Host "Exiting running GlazeWM."
  & (Join-Path $SourceDir "glazewm-cli.exe") command wm-exit | Out-Null

  # The watcher restores windows after the main process exits. Polled,
  # since `Wait-Process` can't open 64-bit processes from a 32-bit shell.
  $deadline = (Get-Date).AddSeconds(30)

  while (Get-Process glazewm, glazewm-watcher -ErrorAction SilentlyContinue) {
    if ((Get-Date) -gt $deadline) {
      throw "GlazeWM didn't exit. Exit it manually and run this script again."
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

foreach ($source in $Binaries.Keys) {
  if (!(Test-Path (Join-Path $SourceDir $source))) {
    throw "Missing '$source' in '$SourceDir'. Build the release binaries first."
  }
}

# UIAccess is only granted if requested in the manifest.
$manifestMatch = Select-String -Path (Join-Path $SourceDir "glazewm.exe") `
  -Pattern 'uiAccess="true"' -SimpleMatch -Quiet

if (!$manifestMatch) {
  throw "glazewm.exe wasn't built with the 'ui_access' feature."
}

$certificate = Get-SigningCertificate

$staging = Join-Path ([IO.Path]::GetTempPath()) "glazewm-install-$PID"
$certPath = Join-Path $staging "signing.cer"

try {
  foreach ($entry in $Binaries.GetEnumerator()) {
    $staged = Join-Path $staging $entry.Value
    New-Item -ItemType Directory -Force (Split-Path $staged) | Out-Null
    Copy-Item (Join-Path $SourceDir $entry.Key) $staged

    $signature = Set-AuthenticodeSignature -FilePath $staged `
      -Certificate $certificate -HashAlgorithm SHA256

    if (!$signature.SignerCertificate) {
      throw "Failed to sign '$($entry.Key)': $($signature.StatusMessage)"
    }
  }

  Export-Certificate -Cert $certificate -FilePath $certPath | Out-Null

  Stop-GlazeWM

  Write-Host "Installing to '$InstallDir' (requires administrator rights)."

  $shell = (Get-Process -Id $PID).Path
  $arguments = @(
    "-NoProfile", "-ExecutionPolicy", "Bypass",
    "-File", "`"$PSCommandPath`"",
    "-Elevated",
    "-StagingDir", "`"$staging`"",
    "-InstallDir", "`"$InstallDir`"",
    "-CertPath", "`"$certPath`""
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

$executable = Join-Path $InstallDir "glazewm.exe"
$signature = Get-AuthenticodeSignature $executable

if ($signature.Status -ne "Valid") {
  throw "Installed glazewm.exe has an invalid signature: $($signature.StatusMessage)"
}

Set-ItemProperty $RunKey -Name "GlazeWM" -Value "`"$executable`""

# Executables with UIAccess have to be launched through the shell (i.e.
# `ShellExecute`), which `Start-Process` does. GlazeWM is launched without
# administrator rights, since this script isn't elevated.
Start-Process $executable

Write-Host "Installed and started '$executable'."
