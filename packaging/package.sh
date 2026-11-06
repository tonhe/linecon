#!/bin/sh
# Builds the download for one target in dist/. Usage: package.sh <target-name>
# Archives keep the executable bit, which a bare file loses inside an Actions zip.
set -eu
name="$1"
mkdir -p dist

# Signs with a Developer ID and notarizes. See docs/macos-signing.md.
sign_and_notarize() {
  app_path="$1"
  bare="$2"
  for needed in MACOS_CERT_P12 MACOS_CERT_PASSWORD APPLE_API_KEY_P8 APPLE_API_KEY_ID APPLE_API_ISSUER; do
    eval "value=\${$needed:-}"
    if [ -z "$value" ]; then
      echo "$needed is not set; refusing to ship a half-signed release" >&2
      exit 1
    fi
  done
  work=$(mktemp -d)
  keychain="$work/signing.keychain-db"
  keychain_password=$(uuidgen)
  trap 'security delete-keychain "$keychain" 2>/dev/null || true; rm -rf "$work"' EXIT
  printf '%s' "$MACOS_CERT_P12" | base64 --decode > "$work/cert.p12"
  printf '%s' "$APPLE_API_KEY_P8" | base64 --decode > "$work/key.p8"
  security create-keychain -p "$keychain_password" "$keychain"
  security set-keychain-settings -lut 3600 "$keychain"
  security unlock-keychain -p "$keychain_password" "$keychain"
  security import "$work/cert.p12" -k "$keychain" -P "$MACOS_CERT_PASSWORD" -T /usr/bin/codesign
  # A .p12 usually holds only the certificate, and without Apple's intermediates macOS
  # does not count it as valid.
  for intermediate in DeveloperIDG2CA DeveloperIDCA; do
    security import "packaging/macos/$intermediate.cer" -k "$keychain" >/dev/null
  done
  security set-key-partition-list -S apple-tool:,apple: -s -k "$keychain_password" "$keychain" >/dev/null
  security list-keychains -d user -s "$keychain" login.keychain-db
  identity=$(security find-identity -v -p codesigning "$keychain" | sed -n 's/.*"\(Developer ID Application:.*\)"/\1/p' | head -1)
  if [ -z "$identity" ]; then
    # Types only: these logs are public, and a certificate's name can hold an email address.
    echo "no valid Developer ID Application certificate in MACOS_CERT_P12. It holds these types:" >&2
    security find-identity -p codesigning "$keychain" | sed -n 's/.*"\([^:"]*\):.*/  \1/p' | sort -u >&2
    exit 1
  fi
  sign() { codesign --force --options runtime --timestamp --sign "$identity" "$@"; }
  sign --identifier io.github.tonhe.linecon "$bare"
  cp "$bare" "$app_path/Contents/MacOS/linecon"
  sign "$app_path"
  # Checks the verdict too: a rejected submission can still exit cleanly.
  notarize() {
    verdict="$work/notarytool.txt"
    xcrun notarytool submit "$1" --key "$work/key.p8" --key-id "$APPLE_API_KEY_ID" \
      --issuer "$APPLE_API_ISSUER" --wait --timeout 30m | tee "$verdict"
    grep -q "status: Accepted" "$verdict"
  }
  ditto -c -k --keepParent "$app_path" "$work/app.zip"
  notarize "$work/app.zip"
  # Fetching and checking the ticket are network calls that sometimes drop, and an accepted build
  # should not fail a release for that.
  attempt=1
  until xcrun stapler staple "$app_path" && xcrun stapler validate "$app_path"; do
    if [ "$attempt" -ge 5 ]; then
      echo "could not staple after $attempt attempts" >&2
      exit 1
    fi
    attempt=$((attempt + 1))
    sleep 30
  done
  spctl --assess --type execute --verbose=2 "$app_path"
  # A bare executable cannot hold a ticket, but Apple's servers know it once submitted.
  ditto -c -k "$bare" "$work/bare.zip"
  notarize "$work/bare.zip"
}
case "$name" in
  windows-*)
    cp target/release/linecon.exe "dist/linecon-$name.exe"
    ;;
  macos-*)
    # CFBundleVersion only allows digits and dots, so no prerelease suffix.
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
    tar_dir=$(mktemp -d)
    cp target/release/linecon "$tar_dir/linecon"
    if [ "${GITHUB_REF_TYPE:-}" = tag ] && [ -z "${MACOS_CERT_P12:-}${APPLE_API_KEY_P8:-}" ]; then
      echo "a release must be signed, and the signing secrets are missing" >&2
      exit 1
    fi
    if [ -n "${MACOS_CERT_P12:-}${APPLE_API_KEY_P8:-}" ]; then
      sign_and_notarize "$app" "$tar_dir/linecon"
    else
      # Without credentials: Gatekeeper still objects to a download.
      codesign --force --sign - "$tar_dir/linecon"
      cp "$tar_dir/linecon" "$app/Contents/MacOS/linecon"
      codesign --force --deep --sign - "$app"
    fi
    codesign --verify --deep --strict "$app"
    "$app/Contents/MacOS/linecon" --version
    # What ships, signed and stapled, has to open its window too.
    packaging/smoke.sh "$app/Contents/MacOS/linecon"
    (cd dist && ditto -c -k --keepParent linecon0.app "linecon0-$name.app.zip")
    rm -rf "$app"
    # Named so it is not mistaken for the app: this one is for terminal use.
    tar -czf "dist/linecon-$name-cli.tar.gz" -C "$tar_dir" linecon
    ;;
  *)
    tar -czf "dist/linecon-$name.tar.gz" -C target/release linecon
    ;;
esac
ls -l dist
