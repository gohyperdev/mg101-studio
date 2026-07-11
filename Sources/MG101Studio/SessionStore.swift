import Foundation
import MG101Core
import MG101Tools

enum SessionState: String, Codable, Sendable {
    case active
    case committed
    case reverted
}

struct AgentSessionMetadata: Codable, Sendable, Identifiable {
    let id: String // UUID String
    var title: String
    var createdAt: Date
    var updatedAt: Date
    var provider: String
    var model: String
    var state: SessionState
    var turnCount: Int
    var totalCost: Double
    var approvedRoots: [String]
    var profileHash: String
    var libraryPath: String

    init(
        id: String,
        title: String,
        createdAt: Date,
        updatedAt: Date,
        provider: String,
        model: String,
        state: SessionState,
        turnCount: Int,
        totalCost: Double,
        approvedRoots: [String],
        profileHash: String,
        libraryPath: String
    ) {
        self.id = id
        self.title = title
        self.createdAt = createdAt
        self.updatedAt = updatedAt
        self.provider = provider
        self.model = model
        self.state = state
        self.turnCount = turnCount
        self.totalCost = totalCost
        self.approvedRoots = approvedRoots
        self.profileHash = profileHash
        self.libraryPath = libraryPath
    }
}

@MainActor
final class SessionStore: ObservableObject {
    @Published private(set) var sessions: [AgentSessionMetadata] = []
    private let fileManager = FileManager.default

    init() {
        try? loadAllSessions()
    }

    nonisolated(unsafe) static var directoryOverride: URL?

    static func sessionsDirectory() throws -> URL {
        if let override = directoryOverride {
            return override
        }
        let root = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        return root
            .appendingPathComponent("MG101Studio", isDirectory: true)
            .appendingPathComponent("AgentSessions", isDirectory: true)
    }

    static func sessionDirectory(id: String) throws -> URL {
        return try sessionsDirectory().appendingPathComponent(id, isDirectory: true)
    }

    func loadAllSessions() throws {
        let dir = try Self.sessionsDirectory()
        try fileManager.createDirectory(at: dir, withIntermediateDirectories: true)
        
        let subdirs = try fileManager.contentsOfDirectory(
            at: dir,
            includingPropertiesForKeys: nil,
            options: [.skipsHiddenFiles]
        )
        
        var loaded: [AgentSessionMetadata] = []
        let decoder = JSONDecoder()
        
        for subdir in subdirs {
            let metaURL = subdir.appendingPathComponent("session.json")
            if fileManager.fileExists(atPath: metaURL.path),
               let data = try? Data(contentsOf: metaURL),
               let meta = try? decoder.decode(AgentSessionMetadata.self, from: data) {
                loaded.append(meta)
            }
        }
        
        self.sessions = loaded.sorted { $0.updatedAt > $1.updatedAt }
    }

    func createSession(
        provider: String,
        model: String,
        profileHash: String,
        libraryPath: String
    ) throws -> AgentSessionMetadata {
        let id = UUID().uuidString
        let meta = AgentSessionMetadata(
            id: id,
            title: "New Agent Session",
            createdAt: Date(),
            updatedAt: Date(),
            provider: provider,
            model: model,
            state: .active,
            turnCount: 0,
            totalCost: 0.0,
            approvedRoots: [],
            profileHash: profileHash,
            libraryPath: libraryPath
        )
        
        try saveSessionMetadata(meta)
        try loadAllSessions()
        return meta
    }

    func saveSessionMetadata(_ meta: AgentSessionMetadata) throws {
        let subdir = try Self.sessionDirectory(id: meta.id)
        try fileManager.createDirectory(at: subdir, withIntermediateDirectories: true)
        let metaURL = subdir.appendingPathComponent("session.json")
        let encoder = JSONEncoder()
        encoder.outputFormatting = .prettyPrinted
        let data = try encoder.encode(meta)
        try data.write(to: metaURL, options: .atomic)
    }

    func deleteSession(id: String) throws {
        let subdir = try Self.sessionDirectory(id: id)
        if fileManager.fileExists(atPath: subdir.path) {
            try fileManager.removeItem(at: subdir)
        }
        try loadAllSessions()
    }

    func appendMessage(sessionID: String, message: ChatMessage) throws {
        let subdir = try Self.sessionDirectory(id: sessionID)
        try fileManager.createDirectory(at: subdir, withIntermediateDirectories: true)
        let messagesURL = subdir.appendingPathComponent("messages.jsonl")

        let encoder = JSONEncoder()
        let data = try encoder.encode(message)
        guard var line = String(data: data, encoding: .utf8) else {
            throw NSError(domain: "SessionStore", code: 1, userInfo: [NSLocalizedDescriptionKey: "Encoding failed"])
        }
        line.append("\n")

        if !fileManager.fileExists(atPath: messagesURL.path) {
            try "".write(to: messagesURL, atomically: true, encoding: .utf8)
        }
        let fileHandle = try FileHandle(forWritingTo: messagesURL)
        defer { try? fileHandle.close() }
        try fileHandle.seekToEnd()
        try fileHandle.write(contentsOf: Data(line.utf8))
        try fileHandle.synchronize()
    }

    func loadMessages(sessionID: String) throws -> [ChatMessage] {
        let subdir = try Self.sessionDirectory(id: sessionID)
        let messagesURL = subdir.appendingPathComponent("messages.jsonl")
        guard fileManager.fileExists(atPath: messagesURL.path) else { return [] }

        let content = try String(contentsOf: messagesURL, encoding: .utf8)
        let decoder = JSONDecoder()
        return try content
            .components(separatedBy: "\n")
            .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            .map { line in
                try decoder.decode(ChatMessage.self, from: Data(line.utf8))
            }
    }

    func compactHistory(sessionID: String, keepLast N: Int, summaryText: String) throws {
        let messages = try loadMessages(sessionID: sessionID)
        guard messages.count > N else { return }

        let remaining = Array(messages.suffix(N))
        let summaryMessage = ChatMessage(
            role: "user",
            content: "Summary of earlier work: " + summaryText
        )

        var compacted = [summaryMessage]
        compacted.append(contentsOf: remaining)

        let subdir = try Self.sessionDirectory(id: sessionID)
        let messagesURL = subdir.appendingPathComponent("messages.jsonl")

        let encoder = JSONEncoder()
        var content = ""
        for msg in compacted {
            let data = try encoder.encode(msg)
            if let line = String(data: data, encoding: .utf8) {
                content.append(line + "\n")
            }
        }
        try content.write(to: messagesURL, atomically: true, encoding: .utf8)
    }
}
