//go:build darwin || windows

package main

// Packaging injects the same Ed25519 key and feed used by existing releases.
// A development build has no updater; release tasks require a nonempty key.
// Only the Sparkle (macOS) and WinSparkle (Windows) updaters read these.
var (
	updatePublicKey string
	updatePlatform  string
)
