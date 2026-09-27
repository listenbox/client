import AppKit
import Darwin
import Foundation

// A windowless app whose admitted work finishes only when the test releases it.
@main
struct InstalledAppFixture {
    static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.prohibited)
        signal(SIGTERM, SIG_IGN)
        let termination = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .global())
        termination.setEventHandler {
            FileHandle.standardOutput.write(Data("S".utf8))
            guard FileHandle.standardInput.readData(ofLength: 1) == Data("G".utf8) else {
                exit(2)
            }
            FileHandle.standardOutput.write(Data("D".utf8))
            exit(0)
        }
        termination.resume()
        app.finishLaunching()
        FileHandle.standardOutput.write(Data("R".utf8))
        withExtendedLifetime(termination) { app.run() }
    }
}
