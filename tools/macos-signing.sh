#!/usr/bin/env bash
set -euo pipefail

directory="${RUNNER_TEMP:?}/listenbox-signing"
keychain="$directory/release.keychain-db"
if [[ "${1:-}" == cleanup ]]; then
  if [[ -f "$directory/keychains" ]]; then
    keychains=()
    while IFS= read -r previous; do keychains+=("$previous"); done < "$directory/keychains"
    security list-keychains -d user -s "${keychains[@]}"
  fi
  if [[ -f "$keychain" ]]; then security delete-keychain "$keychain"; fi
  rm -rf "$directory"
  exit 0
fi
[[ "${1:-}" == setup ]] || { echo 'Expected setup or cleanup' >&2; exit 1; }
for name in MACOS_CERTIFICATE_P12_BASE64 MACOS_CERTIFICATE_PASSWORD MACOS_SIGNING_IDENTITY MACOS_TEAM_ID MACOS_NOTARY_KEY_P8_BASE64 MACOS_NOTARY_KEY_ID MACOS_NOTARY_ISSUER_ID; do
  [[ -n "${!name:-}" ]] || { echo "Missing signing input: $name" >&2; exit 1; }
done
[[ "$MACOS_SIGNING_IDENTITY" == "Developer ID Application: "*" ($MACOS_TEAM_ID)" ]] || { echo 'Wrong Developer ID identity/team' >&2; exit 1; }
umask 077
mkdir -p "$directory"
security list-keychains -d user | sed 's/^ *"//; s/"$//' > "$directory/keychains"
printf '%s' "$MACOS_CERTIFICATE_P12_BASE64" | base64 --decode > "$directory/certificate.p12"
printf '%s' "$MACOS_NOTARY_KEY_P8_BASE64" | base64 --decode > "$directory/notary.p8"
password="$(openssl rand -hex 32)"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$directory/certificate.p12" -k "$keychain" -P "$MACOS_CERTIFICATE_PASSWORD" -T /usr/bin/codesign -T /usr/bin/security
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain" > /dev/null
keychains=("$keychain")
while IFS= read -r previous; do keychains+=("$previous"); done < "$directory/keychains"
security list-keychains -d user -s "${keychains[@]}"
security find-identity -v -p codesigning "$keychain" | grep -F "\"$MACOS_SIGNING_IDENTITY\"" > /dev/null
{
  printf 'LISTENBOX_SIGNING_IDENTITY=%s\n' "$MACOS_SIGNING_IDENTITY"
  printf 'LISTENBOX_SIGNING_KEYCHAIN=%s\n' "$keychain"
  printf 'LISTENBOX_NOTARY_KEY=%s\n' "$directory/notary.p8"
} >> "${GITHUB_ENV:?}"
