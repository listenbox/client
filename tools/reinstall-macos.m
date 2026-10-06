#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static NSString *const bundleID = @"app.listenbox.client";

static NSString *run(NSString *executable, NSArray<NSString *> *arguments) {
    NSTask *process = [NSTask new];
    process.executableURL = [NSURL fileURLWithPath:executable];
    process.arguments = arguments;
    NSError *error = nil;
    if (![process launchAndReturnError:&error]) return error.localizedDescription;
    [process waitUntilExit];
    if (process.terminationStatus != 0) {
        return [NSString stringWithFormat:@"%@ failed with status %d", executable, process.terminationStatus];
    }
    return nil;
}

static NSString *validateBundle(NSURL *url) {
    NSNumber *symbolicLink = nil;
    NSError *error = nil;
    if (![url getResourceValue:&symbolicLink forKey:NSURLIsSymbolicLinkKey error:&error]) {
        return error.localizedDescription;
    }
    if (symbolicLink.boolValue || ![[NSBundle bundleWithURL:url].bundleIdentifier isEqualToString:bundleID]) {
        return [NSString stringWithFormat:@"Expected a Listenbox application bundle at %@", url.path];
    }
    return nil;
}

static NSString *stopInstalledApp(NSURL *destination, NSTimeInterval timeout) {
    NSMutableArray<NSRunningApplication *> *applications = [NSMutableArray new];
    for (NSRunningApplication *application in [NSRunningApplication runningApplicationsWithBundleIdentifier:bundleID]) {
        if ([application.bundleURL.URLByResolvingSymlinksInPath isEqual:destination.URLByResolvingSymlinksInPath]) {
            [applications addObject:application];
        }
    }
    for (NSRunningApplication *application in applications) {
        // SIGTERM enters the same graceful Quit action as the tray menu. Only
        // signal the installed bundle, never a dev build or another checkout.
        if (!application.isTerminated && kill(application.processIdentifier, SIGTERM) != 0 && errno != ESRCH) {
            return [NSString stringWithFormat:@"Cannot ask Listenbox to quit: %s", strerror(errno)];
        }
    }
    NSTimeInterval deadline = NSProcessInfo.processInfo.systemUptime + timeout;
    for (;;) {
        BOOL running = NO;
        for (NSRunningApplication *application in applications) {
            if (!application.isTerminated) {
                running = YES;
                break;
            }
        }
        if (!running) return nil;
        if (NSProcessInfo.processInfo.systemUptime >= deadline) {
            return @"Listenbox is still shutting down; the installed app was not replaced. Run the task again after it quits.";
        }
        // AppKit refreshes isTerminated as this thread's run loop advances.
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.05]];
    }
}

static NSString *reinstall(NSURL *source, NSURL *destination) {
    NSFileManager *files = NSFileManager.defaultManager;
    if ([source isEqual:destination]) return @"Source and destination must be different bundles";
    NSString *failure = validateBundle(source);
    if (failure) return failure;
    if ([files fileExistsAtPath:destination.path]) {
        failure = validateBundle(destination);
        if (failure) return failure;
    }
    // Stage on the destination volume. The old app survives copy/signature
    // failures, interrupted staging, and an app that cannot finish quitting.
    NSURL *staging = [destination.URLByDeletingLastPathComponent
        URLByAppendingPathComponent:[@".listenbox-reinstall-" stringByAppendingString:NSUUID.UUID.UUIDString]
        isDirectory:YES];
    NSError *error = nil;
    if (![files createDirectoryAtURL:staging withIntermediateDirectories:NO attributes:nil error:&error]) {
        return error.localizedDescription;
    }
    @try {
        NSURL *replacement = [staging URLByAppendingPathComponent:@"Listenbox.app" isDirectory:YES];
        failure = run(@"/usr/bin/ditto", @[source.path, replacement.path]);
        if (failure) return failure;
        failure = run(@"/usr/bin/codesign", @[@"--verify", @"--strict", replacement.path]);
        if (failure) return failure;
        failure = stopInstalledApp(destination, 30);
        if (failure) return failure;

        // A swap leaves the old bundle in our staging directory for cleanup.
        // There is no interval with a missing or partially copied installed app.
        unsigned int flags = [files fileExistsAtPath:destination.path] ? RENAME_SWAP : RENAME_EXCL;
        if (renamex_np(replacement.fileSystemRepresentation, destination.fileSystemRepresentation, flags) != 0) {
            return [NSString stringWithFormat:@"Cannot install Listenbox: %s", strerror(errno)];
        }
        return nil;
    } @finally {
        if (![files removeItemAtURL:staging error:&error]) {
            fprintf(stderr, "Could not remove staging directory %s: %s\n", staging.fileSystemRepresentation,
                error.localizedDescription.UTF8String);
        }
    }
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc != 3) {
            fputs("Usage: reinstall-macos SOURCE.app DESTINATION.app\n", stderr);
            return 1;
        }
        NSFileManager *files = NSFileManager.defaultManager;
        NSString *sourcePath = [files stringWithFileSystemRepresentation:argv[1] length:strlen(argv[1])];
        NSString *destinationPath = [files stringWithFileSystemRepresentation:argv[2] length:strlen(argv[2])];
        if (!sourcePath || !destinationPath) {
            fputs("Source and destination must be valid filesystem paths\n", stderr);
            return 1;
        }
        NSURL *source = [NSURL fileURLWithPath:sourcePath].URLByStandardizingPath;
        NSURL *destination = [NSURL fileURLWithPath:destinationPath].URLByStandardizingPath;
        NSString *failure = reinstall(source, destination);
        if (failure) {
            fprintf(stderr, "%s\n", failure.UTF8String);
            return 1;
        }
        printf("Installed %s\n", destination.fileSystemRepresentation);
        return 0;
    }
}
