import AppKit
import Foundation
import MG101Core

@MainActor
final class StudioState: ObservableObject {
    @Published private(set) var patches: [LibraryPatch] = []
    @Published var selectedPatchID: UUID? {
        didSet {
            if selectedPatchID != oldValue {
                undoStack.removeAll()
                redoStack.removeAll()
                proposedOperations.removeAll()
            }
        }
    }
    @Published var showingSetExporter = false
    @Published var setSelection: Set<UUID> = []
    @Published var selectedBlockID = "amp"
    @Published var errorMessage: String?
    @Published var agentPrompt = ""
    @Published var proposedOperations: [PatchOperation] = []
    @Published var agentMessage = ""
    @Published var agentBusy = false
    @Published var agentProvider = AgentProvider(
        rawValue: UserDefaults.standard.string(forKey: "agent.provider") ?? ""
    ) ?? .anthropic
    @Published var openAIEndpoint = UserDefaults.standard.string(
        forKey: "agent.openai.endpoint"
    ) ?? UserDefaults.standard.string(
        forKey: "agent.endpoint"
    ) ?? "https://api.openai.com/v1/chat/completions"
    @Published var openAIModel = UserDefaults.standard.string(
        forKey: "agent.openai.model"
    ) ?? UserDefaults.standard.string(
        forKey: "agent.model"
    ) ?? ""
    @Published var openAIAPIKey = ""
    @Published var anthropicEndpoint = UserDefaults.standard.string(
        forKey: "agent.anthropic.endpoint"
    ) ?? "https://api.anthropic.com/v1/messages"
    @Published var anthropicModel = UserDefaults.standard.string(
        forKey: "agent.anthropic.model"
    ) ?? "claude-sonnet-4-20250514"
    @Published var anthropicAPIKey = ""
    @Published var settingsMessage = ""

    @Published private(set) var profile: DeviceProfile
    @Published private(set) var catalog: EffectCatalog
    @Published private(set) var profileSource: URL?
    private var undoStack: [PatchRecord] = []
    private var redoStack: [PatchRecord] = []

    init() {
        do {
            let active = try ProfileLoader.active()
            profile = active.profile
            catalog = active.catalog
            profileSource = active.source
            patches = try PatchLibraryStore.load(profile: active.profile)
            selectedPatchID = patches.first?.id
        } catch {
            fatalError("Cannot load bundled MG-101 profile: \(error)")
        }
    }

    private var selectedPatchIndex: Int? {
        guard let selectedPatchID else { return nil }
        return patches.firstIndex { $0.id == selectedPatchID }
    }

    var selectedLibraryPatch: LibraryPatch? {
        guard let selectedPatchIndex else { return nil }
        return patches[selectedPatchIndex]
    }

    var patch: PatchRecord? { selectedLibraryPatch?.patch }
    var original: PatchRecord? { selectedLibraryPatch?.original }

    var selectedBlock: DeviceProfile.Block? {
        profile.blocks.first { $0.id == selectedBlockID }
    }

    var selectedModel: EffectCatalog.Model? {
        guard let patch, let block = selectedBlock else { return nil }
        return catalog.model(block: block.id, id: patch.modelID(for: block))
    }

    var differences: [ByteDifference] {
        guard let patch, let original else { return [] }
        return patch.differences(from: original)
    }

    func importPatchPanel() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.data]
        panel.allowsMultipleSelection = true
        panel.message = "Import one or more MG-101 patches or 36-patch sets"
        guard panel.runModal() == .OK else { return }
        importPatches(panel.urls)
    }

    func importSetPanel() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.data]
        panel.allowsMultipleSelection = false
        panel.message = "Import one 36-patch MG101AllPatch set"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        perform {
            let records = try PatchCollection.records(from: url, profile: profile)
            guard records.count == PatchCollection.deviceSetCount else {
                throw PatchCollectionError.recordCount(
                    expected: PatchCollection.deviceSetCount,
                    actual: records.count
                )
            }
            try appendImported(records, sourceName: url.lastPathComponent)
        }
    }

    func open(_ url: URL) {
        importPatches([url])
    }

    func importPatches(_ urls: [URL]) {
        perform {
            var firstImportedID: UUID?
            for url in urls {
                let records = try PatchCollection.records(from: url, profile: profile)
                let ids = try appendImported(records, sourceName: url.lastPathComponent)
                firstImportedID = firstImportedID ?? ids.first
            }
            selectedPatchID = firstImportedID ?? selectedPatchID
            undoStack.removeAll()
            redoStack.removeAll()
        }
    }

    func importProfilePanel() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.message = "Choose a folder containing device-profile.json and effects-catalog.json"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        perform {
            let loaded = try ProfileLoader.install(from: url)
            activateProfile(loaded.0, loaded.1, source: try ProfileLoader.applicationSupportDirectory())
        }
    }

    func useBundledProfile() {
        perform {
            try ProfileLoader.removeActiveOverride()
            let loaded = try ProfileLoader.bundled()
            activateProfile(loaded.0, loaded.1, source: nil)
        }
    }

    func exportCurrentPatch() {
        guard let item = selectedLibraryPatch else { return }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "\(item.patch.name).mg101patch"
        panel.message = "Export a new patch file"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        perform {
            try PatchFileWriter.writeNew(item.patch.data, to: url)
        }
    }

    func beginSetExport() {
        setSelection = Set(patches.prefix(PatchCollection.deviceSetCount).map(\.id))
        showingSetExporter = true
    }

    func toggleSetSelection(_ id: UUID) {
        if setSelection.contains(id) {
            setSelection.remove(id)
        } else if setSelection.count < PatchCollection.deviceSetCount {
            setSelection.insert(id)
        }
    }

    func exportSelectedSet() {
        let selected = patches.filter { setSelection.contains($0.id) }
        guard selected.count == PatchCollection.deviceSetCount else {
            errorMessage = PatchCollectionError.recordCount(
                expected: PatchCollection.deviceSetCount,
                actual: selected.count
            ).localizedDescription
            return
        }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "MG101AllPatch.mg101patch"
        panel.message = "Export the selected 36 patches in library order"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        perform {
            let data = try PatchCollection.deviceSet(
                from: selected.map(\.patch),
                profile: profile
            )
            try PatchFileWriter.writeNew(data, to: url)
            showingSetExporter = false
        }
    }

    func removeSelectedPatch() {
        guard let index = selectedPatchIndex, !patches[index].isFactory else { return }
        perform {
            let removed = patches[index]
            try PatchLibraryStore.remove(removed)
            patches.remove(at: index)
            selectedPatchID = patches.indices.contains(index)
                ? patches[index].id
                : patches.last?.id
        }
    }

    func update(_ body: (inout PatchRecord) throws -> Void) {
        guard let index = selectedPatchIndex else { return }
        var next = patches[index].patch
        perform {
            let previous = next
            try body(&next)
            guard next.data != previous.data else { return }
            undoStack.append(previous)
            redoStack.removeAll()
            patches[index].patch = next
            if patches[index].isFactory {
                patches[index].origin = .editedFactory
                patches[index].sourceName = "Edited factory copy"
            }
            try PatchLibraryStore.persist(patches[index])
        }
    }

    func setParameter(_ parameter: EffectCatalog.Parameter, value: Int) {
        update { try $0.setParameter(parameter, value: value) }
    }

    func setBypass(_ value: Bool) {
        guard let block = selectedBlock else { return }
        update { $0.setBypass(value, block: block) }
    }

    func setName(_ value: String) {
        update { try $0.setName(value) }
    }

    func setBPM(_ value: Int) {
        update { try $0.setBPM(value) }
    }

    func setNamedField(_ name: String, value: Int) {
        update { try $0.setNamedField(name, value: value) }
    }

    func chooseIR() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.wav]
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        update {
            try $0.setIR(
                wav: Data(contentsOf: url),
                name: url.deletingPathExtension().lastPathComponent
            )
        }
    }

    func clearIR() {
        update { $0.clearIR() }
    }

    func changeModel(_ modelID: Int) {
        guard let block = selectedBlock,
              let target = catalog.model(block: block.id, id: modelID)
        else { return }
        update { record in
            let values = target.parameters.map { parameter in
                min(parameter.maximum, max(parameter.minimum, record.value(at: parameter.fileOffset)))
            }
            try record.setModel(
                target,
                block: block,
                values: values,
                bypassed: record.isBypassed(block)
            )
        }
    }

    func undo() {
        guard let index = selectedPatchIndex,
              let previous = undoStack.popLast()
        else { return }
        let current = patches[index].patch
        redoStack.append(current)
        patches[index].patch = previous
        try? PatchLibraryStore.persist(patches[index])
    }

    func redo() {
        guard let index = selectedPatchIndex,
              let next = redoStack.popLast()
        else { return }
        let current = patches[index].patch
        undoStack.append(current)
        patches[index].patch = next
        try? PatchLibraryStore.persist(patches[index])
    }

    func createAgentProposal() {
        guard let patch else { return }
        if agentProvider == .anthropic && anthropicAPIKey.isEmpty {
            loadAPIKeys(allowInteraction: true)
        }
        let configuration = activeAgentConfiguration

        guard configuration.isConfigured else {
            proposedOperations = LocalAgentPlanner.plan(
                prompt: agentPrompt,
                patch: patch,
                profile: profile,
                catalog: catalog
            )
            agentMessage = "Local planner used. Configure provider credentials in Settings for an AI-generated plan."
            return
        }

        agentBusy = true
        Task {
            do {
                let plan = try await AgentAPIClient.plan(
                    prompt: agentPrompt,
                    patch: patch,
                    profile: profile,
                    catalog: catalog,
                    configuration: configuration
                )
                proposedOperations = try plan.operations.map { try $0.operation() }
                agentMessage = plan.message
                errorMessage = nil
            } catch {
                errorMessage = error.localizedDescription
            }
            agentBusy = false
        }
    }

    var activeAgentConfiguration: AgentConfiguration {
        switch agentProvider {
        case .openAICompatible:
            AgentConfiguration(
                provider: .openAICompatible,
                endpoint: openAIEndpoint,
                model: openAIModel,
                apiKey: openAIAPIKey
            )
        case .anthropic:
            AgentConfiguration(
                provider: .anthropic,
                endpoint: anthropicEndpoint,
                model: anthropicModel,
                apiKey: anthropicAPIKey
            )
        }
    }

    func saveAISettings() {
        settingsMessage = ""
        perform {
            guard URL(string: openAIEndpoint) != nil else {
                throw AgentAPIError.endpoint
            }
            guard URL(string: anthropicEndpoint) != nil else {
                throw AgentAPIError.endpoint
            }
            UserDefaults.standard.set(agentProvider.rawValue, forKey: "agent.provider")
            UserDefaults.standard.set(openAIEndpoint, forKey: "agent.openai.endpoint")
            UserDefaults.standard.set(openAIModel, forKey: "agent.openai.model")
            UserDefaults.standard.set(anthropicEndpoint, forKey: "agent.anthropic.endpoint")
            UserDefaults.standard.set(anthropicModel, forKey: "agent.anthropic.model")
            try KeychainStore.write(openAIAPIKey, account: "openai")
            try KeychainStore.write(anthropicAPIKey, account: "anthropic")
            settingsMessage = "Settings and keys saved."
        }
    }

    func loadAPIKeys(allowInteraction: Bool) {
        do {
            openAIAPIKey = try KeychainStore.read(
                account: "openai",
                allowInteraction: allowInteraction
            )
            anthropicAPIKey = try KeychainStore.read(
                account: "anthropic",
                allowInteraction: allowInteraction
            )
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func applyAgentProposal() {
        for operation in proposedOperations {
            switch operation {
            case .setBPM(let bpm):
                update { try $0.setBPM(bpm) }
            case .setName(let name):
                update { try $0.setName(name) }
            case .setNamedField(let name, let value):
                update { try $0.setNamedField(name, value: value) }
            case .setBypass(let blockID, let value):
                guard let block = try? profile.block(blockID) else { continue }
                update { $0.setBypass(value, block: block) }
            case .setParameter(let blockID, let modelID, let name, let value):
                guard let model = catalog.model(block: blockID, id: modelID),
                      let parameter = model.parameters.first(where: { $0.name == name })
                else { continue }
                update { try $0.setParameter(parameter, value: value) }
            case .setByte(let offset, let value):
                update { try $0.setByte(value, at: offset) }
            case .setModel(let blockID, let modelID, let values, let bypassed):
                guard let block = try? profile.block(blockID),
                      let model = catalog.model(block: blockID, id: modelID)
                else { continue }
                update {
                    try $0.setModel(
                        model,
                        block: block,
                        values: values,
                        bypassed: bypassed
                    )
                }
            }
        }
        proposedOperations.removeAll()
    }

    private func perform(_ work: () throws -> Void) {
        do {
            try work()
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    @discardableResult
    private func appendImported(
        _ records: [PatchRecord],
        sourceName: String
    ) throws -> [UUID] {
        var imported: [LibraryPatch] = []
        for record in records {
            let item = LibraryPatch(
                id: UUID(),
                patch: record,
                original: record,
                sourceName: sourceName,
                origin: .imported
            )
            try PatchLibraryStore.persist(item)
            imported.append(item)
        }
        patches.append(contentsOf: imported)
        selectedPatchID = imported.first?.id ?? selectedPatchID
        return imported.map(\.id)
    }

    private func activateProfile(
        _ newProfile: DeviceProfile,
        _ newCatalog: EffectCatalog,
        source: URL?
    ) {
        profile = newProfile
        catalog = newCatalog
        profileSource = source
        patches = (try? PatchLibraryStore.load(profile: newProfile)) ?? []
        selectedPatchID = patches.first?.id
        selectedBlockID = newProfile.blocks.first?.id ?? ""
        proposedOperations.removeAll()
        undoStack.removeAll()
        redoStack.removeAll()
    }
}
