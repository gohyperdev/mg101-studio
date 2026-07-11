import Foundation
import MG101Core
import MG101Tools

struct ToolCall: Codable, Sendable, Equatable {
    let id: String
    let name: String
    let arguments: [String: JSONValue]

    init(id: String, name: String, arguments: [String: JSONValue]) {
        self.id = id
        self.name = name
        self.arguments = arguments
    }
}

struct ToolResult: Codable, Sendable, Equatable {
    let toolUseID: String
    let content: String
    let isError: Bool

    init(toolUseID: String, content: String, isError: Bool) {
        self.toolUseID = toolUseID
        self.content = content
        self.isError = isError
    }
}

struct ChatMessage: Codable, Sendable, Identifiable {
    let id: String // UUID String
    let role: String // "user" | "assistant"
    let content: String
    let timestamp: Date
    let toolCalls: [ToolCall]?
    let toolResults: [ToolResult]?

    init(
        id: String = UUID().uuidString,
        role: String,
        content: String,
        timestamp: Date = Date(),
        toolCalls: [ToolCall]? = nil,
        toolResults: [ToolResult]? = nil
    ) {
        self.id = id
        self.role = role
        self.content = content
        self.timestamp = timestamp
        self.toolCalls = toolCalls
        self.toolResults = toolResults
    }
}

final class AgentLoop: Sendable {
    private let configuration: AgentConfiguration
    private let executor: GUIToolExecutor
    private let systemPrompt: String
    private let sessionID: String
    private let needsConfirmation: @Sendable (DomainCommand) async -> Bool
    private let onMessageStream: @Sendable (String) -> Void

    init(
        configuration: AgentConfiguration,
        executor: GUIToolExecutor,
        systemPrompt: String,
        sessionID: String,
        needsConfirmation: @escaping @Sendable (DomainCommand) async -> Bool,
        onMessageStream: @escaping @Sendable (String) -> Void
    ) {
        var normalizedConfig = configuration
        normalizedConfig.endpoint = Self.normalizeEndpoint(configuration.endpoint, provider: configuration.provider)
        normalizedConfig.model = configuration.model.trimmingCharacters(in: .whitespacesAndNewlines)
        normalizedConfig.apiKey = configuration.apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        self.configuration = normalizedConfig
        self.executor = executor
        self.systemPrompt = systemPrompt
        self.sessionID = sessionID
        self.needsConfirmation = needsConfirmation
        self.onMessageStream = onMessageStream
    }

    static func normalizeEndpoint(_ endpoint: String, provider: AgentProvider) -> String {
        var trimmed = endpoint.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return trimmed }

        if !trimmed.lowercased().hasPrefix("http://") && !trimmed.lowercased().hasPrefix("https://") {
            trimmed = "http://" + trimmed
        }

        guard let url = URL(string: trimmed) else { return trimmed }

        let path = url.path.lowercased()
        if provider == .anthropic {
            if !path.hasSuffix("/messages") {
                if path.isEmpty || path == "/" {
                    return url.appendingPathComponent("v1/messages").absoluteString
                } else {
                    return url.appendingPathComponent("messages").absoluteString
                }
            }
        } else {
            if !path.hasSuffix("/chat/completions") {
                if path.isEmpty || path == "/" {
                    return url.appendingPathComponent("v1/chat/completions").absoluteString
                } else if path.hasSuffix("/v1") {
                    return url.appendingPathComponent("chat/completions").absoluteString
                } else {
                    return url.appendingPathComponent("v1/chat/completions").absoluteString
                }
            }
        }
        return trimmed
    }

    static func isPathApproved(_ path: String, approvedRoots: [String]) -> Bool {
        let standardPath = URL(fileURLWithPath: path).standardized.path
        for root in approvedRoots {
            let standardRoot = URL(fileURLWithPath: root).standardized.path
            if standardPath == standardRoot || standardPath.hasPrefix(standardRoot + "/") {
                return true
            }
        }
        return false
    }

    func run(
        history: [ChatMessage],
        approvedRoots: inout [String]
    ) async throws -> [ChatMessage] {
        var localHistory = history
        var iteration = 0
        let maxIterations = 20

        while iteration < maxIterations {
            iteration += 1
            
            let response: LLMResponse
            if configuration.provider == .anthropic {
                response = try await requestAnthropic(history: localHistory)
            } else {
                response = try await requestOpenAI(history: localHistory)
            }

            if let text = response.text, !text.isEmpty {
                onMessageStream(text)
            }

            guard let toolCalls = response.toolCalls, !toolCalls.isEmpty else {
                if let text = response.text, !text.isEmpty {
                    localHistory.append(ChatMessage(role: "assistant", content: text))
                }
                break
            }

            localHistory.append(ChatMessage(
                role: "assistant",
                content: response.text ?? "",
                toolCalls: toolCalls
            ))

            var results: [ToolResult] = []
            for call in toolCalls {
                do {
                    let cmd = try DomainCommand.parse(name: call.name, arguments: call.arguments)
                    
                    if case .filesystem = ToolDefinition.allDefinitions(variant: .library).first(where: { $0.name == call.name })?.kind {
                        var pathsToCheck: [String] = []
                        if case .importPatch(let p) = cmd { pathsToCheck.append(p) }
                        else if case .exportPatch(_, let p) = cmd { pathsToCheck.append(p) }
                        else if case .setIR(_, let p, _) = cmd { pathsToCheck.append(p) }
                        else if case .listFiles(let p) = cmd { pathsToCheck.append(p) }
                        
                        for p in pathsToCheck {
                            if !Self.isPathApproved(p, approvedRoots: approvedRoots) {
                                let approved = await needsConfirmation(cmd)
                                if approved {
                                    let parent = URL(fileURLWithPath: p).deletingLastPathComponent().path
                                    approvedRoots.append(parent)
                                } else {
                                    throw ToolExecutionError.custom("Access to path '\(p)' denied by user.")
                                }
                            }
                        }
                    }
                    
                    if case .destructive = ToolDefinition.allDefinitions(variant: .library).first(where: { $0.name == call.name })?.kind {
                        let approved = await needsConfirmation(cmd)
                        guard approved else {
                            throw ToolExecutionError.custom("Operation cancelled by user.")
                        }
                    }

                    let resultJSON = try await executor.execute(command: cmd)
                    let encoder = JSONEncoder()
                    encoder.outputFormatting = .sortedKeys
                    let resultData = try encoder.encode(resultJSON)
                    let resultStr = String(decoding: resultData, as: UTF8.self)
                    
                    results.append(ToolResult(toolUseID: call.id, content: resultStr, isError: false))
                } catch {
                    results.append(ToolResult(toolUseID: call.id, content: error.localizedDescription, isError: true))
                }
            }

            localHistory.append(ChatMessage(
                role: "user",
                content: "",
                toolResults: results
            ))
        }

        return localHistory
    }

    private struct LLMResponse {
        let text: String?
        let toolCalls: [ToolCall]?
    }

    static func buildAnthropicRequest(
        url: URL,
        configuration: AgentConfiguration,
        systemPrompt: String,
        history: [ChatMessage]
    ) throws -> URLRequest {
        var payloadMessages: [AnthropicMessagePayload] = []
        for msg in history {
            if let results = msg.toolResults, !results.isEmpty {
                let blocks = results.map {
                    AnthropicContentBlock.toolResult(
                        type: "tool_result",
                        tool_use_id: $0.toolUseID,
                        content: $0.content,
                        is_error: $0.isError ? true : nil
                    )
                }
                payloadMessages.append(AnthropicMessagePayload(role: "user", content: .blocks(blocks)))
            } else if let calls = msg.toolCalls, !calls.isEmpty {
                var blocks: [AnthropicContentBlock] = []
                if !msg.content.isEmpty {
                    blocks.append(.text(type: "text", text: msg.content))
                }
                for call in calls {
                    blocks.append(.toolUse(type: "tool_use", id: call.id, name: call.name, input: call.arguments))
                }
                payloadMessages.append(AnthropicMessagePayload(role: "assistant", content: .blocks(blocks)))
            } else {
                payloadMessages.append(AnthropicMessagePayload(role: msg.role, content: .text(msg.content)))
            }
        }

        let tools = ToolDefinition.allDefinitions(variant: .library).map { $0.anthropicFormat() }

        let body = AnthropicRequest(
            model: configuration.model,
            maxTokens: 4096,
            system: systemPrompt,
            messages: payloadMessages,
            tools: tools
        )

        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue(configuration.apiKey, forHTTPHeaderField: "x-api-key")
        request.setValue("2023-06-01", forHTTPHeaderField: "anthropic-version")
        request.httpBody = try JSONEncoder().encode(body)
        return request
    }

    private func requestAnthropic(history: [ChatMessage]) async throws -> LLMResponse {
        guard let url = URL(string: configuration.endpoint) else { throw AgentAPIError.endpoint }
        let request = try Self.buildAnthropicRequest(
            url: url,
            configuration: configuration,
            systemPrompt: systemPrompt,
            history: history
        )

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse, 200..<300 ~= http.statusCode else {
            let status = (response as? HTTPURLResponse)?.statusCode ?? -1
            let bodyStr = String(decoding: data, as: UTF8.self)
            throw AgentAPIError.custom("Anthropic API error: HTTP \(status). Details: \(bodyStr)")
        }

        let decoded = try JSONDecoder().decode(AnthropicResponse.self, from: data)
        var textContent = ""
        var toolCalls: [ToolCall] = []

        for block in decoded.content {
            switch block {
            case .text(_, let text):
                if let text { textContent.append(text) }
            case .toolUse(_, let id, let name, let input):
                toolCalls.append(ToolCall(id: id, name: name, arguments: input))
            default:
                break
            }
        }

        return LLMResponse(
            text: textContent.isEmpty ? nil : textContent,
            toolCalls: toolCalls.isEmpty ? nil : toolCalls
        )
    }

    static func buildOpenAIRequest(
        url: URL,
        configuration: AgentConfiguration,
        systemPrompt: String,
        history: [ChatMessage]
    ) throws -> URLRequest {
        var payloadMessages: [OpenAIMessagePayload] = []
        payloadMessages.append(OpenAIMessagePayload(role: "system", content: systemPrompt, tool_calls: nil, tool_call_id: nil))

        for msg in history {
            if let results = msg.toolResults, !results.isEmpty {
                for result in results {
                    payloadMessages.append(OpenAIMessagePayload(
                        role: "tool",
                        content: result.content,
                        tool_calls: nil,
                        tool_call_id: result.toolUseID
                    ))
                }
            } else if let calls = msg.toolCalls, !calls.isEmpty {
                let openAICalls = calls.map { call in
                    let argsData = try! JSONEncoder().encode(call.arguments)
                    let argsStr = String(decoding: argsData, as: UTF8.self)
                    return OpenAIToolCall(
                        id: call.id,
                        type: "type",
                        function: OpenAIFunctionCall(name: call.name, arguments: argsStr)
                    )
                }
                payloadMessages.append(OpenAIMessagePayload(
                    role: "assistant",
                    content: msg.content.isEmpty ? nil : msg.content,
                    tool_calls: openAICalls,
                    tool_call_id: nil
                ))
            } else {
                payloadMessages.append(OpenAIMessagePayload(
                    role: msg.role,
                    content: msg.content,
                    tool_calls: nil,
                    tool_call_id: nil
                ))
            }
        }

        let tools = ToolDefinition.allDefinitions(variant: .library).map { $0.openAIFormat() }

        let body = OpenAIRequest(
            model: configuration.model,
            messages: payloadMessages,
            tools: tools,
            temperature: 0.2
        )

        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if !configuration.apiKey.isEmpty {
            request.setValue("Bearer \(configuration.apiKey)", forHTTPHeaderField: "Authorization")
        }
        request.httpBody = try JSONEncoder().encode(body)
        return request
    }

    private func requestOpenAI(history: [ChatMessage]) async throws -> LLMResponse {
        guard let url = URL(string: configuration.endpoint) else { throw AgentAPIError.endpoint }
        let request = try Self.buildOpenAIRequest(
            url: url,
            configuration: configuration,
            systemPrompt: systemPrompt,
            history: history
        )

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse, 200..<300 ~= http.statusCode else {
            let status = (response as? HTTPURLResponse)?.statusCode ?? -1
            let bodyStr = String(decoding: data, as: UTF8.self)
            throw AgentAPIError.custom("OpenAI API error: HTTP \(status). Details: \(bodyStr)")
        }

        let decoded = try JSONDecoder().decode(OpenAIResponse.self, from: data)
        guard let choice = decoded.choices.first else {
            throw AgentAPIError.empty
        }

        var toolCalls: [ToolCall] = []
        if let choiceCalls = choice.message.tool_calls {
            for call in choiceCalls {
                if let argsData = call.function.arguments.data(using: .utf8),
                   let argsJSON = try? JSONDecoder().decode([String: JSONValue].self, from: argsData) {
                    toolCalls.append(ToolCall(id: call.id, name: call.function.name, arguments: argsJSON))
                }
            }
        }

        return LLMResponse(
            text: choice.message.content,
            toolCalls: toolCalls.isEmpty ? nil : toolCalls
        )
    }
}

// Anthropic Request/Response models
private struct AnthropicRequest: Encodable {
    let model: String
    let maxTokens: Int
    let system: String
    let messages: [AnthropicMessagePayload]
    let tools: [[String: JSONValue]]

    enum CodingKeys: String, CodingKey {
        case model, system, messages, tools
        case maxTokens = "max_tokens"
    }
}

private struct AnthropicMessagePayload: Codable {
    let role: String
    let content: AnthropicContent
}

private enum AnthropicContent: Codable {
    case text(String)
    case blocks([AnthropicContentBlock])

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let str = try? container.decode(String.self) {
            self = .text(str)
        } else if let blocks = try? container.decode([AnthropicContentBlock].self) {
            self = .blocks(blocks)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "Invalid AnthropicContent")
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .text(let str): try container.encode(str)
        case .blocks(let blocks): try container.encode(blocks)
        }
    }
}

private enum AnthropicContentBlock: Codable {
    case text(type: String, text: String?)
    case toolUse(type: String, id: String, name: String, input: [String: JSONValue])
    case toolResult(type: String, tool_use_id: String, content: String, is_error: Bool?)
    case unknown(type: String)

    enum CodingKeys: String, CodingKey {
        case type, id, name, input, text, tool_use_id, content, is_error
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)
        switch type {
        case "text":
            let text = try container.decodeIfPresent(String.self, forKey: .text)
            self = .text(type: "text", text: text)
        case "tool_use":
            let id = try container.decode(String.self, forKey: .id)
            let name = try container.decode(String.self, forKey: .name)
            let input = try container.decode([String: JSONValue].self, forKey: .input)
            self = .toolUse(type: "tool_use", id: id, name: name, input: input)
        case "tool_result":
            let toolUseID = try container.decode(String.self, forKey: .tool_use_id)
            let content = try container.decode(String.self, forKey: .content)
            let isError = try container.decodeIfPresent(Bool.self, forKey: .is_error)
            self = .toolResult(type: "tool_result", tool_use_id: toolUseID, content: content, is_error: isError)
        default:
            self = .unknown(type: type)
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .text(let type, let text):
            try container.encode(type, forKey: .type)
            try container.encode(text, forKey: .text)
        case .toolUse(let type, let id, let name, let input):
            try container.encode(type, forKey: .type)
            try container.encode(id, forKey: .id)
            try container.encode(name, forKey: .name)
            try container.encode(input, forKey: .input)
        case .toolResult(let type, let toolUseID, let content, let isError):
            try container.encode(type, forKey: .type)
            try container.encode(toolUseID, forKey: .tool_use_id)
            try container.encode(content, forKey: .content)
            if let isError {
                try container.encode(isError, forKey: .is_error)
            }
        case .unknown(let type):
            try container.encode(type, forKey: .type)
        }
    }
}

private struct AnthropicResponse: Decodable {
    let content: [AnthropicContentBlock]
}

// OpenAI Request/Response models
private struct OpenAIRequest: Encodable {
    let model: String
    let messages: [OpenAIMessagePayload]
    let tools: [[String: JSONValue]]
    let temperature: Double
}

private struct OpenAIMessagePayload: Codable {
    let role: String
    let content: String?
    let tool_calls: [OpenAIToolCall]?
    let tool_call_id: String?
}

private struct OpenAIToolCall: Codable {
    let id: String
    let type: String
    let function: OpenAIFunctionCall
}

private struct OpenAIFunctionCall: Codable {
    let name: String
    let arguments: String
}

private struct OpenAIResponse: Decodable {
    struct Choice: Decodable {
        struct Message: Decodable {
            let role: String
            let content: String?
            let tool_calls: [OpenAIToolCall]?
        }
        let message: Message
    }
    let choices: [Choice]
}

public enum AgentProvider: String, CaseIterable, Identifiable, Sendable {
    case openAICompatible
    case anthropic

    public var id: String { rawValue }
    public var displayName: String {
        switch self {
        case .openAICompatible: "OpenAI-compatible"
        case .anthropic: "Anthropic"
        }
    }
}

public struct AgentConfiguration: Sendable {
    public var provider: AgentProvider
    public var endpoint: String
    public var model: String
    public var apiKey: String

    public init(provider: AgentProvider, endpoint: String, model: String, apiKey: String) {
        self.provider = provider
        self.endpoint = endpoint
        self.model = model
        self.apiKey = apiKey
    }

    public var isConfigured: Bool {
        guard let url = URL(string: endpoint),
              ["https", "http"].contains(url.scheme?.lowercased() ?? ""),
              !model.isEmpty
        else { return false }
        return provider != .anthropic || !apiKey.isEmpty
    }
}

public enum AgentAPIError: LocalizedError {
    case endpoint
    case http(Int)
    case empty
    case missing(String)
    case invalidOperation(String)
    case custom(String)

    public var errorDescription: String? {
        switch self {
        case .endpoint: "Invalid AI endpoint URL."
        case .http(let status): "AI endpoint returned HTTP \(status)."
        case .empty: "AI endpoint returned no message."
        case .missing(let name): "Agent operation is missing \(name)."
        case .invalidOperation(let value): "Agent proposed unsupported operation \(value)."
        case .custom(let msg): msg
        }
    }
}

// Deprecated One-shot API Client backward compatibility
@available(*, deprecated, message: "Use AgentLoop for multi-turn agent runs instead.")
public struct AgentPlan: Decodable, Sendable {
    public let message: String
    public let operations: [AgentCommand]
}

@available(*, deprecated, message: "Use AgentLoop for multi-turn agent runs instead.")
public struct AgentCommand: Decodable, Sendable {
    public let action: String
    public let block: String?
    public let model: Int?
    public let parameter: String?
    public let value: Int?
    public let boolValue: Bool?
    public let text: String?
    public let values: [Int]?

    enum CodingKeys: String, CodingKey {
        case action, block, model, parameter, value, text, values
        case boolValue = "bool_value"
    }

    public func operation() throws -> PatchOperation {
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

@available(*, deprecated, message: "Use AgentLoop for multi-turn agent runs instead.")
public enum AgentAPIClient {
    public static func plan(
        prompt: String,
        patch: PatchRecord,
        profile: DeviceProfile,
        catalog: EffectCatalog,
        configuration: AgentConfiguration
    ) async throws -> AgentPlan {
        throw AgentAPIError.custom("Legacy planning client is deprecated. Shift to AgentLoop.")
    }
}
