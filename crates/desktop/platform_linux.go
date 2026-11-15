package main

func installationLock() (func(), error) { return func() {}, nil }

//nolint:ireturn // Each platform constructs the common native updater interface.
func startUpdater(func(), func()) (nativeUpdater, error) {
	return nil, nil
}
