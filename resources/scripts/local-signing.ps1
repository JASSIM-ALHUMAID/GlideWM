# Shared by `install-local.ps1` and `build-msi.ps1`. Dot-source this file;
# it doesn't do anything on its own.
#
# Windows only grants UIAccess to executables that are signed by a trusted
# certificate and installed in a secure location (e.g. Program Files). These
# helpers build the UIAccess binaries and sign them with a self-signed
# certificate that's trusted machine-wide.

$CertSubject = "CN=GlideWM Local Code Signing"

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

    & $cargoExe build --release -p wm --features ui_access
    if ($LASTEXITCODE -ne 0) {
      throw "Failed to build the WM binary. The build needs a nightly Rust toolchain (see https://rustup.rs)."
    }

    & $cargoExe build --release -p wm-watcher -p wm-cli
    if ($LASTEXITCODE -ne 0) {
      throw "Failed to build the watcher and CLI binaries."
    }
  }
  finally {
    $env:RUSTC = $previousRustc
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

# Signs the file with the certificate. Runs elevated.
function Sign-File([string]$Path, $Certificate) {
  $signature = Set-AuthenticodeSignature -FilePath $Path `
    -Certificate $Certificate -HashAlgorithm SHA256

  if (!$signature.SignerCertificate) {
    throw "Failed to sign '$Path': $($signature.StatusMessage)"
  }
}

# Trusts the certificate machine-wide, and removes stale ones. The public
# certificate is exported to `$WorkDir` for the import. Runs elevated.
function Install-SigningTrust($Certificate, [string]$WorkDir) {
  $certPath = Join-Path $WorkDir "signing.cer"
  Export-Certificate -Cert $Certificate -FilePath $certPath | Out-Null

  foreach ($store in @("Root", "TrustedPublisher")) {
    $storePath = "Cert:\LocalMachine\$store"

    if (!(Test-Path "$storePath\$($Certificate.Thumbprint)")) {
      Import-Certificate -FilePath $certPath `
        -CertStoreLocation $storePath | Out-Null
    }

    Remove-StaleCertificates $storePath $Certificate.Thumbprint
  }

  Remove-StaleCertificates "Cert:\LocalMachine\My" $Certificate.Thumbprint
}
