package youtubei

import (
	"context"
	"crypto/rand"
	"crypto/sha1" // #nosec G505 -- Required by the upstream YouTube protocol.
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"runtime"
	"strings"
	"sync"
	"time"

	q "github.com/buke/quickjs-go"
	"github.com/google/uuid"
	"golang.org/x/sync/errgroup"
)

type FetchRequest struct {
	URL     string      `json:"url"`
	Method  string      `json:"method"`
	Headers [][2]string `json:"headers"`
	Body    []byte      `json:"body"`
}
type FetchResponse struct {
	URL     string      `json:"url"`
	Status  int         `json:"status"`
	Headers [][2]string `json:"headers"`
	Body    []byte      `json:"body"`
}
type (
	Fetch func(context.Context, FetchRequest) (FetchResponse, error)
	Error struct {
		Message           string
		Info              json.RawMessage
		PlayabilityStatus *PlayabilityStatus
		Cause             error
	}
)

const (
	runtimeMemoryLimit = 768 << 20
	runtimeStackLimit  = 4 << 20
)

func (err *Error) Error() string { return err.Message }
func (err *Error) Unwrap() error { return err.Cause }

//nolint:containedctx // Each message owns the caller context until its JS operation and host I/O drain.
type call struct {
	ctx        context.Context
	expression string
	arguments  any
	result     chan reply
}
type reply struct {
	raw []byte
	err error
}
type Engine struct {
	calls     chan call
	done      chan struct{}
	closeOnce sync.Once
	mu        sync.Mutex
	closed    bool
}

func New(ctx context.Context, fetch Fetch, bundle Bundle) (*Engine, error) {
	if len(bundle.bytecode) == 0 {
		return nil, errors.New("empty YouTube.js bundle")
	}
	engine := &Engine{calls: make(chan call), done: make(chan struct{})}
	ready := make(chan error, 1)
	go engine.own(ctx, fetch, bundle, ready)
	if err := <-ready; err != nil {
		return nil, err
	}
	return engine, nil
}

func (engine *Engine) Close() {
	engine.closeOnce.Do(
		func() { engine.mu.Lock(); engine.closed = true; close(engine.calls); engine.mu.Unlock(); <-engine.done },
	)
}

func (engine *Engine) Call(ctx context.Context, expression string, arguments, out any) error {
	request := call{ctx, expression, arguments, make(chan reply, 1)}
	engine.mu.Lock()
	if engine.closed {
		engine.mu.Unlock()
		return errors.New("YouTube runtime closed")
	}
	select {
	case <-ctx.Done():
		engine.mu.Unlock()
		return ctx.Err()
	case <-engine.done:
		engine.mu.Unlock()
		return errors.New("YouTube runtime closed")
	case engine.calls <- request:
		engine.mu.Unlock()
	}
	result := engine.waitReply(ctx, request)
	if result.err != nil {
		return result.err
	}
	if out == nil {
		return nil
	}
	if err := json.Unmarshal(result.raw, out); err != nil {
		return fmt.Errorf("decode upstream YouTube.js value: %w", err)
	}
	return nil
}

//nolint:cyclop,gocognit,gocyclo,fatcontext,funlen,lll,maintidx // The thread-confined bindings and event loop share one request scope.
func (engine *Engine) own(initial context.Context, fetch Fetch, bundle Bundle, ready chan<- error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	defer close(engine.done)
	rt := q.NewRuntime(
		q.WithMemoryLimit(runtimeMemoryLimit),
		q.WithMaxStackSize(runtimeStackLimit),
		q.WithStrictOSThread(true),
	)
	defer rt.Close()
	js := rt.NewBareContext()
	if js == nil {
		ready <- errors.New("create QuickJS context")
		return
	}
	defer js.Close()
	operation := initial
	var jobs errgroup.Group
	hostErrors := make(map[int64]error)
	var nextError int64
	rt.SetInterruptHandler(func() int {
		if operation.Err() != nil {
			return 1
		}
		return 0
	})
	globals := js.Globals()
	defer globals.Free()
	bind := func(name string, fn func(*q.Context, *q.Value, []*q.Value) *q.Value) {
		globals.Set(name, js.NewFunction(fn))
	}
	marshal := func(value any) *q.Value {
		raw, err := json.Marshal(value)
		if err != nil {
			return js.ThrowError(err)
		}
		return js.ParseJSON(string(raw))
	}
	// __hostFetchStart only starts the transport and returns an id; the JS
	// promise and its resolvers live in hostPrimitives. QuickJS-go's NewPromise
	// would evaluate JS and drain scheduled jobs inside this host call, so
	// another call could run inside the caller and a cancellation could land
	// in its internal evaluation, leaving the runtime in an exception state.
	var nextFetch int64
	bind("__hostFetchStart", func(ctx *q.Context, _ *q.Value, args []*q.Value) *q.Value {
		var request FetchRequest
		if len(args) != 1 || json.Unmarshal([]byte(args[0].JSONStringify()), &request) != nil {
			return ctx.ThrowError(errors.New("invalid fetch request"))
		}
		nextFetch++
		id := nextFetch
		requestContext := operation
		jobs.Go(func() error {
			response, err := fetch(requestContext, request)
			ctx.Schedule(func(ctx *q.Context) {
				settled := ctx.NewInt64(id)
				defer settled.Free()
				var result *q.Value
				if err != nil {
					nextError++
					hostErrors[nextError] = err
					failure := ctx.NewError(fmt.Errorf("fetch: %w", err))
					defer failure.Free()
					failure.Set("__goError", ctx.NewInt64(nextError))
					missing := ctx.NewNull()
					defer missing.Free()
					result = globals.Call("__hostFetchSettle", settled, missing, failure)
				} else {
					value := marshal(response)
					defer value.Free()
					result = globals.Call("__hostFetchSettle", settled, value)
				}
				if result != nil {
					result.Free()
				}
			})
			return nil
		})
		return ctx.NewInt64(id)
	})
	bind("__textEncode", func(ctx *q.Context, _ *q.Value, args []*q.Value) *q.Value {
		return ctx.NewArrayBuffer([]byte(args[0].ToString()))
	})
	bind("__textDecode", func(ctx *q.Context, _ *q.Value, args []*q.Value) *q.Value {
		raw, err := base64.StdEncoding.DecodeString(args[0].ToString())
		if err != nil {
			return ctx.ThrowError(err)
		}
		return ctx.NewString(strings.ToValidUTF8(string(raw), "\ufffd"))
	})
	bind(
		"__randomUUID",
		func(ctx *q.Context, _ *q.Value, _ []*q.Value) *q.Value { return ctx.NewString(uuid.NewString()) },
	)
	bind("__randomBytes", func(ctx *q.Context, _ *q.Value, args []*q.Value) *q.Value {
		count := args[0].ToInt64()
		if count < 0 || count > 65536 {
			return ctx.ThrowError(fmt.Errorf("invalid random byte length %d", count))
		}
		raw := make([]byte, count)
		if _, err := rand.Read(raw); err != nil {
			return ctx.ThrowError(err)
		}
		return ctx.NewString(base64.StdEncoding.EncodeToString(raw))
	})
	bind("__digest", func(ctx *q.Context, _ *q.Value, args []*q.Value) *q.Value {
		raw, err := base64.StdEncoding.DecodeString(args[1].ToString())
		if err != nil {
			return ctx.ThrowError(err)
		}
		switch strings.ToUpper(args[0].ToString()) {
		case "SHA-1":
			// #nosec G401 -- SHA-1 is required by YouTube's existing protocol.
			hash := sha1.Sum(raw)
			return ctx.NewArrayBuffer(hash[:])
		case "SHA-256":
			hash := sha256.Sum256(raw)
			return ctx.NewArrayBuffer(hash[:])
		default:
			return ctx.ThrowError(fmt.Errorf("unsupported digest %q", args[0].ToString()))
		}
	})
	bind("__parseURL", func(ctx *q.Context, _ *q.Value, args []*q.Value) *q.Value {
		u, err := url.Parse(args[0].ToString())
		if err != nil {
			return ctx.ThrowError(errors.New("invalid URL"))
		}
		if len(args) > 1 && !args[1].IsUndefined() && args[1].ToString() != "" {
			base, err := url.Parse(args[1].ToString())
			if err != nil {
				return ctx.ThrowError(errors.New("invalid base URL"))
			}
			u = base.ResolveReference(u)
		}
		if !u.IsAbs() {
			return ctx.ThrowError(errors.New("URL must be absolute"))
		}
		password := ""
		username := ""
		if u.User != nil {
			username = u.User.Username()
			password, _ = u.User.Password()
		}
		origin := u.Scheme + "://" + u.Host
		path := u.EscapedPath()
		if path == "" {
			path = "/"
		}
		search := ""
		if u.RawQuery != "" {
			search = "?" + u.RawQuery
		}
		fragment := ""
		if u.Fragment != "" {
			fragment = "#" + u.EscapedFragment()
		}
		return marshal(
			map[string]string{
				"href":     u.String(),
				"protocol": u.Scheme + ":",
				"username": username,
				"password": password,
				"host":     u.Host,
				"hostname": u.Hostname(),
				"port":     u.Port(),
				"pathname": path,
				"search":   search,
				"hash":     fragment,
				"origin":   origin,
			},
		)
	})
	timers := make(map[int64]*time.Timer)
	defer func() {
		for _, timer := range timers {
			timer.Stop()
		}
	}()
	bind("__setTimer", func(ctx *q.Context, _ *q.Value, args []*q.Value) *q.Value {
		id, delay := args[0].ToInt64(), args[1].ToInt64()
		if delay < 0 {
			delay = 0
		}
		timers[id] = time.AfterFunc(time.Duration(delay)*time.Millisecond, func() {
			ctx.Schedule(func(ctx *q.Context) {
				value := ctx.NewInt64(id)
				defer value.Free()
				global := ctx.Globals()
				defer global.Free()
				result := global.Call("__fireTimer", value)
				result.Free()
				delete(timers, id)
			})
		})
		return ctx.NewUndefined()
	})
	bind("__clearTimer", func(ctx *q.Context, _ *q.Value, args []*q.Value) *q.Value {
		id := args[0].ToInt64()
		if timer := timers[id]; timer != nil {
			timer.Stop()
			delete(timers, id)
		}
		return ctx.NewUndefined()
	})
	for _, evaluate := range []func() *q.Value{
		func() *q.Value { return js.Eval(hostPrimitives) },
		func() *q.Value { return js.EvalBytecode(bundle.bytecode) },
		func() *q.Value { return js.Eval(bindings) },
	} {
		result := evaluate()
		if result == nil || result.IsException() {
			err := js.Exception()
			if result != nil {
				result.Free()
			}
			ready <- fmt.Errorf("initialize YouTube.js: %w", err)
			return
		}
		result.Free()
	}
	ready <- nil
	lifetime := js.Eval(`new Promise(resolve=>{globalThis.__finishEngine=resolve})`)
	pending := make(map[int64]call)
	var nextCall int64
	closing := false
	finishIfDrained := func() {
		if closing && len(pending) == 0 {
			result := globals.Call("__finishEngine")
			result.Free()
		}
	}
	complete := func(id int64, value reply) {
		request, ok := pending[id]
		if !ok {
			return
		}
		delete(pending, id)
		if request.ctx.Err() != nil {
			value.err = request.ctx.Err()
		}
		request.result <- value
		finishIfDrained()
	}
	jobs.Go(func() error {
		for request := range engine.calls {
			js.Schedule(func(_ *q.Context) {
				operation = request.ctx
				defer func() { operation = initial }()
				nextCall++
				id := nextCall
				pending[id] = request
				raw, err := json.Marshal(request.arguments)
				if err != nil {
					complete(id, reply{err: err})
					return
				}
				globals.Set("__args", js.ParseJSON(string(raw)))
				promise := js.Eval(
					`(async()=>{try{return JSON.stringify({value:await (` + request.expression + `)(__args)})}catch(e){return JSON.stringify({error:{message:String(e.message||e),stack:e.stack,info:e.info,go_error:__goErrorID(e),playability_status:__playability(e.info?.playability_status??(e.info?.status==='ERROR'?e.info:undefined))??undefined}})}})()`,
				)
				if promise == nil || promise.IsException() {
					err := js.Exception()
					if promise != nil {
						promise.Free()
					}
					complete(id, reply{err: fmt.Errorf("YouTube.js execution: %w", err)})
					return
				}
				fulfilled := js.NewFunction(
					func(ctx *q.Context, _ *q.Value, args []*q.Value) *q.Value {
						var response struct {
							Value json.RawMessage `json:"value"`
							Error *struct {
								Message           string             `json:"message"`
								Info              json.RawMessage    `json:"info"`
								GoError           int64              `json:"go_error"`
								PlayabilityStatus *PlayabilityStatus `json:"playability_status"`
							} `json:"error"`
						}
						err := json.Unmarshal([]byte(args[0].ToString()), &response)
						if response.Error != nil {
							err = &Error{
								Message:           response.Error.Message,
								Info:              response.Error.Info,
								PlayabilityStatus: response.Error.PlayabilityStatus,
								Cause:             hostErrors[response.Error.GoError],
							}
							delete(hostErrors, response.Error.GoError)
						}
						complete(id, reply{raw: response.Value, err: err})
						return ctx.NewUndefined()
					},
				)
				defer fulfilled.Free()
				rejected := js.NewFunction(
					func(ctx *q.Context, _ *q.Value, args []*q.Value) *q.Value {
						message := "JavaScript promise rejected"
						if len(args) > 0 {
							message = args[0].ToString()
						}
						complete(id, reply{err: fmt.Errorf("YouTube.js execution: %s", message)})
						return ctx.NewUndefined()
					},
				)
				defer rejected.Free()
				chained := promise.Then(fulfilled, rejected)
				chained.Free()
				promise.Free()
			})
		}
		js.Schedule(func(*q.Context) { closing = true; finishIfDrained() })
		return nil
	})
	result := js.Await(lifetime)
	if result != nil {
		result.Free()
	}
	for id := range pending {
		err := initial.Err()
		if err == nil {
			err = errors.New("YouTube runtime stopped before completing the operation")
		}
		complete(id, reply{err: err})
	}
	_ = jobs.Wait()
}

func (engine *Engine) waitReply(ctx context.Context, request call) reply {
	var result reply
	select {
	case result = <-request.result:
	case <-ctx.Done():
		return reply{err: ctx.Err()}
	case <-engine.done:
		select {
		case result = <-request.result:
		default:
			return reply{err: errors.New("YouTube runtime closed")}
		}
	}
	return result
}
