import AppKit
import Foundation

private let files = FileManager.default

private func expect(_ condition: Bool, _ message: String) {
    guard condition else { fatalError(message) }
}

private func makeBundle(_ url: URL, fixture: URL, revision: String) throws {
    let contents = url.appendingPathComponent("Contents")
    try files.createDirectory(at: contents.appendingPathComponent("MacOS"), withIntermediateDirectories: true)
    try files.createDirectory(at: contents.appendingPathComponent("Resources"), withIntermediateDirectories: true)
    try files.copyItem(at: fixture, to: contents.appendingPathComponent("MacOS/listenbox-desktop"))
    let plist: [String: String] = [
        "CFBundleIdentifier": "app.listenbox.client",
        "CFBundleExecutable": "listenbox-desktop",
        "CFBundleName": "Listenbox Installer Test",
        "CFBundlePackageType": "APPL",
    ]
    try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0)
        .write(to: contents.appendingPathComponent("Info.plist"))
    try revision.write(to: contents.appendingPathComponent("Resources/revision"), atomically: true, encoding: .utf8)
    if revision == "old" {
        try Data().write(to: contents.appendingPathComponent("Resources/obsolete"))
    }
    try run("/usr/bin/codesign", ["--force", "--sign", "-", url.path])
}

private func revision(_ bundle: URL) throws -> String {
    try String(contentsOf: bundle.appendingPathComponent("Contents/Resources/revision"), encoding: .utf8)
}

private struct RunningFixture {
    let process: Process
    let input = Pipe()
    let output = Pipe()

    init(_ bundle: URL) throws {
        process = Process()
        process.executableURL = bundle.appendingPathComponent("Contents/MacOS/listenbox-desktop")
        process.standardInput = input
        process.standardOutput = output
        try process.run()
        expect(output.fileHandleForReading.readData(ofLength: 1) == Data("R".utf8), "app never became ready")
        expect(NSRunningApplication.runningApplications(withBundleIdentifier: "app.listenbox.client")
            .contains { $0.processIdentifier == process.processIdentifier }, "fixture not registered with AppKit")
    }

    func release() {
        input.fileHandleForWriting.write(Data("G".utf8))
    }

    func cleanUp() {
        if process.isRunning {
            process.terminate()
            release()
            process.waitUntilExit()
        }
    }
}

private func scenario(_ name: String, _ body: (URL) throws -> Void) throws {
    // Every state gate has a hard failure guard, independent of production time.
    let timeout = DispatchWorkItem { fatalError("\(name) did not finish in 5 seconds") }
    DispatchQueue.global().asyncAfter(deadline: .now() + 5, execute: timeout)
    defer { timeout.cancel() }
    let root = files.temporaryDirectory.appendingPathComponent("listenbox-install-test-\(UUID().uuidString)")
    try files.createDirectory(at: root, withIntermediateDirectories: false)
    defer { try? files.removeItem(at: root) }
    try body(root)
    expect(try files.contentsOfDirectory(atPath: root.path).allSatisfy { !$0.hasPrefix(".listenbox-reinstall-") }, "staging leaked")
    print("PASS \(name)")
}

@main
struct ReinstallTests {
    static func main() throws {
        let fixture = URL(fileURLWithPath: CommandLine.arguments[1]).standardizedFileURL
        try scenario("first install and replacement remove obsolete files") { root in
            let source = root.appendingPathComponent("Source.app")
            let destination = root.appendingPathComponent("Listenbox.app")
            try makeBundle(source, fixture: fixture, revision: "old")
            try reinstall(source: source, destination: destination)
            expect(try revision(destination) == "old", "first install missing")
            let update = root.appendingPathComponent("Update.app")
            try makeBundle(update, fixture: fixture, revision: "new")
            try reinstall(source: update, destination: destination)
            expect(try revision(destination) == "new", "replacement missing")
            expect(!files.fileExists(atPath: destination.appendingPathComponent("Contents/Resources/obsolete").path), "replacement merged old files")
            try run("/usr/bin/codesign", ["--verify", "--strict", destination.path])
        }
        try scenario("invalid update preserves installed bundle") { root in
            let source = root.appendingPathComponent("Source.app")
            let destination = root.appendingPathComponent("Listenbox.app")
            try makeBundle(source, fixture: fixture, revision: "new")
            try makeBundle(destination, fixture: fixture, revision: "old")
            try Data("tampered".utf8).write(to: source.appendingPathComponent("Contents/Resources/revision"))
            do {
                try reinstall(source: source, destination: destination)
                fatalError("invalid signature accepted")
            } catch {
                expect(String(describing: error).contains("/usr/bin/codesign failed"), "unexpected failure: \(error)")
            }
            expect(try revision(destination) == "old", "failed update changed installed app")
        }
        try scenario("waits for graceful exit and preserves other copies") { root in
            let source = root.appendingPathComponent("Source.app")
            let destination = root.appendingPathComponent("Listenbox.app")
            let other = root.appendingPathComponent("Other.app")
            try makeBundle(source, fixture: fixture, revision: "new")
            try makeBundle(destination, fixture: fixture, revision: "old")
            try makeBundle(other, fixture: fixture, revision: "old")
            let installed = try RunningFixture(destination)
            defer { installed.cleanUp() }
            let unrelated = try RunningFixture(other)
            defer { unrelated.cleanUp() }
            let drained = DispatchSemaphore(value: 0)
            DispatchQueue.global().async {
                expect(installed.output.fileHandleForReading.readData(ofLength: 1) == Data("S".utf8), "graceful stop not requested")
                expect(try! revision(destination) == "old", "replaced app before work drained")
                expect(unrelated.process.isRunning, "stopped another copy")
                installed.release()
                expect(installed.output.fileHandleForReading.readData(ofLength: 1) == Data("D".utf8), "admitted work did not finish")
                drained.signal()
            }
            try reinstall(source: source, destination: destination)
            expect(drained.wait(timeout: .now() + 1) == .success, "drain not observed")
            installed.process.waitUntilExit()
            expect(installed.process.terminationStatus == 0, "app did not exit gracefully")
            expect(try revision(destination) == "new", "update missing")
            expect(unrelated.process.isRunning, "stopped another copy")
        }
        try scenario("shutdown deadline preserves the old app without force-killing") { root in
            let source = root.appendingPathComponent("Source.app")
            let destination = root.appendingPathComponent("Listenbox.app")
            try makeBundle(source, fixture: fixture, revision: "new")
            try makeBundle(destination, fixture: fixture, revision: "old")
            let installed = try RunningFixture(destination)
            defer { installed.cleanUp() }
            do {
                try reinstall(source: source, destination: destination, shutdownTimeout: 0)
                fatalError("shutdown deadline ignored")
            } catch {
                expect(String(describing: error).contains("still shutting down"), "unexpected failure: \(error)")
            }
            expect(installed.output.fileHandleForReading.readData(ofLength: 1) == Data("S".utf8), "graceful stop not requested")
            expect(try revision(destination) == "old", "replaced app during incomplete shutdown")
            expect(installed.process.isRunning, "force-killed the app")
        }
    }
}
