#!/bin/sh
# Builds the download for one target in dist/. Usage: package.sh <target-name>
# Archives keep the executable bit, which a bare file loses inside an Actions zip.
set -eu
name="$1"
mkdir -p dist
case "$name" in
  windows-*)
    cp target/release/linecon.exe "dist/linecon-$name.exe"
    ;;
  macos-*)
    version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
    app=dist/linecon0.app
    mkdir -p "$app/Contents/MacOS"
    cp target/release/linecon "$app/Contents/MacOS/linecon"
    sed "s/VERSION/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
    mkdir -p "$app/Contents/Resources" dist/linecon.iconset
    for size in 16 32 128 256 512; do
      double=$((size * 2))
      sips -z "$size" "$size" packaging/icon/linecon.png --out "dist/linecon.iconset/icon_${size}x${size}.png" >/dev/null
      sips -z "$double" "$double" packaging/icon/linecon.png --out "dist/linecon.iconset/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns dist/linecon.iconset -o "$app/Contents/Resources/linecon.icns"
    rm -rf dist/linecon.iconset
    test -s "$app/Contents/Resources/linecon.icns"
    # Ad-hoc only: not notarized, so Gatekeeper still objects to a download.
    codesign --force --deep --sign - "$app"
    codesign --verify --deep --strict "$app"
    "$app/Contents/MacOS/linecon" --version
    (cd dist && ditto -c -k --keepParent linecon0.app "linecon0-$name.app.zip")
    rm -rf "$app"
    tar -czf "dist/linecon-$name.tar.gz" -C target/release linecon
    ;;
  *)
    tar -czf "dist/linecon-$name.tar.gz" -C target/release linecon
    ;;
esac
ls -l dist
