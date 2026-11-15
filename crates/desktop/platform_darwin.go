package main

import (
	"encoding/base64"
	"fmt"
	"os"
	"path/filepath"

	"github.com/ebitengine/purego"
	"github.com/ebitengine/purego/objc"
)

func installationLock() (func(), error) { return func() {}, nil }

type sparkleUpdater struct {
	framework                     uintptr
	delegate, controller, updater objc.ID
	install                       objc.Block
	observed                      []string
}

func nsString(value string) objc.ID {
	return objc.ID(objc.GetClass("NSString")).
		Send(objc.RegisterName("stringWithUTF8String:"), value)
}

func startUpdater(requestShutdown, changed func()) (nativeUpdater, error) {
	if updatePublicKey == "" {
		return nil, nil
	}
	key, err := base64.StdEncoding.DecodeString(updatePublicKey)
	if err != nil || len(key) != 32 {
		return nil, fmt.Errorf("invalid Sparkle public key")
	}
	executable, err := os.Executable()
	if err != nil {
		return nil, err
	}
	framework, err := purego.Dlopen(
		filepath.Join(filepath.Dir(executable), "..", "Resources", "Sparkle.framework", "Sparkle"),
		purego.RTLD_NOW|purego.RTLD_GLOBAL,
	)
	if err != nil {
		return nil, fmt.Errorf("load bundled Sparkle: %w", err)
	}
	u := &sparkleUpdater{
		framework: framework,
		observed: []string{
			"canCheckForUpdates", "automaticallyChecksForUpdates", "automaticallyDownloadsUpdates",
		},
	}
	feed := "https://github.com/listenbox/client/releases/latest/download/appcast-" + updatePlatform + ".xml"
	info := objc.ID(objc.GetClass("NSBundle")).
		Send(objc.RegisterName("mainBundle")).
		Send(objc.RegisterName("infoDictionary"))
	for name, expected := range map[string]string{"SUPublicEDKey": updatePublicKey, "SUFeedURL": feed} {
		actual := info.Send(objc.RegisterName("objectForKey:"), nsString(name))
		if actual == 0 ||
			!objc.Send[bool](actual, objc.RegisterName("isEqualToString:"), nsString(expected)) {
			return nil, fmt.Errorf("bundled %s does not match the updater configuration", name)
		}
	}
	class, err := objc.RegisterClass(
		"ListenboxGoUpdaterDelegate",
		objc.GetClass("NSObject"),
		[]*objc.Protocol{objc.GetProtocol("SPUUpdaterDelegate")},
		nil,
		[]objc.MethodDef{
			{
				Cmd: objc.RegisterName("observeValueForKeyPath:ofObject:change:context:"),
				Fn: func(_ objc.ID, _ objc.SEL, _ objc.ID, _ objc.ID, _ objc.ID, _ uintptr) {
					changed()
				},
			},
			{
				Cmd: objc.RegisterName(
					"updater:shouldPostponeRelaunchForUpdate:untilInvokingBlock:",
				),
				Fn: func(_ objc.ID, _ objc.SEL, _ objc.ID, _ objc.ID, install objc.Block) bool {
					if u.install != 0 {
						u.install.Release()
					}
					u.install = install.Copy()
					requestShutdown()
					return true
				},
			},
		},
	)
	if err != nil {
		return nil, err
	}
	u.delegate = objc.ID(class).Send(objc.RegisterName("new"))
	u.controller = objc.ID(objc.GetClass("SPUStandardUpdaterController")).
		Send(objc.RegisterName("alloc")).
		Send(objc.RegisterName("initWithStartingUpdater:updaterDelegate:userDriverDelegate:"), false, u.delegate, objc.ID(0))
	if u.controller == 0 {
		u.close()
		return nil, fmt.Errorf("initialize Sparkle controller")
	}
	u.updater = u.controller.Send(objc.RegisterName("updater"))
	for _, key := range u.observed {
		u.updater.Send(objc.RegisterName("addObserver:forKeyPath:options:context:"),
			u.delegate, nsString(key), uintptr(0), uintptr(0))
	}
	var nativeError objc.ID
	if !objc.Send[bool](u.updater, objc.RegisterName("startUpdater:"), &nativeError) {
		description := nativeError.Send(objc.RegisterName("localizedDescription"))
		err := fmt.Errorf(
			"start Sparkle: %s",
			objc.Send[string](description, objc.RegisterName("UTF8String")),
		)
		u.close()
		return nil, err
	}
	return u, nil
}

func (u *sparkleUpdater) canCheck() bool {
	return objc.Send[bool](u.updater, objc.RegisterName("canCheckForUpdates"))
}

func (u *sparkleUpdater) check() {
	u.controller.Send(objc.RegisterName("checkForUpdates:"), objc.ID(0))
}

func (u *sparkleUpdater) automatic() bool {
	return objc.Send[bool](u.updater, objc.RegisterName("automaticallyChecksForUpdates"))
}

func (u *sparkleUpdater) setAutomatic(enabled bool) {
	u.updater.Send(objc.RegisterName("setAutomaticallyChecksForUpdates:"), enabled)
}

func (u *sparkleUpdater) automaticDownloads() bool {
	return objc.Send[bool](u.updater, objc.RegisterName("automaticallyDownloadsUpdates"))
}

func (u *sparkleUpdater) setAutomaticDownloads(enabled bool) {
	u.updater.Send(objc.RegisterName("setAutomaticallyDownloadsUpdates:"), enabled)
}

func (u *sparkleUpdater) drained() bool {
	if u.install == 0 {
		return false
	}
	install := u.install
	u.install = 0
	install.Invoke()
	install.Release()
	return true
}

func (u *sparkleUpdater) close() {
	if u.install != 0 {
		u.install.Release()
		u.install = 0
	}
	if u.controller != 0 {
		for _, key := range u.observed {
			u.updater.Send(objc.RegisterName("removeObserver:forKeyPath:"), u.delegate, nsString(key))
		}
		u.controller.Send(objc.RegisterName("release"))
		u.controller = 0
	}
	if u.delegate != 0 {
		u.delegate.Send(objc.RegisterName("release"))
		u.delegate = 0
	}
	// Objective-C class implementations remain registered until process exit.
	// Keep the framework loaded while those implementations can be called.
}
