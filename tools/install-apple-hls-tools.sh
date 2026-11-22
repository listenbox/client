#!/usr/bin/env bash
# Mise postinstall for http:apple-hls-tools. Mise has already verified Apple's
# downloaded archive against mise.lock and placed it in the install directory.
# This unpacks it without root and exposes every CLI from the install's bin/.

set -euo pipefail

readonly install_path="${MISE_TOOL_INSTALL_PATH:?run this as the mise postinstall of http:apple-hls-tools}"
work="$(mktemp -d)"
mount_point="$work/mount"
cleanup() {
  if [[ -d "$mount_point" ]] && mount | grep -qF " on $mount_point "; then
    hdiutil detach -quiet "$mount_point" || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

case "$(uname -s)" in
Darwin)
  # The universal macOS binaries only need system frameworks.
  dmg="$(find "$install_path" -name '*.dmg' -print -quit)"
  mkdir "$mount_point"
  hdiutil attach -quiet -nobrowse -readonly -noautoopen -mountpoint "$mount_point" "$dmg"
  packages=("$mount_point"/*.pkg)
  pkgutil --expand-full "${packages[0]}" "$work/pkg"
  hdiutil detach -quiet "$mount_point"
  payloads=("$work"/pkg/*.pkg/Payload)
  find "$install_path" -mindepth 1 -delete
  cp -R "${payloads[0]}/usr/local/." "$install_path/"
  ;;
Linux)
  # Apple's tgz holds a tar of Debian packages and that tar's checksum file.
  cd "$install_path"
  checksums=(./*.sha256)
  sha256sum --check --quiet "${checksums[0]}"
  tar -xf ./*.tar -C "$work"
  root="$install_path/apple"
  for package in "$work"/Dependencies/Deploy/deb/*.deb "$work"/hlstools-*-ubuntu-*.deb; do
    # deploy.sh installs this CoreFoundation build only on Debian.
    [[ "$package" == *corefoundation-release-debian-* ]] && continue
    dpkg-deb --extract "$package" "$root"
  done
  find "$install_path" -mindepth 1 -maxdepth 1 ! -name apple -exec rm -rf {} +
  # Apple's packages expect ldconfig to know these directories.
  libraries="$root/usr/lib64:$root/usr/local/lib:$root/usr/local/lib64"
  mkdir "$install_path/bin"
  for executable in "$root"/usr/local/bin/*; do
    wrapper="$install_path/bin/${executable##*/}"
    cat > "$wrapper" <<EOF
#!/bin/sh
export LD_LIBRARY_PATH="$libraries\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
exec "$executable" "\$@"
EOF
    chmod 0755 "$wrapper"
  done
  ;;
*)
  echo "Apple HLS tools support only macOS and Linux x64" >&2
  exit 1
  ;;
esac

"$install_path/bin/mediastreamvalidator" --version
