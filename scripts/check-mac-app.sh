#!/usr/bin/env bash
# Checks the built Mac app, once in target/release/bundle/macos and once inside
# the mounted .dmg (which is what people download). Run from the repository
# root by each Installer job of .github/workflows/macos.yml, after
# `cargo tauri build --config src-tauri/tauri.bundle.conf.json`:
#
#   bash scripts/check-mac-app.sh <arm64|x86_64>
#
# The argument is the architecture `lipo -archs` must give for both programs.
# Exits 0 when every check holds; otherwise it prints what is wrong, on stderr,
# and exits 1 (2 for a wrong call). Tests: src-tauri/tests/check_mac_app.rs.
#
# For each app it fails unless:
#   - ffmpeg and ffprobe sit in Contents/MacOS and can run;
#   - the app's signature holds (`codesign --verify --deep --strict`), is an
#     ad-hoc one made by the bundler, and has sealed resources (version 2);
#   - the seal lists both programs, or macOS can refuse them after "Open
#     Anyway";
#   - each program has exactly the architecture asked for;
#   - each program needs no newer macOS than 12.0 (`minos` of `vtool
#     -show-build`), the app's own minimumSystemVersion. A higher minos means
#     minimumSystemVersion has to rise, or another ffmpeg build has to be
#     pinned.
# The listing of each Contents/MacOS, the codesign details and each program's
# build info are printed for the log; they are not checks.

set -euo pipefail

# The newest macOS the programs may need.
newest_major=12

fail() {
  echo "check-mac-app: $*" >&2
  exit 1
}

if [ "$#" -ne 1 ] || [ -z "$1" ]; then
  echo "usage: check-mac-app.sh <arm64|x86_64>" >&2
  exit 2
fi
arch=$1

check_app() {
  local app=$1 details tool program archs build minos
  ls -l "$app/Contents/MacOS"
  for tool in ffmpeg ffprobe; do
    program="$app/Contents/MacOS/$tool"
    [ -x "$program" ] || fail "$program is missing or not executable"
  done
  codesign --verify --deep --strict --verbose=2 "$app" ||
    fail "the signature of $app does not verify"
  details=$(codesign -dv "$app" 2>&1)
  echo "$details"
  grep -qx 'Signature=adhoc' <<<"$details" ||
    fail "$app: the signature is not Signature=adhoc"
  grep -q '^Sealed Resources version=2' <<<"$details" ||
    fail "$app: no Sealed Resources version=2"
  for tool in ffmpeg ffprobe; do
    program="$app/Contents/MacOS/$tool"
    if ! plutil -extract "files2.MacOS/$tool" xml1 -o - \
      "$app/Contents/_CodeSignature/CodeResources" >/dev/null; then
      echo "The seal of $app does not list MacOS/$tool. Its entries:" >&2
      plutil -p "$app/Contents/_CodeSignature/CodeResources" >&2 || true
      fail "$app: the seal does not list MacOS/$tool"
    fi
    archs=$(lipo -archs "$program")
    echo "$tool: $archs"
    [ "$archs" = "$arch" ] || fail "$program has the architecture '$archs', not '$arch'"
    build=$(vtool -show-build "$program")
    echo "$build"
    minos=$(awk '$1 == "minos" { print $2; exit }' <<<"$build")
    [ -n "$minos" ] || fail "$program: no minos line in its build info"
    # 12, 12.0 and 12.0.0 are fine; 12.0.1 and 12.1 are not.
    awk -v v="$minos" -v max="$newest_major" 'BEGIN {
      split(v, p, ".")
      exit !(p[1] + 0 < max || (p[1] + 0 == max && p[2] + 0 == 0 && p[3] + 0 == 0))
    }' || fail "$program: minos $minos is newer than $newest_major.0"
  done
}

check_app "target/release/bundle/macos/Video optimizer.app"

set -- target/release/bundle/dmg/*.dmg
[ -e "$1" ] || fail "no .dmg in target/release/bundle/dmg"
dmg=$1
mount=$(mktemp -d)
hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$dmg"
trap 'hdiutil detach "$mount"' EXIT
check_app "$mount/Video optimizer.app"
