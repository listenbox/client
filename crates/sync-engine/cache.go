package syncengine

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"time"

	p "github.com/listenbox/client/crates/sync-engine/publicapi"
)

// The library cache keeps the last Listenbox catalog and episode lists in the
// journal, so a client renders them at once and refreshes them in the
// background. Rows belong to the credential that fetched them: after another
// sign-in they are never read, and the next fetch replaces them.

// existingDatabase opens the journal only when it already exists; reading the
// cache never creates one.
func (c *Client) existingDatabase(ctx context.Context) (*Database, error) {
	c.Engine.mu.Lock()
	open := c.Engine.journal != nil
	c.Engine.mu.Unlock()
	if !open {
		_, err := os.Stat(filepath.Join(c.Config.Directory, "sync.sqlite"))
		if errors.Is(err, os.ErrNotExist) {
			return nil, nil
		}
		if err != nil {
			return nil, err
		}
	}
	return c.Engine.Database(ctx, c.Config)
}

func credentialKey(config Config) string {
	sum := sha256.Sum256([]byte(LoadAuth(config)))
	return hex.EncodeToString(sum[:])
}

// CachedCatalog returns the catalog last fetched with the current credential.
func (c *Client) CachedCatalog(ctx context.Context) (Catalog, bool, error) {
	var catalog Catalog
	found, err := c.readCache(
		ctx,
		"SELECT catalog FROM catalog_cache WHERE origin IS ? AND credential IS ?",
		&catalog,
		c.Config.APIOrigin,
		credentialKey(c.Config),
	)
	return catalog, found, err
}

// CachedEpisodes returns the podcast's episode list last fetched with the
// current credential.
func (c *Client) CachedEpisodes(ctx context.Context, slug string) ([]p.EpisodeListItem, bool, error) {
	var episodes []p.EpisodeListItem
	found, err := c.readCache(
		ctx,
		"SELECT episodes FROM episode_cache WHERE origin IS ? AND show_slug IS ? AND credential IS ?",
		&episodes,
		c.Config.APIOrigin,
		slug,
		credentialKey(c.Config),
	)
	return episodes, found, err
}

func (c *Client) readCache(ctx context.Context, query string, out any, args ...any) (bool, error) {
	if !c.HasCredentials() {
		return false, ErrAuthentication
	}
	db, err := c.existingDatabase(ctx)
	if db == nil || err != nil {
		return false, err
	}
	var raw string
	err = db.readers.QueryRowContext(context.WithoutCancel(ctx), query, args...).Scan(&raw)
	if errors.Is(err, sql.ErrNoRows) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	return true, json.Unmarshal([]byte(raw), out)
}

// saveCache replaces a cached response. The response itself is already valid,
// so a failed write only costs the next launch its instant render.
func (c *Client) saveCache(ctx context.Context, query string, value any, args ...any) {
	raw, err := json.Marshal(value)
	if err != nil {
		return
	}
	db, err := c.Engine.Database(ctx, c.Config)
	if err != nil {
		return
	}
	_, _ = db.writer.ExecContext(context.WithoutCancel(ctx), query, append(args, string(raw))...)
}

func (c *Client) saveCatalog(ctx context.Context, catalog Catalog) {
	c.saveCache(
		ctx,
		"INSERT INTO catalog_cache (origin,credential,catalog) VALUES (?,?,?) "+
			"ON CONFLICT(origin) DO UPDATE SET credential=excluded.credential,catalog=excluded.catalog",
		catalog,
		c.Config.APIOrigin,
		credentialKey(c.Config),
	)
}

func (c *Client) saveEpisodes(ctx context.Context, slug string, episodes []p.EpisodeListItem) {
	c.saveCache(
		ctx,
		"INSERT INTO episode_cache (origin,show_slug,credential,episodes) VALUES (?,?,?,?) "+
			"ON CONFLICT(origin,show_slug) DO UPDATE SET credential=excluded.credential,episodes=excluded.episodes",
		episodes,
		c.Config.APIOrigin,
		slug,
		credentialKey(c.Config),
	)
}

// clearCache forgets every cached response when the journal is already open.
func (c *Client) clearCache() error {
	c.Engine.mu.Lock()
	db := c.Engine.journal
	c.Engine.mu.Unlock()
	if db == nil {
		return nil
	}
	ctx := context.Background()
	if _, err := db.writer.ExecContext(ctx, "DELETE FROM catalog_cache"); err != nil {
		return err
	}
	_, err := db.writer.ExecContext(ctx, "DELETE FROM episode_cache")
	return err
}

// SyncDue lists the podcasts a launch should sync: those no client finished
// syncing within the scan interval, and those with videos this computer still
// has queued. The hourly scan syncs every podcast regardless.
func (c *Client) SyncDue(ctx context.Context, shows []p.Show) ([]string, error) {
	db, err := c.existingDatabase(ctx)
	if err != nil {
		return nil, err
	}
	now := c.Engine.clock.Now()
	var due []string
	for _, show := range shows {
		checked := show.Youtube.LastCheckedAt
		if checked == nil || now.Sub(time.UnixMilli(*checked)) >= scanInterval {
			due = append(due, show.Slug)
			continue
		}
		if db == nil {
			continue
		}
		var queued bool
		if err := db.readers.QueryRowContext(
			context.WithoutCancel(ctx),
			"SELECT EXISTS (SELECT 1 FROM source_items WHERE origin IS ? AND show_slug IS ? AND phase = ?)",
			c.Config.APIOrigin,
			show.Slug,
			phaseQueued,
		).Scan(&queued); err != nil {
			return nil, err
		}
		if queued {
			due = append(due, show.Slug)
		}
	}
	return due, nil
}
