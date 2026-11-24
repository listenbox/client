package thirdparty

import _ "embed"

// YouTubeJSBytecode is generated during the native build with the client's
// pinned QuickJS library. It is trusted build input, never downloaded bytecode.
//
//go:embed bundle/quickjs.bytecode
var YouTubeJSBytecode []byte

// YouTubeJSIdentity describes the embedded bundle's source: its fork commit,
// commit time (the release sequence), size and SHA-256.
//
//go:embed bundle/quickjs.json
var YouTubeJSIdentity []byte
