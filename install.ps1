# codekurve standalone installer for Windows (PowerShell).
#
# Downloads the single static release binary from GitHub Releases — no Node
# runtime, no build tools required.
#
#   irm https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.ps1 | iex
#
# Upgrade: re-run this same command (overwrites the binary in place), or run
# `codekurve update`, which spawns exactly this script.
#
# Uninstall (irm | iex cannot pass arguments, so use the scriptblock form):
#   &([scriptblock]::Create((irm https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.ps1))) -Uninstall
#
# Environment:
#   CODEKURVE_VERSION  release tag to install (default: latest)
#   CODEKURVE_BIN_DIR  install location (default: %LOCALAPPDATA%\codekurve\bin)

param([switch]$Uninstall)

$ErrorActionPreference = 'Stop'
$repo = 'luisantonio1493/codeKurve'
$binDir = if ($env:CODEKURVE_BIN_DIR) { $env:CODEKURVE_BIN_DIR } else { Join-Path $env:LOCALAPPDATA 'codekurve\bin' }
$dest = Join-Path $binDir 'codekurve.exe'

# Mirrors `install.sh --uninstall`: removes the binary only. PATH is left
# alone — the entry is harmless once empty, and stripping it could clobber a
# concurrent edit to the user's PATH.
if ($Uninstall) {
  Remove-Item -Force -ErrorAction SilentlyContinue $dest
  Write-Host "codekurve uninstalled (removed $dest)."
  exit 0
}

# 1. Detect architecture. Only an x64 Windows build is published today; on
# Arm64 there is no native binary yet, so fail clearly instead of silently
# handing back a binary that won't run (x64-under-emulation is a footgun for
# a dev CLI — better the user knows and asks for an arm64 build if they need it).
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($arch -eq 'Arm64') {
  throw "codekurve: no Windows arm64 build is published yet (only codekurve-windows-x64.exe). Run under x64 Windows/WSL, or open an issue if you need native arm64."
}
$asset = 'codekurve-windows-x64.exe'

# 2. Resolve the version (latest release unless pinned).
$version = $env:CODEKURVE_VERSION
if (-not $version) {
  $version = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
}
if (-not $version) { throw "codekurve: could not resolve latest version; set CODEKURVE_VERSION." }
if ($version -notmatch '^v') { $version = "v$version" }

# 3. Download the binary and verify it before touching the installed copy.
#
# Integrity: the release publishes a SHA256SUMS manifest (`<sha256>  <file>`)
# next to the binaries. The download is checked against its entry and smoke
# tested with --version before anything at $dest is replaced.
$releaseBase = "https://github.com/$repo/releases/download/$version"
$url = "$releaseBase/$asset"
Write-Host "Installing codekurve $version ($asset)..."
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
# The .exe suffix lets the smoke test execute the temp file directly.
$tmp = Join-Path $binDir ("codekurve.tmp." + [guid]::NewGuid().ToString() + ".exe")
try {
  try {
    Invoke-WebRequest -Uri $url -OutFile $tmp
  } catch {
    throw "codekurve: download failed: $url`n$_"
  }

  try {
    $manifestResponse = Invoke-WebRequest -Uri "$releaseBase/SHA256SUMS" -UseBasicParsing
  } catch {
    throw "codekurve: could not download SHA256SUMS from $releaseBase; refusing to install an unverified binary.`n$_"
  }
  # Release assets are served as octet-stream, so Content may arrive as bytes.
  $manifest = $manifestResponse.Content
  if ($manifest -is [byte[]]) { $manifest = [System.Text.Encoding]::UTF8.GetString($manifest) }
  $expected = @()
  foreach ($line in ($manifest -split "`r?`n")) {
    if ($line -match '^\s*([0-9A-Fa-f]{64})\s+\*?(.+?)\s*$' -and $Matches[2] -eq $asset) {
      $expected += $Matches[1]
    }
  }
  if ($expected.Count -ne 1) {
    throw "codekurve: SHA256SUMS has $($expected.Count) entries for $asset (expected exactly 1); refusing to install."
  }
  $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $tmp).Hash
  if ($actual -ine $expected[0]) {
    throw "codekurve: checksum mismatch for $asset (expected $($expected[0]), got $actual); refusing to install."
  }

  # Smoke test the verified file before it replaces anything.
  & $tmp --version *> $null
  if ($LASTEXITCODE -ne 0) {
    throw "codekurve: downloaded $asset failed its '--version' smoke test; existing install left unchanged."
  }

  # Keep the previous binary for manual rollback, then replace it.
  if (Test-Path -LiteralPath $dest) {
    try {
      Copy-Item -Force -LiteralPath $dest -Destination "$dest.prev"
    } catch {
      throw "codekurve: could not back up $dest to $dest.prev (file in use?). Close any running codekurve.exe (or terminal using it) and re-run this installer.`n$_"
    }
  }
  try {
    Move-Item -Force $tmp $dest
  } catch {
    throw "codekurve: could not replace $dest (file in use). Close any running codekurve.exe (or terminal using it) and re-run this installer.`n$_"
  }
} catch {
  Remove-Item -Force -ErrorAction SilentlyContinue $tmp
  throw
}

Write-Host "Installed  $dest"

# 4. Put the install dir on the user's PATH if it isn't already there.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $binDir) {
  [Environment]::SetEnvironmentVariable('Path', "$binDir;$userPath", 'User')
  Write-Host "Added $binDir to your PATH (restart your terminal to pick it up)."
}

# 5. Warn if a different codekurve earlier on PATH will shadow this install.
# Check both the persisted PATH a fresh shell sees (Machine + User) and this
# session's PATH (catches dirs a shell profile injects).
function Find-FirstCodekurve([string]$pathStr) {
  foreach ($dir in ($pathStr -split ';')) {
    if (-not $dir) { continue }
    $cand = Join-Path $dir 'codekurve.exe'
    if (Test-Path -LiteralPath $cand) { return $cand }
  }
  return $null
}
$machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
$freshPath = ((@($machinePath, [Environment]::GetEnvironmentVariable('Path', 'User')) | Where-Object { $_ }) -join ';')
$shadow = $null
foreach ($winner in @((Find-FirstCodekurve $env:Path), (Find-FirstCodekurve $freshPath))) {
  if ($winner -and ($winner -ne $dest)) { $shadow = $winner; break }
}
if ($shadow) {
  Write-Warning "Another codekurve is earlier on your PATH and will run instead of this install:"
  Write-Warning "  $shadow"
  Write-Warning "  (this install: $dest)"
  Write-Warning "Remove the other copy or put '$binDir' first on your PATH."
}

Write-Host "Run: codekurve --help"
