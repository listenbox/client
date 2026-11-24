package youtubei

import (
	"errors"
	"fmt"
	"runtime"

	q "github.com/buke/quickjs-go"
	thirdparty "github.com/listenbox/client/third_party"
)

// BundleABI names the contract between these bindings and host primitives and
// the YouTube.js bundle they evaluate. Bump it whenever a bundle built from
// newer sources would break older bindings, or newer bindings need a newer
// bundle; signed bundle releases carry the ABI they were built for.
const BundleABI = 1

// Bundle is YouTube.js code an Engine evaluates. Its bytecode only ever comes
// from the build (Embedded) or from compiling source in this process
// (Compile): QuickJS trusts bytecode, so it is never read from elsewhere.
type Bundle struct{ bytecode []byte }

// Embedded returns the bundle compiled into this build.
func Embedded() Bundle { return Bundle{thirdparty.YouTubeJSBytecode} }

// Compile compiles bundle source in a throwaway runtime. Syntax errors fail
// here, before any Engine depends on the bundle.
func Compile(source []byte) (Bundle, error) {
	type result struct {
		bytecode []byte
		err      error
	}
	done := make(chan result, 1)
	go func() {
		runtime.LockOSThread()
		defer runtime.UnlockOSThread()
		rt := q.NewRuntime(
			q.WithMemoryLimit(runtimeMemoryLimit),
			q.WithMaxStackSize(runtimeStackLimit),
			q.WithStrictOSThread(true),
		)
		defer rt.Close()
		js := rt.NewBareContext()
		if js == nil {
			done <- result{err: errors.New("create QuickJS context")}
			return
		}
		defer js.Close()
		bytecode, err := js.Compile(string(source), q.EvalFileName("youtubejs.js"))
		done <- result{bytecode, err}
	}()
	compiled := <-done
	if compiled.err != nil {
		return Bundle{}, fmt.Errorf("compile YouTube.js bundle: %w", compiled.err)
	}
	return Bundle{compiled.bytecode}, nil
}
