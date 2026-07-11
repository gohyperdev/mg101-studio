import SwiftUI

struct AISettingsView: View {
    @EnvironmentObject private var state: StudioState

    var body: some View {
        Form {
            Section("Default provider") {
                Picker("Provider", selection: $state.agentProvider) {
                    ForEach(AgentProvider.allCases) { provider in
                        Text(provider.displayName).tag(provider)
                    }
                }
                .pickerStyle(.segmented)
            }

            Section("OpenAI-compatible API") {
                TextField("Endpoint", text: $state.openAIEndpoint)
                TextField("Model", text: $state.openAIModel)
                SecureField("API key", text: $state.openAIAPIKey)
                Text("Works with OpenAI and gateways exposing the Chat Completions format.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section("Anthropic API") {
                TextField("Endpoint", text: $state.anthropicEndpoint)
                TextField("Model", text: $state.anthropicModel)
                SecureField("API key", text: $state.anthropicAPIKey)
                Text("Uses the native Messages API with x-api-key and anthropic-version headers.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

        }
        .formStyle(.grouped)
        .safeAreaInset(edge: .bottom) {
            HStack {
                if !state.settingsMessage.isEmpty {
                    Text(state.settingsMessage)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button("Save") { state.saveAISettings() }
                    .buttonStyle(.borderedProminent)
            }
            .padding()
            .background(.bar)
        }
        .padding()
        .frame(width: 600, height: 520)
        .navigationTitle("AI Providers")
        .onAppear {
            state.loadAPIKeys(allowInteraction: true)
        }
    }
}
