package syncengine

import "os"

func durableSync(file *os.File) error { return file.Sync() }
