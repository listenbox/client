// Measurement-only injection into the locally signed profiling bundle.
// Metal's presentedTime measures pixels reaching the display, not submission.
#import <Foundation/Foundation.h>
#import <QuartzCore/CAMetalLayer.h>
#import <Metal/Metal.h>
#import <objc/runtime.h>
#include <fcntl.h>
#include <stdatomic.h>
#include <unistd.h>

static int events = -1;
static atomic_bool observed;
static double loadedTime;
static double acquiredTime;
static id<CAMetalDrawable> (*nextDrawable)(id, SEL);

static id<CAMetalDrawable> measuredNextDrawable(id layer, SEL selector) {
    if (!acquiredTime) acquiredTime = CACurrentMediaTime() * 1000;
    id<CAMetalDrawable> drawable = nextDrawable(layer, selector);
    if (drawable && !atomic_load(&observed)) {
        [drawable addPresentedHandler:^(id<MTLDrawable> presented) {
            // Discarded drawables also invoke the handler, with time zero.
            if (presented.presentedTime <= 0 || atomic_exchange(&observed, true)) return;
            char message[512];
            int length = snprintf(message, sizeof(message),
                "{\"pid\":%d,\"presented_uptime_ms\":%.6f,\"observer_loaded_uptime_ms\":%.6f,\"first_drawable_uptime_ms\":%.6f}\n",
                getpid(), presented.presentedTime * 1000, loadedTime, acquiredTime);
            if (length > 0 && length < (int)sizeof(message)) {
                (void)write(events, message, (size_t)length);
            }
        }];
    }
    return drawable;
}

__attribute__((constructor)) static void observeStartup(void) {
    loadedTime = CACurrentMediaTime() * 1000;
    const char *path = getenv("LISTENBOX_STARTUP_EVENTS");
    if (!path) return;
    events = open(path, O_WRONLY | O_NONBLOCK | O_CLOEXEC);
    if (events < 0) return;
    Method method = class_getInstanceMethod(CAMetalLayer.class, @selector(nextDrawable));
    nextDrawable = (void *)method_setImplementation(method, (IMP)measuredNextDrawable);
}
