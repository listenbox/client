package main

import "time"

const (
	doublePress = time.Second
	quitHold    = 2 * time.Second
	quitNotice  = 2 * time.Second
	quitFade    = 200 * time.Millisecond
)

// A shortcut confirms on release, so the key-up cannot reach the next app.
type quitGuard struct {
	lastPress, heldSince time.Time
	confirmed            bool
}

func (q *quitGuard) press(now time.Time, repeat bool) {
	if repeat {
		return
	}
	q.confirmed = !q.lastPress.IsZero() && now.Sub(q.lastPress) < doublePress
	q.lastPress, q.heldSince = now, now
}

func (q *quitGuard) holding(now time.Time) bool {
	q.confirmed = q.confirmed || (!q.heldSince.IsZero() && now.Sub(q.heldSince) >= quitHold)
	return q.confirmed
}

func (q *quitGuard) release() bool {
	if q.heldSince.IsZero() {
		return false
	}
	confirmed := q.confirmed
	q.heldSince, q.confirmed = time.Time{}, false
	return confirmed
}
