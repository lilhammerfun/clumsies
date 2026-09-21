import Foundation

/// Arms a relaunch only after the normal quit guards and pending saves succeed.
@MainActor
final class AppRestartController {
    private var requested = false
    private var exitSignal: FileHandle?
    private let prepareRelaunch: () throws -> FileHandle

    init(prepareRelaunch: @escaping () throws -> FileHandle = { try prepare() }) {
        self.prepareRelaunch = prepareRelaunch
    }

    func request(terminate: () -> Void) {
        guard !requested else { return }
        requested = true
        terminate()
    }

    func finishTermination(allowed: Bool) throws -> Bool {
        defer { requested = false }
        guard allowed else { return false }
        if requested, exitSignal == nil {
            exitSignal = try prepareRelaunch()
        }
        return true
    }

    static func prepare(
        applicationURL: URL = Bundle.main.bundleURL,
        openCommand: URL = URL(fileURLWithPath: "/usr/bin/open")
    ) throws -> FileHandle {
        let signal = Pipe()
        let helper = Process()
        helper.executableURL = URL(fileURLWithPath: "/bin/sh")
        // EOF arrives when this App exits and closes its retained write handle.
        // This avoids a sleep-based race or opening another installed App copy.
        helper.arguments = ["-c", "/bin/cat >/dev/null; exec \"$2\" -n \"$1\"",
                            "clumsies-relaunch", applicationURL.path, openCommand.path]
        helper.standardInput = signal
        helper.standardOutput = FileHandle.nullDevice
        try helper.run()
        signal.fileHandleForReading.closeFile()
        return signal.fileHandleForWriting
    }
}
