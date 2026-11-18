package main

type nativeUpdater interface {
	canCheck() bool
	check()
	automatic() bool
	setAutomatic(enabled bool)
	automaticDownloads() bool
	setAutomaticDownloads(enabled bool)
	drained() bool
	close()
}
