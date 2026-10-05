#!/usr/bin/env bash

set -euo pipefail

readonly expected_sha256="37666d6afa569cbd1c6ed9ad4ac6503654c1cd47110a30f72afd72f4332ac8f0"
readonly archive="${1:?Apple HLS tools archive path is required}"

read -r actual_sha256 _ < <(sha256sum "$archive")
if [[ "$actual_sha256" != "$expected_sha256" ]]; then
  echo "Apple HLS tools archive SHA-256 is $actual_sha256, want $expected_sha256" >&2
  exit 1
fi

# Ubuntu provides this canonical OS metadata file.
# shellcheck disable=SC1091
source /etc/os-release
if [[ "$ID" != "ubuntu" ]]; then
  echo "Apple HLS tools CI package requires Ubuntu, found $ID" >&2
  exit 1
fi
if [[ "$(dpkg --print-architecture)" != "amd64" ]]; then
  echo "Apple HLS tools CI package requires amd64" >&2
  exit 1
fi

install_root="$(mktemp -d)"
trap 'rm -rf "$install_root"' EXIT
tar -xf "$archive" -C "$install_root"
dependency_root="$install_root/Dependencies/Deploy/deb"
packages=(
  "$dependency_root/libblocksruntime-release-0.4.1-1.1.20241025.HLSPublic01.lts.x86_64.deb"
  "$dependency_root/libdispatch-release-5.8-1.1.20241025.HLSPublic01.lts.x86_64.deb"
  "$dependency_root/libmd-release-1.1.0-1.1.20241025.HLSPublic01.lts.x86_64.deb"
  "$dependency_root/libbsd-release-0.12.2-1.1.20241025.HLSPublic01.lts.x86_64.deb"
  "$dependency_root/corefoundation-release-ubuntu-5.9-1.1.20251106.1.26.x86_64.deb"
  "$dependency_root/IFS4L-release-1.8-1.20241029.HLSPublic01.lts.x86_64.deb"
  "$dependency_root/calinuxbase-release-public-1-1.20241106.REL.lts.26e7711f4.x86_64.deb"
  "$dependency_root/caulk-release-public-1.185.1-1.20241106.REL.lts.4542dbdad.x86_64.deb"
  "$dependency_root/coreaudioservices-release-public-1.1463.1-1.20241106.REL.lts.af8a62253.x86_64.deb"
  "$dependency_root/audiocodecs-hls-public-release-1.754.1-1.20241106.REL.lts.b1fe91791.x86_64.deb"
  "$dependency_root/CoreGraphics-hls-public-release-0.0.1.24299-1.20241025.1ENG010434.x86_64.deb"
  "$dependency_root/ColorSync-release-3703.1-1.20241025.1ENG010814.x86_64.deb"
  "$dependency_root/CoreVideo-release-330.2-1.20241025.99.x86_64.deb"
  "$install_root/hlstools-ubuntu-release-1.26.143-1.26.143-1.20260527.5.x86_64.deb"
)

elevate=()
if [[ "$(id -u)" != "0" ]]; then
  elevate=(sudo)
fi
"${elevate[@]}" apt-get update
"${elevate[@]}" env DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
  libatomic1 \
  libcurl4t64 \
  libgcc-s1 \
  libicu74 \
  libssl3t64 \
  libstdc++6 \
  libuuid1
for package_file in "${packages[@]}"; do
  "${elevate[@]}" dpkg --install "$package_file"
done
printf '/usr/lib64\n/usr/local/lib64\n' > "$install_root/60-apple-hls-tools.conf"
"${elevate[@]}" install --mode=0644 \
  "$install_root/60-apple-hls-tools.conf" \
  /etc/ld.so.conf.d/60-apple-hls-tools.conf
"${elevate[@]}" ldconfig

mediastreamvalidator --version
