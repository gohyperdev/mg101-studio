import Foundation
import MG101Core

struct LibraryPatch: Identifiable {
    enum Origin: String {
        case factory = "Factory"
        case imported = "Imported"
        case editedFactory = "Edited factory"
    }

    let id: UUID
    var patch: PatchRecord
    let original: PatchRecord
    var sourceName: String
    var origin: Origin

    var isFactory: Bool { origin == .factory }
}

enum PatchLibraryStore {
    static func load(profile: DeviceProfile) throws -> [LibraryPatch] {
        let factory = try PatchCollection.bundledFactoryRecords(profile: profile)
            .enumerated()
            .map { index, patch in
                LibraryPatch(
                    id: UUID(),
                    patch: patch,
                    original: patch,
                    sourceName: String(format: "Factory %02d", index + 1),
                    origin: .factory
                )
            }

        let directory = try libraryDirectory()
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        let urls = try FileManager.default.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: nil,
            options: [.skipsHiddenFiles]
        )
        let imported: [LibraryPatch] = urls
            .filter { $0.pathExtension.lowercased() == "mg101patch" }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }
            .compactMap { url in
                guard let id = UUID(uuidString: url.deletingPathExtension().lastPathComponent),
                      let patch = try? PatchRecord(contentsOf: url, profile: profile)
                else { return nil }
                return LibraryPatch(
                    id: id,
                    patch: patch,
                    original: patch,
                    sourceName: "Library",
                    origin: .imported
                )
            }
        return factory + imported
    }

    static func persist(_ item: LibraryPatch) throws {
        let directory = try libraryDirectory()
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        try item.patch.data.write(
            to: directory
                .appendingPathComponent(item.id.uuidString)
                .appendingPathExtension("mg101patch"),
            options: .atomic
        )
    }

    static func remove(_ item: LibraryPatch) throws {
        let url = try libraryDirectory()
            .appendingPathComponent(item.id.uuidString)
            .appendingPathExtension("mg101patch")
        if FileManager.default.fileExists(atPath: url.path) {
            try FileManager.default.removeItem(at: url)
        }
    }

    private static func libraryDirectory() throws -> URL {
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
}
