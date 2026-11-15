//go:build linux || darwin

package syncengine

/*
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
static int listenbox_full_sync(int fd) {
#ifdef __APPLE__
    // F_FULLFSYNC also flushes the drive's write cache.
    if (fcntl(fd, F_FULLFSYNC) == 0) return 0;
    if (errno != ENOTSUP && errno != EINVAL) return -1;
#endif
    return fsync(fd);
}
*/
import "C"

import "os"

// Prepared files cross the same native durability boundary on both Unix
// platforms. This also keeps filesystem interposition and tracing effective.
func durableSync(file *os.File) error {
	result, err := C.listenbox_full_sync(C.int(file.Fd()))
	if result != 0 {
		return &os.PathError{Op: "sync", Path: file.Name(), Err: err}
	}
	return nil
}
