package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync/atomic"
	"time"

	s "github.com/listenbox/client/crates/sync-engine"
)

type diagnosticWriter struct {
	lines   chan []byte
	dropped atomic.Uint64
}

const (
	diagnosticDirectoryMode = 0o700
	diagnosticFileMode      = 0o600
	diagnosticQueueSize     = 1024
	diagnosticFileLimit     = 48
	diagnosticTailFiles     = 2
	diagnosticTailBytes     = 256 << 10
	diagnosticTailLines     = 100
)

func (w *diagnosticWriter) Write(raw []byte) (int, error) {
	select {
	case w.lines <- bytes.Clone(raw):
	default:
		w.dropped.Add(1)
	}
	return len(raw), nil
}

func openDiagnostics(profile string) (func(), error) {
	directory := filepath.Join(profile, "diagnostics")
	if err := os.MkdirAll(directory, diagnosticDirectoryMode); err != nil {
		return nil, err
	}
	writer := &diagnosticWriter{lines: make(chan []byte, diagnosticQueueSize)}
	finished := make(chan struct{})
	go func() {
		defer close(finished)
		var file *os.File
		var hour string
		defer func() {
			if file != nil {
				_ = file.Close()
			}
		}()
		for line := range writer.lines {
			now := time.Now().UTC().Format("2006-01-02-15")
			if now != hour {
				if file != nil {
					_ = file.Close()
				}
				file, _ = os.OpenFile(
					filepath.Join(directory, "desktop."+now+".jsonl"),
					os.O_CREATE|os.O_APPEND|os.O_WRONLY,
					diagnosticFileMode,
				)
				hour = now
				paths, _ := filepath.Glob(filepath.Join(directory, "desktop.*.jsonl"))
				slices.Sort(paths)
				for _, path := range paths[:max(0, len(paths)-diagnosticFileLimit)] {
					_ = os.Remove(path)
				}
			}
			if file != nil {
				_, _ = file.Write(line)
			}
		}
	}()
	slog.SetDefault(
		slog.New(slog.NewJSONHandler(writer, &slog.HandlerOptions{Level: slog.LevelInfo})),
	)
	slog.Info("desktop started", "pid", os.Getpid(), "version", s.BuildVersion(), "commit", s.SourceCommit)
	return func() {
		slog.Info("desktop stopped", "dropped_log_lines", writer.dropped.Load())
		close(writer.lines)
		<-finished
	}, nil
}

func reportDiagnostics(profile string) error {
	directory := filepath.Join(profile, "diagnostics")
	fmt.Println("Diagnostics:", directory)
	paths, err := filepath.Glob(filepath.Join(directory, "desktop.*.jsonl"))
	if err != nil {
		return err
	}
	slices.Sort(paths)
	for _, path := range paths[max(0, len(paths)-diagnosticTailFiles):] {
		fmt.Println("Log:", path)
		file, err := os.Open(path)
		if err != nil {
			return err
		}
		stat, err := file.Stat()
		if err != nil {
			file.Close()
			return err
		}
		offset := max(int64(0), stat.Size()-diagnosticTailBytes)
		_, _ = file.Seek(offset, io.SeekStart)
		raw, err := io.ReadAll(io.LimitReader(file, diagnosticTailBytes))
		file.Close()
		if err != nil {
			return err
		}
		lines := strings.Split(strings.TrimSpace(string(raw)), "\n")
		for _, line := range lines[max(0, len(lines)-diagnosticTailLines):] {
			if json.Valid([]byte(line)) {
				fmt.Println(line)
			}
		}
	}
	return nil
}
