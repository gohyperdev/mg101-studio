import Foundation
import Testing
@testable import MG101Studio

struct AgentAPIClientTests {
    @Test func anthropicRequestUsesNativeMessagesContract() throws {
        let configuration = AgentConfiguration(
            provider: .anthropic,
            endpoint: "https://api.anthropic.com/v1/messages",
            model: "claude-sonnet-4-20250514",
            apiKey: "test-secret"
        )
        let request = try AgentLoop.buildAnthropicRequest(
            url: #require(URL(string: configuration.endpoint)),
            configuration: configuration,
            systemPrompt: "system instructions",
            history: [ChatMessage(role: "user", content: "user request")]
        )

        #expect(request.httpMethod == "POST")
        #expect(request.value(forHTTPHeaderField: "content-type") == "application/json")
        #expect(request.value(forHTTPHeaderField: "x-api-key") == "test-secret")
        #expect(request.value(forHTTPHeaderField: "anthropic-version") == "2023-06-01")

        let body = try #require(request.httpBody)
        let decoded = try JSONSerialization.jsonObject(with: body)
        let object = try #require(decoded as? [String: Any])
        #expect(object["model"] as? String == "claude-sonnet-4-20250514")
        #expect(object["max_tokens"] as? Int == 4_096)
        #expect(object["system"] as? String == "system instructions")
        let messages = try #require(object["messages"] as? [[String: Any]])
        #expect(messages.count == 1)
        #expect(messages[0]["role"] as? String == "user")
        #expect(messages[0]["content"] as? String == "user request")
    }

    @Test func anthropicRequiresKeyButLocalOpenAICompatibleMayNot() {
        let anthropic = AgentConfiguration(
            provider: .anthropic,
            endpoint: "https://api.anthropic.com/v1/messages",
            model: "claude-sonnet-4-20250514",
            apiKey: ""
        )
        let local = AgentConfiguration(
            provider: .openAICompatible,
            endpoint: "http://127.0.0.1:1234/v1/chat/completions",
            model: "local-model",
            apiKey: ""
        )
        #expect(!anthropic.isConfigured)
        #expect(local.isConfigured)
    }

    @Test func testEndpointNormalization() {
        let normalize = AgentLoop.normalizeEndpoint

        #expect(normalize("localhost:1234", .openAICompatible) == "http://localhost:1234/v1/chat/completions")
        #expect(normalize("http://localhost:11434", .openAICompatible) == "http://localhost:11434/v1/chat/completions")
        #expect(normalize("http://localhost:11434/v1", .openAICompatible) == "http://localhost:11434/v1/chat/completions")
        #expect(normalize("https://api.openai.com", .openAICompatible) == "https://api.openai.com/v1/chat/completions")
        #expect(normalize("https://api.anthropic.com", .anthropic) == "https://api.anthropic.com/v1/messages")

        #expect(normalize("http://127.0.0.1:8080/my/custom/path", .openAICompatible) == "http://127.0.0.1:8080/my/custom/path/v1/chat/completions")
        #expect(normalize("http://127.0.0.1:8080/custom/chat/completions", .openAICompatible) == "http://127.0.0.1:8080/custom/chat/completions")
    }
}
