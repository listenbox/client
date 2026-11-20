package main

import (
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"structs"
	"unsafe"

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

//nolint:ireturn // Each platform constructs the common native updater interface.
func startUpdater(requestShutdown, changed func()) (nativeUpdater, error) {
	if updatePublicKey == "" {
		return nil, nil
	}
	key, err := base64.StdEncoding.DecodeString(updatePublicKey)
	if err != nil || len(key) != 32 {
		return nil, errors.New("invalid Sparkle public key")
	}
	framework, err := loadSparkle()
	if err != nil {
		return nil, err
	}
	if err := verifyBundledUpdaterConfig(); err != nil {
		return nil, err
	}
	u := &sparkleUpdater{
		framework: framework,
		observed: []string{
			"canCheckForUpdates", "automaticallyChecksForUpdates", "automaticallyDownloadsUpdates",
		},
	}
	if err := u.start(requestShutdown, changed); err != nil {
		u.close()
		return nil, err
	}
	return u, nil
}

// loadSparkle opens the Sparkle framework bundled inside the app's Resources.
func loadSparkle() (uintptr, error) {
	executable, err := os.Executable()
	if err != nil {
		return 0, err
	}
	framework, err := purego.Dlopen(
		filepath.Join(filepath.Dir(executable), "..", "Resources", "Sparkle.framework", "Sparkle"),
		purego.RTLD_NOW|purego.RTLD_GLOBAL,
	)
	if err != nil {
		return 0, fmt.Errorf("load bundled Sparkle: %w", err)
	}
	return framework, nil
}

// verifyBundledUpdaterConfig requires Info.plist to carry the injected key and feed,
// because Sparkle reads them from the bundle rather than from the binary.
func verifyBundledUpdaterConfig() error {
	feed := "https://github.com/listenbox/client/releases/latest/download/appcast-" + updatePlatform + ".xml"
	info := objc.ID(objc.GetClass("NSBundle")).
		Send(objc.RegisterName("mainBundle")).
		Send(objc.RegisterName("infoDictionary"))
	for name, expected := range map[string]string{"SUPublicEDKey": updatePublicKey, "SUFeedURL": feed} {
		actual := info.Send(objc.RegisterName("objectForKey:"), nsString(name))
		if actual == 0 ||
			!objc.Send[bool](actual, objc.RegisterName("isEqualToString:"), nsString(expected)) {
			return fmt.Errorf("bundled %s does not match the updater configuration", name)
		}
	}
	return nil
}

// registerDelegateClass defines the Objective-C delegate that reports Sparkle
// state changes and postpones relaunch until the app has shut down.
func (u *sparkleUpdater) registerDelegateClass(requestShutdown, changed func()) (objc.Class, error) {
	return objc.RegisterClass(
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
}

// start creates the Sparkle controller, observes its settings, and starts it.
// The caller releases partially created state with close on error.
func (u *sparkleUpdater) start(requestShutdown, changed func()) error {
	class, err := u.registerDelegateClass(requestShutdown, changed)
	if err != nil {
		return err
	}
	u.delegate = objc.ID(class).Send(objc.RegisterName("new"))
	u.controller = objc.ID(objc.GetClass("SPUStandardUpdaterController")).
		Send(objc.RegisterName("alloc")).
		Send(objc.RegisterName("initWithStartingUpdater:updaterDelegate:userDriverDelegate:"), false, u.delegate, objc.ID(0))
	if u.controller == 0 {
		return errors.New("initialize Sparkle controller")
	}
	u.updater = u.controller.Send(objc.RegisterName("updater"))
	for _, key := range u.observed {
		u.updater.Send(objc.RegisterName("addObserver:forKeyPath:options:context:"),
			u.delegate, nsString(key), uintptr(0), uintptr(0))
	}
	var nativeError objc.ID
	if !objc.Send[bool](u.updater, objc.RegisterName("startUpdater:"), &nativeError) {
		description := nativeError.Send(objc.RegisterName("localizedDescription"))
		return fmt.Errorf(
			"start Sparkle: %s",
			objc.Send[string](description, objc.RegisterName("UTF8String")),
		)
	}
	return nil
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
	invokeNativeBlock(install)
	install.Release()
	return true
}

// nativeBlockLiteral is the head of every block, as fixed by the Block ABI:
// https://clang.llvm.org/docs/Block-ABI-Apple.html#high-level
type nativeBlockLiteral struct {
	_               structs.HostLayout
	isa             uintptr
	flags, reserved int32
	invoke          uintptr
}

// invokeNativeBlock calls a void-returning, argument-less block that
// Objective-C created. purego's Block.Invoke only dispatches blocks made by
// objc.NewBlock and panics on Sparkle's, killing the app before the installer
// is told to relaunch it; the ABI's invoke function takes the block itself.
func invokeNativeBlock(block objc.Block) {
	// The block lives on the Objective-C heap, outside Go's collector.
	literal := *(**nativeBlockLiteral)(unsafe.Pointer(&block))
	purego.SyscallN(literal.invoke, uintptr(block))
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
