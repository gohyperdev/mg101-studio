import Foundation
import MG101Core

enum AgentProvider: String, CaseIterable, Identifiable, Sendable {
    case openAICompatible
    case anthropic

    var id: String { rawValue }
    var displayName: String {
        switch self {
        case .openAICompatible: "OpenAI-compatible"
        case .anthropic: "Anthropic"
        }
    }
}

struct AgentConfiguration: Sendable {
    var provider: AgentProvider
    var endpoint: String
    var model: String
    var apiKey: String

    var isConfigured: Bool {
        guard let url = URL(string: endpoint),
              ["https", "http"].contains(url.scheme?.lowercased() ?? ""),
              !model.isEmpty
        else { return false }
        return provider != .anthropic || !apiKey.isEmpty
    }
}

struct AgentPlan: Decodable, Sendable {
    let message: String
    let operations: [AgentCommand]
}

struct AgentCommand: Decodable, Sendable {
    let action: String
    let block: String?
    let model: Int?
    let parameter: String?
    let value: Int?
    let boolValue: Bool?
    let text: String?
    let values: [Int]?

    enum CodingKeys: String, CodingKey {
        case action, block, model, parameter, value, text, values
        case boolValue = "bool_value"
    }

    func operation() throws -> PatchOperation {
        switch action {
        case "set_bpm":
            return .setBPM(try required(value, "value"))
        case "set_name":
            return .setName(try required(text, "text"))
        case "set_named_field":
            return .setNamedField(
                name: try required(parameter, "parameter"),
                value: try required(value, "value")
            )
        case "set_bypass":
            return .setBypass(
                block: try required(block, "block"),
                value: try required(boolValue, "bool_value")
            )
        case "set_parameter":
            return .setParameter(
                block: try required(block, "block"),
                model: try required(model, "model"),
                name: try required(parameter, "parameter"),
                value: try required(value, "value")
            )
        case "set_model":
            return .setModel(
                block: try required(block, "block"),
                model: try required(model, "model"),
                values: try required(values, "values"),
                bypassed: boolValue ?? false
            )
        default:
            throw AgentAPIError.invalidOperation(action)
        }
    }

    private func required<T>(_ value: T?, _ name: String) throws -> T {
        guard let value else { throw AgentAPIError.missing(name) }
        return value
    }
}

enum AgentAPIClient {
    static func plan(
        prompt: String,
        patch: PatchRecord,
        profile: DeviceProfile,
        catalog: EffectCatalog,
        configuration: AgentConfiguration
    ) async throws -> AgentPlan {
        guard let endpoint = URL(string: configuration.endpoint) else {
            throw AgentAPIError.endpoint
        }

        let context = try makeContext(
            patch: patch,
            profile: profile,
            catalog: catalog
        )
        let system = """
        You are the embedded MG-101 patch agent. Return JSON only, without markdown.
        Never invent blocks, model ids, parameter names or ranges.
        You may emit actions: set_bpm, set_name, set_named_field, set_bypass, set_parameter, set_model.
        set_model must provide every active parameter value in catalog order.
        Schema:
        {"message":"short explanation","operations":[
          {"action":"set_bpm","value":120},
          {"action":"set_named_field","parameter":"send","value":50},
          {"action":"set_bypass","block":"dly","bool_value":true},
          {"action":"set_parameter","block":"amp","model":2,"parameter":"gain","value":55},
          {"action":"set_model","block":"mod","model":6,"values":[40,50,60,70],"bool_value":false}
        ]}
        The application will validate and show every operation to the user before applying it.
        """
        let user = "Current patch and available controls:\n\(context)\n\nUser request:\n\(prompt)"
        let content: String
        switch configuration.provider {
        case .openAICompatible:
            content = try await requestOpenAICompatible(
                endpoint: endpoint,
                system: system,
                user: user,
                configuration: configuration
            )
        case .anthropic:
            content = try await requestAnthropic(
                endpoint: endpoint,
                system: system,
                user: user,
                configuration: configuration
            )
        }

        let clean = content
            .replacingOccurrences(of: "```json", with: "")
            .replacingOccurrences(of: "```", with: "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return try JSONDecoder().decode(AgentPlan.self, from: Data(clean.utf8))
    }

    private static func requestOpenAICompatible(
        endpoint: URL,
        system: String,
        user: String,
        configuration: AgentConfiguration
    ) async throws -> String {
        let body = ChatRequest(
            model: configuration.model,
            messages: [
                .init(role: "system", content: system),
                .init(role: "user", content: user),
            ],
            temperature: 0.2
        )
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if !configuration.apiKey.isEmpty {
            request.setValue(
                "Bearer \(configuration.apiKey)",
                forHTTPHeaderField: "Authorization"
            )
        }
        request.httpBody = try JSONEncoder().encode(body)

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse, 200..<300 ~= http.statusCode else {
            throw AgentAPIError.http((response as? HTTPURLResponse)?.statusCode ?? -1)
        }
        let decoded = try JSONDecoder().decode(ChatResponse.self, from: data)
        guard let content = decoded.choices.first?.message.content else {
            throw AgentAPIError.empty
        }
        return content
    }

    private static func requestAnthropic(
        endpoint: URL,
        system: String,
        user: String,
        configuration: AgentConfiguration
    ) async throws -> String {
        let request = try anthropicURLRequest(
            endpoint: endpoint,
            system: system,
            user: user,
            configuration: configuration
        )

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse, 200..<300 ~= http.statusCode else {
            throw AgentAPIError.http((response as? HTTPURLResponse)?.statusCode ?? -1)
        }
        let decoded = try JSONDecoder().decode(AnthropicResponse.self, from: data)
        let content = decoded.content
            .filter { $0.type == "text" }
            .compactMap(\.text)
            .joined()
        guard !content.isEmpty else { throw AgentAPIError.empty }
        return content
    }

    static func anthropicURLRequest(
        endpoint: URL,
        system: String,
        user: String,
        configuration: AgentConfiguration
    ) throws -> URLRequest {
        let body = AnthropicRequest(
            model: configuration.model,
            maxTokens: 2_048,
            system: system,
            messages: [.init(role: "user", content: user)]
        )
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue(configuration.apiKey, forHTTPHeaderField: "x-api-key")
        request.setValue("2023-06-01", forHTTPHeaderField: "anthropic-version")
        request.httpBody = try JSONEncoder().encode(body)
        return request
    }

    private static func makeContext(
        patch: PatchRecord,
        profile: DeviceProfile,
        catalog: EffectCatalog
    ) throws -> String {
        let blocks: [[String: Any]] = profile.blocks.map { block in
            let modelID = patch.modelID(for: block)
            let current = catalog.model(block: block.id, id: modelID)
            let models = catalog.models(for: block.id).map { model in
                [
                    "id": model.modelID,
                    "name": model.displayName,
                    "parameters": model.parameters.map {
                        [
                            "name": $0.name,
                            "minimum": $0.minimum,
                            "maximum": $0.maximum,
                        ] as [String: Any]
                    },
                ] as [String: Any]
            }
            return [
                "block": block.id,
                "current_model": modelID,
                "current_model_name": current?.displayName ?? "unknown",
                "bypassed": patch.isBypassed(block),
                "models": models,
            ]
        }
        let object: [String: Any] = [
            "name": patch.name,
            "bpm": patch.bpm,
            "ir": ["present": patch.irPresent, "name": patch.irName] as [String: Any],
            "named_fields": profile.namedFields.mapValues {
                [
                    "value": patch.value(at: $0.offset),
                    "minimum": $0.minimum,
                    "maximum": $0.maximum,
                ]
            },
            "blocks": blocks,
        ]
        let data = try JSONSerialization.data(
            withJSONObject: object,
            options: [.prettyPrinted, .sortedKeys]
        )
        return String(decoding: data, as: UTF8.self)
    }
}

private struct ChatRequest: Encodable {
    struct Message: Encodable {
        let role: String
        let content: String
    }
    let model: String
    let messages: [Message]
    let temperature: Double
}

private struct ChatResponse: Decodable {
    struct Choice: Decodable {
        struct Message: Decodable {
            let content: String?
        }
        let message: Message
    }
    let choices: [Choice]
}

private struct AnthropicRequest: Encodable {
    struct Message: Encodable {
        let role: String
        let content: String
    }
    let model: String
    let maxTokens: Int
    let system: String
    let messages: [Message]

    enum CodingKeys: String, CodingKey {
        case model, system, messages
        case maxTokens = "max_tokens"
    }
}

private struct AnthropicResponse: Decodable {
    struct Content: Decodable {
        let type: String
        let text: String?
    }
    let content: [Content]
}

enum AgentAPIError: LocalizedError {
    case endpoint
    case http(Int)
    case empty
    case missing(String)
    case invalidOperation(String)

    var errorDescription: String? {
        switch self {
        case .endpoint: "Invalid AI endpoint URL."
        case .http(let status): "AI endpoint returned HTTP \(status)."
        case .empty: "AI endpoint returned no message."
        case .missing(let name): "Agent operation is missing \(name)."
        case .invalidOperation(let value): "Agent proposed unsupported operation \(value)."
        }
    }
}
