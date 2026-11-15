package syncengine

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"time"

	"github.com/gofrs/flock"
)

const (
	profileLockInterval = 10 * time.Millisecond
)

func profileLock(ctx context.Context, path, name string) (*flock.Flock, error) {
	if err := os.MkdirAll(filepath.Dir(path), profileDirectoryMode); err != nil {
		return nil, err
	}
	lock := flock.New(path)
	slog.Info("acquiring profile lock", "lock", name)
	ok, err := lock.TryLockContext(ctx, profileLockInterval)
	if ctx.Err() != nil {
		lock.Close()
		return nil, fmt.Errorf("operation cancelled while acquiring %s: %w", name, ctx.Err())
	}
	if err != nil || !ok {
		lock.Close()
		return nil, fmt.Errorf("acquire %s lock: %w", name, err)
	}
	slog.Info("profile lock acquired", "lock", name)
	return lock, nil
}
