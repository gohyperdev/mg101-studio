import AppKit
import Foundation
import MG101Core
import MG101Tools

struct ConfirmationRequest: Identifiable, Sendable {
    let id = UUID()
    let command: DomainCommand
    let continuation: CheckedContinuation<Bool, Never>
}

@MainActor
final class StudioState: ObservableObject {
    @Published private(set) var patches: [LibraryPatch] = []
    @Published var selectedPatchID: PatchID? {
        didSet {
            if selectedPatchID != oldValue {
                undoStack.removeAll()
                redoStack.removeAll()
                proposedOperations.removeAll()
            }
        }
    }
    @Published var showingSetExporter = false
    @Published var setSelection: Set<PatchID> = []
    @Published var selectedBlockID = "amp"
    @Published var errorMessage: String?
    @Published var activeSessionID: String?

    var activeJournal: WALJournal? {
        guard let id = activeSessionID else { return nil }
        guard let url = try? SessionStore.sessionDirectory(id: id).appendingPathComponent("journal.jsonl") else { return nil }
        return WALJournal(url: url)
    }

    @Published var pendingConfirmation: ConfirmationRequest?
    @Published var chatMessages: [ChatMessage] = []
    @Published var currentSession: AgentSessionMetadata?
    let sessionStore = SessionStore()
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
    private var bridgeServer: MCPBridgeServer?

    init() {
        do {
            let active = try ProfileLoader.active()
            profile = active.profile
            catalog = active.catalog
            profileSource = active.source
            
            // WAL Recovery
            recoverDanglingTransactions()
            
            patches = try PatchLibraryStore.load(profile: active.profile)
            selectedPatchID = patches.first?.id

            // Start MCPBridgeServer
            do {
                let server = try MCPBridgeServer(state: self)
                server.start()
                self.bridgeServer = server
            } catch {
                print("Failed to start MCPBridge: \(error)")
            }
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
            var firstImportedID: PatchID?
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
        guard activeSessionID == nil else {
            errorMessage = "Nie można zmienić profilu urządzenia przy aktywnej sesji rozmowy."
            return
        }
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
        guard activeSessionID == nil else {
            errorMessage = "Nie można zmienić profilu urządzenia przy aktywnej sesji rozmowy."
            return
        }
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

    func toggleSetSelection(_ id: PatchID) {
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
            try PatchLibraryStore.updateIndex(patches: patches)
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
            patches[index].revision += 1
            if patches[index].isFactory {
                patches[index].origin = .editedFactory
                patches[index].sourceName = "Edited factory copy"
                try PatchLibraryStore.saveOriginal(patches[index].original, id: patches[index].id)
            }
            try PatchLibraryStore.persist(patches[index])
            try PatchLibraryStore.updateIndex(patches: patches)
        }
    }

    func mutatePatch(id: PatchID, expectedRevision: Int, body: (inout PatchRecord) throws -> Void) throws {
        guard let index = patches.firstIndex(where: { $0.id == id }) else {
            throw ToolExecutionError.notFound(id)
        }
        guard patches[index].revision == expectedRevision else {
            throw ToolExecutionError.conflict(currentRevision: patches[index].revision)
        }
        var next = patches[index].patch
        let previous = next
        try body(&next)
        guard next.data != previous.data else { return }
        
        var walEntry: TransactionEntry?
        if let journal = activeJournal {
            let sequence = (try? journal.loadEntries().count) ?? 0
            let entry = TransactionEntry(
                sequence: sequence,
                timestamp: Date(),
                toolName: "mutate",
                patchID: id,
                revisionBefore: patches[index].revision,
                inverse: .restoreBytes(patchID: id, blob: previous.data),
                beforeHash: previous.data.sha256Hex(),
                afterHash: next.data.sha256Hex(),
                state: .prepared
            )
            try journal.append(entry)
            walEntry = entry
        }
        
        if id == selectedPatchID {
            undoStack.append(previous)
            redoStack.removeAll()
        }
        
        patches[index].patch = next
        patches[index].revision += 1
        
        if patches[index].isFactory {
            patches[index].origin = .editedFactory
            patches[index].sourceName = "Edited factory copy"
            try PatchLibraryStore.saveOriginal(patches[index].original, id: patches[index].id)
        }
        
        try PatchLibraryStore.persist(patches[index])
        try PatchLibraryStore.updateIndex(patches: patches)
        
        if let journal = activeJournal, var entry = walEntry {
            entry.state = .committed
            try journal.append(entry)
        }
    }

    func duplicatePatch(id: PatchID, expectedRevision: Int) throws -> PatchID {
        guard let index = patches.firstIndex(where: { $0.id == id }) else {
            throw ToolExecutionError.notFound(id)
        }
        guard patches[index].revision == expectedRevision else {
            throw ToolExecutionError.conflict(currentRevision: patches[index].revision)
        }
        
        let source = patches[index]
        let newID = UUID().uuidString
        
        var walEntry: TransactionEntry?
        if let journal = activeJournal {
            let sequence = (try? journal.loadEntries().count) ?? 0
            let entry = TransactionEntry(
                sequence: sequence,
                timestamp: Date(),
                toolName: "duplicate",
                patchID: newID,
                revisionBefore: 0,
                inverse: .removePatch(patchID: newID),
                beforeHash: "",
                afterHash: source.patch.data.sha256Hex(),
                state: .prepared
            )
            try journal.append(entry)
            walEntry = entry
        }
        
        let copy = LibraryPatch(
            id: newID,
            patch: source.patch,
            original: source.original,
            sourceName: "Copy of \(source.patch.name)",
            origin: .imported,
            revision: 1
        )
        
        try PatchLibraryStore.persist(copy)
        try PatchLibraryStore.saveOriginal(source.original, id: newID)
        patches.append(copy)
        try PatchLibraryStore.updateIndex(patches: patches)
        
        if let journal = activeJournal, var entry = walEntry {
            entry.state = .committed
            try journal.append(entry)
        }
        return newID
    }

    func deletePatch(id: PatchID, expectedRevision: Int) throws {
        guard let index = patches.firstIndex(where: { $0.id == id }) else {
            throw ToolExecutionError.notFound(id)
        }
        guard patches[index].revision == expectedRevision else {
            throw ToolExecutionError.conflict(currentRevision: patches[index].revision)
        }
        let removed = patches[index]
        
        var walEntry: TransactionEntry?
        if let sessionID = activeSessionID, let journal = activeJournal {
            let sequence = (try? journal.loadEntries().count) ?? 0
            let meta = PatchLibraryStore.makeStagedMetadata(removed)
            let entry = TransactionEntry(
                sequence: sequence,
                timestamp: Date(),
                toolName: "delete",
                patchID: id,
                revisionBefore: removed.revision,
                inverse: .restoreFromStaging(patchID: id, meta: meta),
                beforeHash: removed.patch.data.sha256Hex(),
                afterHash: "",
                state: .prepared
            )
            try journal.append(entry)
            walEntry = entry
            
            try PatchLibraryStore.stagePhysicalFiles(removed, sessionID: sessionID)
        } else {
            try PatchLibraryStore.remove(removed)
        }
        
        patches.remove(at: index)
        if selectedPatchID == id {
            selectedPatchID = patches.indices.contains(index)
                ? patches[index].id
                : patches.last?.id
        }
        try PatchLibraryStore.updateIndex(patches: patches)
        
        if let journal = activeJournal, var entry = walEntry {
            entry.state = .committed
            try journal.append(entry)
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
        patches[index].revision += 1
        try? PatchLibraryStore.persist(patches[index])
        try? PatchLibraryStore.updateIndex(patches: patches)
    }

    func redo() {
        guard let index = selectedPatchIndex,
              let next = redoStack.popLast()
        else { return }
        let current = patches[index].patch
        undoStack.append(current)
        patches[index].patch = next
        patches[index].revision += 1
        try? PatchLibraryStore.persist(patches[index])
        try? PatchLibraryStore.updateIndex(patches: patches)
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
            
            do {
                try KeychainStore.write(openAIAPIKey, account: "openai")
                UserDefaults.standard.removeObject(forKey: "agent.openai.apikey.fallback")
            } catch {
                UserDefaults.standard.set(openAIAPIKey, forKey: "agent.openai.apikey.fallback")
            }

            do {
                try KeychainStore.write(anthropicAPIKey, account: "anthropic")
                UserDefaults.standard.removeObject(forKey: "agent.anthropic.apikey.fallback")
            } catch {
                UserDefaults.standard.set(anthropicAPIKey, forKey: "agent.anthropic.apikey.fallback")
            }

            settingsMessage = "Settings and keys saved."
        }
    }

    func loadAPIKeys(allowInteraction: Bool) {
        do {
            openAIAPIKey = try KeychainStore.read(
                account: "openai",
                allowInteraction: allowInteraction
            )
        } catch {
            openAIAPIKey = UserDefaults.standard.string(forKey: "agent.openai.apikey.fallback") ?? ProcessInfo.processInfo.environment["OPENAI_API_KEY"] ?? ""
        }

        do {
            anthropicAPIKey = try KeychainStore.read(
                account: "anthropic",
                allowInteraction: allowInteraction
            )
        } catch {
            anthropicAPIKey = UserDefaults.standard.string(forKey: "agent.anthropic.apikey.fallback") ?? ProcessInfo.processInfo.environment["ANTHROPIC_API_KEY"] ?? ""
        }

        if openAIAPIKey.isEmpty {
            openAIAPIKey = UserDefaults.standard.string(forKey: "agent.openai.apikey.fallback") ?? ProcessInfo.processInfo.environment["OPENAI_API_KEY"] ?? ""
        }
        if anthropicAPIKey.isEmpty {
            anthropicAPIKey = UserDefaults.standard.string(forKey: "agent.anthropic.apikey.fallback") ?? ProcessInfo.processInfo.environment["ANTHROPIC_API_KEY"] ?? ""
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
    ) throws -> [PatchID] {
        var imported: [LibraryPatch] = []
        for record in records {
            let item = LibraryPatch(
                id: UUID().uuidString,
                patch: record,
                original: record,
                sourceName: sourceName,
                origin: .imported,
                revision: 1
            )
            try PatchLibraryStore.persist(item)
            try PatchLibraryStore.saveOriginal(record, id: item.id)
            imported.append(item)
        }
        patches.append(contentsOf: imported)
        selectedPatchID = imported.first?.id ?? selectedPatchID
        try PatchLibraryStore.updateIndex(patches: patches)
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

    func revertSession() throws {
        guard let sessionID = activeSessionID, let journal = activeJournal else {
            throw ToolExecutionError.custom("No active session or journal found")
        }
        let entries = try journal.loadEntries()
        let committedSeqs = Set(entries.filter { $0.state == .committed }.map(\.sequence))
        let toRevert = entries.filter { $0.state == .prepared && (committedSeqs.contains($0.sequence) || $0.sequence == entries.last?.sequence) }
            .sorted { $0.sequence > $1.sequence }
        
        for entry in toRevert {
            try applyInverse(entry, sessionID: sessionID)
        }
        
        try? PatchLibraryStore.clearStaging(sessionID: sessionID)
        try journal.rewrite([])
        
        self.patches = try PatchLibraryStore.load(profile: profile)
        if selectedPatchID == nil || !patches.contains(where: { $0.id == selectedPatchID }) {
            selectedPatchID = patches.first?.id
        }
    }
    
    func revertLastAgentAction() throws {
        guard let sessionID = activeSessionID, let journal = activeJournal else {
            throw ToolExecutionError.custom("No active session or journal found")
        }
        let entries = try journal.loadEntries()
        let committed = entries.filter { $0.state == .committed }.sorted { $0.sequence > $1.sequence }
        guard let lastCommitted = committed.first else {
            throw ToolExecutionError.custom("No committed actions to revert")
        }
        
        try applyInverse(lastCommitted, sessionID: sessionID)
        
        let filtered = entries.filter { $0.sequence != lastCommitted.sequence }
        try journal.rewrite(filtered)
        
        self.patches = try PatchLibraryStore.load(profile: profile)
        if selectedPatchID == nil || !patches.contains(where: { $0.id == selectedPatchID }) {
            selectedPatchID = patches.first?.id
        }
    }
    
    private func applyInverse(_ entry: TransactionEntry, sessionID: String) throws {
        switch entry.inverse {
        case .restoreBytes(let id, let blob):
            guard let index = patches.firstIndex(where: { $0.id == id }) else {
                throw ToolExecutionError.custom("Revert conflict: patch \(id) missing from library")
            }
            let currentHash = patches[index].patch.data.sha256Hex()
            guard currentHash == entry.afterHash else {
                throw ToolExecutionError.custom("Revert conflict: patch \(id) has manual edits")
            }
            var item = patches[index]
            item.patch = try PatchRecord(data: blob, profile: profile)
            item.revision = entry.revisionBefore
            
            if id.hasPrefix("factory-") && item.patch.data == item.original.data {
                item.origin = .factory
                let indexInt = Int(id.dropFirst(8)) ?? 1
                item.sourceName = String(format: "Factory %02d", indexInt)
                let directory = try FileManager.default.url(
                    for: .applicationSupportDirectory,
                    in: .userDomainMask,
                    appropriateFor: nil,
                    create: true
                ).appendingPathComponent("MG101Studio", isDirectory: true)
                 .appendingPathComponent("PatchLibrary", isDirectory: true)
                let patchURL = directory.appendingPathComponent(id).appendingPathExtension("mg101patch")
                try? FileManager.default.removeItem(at: patchURL)
            } else {
                try PatchLibraryStore.persist(item)
            }
            
        case .removePatch(let id):
            guard let index = patches.firstIndex(where: { $0.id == id }) else {
                return
            }
            let currentHash = patches[index].patch.data.sha256Hex()
            guard currentHash == entry.afterHash else {
                throw ToolExecutionError.custom("Revert conflict: duplicated patch \(id) has manual edits")
            }
            let removed = patches[index]
            try PatchLibraryStore.remove(removed)
            
        case .restoreFromStaging(let id, let meta):
            guard !patches.contains(where: { $0.id == id }) else {
                throw ToolExecutionError.custom("Revert conflict: patch \(id) already exists")
            }
            try PatchLibraryStore.restoreFromStaging(id: id, sessionID: sessionID, meta: meta)
        }
    }
    
    func recoverDanglingTransactions() {
        do {
            let sessionsDir = try SessionStore.sessionsDirectory()
            guard FileManager.default.fileExists(atPath: sessionsDir.path) else { return }
            let sessionDirs = try FileManager.default.contentsOfDirectory(
                at: sessionsDir,
                includingPropertiesForKeys: nil,
                options: [.skipsHiddenFiles]
            )
            for sessionDir in sessionDirs {
                let journalURL = sessionDir.appendingPathComponent("journal.jsonl")
                let metaURL = sessionDir.appendingPathComponent("session.json")
                guard FileManager.default.fileExists(atPath: journalURL.path),
                      FileManager.default.fileExists(atPath: metaURL.path)
                else { continue }
                
                let journal = WALJournal(url: journalURL)
                let entries = try journal.loadEntries()
                let prepared = entries.filter { $0.state == .prepared }
                let committed = Set(entries.filter { $0.state == .committed }.map(\.sequence))
                
                var dangling = prepared.filter { !committed.contains($0.sequence) }
                if !dangling.isEmpty {
                    dangling.sort { $0.sequence > $1.sequence }
                    let sessionID = sessionDir.lastPathComponent
                    
                    let libraryDir = try PatchLibraryStore.libraryDirectory()
                    
                    for entry in dangling {
                        let patchFile = libraryDir.appendingPathComponent(entry.patchID).appendingPathExtension("mg101patch")
                        let fileExists = FileManager.default.fileExists(atPath: patchFile.path)
                        let currentHash: String
                        if fileExists, let data = try? Data(contentsOf: patchFile) {
                            currentHash = data.sha256Hex()
                        } else {
                            currentHash = ""
                        }
                        
                        if currentHash == entry.beforeHash {
                            let filtered = entries.filter { $0.sequence != entry.sequence }
                            try journal.rewrite(filtered)
                        } else if currentHash == entry.afterHash {
                            var committedEntry = entry
                            committedEntry.state = .committed
                            try journal.append(committedEntry)
                        } else {
                            switch entry.inverse {
                            case .restoreBytes(_, let blob):
                                try blob.write(to: patchFile, options: .atomic)
                            case .removePatch(let id):
                                if fileExists {
                                    try? FileManager.default.removeItem(at: patchFile)
                                    let originalFile = libraryDir.appendingPathComponent(".originals").appendingPathComponent(id).appendingPathExtension("mg101patch")
                                    try? FileManager.default.removeItem(at: originalFile)
                                }
                            case .restoreFromStaging(let id, let meta):
                                try? PatchLibraryStore.restoreFromStaging(id: id, sessionID: sessionID, meta: meta)
                            }
                            let filtered = entries.filter { $0.sequence != entry.sequence }
                            try journal.rewrite(filtered)
                        }
                    }
                }
            }
        } catch {
            print("WAL Recovery failed: \(error)")
        }
    }

    func startNewSession() {
        perform {
            // Make a unique hash representing active profile to bind session
            let profileHash = Self.calculateProfileHash()
            let path = try FileManager.default.url(
                for: .applicationSupportDirectory,
                in: .userDomainMask,
                appropriateFor: nil,
                create: true
            ).appendingPathComponent("MG101Studio/PatchLibrary").path
            
            let meta = try sessionStore.createSession(
                provider: agentProvider.displayName,
                model: agentProvider == .anthropic ? anthropicModel : openAIModel,
                profileHash: profileHash,
                libraryPath: path
            )
            self.currentSession = meta
            self.activeSessionID = meta.id
            self.chatMessages = []
        }
    }
    
    func selectSession(_ session: AgentSessionMetadata) {
        perform {
            let currentHash = Self.calculateProfileHash()
            guard session.profileHash == currentHash else {
                throw ToolExecutionError.custom("Nie można wznowić sesji: profil powiązany z sesją nie zgadza się z aktualnym profilem urządzenia.")
            }
            self.currentSession = session
            self.activeSessionID = session.id
            self.chatMessages = try sessionStore.loadMessages(sessionID: session.id)
        }
    }
    
    func runAgent() {
        guard let session = currentSession else { return }
        let configuration = activeAgentConfiguration
        
        agentBusy = true
        agentMessage = ""
        errorMessage = nil
        
        let sessionID = session.id
        
        let system = """
        You are the native NUX MG-101 Preset Editor agent.
        You communicate via multi-turn tool calling.
        Never invent blocks, model ids, parameter names or ranges.
        Use tools to list patches, get detail, mutate blocks/models/parameters, or duplicate/delete presets.
        Always verify current revisions before mutating.
        """
        
        let executor = GUIToolExecutor(state: self)
        
        Task {
            do {
                var approvedRoots = session.approvedRoots
                
                let loop = AgentLoop(
                    configuration: configuration,
                    executor: executor,
                    systemPrompt: system,
                    sessionID: sessionID,
                    needsConfirmation: { [weak self] cmd in
                        guard let self else { return false }
                        return await withCheckedContinuation { continuation in
                            Task { @MainActor in
                                self.pendingConfirmation = ConfirmationRequest(
                                    command: cmd,
                                    continuation: continuation
                                )
                            }
                        }
                    },
                    onMessageStream: { [weak self] chunk in
                        guard let self else { return }
                        Task { @MainActor in
                            self.agentMessage.append(chunk)
                        }
                    }
                )
                
                let userMsg = ChatMessage(role: "user", content: agentPrompt)
                await MainActor.run {
                    self.chatMessages.append(userMsg)
                    self.agentPrompt = ""
                }
                try sessionStore.appendMessage(sessionID: sessionID, message: userMsg)
                
                let updatedHistory = try await loop.run(history: chatMessages, approvedRoots: &approvedRoots)
                
                try await MainActor.run {
                    var updatedSession = session
                    updatedSession.approvedRoots = approvedRoots
                    updatedSession.turnCount += 1
                    updatedSession.updatedAt = Date()
                    
                    self.currentSession = updatedSession
                    try self.sessionStore.saveSessionMetadata(updatedSession)
                    
                    let newMsgs = updatedHistory.suffix(from: self.chatMessages.count)
                    for newMsg in newMsgs {
                        try self.sessionStore.appendMessage(sessionID: sessionID, message: newMsg)
                    }
                    self.chatMessages = updatedHistory
                    self.agentMessage = ""
                }
            } catch {
                await MainActor.run {
                    self.errorMessage = error.localizedDescription
                }
            }
            await MainActor.run {
                self.agentBusy = false
            }
        }
    }
    
    func commitSession() {
        guard var session = currentSession else { return }
        perform {
            session.state = .committed
            session.updatedAt = Date()
            self.currentSession = session
            try sessionStore.saveSessionMetadata(session)
            try? PatchLibraryStore.clearStaging(sessionID: session.id)
        }
    }

    static func calculateProfileHash() -> String {
        do {
            let directory = try ProfileLoader.applicationSupportDirectory()
            let profileURL = directory.appendingPathComponent(ProfileLoader.profileFileName)
            let catalogURL = directory.appendingPathComponent(ProfileLoader.catalogFileName)

            var profileData: Data
            var catalogData: Data

            if FileManager.default.fileExists(atPath: profileURL.path),
               FileManager.default.fileExists(atPath: catalogURL.path) {
                profileData = try Data(contentsOf: profileURL)
                catalogData = try Data(contentsOf: catalogURL)
            } else {
                guard let bundledProfileURL = ProfileLoader.bundledResourceURL(forResource: "device-profile", withExtension: "json"),
                      let bundledCatalogURL = ProfileLoader.bundledResourceURL(forResource: "effects-catalog", withExtension: "json")
                else {
                    return "default"
                }
                profileData = try Data(contentsOf: bundledProfileURL)
                catalogData = try Data(contentsOf: bundledCatalogURL)
            }

            var combined = Data()
            combined.append(profileData)
            combined.append(catalogData)
            return combined.sha256Hex()
        } catch {
            return "default"
        }
    }

    func importPatch(path: String) throws -> [PatchID] {
        let url = URL(fileURLWithPath: path)
        let records = try PatchCollection.records(from: url, profile: profile)
        guard !records.isEmpty else { return [] }

        var importedIDs: [PatchID] = []
        var walEntries: [TransactionEntry] = []
        let count = (try? activeJournal?.loadEntries().count) ?? 0

        for (index, record) in records.enumerated() {
            let newID = UUID().uuidString

            if let journal = activeJournal {
                let entry = TransactionEntry(
                    sequence: count + index,
                    timestamp: Date(),
                    toolName: "import_patch",
                    patchID: newID,
                    revisionBefore: 0,
                    inverse: .removePatch(patchID: newID),
                    beforeHash: "",
                    afterHash: record.data.sha256Hex(),
                    state: .prepared
                )
                try journal.append(entry)
                walEntries.append(entry)
            }

            let item = LibraryPatch(
                id: newID,
                patch: record,
                original: record,
                sourceName: url.deletingPathExtension().lastPathComponent,
                origin: .imported,
                revision: 1
            )
            try PatchLibraryStore.persist(item)
            try PatchLibraryStore.saveOriginal(record, id: item.id)
            patches.append(item)
            importedIDs.append(newID)
        }

        selectedPatchID = importedIDs.first ?? selectedPatchID
        try PatchLibraryStore.updateIndex(patches: patches)

        if let journal = activeJournal {
            for entry in walEntries {
                var commit = entry
                commit.state = .committed
                try journal.append(commit)
            }
        }

        return importedIDs
    }

    func reloadPatches() throws {
        self.patches = try PatchLibraryStore.load(profile: profile)
    }
}
