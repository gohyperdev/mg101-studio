import Foundation
import CryptoKit
import MG101Core
import MG101Tools

public enum EntryState: String, Codable, Sendable {
    case prepared
    case committed
}

public struct StagedMetadata: Codable, Sendable, Equatable {
    public let origin: String
    public let sourceName: String
    public let revision: Int
    public let fileName: String

    public init(origin: String, sourceName: String, revision: Int, fileName: String) {
        self.origin = origin
        self.sourceName = sourceName
        self.revision = revision
        self.fileName = fileName
    }
}

public enum InverseOperation: Codable, Sendable, Equatable {
    case restoreBytes(patchID: PatchID, blob: Data)
    case removePatch(patchID: PatchID)
    case restoreFromStaging(patchID: PatchID, meta: StagedMetadata)
}

public struct TransactionEntry: Codable, Sendable {
    public let sequence: Int
    public let timestamp: Date
    public let toolName: String
    public let patchID: PatchID
    public let revisionBefore: Int
    public let inverse: InverseOperation
    public let beforeHash: String // SHA256 in hex
    public let afterHash: String  // SHA256 in hex
    public var state: EntryState

    public init(
        sequence: Int,
        timestamp: Date,
        toolName: String,
        patchID: PatchID,
        revisionBefore: Int,
        inverse: InverseOperation,
        beforeHash: String,
        afterHash: String,
        state: EntryState
    ) {
        self.sequence = sequence
        self.timestamp = timestamp
        self.toolName = toolName
        self.patchID = patchID
        self.revisionBefore = revisionBefore
        self.inverse = inverse
        self.beforeHash = beforeHash
        self.afterHash = afterHash
        self.state = state
    }
}

extension Data {
    public func sha256Hex() -> String {
        let hash = SHA256.hash(data: self)
        return hash.compactMap { String(format: "%02x", $0) }.joined()
    }
}

public final class WALJournal: Sendable {
    public let journalURL: URL

    public init(url: URL) {
        self.journalURL = url
    }

    public func append(_ entry: TransactionEntry) throws {
        let data = try JSONEncoder().encode(entry)
        guard var line = String(data: data, encoding: .utf8) else {
            throw NSError(domain: "WALJournal", code: 1, userInfo: [NSLocalizedDescriptionKey: "Encoding failed"])
        }
        line.append("\n")

        let directory = journalURL.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        if !FileManager.default.fileExists(atPath: journalURL.path) {
            try "".write(to: journalURL, atomically: true, encoding: .utf8)
        }
        let fileHandle = try FileHandle(forWritingTo: journalURL)
        defer { try? fileHandle.close() }
        try fileHandle.seekToEnd()
        try fileHandle.write(contentsOf: Data(line.utf8))
        try fileHandle.synchronize() // macOS fsync
    }

    public func loadEntries() throws -> [TransactionEntry] {
        guard FileManager.default.fileExists(atPath: journalURL.path) else { return [] }
        let content = try String(contentsOf: journalURL, encoding: .utf8)
        let decoder = JSONDecoder()
        return try content
            .components(separatedBy: "\n")
            .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            .map { line in
                try decoder.decode(TransactionEntry.self, from: Data(line.utf8))
            }
    }

    public func rewrite(_ entries: [TransactionEntry]) throws {
        let encoder = JSONEncoder()
        var content = ""
        for entry in entries {
            let data = try encoder.encode(entry)
            if let line = String(data: data, encoding: .utf8) {
                content.append(line + "\n")
            }
        }
        try content.write(to: journalURL, atomically: true, encoding: .utf8)
    }
}
