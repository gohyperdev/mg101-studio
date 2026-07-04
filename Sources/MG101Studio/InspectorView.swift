import MG101Core
import SwiftUI

struct InspectorView: View {
    @EnvironmentObject private var state: StudioState
    @State private var tab = "changes"

    var body: some View {
        VStack(spacing: 0) {
            Picker("", selection: $tab) {
                Text("Changes").tag("changes")
                Text("Binary").tag("binary")
                Text("Agent").tag("agent")
                Text("MCP").tag("mcp")
            }
            .pickerStyle(.segmented)
            .padding()
            Divider()
            if tab == "agent" {
                AgentView()
            } else if tab == "mcp" {
                MCPInfoView()
            } else if tab == "binary" {
                BinaryInspectorView()
            } else {
                changes
            }
        }
    }

    private var changes: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let patch = state.patch {
                Group {
                    LabeledContent("Slot", value: "\(patch.slotIndex)")
                    LabeledContent("BPM", value: "\(patch.bpm)")
                    LabeledContent("Bytes", value: "\(patch.data.count)")
                    LabeledContent("Changed", value: "\(state.differences.count)")
                }
                .padding(.horizontal)
            }
            List(state.differences) { difference in
                HStack {
                    Text(String(format: "0x%04X", difference.offset))
                        .font(.system(.caption, design: .monospaced))
                    Spacer()
                    Text(String(format: "%02X → %02X", difference.before, difference.after))
                        .font(.system(.caption, design: .monospaced))
                }
            }
        }
        .padding(.top)
    }
}

private struct BinaryInspectorView: View {
    @EnvironmentObject private var state: StudioState
    @State private var layout = BinaryLayout.logical
    @State private var base = BinaryNumberBase.hexadecimal

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Picker("Layout", selection: $layout) {
                    ForEach(BinaryLayout.allCases) { item in
                        Text(item.label).tag(item)
                    }
                }
                .pickerStyle(.segmented)
                Picker("Values", selection: $base) {
                    ForEach(BinaryNumberBase.allCases) { item in
                        Text(item.label).tag(item)
                    }
                }
                .pickerStyle(.segmented)
            }
            .labelsHidden()
            .padding(10)
            Divider()

            if let patch = state.patch {
                if layout == .raw {
                    rawView(patch)
                } else {
                    logicalView(patch)
                }
            } else {
                ContentUnavailableView("No patch selected", systemImage: "doc.binary")
            }
        }
    }

    private func rawView(_ patch: PatchRecord) -> some View {
        let changed = Set(state.differences.map(\.offset))
        return List(rows(for: patch.data)) { row in
            HStack(spacing: 10) {
                Text(formatOffset(row.offset))
                    .foregroundStyle(.secondary)
                Text(formatBytes(row.bytes))
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(row.ascii)
                    .foregroundStyle(.secondary)
            }
            .font(.system(size: 10, design: .monospaced))
            .listRowBackground(
                row.range.contains(where: changed.contains)
                    ? Color.orange.opacity(0.18)
                    : Color.clear
            )
        }
    }

    private func logicalView(_ patch: PatchRecord) -> some View {
        let changed = Set(state.differences.map(\.offset))
        return List {
            Section("Patch record") {
                logicalRow(
                    LogicalField(
                        id: "patch.slot",
                        label: "Destination slot",
                        offset: 0,
                        length: 4,
                        value: .number(Int(patch.slotIndex))
                    ),
                    changed: changed
                )
                logicalRow(
                    LogicalField(
                        id: "patch.name",
                        label: "Patch name",
                        offset: state.profile.patchName.offset,
                        length: state.profile.patchName.length,
                        value: .text(patch.name)
                    ),
                    changed: changed
                )
                logicalRow(
                    LogicalField(
                        id: "patch.bpm",
                        label: "BPM",
                        offset: state.profile.bpm.msbOffset,
                        length: 2,
                        value: .number(patch.bpm)
                    ),
                    changed: changed
                )
            }

            Section("Named global fields") {
                ForEach(
                    state.profile.namedFields.sorted { $0.value.offset < $1.value.offset },
                    id: \.key
                ) { name, definition in
                    logicalRow(
                        LogicalField(
                            id: "field.\(name)",
                            label: name,
                            offset: definition.offset,
                            length: 1,
                            value: .number(patch.value(at: definition.offset))
                        ),
                        changed: changed
                    )
                }
            }

            ForEach(state.profile.blocks) { block in
                let modelID = patch.modelID(for: block)
                let model = state.catalog.model(block: block.id, id: modelID)
                Section("\(block.displayName) · \(model?.displayName ?? "model \(modelID)")") {
                    logicalRow(
                        LogicalField(
                            id: "block.\(block.id).selector",
                            label: patch.isBypassed(block) ? "Selector · bypassed" : "Selector · active",
                            offset: block.selectorOffset,
                            length: 1,
                            value: .number(Int(patch.selector(for: block)))
                        ),
                        changed: changed
                    )
                    if let model {
                        ForEach(model.parameters) { parameter in
                            logicalRow(
                                LogicalField(
                                    id: "block.\(block.id).\(parameter.name)",
                                    label: parameter.displayName ?? parameter.name,
                                    offset: parameter.fileOffset,
                                    length: 1,
                                    value: .number(patch.value(at: parameter.fileOffset))
                                ),
                                changed: changed
                            )
                        }
                    }
                }
            }

            Section("Local IR") {
                logicalRow(
                    LogicalField(
                        id: "ir.present",
                        label: "Presence flag",
                        offset: 0x82,
                        length: 4,
                        value: .number(patch.irPresent ? 1 : 0)
                    ),
                    changed: changed
                )
                logicalRow(
                    LogicalField(
                        id: "ir.name",
                        label: "IR name",
                        offset: 0x86,
                        length: 32,
                        value: .text(patch.irName.isEmpty ? "—" : patch.irName)
                    ),
                    changed: changed
                )
                logicalRow(
                    LogicalField(
                        id: "ir.wave",
                        label: "Embedded RIFF/WAVE region",
                        offset: 0xA6,
                        length: patch.profile.recordSize - 0xA6,
                        value: .text(patch.irPresent ? "8236 bytes" : "empty")
                    ),
                    changed: changed
                )
            }
        }
    }

    private func logicalRow(
        _ field: LogicalField,
        changed: Set<Int>
    ) -> some View {
        HStack(spacing: 10) {
            Text(formatOffset(field.offset))
                .frame(width: 58, alignment: .trailing)
                .foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(field.label)
                Text(field.length == 1 ? "1 byte" : "\(field.length) bytes")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
            Spacer()
            Text(formatValue(field.value))
                .foregroundStyle(.orange)
                .textSelection(.enabled)
        }
        .font(.system(size: 11, design: .monospaced))
        .listRowBackground(
            field.range.contains(where: changed.contains)
                ? Color.orange.opacity(0.18)
                : Color.clear
        )
    }

    private func rows(for data: Data) -> [BinaryRow] {
        stride(from: 0, to: data.count, by: 16).map { offset in
            let end = min(offset + 16, data.count)
            let bytes = Array(data[offset..<end])
            return BinaryRow(
                offset: offset,
                range: offset..<end,
                bytes: bytes,
                ascii: String(bytes.map { byte in
                    (32...126).contains(byte) ? Character(UnicodeScalar(byte)) : "."
                })
            )
        }
    }

    private func formatOffset(_ value: Int) -> String {
        switch base {
        case .hexadecimal: String(format: "0x%04X", value)
        case .decimal: "\(value)"
        }
    }

    private func formatBytes(_ bytes: [UInt8]) -> String {
        switch base {
        case .hexadecimal:
            bytes.map { String(format: "%02X", $0) }.joined(separator: " ")
        case .decimal:
            bytes.map { String(format: "%3d", $0) }.joined(separator: " ")
        }
    }

    private func formatValue(_ value: LogicalField.Value) -> String {
        switch value {
        case .text(let text):
            text
        case .number(let number):
            base == .hexadecimal
                ? String(format: number > 255 ? "0x%X" : "0x%02X", number)
                : "\(number)"
        }
    }
}

private struct BinaryRow: Identifiable {
    let offset: Int
    let range: Range<Int>
    let bytes: [UInt8]
    let ascii: String
    var id: Int { offset }
}

private enum BinaryLayout: String, CaseIterable, Identifiable {
    case logical
    case raw
    var id: String { rawValue }
    var label: String { self == .logical ? "Logical" : "Raw" }
}

private enum BinaryNumberBase: String, CaseIterable, Identifiable {
    case hexadecimal
    case decimal
    var id: String { rawValue }
    var label: String { self == .hexadecimal ? "Hex" : "Decimal" }
}

private struct LogicalField: Identifiable {
    enum Value {
        case number(Int)
        case text(String)
    }

    let id: String
    let label: String
    let offset: Int
    let length: Int
    let value: Value
    var range: Range<Int> { offset..<(offset + length) }
}

private struct AgentView: View {
    @EnvironmentObject private var state: StudioState

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("AI change planner")
                .font(.headline)
            Text("Provider: \(state.agentProvider.displayName). API keys are configured in Settings and stored in Keychain.")
                .font(.caption)
                .foregroundStyle(.secondary)
            SettingsLink {
                Label("Configure AI providers…", systemImage: "gearshape")
            }
            TextEditor(text: $state.agentPrompt)
                .font(.body)
                .frame(minHeight: 120)
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(.separator))
            Button(state.agentBusy ? "Planning…" : "Create proposal") {
                state.createAgentProposal()
            }
                .buttonStyle(.borderedProminent)
                .disabled(state.patch == nil || state.agentPrompt.isEmpty || state.agentBusy)

            if !state.agentMessage.isEmpty {
                Text(state.agentMessage)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            List(Array(state.proposedOperations.enumerated()), id: \.offset) { _, operation in
                Text(String(describing: operation))
                    .font(.caption.monospaced())
            }

            HStack {
                Button("Discard") { state.proposedOperations.removeAll() }
                Spacer()
                Button("Apply changes") { state.applyAgentProposal() }
                    .buttonStyle(.borderedProminent)
                    .tint(.orange)
                    .disabled(state.proposedOperations.isEmpty)
            }
        }
        .padding()
    }
}
