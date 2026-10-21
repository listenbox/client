// Launch Services -> first displayed Metal drawable, on the same host clock.
#import <AppKit/AppKit.h>
#import <QuartzCore/QuartzCore.h>
#include <fcntl.h>
#include <notify.h>
#include <signal.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc < 4 || argc > 5) {
            fputs("Usage: startup-launch APP OBSERVER.dylib EVENTS_FIFO [TLS_FIFO]\n", stderr);
            return 64;
        }
        NSURL *bundle = [NSURL fileURLWithPath:@(argv[1])].URLByStandardizingPath;
        NSBundle *info = [NSBundle bundleWithURL:bundle];
        if (!info.bundleIdentifier) return 1;
        for (NSRunningApplication *running in [NSRunningApplication runningApplicationsWithBundleIdentifier:info.bundleIdentifier]) {
            if ([running.bundleURL.URLByResolvingSymlinksInPath isEqual:bundle.URLByResolvingSymlinksInPath]) {
                fputs("The capture bundle is already running. Quit it before profiling startup.\n", stderr);
                return 1;
            }
        }
        int eventFD = open(argv[3], O_RDWR | O_NONBLOCK | O_CLOEXEC);
        if (eventFD < 0) { perror("open startup events"); return 1; }
        // Keeping this writer open blocks certificate reads until the first
        // displayed frame. On failure the timeout releases it for clean shutdown.
        __block int tlsFD = argc == 5 ? open(argv[4], O_RDWR | O_NONBLOCK | O_CLOEXEC) : -1;
        if (argc == 5 && tlsFD < 0) { perror("open certificate gate"); return 1; }
        NSData *certificate = nil;
        if (argc == 5) {
            NSString *roots = [NSString stringWithContentsOfFile:@"/etc/ssl/cert.pem" encoding:NSUTF8StringEncoding error:nil];
            NSRange begin = [roots rangeOfString:@"-----BEGIN CERTIFICATE-----"];
            NSRange end = [roots rangeOfString:@"-----END CERTIFICATE-----"];
            if (!roots || begin.location == NSNotFound || end.location == NSNotFound) return 1;
            certificate = [[roots substringWithRange:NSMakeRange(begin.location, NSMaxRange(end) - begin.location)] dataUsingEncoding:NSUTF8StringEncoding];
            if (certificate.length > 4096) return 1;
        }
        __block NSRunningApplication *application = nil;
        __block NSDictionary *frame = nil;
        __block BOOL stopping = NO;
        __block BOOL timedOut = NO;
        __block double requestTime = 0;
        __block double launchedTime = 0;
        void (^stop)(void) = ^{
            if (!application || stopping) return;
            stopping = YES;
            if (tlsFD >= 0) {
                // Replace the pathname before waking its current reader. Later
                // clients must read a regular, valid PEM instead of blocking
                // on another FIFO open or panicking on a removed file.
                NSString *released = [@(argv[4]) stringByAppendingString:@".released"];
                if (![certificate writeToFile:released atomically:NO] || rename(released.fileSystemRepresentation, argv[4]) != 0) {
                    perror("release certificate gate"); exit(1);
                }
                if (write(tlsFD, certificate.bytes, certificate.length) != (ssize_t)certificate.length) {
                    perror("write certificate gate"); exit(1);
                }
                close(tlsFD);
                tlsFD = -1;
            }
            if (kill(application.processIdentifier, SIGTERM) != 0) { perror("quit captured app"); exit(1); }
        };
        NSNotificationCenter *notifications = NSWorkspace.sharedWorkspace.notificationCenter;
        [notifications addObserverForName:NSWorkspaceDidTerminateApplicationNotification object:nil queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *notification) {
            NSRunningApplication *ended = notification.userInfo[NSWorkspaceApplicationKey];
            if (!application || ended.processIdentifier != application.processIdentifier) return;
            if (!frame || timedOut) exit(1);
            double presentedTime = [frame[@"presented_uptime_ms"] doubleValue];
            NSDictionary *result = @{
                @"pid": @(application.processIdentifier),
                @"bundle": bundle.path,
                @"launch_request_uptime_ms": @(requestTime),
                @"application_opened_uptime_ms": @(launchedTime),
                @"first_presented_uptime_ms": @(presentedTime),
                @"request_to_first_presented_ms": @(presentedTime - requestTime),
                @"request_to_observer_loaded_ms": @([frame[@"observer_loaded_uptime_ms"] doubleValue] - requestTime),
                @"request_to_first_drawable_ms": @([frame[@"first_drawable_uptime_ms"] doubleValue] - requestTime),
                @"certificate_gate": argc == 5 ? @YES : @NO,
                @"measurement": @"Launch Services request to Metal drawable presentedTime",
            };
            NSData *json = [NSJSONSerialization dataWithJSONObject:result options:NSJSONWritingPrettyPrinted error:nil];
            fwrite(json.bytes, 1, json.length, stdout);
            fputc('\n', stdout);
            exit(0);
        }];
        dispatch_source_t input = dispatch_source_create(DISPATCH_SOURCE_TYPE_READ, (uintptr_t)eventFD, 0, dispatch_get_main_queue());
        NSMutableData *buffer = [NSMutableData new];
        dispatch_source_set_event_handler(input, ^{
            char bytes[512];
            ssize_t length;
            while ((length = read(eventFD, bytes, sizeof(bytes))) > 0) {
                [buffer appendBytes:bytes length:(NSUInteger)length];
            }
            if (!buffer.length || ((const char *)buffer.bytes)[buffer.length - 1] != '\n') return;
            NSDictionary *event = [NSJSONSerialization JSONObjectWithData:buffer options:0 error:nil];
            if (![event isKindOfClass:NSDictionary.class] || [event[@"presented_uptime_ms"] doubleValue] <= requestTime) {
                fprintf(stderr, "Invalid first-presentation event (request %.6f): %s\n", requestTime,
                    [[NSString alloc] initWithData:buffer encoding:NSUTF8StringEncoding].UTF8String);
                timedOut = YES;
                stop();
                return;
            }
            frame = event;
            if (application && [frame[@"pid"] intValue] != application.processIdentifier) {
                fputs("Presentation belongs to a different process\n", stderr);
                timedOut = YES;
            }
            stop();
        });
        dispatch_resume(input);
        NSWorkspaceOpenConfiguration *configuration = NSWorkspaceOpenConfiguration.configuration;
        configuration.createsNewApplicationInstance = YES;
        configuration.activates = YES;
        configuration.addsToRecentItems = NO;
        configuration.promptsUserIfNeeded = NO;
        NSMutableDictionary *environment = [NSProcessInfo.processInfo.environment mutableCopy];
        environment[@"DYLD_INSERT_LIBRARIES"] = @(argv[2]);
        environment[@"LISTENBOX_STARTUP_EVENTS"] = @(argv[3]);
        if (argc == 5) environment[@"SSL_CERT_FILE"] = @(argv[4]);
        configuration.environment = environment;
        void (^launch)(void) = ^{
            requestTime = CACurrentMediaTime() * 1000;
            [NSWorkspace.sharedWorkspace openApplicationAtURL:bundle configuration:configuration completionHandler:^(NSRunningApplication *opened, NSError *error) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (error) { fprintf(stderr, "%s\n", error.localizedDescription.UTF8String); exit(1); }
                    application = opened;
                    launchedTime = CACurrentMediaTime() * 1000;
                    if (frame && [frame[@"pid"] intValue] != application.processIdentifier) timedOut = YES;
                    if (frame || timedOut) stop();
                });
            }];
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                if (!frame) {
                    fputs("No displayed Metal frame within 5 seconds; startup capture failed.\n", stderr);
                    timedOut = YES;
                    stop();
                }
            });
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 9 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                fputs("Captured app did not finish graceful shutdown; inspect its diagnostics.\n", stderr);
                if (tlsFD >= 0) close(tlsFD);
                exit(1);
            });
        };
        const char *notification = getenv("LISTENBOX_STARTUP_NOTIFY");
        if (notification) {
            int token;
            if (notify_register_dispatch(notification, &token, dispatch_get_main_queue(), ^(int received) {
                notify_cancel(received);
                launch();
            }) != NOTIFY_STATUS_OK) return 1;
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 15 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
                if (!requestTime) { fputs("Instruments did not start recording\n", stderr); exit(1); }
            });
            const char *ready = getenv("LISTENBOX_STARTUP_READY");
            if (!ready) return 1;
            int output = open(ready, O_WRONLY | O_CLOEXEC);
            if (output < 0 || write(output, "ready\n", 6) != 6) return 1;
            close(output);
        } else {
            launch();
        }
        [NSRunLoop.currentRunLoop run];
    }
    return 1;
}
