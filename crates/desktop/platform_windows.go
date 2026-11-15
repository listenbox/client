package main

import (
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"

	winsparkle "github.com/abemedia/go-winsparkle"
	s "github.com/listenbox/client/crates/sync-engine"
	"golang.org/x/sys/windows"
	"golang.org/x/sys/windows/registry"
)

func installationLock() (func(), error) {
	name, _ := windows.UTF16PtrFromString(`Local\ListenboxDesktop`)
	mutex, err := windows.CreateMutex(nil, false, name)
	if err != nil && !errors.Is(err, windows.ERROR_ALREADY_EXISTS) {
		return nil, err
	}
	return func() { _ = windows.CloseHandle(mutex) }, nil
}

type winSparkle struct{ dll *windows.DLL }

func startUpdater(requestShutdown func(), _ func()) (nativeUpdater, error) {
	if updatePublicKey == "" {
		return nil, nil
	}
	key, err := base64.StdEncoding.DecodeString(updatePublicKey)
	if err != nil || len(key) != 32 {
		return nil, fmt.Errorf("invalid WinSparkle public key")
	}
	executable, err := os.Executable()
	if err != nil {
		return nil, err
	}
	// Load our verified adjacent DLL before the binding resolves its procedures.
	dll, err := windows.LoadDLL(filepath.Join(filepath.Dir(executable), "WinSparkle.dll"))
	if err != nil {
		return nil, fmt.Errorf("load bundled WinSparkle: %w", err)
	}
	if err := winsparkle.SetEdDSAPublicKey(updatePublicKey); err != nil {
		dll.Release()
		return nil, err
	}
	winsparkle.SetAppDetails(applicationName, applicationName, s.BuildVersion())
	winsparkle.SetAppBuildVersion(s.BuildVersion() + ".0")
	const preferences = `Software\Listenbox\Updates`
	winsparkle.SetRegistryPath(preferences)
	winsparkle.SetAppcastURL(
		"https://github.com/listenbox/client/releases/latest/download/appcast-" + updatePlatform + ".xml",
	)
	winsparkle.SetUpdateCheckInterval(24 * time.Hour)
	winsparkle.SetShutdownRequestCallback(requestShutdown)
	missing := true
	for _, hive := range []registry.Key{registry.CURRENT_USER, registry.LOCAL_MACHINE} {
		key, err := registry.OpenKey(hive, preferences, registry.QUERY_VALUE)
		if errors.Is(err, windows.ERROR_FILE_NOT_FOUND) {
			continue
		}
		if err != nil {
			missing = false
			break
		}
		_, _, err = key.GetValue("CheckForUpdates", nil)
		key.Close()
		if !errors.Is(err, windows.ERROR_FILE_NOT_FOUND) {
			missing = false
			break
		}
	}
	if missing {
		winsparkle.SetAutomaticCheckForUpdates(true)
	}
	winsparkle.Init()
	return &winSparkle{dll: dll}, nil
}
func (u *winSparkle) canCheck() bool             { return true }
func (u *winSparkle) check()                     { winsparkle.CheckUpdateWithUI() }
func (u *winSparkle) automatic() bool            { return winsparkle.GetAutomaticCheckForUpdates() }
func (u *winSparkle) setAutomatic(enabled bool)  { winsparkle.SetAutomaticCheckForUpdates(enabled) }
func (u *winSparkle) automaticDownloads() bool   { return false }
func (u *winSparkle) setAutomaticDownloads(bool) {}
func (u *winSparkle) drained() bool              { return false }
func (u *winSparkle) close()                     { winsparkle.Cleanup(); u.dll.Release() }
