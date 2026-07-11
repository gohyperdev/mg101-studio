import Foundation

public struct EffectCatalog: Codable, Sendable {
    public struct Module: Codable, Sendable {
        public let models: [String: Model]
    }

    public struct Model: Codable, Identifiable, Sendable {
        public let displayName: String
        public let slug: String
        public let modelID: Int
        public let parameters: [Parameter]

        public var id: Int { modelID }

        enum CodingKeys: String, CodingKey {
            case displayName = "display_name"
            case slug
            case modelID = "model_id"
            case parameters
        }
    }

    public struct Parameter: Codable, Identifiable, Sendable {
        public let name: String
        public let localIndex: Int
        public let fileOffset: Int
        public let rawRange: [Int]
        public let displayName: String?
        public let semanticConfidence: String?

        public var id: Int { fileOffset }
        public var minimum: Int { rawRange.first ?? 0 }
        public var maximum: Int { rawRange.dropFirst().first ?? 100 }

        enum CodingKeys: String, CodingKey {
            case name
            case localIndex = "local_index"
            case fileOffset = "file_offset"
            case rawRange = "raw_range"
            case displayName = "display_name"
            case semanticConfidence = "semantic_confidence"
        }
    }

    public let modules: [String: Module]

    public func models(for block: String) -> [Model] {
        guard let values = modules[block]?.models.values else { return [] }
        return values.sorted { $0.modelID < $1.modelID }
    }

    public func model(block: String, id: Int) -> Model? {
        modules[block]?.models[String(id)]
    }
}
