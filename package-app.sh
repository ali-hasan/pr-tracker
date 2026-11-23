#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

cargo build --release

APP="$HOME/Applications/PR Tracker.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp Info.plist "$APP/Contents/Info.plist"
cp target/release/pr-tracker "$APP/Contents/MacOS/pr-tracker"
chmod +x "$APP/Contents/MacOS/pr-tracker"

# macOS remembers folder-access answers per signing identity. The default ad-hoc signature
# changes on every build, so each rebuild looked like a new app and asked again. Sign with
# your Apple Development certificate when you have one (or set CODESIGN_IDENTITY), so the
# answers stick across rebuilds.
IDENTITY="${CODESIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null \
  | sed -n 's/.*"\(Apple Development: .*\)"/\1/p' | head -1)}"
codesign --force --sign "${IDENTITY:--}" --identifier local.pr-tracker "$APP"

/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister -f "$APP"

echo "Installed to $APP"
