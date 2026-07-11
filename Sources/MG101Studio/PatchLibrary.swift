import Foundation
import MG101Core
import MG101Tools

struct LibraryPatch: Identifiable {
    enum Origin: String, Codable {
        case factory = "Factory"
        case imported = "Imported"
        case editedFactory = "Edited factory"
    }

    let id: PatchID
    var patch: PatchRecord
    let original: PatchRecord
    var sourceName: String
    var origin: Origin
    var revision: Int

    var isFactory: Bool { origin == .factory }
}

struct LibraryIndexEntry: Codable {
    let id: String
    let origin: String
    let sourceName: String
    let revision: Int
}

struct LibraryIndex: Codable {
    var patches: [LibraryIndexEntry]
}

enum PatchLibraryStore {
    static func load(profile: DeviceProfile) throws -> [LibraryPatch] {
        let directory = try libraryDirectory()
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        
        let indexURL = directory.appendingPathComponent("library-index.json")
        
        // 1. Check if index exists. If not, perform migration.
        if !FileManager.default.fileExists(atPath: indexURL.path) {
            try migrateLegacyLibrary(profile: profile, directory: directory, indexURL: indexURL)
        }
        
        // 2. Read index
        let indexData = try Data(contentsOf: indexURL)
        let index = try JSONDecoder().decode(LibraryIndex.self, from: indexData)
        
        let factoryRecords = try PatchCollection.bundledFactoryRecords(profile: profile)
        
        var loadedPatches: [LibraryPatch] = []
        
        for entry in index.patches {
            let id = entry.id
            let origin = LibraryPatch.Origin(rawValue: entry.origin) ?? .imported
            let sourceName = entry.sourceName
            let revision = entry.revision
            
            let patchFile = directory.appendingPathComponent(id).appendingPathExtension("mg101patch")
            let originalFile = directory.appendingPathComponent(".originals").appendingPathComponent(id).appendingPathExtension("mg101patch")
            
            // Load current record
            let record: PatchRecord
            if FileManager.default.fileExists(atPath: patchFile.path) {
                record = try PatchRecord(contentsOf: patchFile, profile: profile)
            } else if id.hasPrefix("factory-"),
                      let indexInt = Int(id.dropFirst(8)),
                      indexInt >= 1 && indexInt <= factoryRecords.count {
                record = factoryRecords[indexInt - 1]
            } else {
                // If it's missing, skip or fallback
                continue
            }
            
            // Load original record
            let originalRecord: PatchRecord
            if FileManager.default.fileExists(atPath: originalFile.path) {
                originalRecord = try PatchRecord(contentsOf: originalFile, profile: profile)
            } else if id.hasPrefix("factory-"),
                      let indexInt = Int(id.dropFirst(8)),
                      indexInt >= 1 && indexInt <= factoryRecords.count {
                originalRecord = factoryRecords[indexInt - 1]
            } else {
                originalRecord = record
            }
            
            let patch = LibraryPatch(
                id: id,
                patch: record,
                original: originalRecord,
                sourceName: sourceName,
                origin: origin,
                revision: revision
            )
            loadedPatches.append(patch)
        }
        
        return loadedPatches
    }

    static func persist(_ item: LibraryPatch) throws {
        let directory = try libraryDirectory()
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        let patchURL = directory.appendingPathComponent(item.id).appendingPathExtension("mg101patch")
        try item.patch.data.write(to: patchURL, options: .atomic)
    }

    static func saveOriginal(_ record: PatchRecord, id: String) throws {
        let directory = try libraryDirectory()
        let originalsDir = directory.appendingPathComponent(".originals")
        try FileManager.default.createDirectory(at: originalsDir, withIntermediateDirectories: true)
        let originalURL = originalsDir.appendingPathComponent(id).appendingPathExtension("mg101patch")
        try record.data.write(to: originalURL, options: .atomic)
    }

    static func remove(_ item: LibraryPatch) throws {
        let directory = try libraryDirectory()
        let url = directory.appendingPathComponent(item.id).appendingPathExtension("mg101patch")
        if FileManager.default.fileExists(atPath: url.path) {
            try FileManager.default.removeItem(at: url)
        }
        
        let originalUrl = directory.appendingPathComponent(".originals").appendingPathComponent(item.id).appendingPathExtension("mg101patch")
        if FileManager.default.fileExists(atPath: originalUrl.path) {
            try FileManager.default.removeItem(at: originalUrl)
        }
    }

    static func updateIndex(patches: [LibraryPatch]) throws {
        let directory = try libraryDirectory()
        let indexURL = directory.appendingPathComponent("library-index.json")
        let entries = patches.map {
            LibraryIndexEntry(
                id: $0.id,
                origin: $0.origin.rawValue,
                sourceName: $0.sourceName,
                revision: $0.revision
            )
        }
        let index = LibraryIndex(patches: entries)
        let encoder = JSONEncoder()
        encoder.outputFormatting = .prettyPrinted
        let data = try encoder.encode(index)
        try data.write(to: indexURL, options: .atomic)
    }

    nonisolated(unsafe) static var directoryOverride: URL?

    static func libraryDirectory() throws -> URL {
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
            .appendingPathComponent("PatchLibrary", isDirectory: true)
    }

    private static func migrateLegacyLibrary(profile: DeviceProfile, directory: URL, indexURL: URL) throws {
        // Create backup directory
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyyMMdd-HHmmss"
        let timestamp = formatter.string(from: Date())
        let backupDirectory = directory.deletingLastPathComponent().appendingPathComponent("PatchLibrary_backup_\(timestamp)")
        
        // Backup existing files
        if FileManager.default.fileExists(atPath: directory.path) {
            try FileManager.default.createDirectory(at: backupDirectory, withIntermediateDirectories: true)
            let items = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            for item in items {
                // Avoid copying subdirectories
                var isDir: ObjCBool = false
                if FileManager.default.fileExists(atPath: item.path, isDirectory: &isDir), !isDir.boolValue {
                    let dest = backupDirectory.appendingPathComponent(item.lastPathComponent)
                    try FileManager.default.copyItem(at: item, to: dest)
                }
            }
        }
        
        // Prepare original storage
        let originalsDir = directory.appendingPathComponent(".originals")
        try FileManager.default.createDirectory(at: originalsDir, withIntermediateDirectories: true)
        
        // Load bundled factory patches
        let factory = try PatchCollection.bundledFactoryRecords(profile: profile)
        var initialEntries: [LibraryIndexEntry] = []
        
        // Add factory patches
        for index in 0..<factory.count {
            let id = String(format: "factory-%02d", index + 1)
            initialEntries.append(
                LibraryIndexEntry(
                    id: id,
                    origin: LibraryPatch.Origin.factory.rawValue,
                    sourceName: String(format: "Factory %02d", index + 1),
                    revision: 1
                )
            )
        }
        
        // Read existing legacy files from directory
        let urls = try FileManager.default.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: nil,
            options: [.skipsHiddenFiles]
        )
        
        let legacyFiles = urls.filter { $0.pathExtension.lowercased() == "mg101patch" }
        
        for url in legacyFiles {
            guard let patch = try? PatchRecord(contentsOf: url, profile: profile) else { continue }
            let data = patch.data
            
            // Check if identical to any bundled factory patch
            let isFactoryDuplicate = factory.contains { $0.data == data }
            if isFactoryDuplicate {
                // Delete legacy duplicate
                try? FileManager.default.removeItem(at: url)
                continue
            }
            
            let id = url.deletingPathExtension().lastPathComponent
            
            // Save original to .originals/
            let originalURL = originalsDir.appendingPathComponent(id).appendingPathExtension("mg101patch")
            try data.write(to: originalURL, options: .atomic)
            
            // Add to index
            initialEntries.append(
                LibraryIndexEntry(
                    id: id,
                    origin: LibraryPatch.Origin.imported.rawValue,
                    sourceName: "Library",
                    revision: 1
                )
            )
        }
        
        // Write the index
        let index = LibraryIndex(patches: initialEntries)
        let encoder = JSONEncoder()
        encoder.outputFormatting = .prettyPrinted
        let data = try encoder.encode(index)
        try data.write(to: indexURL, options: .atomic)
    }

    static func makeStagedMetadata(_ item: LibraryPatch) -> StagedMetadata {
        return StagedMetadata(
            origin: item.origin.rawValue,
            sourceName: item.sourceName,
            revision: item.revision,
            fileName: item.id + ".mg101patch"
        )
    }

    static func stagePhysicalFiles(_ item: LibraryPatch, sessionID: String) throws {
        let directory = try libraryDirectory()
        let stagingDir = directory.appendingPathComponent(".staging").appendingPathComponent(sessionID)
        try FileManager.default.createDirectory(at: stagingDir, withIntermediateDirectories: true)
        
        let patchFile = directory.appendingPathComponent(item.id).appendingPathExtension("mg101patch")
        let originalFile = directory.appendingPathComponent(".originals").appendingPathComponent(item.id).appendingPathExtension("mg101patch")
        
        let stagedPatch = stagingDir.appendingPathComponent(item.id).appendingPathExtension("mg101patch")
        let stagedOriginal = stagingDir.appendingPathComponent("_originals_" + item.id).appendingPathExtension("mg101patch")
        
        if FileManager.default.fileExists(atPath: patchFile.path) {
            try FileManager.default.moveItem(at: patchFile, to: stagedPatch)
        }
        if FileManager.default.fileExists(atPath: originalFile.path) {
            try FileManager.default.moveItem(at: originalFile, to: stagedOriginal)
        }
    }

    static func restoreFromStaging(id: String, sessionID: String, meta: StagedMetadata) throws {
        let directory = try libraryDirectory()
        let stagingDir = directory.appendingPathComponent(".staging").appendingPathComponent(sessionID)
        
        let stagedPatch = stagingDir.appendingPathComponent(id).appendingPathExtension("mg101patch")
        let stagedOriginal = stagingDir.appendingPathComponent("_originals_" + id).appendingPathExtension("mg101patch")
        
        let patchFile = directory.appendingPathComponent(id).appendingPathExtension("mg101patch")
        let originalFile = directory.appendingPathComponent(".originals").appendingPathComponent(id).appendingPathExtension("mg101patch")
        
        if FileManager.default.fileExists(atPath: stagedPatch.path) {
            try FileManager.default.moveItem(at: stagedPatch, to: patchFile)
        }
        if FileManager.default.fileExists(atPath: stagedOriginal.path) {
            try FileManager.default.moveItem(at: stagedOriginal, to: originalFile)
        }
    }
    
    static func clearStaging(sessionID: String) throws {
        let directory = try libraryDirectory()
        let stagingDir = directory.appendingPathComponent(".staging").appendingPathComponent(sessionID)
        if FileManager.default.fileExists(atPath: stagingDir.path) {
            try FileManager.default.removeItem(at: stagingDir)
        }
    }
}
