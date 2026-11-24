// Package youtubejsrelease defines the signed manifest that authenticates
// YouTube.js bundles published as GitHub releases of the fork. The release
// key's private half stays offline; GitHub, the network and the profile
// directory are untrusted, so clients evaluate only source this key signed.
package youtubejsrelease

import (
	"bytes"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"regexp"
	"strconv"
	"strings"
)

const (
	Repository    = "listenbox/YouTube.js"
	SourceAsset   = "youtubejs.js"
	ManifestAsset = "youtubejs.json"
	// SourceLimit and ManifestLimit bound every download before verification.
	SourceLimit   = 16 << 20
	ManifestLimit = 64 << 10
	// signatureContext separates these signatures from any other use of the key.
	signatureContext = "listenbox youtubejs release v1\n"
)

// PublicKey verifies production releases.
var PublicKey = ed25519.PublicKey(mustDecode("AR4hVDIhsP1dYU2UfIviBBrTqr7jmawFksR2bjRUTM4="))

// Manifest describes one bundle. Release is its version number within its
// ABI, assigned by the fork's release workflow. Sequence is its YouTube.js
// commit time, which the embedded bundle also knows; clients only move to a
// later sequence, so an old signed bundle cannot be replayed under a new tag.
type Manifest struct {
	ABI      int    `json:"abi"`
	Release  int64  `json:"release"`
	Sequence int64  `json:"sequence"`
	SHA256   string `json:"sha256"`
	Size     int64  `json:"size"`
	Commit   string `json:"commit"`
}

type envelope struct {
	Payload   []byte `json:"payload"`
	Signature []byte `json:"signature"`
}

var (
	digestPattern = regexp.MustCompile(`^[0-9a-f]{64}$`)
	commitPattern = regexp.MustCompile(`^[0-9a-f]{40}$`)
)

func mustDecode(value string) []byte {
	raw, err := base64.StdEncoding.DecodeString(value)
	if err != nil || len(raw) != ed25519.PublicKeySize {
		panic("invalid YouTube.js release key")
	}
	return raw
}

// TagPrefix starts every release tag for one bundle ABI. Releases are
// versioned bundle-v<ABI>.<release>: the major version is the host contract
// and the release number only grows, so clients list just their ABI's tags.
func TagPrefix(abi int) string { return "bundle-v" + strconv.Itoa(abi) + "." }

// Tag names the GitHub release that carries the manifest.
func (m Manifest) Tag() string { return TagPrefix(m.ABI) + strconv.FormatInt(m.Release, 10) }

// ReleaseOf parses a release tag for abi; ok is false for any other tag.
func ReleaseOf(tag string, abi int) (int64, bool) {
	digits, found := strings.CutPrefix(tag, TagPrefix(abi))
	if !found || digits == "" || strings.TrimLeft(digits, "0123456789") != "" || digits[0] == '0' {
		return 0, false
	}
	release, err := strconv.ParseInt(digits, 10, 64)
	return release, err == nil
}

// Matches reports whether source is exactly the described bundle.
func (m Manifest) Matches(source []byte) error {
	if int64(len(source)) != m.Size {
		return fmt.Errorf("YouTube.js bundle is %d bytes, manifest says %d", len(source), m.Size)
	}
	digest := sha256.Sum256(source)
	if hex.EncodeToString(digest[:]) != m.SHA256 {
		return errors.New("YouTube.js bundle digest does not match its manifest")
	}
	return nil
}

func (m Manifest) validate() error {
	switch {
	case m.ABI <= 0:
		return errors.New("manifest ABI must be positive")
	case m.Release <= 0:
		return errors.New("manifest release must be positive")
	case m.Sequence <= 0:
		return errors.New("manifest sequence must be positive")
	case !digestPattern.MatchString(m.SHA256):
		return errors.New("manifest digest must be lowercase SHA-256 hex")
	case m.Size <= 0 || m.Size > SourceLimit:
		return fmt.Errorf("manifest size %d is outside 1..%d", m.Size, SourceLimit)
	case !commitPattern.MatchString(m.Commit):
		return errors.New("manifest commit must be a full lowercase SHA-1")
	}
	return nil
}

// Open verifies a manifest asset's signature before decoding its claims.
func Open(raw []byte, key ed25519.PublicKey) (Manifest, error) {
	var signed envelope
	if err := strictDecode(raw, &signed); err != nil {
		return Manifest{}, fmt.Errorf("decode YouTube.js release manifest: %w", err)
	}
	if len(signed.Signature) != ed25519.SignatureSize ||
		!ed25519.Verify(key, append([]byte(signatureContext), signed.Payload...), signed.Signature) {
		return Manifest{}, errors.New("YouTube.js release manifest signature is invalid")
	}
	var manifest Manifest
	if err := strictDecode(signed.Payload, &manifest); err != nil {
		return Manifest{}, fmt.Errorf("decode signed YouTube.js manifest: %w", err)
	}
	return manifest, manifest.validate()
}

func strictDecode(raw []byte, out any) error {
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(out); err != nil {
		return err
	}
	if _, err := decoder.Token(); !errors.Is(err, io.EOF) {
		return errors.New("trailing data")
	}
	return nil
}
