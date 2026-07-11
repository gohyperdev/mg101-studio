import Foundation
import MG101Core
import MG101Tools

enum ToolExecutionError: Error, LocalizedError {
    case notFound(PatchID)
    case conflict(currentRevision: Int)
    case custom(String)

    var errorDescription: String? {
        switch self {
        case .notFound(let id): "Patch not found: \(id)"
        case .conflict(let rev): "Concurrency conflict: expected revision matches but current is \(rev)"
        case .custom(let str): str
        }
    }
}

@MainActor
final class GUIToolExecutor {
    private let state: StudioState

    init(state: StudioState) {
        self.state = state
    }

    func execute(command: DomainCommand) async throws -> JSONValue {
        switch command {
        case .listPatches:
            let list = state.patches.enumerated().map { index, item in
                let isModified = item.patch.data != item.original.data
                return JSONValue.object([
                    "patchID": .string(item.id),
                    "slot": .number(Double(index + 1)),
                    "name": .string(item.patch.name),
                    "origin": .string(item.origin.rawValue),
                    "revision": .number(Double(item.revision)),
                    "isModified": .bool(isModified)
                ])
            }
            return .array(list)

        case .getPatch(let patchID):
            guard let item = state.patches.first(where: { $0.id == patchID }) else {
                throw ToolExecutionError.notFound(patchID)
            }
            let patch = item.patch
            let blocks = state.profile.blocks.map { block in
                let modelID = patch.modelID(for: block)
                let current = state.catalog.model(block: block.id, id: modelID)
                let parameters = current?.parameters.map { param in
                    JSONValue.object([
                        "name": .string(param.name),
                        "value": .number(Double(patch.value(at: param.fileOffset))),
                        "minimum": .number(Double(param.minimum)),
                        "maximum": .number(Double(param.maximum))
                    ])
                } ?? []
                return JSONValue.object([
                    "block": .string(block.id),
                    "current_model": .number(Double(modelID)),
                    "current_model_name": .string(current?.displayName ?? "unknown"),
                    "bypassed": .bool(patch.isBypassed(block)),
                    "parameters": .array(parameters)
                ])
            }
            let namedFields = state.profile.namedFields.mapValues { field in
                JSONValue.object([
                    "value": .number(Double(patch.value(at: field.offset))),
                    "minimum": .number(Double(field.minimum)),
                    "maximum": .number(Double(field.maximum))
                ])
            }
            return .object([
                "patchID": .string(item.id),
                "name": .string(patch.name),
                "bpm": .number(Double(patch.bpm)),
                "revision": .number(Double(item.revision)),
                "ir": .object([
                    "present": .bool(patch.irPresent),
                    "name": .string(patch.irName)
                ]),
                "blocks": .array(blocks),
                "named_fields": .object(namedFields)
            ])

        case .getSelection:
            return .object([
                "selectedPatchID": state.selectedPatchID.map { .string($0) } ?? .null,
                "selectedBlockID": .string(state.selectedBlockID)
            ])

        case .listModels(let block):
            let models = state.catalog.models(for: block)
            let list = models.map { model in
                JSONValue.object([
                    "modelID": .number(Double(model.modelID)),
                    "displayName": .string(model.displayName),
                    "parameters": .array(model.parameters.map { param in
                        JSONValue.object([
                            "name": .string(param.name),
                            "minimum": .number(Double(param.minimum)),
                            "maximum": .number(Double(param.maximum))
                        ])
                    })
                ])
            }
            return .array(list)

        case .getProfile:
            let blocks = state.profile.blocks.map { block in
                JSONValue.object([
                    "id": .string(block.id),
                    "displayName": .string(block.displayName)
                ])
            }
            let namedFields = state.profile.namedFields.mapValues { field in
                JSONValue.object([
                    "minimum": .number(Double(field.minimum)),
                    "maximum": .number(Double(field.maximum))
                ])
            }
            return .object([
                "recordSize": .number(Double(state.profile.recordSize)),
                "blocks": .array(blocks),
                "namedFields": .object(namedFields)
            ])

        case .getDiff(let patchID):
            guard let item = state.patches.first(where: { $0.id == patchID }) else {
                throw ToolExecutionError.notFound(patchID)
            }
            let diffs = item.patch.differences(from: item.original)
            let list = diffs.map { diff in
                JSONValue.object([
                    "offset": .number(Double(diff.offset)),
                    "before": .number(Double(diff.before)),
                    "after": .number(Double(diff.after))
                ])
            }
            return .array(list)

        case .setParameter(let target, let blockID, let parameterName, let value):
            guard case .library(let patchID, let revision) = target else {
                throw ToolExecutionError.custom("GUIToolExecutor requires library target")
            }
            try state.mutatePatch(id: patchID, expectedRevision: revision) { patch in
                let block = try state.profile.block(blockID)
                let modelID = patch.modelID(for: block)
                guard let model = state.catalog.model(block: blockID, id: modelID),
                      let parameter = model.parameters.first(where: { $0.name == parameterName })
                else {
                    throw ToolExecutionError.custom("Unknown parameter \(blockID).\(parameterName)")
                }
                try patch.setParameter(parameter, value: value)
            }
            return .object(["success": .bool(true)])

        case .setModel(let target, let blockID, let modelID, let values, let bypassed):
            guard case .library(let patchID, let revision) = target else {
                throw ToolExecutionError.custom("GUIToolExecutor requires library target")
            }
            try state.mutatePatch(id: patchID, expectedRevision: revision) { patch in
                let block = try state.profile.block(blockID)
                guard let model = state.catalog.model(block: blockID, id: modelID) else {
                    throw ToolExecutionError.custom("Unknown model \(blockID).\(modelID)")
                }
                try patch.setModel(model, block: block, values: values, bypassed: bypassed)
            }
            return .object(["success": .bool(true)])

        case .setBypass(let target, let blockID, let bypassed):
            guard case .library(let patchID, let revision) = target else {
                throw ToolExecutionError.custom("GUIToolExecutor requires library target")
            }
            try state.mutatePatch(id: patchID, expectedRevision: revision) { patch in
                let block = try state.profile.block(blockID)
                patch.setBypass(bypassed, block: block)
            }
            return .object(["success": .bool(true)])

        case .setName(let target, let name):
            guard case .library(let patchID, let revision) = target else {
                throw ToolExecutionError.custom("GUIToolExecutor requires library target")
            }
            try state.mutatePatch(id: patchID, expectedRevision: revision) { patch in
                try patch.setName(name)
            }
            return .object(["success": .bool(true)])

        case .setBPM(let target, let bpm):
            guard case .library(let patchID, let revision) = target else {
                throw ToolExecutionError.custom("GUIToolExecutor requires library target")
            }
            try state.mutatePatch(id: patchID, expectedRevision: revision) { patch in
                try patch.setBPM(bpm)
            }
            return .object(["success": .bool(true)])

        case .setNamedField(let target, let field, let value):
            guard case .library(let patchID, let revision) = target else {
                throw ToolExecutionError.custom("GUIToolExecutor requires library target")
            }
            try state.mutatePatch(id: patchID, expectedRevision: revision) { patch in
                try patch.setNamedField(field, value: value)
            }
            return .object(["success": .bool(true)])

        case .clearIR(let target):
            guard case .library(let patchID, let revision) = target else {
                throw ToolExecutionError.custom("GUIToolExecutor requires library target")
            }
            try state.mutatePatch(id: patchID, expectedRevision: revision) { patch in
                IRService.clearIR(in: &patch)
            }
            return .object(["success": .bool(true)])

        case .duplicatePatch(let target):
            guard case .library(let patchID, let revision) = target else {
                throw ToolExecutionError.custom("GUIToolExecutor requires library target")
            }
            let newID = try state.duplicatePatch(id: patchID, expectedRevision: revision)
            return .object(["success": .bool(true), "newPatchID": .string(newID)])

        case .revertLastAgentAction:
            try state.revertLastAgentAction()
            return .object(["success": .bool(true)])

        case .selectPatch(let patchID):
            guard state.patches.contains(where: { $0.id == patchID }) else {
                throw ToolExecutionError.notFound(patchID)
            }
            state.selectedPatchID = patchID
            return .object(["success": .bool(true)])

        case .setIR(let target, let wavPath, let name):
            guard case .library(let patchID, let revision) = target else {
                throw ToolExecutionError.custom("GUIToolExecutor requires library target")
            }
            let data = try Data(contentsOf: URL(fileURLWithPath: wavPath))
            try state.mutatePatch(id: patchID, expectedRevision: revision) { patch in
                try IRService.embedIR(wavData: data, name: name, into: &patch)
            }
            return .object(["success": .bool(true)])

        case .importPatch(let path):
            let ids = try state.importPatch(path: path)
            return .object(["success": .bool(true), "importedPatchIDs": .array(ids.map { .string($0) })])

        case .exportPatch(let target, let destinationPath):
            guard case .library(let patchID, let revision) = target else {
                throw ToolExecutionError.custom("GUIToolExecutor requires library target")
            }
            guard let index = state.patches.firstIndex(where: { $0.id == patchID }) else {
                throw ToolExecutionError.notFound(patchID)
            }
            guard state.patches[index].revision == revision else {
                throw ToolExecutionError.conflict(currentRevision: state.patches[index].revision)
            }
            
            var destURL = URL(fileURLWithPath: destinationPath)
            var isDir: ObjCBool = false
            if FileManager.default.fileExists(atPath: destURL.path, isDirectory: &isDir), isDir.boolValue {
                let name = state.patches[index].patch.name.trimmingCharacters(in: .whitespacesAndNewlines)
                let filename = name.isEmpty ? patchID : name
                destURL = destURL.appendingPathComponent(filename).appendingPathExtension("mg101patch")
            }
            
            try PatchExportService.exportPatch(state.patches[index].patch, to: destURL)
            return .object(["success": .bool(true)])

        case .listFiles(let path):
            let url = URL(fileURLWithPath: path)
            var isDir: ObjCBool = false
            guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir), isDir.boolValue else {
                throw ToolExecutionError.custom("Path is not a directory or does not exist")
            }
            let files = try FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
            let filenames = files.map { $0.lastPathComponent }
            return .object(["success": .bool(true), "files": .array(filenames.map { .string($0) })])

        case .deletePatch(let target):
            guard case .library(let patchID, let revision) = target else {
                throw ToolExecutionError.custom("GUIToolExecutor requires library target")
            }
            try state.deletePatch(id: patchID, expectedRevision: revision)
            return .object(["success": .bool(true)])

        case .revertSession:
            try state.revertSession()
            return .object(["success": .bool(true)])
        }
    }
}
