#import <Cocoa/Cocoa.h>
#import <Sparkle/Sparkle.h>
#import <objc/runtime.h>

static void (*eventCallback)(int);
static BOOL terminationPermitted;

static NSApplicationTerminateReply shouldTerminate(id object, SEL selector, NSApplication *application) {
    if (terminationPermitted) return NSTerminateNow;
    eventCallback(2);
    return NSTerminateCancel;
}

@interface ListenboxUpdater : NSObject <SPUUpdaterDelegate>
@property(nonatomic, strong) SPUStandardUpdaterController *controller;
@property(nonatomic, copy) void (^installHandler)(void);
@end

@implementation ListenboxUpdater
- (void)observeValueForKeyPath:(NSString *)keyPath ofObject:(id)object
                       change:(NSDictionary *)change context:(void *)context {
    if ([keyPath isEqualToString:@"canCheckForUpdates"]) eventCallback(1);
}
- (BOOL)updater:(SPUUpdater *)updater shouldPostponeRelaunchForUpdate:(SUAppcastItem *)item
    untilInvokingBlock:(void (^)(void))installHandler {
    self.installHandler = installHandler;
    eventCallback(2);
    return YES;
}
@end

static ListenboxUpdater *delegate;

int listenbox_updater_init(const char *key, const char *feed, void (*callback)(int)) {
    NSCAssert([NSThread isMainThread], @"Updater must be initialized on the main thread");
    NSDictionary *info = NSBundle.mainBundle.infoDictionary;
    if (![info[@"SUPublicEDKey"] isEqualToString:@(key)] ||
        ![info[@"SUFeedURL"] isEqualToString:@(feed)]) return 0;
    eventCallback = callback;
    // Sparkle may omit its postponement callback when an update is already
    // staged for installation on quit. Every Cocoa termination must drain work.
    if (!class_addMethod([NSApp.delegate class], @selector(applicationShouldTerminate:),
            (IMP)shouldTerminate, "Q@:@")) return 0;
    delegate = [ListenboxUpdater new];
    delegate.controller = [[SPUStandardUpdaterController alloc]
        initWithStartingUpdater:NO updaterDelegate:delegate userDriverDelegate:nil];
    [delegate.controller.updater addObserver:delegate forKeyPath:@"canCheckForUpdates"
        options:NSKeyValueObservingOptionInitial | NSKeyValueObservingOptionNew context:NULL];
    NSError *error = nil;
    if (![delegate.controller.updater startUpdater:&error]) {
        NSLog(@"Cannot start Listenbox updater: %@", error);
        return 0;
    }
    return 1;
}
int listenbox_updater_can_check(void) { return delegate.controller.updater.canCheckForUpdates; }
int listenbox_updater_automatic(void) { return delegate.controller.updater.automaticallyChecksForUpdates; }
void listenbox_updater_set_automatic(int enabled) {
    delegate.controller.updater.automaticallyChecksForUpdates = enabled != 0;
}
void listenbox_updater_check(void) { [delegate.controller checkForUpdates:nil]; }
void listenbox_updater_drained(void) {
    terminationPermitted = YES;
    void (^handler)(void) = delegate.installHandler;
    delegate.installHandler = nil;
    if (handler) handler();
    else [NSApp terminate:nil];
}
void listenbox_updater_permit_termination(void) { terminationPermitted = YES; }
void listenbox_updater_cleanup(void) {
    [delegate.controller.updater removeObserver:delegate forKeyPath:@"canCheckForUpdates"];
    delegate = nil;
}
