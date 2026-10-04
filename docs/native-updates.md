# Native desktop updates

Implementation contract: Sparkle 2.10.0 on macOS ARM64 and WinSparkle 0.9.4
on Windows x64/ARM64 own checking, persisted preferences, download verification,
native prompts, deferral and installation. The pinned archives are SHA-256
verified before extraction, and their licenses accompany installed files.
Developer builds have no native updater or production checks. Release builds
enable `native-updater` explicitly and require a valid public key.

Both frameworks check every 24 hours. Settings exposes Check for Updates and
Automatically Check for Updates on macOS and Windows, using the native
framework's persisted preference. Developer builds show both controls disabled.
The macOS Listenbox menu also offers Check for Updates. Manual checks bring the
app forward and use native current/error feedback.

The workspace TaskTracker remains the authoritative owner of admitted work.
Updater callbacks only enqueue a request onto GPUI. The workspace cancels new
work, closes admission and waits for admitted writes, embedded FFmpeg cleanup
and SQLite commits. Sparkle's retained installation handler runs on the main
thread after that drain. WinSparkle's shutdown callback never waits or cleans
up the DLL; cleanup happens after GPUI returns and the final runtime drain.
The Windows installer waits for the app's mutex and never terminates the old
process. If draining stalls, replacement is refused and the old installation
remains usable. Credentials, settings, journals and downloads are owned by the
persistent profile, outside all installation directories.

The macOS bridge also gates Cocoa termination, covering Sparkle paths that omit
the postponement callback when an update is already staged. It routes termination
through the same workspace drain and permits Cocoa to exit only afterward. An
update requested during an existing drain uses that drain instead of discarding
the installation request. A logout already in progress finishes its explicit
credential removal before the update proceeds.

## Stable identity and publication

`Cargo.toml` workspace version is the sole release identity: `X.Y.Z`, with
each component between 0 and 65535. Allocate a strictly greater version, update
the lockfile and create the matching immutable `vX.Y.Z` tag. macOS
compares `X.Y.Z`; Windows compares `X.Y.Z.0`. Display version is `X.Y.Z`;
the source commit is provenance, never update ordering. Every target comes
from the tagged commit. Master pushes run standard CI only; native builds run
on pull requests and version tags, avoiding duplicate native builds when a
release commit and its tag are pushed. Pull request builds publish no stable
release and receive no production secrets.

Windows PR jobs compile the native updater, verify DLL loading and
compile the same per-user installer as stable releases. Their
`desktop-validation` environment contains only public updater keys. macOS
validation packages an ad-hoc development bundle; protected tags enable its
native updater and Developer ID/notarization path.

The `desktop-release` environment allows only `v*` tags and has no required
reviewers or wait timer. Pushing a matching version tag automatically starts
the release jobs and publication without manual deployment approval. Its
jobs sign/notarize the macOS artifact and build
Windows per-user installers before generating separate architecture appcasts.
Final payload bytes receive Ed25519 signatures after packaging and macOS
stapling. Missing updater or Apple signing credentials fail closed.

Windows update authenticity uses the embedded Ed25519 public key and final
installer signature in the appcast. It does not require a Microsoft account or
an Authenticode certificate. Current Windows executables/installers are unsigned
for publisher identity and may show SmartScreen warnings. The SignPath
application proceeds independently; its approval does not block these releases.

The publisher stages the complete set in a draft, verifies uploaded bytes,
publishes with `latest=false`, verifies anonymous tag-specific HTTPS downloads,
then advances Latest only after rejecting stale versions. A single release
concurrency group serializes promotion. Draft retries compare bytes and identity;
published assets are never overwritten. A failed build, signature, upload or
download check leaves the previous stable release selected. Fix a published
defect with a higher version. Never delete/reuse a published tag or edit its
assets. Remove an abandoned draft only after checking its tag and commit.

## Owner setup

Environment secrets (enter directly in GitHub; never in chat or public source):

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE_P12_BASE64` | Password-protected Developer ID Application certificate and private key, base64 transport |
| `MACOS_CERTIFICATE_PASSWORD` | Its export password |
| `MACOS_NOTARY_KEY_P8_BASE64` | Team App Store Connect API private key, base64 transport |
| `MACOS_NOTARY_KEY_ID` | API key ID |
| `MACOS_NOTARY_ISSUER_ID` | Team issuer UUID |
| `MACOS_UPDATE_SEED_BASE64` | Independent raw 32-byte Ed25519 private seed |
| `WINDOWS_UPDATE_SEED_BASE64` | Independent raw 32-byte Ed25519 private seed |

Environment variables: `MACOS_SIGNING_IDENTITY`, `MACOS_TEAM_ID`,
`MACOS_UPDATE_PUBLIC_KEY`, `WINDOWS_UPDATE_PUBLIC_KEY` (base64 raw 32-byte public
keys). The owner keeps the complete signing backup in `apps/client-signing/` in
the private parent repository, outside this public submodule. That folder has
separate `macos/` and `windows/` material plus restore/build Moon commands.
Updater keys
do not confer Apple/Windows distribution trust. Rotation requires a planned
release signed by the currently trusted updater key before embedding a new key;
do not replace a public key and assume installed clients will trust it.

The macOS job imports into an ephemeral keychain, validates the configured
identity/team, re-signs Sparkle helpers inside out with hardened runtime, signs
the app, waits for notarization acceptance, staples the app, creates/signs the
DMG, notarizes/staples the DMG and verifies final bytes. An always-run step
removes credentials and the keychain. Windows packages the application,
architecture-matched WinSparkle DLL and licenses with Inno Setup, then signs
the final installer bytes with Ed25519 for the appcast.

## Installation and recovery

The first updater-enabled release needs manual installation. macOS users drag
the app to Applications. Windows users install the matching per-user Setup EXE
under `%LOCALAPPDATA%\Programs\Listenbox`; old portable users migrate by
closing the portable app and running Setup. The existing profile at
`%LOCALAPPDATA%\Listenbox` is preserved. Bare EXEs and portable ZIPs are no
longer release deliverables. Uninstall removes installation files only.

Interrupted downloads do not alter the installation. Native signature rejection
must precede installer launch. Installer permission/replacement failures remain
visible and must leave the profile intact; rerun the installer after closing
the app. No force-kill or profile cleanup is an update recovery mechanism.

## Verification plan and recorded evidence

Required proof at the native installed-app boundary, on clean macOS ARM64,
Windows x64 and native Windows ARM64: install updater-enabled N, publish N+1 through
Actions, check manually/periodically, defer/cancel, update while admitted work
is gated, observe actual process closure and changed executable/version/arch,
then verify credentials, settings, SQLite and downloads survive. The next
check must report current. Test equal/older versions through pinned frameworks;
reject modified/missing/wrong signatures, malformed feeds and incompatible OS.
Inject interrupted install/download and denied permissions without repairing
state before assertions. Assert admitted work is reached before cancellation.

Publication proof must cover missing targets, missing signing/notary inputs,
partial uploads, same-identity draft retries, conflicting bytes, and stale
promotion. Tests must exercise the actual publisher boundary rather than
restating manifest values. Local smoke checks and compilation do not prove
the real cross-platform upgrade path. No real cross-platform upgrade result
has been recorded yet. Apple credentials are provisioned; native Windows
machines and two complete release versions are needed for upgrade acceptance.
