//! Application-level connection admission. TCP owns congestion control; we probe
//! whether another episode actually adds useful throughput. Both directions can
//! run together, but only one probes at a time so they do not chase each other.

pub const INITIAL_DOWNLOADS: usize = 2;
pub const INITIAL_UPLOADS: usize = 2;
pub const MAX_DOWNLOADS: usize = 16;
pub const MAX_UPLOADS: usize = 8;
// Preserve the existing grace periods with one-second throughput samples.
const STALLED_SAMPLES: usize = 10;
const PROBE_SAMPLES: usize = 15;

#[derive(Clone, Copy)]
pub(super) struct Observation {
    pub rate: f64,
    pub active: usize,
    pub saturated: bool,
    pub stable: bool,
    pub failed: bool,
}

struct Direction {
    limit: usize,
    maximum: usize,
    baseline: Option<(f64, usize)>,
    cooldown: usize,
    stalled: usize,
}

struct Probe {
    direction: usize,
    previous_limit: usize,
    baseline: [Observation; 2],
    windows: usize,
}

pub(super) struct NetworkLimits {
    directions: [Direction; 2],
    probe: Option<Probe>,
    next: usize,
}

impl Default for NetworkLimits {
    fn default() -> Self {
        Self {
            directions: [
                Direction::new(INITIAL_DOWNLOADS, MAX_DOWNLOADS),
                Direction::new(INITIAL_UPLOADS, MAX_UPLOADS),
            ],
            probe: None,
            next: 0,
        }
    }
}

impl Direction {
    fn new(limit: usize, maximum: usize) -> Self {
        Self {
            limit,
            maximum,
            baseline: None,
            cooldown: 0,
            stalled: 0,
        }
    }

    fn back_off(&mut self) {
        self.limit = (self.limit / 2).max(1);
        self.baseline = None;
        self.cooldown = 3;
        self.stalled = 0;
    }
}

impl NetworkLimits {
    pub fn limits(&self) -> [usize; 2] {
        self.directions.each_ref().map(|direction| direction.limit)
    }

    pub fn sample(&mut self, observations: [Observation; 2]) {
        // Resolve a probe before ordinary congestion feedback: a harmful upload
        // probe must be rolled back, rather than shrinking the download budget.
        if let Some(mut probe) = self.probe.take() {
            probe.windows += 1;
            let own = observations[probe.direction];
            let peer = 1 - probe.direction;
            let comparable = observations.iter().all(|sample| sample.stable)
                && own.active > probe.baseline[probe.direction].active
                && observations[peer].active == probe.baseline[peer].active;
            let no_demand = !own.saturated || own.active == 0;
            let failed = observations.iter().any(|sample| sample.failed);
            if comparable || no_demand || failed || probe.windows >= PROBE_SAMPLES {
                // Multiplying relative rates is equivalent to adding log-rate
                // utilities (proportional fairness). Raw byte totals would let a
                // fast downlink hide a severe regression on a slow uplink.
                let gain = observations.iter().zip(probe.baseline).try_fold(
                    1.,
                    |gain, (current, previous)| {
                        if previous.active == 0 || previous.rate == 0. {
                            Some(gain)
                        } else if previous.rate > 0. && current.rate > 0. {
                            Some(gain * current.rate / previous.rate)
                        } else {
                            None
                        }
                    },
                );
                if !comparable || no_demand || failed || gain.is_none_or(|gain| gain < 1.05) {
                    self.directions[probe.direction].limit = probe.previous_limit;
                    self.directions[probe.direction].cooldown = 3;
                }
                // A changed population is not an independent congestion sample.
                for direction in &mut self.directions {
                    direction.baseline = None;
                    direction.stalled = 0;
                }
                for (direction, sample) in self.directions.iter_mut().zip(observations) {
                    if sample.failed {
                        direction.back_off();
                    }
                }
            } else {
                self.probe = Some(probe);
            }
            return;
        }

        let mut backed_off = false;
        for (direction, sample) in self.directions.iter_mut().zip(observations) {
            if sample.failed {
                direction.back_off();
                backed_off = true;
                continue;
            }
            if !sample.saturated || sample.active == 0 || !sample.stable {
                direction.baseline = None;
                direction.stalled = 0;
                continue;
            }
            direction.stalled = if sample.rate == 0. {
                direction.stalled + 1
            } else {
                0
            };
            let slowdown = direction.baseline.is_some_and(|(rate, active)| {
                active == sample.active && sample.rate > 0. && sample.rate < rate * 0.65
            });
            if direction.stalled >= STALLED_SAMPLES || slowdown {
                direction.back_off();
                backed_off = true;
                continue;
            }
            direction.baseline = Some((sample.rate, sample.active));
            direction.cooldown = direction.cooldown.saturating_sub(1);
        }
        if backed_off {
            return;
        }
        for index in [self.next, 1 - self.next] {
            let sample = observations[index];
            let direction = &mut self.directions[index];
            if sample.saturated
                && sample.stable
                && sample.rate > 0.
                && direction.cooldown == 0
                && direction.limit < direction.maximum
                && observations.iter().all(|sample| sample.stable)
            {
                self.probe = Some(Probe {
                    direction: index,
                    previous_limit: direction.limit,
                    baseline: observations,
                    windows: 0,
                });
                direction.limit += 1;
                self.next = 1 - index;
                break;
            }
        }
    }
}
