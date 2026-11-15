// Command youtubejsbuild compiles the bundled upstream JavaScript with the same
// native QuickJS library that the client embeds.
package main

import (
	"fmt"
	"os"
	"runtime"

	q "github.com/buke/quickjs-go"
)

const generatedFileMode = 0o600

func main() {
	if err := compileBundle(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func compileBundle() error {
	source, err := os.ReadFile("third_party/bundle/quickjs.js")
	if err != nil {
		return fmt.Errorf("read bundled YouTube.js: %w", err)
	}
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	rt := q.NewRuntime(q.WithStrictOSThread(true))
	defer rt.Close()
	js := rt.NewBareContext()
	defer js.Close()
	bytecode, err := js.Compile(string(source), q.EvalFileName("youtubejs.js"))
	if err != nil {
		return fmt.Errorf("compile bundled YouTube.js: %w", err)
	}
	// #nosec G703 -- Moon owns this fixed build output; source bytes never select its path.
	if err := os.WriteFile("third_party/bundle/quickjs.bytecode", bytecode, generatedFileMode); err != nil {
		return fmt.Errorf("write compiled YouTube.js: %w", err)
	}
	return nil
}
