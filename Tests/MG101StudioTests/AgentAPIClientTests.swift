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
        let request = try AgentAPIClient.anthropicURLRequest(
            endpoint: #require(URL(string: configuration.endpoint)),
            system: "system instructions",
            user: "user request",
            configuration: configuration
        )

        #expect(request.httpMethod == "POST")
        #expect(request.value(forHTTPHeaderField: "content-type") == "application/json")
        #expect(request.value(forHTTPHeaderField: "x-api-key") == "test-secret")
        #expect(request.value(forHTTPHeaderField: "anthropic-version") == "2023-06-01")

        let body = try #require(request.httpBody)
        let decoded = try JSONSerialization.jsonObject(with: body)
        let object = try #require(decoded as? [String: Any])
        #expect(object["model"] as? String == "claude-sonnet-4-20250514")
        #expect(object["max_tokens"] as? Int == 2_048)
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
}
