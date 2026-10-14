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

# 3. Download the binary and verify it before touching the installed copy.
#
# Integrity: the release publishes a SHA256SUMS manifest (`<sha256>  <file>`)
# next to the binaries. The binary is checked against its entry before it is
# made executable or moved into place. A missing manifest, missing entry, or
# mismatch aborts with the existing install left untouched.
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}
if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
  echo "codekurve: neither sha256sum nor shasum is available; cannot verify the download." >&2
  exit 1
fi

release_base="https://github.com/$REPO/releases/download/$version"
url="$release_base/$asset"
echo "Installing codekurve $version ($asset)..."
mkdir -p "$BIN_DIR"
tmp="$(mktemp "$BIN_DIR/.codekurve.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
curl -fsSL "$url" -o "$tmp" || { echo "codekurve: download failed: $url" >&2; exit 1; }

manifest="$(curl -fsSL "$release_base/SHA256SUMS")" \
  || { echo "codekurve: could not download SHA256SUMS from $release_base; refusing to install an unverified binary." >&2; exit 1; }
expected="$(printf '%s\n' "$manifest" \
  | awk -v f="$asset" '{ n = $2; sub(/^\*/, "", n); if (n == f) print $1 }')"
expected_count="$(printf '%s\n' "$expected" | grep -c . || true)"
if [ "$expected_count" -ne 1 ]; then
  echo "codekurve: SHA256SUMS has $expected_count entries for $asset (expected exactly 1); refusing to install." >&2
  exit 1
fi
expected="$(printf '%s' "$expected" | tr 'A-F' 'a-f')"
actual="$(sha256_of "$tmp" | tr 'A-F' 'a-f')"
if [ "$actual" != "$expected" ]; then
  echo "codekurve: checksum mismatch for $asset (expected $expected, got $actual); refusing to install." >&2
  exit 1
fi

# Smoke test the verified file before it replaces anything.
chmod +x "$tmp"
if ! "$tmp" --version >/dev/null 2>&1; then
  echo "codekurve: downloaded $asset failed its '--version' smoke test; existing install left unchanged." >&2
  exit 1
fi

# Keep the previous binary for manual rollback, then replace it.
if [ -e "$DEST" ]; then
  cp -p "$DEST" "$DEST.prev"
fi
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
