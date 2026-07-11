import Foundation

public struct DeviceProfile: Codable, Sendable {
    public struct ByteRange: Codable, Sendable {
        public let offset: Int
        public let length: Int
    }

    public struct BPM: Codable, Sendable {
        public let msbOffset: Int
        public let lsbOffset: Int
        public let minimum: Int
        public let maximum: Int
    }

    public struct Block: Codable, Identifiable, Hashable, Sendable {
        public let id: String
        public let displayName: String
        public let selectorOffset: Int
        public let parameterOffsets: [Int]
    }

    public struct NamedField: Codable, Sendable {
        public let offset: Int
        public let minimum: Int
        public let maximum: Int
    }

    public let schemaVersion: Int
    public let id: String
    public let displayName: String
    public let recordSize: Int
    public let patchName: ByteRange
    public let bpm: BPM
    public let blocks: [Block]
    public let namedFields: [String: NamedField]

    public func block(_ id: String) throws -> Block {
        guard let block = blocks.first(where: { $0.id == id }) else {
            throw ProfileError.unknownBlock(id)
        }
        return block
    }
}

public enum ProfileError: LocalizedError {
    case missingResource(String)
    case unknownBlock(String)
    case unknownModel(String, Int)
    case malformed(String)

    public var errorDescription: String? {
        switch self {
        case .missingResource(let name): "Missing bundled profile resource: \(name)"
        case .unknownBlock(let id): "Unknown block: \(id)"
        case .unknownModel(let block, let id): "Unknown \(block) model: \(id)"
        case .malformed(let message): "Malformed profile: \(message)"
        }
    }
}

public enum ProfileLoader {
    public static let profileFileName = "device-profile.json"
    public static let catalogFileName = "effects-catalog.json"

    public static func bundled() throws -> (DeviceProfile, EffectCatalog) {
        guard let profileURL = bundledResourceURL(
            forResource: "device-profile",
            withExtension: "json"
        ) else {
            throw ProfileError.missingResource("device-profile.json")
        }
        guard let catalogURL = bundledResourceURL(
            forResource: "effects-catalog",
            withExtension: "json"
        ) else {
            throw ProfileError.missingResource("effects-catalog.json")
        }
        return try load(profileURL: profileURL, catalogURL: catalogURL)
    }

    public static func bundledResourceURL(
        forResource name: String,
        withExtension extensionName: String
    ) -> URL? {
        let installedBundle = Bundle.main.resourceURL
            .map { $0.appendingPathComponent("MG101Studio_MG101Core.bundle") }
            .flatMap(Bundle.init(url:))
        return (installedBundle ?? Bundle.module).url(
            forResource: name,
            withExtension: extensionName
        )
    }

    public static func load(
        profileURL: URL,
        catalogURL: URL
    ) throws -> (DeviceProfile, EffectCatalog) {
        let decoder = JSONDecoder()
        let profile = try decoder.decode(
            DeviceProfile.self,
            from: Data(contentsOf: profileURL)
        )
        let catalog = try decoder.decode(
            EffectCatalog.self,
            from: Data(contentsOf: catalogURL)
        )
        try validate(profile: profile, catalog: catalog)
        return (profile, catalog)
    }

    public static func load(from directory: URL) throws -> (DeviceProfile, EffectCatalog) {
        try load(
            profileURL: directory.appendingPathComponent(profileFileName),
            catalogURL: directory.appendingPathComponent(catalogFileName)
        )
    }

    public static func applicationSupportDirectory() throws -> URL {
        let root = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        return root
            .appendingPathComponent("MG101Studio", isDirectory: true)
            .appendingPathComponent("Profiles", isDirectory: true)
            .appendingPathComponent("active", isDirectory: true)
    }

    public static func active() throws -> (
        profile: DeviceProfile,
        catalog: EffectCatalog,
        source: URL?
    ) {
        let directory = try applicationSupportDirectory()
        let profileURL = directory.appendingPathComponent(profileFileName)
        let catalogURL = directory.appendingPathComponent(catalogFileName)
        if FileManager.default.fileExists(atPath: profileURL.path),
           FileManager.default.fileExists(atPath: catalogURL.path) {
            let loaded = try load(profileURL: profileURL, catalogURL: catalogURL)
            return (loaded.0, loaded.1, directory)
        }
        let loaded = try bundled()
        return (loaded.0, loaded.1, nil)
    }

    public static func install(from sourceDirectory: URL) throws -> (
        DeviceProfile,
        EffectCatalog
    ) {
        let loaded = try load(from: sourceDirectory)
        let destination = try applicationSupportDirectory()
        let manager = FileManager.default
        try manager.createDirectory(
            at: destination,
            withIntermediateDirectories: true
        )
        for name in [profileFileName, catalogFileName] {
            let source = sourceDirectory.appendingPathComponent(name)
            let target = destination.appendingPathComponent(name)
            let temporary = destination.appendingPathComponent(".\(name).new")
            try? manager.removeItem(at: temporary)
            try manager.copyItem(at: source, to: temporary)
            if manager.fileExists(atPath: target.path) {
                _ = try manager.replaceItemAt(target, withItemAt: temporary)
            } else {
                try manager.moveItem(at: temporary, to: target)
            }
        }
        return loaded
    }

    public static func removeActiveOverride() throws {
        let directory = try applicationSupportDirectory()
        if FileManager.default.fileExists(atPath: directory.path) {
            try FileManager.default.removeItem(at: directory)
        }
    }

    public static func validate(
        profile: DeviceProfile,
        catalog: EffectCatalog
    ) throws {
        guard profile.schemaVersion == 1 else {
            throw ProfileError.malformed(
                "Unsupported schemaVersion \(profile.schemaVersion); expected 1."
            )
        }
        guard profile.recordSize > 0 else {
            throw ProfileError.malformed("recordSize must be positive.")
        }
        func requireOffset(_ offset: Int, _ label: String) throws {
            guard 0..<profile.recordSize ~= offset else {
                throw ProfileError.malformed(
                    "\(label) offset \(offset) is outside the record."
                )
            }
        }
        guard profile.patchName.length > 0,
              profile.patchName.offset >= 0,
              profile.patchName.offset + profile.patchName.length <= profile.recordSize
        else {
            throw ProfileError.malformed("patchName range is outside the record.")
        }
        try requireOffset(profile.bpm.msbOffset, "bpm.msb")
        try requireOffset(profile.bpm.lsbOffset, "bpm.lsb")
        guard profile.bpm.minimum <= profile.bpm.maximum else {
            throw ProfileError.malformed("BPM range is reversed.")
        }

        var blockIDs = Set<String>()
        var selectorOffsets = Set<Int>()
        for block in profile.blocks {
            guard blockIDs.insert(block.id).inserted else {
                throw ProfileError.malformed("Duplicate block id \(block.id).")
            }
            guard selectorOffsets.insert(block.selectorOffset).inserted else {
                throw ProfileError.malformed(
                    "Duplicate selector offset \(block.selectorOffset)."
                )
            }
            try requireOffset(block.selectorOffset, "\(block.id).selector")
            for offset in block.parameterOffsets {
                try requireOffset(offset, "\(block.id).parameter")
            }
        }
        for (name, field) in profile.namedFields {
            try requireOffset(field.offset, name)
            guard field.minimum <= field.maximum,
                  0...255 ~= field.minimum,
                  0...255 ~= field.maximum
            else {
                throw ProfileError.malformed("Invalid range for field \(name).")
            }
        }

        for (blockID, module) in catalog.modules {
            guard let block = profile.blocks.first(where: { $0.id == blockID }) else {
                throw ProfileError.malformed(
                    "Catalog module \(blockID) has no profile block."
                )
            }
            var modelIDs = Set<Int>()
            for (key, model) in module.models {
                guard key == String(model.modelID), 0...63 ~= model.modelID else {
                    throw ProfileError.malformed(
                        "Invalid model key/id \(blockID).\(key)."
                    )
                }
                guard modelIDs.insert(model.modelID).inserted else {
                    throw ProfileError.malformed(
                        "Duplicate model id \(blockID).\(model.modelID)."
                    )
                }
                var parameterOffsets = Set<Int>()
                for parameter in model.parameters {
                    guard parameter.rawRange.count == 2,
                          0...255 ~= parameter.minimum,
                          0...255 ~= parameter.maximum,
                          parameter.minimum <= parameter.maximum
                    else {
                        throw ProfileError.malformed(
                            "Invalid range for \(blockID).\(model.modelID).\(parameter.name)."
                        )
                    }
                    guard block.parameterOffsets.contains(parameter.fileOffset) else {
                        throw ProfileError.malformed(
                            "Parameter \(blockID).\(model.modelID).\(parameter.name) uses offset \(parameter.fileOffset) outside its block."
                        )
                    }
                    guard parameterOffsets.insert(parameter.fileOffset).inserted else {
                        throw ProfileError.malformed(
                            "Duplicate parameter offset in \(blockID).\(model.modelID)."
                        )
                    }
                }
            }
        }
    }
}
