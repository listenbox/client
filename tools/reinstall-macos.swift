import AppKit
import Darwin
import Foundation

private let bundleID = "app.listenbox.client"

struct InstallError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

func run(_ executable: String, _ arguments: [String]) throws {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    try process.run()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
        throw InstallError("\(executable) failed with status \(process.terminationStatus)")
    }
}

func validateBundle(_ url: URL) throws {
    guard try url.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink != true,
          Bundle(url: url)?.bundleIdentifier == bundleID else {
        throw InstallError("Expected a Listenbox application bundle at \(url.path)")
    }
}

func stopInstalledApp(_ destination: URL, timeout: TimeInterval) throws {
    let applications = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
        .filter { $0.bundleURL?.resolvingSymlinksInPath() == destination.resolvingSymlinksInPath() }
    for application in applications where !application.isTerminated {
        // SIGTERM enters the same graceful Quit action as the tray menu. Only
        // signal the installed bundle, never a dev build or another checkout.
        if kill(application.processIdentifier, SIGTERM) != 0 && errno != ESRCH {
            throw InstallError("Cannot ask Listenbox to quit: \(String(cString: strerror(errno)))")
        }
    }
    let deadline = ProcessInfo.processInfo.systemUptime + timeout
    while applications.contains(where: { !$0.isTerminated }) {
        guard ProcessInfo.processInfo.systemUptime < deadline else {
            throw InstallError("Listenbox is still shutting down; the installed app was not replaced. Run the task again after it quits.")
        }
        // AppKit refreshes isTerminated as this thread's run loop advances.
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
    }
}

func reinstall(source: URL, destination: URL, shutdownTimeout: TimeInterval = 30) throws {
    let files = FileManager.default
    guard source.standardizedFileURL != destination.standardizedFileURL else {
        throw InstallError("Source and destination must be different bundles")
    }
    try validateBundle(source)
    if files.fileExists(atPath: destination.path) {
        try validateBundle(destination)
    }
    // Stage on the destination volume. The old app survives copy/signature
    // failures, interrupted staging, and an app that cannot finish quitting.
    let staging = destination.deletingLastPathComponent()
        .appendingPathComponent(".listenbox-reinstall-\(UUID().uuidString)", isDirectory: true)
    try files.createDirectory(at: staging, withIntermediateDirectories: false)
    defer {
        do { try files.removeItem(at: staging) }
        catch { fputs("Could not remove staging directory \(staging.path): \(error)\n", stderr) }
    }
    let replacement = staging.appendingPathComponent("Listenbox.app", isDirectory: true)
    try run("/usr/bin/ditto", [source.path, replacement.path])
    try run("/usr/bin/codesign", ["--verify", "--strict", replacement.path])
    try stopInstalledApp(destination, timeout: shutdownTimeout)

    // A swap leaves the old bundle in our staging directory for cleanup. There
    // is no interval with a missing or partially copied installed application.
    let flags = files.fileExists(atPath: destination.path) ? RENAME_SWAP : RENAME_EXCL
    guard renamex_np(replacement.path, destination.path, UInt32(flags)) == 0 else {
        throw InstallError("Cannot install Listenbox: \(String(cString: strerror(errno)))")
    }
    print("Installed \(destination.path)")
}

#if !REINSTALL_TESTS
@main
struct ReinstallMacOS {
    static func main() {
        do {
            guard CommandLine.arguments.count == 3 else {
                throw InstallError("Usage: reinstall-macos SOURCE.app DESTINATION.app")
            }
            try reinstall(
                source: URL(fileURLWithPath: CommandLine.arguments[1]).standardizedFileURL,
                destination: URL(fileURLWithPath: CommandLine.arguments[2]).standardizedFileURL
            )
        } catch {
            fputs("\(error)\n", stderr)
            exit(1)
        }
    }
}
#endif
