package thirdparty

import _ "embed"

// YouTubeJSBytecode is generated during the native build with the client's
// pinned QuickJS library. It is trusted build input, never downloaded bytecode.
//
//go:embed bundle/quickjs.bytecode
var YouTubeJSBytecode []byte
