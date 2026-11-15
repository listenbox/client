package syncengine

import (
	"context"
	"fmt"
	"slices"
	"sync"
	"time"

	"github.com/jonboulle/clockwork"
)

type Phase struct{ value string }

const (
	phaseQueued         = "queued"
	phaseDownloading    = "downloading"
	phasePreparing      = "preparing"
	phaseUploading      = "uploading"
	phasePublishing     = "publishing"
	phaseRetrying       = "retrying"
	phaseComplete       = "complete"
	phaseSkipped        = "skipped"
	phaseFailed         = "failed"
	phaseResolving      = "resolving"
	phaseWaitingPrepare = "waiting_to_prepare"
	phaseWaitingUpload  = "waiting_to_upload"
)

const (
	mediaStage        = 2
	networkStageCount = 2
)

func Queued() Phase           { return Phase{phaseQueued} }
func Downloading() Phase      { return Phase{phaseDownloading} }
func Preparing() Phase        { return Phase{phasePreparing} }
func Uploading() Phase        { return Phase{phaseUploading} }
func Publishing() Phase       { return Phase{phasePublishing} }
func Retrying() Phase         { return Phase{phaseRetrying} }
func Complete() Phase         { return Phase{phaseComplete} }
func Skipped() Phase          { return Phase{phaseSkipped} }
func Failed() Phase           { return Phase{phaseFailed} }
func Resolving() Phase        { return Phase{phaseResolving} }
func WaitingToPrepare() Phase { return Phase{phaseWaitingPrepare} }
func WaitingToUpload() Phase  { return Phase{phaseWaitingUpload} }
func NewPhaseFromValue(value string) (Phase, error) {
	if !slices.Contains(
		[]string{
			phaseQueued,
			phaseDownloading,
			phasePreparing,
			phaseUploading,
			phasePublishing,
			phaseRetrying,
			phaseComplete,
			phaseSkipped,
			phaseFailed,
			phaseResolving,
			phaseWaitingPrepare,
			phaseWaitingUpload,
		},
		value,
	) {
		return Phase{}, fmt.Errorf("invalid phase %q", value)
	}
	return Phase{value}, nil
}
func (p Phase) String() string { return p.value }
func (p Phase) Active() bool {
	return p.value != "" && p != Complete() && p != Skipped() && p != Failed() && p != Queued()
}

type Download struct {
	ID, SourceID, SourceTitle, SourceURL, Title string
	DurationSeconds                             *int64
	Position                                    *int64
	Phase                                       Phase
	Reason, Error                               string
	Attempt                                     int
	Received, Total, Uploaded, UploadTotal      int64
	Started                                     time.Time
	Ranges                                      []ByteRange
	rate                                        trafficRate
}

type trafficRate struct {
	since, lastProgress time.Time
	bytes, perSecond    int64
}

func (r *trafficRate) record(bytes int64, now time.Time) {
	if r.since.IsZero() {
		r.since = now
	}
	r.bytes += bytes
	if bytes > 0 {
		r.lastProgress = now
	}
	if elapsed := now.Sub(r.since); elapsed >= time.Second {
		r.perSecond = int64(float64(r.bytes) / elapsed.Seconds())
		r.bytes = 0
		r.since = now
	}
}

func (r *trafficRate) current(now time.Time) int64 {
	if now.Sub(r.lastProgress) >= 3*time.Second {
		return 0
	}
	return r.perSecond
}
func (d Download) BytesPerSecond(now time.Time) int64 { return d.rate.current(now) }

type ByteRange struct {
	Start, End, Received int64
	Complete             bool
}
type DownloadManager struct {
	mu                        sync.Mutex
	items                     map[string]Download
	Changed                   func()
	changed                   chan struct{}
	adaptive                  networkLimits
	active, waiting           [3]int
	bytes                     [2]int64
	failed, populationChanged [2]bool
	sampledAt                 time.Time
	clock                     clockwork.Clock
}

func NewDownloadManager() *DownloadManager { return newDownloadManager(clockwork.NewRealClock()) }
func newDownloadManager(clock clockwork.Clock) *DownloadManager {
	return &DownloadManager{
		items:     make(map[string]Download),
		changed:   make(chan struct{}),
		adaptive:  newNetworkLimits(),
		clock:     clock,
		sampledAt: clock.Now(),
	}
}

func (m *DownloadManager) Changes() <-chan struct{} {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.changed
}

func (m *DownloadManager) Snapshot() []Download {
	m.mu.Lock()
	defer m.mu.Unlock()
	items := make([]Download, 0, len(m.items))
	for _, item := range m.items {
		item.Ranges = slices.Clone(item.Ranges)
		items = append(items, item)
	}
	slices.SortFunc(items, func(a, b Download) int {
		if a.SourceID != b.SourceID {
			return compare(a.SourceID, b.SourceID)
		}
		if a.Position != nil && b.Position != nil {
			return int(*a.Position - *b.Position)
		}
		return compare(a.SourceURL, b.SourceURL)
	})
	return items
}

func (m *DownloadManager) Clear() { m.mu.Lock(); m.items = make(map[string]Download); m.mu.Unlock() }

func (m *DownloadManager) Limits() (int, int) {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.adaptive.directions[0].limit, m.adaptive.directions[1].limit
}

func (m *DownloadManager) signalLocked() { close(m.changed); m.changed = make(chan struct{}) }

func compare(a, b string) int {
	if a < b {
		return -1
	}
	if a > b {
		return 1
	}
	return 0
}

func (m *DownloadManager) update(id string, change func(*Download)) {
	m.mu.Lock()
	item := m.items[id]
	change(&item)
	m.items[id] = item
	m.sampleLocked()
	m.signalLocked()
	notify := m.Changed
	m.mu.Unlock()
	if notify != nil {
		notify()
	}
}

func (m *DownloadManager) enqueue(
	show, title string,
	videos []Video,
	existing map[string]bool,
) []string {
	var additions []string
	for i, video := range videos {
		id := show + "/" + video.ID
		position := int64(i)
		phase := Queued()
		if existing[video.URL()] {
			phase = Complete()
		} else {
			additions = append(additions, id)
		}
		m.update(id, func(item *Download) {
			now := m.clock.Now()
			*item = Download{
				ID:              id,
				SourceID:        show,
				SourceTitle:     title,
				SourceURL:       video.URL(),
				Title:           video.Title,
				DurationSeconds: video.DurationSeconds,
				Position:        &position,
				Phase:           phase,
				Started:         now,
				rate:            trafficRate{since: now, lastProgress: now},
			}
		})
	}
	return additions
}

func (m *DownloadManager) rangeProgress(id string, start, received int64, complete, network bool) {
	m.mu.Lock()
	item := m.items[id]
	for i, r := range item.Ranges {
		if r.Start == start {
			received = min(received, r.End-r.Start+1)
			if network {
				m.bytes[0] += max(0, received-r.Received)
				item.rate.record(max(0, received-r.Received), m.clock.Now())
			}
			item.Received += received - r.Received
			item.Ranges[i].Received = received
			item.Ranges[i].Complete = complete
			break
		}
	}
	m.items[id] = item
	m.sampleLocked()
	m.signalLocked()
	notify := m.Changed
	m.mu.Unlock()
	if notify != nil {
		notify()
	}
}

func (m *DownloadManager) uploadProgress(id string, count int64) {
	m.mu.Lock()
	item := m.items[id]
	item.Uploaded = min(item.UploadTotal, item.Uploaded+count)
	item.rate.record(count, m.clock.Now())
	m.items[id] = item
	m.bytes[1] += count
	m.sampleLocked()
	m.signalLocked()
	notify := m.Changed
	m.mu.Unlock()
	if notify != nil {
		notify()
	}
}

func (m *DownloadManager) sampleLocked() {
	now := m.clock.Now()
	elapsed := now.Sub(m.sampledAt)
	if elapsed < time.Second {
		return
	}
	var observations [2]observation
	for i := range observations {
		observations[i] = observation{
			rate:      float64(m.bytes[i]) / elapsed.Seconds(),
			active:    m.active[i],
			saturated: m.active[i] >= m.adaptive.directions[i].limit && m.waiting[i] > 0,
			stable:    !m.populationChanged[i],
			failed:    m.failed[i],
		}
	}
	m.adaptive.sample(observations)
	m.bytes = [2]int64{}
	m.failed = [2]bool{}
	m.populationChanged = [2]bool{}
	m.sampledAt = now
}

func (m *DownloadManager) sampleLoop(ctx context.Context) {
	ticker := m.clock.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.Chan():
			m.mu.Lock()
			m.sampleLocked()
			m.signalLocked()
			m.mu.Unlock()
		}
	}
}

type Stage struct{ index int }

func DownloadStage() Stage { return Stage{0} }
func UploadStage() Stage   { return Stage{1} }
func PrepareStage() Stage  { return Stage{2} }

func (m *DownloadManager) acquireStage(
	ctx context.Context,
	id string,
	stage Stage,
) (func(error), error) {
	index := stage.index
	m.mu.Lock()
	m.waiting[index]++
	waitingPhase := Queued()
	activePhase := Resolving()
	if index == 1 {
		waitingPhase, activePhase = WaitingToUpload(), Uploading()
	}
	if index == mediaStage {
		waitingPhase, activePhase = WaitingToPrepare(), Preparing()
	}
	item := m.items[id]
	item.Phase = waitingPhase
	m.items[id] = item
	m.signalLocked()
	for {
		if ctx.Err() != nil {
			m.waiting[index]--
			m.signalLocked()
			m.mu.Unlock()
			return nil, ctx.Err()
		}
		limit := 2
		if index < networkStageCount {
			limit = m.adaptive.directions[index].limit
		}
		if m.active[index] < limit {
			m.waiting[index]--
			m.active[index]++
			if index < networkStageCount {
				m.populationChanged[index] = true
			}
			item := m.items[id]
			item.Phase = activePhase
			m.items[id] = item
			m.signalLocked()
			m.mu.Unlock()
			return m.stageRelease(ctx, index), nil
		}
		changed := m.changed
		m.mu.Unlock()
		select {
		case <-ctx.Done():
		case <-changed:
		}
		m.mu.Lock()
	}
}

func (m *DownloadManager) stageRelease(ctx context.Context, index int) func(error) {
	var once sync.Once
	return func(err error) {
		once.Do(func() {
			m.mu.Lock()
			m.active[index]--
			if index < networkStageCount {
				m.populationChanged[index] = true
				if err != nil && ctx.Err() == nil && retryable(err) {
					m.failed[index] = true
				}
			}
			m.sampleLocked()
			m.signalLocked()
			m.mu.Unlock()
		})
	}
}
