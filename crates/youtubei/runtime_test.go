package youtubei

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"net/http"
	"slices"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"golang.org/x/sync/errgroup"
)

func noRuntimeError(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

func runtimeEngine(t *testing.T, fetch Fetch) *Engine {
	t.Helper()
	ctx, cancel := context.WithCancel(t.Context())
	if fetch == nil {
		fetch = func(context.Context, FetchRequest) (FetchResponse, error) {
			return FetchResponse{}, errors.New("unexpected provider request")
		}
	}
	engine, err := New(ctx, fetch)
	noRuntimeError(t, err)
	t.Cleanup(func() { cancel(); engine.Close() })
	_, err = engine.Initialize(
		ctx,
		SessionOptions{GenerateSessionLocally: true, FailFast: true, Timezone: "UTC"},
	)
	noRuntimeError(t, err)
	return engine
}

func assertRuntime(t *testing.T, engine *Engine, expression string) {
	t.Helper()
	var valid bool
	noRuntimeError(t, engine.Call(t.Context(), expression, nil, &valid))
	if !valid {
		t.Fatal("upstream runtime contract failed:", expression)
	}
}

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
func TestPublishedBundleCanProbeOptionalNativeModules(t *testing.T) {
	t.Parallel()
	engine := runtimeEngine(t, nil)
	assertRuntime(
		t,
		engine,
		`()=>{const {createRequire}=require('module');const load=createRequire('/');if(typeof load('node:buffer').Buffer!=='function')return false;let rejected=0;for(const name of ['worker_threads','./local.js']){try{load(name)}catch{rejected++}}try{createRequire('relative.js')}catch{rejected++}return rejected===3}`,
	)
}

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
func TestTypedSnapshotsPreserveDatesNanAndUnknownNodes(t *testing.T) {
	t.Parallel()
	engine := runtimeEngine(t, nil)
	var page Playlist
	noRuntimeError(t, engine.Call(t.Context(), `()=>{
      const video=new YouTubeJS.YTNodes.PlaylistVideo({videoId:'abcdefghijk',isPlayable:true,title:{simpleText:'',accessibility:{accessibilityData:{label:''}}},upcomingEventData:{startTime:'1790812800'}});
      const lockup=new YouTubeJS.YTNodes.LockupView({contentId:'abcdefghijk',contentType:'LOCKUP_CONTENT_TYPE_FUTURE_KIND'});
      const message=new YouTubeJS.YTNodes.Message({text:{simpleText:'Description'}});
      return __playlist({info:{},items:[video,lockup,message]});
    }`, nil, &page))
	if len(page.Items) != 3 || page.Items[0].Title != "N/A" ||
		!math.IsNaN(page.Items[0].DurationSeconds) ||
		page.Items[0].Upcoming == nil ||
		*page.Items[0].Upcoming != 1790812800000 ||
		page.Items[1].ContentType != "FUTURE_KIND" ||
		page.Items[2].Type != "Message" {
		t.Fatalf("snapshot lost upstream semantics: %+v", page.Items)
	}
	var format Format
	noRuntimeError(
		t,
		engine.Call(
			t.Context(),
			`raw=>{const f=new YouTubeJS.Misc.Format(raw);return {itag:f.itag,content_length:f.content_length}}`,
			map[string]any{
				"itag":          140,
				"mimeType":      `audio/mp4; codecs="mp4a.40.2"`,
				"bitrate":       128000,
				"contentLength": "NaN",
			},
			&format,
		),
	)
	if format.ContentLength == nil || !math.IsNaN(*format.ContentLength) {
		t.Fatal("non-finite format length was lost")
	}
}

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
const audioFormatJSON = `{"itag":140,"mimeType":"audio/mp4; codecs=\"mp4a.40.2\"","bitrate":128000,"audioQuality":"AUDIO_QUALITY_MEDIUM","contentLength":"12345","approxDurationMs":"1000","lastModified":"0","url":"https://example.googlevideo.com/audio"}`

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
func TestTypedCacheAndPlayerKeepFormatDecipheringInUpstream(t *testing.T) {
	t.Parallel()
	engine := runtimeEngine(t, nil)
	assertRuntime(t, engine, `async()=>{
      const cache=new YouTubeJS.UniversalCache(false);
      const output="const exportedVars={nsigFunction:(url,sp,sig)=>{const values=new URL(url).searchParams;return new class{transform(){values.set('n','decoded_n');values.set(sp,sig.split('').reverse().join(''))}get(k){return values.get(k)}}}};";
      const player=new YouTubeJS.Player('fixture',1234,{output});await player.cache(cache);
      const cached=await YouTubeJS.Player.create(cache,()=>{throw Error('cache miss')},undefined,'fixture');
      const format=new YouTubeJS.Misc.Format({...`+audioFormatJSON+`,url:undefined,signatureCipher:'url=https%3A%2F%2Fexample.googlevideo.com%2Faudio%3Fn%3Doriginal&sp=sig&s=abc'});
      const url=await format.decipher(cached);return cached.signature_timestamp===1234&&url.includes('n=decoded_n')&&url.includes('sig=cba');
    }`)
}

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
func TestInstancesReuseEngineAndKeepSessionAndObjectsAlive(t *testing.T) {
	t.Parallel()
	engine := runtimeEngine(t, nil)
	assertRuntime(t, engine, `async()=>{
      globalThis.retainedSession=__innertube.session;
      const second=new YouTubeJS.Innertube(retainedSession);
      const independent=await YouTubeJS.Innertube.create({generate_session_locally:true,retrieve_player:false,retrieve_innertube_config:false,timezone:'UTC',fetch});
      globalThis.retainedText=new YouTubeJS.Misc.Text({runs:[{text:'one'},{text:' two'}]});
      return second.session===retainedSession&&independent.session!==retainedSession;
    }`)
	assertRuntime(
		t,
		engine,
		`()=>retainedText.toString()==='one two'&&retainedSession.context.client.clientName==='WEB'`,
	)
}

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
func TestNativeCallbacksPropertiesErrorsAndRuntimeBoundaries(t *testing.T) {
	t.Parallel()
	failure := errors.New("fixture failure")
	engine := runtimeEngine(
		t,
		func(context.Context, FetchRequest) (FetchResponse, error) { return FetchResponse{}, failure },
	)
	assertRuntime(
		t,
		engine,
		`()=>{globalThis.retainedObject={text:new YouTubeJS.Misc.Text({simpleText:'retained'})};return retainedObject.text===retainedObject.text&&retainedObject.missing===undefined}`,
	)
	err := engine.Call(
		t.Context(),
		`async()=>await fetch('https://fixture.invalid/error')`,
		nil,
		nil,
	)
	if !errors.Is(err, failure) {
		t.Fatalf("native error lost its cause: %v", err)
	}
	assertRuntime(t, engine, `()=>retainedObject.text.toString()==='retained'`)
	other := runtimeEngine(t, nil)
	if _, err := other.Decipher(t.Context(), Format{Handle: 1, owner: engine}); err == nil {
		t.Fatal("accepted a format from another runtime")
	}
}

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
func TestFetchPreservesBinaryBodiesHeadersStatusAndErrors(t *testing.T) {
	t.Parallel()
	var mu sync.Mutex
	var requests []FetchRequest
	engine := runtimeEngine(
		t,
		func(_ context.Context, request FetchRequest) (FetchResponse, error) {
			mu.Lock()
			requests = append(requests, request)
			mu.Unlock()
			if strings.HasSuffix(request.URL, "/error") {
				return FetchResponse{}, errors.New("transport unavailable")
			}
			if request.Method == http.MethodHead {
				return FetchResponse{Status: 204}, nil
			}
			return FetchResponse{
				Status:  206,
				Headers: [][2]string{{"x-response", "present"}},
				Body:    []byte{255, 128, 0},
			}, nil
		},
	)
	assertRuntime(
		t,
		engine,
		`async()=>{const r=await fetch('https://fixture.invalid/echo',{method:'POST',headers:{'x-request':'present'},body:new Uint8Array([0,128,255])});const bytes=Array.from(new Uint8Array(await r.arrayBuffer()));const empty=await fetch('https://fixture.invalid/empty',{method:'HEAD'});let failed=false;try{await fetch('https://fixture.invalid/error')}catch(e){failed=e.message.includes('transport unavailable')}return r.status===206&&r.headers.get('x-response')==='present'&&bytes.join(',')==='255,128,0'&&empty.status===204&&await empty.text()===''&&failed}`,
	)
	mu.Lock()
	defer mu.Unlock()
	if len(requests) != 3 || requests[0].Method != http.MethodPost ||
		!slices.Equal(requests[0].Body, []byte{0, 128, 255}) ||
		!slices.Contains(requests[0].Headers, [2]string{"x-request", "present"}) ||
		len(requests[1].Body) != 0 {
		t.Fatalf("binary request changed: %+v", requests)
	}
}

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
func TestLibraryNodeChecksAndFormatMethodsPreservePrototypes(t *testing.T) {
	t.Parallel()
	engine := runtimeEngine(t, nil)
	assertRuntime(
		t,
		engine,
		`async()=>{const n=new YouTubeJS.YTNodes.PlaylistVideo({videoId:'abcdefghijk',title:{simpleText:'Episode',accessibility:{accessibilityData:{label:'Episode'}}},isPlayable:true,lengthSeconds:'300',videoInfo:{simpleText:'50K views • 10 years ago'}});const f=new YouTubeJS.Misc.Format(`+audioFormatJSON+`);return n.is(YouTubeJS.YTNodes.PlaylistVideo)&&!n.is(YouTubeJS.YTNodes.LockupView,YouTubeJS.YTNodes.Alert)&&n.video_info.toString()==='50K views • 10 years ago'&&n.duration.seconds===300&&f.itag===140&&f.has_audio&&!f.has_video&&f.content_length===12345&&await f.decipher()==='https://example.googlevideo.com/audio'}`,
	)
}

func responseFetch(t *testing.T, responses []json.RawMessage) (Fetch, *[]FetchRequest) {
	t.Helper()
	var mu sync.Mutex
	requests := new([]FetchRequest)
	return func(_ context.Context, request FetchRequest) (FetchResponse, error) {
		mu.Lock()
		defer mu.Unlock()
		index := len(*requests)
		*requests = append(*requests, request)
		if index >= len(responses) {
			return FetchResponse{}, fmt.Errorf("unexpected request %s", request.URL)
		}
		return FetchResponse{Status: 200, Body: responses[index]}, nil
	}, requests
}

//nolint:cyclop,lll // Verify every continuation and the complete embedded-runtime provider transcript.
func TestActualFlatPlaylistContinuationsUseOnlyBrowseRequests(t *testing.T) {
	t.Parallel()
	fetch, requests := responseFetch(t, []json.RawMessage{
		json.RawMessage(
			`{"metadata":{"playlistMetadataRenderer":{"title":"Interviews","description":"Expert interviews"}},"sidebar":{"playlistSidebarRenderer":{"items":[{"playlistSidebarPrimaryInfoRenderer":{"stats":[{"simpleText":"3 episodes"}]}}]}},"contents":{"playlistVideoListRenderer":{"contents":[{"playlistVideoRenderer":{"videoId":"abcdefghijk","isPlayable":true,"lengthSeconds":"1","title":{"simpleText":"First","accessibility":{"accessibilityData":{"label":"First"}}}}},{"continuationItemRenderer":{"continuationEndpoint":{"commandMetadata":{"webCommandMetadata":{"apiUrl":"/youtubei/v1/browse","sendPost":true}},"continuationCommand":{"token":"next-page","request":"CONTINUATION_REQUEST_TYPE_BROWSE"}}}}]}}}`,
		),
		json.RawMessage(
			`{"onResponseReceivedActions":[{"appendContinuationItemsAction":{"continuationItems":[{"playlistVideoRenderer":{"videoId":"T6juU_4UqKI","isPlayable":false,"lengthSeconds":"1","title":{"simpleText":"Unavailable","accessibility":{"accessibilityData":{"label":"Unavailable"}}}}},{"playlistVideoRenderer":{"videoId":"lmnopqrstuv","isPlayable":true,"lengthSeconds":"1","title":{"simpleText":"Last","accessibility":{"accessibilityData":{"label":"Last"}}}}}]}}]}`,
		),
	})
	engine := runtimeEngine(t, fetch)
	page, err := engine.PlaylistHead(t.Context(), "PLtest")
	noRuntimeError(t, err)
	if page.Title != "Interviews" || !page.HasContinuation || len(page.Items) != 1 {
		t.Fatalf("first page: %+v", page)
	}
	next, err := engine.Continuation(t.Context(), page)
	noRuntimeError(t, err)
	if next.HasContinuation || len(next.Items) != 2 || next.Items[0].ID != "T6juU_4UqKI" ||
		next.Items[0].IsPlayable {
		t.Fatalf("continued page: %+v", next)
	}
	if len(*requests) != 2 {
		t.Fatal("playlist made extra requests")
	}
	for index, request := range *requests {
		if !strings.Contains(request.URL, "/youtubei/v1/browse?") {
			t.Fatal(request.URL)
		}
		var body map[string]any
		noRuntimeError(t, json.Unmarshal(request.Body, &body))
		if index == 0 && body["params"] != "wgYCCAA=" {
			t.Fatal(body)
		}
		if index == 1 && body["continuation"] != "next-page" {
			t.Fatal(body)
		}
	}
}

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
func TestBasicInfoUsesUpstreamClientAndReturnsLiveFormatHandles(t *testing.T) {
	t.Parallel()
	fetch, requests := responseFetch(
		t,
		[]json.RawMessage{
			json.RawMessage(
				`{"videoDetails":{"videoId":"abcdefghijk","title":"Episode","lengthSeconds":"1"},"playabilityStatus":{"status":"OK"},"streamingData":{"expiresInSeconds":"3600","formats":[{"itag":18,"url":"https://example.googlevideo.com/video","mimeType":"video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"","bitrate":100000,"width":640,"height":360,"qualityLabel":"360p","audioQuality":"AUDIO_QUALITY_MEDIUM"}],"adaptiveFormats":[` + audioFormatJSON + `]}}`,
			),
		},
	)
	engine := runtimeEngine(t, fetch)
	info, err := engine.VideoInfo(t.Context(), "abcdefghijk", VisionOS())
	noRuntimeError(t, err)
	if info.BasicInfo.Title != "Episode" || len(info.Formats) != 2 || info.Formats[1].ITag != 140 ||
		info.Formats[0].Height == nil ||
		*info.Formats[0].Height != 360 ||
		!info.Formats[0].HasAudio ||
		!info.Formats[0].HasVideo {
		t.Fatalf("video info: %+v", info)
	}
	target, err := engine.Decipher(t.Context(), info.Formats[0])
	noRuntimeError(t, err)
	if target != "https://example.googlevideo.com/video" {
		t.Fatal(target)
	}
	var body struct {
		Context struct {
			Client struct {
				ClientName string `json:"clientName"`
			} `json:"client"`
		} `json:"context"`
	}
	noRuntimeError(t, json.Unmarshal((*requests)[0].Body, &body))
	if body.Context.Client.ClientName != "VISIONOS" {
		t.Fatal(body.Context.Client.ClientName)
	}
}

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
func TestNativePlatformCacheHashAndPlayerEvaluator(t *testing.T) {
	t.Parallel()
	engine := runtimeEngine(t, nil)
	assertRuntime(
		t,
		engine,
		`async()=>{const cache=new YouTubeJS.UniversalCache(false);const bytes=new Uint8Array([0,1,255]).buffer;await cache.set('key',bytes);const result=Array.from(new Uint8Array(await cache.get('key')));await cache.remove('key');const shim=YouTubeJS.Platform.shim;const value=await shim.eval({output:'return {doubled:n*2};'},{n:21});const player=new YouTubeJS.Player('fixture',1234,{output:''});__innertube.session.player=player;return result.join(',')==='0,1,255'&&(await cache.get('key'))==null&&(await shim.sha1Hash('abc'))==='a9993e364706816aba3e25717850c26c9cd0d89d'&&value.doubled===42&&__innertube.session.player===player}`,
	)
}

func gatedRuntime(t *testing.T) (*Engine, chan string, func(string)) {
	t.Helper()
	admitted := make(chan string, 4)
	gates := map[string]chan struct{}{"1": make(chan struct{}), "2": make(chan struct{})}
	var mu sync.Mutex
	engine := runtimeEngine(
		t,
		func(ctx context.Context, request FetchRequest) (FetchResponse, error) {
			id := request.URL[strings.LastIndex(request.URL, "/")+1:]
			admitted <- id
			select {
			case <-gates[id]:
				return FetchResponse{Status: 200, Body: []byte(id)}, nil
			case <-ctx.Done():
				return FetchResponse{}, ctx.Err()
			}
		},
	)
	release := func(id string) {
		mu.Lock()
		defer mu.Unlock()
		select {
		case <-gates[id]:
		default:
			close(gates[id])
		}
	}
	t.Cleanup(func() { release("1"); release("2") })
	return engine, admitted, release
}

func awaitAdmission(t *testing.T, admitted <-chan string) {
	t.Helper()
	select {
	case <-admitted:
	case <-time.After(2 * time.Second):
		t.Fatal("runtime calls did not overlap at the native fetch boundary")
	}
}

const overlapCall = `async id=>Number(await (await fetch('https://fixture.invalid/'+id)).text())`

func TestAsynchronousCallsOverlapInOneEngine(t *testing.T) {
	t.Parallel()
	engine, admitted, release := gatedRuntime(t)
	var results [2]int
	var group errgroup.Group
	for index := range results {
		group.Go(
			func() error { return engine.Call(t.Context(), overlapCall, index+1, &results[index]) },
		)
	}
	awaitAdmission(t, admitted)
	awaitAdmission(t, admitted)
	release("1")
	release("2")
	noRuntimeError(t, group.Wait())
	if results != [2]int{1, 2} {
		t.Fatal(results)
	}
}

func TestStaggeredCallsSurviveCompletionAndCancellationOfASibling(t *testing.T) {
	t.Parallel()
	for _, cancelSibling := range []bool{false, true} {
		t.Run(strconv.FormatBool(cancelSibling), func(t *testing.T) {
			t.Parallel()
			engine, admitted, release := gatedRuntime(t)
			ctx, cancel := context.WithCancel(t.Context())
			defer cancel()
			first, second := make(chan error, 1), make(chan error, 1)
			var one, two int
			go func() { first <- engine.Call(t.Context(), overlapCall, 1, &one) }()
			go func() { second <- engine.Call(ctx, overlapCall, 2, &two) }()
			awaitAdmission(t, admitted)
			awaitAdmission(t, admitted)
			if cancelSibling {
				cancel()
				if err := <-second; !errors.Is(err, context.Canceled) {
					t.Fatal(err)
				}
			}
			release("1")
			noRuntimeError(t, <-first)
			assertRuntime(t, engine, `()=>true`)
			release("2")
			if !cancelSibling {
				noRuntimeError(t, <-second)
				if two != 2 {
					t.Fatal(two)
				}
			}
			if one != 1 {
				t.Fatal(one)
			}
		})
	}
}

func TestIndependentWorkerThreadsEachOwnAReusableEngine(t *testing.T) {
	t.Parallel()
	var group errgroup.Group
	for range 3 {
		group.Go(func() error {
			ctx, cancel := context.WithCancel(t.Context())
			defer cancel()
			engine, err := New(ctx, func(context.Context, FetchRequest) (FetchResponse, error) {
				return FetchResponse{}, errors.New("unexpected fetch")
			})
			if err != nil {
				return err
			}
			defer engine.Close()
			_, err = engine.Initialize(
				ctx,
				SessionOptions{GenerateSessionLocally: true, Timezone: "UTC"},
			)
			if err != nil {
				return err
			}
			for index := range 3 {
				var value int
				if err := engine.Call(
					ctx,
					`()=>{globalThis.count=(globalThis.count??0)+1;return count}`,
					nil,
					&value,
				); err != nil {
					return err
				}
				if value != index+1 {
					return errors.New("worker state crossed runtime boundaries")
				}
			}
			return nil
		})
	}
	noRuntimeError(t, group.Wait())
}

func TestSharedWorkerOverlapsCallsFromOtherThreadsAndDrainsOnDrop(t *testing.T) {
	t.Parallel()
	engine, admitted, release := gatedRuntime(t)
	var group errgroup.Group
	for id := 1; id <= 2; id++ {
		group.Go(func() error {
			var got int
			err := engine.Call(t.Context(), overlapCall, id, &got)
			if err == nil && got != id {
				return errors.New("response belongs to another caller")
			}
			return err
		})
	}
	awaitAdmission(t, admitted)
	awaitAdmission(t, admitted)
	closed := make(chan struct{})
	go func() { engine.Close(); close(closed) }()
	select {
	case <-closed:
		t.Fatal("worker closed before accepted handlers drained")
	default:
	}
	release("1")
	release("2")
	noRuntimeError(t, group.Wait())
	select {
	case <-closed:
	case <-time.After(2 * time.Second):
		t.Fatal("worker did not finish after accepted calls drained")
	}
	if err := engine.Call(t.Context(), `()=>true`, nil, nil); err == nil {
		t.Fatal("worker accepted work after close")
	}
}

func TestCancellationInterruptsJavascriptExecution(t *testing.T) {
	t.Parallel()
	entered := make(chan struct{})
	engine := runtimeEngine(t, func(context.Context, FetchRequest) (FetchResponse, error) {
		close(entered)
		return FetchResponse{Status: 200}, nil
	})
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	done := make(chan error, 1)
	go func() {
		done <- engine.Call(ctx, `()=>{fetch('https://fixture.invalid/entered');while(true){}}`, nil, nil)
	}()
	select {
	case <-entered:
	case <-time.After(2 * time.Second):
		t.Fatal("JavaScript execution did not start")
	}
	cancel()
	select {
	case err := <-done:
		if !errors.Is(err, context.Canceled) {
			t.Fatal(err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("cancellation did not interrupt JavaScript")
	}
	assertRuntime(t, engine, `()=>true`)
}

// A call sent while another call's JavaScript keeps fetching waits for it: it
// never runs inside the first call, so cancelling the first still interrupts
// it and the second then completes.
func TestCancellationInterruptsAFetchingCallBeforeTheNextCall(t *testing.T) {
	t.Parallel()
	const pollsAfterSecondCall = 16
	entered, advanced, nested := make(chan struct{}), make(chan struct{}), make(chan struct{})
	var enter, advance, nest sync.Once
	var polls, target atomic.Int64
	target.Store(math.MaxInt64)
	engine := runtimeEngine(t, func(_ context.Context, request FetchRequest) (FetchResponse, error) {
		if strings.HasSuffix(request.URL, "/nested") {
			nest.Do(func() { close(nested) })
			return FetchResponse{Status: 200}, nil
		}
		enter.Do(func() { close(entered) })
		if polls.Add(1) >= target.Load() {
			advance.Do(func() { close(advanced) })
		}
		return FetchResponse{Status: 200}, nil
	})
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	first := make(chan error, 1)
	go func() {
		first <- engine.Call(
			ctx,
			`()=>{for(let i=0;;i++){if(i%10000===0)fetch('https://fixture.invalid/poll')}}`,
			nil,
			nil,
		)
	}()
	awaitClosed(t, entered, "first call did not start")
	second := make(chan error, 1)
	var valid bool
	target.Store(polls.Load() + pollsAfterSecondCall)
	go func() {
		second <- engine.Call(t.Context(), `()=>{fetch('https://fixture.invalid/nested');return true}`, nil, &valid)
	}()
	select {
	case <-advanced:
	case <-nested:
		t.Fatal("second call ran inside the first")
	case <-time.After(2 * time.Second):
		t.Fatal("first call stopped fetching")
	}
	cancel()
	if err := awaitError(t, first, "first call did not end"); !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
	noRuntimeError(t, awaitError(t, second, "second call did not complete after the first was cancelled"))
	if !valid {
		t.Fatal("second call returned the wrong value")
	}
}

// awaitClosed and awaitError wait for one test step under its guard.
func awaitClosed(t *testing.T, signal <-chan struct{}, failure string) {
	t.Helper()
	select {
	case <-signal:
	case <-time.After(2 * time.Second):
		t.Fatal(failure)
	}
}

func awaitError(t *testing.T, result <-chan error, failure string) error {
	t.Helper()
	select {
	case err := <-result:
		return err
	case <-time.After(2 * time.Second):
		t.Fatal(failure)
		return nil
	}
}
