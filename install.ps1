# codekurve standalone installer for Windows (PowerShell).
#
# Downloads the single static release binary from GitHub Releases — no Node
# runtime, no build tools required.
#
#   irm https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.ps1 | iex
#
# Upgrade: re-run this same command (overwrites the binary in place). There is
# no `codekurve upgrade` subcommand — re-running the installer *is* the
# upgrade path.
#
# Environment:
#   CODEKURVE_VERSION  release tag to install (default: latest)
#   CODEKURVE_BIN_DIR  install location (default: %LOCALAPPDATA%\codekurve\bin)

$ErrorActionPreference = 'Stop'
$repo = 'luisantonio1493/codeKurve'
$binDir = if ($env:CODEKURVE_BIN_DIR) { $env:CODEKURVE_BIN_DIR } else { Join-Path $env:LOCALAPPDATA 'codekurve\bin' }
$dest = Join-Path $binDir 'codekurve.exe'

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

# 3. Download the raw binary directly to its final destination.
$url = "https://github.com/$repo/releases/download/$version/$asset"
Write-Host "Installing codekurve $version ($asset)..."
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
$tmp = Join-Path $binDir ("codekurve.tmp." + [guid]::NewGuid().ToString())
try {
  Invoke-WebRequest -Uri $url -OutFile $tmp
} catch {
  Remove-Item -Force -ErrorAction SilentlyContinue $tmp
  throw "codekurve: download failed: $url`n$_"
}
Move-Item -Force $tmp $dest

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
