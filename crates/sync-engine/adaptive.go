package syncengine

type observation struct {
	rate                      float64
	active                    int
	saturated, stable, failed bool
}
type direction struct {
	limit, maximum, cooldown, stalled int
	baseline                          *observation
}
type networkProbe struct {
	direction, previousLimit, windows int
	baseline                          [2]observation
}
type networkLimits struct {
	directions [2]direction
	probe      *networkProbe
	next       int
}

const (
	initialNetworkLimit  = 2
	maximumDownloadLimit = 16
	maximumUploadLimit   = 8
	networkBackoffFactor = 2
)

func newNetworkLimits() networkLimits {
	return networkLimits{
		directions: [2]direction{
			{limit: initialNetworkLimit, maximum: maximumDownloadLimit},
			{limit: initialNetworkLimit, maximum: maximumUploadLimit},
		},
	}
}

func (d *direction) backOff() {
	d.limit = max(1, d.limit/networkBackoffFactor)
	d.baseline = nil
	d.cooldown = 3
	d.stalled = 0
}

func (n *networkLimits) sample(observations [2]observation) {
	if probe := n.probe; probe != nil {
		n.sampleProbe(probe, observations)
		return
	}
	backedOff := false
	for i, sample := range observations {
		if n.directions[i].sample(sample) {
			backedOff = true
		}
	}
	if backedOff {
		return
	}
	n.startProbe(observations)
}

func (n *networkLimits) sampleProbe(probe *networkProbe, observations [2]observation) {
	n.probe = nil
	probe.windows++
	own := observations[probe.direction]
	comparableSamples := probe.comparable(observations)
	noDemand := !own.saturated || own.active == 0
	failed := observations[0].failed || observations[1].failed
	if !comparableSamples && !noDemand && !failed && probe.windows < 15 {
		n.probe = probe
		return
	}
	gain := probeGain(probe, observations)
	if !comparableSamples || noDemand || failed || gain < 1.05 {
		n.directions[probe.direction].limit = probe.previousLimit
		n.directions[probe.direction].cooldown = 3
	}
	n.finishProbe(observations)
}

func (d *direction) sample(sample observation) bool {
	if sample.failed {
		d.backOff()
		return true
	}
	if !sample.saturated || sample.active == 0 || !sample.stable {
		d.baseline = nil
		d.stalled = 0
		return false
	}
	if sample.rate == 0 {
		d.stalled++
	} else {
		d.stalled = 0
	}
	slowdown := d.baseline != nil && d.baseline.active == sample.active && sample.rate > 0 &&
		sample.rate < d.baseline.rate*.65
	if d.stalled >= 10 || slowdown {
		d.backOff()
		return true
	}
	baseline := sample
	d.baseline = &baseline
	d.cooldown = max(0, d.cooldown-1)
	return false
}

func (n *networkLimits) startProbe(observations [2]observation) {
	for _, i := range []int{n.next, 1 - n.next} {
		sample := observations[i]
		d := &n.directions[i]
		if sample.saturated && sample.stable && sample.rate > 0 && d.cooldown == 0 &&
			d.limit < d.maximum &&
			observations[0].stable &&
			observations[1].stable {
			n.probe = &networkProbe{direction: i, previousLimit: d.limit, baseline: observations}
			d.limit++
			n.next = 1 - i
			break
		}
	}
}

func probeGain(probe *networkProbe, observations [2]observation) float64 {
	gain := 1.0
	for i, current := range observations {
		previous := probe.baseline[i]
		if previous.active == 0 || previous.rate == 0 {
			continue
		}
		if current.rate > 0 {
			gain *= current.rate / previous.rate
		} else {
			gain = 0
		}
	}
	return gain
}

func (probe *networkProbe) comparable(observations [2]observation) bool {
	own := observations[probe.direction]
	peer := 1 - probe.direction
	return observations[0].stable && observations[1].stable &&
		own.active > probe.baseline[probe.direction].active &&
		observations[peer].active == probe.baseline[peer].active
}

func (n *networkLimits) finishProbe(observations [2]observation) {
	for i := range n.directions {
		n.directions[i].baseline = nil
		n.directions[i].stalled = 0
		if observations[i].failed {
			n.directions[i].backOff()
		}
	}
}
