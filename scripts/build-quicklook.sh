#!/bin/bash
# Build directly into one ignored, non-indexed output. No Xcode project.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
target="${1:-${TAURI_ENV_TARGET_TRIPLE:-$(rustc -vV | sed -n 's/^host: //p')}}"
out="$root/src-tauri/target/quicklook.noindex"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
case "$target" in
  universal-apple-darwin) arches=(aarch64 x86_64);;
  aarch64-apple-darwin) arches=(aarch64);;
  x86_64-apple-darwin) arches=(x86_64);;
  *) echo "Unsupported Quick Look target: $target" >&2; exit 1;;
esac
mkdir -p "$out/module-cache"
export CLANG_MODULE_CACHE_PATH="$out/module-cache"
export SWIFT_MODULECACHE_PATH="$out/module-cache"
for arch in "${arches[@]}"; do
  cargo build --offline --manifest-path src-tauri/Cargo.toml --release --target "$arch-apple-darwin" -p tmv-thumbnail-ffi
done
for kind in Thumbnail Preview; do
  bundle="$out/Tmv${kind}.appex"
  mkdir -p "$bundle/Contents/MacOS"
  for arch in "${arches[@]}"; do
    swiftarch="$arch"; [ "$arch" != aarch64 ] || swiftarch=arm64
    framework=QuickLookThumbnailing; minimum=10.15
    if [ "$kind" = Preview ]; then framework=QuickLookUI; minimum=12.0; fi
    xcrun swiftc -module-cache-path "$out/module-cache" -O -parse-as-library -application-extension -sdk "$sdk" \
      -target "$swiftarch-apple-macosx$minimum" -module-name "Tmv$kind" \
      -import-objc-header packaging/macos/Thumbnail.h \
      packaging/macos/Image.swift "packaging/macos/${kind}Provider.swift" \
      "src-tauri/target/$arch-apple-darwin/release/libtmv_thumbnail_ffi.a" \
      -framework "$framework" -framework AppKit -framework Security -framework SystemConfiguration \
      -lc++ -lz -Xlinker -dead_strip -Xlinker -e -Xlinker _NSExtensionMain \
      -o "$out/$kind-$arch"
  done
  binaries=()
  for arch in "${arches[@]}"; do binaries+=("$out/$kind-$arch"); done
  if [ "${#binaries[@]}" = 1 ]; then cp "${binaries[0]}" "$bundle/Contents/MacOS/Tmv$kind"
  else lipo -create "${binaries[@]}" -output "$bundle/Contents/MacOS/Tmv$kind"; fi
  python3 scripts/quicklook-plist.py "$kind" "$bundle/Contents/Info.plist"
  codesign --force --sign - --entitlements packaging/macos/extension.entitlements "$bundle"
  codesign --verify --strict "$bundle"
done
