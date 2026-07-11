import MG101Core
import MG101Tools
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
    @State private var showingSettings = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let session = state.currentSession {
                sessionActiveView(session: session)
            } else {
                sessionListView
            }
        }
    }

    @ViewBuilder
    private var sessionListView: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("Sesje Agenta AI")
                    .font(.title2)
                    .bold()
                Spacer()
                Button(action: { state.startNewSession() }) {
                    Label("Nowa Sesja", systemImage: "plus")
                }
                .buttonStyle(.borderedProminent)
            }
            .padding(.bottom, 8)

            Text("Wybierz istniejącą sesję lub utwórz nową, aby edytować presety za pomocą chatu.")
                .font(.body)
                .foregroundStyle(.secondary)

            ScrollView {
                VStack(spacing: 12) {
                    ForEach(state.sessionStore.sessions) { session in
                        HStack {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(session.title)
                                    .font(.headline)
                                HStack(spacing: 8) {
                                    Text("Stan: \(session.state.rawValue)")
                                        .font(.caption)
                                        .foregroundStyle(session.state == .active ? .green : .secondary)
                                    Text("Tury: \(session.turnCount)")
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                            }
                            Spacer()
                            Button(action: { state.selectSession(session) }) {
                                Text("Otwórz")
                            }
                            .buttonStyle(.bordered)
                            
                            Button(role: .destructive, action: {
                                try? state.sessionStore.deleteSession(id: session.id)
                            }) {
                                Image(systemName: "trash")
                            }
                            .buttonStyle(.borderless)
                            .tint(.red)
                        }
                        .padding()
                        .background(RoundedRectangle(cornerRadius: 12).fill(Color(nsColor: .controlBackgroundColor)))
                        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color.gray.opacity(0.25), lineWidth: 1))
                    }
                }
            }
        }
        .padding()
    }

    @ViewBuilder
    private func sessionActiveView(session: AgentSessionMetadata) -> some View {
        VStack(spacing: 0) {
            // Toolbar
            HStack {
                Button(action: { state.currentSession = nil }) {
                    Image(systemName: "chevron.left")
                }
                .buttonStyle(.borderless)

                Menu {
                    Button("Nowa Rozmowa") {
                        state.startNewSession()
                    }
                    Divider()
                    ForEach(state.sessionStore.sessions) { s in
                        Button(s.title) {
                            state.selectSession(s)
                        }
                    }
                } label: {
                    Text(session.title)
                        .font(.headline)
                        .lineLimit(1)
                }
                .menuStyle(.borderlessButton)

                Spacer()

                HStack(spacing: 12) {
                    Text("Tury: \(session.turnCount)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    
                    if session.state == .active {
                        Button("Commit") {
                            state.commitSession()
                        }
                        .buttonStyle(.borderedProminent)
                        .tint(.green)

                        Button("Revert") {
                            try? state.revertSession()
                        }
                        .buttonStyle(.bordered)
                        .tint(.red)
                    } else {
                        Text("Stan: \(session.state.rawValue.uppercased())")
                            .font(.caption)
                            .bold()
                            .foregroundStyle(session.state == .committed ? .blue : .gray)
                    }
                }
            }
            .padding()
            .background(Color(nsColor: .windowBackgroundColor))

            Divider()

            // Chat Messages List
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        ForEach(state.chatMessages) { msg in
                            messageBubble(msg: msg)
                        }
                        
                        if !state.agentMessage.isEmpty {
                            streamingBubble(text: state.agentMessage)
                        }
                    }
                    .padding()
                }
                .onChange(of: state.chatMessages.count) { _ in
                    if let last = state.chatMessages.last {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }

            Divider()

            // Confirmations
            if let pending = state.pendingConfirmation {
                VStack(spacing: 12) {
                    Text("Autoryzacja operacji")
                        .font(.headline)
                        .foregroundStyle(.orange)
                    
                    Text(describeCommand(pending.command))
                        .font(.caption)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal)
                    
                    HStack(spacing: 20) {
                        Button("Odmów", role: .cancel) {
                            pending.continuation.resume(returning: false)
                            state.pendingConfirmation = nil
                        }
                        .buttonStyle(.bordered)
                        
                        Button("Zezwól") {
                            pending.continuation.resume(returning: true)
                            state.pendingConfirmation = nil
                        }
                        .buttonStyle(.borderedProminent)
                        .tint(.green)
                    }
                }
                .padding()
                .background(.ultraThinMaterial)
                .cornerRadius(12)
                .overlay(RoundedRectangle(cornerRadius: 12).stroke(.orange.opacity(0.5)))
                .padding()
            }

            // Input Area
            if session.state == .active {
                HStack(alignment: .bottom) {
                    TextField("Napisz prompt do AI...", text: $state.agentPrompt, axis: .vertical)
                        .font(.body)
                        .lineLimit(1...5)
                        .textFieldStyle(.roundedBorder)
                        .disabled(state.agentBusy)
                        .onSubmit {
                            if !state.agentPrompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !state.agentBusy {
                                state.runAgent()
                            }
                        }

                    Button(action: { state.runAgent() }) {
                        if state.agentBusy {
                            ProgressView()
                                .controlSize(.small)
                        } else {
                            Image(systemName: "paperplane.fill")
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(state.agentPrompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || state.agentBusy)
                }
                .padding()
                .background(Color(nsColor: .windowBackgroundColor))
            } else {
                Text("Ta sesja została zamknięta (\(session.state.rawValue)). Załóż nową sesję, aby edytować.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding()
            }
        }
    }

    @ViewBuilder
    private func messageBubble(msg: ChatMessage) -> some View {
        HStack(alignment: .top) {
            if msg.role == "user" {
                Spacer()
                VStack(alignment: .trailing, spacing: 4) {
                    if !msg.content.isEmpty {
                        Text(msg.content)
                            .padding(10)
                            .background(Color.blue)
                            .foregroundStyle(.white)
                            .cornerRadius(12)
                    }
                    if let results = msg.toolResults {
                        VStack(alignment: .trailing, spacing: 4) {
                            ForEach(results, id: \.toolUseID) { res in
                                Text("🔧 \(res.toolUseID.prefix(8)): \(res.isError ? "BŁĄD" : "OK")")
                                    .font(.system(.caption2, design: .monospaced))
                                    .padding(4)
                                    .background(res.isError ? Color.red.opacity(0.2) : Color.green.opacity(0.2))
                                    .cornerRadius(6)
                            }
                        }
                    }
                }
            } else {
                VStack(alignment: .leading, spacing: 4) {
                    if !msg.content.isEmpty {
                        Text(msg.content)
                            .padding(10)
                            .background(Color(nsColor: .controlBackgroundColor))
                            .cornerRadius(12)
                            .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color.gray.opacity(0.25), lineWidth: 1))
                    }
                    if let calls = msg.toolCalls {
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(calls, id: \.id) { call in
                                Text("🔧 Wywołanie: \(call.name)")
                                    .font(.system(.caption2, design: .monospaced))
                                    .padding(4)
                                    .background(Color.orange.opacity(0.2))
                                    .cornerRadius(6)
                            }
                        }
                    }
                }
                Spacer()
            }
        }
        .id(msg.id)
    }

    @ViewBuilder
    private func streamingBubble(text: String) -> some View {
        HStack {
            Text(text)
                .padding(10)
                .background(Color(nsColor: .controlBackgroundColor).opacity(0.8))
                .cornerRadius(12)
                .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color.gray.opacity(0.25), lineWidth: 1))
            Spacer()
        }
    }

    private func describeCommand(_ command: DomainCommand) -> String {
        switch command {
        case .deletePatch(let target):
            if case .library(let patchID, _) = target {
                return "Miękkie usunięcie patcha o ID: \(patchID)"
            }
            return "Miękkie usunięcie patcha"
        case .revertSession:
            return "Cofnięcie całej sesji (powrót do stanu początkowego)"
        case .importPatch(let path):
            return "Import patcha ze ścieżki: \(path)"
        case .exportPatch(_, let path):
            return "Eksport patcha do ścieżki: \(path)"
        case .setIR(_, let path, let name):
            return "Osadzenie pliku IR '\(name)' ze ścieżki: \(path)"
        default:
            return "Wykonanie operacji: \(command)"
        }
    }
}
