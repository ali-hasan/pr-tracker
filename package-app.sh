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

/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister -f "$APP"

echo "Installed to $APP"
