package syncengine

/*
#cgo pkg-config: --static libavformat libavdevice libavfilter libavcodec libswresample libswscale libavutil
#cgo linux LDFLAGS: -lstdc++
#cgo darwin windows LDFLAGS: -lc++
#include <libavcodec/avcodec.h>
*/
import "C"

// FFmpegConfiguration identifies the linked, source-built codec library in
// diagnostics. astiav exposes codecs but not avcodec_configuration itself.
func FFmpegConfiguration() string { return C.GoString(C.avcodec_configuration()) }
