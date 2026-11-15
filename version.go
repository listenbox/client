package client

import (
	_ "embed"
	"strings"
)

//go:embed VERSION
var version string

// Version returns the release identity shared with the packaging tools.
func Version() string { return strings.TrimSpace(version) }
