package syncengine

import (
	"database/sql"
	"fmt"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/tailscale/sqlite"
)

// This extra migration exists only in the isolated compatibility test process.
// The shipped desired schema and the three historical migrations stay intact.
//
//nolint:funlen,paralleltest // The isolated driver owns the temporary schema and proves the whole Rust checkpoint.
func TestAtlasForwardSchemaContinuesRustHistory(t *testing.T) {
	config := liveConfig(t)
	uri, err := databaseFileURI(filepath.ToSlash(filepath.Join(config.Directory, "sync.sqlite")))
	requireNoError(t, err)
	raw := sql.OpenDB(sqlite.Connector(uri, nil, nil))
	defer raw.Close()
	var operation, manifest, session string
	requireNoError(
		t,
		raw.QueryRowContext(t.Context(), "SELECT operation_id,manifest,upload_session_id FROM transfers").
			Scan(&operation, &manifest, &session),
	)
	if operation == "" || !strings.Contains(manifest, "operation_id") || session == "" {
		t.Fatal("Rust did not leave an admitted prepared checkpoint")
	}
	readHistory := func() []string {
		rows, err := raw.QueryContext(
			t.Context(),
			"SELECT version,name,applied_on,checksum FROM refinery_schema_history ORDER BY version",
		)
		requireNoError(t, err)
		defer rows.Close()
		var history []string
		for rows.Next() {
			var version int
			var name, applied, checksum string
			requireNoError(t, rows.Scan(&version, &name, &applied, &checksum))
			history = append(history, fmt.Sprintf("%d:%s:%s:%s", version, name, applied, checksum))
		}
		requireNoError(t, rows.Err())
		return history
	}
	history := readHistory()
	if len(history) != 3 {
		t.Fatalf("Rust migration history: %v", history)
	}
	production := desiredSchema
	desiredSchema += "\nCREATE TABLE forward_migration_probe (operation_id TEXT PRIMARY KEY REFERENCES" +
		" transfers(operation_id), inspected_at INTEGER NOT NULL CHECK(inspected_at>=0));"
	defer func() { desiredSchema = production }()
	db, err := OpenDatabase(t.Context(), config.Directory)
	requireNoError(t, err)
	_, err = db.writer.ExecContext(
		t.Context(),
		"INSERT INTO forward_migration_probe VALUES (?,?)",
		operation,
		1,
	)
	requireNoError(t, err)
	requireNoError(t, db.Close())
	db, err = OpenDatabase(t.Context(), config.Directory)
	requireNoError(t, err)
	var saved string
	requireNoError(
		t,
		db.readers.QueryRowContext(t.Context(), "SELECT operation_id FROM forward_migration_probe").
			Scan(&saved),
	)
	if saved != operation || !reflect.DeepEqual(history, readHistory()) {
		t.Fatal("Atlas continuation changed the Rust checkpoint or migration history")
	}
	var afterManifest, afterSession string
	requireNoError(
		t,
		db.readers.QueryRowContext(
			t.Context(),
			"SELECT manifest,upload_session_id FROM transfers WHERE operation_id IS ?",
			operation,
		).
			Scan(&afterManifest, &afterSession),
	)
	if afterManifest != manifest || afterSession != session {
		t.Fatal("Atlas changed the admitted upload")
	}
	_, err = db.writer.ExecContext(t.Context(), "DROP TABLE forward_migration_probe")
	requireNoError(t, err)
	requireNoError(t, db.Close())
	desiredSchema = production
	db, err = OpenDatabase(t.Context(), config.Directory)
	requireNoError(t, err)
	requireNoError(t, db.Close())
}
