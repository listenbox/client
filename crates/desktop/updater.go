package main

// Packaging injects the same Ed25519 key and feed used by existing releases.
// A development build has no updater; release tasks require a nonempty key.
//
//nolint:unused // Native release builds inject these values; macOS and Windows consume them.
var (
	updatePublicKey string
	updatePlatform  string
)

type nativeUpdater interface {
	canCheck() bool
	check()
	automatic() bool
	setAutomatic(enabled bool)
	automaticDownloads() bool
	setAutomaticDownloads(enabled bool)
	drained() bool
	close()
}
