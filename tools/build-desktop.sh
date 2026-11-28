#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

platform="${1:?Pass the native GOOS/GOARCH target}"
case "$platform" in
  darwin/arm64) release_platform=macos-arm64 ;;
  windows/amd64) release_platform=windows-x64 ;;
  windows/arm64) release_platform=windows-arm64 ;;
  *) echo "Unsupported native release target: $platform" >&2; exit 1 ;;
esac
if [[ "$(go env GOOS)/$(go env GOARCH)" != "$platform" ]]; then
  echo "Build $platform on a native runner with its matching codec libraries." >&2
  exit 1
fi

if [[ "$platform" == windows/* ]]; then
  : "${MSYS2_LOCATION:?Install the native MSYS2 SDK first}"
  flavor=clang64
  if [[ "$platform" == windows/arm64 ]]; then flavor=clangarm64; fi
  native_sdk_root="$(cygpath -u "$MSYS2_LOCATION")"
  sdk="$native_sdk_root/$flavor/bin"
  export PATH="$sdk:$native_sdk_root/usr/bin:$PATH"
fi

version="$(cat VERSION)"
commit="$(git rev-parse HEAD)"
key="${LISTENBOX_UPDATE_PUBLIC_KEY:-}"
if [[ "${LISTENBOX_PRODUCTION_RELEASE:-0}" == 1 ]]; then
  : "${key:?A production release requires the existing Ed25519 update key}"
fi
export MYGO_LDFLAGS="${MYGO_LDFLAGS:+$MYGO_LDFLAGS }-X github.com/listenbox/client/crates/sync-engine.Version=$version -X github.com/listenbox/client/crates/sync-engine.SourceCommit=$commit -X github.com/listenbox/client/crates/sync-engine.Production=1 -X main.updatePublicKey=$key -X main.updatePlatform=$release_platform"

options=(-platform "$platform")
if [[ "$platform" == windows/* ]]; then options+=(-skip-installer); fi
go tool mygo build "${options[@]}" crates/desktop

runtime_environment=()
if [[ "$platform" == darwin/arm64 ]]; then
  bundle=crates/desktop/dist/mygo/darwin-arm64/Listenbox.app
  executable="$bundle/Contents/MacOS/Listenbox"
  image="crates/desktop/dist/macos/Listenbox-$version-macos-arm64.dmg"
  mkdir -p crates/desktop/dist/macos
  cp "crates/desktop/dist/mygo/darwin-arm64/listenbox-$version-arm64.dmg" "$image"
  lipo "$executable" -verify_arch arm64
  dependencies="$(otool -L "$executable")"
  printf '%s\n' "$dependencies"
  while IFS= read -r dependency; do
    case "$dependency" in
      /usr/lib/*|/System/Library/*) ;;
      *) echo "Packaged executable requires a non-system library: $dependency" >&2; exit 1 ;;
    esac
  done < <(printf '%s\n' "$dependencies" | awk 'NR > 1 {print $1}')
  while IFS= read -r search_path; do
    case "$search_path" in
      @executable_path/*|@loader_path/*) ;;
      *) echo "Packaged executable has an external runtime search path: $search_path" >&2; exit 1 ;;
    esac
  done < <(otool -l "$executable" | awk '$1 == "cmd" && $2 == "LC_RPATH" {rpath=1; next} rpath && $1 == "path" {print $2; rpath=0}')
  codesign --verify --deep --strict "$bundle"
  if [[ "${LISTENBOX_PRODUCTION_RELEASE:-0}" == 1 ]]; then
    xcrun stapler validate "$bundle"
    xcrun stapler validate "$image"
    spctl --assess --type execute --verbose=2 "$bundle"
  fi
  verification_root="$(mktemp -d)"
  trap 'hdiutil detach "$verification_root/mount" >/dev/null 2>&1 || true; rm -rf "$verification_root"' EXIT
  mkdir "$verification_root/mount"
  hdiutil attach -readonly -nobrowse -mountpoint "$verification_root/mount" "$image"
  bundle="$verification_root/mount/Listenbox.app"
  executable="$bundle/Contents/MacOS/Listenbox"
  codesign --verify --deep --strict "$bundle"
  if [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$bundle/Contents/Info.plist")" != icon.png ]] ||
    ! cmp -s crates/desktop/assets/icon.png "$bundle/Contents/Resources/icon.png"; then
    echo 'The installed app must use the original Listenbox icon PNG.' >&2
    exit 1
  fi
  resources=(COPYING LICENSE THIRD-PARTY-NOTICES.txt NOTICE.txt)
  if [[ -n "$key" ]]; then resources+=(Sparkle.LICENSE.txt Sparkle.framework/Sparkle); fi
  for resource in "${resources[@]}"; do
    if [[ ! -f "$bundle/Contents/Resources/$resource" ]]; then
      echo "Installed app is missing a required resource: $resource" >&2
      exit 1
    fi
  done
  runtime_environment=(env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin)
else
  package="crates/desktop/dist/$release_platform"
  mkdir -p "$package"
  cp -R "crates/desktop/dist/mygo/${platform//\//-}/." "$package/"
  executable="$package/listenbox-desktop.exe"
  machine=IMAGE_FILE_MACHINE_AMD64
  if [[ "$platform" == windows/arm64 ]]; then machine=IMAGE_FILE_MACHINE_ARM64; fi
  for file in "$executable" "$package/WinSparkle.dll"; do
    headers="$(llvm-readobj --file-headers "$file")"
    if [[ "$headers" != *"$machine"* ]]; then
      echo "Wrong PE architecture: $file" >&2
      exit 1
    fi
  done
  headers="$(llvm-readobj --file-headers "$executable")"
  if [[ "$headers" != *IMAGE_SUBSYSTEM_WINDOWS_GUI* ]]; then
    echo 'The desktop executable must use the Windows GUI subsystem.' >&2
    exit 1
  fi
  imports="$(llvm-readobj --coff-imports "$executable")"
  printf '%s\n' "$imports"
  if [[ "${imports,,}" =~ (vcruntime|msvcp|ucrtbased|libc\+\+|libunwind|libgcc|libwinpthread|avcodec|avformat|avdevice|avfilter|avutil|swscale|swresample).*\.dll ]]; then
    echo 'The shipped executable requires an undistributed compiler or codec runtime.' >&2
    exit 1
  fi
fi
reported_version="$("${runtime_environment[@]}" "$executable" --version)"
if [[ "$reported_version" != "Listenbox $version ($commit)" ]]; then
  echo 'Packaged desktop version/source or startup is invalid.' >&2
  exit 1
fi
