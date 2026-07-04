import MG101Core
import SwiftUI

struct ContentView: View {
    @EnvironmentObject private var state: StudioState

    var body: some View {
        NavigationSplitView {
            List(selection: $state.selectedPatchID) {
                ForEach(Array(state.patches.enumerated()), id: \.element.id) { index, item in
                    HStack(spacing: 8) {
                        Text(String(format: "%02d", index + 1))
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.secondary)
                            .frame(width: 25, alignment: .trailing)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(item.patch.name.isEmpty ? "Unnamed patch" : item.patch.name)
                                .lineLimit(1)
                            Text(item.origin.rawValue)
                                .font(.caption2)
                                .foregroundStyle(item.isFactory ? Color.secondary : Color.orange)
                        }
                        Spacer()
                        if item.patch.irPresent {
                            Image(systemName: "waveform.path")
                                .foregroundStyle(.blue)
                        }
                    }
                    .tag(item.id)
                }
            }
            .navigationTitle("Patch Library")
            .navigationSplitViewColumnWidth(min: 320, ideal: 340, max: 420)
            .safeAreaInset(edge: .bottom) {
                sidebarActions
            }
        } content: {
            VStack(spacing: 0) {
                chain
                Divider()
                if state.patch == nil {
                    ContentUnavailableView(
                        "No patch selected",
                        systemImage: "waveform.path.ecg",
                        description: Text("Import patches or select one from the library.")
                    )
                } else {
                    BlockEditorView()
                }
            }
            .navigationTitle(state.patch?.name ?? "MG101 Studio")
        } detail: {
            InspectorView()
                .frame(minWidth: 340)
        }
        .toolbar {
            Menu("Profile", systemImage: "gearshape.2") {
                Button("Import Profile Folder…") { state.importProfilePanel() }
                Button("Use Bundled Profile") { state.useBundledProfile() }
                    .disabled(state.profileSource == nil)
            }
        }
        .sheet(isPresented: $state.showingSetExporter) {
            SetExportView()
                .environmentObject(state)
        }
        .alert(
            "Operation failed",
            isPresented: Binding(
                get: { state.errorMessage != nil },
                set: { if !$0 { state.errorMessage = nil } }
            )
        ) {
            Button("OK") { state.errorMessage = nil }
        } message: {
            Text(state.errorMessage ?? "")
        }
        .onOpenURL { state.open($0) }
    }

    private var sidebarActions: some View {
        VStack(spacing: 10) {
            HStack(spacing: 6) {
                Button {
                    state.importPatchPanel()
                } label: {
                    Label("Import Files", systemImage: "square.and.arrow.down")
                }
                Button {
                    state.importSetPanel()
                } label: {
                    Label("Import Set", systemImage: "square.stack.3d.down.right")
                }
            }
            .buttonStyle(.bordered)

            HStack(spacing: 6) {
                Button {
                    state.exportCurrentPatch()
                } label: {
                    Label("Export Patch", systemImage: "square.and.arrow.up")
                }
                .disabled(state.patch == nil)
                Button {
                    state.beginSetExport()
                } label: {
                    Label("Export Set", systemImage: "square.stack.3d.up")
                }
                .disabled(state.patches.count < PatchCollection.deviceSetCount)
            }
            .buttonStyle(.bordered)

            HStack {
                Button(role: .destructive) {
                    state.removeSelectedPatch()
                } label: {
                    Label("Remove from Library", systemImage: "trash")
                }
                .disabled(state.selectedLibraryPatch?.isFactory != false)
            }
            .buttonStyle(.bordered)

            Divider()
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(state.patches.count) patches")
                        .font(.caption.bold())
                    Text(state.profileSource == nil ? "Bundled profile" : "External profile")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                Spacer()
            }
        }
        .padding()
        .frame(maxWidth: .infinity)
        .background(.bar)
    }

    private var chain: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                ForEach(state.profile.blocks) { block in
                    Button {
                        state.selectedBlockID = block.id
                    } label: {
                        VStack(spacing: 4) {
                            Image(systemName: icon(block.id))
                            Text(block.displayName)
                                .font(.caption.bold())
                        }
                        .frame(width: 70, height: 54)
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(state.selectedBlockID == block.id ? .orange : blockColor(block))
                }
            }
            .padding()
        }
        .background(.black.opacity(0.22))
    }

    private func blockColor(_ block: DeviceProfile.Block) -> Color {
        guard let patch = state.patch else { return .gray }
        return patch.isBypassed(block) ? .gray : .green
    }

    private func icon(_ id: String) -> String {
        [
            "amp": "amplifier", "dly": "repeat", "rvb": "water.waves",
            "eq": "slider.horizontal.3", "cab": "hifispeaker",
            "mod": "waveform", "efx": "bolt", "cmp": "arrow.down.right.and.arrow.up.left",
        ][id] ?? "circle.hexagongrid"
    }
}

private struct SetExportView: View {
    @EnvironmentObject private var state: StudioState

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Export device set")
                    .font(.title2.bold())
                Text("Select exactly 36 patches. Library order becomes device slot order 1–36.")
                    .foregroundStyle(.secondary)
                ProgressView(
                    value: Double(state.setSelection.count),
                    total: Double(PatchCollection.deviceSetCount)
                )
                Text("\(state.setSelection.count) / \(PatchCollection.deviceSetCount) selected")
                    .font(.caption.monospacedDigit())
            }
            .padding()
            .frame(maxWidth: .infinity, alignment: .leading)

            List {
                ForEach(Array(state.patches.enumerated()), id: \.element.id) { index, item in
                    Toggle(
                        isOn: Binding(
                            get: { state.setSelection.contains(item.id) },
                            set: { _ in state.toggleSetSelection(item.id) }
                        )
                    ) {
                        HStack {
                            Text(String(format: "%02d", index + 1))
                                .font(.caption.monospacedDigit())
                                .foregroundStyle(.secondary)
                            Text(item.patch.name)
                            Spacer()
                            Text(item.origin.rawValue)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .toggleStyle(.checkbox)
                    .disabled(
                        !state.setSelection.contains(item.id)
                        && state.setSelection.count >= PatchCollection.deviceSetCount
                    )
                }
            }

            HStack {
                Button("Cancel") { state.showingSetExporter = false }
                Spacer()
                Button("Export 36-patch set…") { state.exportSelectedSet() }
                    .buttonStyle(.borderedProminent)
                    .disabled(state.setSelection.count != PatchCollection.deviceSetCount)
            }
            .padding()
            .background(.bar)
        }
        .frame(minWidth: 620, minHeight: 650)
    }
}
