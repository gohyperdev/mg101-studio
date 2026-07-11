import Foundation

public actor PatchWorkspace {
    public private(set) var profile: DeviceProfile
    public private(set) var catalog: EffectCatalog
    public private(set) var patch: PatchRecord?
    public private(set) var original: PatchRecord?
    public private(set) var sourceURL: URL?

    public init(profile: DeviceProfile, catalog: EffectCatalog) {
        self.profile = profile
        self.catalog = catalog
    }

    public func open(_ url: URL) throws {
        let loaded = try PatchRecord(contentsOf: url, profile: profile)
        patch = loaded
        original = loaded
        sourceURL = url
    }

    public func snapshot() -> PatchRecord? { patch }
    public func originalSnapshot() -> PatchRecord? { original }

    public func mutate(_ operation: PatchOperation) throws {
        guard var current = patch else { throw WorkspaceError.noDocument }
        switch operation {
        case .setByte(let offset, let value):
            try current.setByte(value, at: offset)
        case .setBPM(let value):
            try current.setBPM(value)
        case .setName(let value):
            try current.setName(value)
        case .setNamedField(let name, let value):
            try current.setNamedField(name, value: value)
        case .setBypass(let blockID, let value):
            current.setBypass(value, block: try profile.block(blockID))
        case .setParameter(let blockID, let modelID, let name, let value):
            guard let model = catalog.model(block: blockID, id: modelID),
                  let parameter = model.parameters.first(where: { $0.name == name })
            else {
                throw WorkspaceError.unknownParameter(blockID, modelID, name)
            }
            try current.setParameter(parameter, value: value)
        case .setModel(let blockID, let modelID, let values, let bypassed):
            let block = try profile.block(blockID)
            guard let model = catalog.model(block: blockID, id: modelID) else {
                throw ProfileError.unknownModel(blockID, modelID)
            }
            try current.setModel(
                model,
                block: block,
                values: values,
                bypassed: bypassed
            )
        }
        patch = current
    }

    public func save(to url: URL) throws {
        guard let patch else { throw WorkspaceError.noDocument }
        try PatchFileWriter.writeNew(patch.data, to: url)
    }
}

public enum PatchOperation: Codable, Sendable {
    case setByte(offset: Int, value: Int)
    case setBPM(Int)
    case setName(String)
    case setNamedField(name: String, value: Int)
    case setBypass(block: String, value: Bool)
    case setParameter(block: String, model: Int, name: String, value: Int)
    case setModel(block: String, model: Int, values: [Int], bypassed: Bool)
}

public enum WorkspaceError: LocalizedError {
    case noDocument
    case unknownParameter(String, Int, String)

    public var errorDescription: String? {
        switch self {
        case .noDocument: "No patch is open."
        case .unknownParameter(let block, let model, let name):
            "Unknown parameter \(block).\(model).\(name)."
        }
    }
}
