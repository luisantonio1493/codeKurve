#!/bin/sh
#
# Tests install.sh checksum verification and rollback behaviour.
#
# Runs the real install.sh against a fake release served by a stub `curl`,
# with a stub `uname` pinned to Linux/x86_64. Everything happens inside a
# temp directory; no network access and no real binary is touched.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INSTALLER="$ROOT/install.sh"
ASSET="codekurve-linux-x64"
VERSION="v9.9.9"

work="$(mktemp -d "${TMPDIR:-/tmp}/codekurve-install-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT

stubs="$work/stubs"
release="$work/release"
bin_dir="$work/bin"
mkdir -p "$stubs" "$release" "$bin_dir"

cat > "$stubs/uname" <<'EOF'
#!/bin/sh
case "$1" in
  -s) echo Linux ;;
  -m) echo x86_64 ;;
esac
EOF

# Serves files from $FAKE_RELEASE by URL basename; 404s when absent.
cat > "$stubs/curl" <<'EOF'
#!/bin/sh
out=""; url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
src="$FAKE_RELEASE/${url##*/}"
if [ ! -f "$src" ]; then echo "curl: 404 $url" >&2; exit 22; fi
if [ -n "$out" ]; then cp "$src" "$out"; else cat "$src"; fi
EOF
chmod +x "$stubs/uname" "$stubs/curl"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# Build a release: a working binary plus a manifest line for it.
make_release() {
  rm -f "$release"/*
  printf '#!/bin/sh\n[ "$1" = "--version" ] && echo "codekurve 9.9.9"\nexit 0\n' > "$release/$ASSET"
  chmod +x "$release/$ASSET"
  printf '%s  %s\n' "$(sha256_of "$release/$ASSET")" "$ASSET" > "$release/SHA256SUMS"
}

run_installer() {
  FAKE_RELEASE="$release" PATH="$stubs:$PATH" \
    CODEKURVE_VERSION="$VERSION" CODEKURVE_BIN_DIR="$bin_dir" \
    sh "$INSTALLER" >"$work/out" 2>"$work/err"
}

fail_count=0
pass() { echo "PASS  $1"; }
fail() { echo "FAIL  $1"; sed 's/^/      /' "$work/err" | tail -n 5; fail_count=$((fail_count + 1)); }

# expect_success NAME: installer must exit 0.
expect_success() {
  if run_installer; then pass "$1"; else fail "$1 (expected success)"; fi
}
# expect_refusal NAME: installer must exit non-zero and keep the old binary.
expect_refusal() {
  if run_installer; then
    fail "$1 (expected refusal, got success)"
  elif [ "$(cat "$bin_dir/codekurve")" != "$previous" ]; then
    fail "$1 (installed binary was modified)"
  elif [ -n "$(ls -A "$bin_dir" | grep '^\.codekurve\.' || true)" ]; then
    fail "$1 (temp file left behind)"
  else
    pass "$1"
  fi
}

# Existing install that the failure cases must leave untouched.
previous='#!/bin/sh
echo "previous codekurve"'
printf '%s\n' "$previous" > "$bin_dir/codekurve"
chmod +x "$bin_dir/codekurve"

# 1. Matching hash: installs, keeps the previous binary as .prev.
make_release
expect_success "matching checksum installs"
if [ "$(cat "$bin_dir/codekurve.prev")" = "$previous" ] && "$bin_dir/codekurve" --version | grep -q 9.9.9; then
  pass "previous binary kept as .prev and new binary runs"
else
  fail "previous binary kept as .prev and new binary runs"
fi
previous="$(cat "$bin_dir/codekurve")"

# 2. Uppercase hash in the manifest still matches (case-insensitive compare).
make_release
upper="$(sha256_of "$release/$ASSET" | tr 'a-f' 'A-F')"
printf '%s  %s\n' "$upper" "$ASSET" > "$release/SHA256SUMS"
expect_success "uppercase manifest hash matches"
previous="$(cat "$bin_dir/codekurve")"

# 3. Flipped byte in the binary: mismatch, old binary kept.
make_release
sed 's/9\.9\.9/9.9.8/' "$release/$ASSET" > "$work/tampered" && mv "$work/tampered" "$release/$ASSET"
chmod +x "$release/$ASSET"
expect_refusal "flipped byte is refused"

# 4. Manifest without an entry for the asset: refused.
make_release
printf '%s  %s\n' "$(sha256_of "$release/$ASSET")" "codekurve-other-asset" > "$release/SHA256SUMS"
expect_refusal "missing manifest entry is refused"

# 5. No manifest at all: refused.
make_release
rm -f "$release/SHA256SUMS"
expect_refusal "missing SHA256SUMS is refused"

# 6. Matching hash but failing --version smoke test: refused before replace.
make_release
printf '#!/bin/sh\nexit 1\n' > "$release/$ASSET"
printf '%s  %s\n' "$(sha256_of "$release/$ASSET")" "$ASSET" > "$release/SHA256SUMS"
expect_refusal "failing --version smoke test is refused"

if [ "$fail_count" -ne 0 ]; then
  echo "$fail_count check(s) failed"
  exit 1
fi
echo "all install checksum checks passed"
