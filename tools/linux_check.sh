#!/usr/bin/env bash
# Builds, tests and packages Sweepwise on Linux, then writes tools/linux/RESULT.json.
#   Run inside Linux (or Ubuntu on WSL):  bash tools/linux_check.sh
# It works on a copy in ~/sweepwise-linux, so the Windows build folders are never touched.
# Needs: Rust, Node.js + npm, and the web-view libraries:
#   sudo apt-get install -y libwebkit2gtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev libxdo-dev nodejs npm
set -u
SRC="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${SWEEPWISE_LINUX_WORK:-$HOME/sweepwise-linux}"
OUT="$SRC/tools/linux"
export PATH="$HOME/.cargo/bin:$PATH"
mkdir -p "$WORK" "$OUT"
say() { echo "[linux check] $*"; }

# 1. Fresh copy of the code (build caches in the copy are kept, so later runs are quick).
say "copying the project to $WORK"
rm -rf "$WORK/src" "$WORK/src-tauri/src" "$WORK/src-tauri/icons" "$WORK/src-tauri/capabilities"
(cd "$SRC" && tar --exclude=./node_modules --exclude=./src-tauri/target --exclude=./src-tauri/gen \
    --exclude=./tools/screens --exclude=./tools/linux -cf - .) | (cd "$WORK" && tar -xf -)
cd "$WORK" || exit 1

# 2. The project's own Tauri command-line tool, for Linux.
if [ ! -x node_modules/.bin/tauri ]; then
    say "installing the project's build tool (npm ci)"
    npm ci --no-audit --no-fund > "$OUT/npm.log" 2>&1 || { say "npm ci failed: see tools/linux/npm.log"; exit 1; }
fi

# 3. Every test, as built for Linux.
say "running the tests"
(cd src-tauri && CARGO_BUILD_JOBS=2 cargo test -j 2 -- --test-threads=1) > "$OUT/test.log" 2>&1
TEST_EXIT=$?
read -r PASSED FAILED < <(awk '/^test result:/ { for (i = 2; i <= NF; i++) { if ($i == "passed;") p += $(i-1); if ($i == "failed;") f += $(i-1) } } END { print p+0, f+0 }' "$OUT/test.log")
if [ "$TEST_EXIT" -ne 0 ] && [ "$FAILED" -eq 0 ]; then FAILED=1; fi
say "tests: $PASSED passed, $FAILED failed"

# 4. The two Linux packages: .deb (Ubuntu, Mint, Debian) and AppImage (most others).
say "building the .deb and AppImage"
CARGO_BUILD_JOBS=2 npx tauri build --bundles deb,appimage > "$OUT/build.log" 2>&1
BUILD_EXIT=$?
DEB=$(ls -t src-tauri/target/release/bundle/deb/*.deb 2>/dev/null | head -1)
APPIMAGE=$(ls -t src-tauri/target/release/bundle/appimage/*.AppImage 2>/dev/null | head -1)
rm -f "$OUT"/*.deb "$OUT"/*.AppImage
[ -n "$DEB" ] && cp "$DEB" "$OUT/"
[ -n "$APPIMAGE" ] && cp "$APPIMAGE" "$OUT/"
say "packages: ${DEB:-none} ${APPIMAGE:-none} (build exit $BUILD_EXIT)"
# Fedora: an .rpm as well (optional; the AppImage works there too).
RPM=""
rm -f "$OUT"/*.rpm
if [ "$BUILD_EXIT" -eq 0 ]; then
    CARGO_BUILD_JOBS=2 npx tauri bundle --bundles rpm >> "$OUT/build.log" 2>&1 && RPM=$(ls -t src-tauri/target/release/bundle/rpm/*.rpm 2>/dev/null | head -1)
    [ -n "$RPM" ] && cp "$RPM" "$OUT/"
fi

# 5. The built program's own self-test, with a throwaway data folder.
DATA=$(mktemp -d)
SELF=$(SWEEPWISE_DATA="$DATA" ./src-tauri/target/release/sweepwise --self-test 2>/dev/null | tail -1)
rm -rf "$DATA"

# 6. Results.
python3 - "$OUT/RESULT.json" "$PASSED" "$FAILED" "$TEST_EXIT" "$BUILD_EXIT" "${DEB:-}" "${APPIMAGE:-}" "$SELF" "$RPM" <<'PY'
import datetime, json, os, sys
out, passed, failed, test_exit, build_exit, deb, appimage, self_line, rpm = sys.argv[1:10]
try:
    st = json.loads(self_line)
except Exception:
    st = {}
distro = ''
try:
    for line in open('/etc/os-release', encoding='utf-8'):
        if line.startswith('PRETTY_NAME='):
            distro = line.split('=', 1)[1].strip().strip('"')
except OSError:
    pass
r = {
    'platform': st.get('platform', 'unknown'),
    'self_test_ok': bool(st.get('ok')),
    'version': st.get('version'),
    'features': st.get('features', []),
    'tests_passed': int(passed),
    'tests_failed': int(failed),
    'test_exit': int(test_exit),
    'build_exit': int(build_exit),
    'deb': os.path.basename(deb) if deb else None,
    'appimage': os.path.basename(appimage) if appimage else None,
    'rpm': os.path.basename(rpm) if rpm else None,
    'distro': distro,
    'when': datetime.datetime.now().isoformat(timespec='seconds'),
}
with open(out, 'w', encoding='utf-8') as f:
    json.dump(r, f, indent=1)
print(json.dumps({k: v for k, v in r.items() if k != 'features'}))
PY
