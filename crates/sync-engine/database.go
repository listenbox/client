package syncengine

import (
	"context"
	"database/sql"
	"database/sql/driver"
	_ "embed"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"

	"ariga.io/atlas/sql/migrate"
	"ariga.io/atlas/sql/schema"
	atlassqlite "ariga.io/atlas/sql/sqlite"
	"github.com/google/uuid"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
	"github.com/tailscale/sqlite"
)

//go:embed schema.sql
var desiredSchema string

// Database owns durable checkpoints. Once admitted, journal operations drain
// independently of request cancellation; the client joins them before Close.
type Database struct {
	writer, readers *sql.DB
	root            string
}

const (
	journalReaders = 8
)

func OpenDatabase(ctx context.Context, directory string) (*Database, error) {
	lock, err := profileLock(ctx, filepath.Join(directory, "sync-schema.lock"), "sync journal")
	if err != nil {
		return nil, err
	}
	defer lock.Close()
	path := filepath.Join(directory, "sync.sqlite")
	uri, err := databaseFileURI(path)
	if err != nil {
		return nil, err
	}
	initialize := func(_ context.Context, conn driver.ConnPrepareContext) error {
		raw, ok := conn.(sqlite.SQLConn)
		if !ok {
			return fmt.Errorf("unexpected SQLite connection %T", conn)
		}
		return sqlite.ExecScript(
			raw,
			"PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;",
		)
	}
	writer := sql.OpenDB(sqlite.Connector(uri, initialize, nil))
	writer.SetMaxOpenConns(1)
	if _, err := writer.ExecContext(ctx, "PRAGMA journal_mode=WAL"); err != nil {
		writer.Close()
		return nil, err
	}
	db := &Database{writer: writer, root: filepath.Join(directory, "transfers")}
	if err := db.ensureSchema(ctx); err != nil {
		writer.Close()
		return nil, fmt.Errorf("initialize sync journal: %w", err)
	}
	db.readers = sql.OpenDB(
		sqlite.Connector(
			uri+"?mode=ro",
			func(ctx context.Context, conn driver.ConnPrepareContext) error {
				if err := initialize(ctx, conn); err != nil {
					return err
				}
				raw, ok := conn.(sqlite.SQLConn)
				if !ok {
					return fmt.Errorf("unexpected SQLite reader connection %T", conn)
				}
				return sqlite.ExecScript(raw, "PRAGMA query_only=ON;")
			},
			nil,
		),
	)
	db.readers.SetMaxOpenConns(journalReaders)
	return db, nil
}

func databaseFileURI(path string) (string, error) {
	absolute, err := filepath.Abs(path)
	if err != nil {
		return "", fmt.Errorf("resolve database path %q: %w", path, err)
	}
	uriPath := filepath.ToSlash(absolute)
	// SQLite requires /C:/... for drive paths, with an empty URI authority.
	if !strings.HasPrefix(uriPath, "/") {
		uriPath = "/" + uriPath
	}
	return (&url.URL{Scheme: fileScheme, Path: uriPath}).String(), nil
}

func (db *Database) Close() error {
	readErr := db.readers.Close()
	err := db.writer.Close()
	if err != nil {
		return err
	}
	return readErr
}

func withScript(ctx context.Context, db *sql.DB, script string) error {
	conn, err := db.Conn(ctx)
	if err != nil {
		return err
	}
	defer conn.Close()
	return conn.Raw(func(raw any) error {
		conn, ok := raw.(sqlite.SQLConn)
		if !ok {
			return fmt.Errorf("unexpected SQLite script connection %T", raw)
		}
		return sqlite.ExecScript(conn, script)
	})
}

func (db *Database) Operation(
	ctx context.Context,
	origin, show, source, collection string,
) (string, error) {
	ctx = context.WithoutCancel(ctx)
	tx, err := db.writer.BeginTx(ctx, nil)
	if err != nil {
		return "", err
	}
	defer func() { _ = tx.Rollback() }()
	if _, err := tx.ExecContext(
		ctx,
		"INSERT INTO transfers (origin,show_slug,source_url,collection_url,operation_id) "+
			"VALUES (?,?,?,?,?) ON CONFLICT DO NOTHING",
		origin,
		show,
		source,
		collection,
		uuid.NewString(),
	); err != nil {
		return "", err
	}
	var id string
	if err := tx.QueryRowContext(ctx, "SELECT operation_id FROM transfers WHERE origin IS ? AND show_slug IS ? AND sour"+
		"ce_url IS ? AND collection_url IS ?", origin, show, source, collection).
		Scan(&id); err != nil {
		return "", err
	}
	return id, tx.Commit()
}

func (db *Database) Directory(operation string) (string, error) {
	id, err := uuid.Parse(operation)
	if err != nil {
		return "", fmt.Errorf("invalid transfer operation %q", operation)
	}
	directory := filepath.Join(db.root, id.String())
	return directory, os.MkdirAll(directory, profileDirectoryMode)
}

func (db *Database) Prepared(
	ctx context.Context,
	operation string,
) (*p.CreateEpisodePackage, error) {
	ctx = context.WithoutCancel(ctx)
	var raw sql.NullString
	if err := db.readers.QueryRowContext(
		ctx, "SELECT manifest FROM transfers WHERE operation_id IS ?", operation).
		Scan(&raw); err != nil {
		return nil, err
	}
	if !raw.Valid {
		return nil, nil
	}
	var manifest p.CreateEpisodePackage
	if err := json.Unmarshal([]byte(raw.String), &manifest); err != nil {
		return nil, err
	}
	return &manifest, nil
}

func (db *Database) SavePrepared(ctx context.Context, manifest p.CreateEpisodePackage) error {
	ctx = context.WithoutCancel(ctx)
	raw, err := json.Marshal(manifest)
	if err != nil {
		return err
	}
	_, err = db.writer.ExecContext(ctx,
		"UPDATE transfers SET manifest=? WHERE operation_id IS ?",
		string(raw),
		manifest.OperationId,
	)
	return err
}

func (db *Database) Session(ctx context.Context, operation, session string) error {
	ctx = context.WithoutCancel(ctx)
	tx, err := db.writer.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	if _, err := tx.ExecContext(
		ctx,
		"DELETE FROM parts WHERE operation_id IS ? AND NOT EXISTS (SELECT 1 FROM transfer"+
			"s WHERE operation_id IS ? AND upload_session_id IS ?)",
		operation,
		operation,
		session,
	); err != nil {
		return err
	}
	if _, err := tx.ExecContext(ctx,
		"UPDATE transfers SET upload_session_id=? WHERE operation_id IS ?",
		session,
		operation,
	); err != nil {
		return err
	}
	return tx.Commit()
}

func (db *Database) HasPart(
	ctx context.Context,
	operation string,
	object int,
	part int32,
) (bool, error) {
	ctx = context.WithoutCancel(ctx)
	var exists int
	err := db.readers.QueryRowContext(
		ctx, "SELECT count(*) FROM parts WHERE operation_id IS ? AND object_index IS ? AND par"+
			"t_number IS ?", operation, object, part).
		Scan(&exists)
	return exists > 0, err
}

func (db *Database) SavePart(ctx context.Context, operation string, object int, part int32) error {
	ctx = context.WithoutCancel(ctx)
	_, err := db.writer.ExecContext(ctx,
		"INSERT INTO parts VALUES (?,?,?) ON CONFLICT DO NOTHING",
		operation,
		object,
		part,
	)
	return err
}

func (db *Database) Download(ctx context.Context, operation, name, identity string) error {
	ctx = context.WithoutCancel(ctx)
	tx, err := db.writer.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	if _, err := tx.ExecContext(ctx,
		"DELETE FROM downloads WHERE operation_id IS ? AND name IS ? AND identity IS NOT ?",
		operation,
		name,
		identity,
	); err != nil {
		return err
	}
	if _, err := tx.ExecContext(ctx,
		"INSERT INTO downloads VALUES (?,?,?) ON CONFLICT DO NOTHING",
		operation,
		name,
		identity,
	); err != nil {
		return err
	}
	return tx.Commit()
}

func (db *Database) RangeHash(
	ctx context.Context,
	operation, name string,
	start int64,
) (string, error) {
	ctx = context.WithoutCancel(ctx)
	var hash string
	err := db.readers.QueryRowContext(
		ctx,
		"SELECT sha256 FROM download_ranges WHERE operation_id IS ? AND name IS ? AND start IS ?",
		operation,
		name,
		start,
	).
		Scan(&hash)
	if err == sql.ErrNoRows {
		return "", nil
	}
	return hash, err
}

func (db *Database) SaveRange(
	ctx context.Context,
	operation, name string,
	start int64,
	hash string,
) error {
	ctx = context.WithoutCancel(ctx)
	_, err := db.writer.ExecContext(
		ctx,
		"INSERT INTO download_ranges VALUES (?,?,?,?) ON CONFLICT DO UPDATE SET sha256=ex"+
			"cluded.sha256",
		operation,
		name,
		start,
		hash,
	)
	return err
}

func (db *Database) Forget(ctx context.Context, origin, show, source string) error {
	ctx = context.WithoutCancel(ctx)
	rows, err := db.readers.QueryContext(
		ctx,
		"SELECT operation_id FROM transfers WHERE origin IS ? AND show_slug IS ? AND sour"+
			"ce_url IS ?",
		origin,
		show,
		source,
	)
	if err != nil {
		return err
	}
	var ids []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return err
		}
		ids = append(ids, id)
	}
	if err := rows.Close(); err != nil {
		return err
	}
	for _, id := range ids {
		directory, err := db.Directory(id)
		if err != nil {
			return err
		}
		if err := os.RemoveAll(directory); err != nil {
			return err
		}
	}
	_, err = db.writer.ExecContext(ctx,
		"DELETE FROM transfers WHERE origin IS ? AND show_slug IS ? AND source_url IS ?",
		origin,
		show,
		source,
	)
	return err
}

func (db *Database) Snapshot(ctx context.Context,
	origin, show, collection string,
	videos []Video,
	canRemove bool,
) error {
	ctx = context.WithoutCancel(ctx)
	tx, err := db.writer.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	present := make(map[string]bool)
	for _, v := range videos {
		present[v.URL()] = true
	}
	if canRemove {
		if err := removeMissingSources(ctx, tx, origin, show, present); err != nil {
			return err
		}
	}
	for i, v := range videos {
		if _, err := tx.ExecContext(
			ctx,
			"INSERT INTO source_items (origin,show_slug,collection_url,source_url,position,ti"+
				"tle,duration_seconds,phase) VALUES (?,?,?,?,?,?,?,'queued') ON CONFLICT(origin,s"+
				"how_slug,source_url) DO UPDATE SET position=excluded.position,title=excluded.tit"+
				"le,duration_seconds=excluded.duration_seconds",
			origin,
			show,
			collection,
			v.URL(),
			i,
			v.Title,
			v.DurationSeconds,
		); err != nil {
			return err
		}
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	if !canRemove {
		return nil
	}
	return db.forgetMissingTransfers(ctx, origin, show, present)
}

func (db *Database) Outcome(ctx context.Context, origin, show string, item Download) error {
	ctx = context.WithoutCancel(ctx)
	phase := item.Phase.String()
	if item.Phase != Complete() && item.Phase != Skipped() && item.Phase != Failed() {
		phase = phaseQueued
	}
	var reason, failure *string
	if item.Reason != "" {
		reason = &item.Reason
	}
	if item.Error != "" {
		failure = &item.Error
	}
	_, err := db.writer.ExecContext(
		ctx,
		"UPDATE source_items SET title=?,duration_seconds=?,phase=?,reason=?,error=? WHER"+
			"E origin IS ? AND show_slug IS ? AND source_url IS ?",
		item.Title,
		item.DurationSeconds,
		phase,
		reason,
		failure,
		origin,
		show,
		item.SourceURL,
	)
	return err
}

func (db *Database) Published(ctx context.Context, origin, show string, sources []string) error {
	ctx = context.WithoutCancel(ctx)
	tx, err := db.writer.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	for _, source := range sources {
		if _, err := tx.ExecContext(
			ctx,
			"UPDATE source_items SET phase='complete',reason=NULL,error=NULL WHERE origin IS "+
				"? AND show_slug IS ? AND source_url IS ?",
			origin,
			show,
			source,
		); err != nil {
			return err
		}
	}
	return tx.Commit()
}

func (db *Database) CompleteSync(ctx context.Context, origin, show string) error {
	ctx = context.WithoutCancel(ctx)
	_, err := db.writer.ExecContext(
		ctx,
		"INSERT INTO sync_completions VALUES (?,?,?) ON CONFLICT DO UPDATE SET completed_"+
			"at=excluded.completed_at",
		origin,
		show,
		time.Now().UTC().UnixMilli(),
	)
	return err
}

func (db *Database) LastSynced(ctx context.Context, origin, show string) (*time.Time, error) {
	ctx = context.WithoutCancel(ctx)
	var millis int64
	err := db.readers.QueryRowContext(
		ctx,
		"SELECT completed_at FROM sync_completions WHERE origin IS ? AND show_slug IS ?",
		origin,
		show,
	).
		Scan(&millis)
	if err == sql.ErrNoRows {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	instant := time.UnixMilli(millis).UTC()
	return &instant, nil
}

func (db *Database) Items(ctx context.Context, origin, show string) ([]Download, error) {
	ctx = context.WithoutCancel(ctx)
	rows, err := db.readers.QueryContext(
		ctx,
		"SELECT source_url,position,title,duration_seconds,phase,reason,error FROM source"+
			"_items WHERE origin IS ? AND show_slug IS ? ORDER BY position,source_url",
		origin,
		show,
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var items []Download
	for rows.Next() {
		var item Download
		var phase string
		var reason, failure sql.NullString
		if err := rows.Scan(
			&item.SourceURL,
			&item.Position,
			&item.Title,
			&item.DurationSeconds,
			&phase,
			&reason,
			&failure,
		); err != nil {
			return nil, err
		}
		item.ID = show + "/" + strings.TrimPrefix(
			item.SourceURL,
			"https://www.youtube.com/watch?v=",
		)
		item.SourceID = show
		item.SourceTitle = show
		item.Phase, _ = NewPhaseFromValue(phase)
		item.Reason = reason.String
		item.Error = failure.String
		items = append(items, item)
	}
	return items, rows.Err()
}

// forgetMissingTransfers drops resumable work for videos a complete scan no
// longer lists. Failed transfers are otherwise kept until they succeed, which
// never happens once their source is gone; the API expires abandoned packages.
func (db *Database) forgetMissingTransfers(
	ctx context.Context,
	origin, show string,
	present map[string]bool,
) error {
	rows, err := db.readers.QueryContext(ctx,
		"SELECT DISTINCT source_url FROM transfers WHERE origin IS ? AND show_slug IS ?",
		origin,
		show,
	)
	if err != nil {
		return err
	}
	var missing []string
	for rows.Next() {
		var source string
		if err := rows.Scan(&source); err != nil {
			rows.Close()
			return err
		}
		if !present[source] {
			missing = append(missing, source)
		}
	}
	if err := rows.Close(); err != nil {
		return err
	}
	for _, source := range missing {
		if err := db.Forget(ctx, origin, show, source); err != nil {
			return err
		}
	}
	return nil
}

func (db *Database) ensureSchema(ctx context.Context) error {
	conn, err := db.writer.Conn(ctx)
	if err != nil {
		return err
	}
	defer conn.Close()
	plan, err := planJournalSchema(ctx, conn)
	if err != nil {
		return err
	}
	// Atlas may rebuild a table to add constraints. Foreign-key enforcement must
	// be disabled outside the transaction, then validated before committing it.
	if _, err := conn.ExecContext(ctx, "PRAGMA foreign_keys=OFF"); err != nil {
		return err
	}
	defer func() { _, _ = conn.ExecContext(context.WithoutCancel(ctx), "PRAGMA foreign_keys=ON") }()
	tx, err := conn.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	for _, change := range plan.Changes {
		if _, err := tx.ExecContext(ctx, change.Cmd, change.Args...); err != nil {
			return err
		}
	}
	rows, err := tx.QueryContext(ctx, "PRAGMA foreign_key_check")
	if err != nil {
		return err
	}
	violations := rows.Next()
	rows.Close()
	if violations {
		return errors.New("journal schema change violates a foreign key")
	}
	return tx.Commit()
}

func normalizeConstraints(inspected *schema.Schema) error {
	for _, table := range inspected.Tables {
		for _, index := range table.Indexes {
			if !strings.HasPrefix(index.Name, "sqlite_autoindex") {
				continue
			}
			primary := false
			for _, attr := range index.Attrs {
				if origin, ok := attr.(*atlassqlite.IndexOrigin); ok && origin.O == "p" {
					primary = true
				}
			}
			if primary {
				continue
			}
			names := []string{table.Name}
			for _, part := range index.Parts {
				if part.C == nil {
					return fmt.Errorf(
						"unexpected expression in journal constraint %q",
						index.Name,
					)
				}
				names = append(names, part.C.Name)
			}
			index.Name = strings.Join(names, "_")
		}
	}
	return nil
}

func validateSchemaChanges(changes []schema.Change) error {
	for _, change := range changes {
		switch change := change.(type) {
		case *schema.AddTable:
		case *schema.ModifyTable:
			for _, inner := range change.Changes {
				switch inner := inner.(type) {
				case *schema.AddColumn, *schema.AddCheck:
				case *schema.DropIndex:
					return fmt.Errorf(
						"refusing to drop journal index %q on %q",
						inner.I.Name,
						change.T.Name,
					)
				default:
					return fmt.Errorf(
						"refusing destructive journal schema change %T on %q",
						inner,
						change.T.Name,
					)
				}
			}
		default:
			return fmt.Errorf("refusing destructive journal schema change %T", change)
		}
	}
	return nil
}

func planJournalSchema(ctx context.Context, conn *sql.Conn) (*migrate.Plan, error) {
	dev := sql.OpenDB(
		sqlite.Connector("file:/listenbox-schema-"+uuid.NewString()+"?vfs=memdb", nil, nil),
	)
	dev.SetMaxOpenConns(1)
	defer dev.Close()
	if err := withScript(ctx, dev, desiredSchema); err != nil {
		panic(fmt.Errorf("load embedded journal schema: %w", err))
	}
	desired, err := atlassqlite.Open(dev)
	if err != nil {
		return nil, err
	}
	want, err := desired.InspectSchema(ctx, "main", nil)
	if err != nil {
		return nil, err
	}
	// Atlas creates inline UNIQUE constraints as named indexes. Compare those
	// constraints by their columns so SQLite's generated names and
	// the names Atlas assigns during CREATE INDEX represent the same schema.

	if err := normalizeConstraints(want); err != nil {
		return nil, err
	}
	current, err := atlassqlite.Open(conn)
	if err != nil {
		return nil, err
	}
	tables := make([]string, len(want.Tables))
	for i, table := range want.Tables {
		tables[i] = table.Name
	}
	have, err := current.InspectSchema(ctx, "main", &schema.InspectOptions{Tables: tables})
	if err != nil {
		return nil, err
	}
	if err := normalizeConstraints(have); err != nil {
		return nil, err
	}
	changes, err := current.SchemaDiff(have, want)
	if err != nil {
		return nil, err
	}
	if err := validateSchemaChanges(changes); err != nil {
		return nil, err
	}
	return current.PlanChanges(ctx, "listenbox-journal", changes)
}

func removeMissingSources(
	ctx context.Context,
	tx *sql.Tx,
	origin, show string,
	present map[string]bool,
) error {
	rows, err := tx.QueryContext(ctx,
		"SELECT source_url FROM source_items WHERE origin IS ? AND show_slug IS ?",
		origin,
		show,
	)
	if err != nil {
		return err
	}
	var removed []string
	for rows.Next() {
		var source string
		if err := rows.Scan(&source); err != nil {
			rows.Close()
			return err
		}
		if !present[source] {
			removed = append(removed, source)
		}
	}
	if err := rows.Close(); err != nil {
		return err
	}
	for _, source := range removed {
		if _, err := tx.ExecContext(ctx,
			"DELETE FROM source_items WHERE origin IS ? AND show_slug IS ? AND source_url IS ?",
			origin,
			show,
			source,
		); err != nil {
			return err
		}
	}
	return nil
}
