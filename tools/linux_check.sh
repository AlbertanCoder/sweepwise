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
say "tests finished (exit $TEST_EXIT)"

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

# 5. Results: the tests, the built program's own self-test and the packages (same writer as the GitHub checks).
python3 "$SRC/tools/linux_result.py" "$OUT/test.log" "$OUT" "$WORK/src-tauri/target/release/sweepwise" "$TEST_EXIT" "$BUILD_EXIT"
