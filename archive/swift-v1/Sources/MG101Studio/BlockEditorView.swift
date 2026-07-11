import MG101Core
import MG101Tools
import SwiftUI

struct BlockEditorView: View {
    @EnvironmentObject private var state: StudioState

    var body: some View {
        if let patch = state.patch, let block = state.selectedBlock {
            let modelID = patch.modelID(for: block)
            let models = state.catalog.models(for: block.id)
            let model = state.catalog.model(block: block.id, id: modelID)

            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    patchControls(patch)
                    HStack {
                        VStack(alignment: .leading) {
                            Text(block.displayName)
                                .font(.largeTitle.bold())
                            Text(model?.displayName ?? "Unknown model \(modelID)")
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        Toggle(
                            "Bypass",
                            isOn: Binding(
                                get: { state.patch?.isBypassed(block) ?? false },
                                set: { state.setBypass($0) }
                            )
                        )
                        .toggleStyle(.switch)
                    }

                    if !models.isEmpty {
                        Menu {
                            ForEach(models) { item in
                                Button {
                                    state.changeModel(item.modelID)
                                } label: {
                                    if item.modelID == modelID {
                                        Label(
                                            "\(item.modelID). \(item.displayName)",
                                            systemImage: "checkmark"
                                        )
                                    } else {
                                        Text("\(item.modelID). \(item.displayName)")
                                    }
                                }
                            }
                        } label: {
                            LabeledContent(
                                "Model",
                                value: model.map { "\($0.modelID). \($0.displayName)" }
                                    ?? "Unknown \(modelID)"
                            )
                            .frame(maxWidth: 360)
                        }
                    }

                    if let model {
                        LazyVGrid(
                            columns: [GridItem(.adaptive(minimum: 180), spacing: 20)],
                            spacing: 20
                        ) {
                            ForEach(model.parameters) { parameter in
                                ParameterControl(parameter: parameter)
                            }
                        }
                    } else {
                        Text("This selector is preserved, but its model is not defined by the active profile.")
                            .foregroundStyle(.secondary)
                    }
                }
                .padding(28)
            }
        }
    }

    private func patchControls(_ patch: PatchRecord) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                PatchNameField()
                Stepper(
                    "BPM \(state.patch?.bpm ?? 120)",
                    value: Binding(
                        get: { state.patch?.bpm ?? 120 },
                        set: { state.setBPM($0) }
                    ),
                    in: state.profile.bpm.minimum...state.profile.bpm.maximum
                )
            }
            HStack {
                namedSlider("Send", field: "send")
                namedSlider("Return", field: "return")
                Spacer()
                Label(
                    patch.irPresent ? patch.irName : "Built-in cabinet",
                    systemImage: patch.irPresent ? "waveform.path" : "hifispeaker"
                )
                Button("Load IR…") { state.chooseIR() }
                Button("Clear IR") { state.clearIR() }
                    .disabled(!patch.irPresent)
            }
        }
        .padding()
        .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 12))
    }

    private func namedSlider(_ title: String, field: String) -> some View {
        let definition = state.profile.namedFields[field]
        return HStack {
            Text(title)
            Slider(
                value: Binding(
                    get: {
                        guard let patch = state.patch, let definition else { return 0 }
                        return Double(patch.value(at: definition.offset))
                    },
                    set: { state.setNamedField(field, value: Int($0.rounded())) }
                ),
                in: Double(definition?.minimum ?? 0)...Double(definition?.maximum ?? 100),
                step: 1
            )
            .frame(width: 110)
        }
    }
}

private struct PatchNameField: View {
    @EnvironmentObject private var state: StudioState
    @State private var draft = ""
    @FocusState private var focused: Bool
    @State private var editing = false

    var body: some View {
        Group {
            if editing {
                HStack {
                    TextField("Patch name", text: $draft)
                        .textFieldStyle(.roundedBorder)
                        .focused($focused)
                        .onSubmit { commit() }
                    Button {
                        commit()
                    } label: {
                        Image(systemName: "checkmark")
                    }
                    Button {
                        cancel()
                    } label: {
                        Image(systemName: "xmark")
                    }
                }
                .onAppear { focused = true }
            } else {
                HStack {
                    Text(state.patch?.name ?? "Unnamed patch")
                        .font(.headline)
                        .lineLimit(1)
                    Button {
                        draft = state.patch?.name ?? ""
                        editing = true
                    } label: {
                        Label("Rename", systemImage: "pencil")
                    }
                }
            }
        }
        .task(id: state.selectedPatchID) {
            editing = false
            draft = state.patch?.name ?? ""
        }
    }

    private func cancel() {
        editing = false
        focused = false
        draft = state.patch?.name ?? ""
    }

    private func commit() {
        if let current = state.patch?.name, draft != current {
            state.setName(draft)
            if state.errorMessage != nil {
                return
            }
        }
        editing = false
        focused = false
    }
}

private struct ParameterControl: View {
    @EnvironmentObject private var state: StudioState
    let parameter: EffectCatalog.Parameter

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(parameter.displayName ?? parameter.name.uppercased())
                    .font(.headline)
                Spacer()
                Text("\(state.patch?.value(at: parameter.fileOffset) ?? 0)")
                    .monospacedDigit()
                    .foregroundStyle(.orange)
            }
            Slider(
                value: Binding(
                    get: { Double(state.patch?.value(at: parameter.fileOffset) ?? 0) },
                    set: { state.setParameter(parameter, value: Int($0.rounded())) }
                ),
                in: Double(parameter.minimum)...Double(parameter.maximum),
                step: 1
            )
            HStack {
                Text("\(parameter.minimum)")
                Spacer()
                Text(String(format: "0x%02X", parameter.fileOffset))
                Spacer()
                Text("\(parameter.maximum)")
            }
            .font(.caption2)
            .foregroundStyle(.secondary)
        }
        .padding()
        .background(.quaternary.opacity(0.6), in: RoundedRectangle(cornerRadius: 12))
    }
}
