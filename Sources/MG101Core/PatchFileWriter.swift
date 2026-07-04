import Foundation

public enum PatchFileWriter {
    public static func writeNew(_ data: Data, to destination: URL) throws {
        let manager = FileManager.default
        guard !manager.fileExists(atPath: destination.path) else {
            throw CocoaError(.fileWriteFileExists)
        }

        let directory = destination.deletingLastPathComponent()
        let temporary = directory.appendingPathComponent(
            ".\(destination.lastPathComponent).\(UUID().uuidString).tmp"
        )
        defer { try? manager.removeItem(at: temporary) }

        try data.write(to: temporary, options: .atomic)
        try manager.moveItem(at: temporary, to: destination)
    }
}
