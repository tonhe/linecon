#!/bin/bash
# Proves the real window opens on this runner, and photographs it where possible.
set -u
# Usage: smoke.sh [binary], the release build by default.
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) binary=${1:-target/release/linecon.exe} ;;
  *) binary=${1:-target/release/linecon} ;;
esac
rm -f smoke.txt
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    product=$(powershell -NoProfile -Command "(Get-Item '$binary').VersionInfo.ProductName" | tr -d '\r')
    echo "product name: $product"
    [ "$product" = linecon0 ] || { echo "the executable has lost its version details"; exit 1; }
    ;;
esac
export LINECON_SMOKE_FRAMES=300
if [ "$(uname -s)" = Linux ]; then
  Xvfb :99 -screen 0 1280x800x24 >/dev/null 2>&1 &
  export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1
  sleep 1
fi
"$binary" --smoke-test smoke.txt &
app=$!
for _ in $(seq 1 90); do
  sleep 1
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
      if [ ! -e smoke-screen.png ] && [ "$_" != 0 ]; then :; fi ;;
  esac
  [ -s smoke.txt ] && break
  kill -0 "$app" 2>/dev/null || break
  if [ "${shot:-0}" = 0 ] && [ "$SECONDS" -ge 6 ]; then
    shot=1
    case "$(uname -s)" in
      MINGW*|MSYS*|CYGWIN*)
        powershell -NoProfile -Command "Add-Type -AssemblyName System.Windows.Forms,System.Drawing; \$b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds; \$i=New-Object System.Drawing.Bitmap \$b.Width,\$b.Height; [System.Drawing.Graphics]::FromImage(\$i).CopyFromScreen(\$b.Location,[System.Drawing.Point]::Empty,\$b.Size); \$i.Save('smoke-screen.png')" || true ;;
      Darwin) screencapture -x smoke-screen.png || true ;;
    esac
  fi
done
kill "$app" 2>/dev/null
wait "$app" 2>/dev/null
echo "--- smoke result"
cat smoke.txt 2>/dev/null || echo "no result: the window never finished opening"
grep -q "^window opened" smoke.txt 2>/dev/null
