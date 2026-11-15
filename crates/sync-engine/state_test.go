package syncengine

import (
	"bufio"
	"context"
	"database/sql"
	_ "embed"
	"fmt"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/gofrs/flock"
	"github.com/jonboulle/clockwork"
	"github.com/tailscale/sqlite"
	"golang.org/x/sync/errgroup"
)

func requireNoError(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

func TestMissingUserConfigUsesProductionDefaults(t *testing.T) {
	t.Parallel()
	config, err := LoadConfigIn("", t.TempDir())
	requireNoError(t, err)
	if config.APIOrigin != "https://v1.listenbox.app" ||
		config.DashboardOrigin != "https://web.listenbox.app" {
		t.Fatalf("unexpected defaults: %+v", config)
	}
}

func TestGeneratedConfigDecodesYamlAndNormalizesOrigins(t *testing.T) {
	t.Parallel()
	directory := t.TempDir()
	requireNoError(
		t,
		os.WriteFile(
			filepath.Join(directory, "config.yaml"),
			[]byte(
				"api_origin: http://localhost:8080/\ndashboard_origin: https://web.listenbox.app/\n",
			),
			0o600,
		),
	)
	config, err := LoadConfigIn("", directory)
	requireNoError(t, err)
	if config.APIOrigin != "http://localhost:8080" ||
		config.DashboardOrigin != "https://web.listenbox.app" {
		t.Fatalf("origins not normalized: %+v", config)
	}
	for _, raw := range []string{
		"api_origin: [one, two]",
		"api_origin: https://example.com/path\ndashboard_origin: https://web.example.com",
		"api_origin: https://example.com\nunknown: true",
		"api_origin: https://example.com:99999\ndashboard_origin: https://example.com",
	} {
		requireNoError(t, os.WriteFile(filepath.Join(directory, "config.yaml"), []byte(raw), 0o600))
		if _, err := LoadConfigIn("", directory); err == nil {
			t.Fatalf("accepted invalid config %q", raw)
		}
	}
}

func cookieExport(value string) string {
	return "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\t" + value + "\n"
}

func cookieURL() *url.URL { u, _ := url.Parse("https://www.youtube.com/youtubei/v1/player"); return u }

func cookieHeader(t *testing.T, jar *CookieJar) string {
	t.Helper()
	snapshot, err := jar.snapshot()
	requireNoError(t, err)
	return snapshot.header(cookieURL())
}

func TestValidatesExportsWithoutDisclosingValuesOrReplacingGoodState(t *testing.T) {
	t.Parallel()
	jar := NewCookieJar(t.TempDir())
	ctx := t.Context()
	requireNoError(
		t,
		jar.Import(ctx, strings.ReplaceAll(cookieExport("private-value"), "\n", "\r\n")),
	)
	for _, input := range []string{
		`[{"value":"private-value"}]`,
		cookieExport("private-value; other=oops"),
		cookieExport("private-value\rinjected"),
		strings.ReplaceAll(cookieExport("private-value"), netscapeTrue, "maybe"),
		strings.Repeat("x", maxCookieBytes+1),
	} {
		err := jar.Import(ctx, input)
		if err == nil || strings.Contains(err.Error(), "private-value") {
			t.Fatalf("invalid export disclosed a value or succeeded: %v", err)
		}
		if got := cookieHeader(t, jar); got != "SAPISID=private-value" {
			t.Fatalf("replaced committed cookies: %s", got)
		}
	}
}

func TestRespectsHostPathSecureExpiryAndHttpOnly(t *testing.T) {
	t.Parallel()
	jar := NewCookieJar(t.TempDir())
	requireNoError(
		t,
		jar.Import(
			t.Context(),
			"# HTTP Cookie File\n#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tsecure\nwww.y"+
				"outube.com\tFALSE\t/youtubei\tFALSE\t0\tHOST\tonly\n.youtube.com\tTRUE\t/\tTRUE"+
				"\t1\tOLD\texpired\n.example.com\tTRUE\t/\tTRUE\t0\tOTHER\tignored,elsewhere\n",
		),
	)
	snapshot, err := jar.snapshot()
	requireNoError(t, err)
	for raw, want := range map[string]string{
		cookieURL().String():                "HOST=only; SID=secure",
		"https://m.youtube.com/":            "SID=secure",
		"https://www.youtube.com/youtubeix": "SID=secure",
		"http://m.youtube.com/":             "",
		"https://youtube.com.example.com/":  "",
		"https://media.googlevideo.com/":    "",
	} {
		u, _ := url.Parse(raw)
		if got := snapshot.header(u); got != want {
			t.Fatalf("cookie scope %s: %q, want %q", raw, got, want)
		}
	}
}

func TestMergesConcurrentUpdatesWithoutStaleOverwritesOrResurrection(t *testing.T) {
	t.Parallel()
	jar := NewCookieJar(t.TempDir())
	ctx := t.Context()
	requireNoError(t, jar.Import(ctx, cookieExport("seed")))
	initial, err := jar.snapshot()
	requireNoError(t, err)
	gate := make(chan struct{})
	var group errgroup.Group
	for _, header := range []string{
		"FIRST=one; Domain=youtube.com; Path=/; Secure",
		"SECOND=two; Domain=youtube.com; Path=/; Secure",
	} {
		group.Go(
			func() error { <-gate; return jar.update(ctx, initial, cookieURL(), []string{header}) },
		)
	}
	close(gate)
	requireNoError(t, group.Wait())
	both := cookieHeader(t, jar)
	if !strings.Contains(both, "FIRST=one") || !strings.Contains(both, "SECOND=two") {
		t.Fatal(both)
	}
	update := func(base cookieSnapshot, header string) {
		requireNoError(
			t,
			jar.update(
				ctx,
				base,
				cookieURL(),
				[]string{header + "; Domain=youtube.com; Path=/; Secure"},
			),
		)
	}
	update(initial, "SAPISID=new")
	update(initial, "SAPISID=stale")
	if !strings.Contains(cookieHeader(t, jar), "SAPISID=new") {
		t.Fatal("stale writer replaced a new revision")
	}
	beforeDelete, err := jar.snapshot()
	requireNoError(t, err)
	update(beforeDelete, "SAPISID=; Max-Age=0")
	update(beforeDelete, "SAPISID=resurrected")
	if strings.Contains(cookieHeader(t, jar), "SAPISID=") {
		t.Fatal("stale writer resurrected a removed cookie")
	}
	requireNoError(t, jar.Import(ctx, cookieExport("replacement")))
	update(initial, "SAPISID=late")
	if got := cookieHeader(t, jar); got != "SAPISID=replacement" {
		t.Fatal(got)
	}
	beforeRemove, err := jar.snapshot()
	requireNoError(t, err)
	requireNoError(t, jar.Remove(ctx))
	update(beforeRemove, "SAPISID=late")
	enabled, err := jar.Enabled()
	requireNoError(t, err)
	if enabled {
		t.Fatal("late writer enabled a removed session")
	}
}

func TestReadersDoNotWaitForWriterAndIgnoreUncommittedTemporaryFiles(t *testing.T) {
	t.Parallel()
	directory := t.TempDir()
	jar := NewCookieJar(directory)
	requireNoError(t, jar.Import(t.Context(), cookieExport("committed")))
	lock := flock.New(filepath.Join(directory, "youtube-cookies", "write.lock"))
	requireNoError(t, lock.Lock())
	defer lock.Close()
	requireNoError(
		t,
		os.WriteFile(
			filepath.Join(directory, "youtube-cookies", "incomplete.tmp"),
			[]byte("{"),
			0o600,
		),
	)
	result := make(chan string, 1)
	go func() {
		snapshot, err := jar.snapshot()
		if err != nil {
			result <- err.Error()
			return
		}
		result <- snapshot.header(cookieURL())
	}()
	select {
	case got := <-result:
		if got != "SAPISID=committed" {
			t.Fatal(got)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("reader blocked on the writer lock")
	}
}

func TestRejectsForeignResponseCookiesAndPersistsRotationAcrossReopen(t *testing.T) {
	t.Parallel()
	directory := t.TempDir()
	jar := NewCookieJar(directory)
	requireNoError(t, jar.Import(t.Context(), cookieExport("seed")))
	base, err := jar.snapshot()
	requireNoError(t, err)
	requireNoError(
		t,
		jar.update(
			t.Context(),
			base,
			cookieURL(),
			[]string{
				"SAPISID=rotated; Domain=youtube.com; Path=/; Secure; Max-Age=3600",
				"FOREIGN=no; Domain=com; Path=/",
				"OTHER=no; Domain=example.com; Path=/",
			},
		),
	)
	if got := cookieHeader(t, NewCookieJar(directory)); got != "SAPISID=rotated" {
		t.Fatal(got)
	}
}

func TestProcessWriter(t *testing.T) {
	t.Parallel()
	directory := os.Getenv("LISTENBOX_COOKIE_TEST_PROFILE")
	if directory == "" {
		t.Skip("child process for the committed-revision test")
	}
	jar := NewCookieJar(directory)
	base, err := jar.snapshot()
	requireNoError(t, err)
	fmt.Println("COOKIE_WRITER_READY")
	line, err := bufio.NewReader(os.Stdin).ReadString('\n')
	requireNoError(t, err)
	if strings.TrimSpace(line) != "commit" {
		t.Fatal("missing commit gate")
	}
	requireNoError(
		t,
		jar.update(
			t.Context(),
			base,
			cookieURL(),
			[]string{
				"SAPISID=stale-process; Domain=youtube.com; Path=/; Secure",
				"PROCESS=merged; Domain=youtube.com; Path=/; Secure",
			},
		),
	)
}

func TestProcessUpdatesCompareAgainstCommittedRevisions(t *testing.T) {
	t.Parallel()
	directory := t.TempDir()
	jar := NewCookieJar(directory)
	requireNoError(t, jar.Import(t.Context(), cookieExport("seed")))
	executable, err := os.Executable()
	requireNoError(t, err)
	ctx, cancel := context.WithTimeout(t.Context(), 3*time.Second)
	defer cancel()
	child := exec.CommandContext(
		ctx,
		executable,
		"-test.run=^TestProcessWriter$",
		"-test.timeout=3s",
	)
	child.Env = append(os.Environ(), "LISTENBOX_COOKIE_TEST_PROFILE="+directory)
	stdout, err := child.StdoutPipe()
	requireNoError(t, err)
	stdin, err := child.StdinPipe()
	requireNoError(t, err)
	child.Stderr = os.Stderr
	requireNoError(t, child.Start())
	ready := make(chan struct{})
	go func() {
		scanner := bufio.NewScanner(stdout)
		for scanner.Scan() {
			if scanner.Text() == "COOKIE_WRITER_READY" {
				close(ready)
			}
		}
	}()
	select {
	case <-ready:
	case <-ctx.Done():
		t.Fatal("child did not establish a cookie snapshot")
	}
	base, err := jar.snapshot()
	requireNoError(t, err)
	requireNoError(
		t,
		jar.update(
			t.Context(),
			base,
			cookieURL(),
			[]string{"SAPISID=new-process; Domain=youtube.com; Path=/; Secure"},
		),
	)
	_, err = fmt.Fprintln(stdin, "commit")
	requireNoError(t, err)
	stdin.Close()
	requireNoError(t, child.Wait())
	header := cookieHeader(t, jar)
	if !strings.Contains(header, "SAPISID=new-process") ||
		!strings.Contains(header, "PROCESS=merged") ||
		strings.Contains(header, "stale-process") {
		t.Fatal(header)
	}
}

func TestReadersSeeCommittedSnapshotsWhileTheOnlyWriterIsBusy(t *testing.T) {
	t.Parallel()
	db, err := OpenDatabase(t.Context(), t.TempDir())
	requireNoError(t, err)
	defer db.Close()
	operation, err := db.Operation(t.Context(), "origin", "show", "video", "playlist")
	requireNoError(t, err)
	tx, err := db.writer.BeginTx(t.Context(), nil)
	requireNoError(t, err)
	defer func() { _ = tx.Rollback() }()
	_, err = tx.ExecContext(t.Context(), "UPDATE transfers SET operation_id='uncommitted'")
	requireNoError(t, err)
	ctx, cancel := context.WithTimeout(t.Context(), 2*time.Second)
	defer cancel()
	var got string
	requireNoError(
		t,
		db.readers.QueryRowContext(ctx, "SELECT operation_id FROM transfers").Scan(&got),
	)
	if got != operation {
		t.Fatalf("reader observed uncommitted work: %q", got)
	}
}

//go:embed tests/fixtures/V1__initial.sql
var historicalV1 string

func historicalJournal(t *testing.T, directory string) *sql.DB {
	t.Helper()
	uri, err := databaseFileURI(filepath.Join(directory, "sync.sqlite"))
	requireNoError(t, err)
	db := sql.OpenDB(sqlite.Connector(uri, nil, nil))
	db.SetMaxOpenConns(1)
	requireNoError(
		t,
		withScript(
			t.Context(),
			db,
			historicalV1+"\nCREATE TABLE refinery_schema_history (version INT4 PRIMARY KEY,name VARCHAR(25"+
				"5),applied_on VARCHAR(255),checksum VARCHAR(255));\nINSERT INTO refinery_schema_"+
				"history VALUES(1,'initial','2026-01-01T00:00:00Z','5099304756189747416');",
		),
	)
	return db
}

func journalHistory(t *testing.T, db *sql.DB) []string {
	t.Helper()
	rows, err := db.QueryContext(t.Context(),
		"SELECT version,name,applied_on,checksum FROM refinery_schema_history ORDER BY version",
	)
	requireNoError(t, err)
	defer rows.Close()
	var history []string
	for rows.Next() {
		var version int
		var name, applied, checksum string
		requireNoError(t, rows.Scan(&version, &name, &applied, &checksum))
		history = append(history, fmt.Sprintf("%d %s %s %s", version, name, applied, checksum))
	}
	requireNoError(t, rows.Err())
	return history
}

func TestReopeningPreservesWorkAndRejectsChangedOrMissingHistory(t *testing.T) {
	t.Parallel()
	directory := t.TempDir()
	db, err := OpenDatabase(t.Context(), directory)
	requireNoError(t, err)
	operation, err := db.Operation(t.Context(), "origin", "show", "video", "playlist")
	requireNoError(t, err)
	before := journalHistory(t, db.readers)
	requireNoError(t, db.Close())
	db, err = OpenDatabase(t.Context(), directory)
	requireNoError(t, err)
	got, err := db.Operation(t.Context(), "origin", "show", "video", "playlist")
	requireNoError(t, err)
	if got != operation || !reflect.DeepEqual(journalHistory(t, db.readers), before) {
		t.Fatal("reopening changed work or migration history")
	}
	_, err = db.writer.ExecContext(t.Context(), "UPDATE refinery_schema_history SET checksum='0'")
	requireNoError(t, err)
	requireNoError(t, db.Close())
	if invalid, err := OpenDatabase(t.Context(), directory); err == nil {
		invalid.Close()
		t.Fatal("accepted divergent migration history")
	}
	uri, err := databaseFileURI(filepath.Join(directory, "sync.sqlite"))
	requireNoError(t, err)
	raw := sql.OpenDB(sqlite.Connector(uri, nil, nil))
	_, err = raw.ExecContext(t.Context(), "DROP TABLE refinery_schema_history")
	requireNoError(t, err)
	requireNoError(t, raw.Close())
	if invalid, err := OpenDatabase(t.Context(), directory); err == nil {
		invalid.Close()
		t.Fatal("accepted missing migration history")
	}
}

func TestGroupedFailureRollsBackPendingSchemaAndHistory(t *testing.T) {
	t.Parallel()
	validDirectory := t.TempDir()
	valid := historicalJournal(t, validDirectory)
	requireNoError(t, valid.Close())
	migrated, err := OpenDatabase(t.Context(), validDirectory)
	requireNoError(t, err)
	requireNoError(t, migrated.Close())
	directory := t.TempDir()
	legacy := historicalJournal(t, directory)
	_, err = legacy.ExecContext(t.Context(), "INSERT INTO parts VALUES('absent-operation',0,1)")
	requireNoError(t, err)
	before := journalHistory(t, legacy)
	requireNoError(t, legacy.Close())
	if invalid, err := OpenDatabase(t.Context(), directory); err == nil {
		invalid.Close()
		t.Fatal("accepted an orphaned historical checkpoint")
	} else if !strings.Contains(err.Error(), "foreign key") {
		t.Fatal("migration failed before its foreign-key validation", err)
	}
	uri, err := databaseFileURI(filepath.Join(directory, "sync.sqlite"))
	requireNoError(t, err)
	raw := sql.OpenDB(sqlite.Connector(uri, nil, nil))
	defer raw.Close()
	if !reflect.DeepEqual(journalHistory(t, raw), before) {
		t.Fatal("failed schema update committed migration history")
	}
	var count int
	requireNoError(
		t,
		raw.QueryRowContext(t.Context(), "SELECT count(*) FROM pragma_table_info('source_items') WHERE name IS 'phase'").
			Scan(&count),
	)
	if count != 0 {
		t.Fatal("failed schema update left pending columns")
	}
	requireNoError(
		t,
		raw.QueryRowContext(t.Context(), "SELECT count(*) FROM sqlite_master WHERE name IS 'sync_completion'").
			Scan(&count),
	)
	if count != 0 {
		t.Fatal("failed schema update left a pending table")
	}
}

func TestDisplayedSpeedUsesRecentTrafficAndExpiresWhenStalled(t *testing.T) {
	t.Parallel()
	now := time.Now()
	rate := trafficRate{since: now, lastProgress: now}
	rate.record(10000, now.Add(time.Second))
	if got := rate.current(now.Add(time.Second)); got != 10000 {
		t.Fatal(got)
	}
	rate.record(1000, now.Add(2*time.Second))
	if got := rate.current(now.Add(2 * time.Second)); got != 1000 {
		t.Fatal(got)
	}
	if got := rate.current(now.Add(5 * time.Second)); got != 0 {
		t.Fatal(got)
	}
}

func TestAutomaticScanWaitUsesSharedClockAndCancelsPromptly(t *testing.T) {
	t.Parallel()
	clock := clockwork.NewFakeClock()
	engine := NewEngine(clock)
	defer engine.Close()
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	result := make(chan bool, 1)
	go func() { result <- engine.NextScan(ctx) }()
	guard, stop := context.WithTimeout(t.Context(), 2*time.Second)
	defer stop()
	requireNoError(t, clock.BlockUntilContext(guard, 2))
	clock.Advance(time.Hour)
	select {
	case next := <-result:
		if !next {
			t.Fatal("hourly scan was cancelled")
		}
	case <-guard.Done():
		t.Fatal("hourly scan did not wake")
	}
	go func() { result <- engine.NextScan(ctx) }()
	cancel()
	select {
	case next := <-result:
		if next {
			t.Fatal("cancelled hourly wait succeeded")
		}
	case <-guard.Done():
		t.Fatal("hourly wait ignored cancellation")
	}
}

func TestAllShowsUseOneHourlyClockAndSkipMissedTicks(t *testing.T) {
	t.Parallel()
	clock := clockwork.NewFakeClock()
	engine := NewEngine(clock)
	defer engine.Close()
	guard, stop := context.WithTimeout(t.Context(), 2*time.Second)
	defer stop()
	clock.Advance(25 * time.Second)
	result := make(chan bool, 2)
	go func() { result <- engine.NextScan(guard) }()
	go func() { result <- engine.NextScan(guard) }()
	requireNoError(t, clock.BlockUntilContext(guard, 3))
	clock.Advance(time.Hour - 25*time.Second)
	for range 2 {
		select {
		case next := <-result:
			if !next {
				t.Fatal("shared scan cancelled")
			}
		case <-guard.Done():
			t.Fatal("shows did not share the hourly deadline")
		}
	}
	clock.Advance(2*time.Hour + 25*time.Second)
	go func() { result <- engine.NextScan(guard) }()
	requireNoError(t, clock.BlockUntilContext(guard, 2))
	select {
	case <-result:
		t.Fatal("missed tick was replayed")
	default:
	}
	clock.Advance(time.Hour - 25*time.Second)
	select {
	case next := <-result:
		if !next {
			t.Fatal("next scan cancelled")
		}
	case <-guard.Done():
		t.Fatal("next hourly scan did not wake")
	}
}

// Keep structured checkpoints readable to both binaries; the API E2E supplies
// the actual interrupted package for the historical V1 recovery driver.
func TestPrepareV1Checkpoint(t *testing.T) {
	t.Parallel()
	if !fixtureInvocation {
		t.Skip("invoked by the API's V1 recovery scenario")
	}
	profile := os.Getenv("LISTENBOX_PROFILE_DIR")
	current := filepath.Join(profile, "sync.sqlite")
	historical := t.TempDir()
	legacy := historicalJournal(t, historical)
	_, err := legacy.ExecContext(t.Context(), "ATTACH DATABASE ? AS checkpoint", current)
	requireNoError(t, err)
	requireNoError(
		t,
		withScript(
			t.Context(),
			legacy,
			"INSERT INTO transfers SELECT * FROM checkpoint.transfers; INSERT INTO parts SELE"+
				"CT * FROM checkpoint.parts; INSERT INTO downloads SELECT * FROM checkpoint.downl"+
				"oads; INSERT INTO download_ranges SELECT * FROM checkpoint.download_ranges; INSE"+
				"RT INTO source_items SELECT origin,show_slug,collection_url,source_url,position "+
				"FROM checkpoint.source_items;",
		),
	)
	var prepared int
	requireNoError(
		t,
		legacy.QueryRowContext(t.Context(), "SELECT count(*) FROM transfers WHERE manifest IS NOT NULL").
			Scan(&prepared),
	)
	if prepared != 1 {
		t.Fatalf("historical fixture has %d prepared transfers", prepared)
	}
	requireNoError(t, legacy.Close())
	// #nosec G703 -- The test owns the configured temporary profile and artifact paths.
	requireNoError(t, os.Rename(current, filepath.Join(profile, "sync-checkpoint-copy.sqlite")))
	raw, err := os.ReadFile(filepath.Join(historical, "sync.sqlite"))
	requireNoError(t, err)
	// #nosec G703 -- The test owns the configured temporary profile and artifact paths.
	requireNoError(t, os.WriteFile(current, raw, 0o600))
}
