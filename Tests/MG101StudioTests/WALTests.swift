import Foundation
import Testing
@testable import MG101Studio
import MG101Core
import MG101Tools

@MainActor
struct WALTests {
    @Test func testWALJournalWriteAndLoad() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let journalURL = tempDir.appendingPathComponent("journal.jsonl")
        let journal = WALJournal(url: journalURL)

        let entry = TransactionEntry(
            sequence: 0,
            timestamp: Date(),
            toolName: "test",
            patchID: "factory-01",
            revisionBefore: 1,
            inverse: .restoreBytes(patchID: "factory-01", blob: Data(repeating: 0, count: 8402)),
            beforeHash: "hash1",
            afterHash: "hash2",
            state: .prepared
        )

        try journal.append(entry)

        let entries = try journal.loadEntries()
        #expect(entries.count == 1)
        #expect(entries[0].toolName == "test")
        #expect(entries[0].state == .prepared)

        var commit = entry
        commit.state = .committed
        try journal.append(commit)

        let updatedEntries = try journal.loadEntries()
        #expect(updatedEntries.count == 2)
        #expect(updatedEntries[1].state == .committed)
    }

    @Test func testInverseOperationEncoding() throws {
        let op = InverseOperation.restoreBytes(patchID: "test-patch", blob: Data([1, 2, 3]))
        let encoder = JSONEncoder()
        let decoder = JSONDecoder()

        let data = try encoder.encode(op)
        let decoded = try decoder.decode(InverseOperation.self, from: data)

        #expect(decoded == op)
    }

    @Test func testCrashBetweenPreparedAndCommitted() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        PatchLibraryStore.directoryOverride = tempDir.appendingPathComponent("PatchLibrary")
        SessionStore.directoryOverride = tempDir.appendingPathComponent("AgentSessions")
        defer {
            PatchLibraryStore.directoryOverride = nil
            SessionStore.directoryOverride = nil
        }

        // Initialize empty library
        let active = try ProfileLoader.active()
        let patches = try PatchLibraryStore.load(profile: active.profile)
        #expect(!patches.isEmpty) // Should load factory patches

        let patchToModify = patches[0]

        // 1. Setup a session
        let sessionStore = SessionStore()
        let session = try sessionStore.createSession(
            provider: "test",
            model: "test",
            profileHash: StudioState.calculateProfileHash(),
            libraryPath: PatchLibraryStore.libraryDirectory().path
        )

        // 2. Write prepared log entry but NO committed counterpart
        let sessionDir = try SessionStore.sessionDirectory(id: session.id)
        try FileManager.default.createDirectory(at: sessionDir, withIntermediateDirectories: true)
        let journalURL = sessionDir.appendingPathComponent("journal.jsonl")
        let journal = WALJournal(url: journalURL)

        let originalBytes = patchToModify.patch.data
        var modifiedBytes = originalBytes
        modifiedBytes[100] = modifiedBytes[100] ^ 0xFF

        // Prepare inverse operation
        let entry = TransactionEntry(
            sequence: 0,
            timestamp: Date(),
            toolName: "set_parameter",
            patchID: patchToModify.id,
            revisionBefore: patchToModify.revision,
            inverse: .restoreBytes(patchID: patchToModify.id, blob: originalBytes),
            beforeHash: originalBytes.sha256Hex(),
            afterHash: modifiedBytes.sha256Hex(),
            state: .prepared
        )
        try journal.append(entry)

        // Write a corrupted patch to disk (mismatching both hashes to force rollback)
        var corruptBytes = modifiedBytes
        corruptBytes[200] = corruptBytes[200] ^ 0xAA
        let patchURL = try PatchLibraryStore.libraryDirectory().appendingPathComponent(patchToModify.id).appendingPathExtension("mg101patch")
        try corruptBytes.write(to: patchURL, options: .atomic)

        // 3. Initialize StudioState, which runs recoverDanglingTransactions() automatically
        let state = StudioState()

        // 4. Verify the patch was reverted back to originalBytes during startup recovery
        let recoveredPatch = state.patches.first { $0.id == patchToModify.id }!
        #expect(recoveredPatch.patch.data == originalBytes)
    }

    @Test func testRevisionConflict() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        PatchLibraryStore.directoryOverride = tempDir.appendingPathComponent("PatchLibrary")
        SessionStore.directoryOverride = tempDir.appendingPathComponent("AgentSessions")
        defer {
            PatchLibraryStore.directoryOverride = nil
            SessionStore.directoryOverride = nil
        }

        let state = StudioState()
        let patch = state.patches[0]

        // Try mutating with wrong expected revision
        let wrongRevision = patch.revision + 10

        #expect(throws: (any Error).self) {
            try state.mutatePatch(id: patch.id, expectedRevision: wrongRevision) { _ in }
        }

        #expect(throws: (any Error).self) {
            try state.deletePatch(id: patch.id, expectedRevision: wrongRevision)
        }
    }

    @Test func testHashConflictOnRevert() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        PatchLibraryStore.directoryOverride = tempDir.appendingPathComponent("PatchLibrary")
        SessionStore.directoryOverride = tempDir.appendingPathComponent("AgentSessions")
        defer {
            PatchLibraryStore.directoryOverride = nil
            SessionStore.directoryOverride = nil
        }

        let state = StudioState()
        state.startNewSession()
        let patch = state.patches[0]

        // Perform a valid mutation
        try state.mutatePatch(id: patch.id, expectedRevision: patch.revision) { patchRec in
            try patchRec.setBPM(140)
        }

        // Manually corrupt/modify the patch on disk to mismatch the journal's afterHash
        let patchURL = try PatchLibraryStore.libraryDirectory().appendingPathComponent(patch.id).appendingPathExtension("mg101patch")
        var data = try Data(contentsOf: patchURL)
        data[200] = data[200] ^ 0xAA // manual corruption
        try data.write(to: patchURL, options: .atomic)

        // Reload state patches
        try state.reloadPatches()

        // Revert last action should fail with revert conflict
        #expect(throws: (any Error).self) {
            try state.revertLastAgentAction()
        }
    }

    @Test func testMigrationOfIndex() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        PatchLibraryStore.directoryOverride = tempDir.appendingPathComponent("PatchLibrary")
        defer { PatchLibraryStore.directoryOverride = nil }

        let active = try ProfileLoader.active()

        // Create a legacy directory without library-index.json
        let libDir = try PatchLibraryStore.libraryDirectory()
        try FileManager.default.createDirectory(at: libDir, withIntermediateDirectories: true)

        // Put user presets
        let samplePresetBytes = try PatchCollection.bundledFactoryRecords(profile: active.profile)[0].data

        // Modify user presets slightly so they are not identical to any factory preset
        var userPreset1 = samplePresetBytes
        userPreset1[15] = 99

        let file1 = libDir.appendingPathComponent("user-patch-1.mg101patch")
        let file2 = libDir.appendingPathComponent("user-patch-2.mg101patch")
        try userPreset1.write(to: file1, options: .atomic)
        try userPreset1.write(to: file2, options: .atomic) // deliberate duplicate copy

        // Load patches, triggering migration
        let loaded = try PatchLibraryStore.load(profile: active.profile)

        // Deliberate duplicate copy should NOT be removed!
        let user1Exists = loaded.contains { $0.id == "user-patch-1" }
        let user2Exists = loaded.contains { $0.id == "user-patch-2" }
        #expect(user1Exists)
        #expect(user2Exists)
    }

    @Test func testSessionStateMachine() throws {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        SessionStore.directoryOverride = tempDir.appendingPathComponent("AgentSessions")
        defer { SessionStore.directoryOverride = nil }

        let store = SessionStore()

        let session = try store.createSession(
            provider: "Anthropic",
            model: "claude-sonnet",
            profileHash: "dummy-hash",
            libraryPath: "/dummy/path"
        )

        #expect(session.provider == "Anthropic")
        #expect(session.state == .active)

        // Append messages and check compaction
        try store.appendMessage(sessionID: session.id, message: ChatMessage(role: "user", content: "Hello"))
        try store.appendMessage(sessionID: session.id, message: ChatMessage(role: "assistant", content: "Hi!"))

        let messages = try store.loadMessages(sessionID: session.id)
        #expect(messages.count == 2)

        // Compact session
        try store.compactHistory(sessionID: session.id, keepLast: 1, summaryText: "Summary")

        // Verify compaction (keeps summary message + last 1 turn = 2 messages total)
        let compacted = try store.loadMessages(sessionID: session.id)
        #expect(compacted.count == 2)
    }
}
