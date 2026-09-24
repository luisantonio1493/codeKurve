#!/bin/sh
#
# codekurve standalone installer (macOS/Linux).
#
# Downloads the single static release binary from GitHub Releases — no Node
# runtime, no build tools, nothing else to unpack.
#
#   curl -fsSL https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.sh | sh
#
# Upgrade:   re-run this same command (overwrites the binary in place).
# Uninstall: curl -fsSL .../install.sh | sh -s -- --uninstall
#
# Environment:
#   CODEKURVE_VERSION  release tag to install (default: latest)
#   CODEKURVE_BIN_DIR  install location (default: ~/.local/bin)
set -eu

REPO="luisantonio1493/codeKurve"
BIN_DIR="${CODEKURVE_BIN_DIR:-$HOME/.local/bin}"
DEST="$BIN_DIR/codekurve"

if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$DEST"
  echo "codekurve uninstalled (removed $DEST)."
  exit 0
fi

# 1. Detect platform -> release asset name.
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin) os="macos" ;;
  Linux)  os="linux" ;;
  *) echo "codekurve: unsupported OS '$os'." >&2; exit 1 ;;
esac
case "$arch" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64|amd64)  arch="x64" ;;
  *) echo "codekurve: unsupported architecture '$arch'." >&2; exit 1 ;;
esac
if [ "$os" = "linux" ] && [ "$arch" = "aarch64" ]; then
  echo "codekurve: no linux-aarch64 build is published yet." >&2
  exit 1
fi
asset="codekurve-${os}-${arch}"

# 2. Resolve the version (latest release unless pinned).
#
# Resolve "latest" from the releases/latest *web* redirect, not the GitHub
# API: the unauthenticated API is rate-limited to 60 requests/hour per IP and
# returns 403 once exhausted. The redirect has no such limit. Fall back to
# the API if the redirect can't be read.
version="${CODEKURVE_VERSION:-}"
if [ -z "$version" ]; then
  version="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" \
    | sed -n 's#.*/releases/tag/##p')"
fi
if [ -z "$version" ]; then
  version="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
fi
[ -n "$version" ] || { echo "codekurve: could not resolve latest version; set CODEKURVE_VERSION (e.g. CODEKURVE_VERSION=v0.1.0)." >&2; exit 1; }
# Release tags are vX.Y.Z; accept a bare X.Y.Z in CODEKURVE_VERSION too.
case "$version" in v*) ;; *) version="v$version" ;; esac

# SHA-256 of a file, lowercase hex. Linux ships `sha256sum`, macOS ships
# `shasum`; `openssl` is the last resort. No tool -> refuse to install rather
# than skip the check.
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 -r "$1" | awk '{print $1}'
  else
    echo "codekurve: need sha256sum, shasum or openssl to verify the download." >&2
    return 1
  fi
}

# 3. Download the raw binary next to its final destination, verify it against
# the release's SHA256SUMS, and only then move it into place. A mismatch or a
# missing checksum aborts: nothing is installed. This catches a corrupted or
# truncated download or a swapped CDN response. It does NOT protect against
# someone who can edit the GitHub release itself (they could replace
# SHA256SUMS too); for provenance see docs/SECURITY_MODEL.md.
base="https://github.com/$REPO/releases/download/$version"
url="$base/$asset"
echo "Installing codekurve $version ($asset)..."
mkdir -p "$BIN_DIR"
tmp="$(mktemp "$BIN_DIR/.codekurve.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
curl -fsSL "$url" -o "$tmp" || { echo "codekurve: download failed: $url" >&2; exit 1; }

sums="$(curl -fsSL "$base/SHA256SUMS")" \
  || { echo "codekurve: could not download $base/SHA256SUMS; refusing to install unverified binary." >&2; exit 1; }
# `sha256sum` lines are "<hash>  <name>" (or "<hash> *<name>" in binary mode).
expected="$(printf '%s\n' "$sums" | awk -v a="$asset" '$2 == a || $2 == "*" a { print tolower($1); exit }')"
[ -n "$expected" ] \
  || { echo "codekurve: SHA256SUMS for $version has no entry for $asset; refusing to install." >&2; exit 1; }
actual="$(sha256_of "$tmp")" || exit 1
if [ "$actual" != "$expected" ]; then
  echo "codekurve: checksum mismatch for $asset ($version); refusing to install." >&2
  echo "  expected $expected" >&2
  echo "  actual   $actual" >&2
  exit 1
fi
echo "Verified   SHA-256 $actual"

chmod +x "$tmp"
mv "$tmp" "$DEST"
trap - EXIT

echo "Installed  $DEST"

# 4. PATH sanity. Two ways this install can fail to be the codekurve that runs:
#   1. $BIN_DIR isn't on PATH at all.
#   2. A *different* codekurve sits earlier on PATH and shadows ours.
# Walk PATH once: note whether $BIN_DIR is present and which codekurve wins.
on_path=0
winner=""
oldifs="$IFS"; IFS=:
for dir in $PATH; do
  [ -n "$dir" ] || continue
  if [ "$dir" = "$BIN_DIR" ]; then on_path=1; fi
  if [ -z "$winner" ] && [ -x "$dir/codekurve" ] && [ ! -d "$dir/codekurve" ]; then
    winner="$dir/codekurve"
  fi
done
IFS="$oldifs"

if [ "$on_path" -eq 0 ]; then
  echo ""
  echo "$BIN_DIR is not on your PATH. Add it:"
  echo "  export PATH=\"$BIN_DIR:\$PATH\""
elif [ -n "$winner" ] && [ "$winner" != "$DEST" ]; then
  echo ""
  echo "Warning: another codekurve is earlier on your PATH and will run instead:"
  echo "  $winner"
  echo "  (this install: $DEST)"
  echo "Remove the other copy or put $BIN_DIR first on PATH."
fi

echo ""
echo "Done. Run: codekurve --help"
