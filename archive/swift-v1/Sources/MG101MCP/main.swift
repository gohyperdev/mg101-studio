import Foundation
import MCP
import MG101Core
import MG101Tools

private let server = Server(
    name: "mg101-studio",
    version: "1.0.0",
    capabilities: .init(tools: .init(listChanged: false))
)

private let activeProfile = try ProfileLoader.active()
private let profile = activeProfile.profile
private let catalog = activeProfile.catalog

private func toMCPValue(_ json: JSONValue) throws -> MCP.Value {
    let data = try JSONEncoder().encode(json)
    return try JSONDecoder().decode(MCP.Value.self, from: data)
}

private func toArguments(_ params: CallTool.Parameters) throws -> [String: JSONValue] {
    let args = params.arguments ?? [:]
    let data = try JSONEncoder().encode(args)
    return try JSONDecoder().decode([String: JSONValue].self, from: data)
}

private func text(_ value: String, error: Bool = false) -> CallTool.Result {
    .init(
        content: [.text(text: value, annotations: nil, _meta: nil)],
        isError: error
    )
}

private func writePatch(_ patch: PatchRecord, output: String) throws {
    let outputURL = URL(fileURLWithPath: output)
    guard !FileManager.default.fileExists(atPath: outputURL.path) else {
        throw MCPToolError.exists(output)
    }
    try PatchFileWriter.writeNew(patch.data, to: outputURL)
}

private enum MCPToolError: LocalizedError {
    case missing(String)
    case parameter(String, Int, String)
    case exists(String)
    case unknownTool(String)

    var errorDescription: String? {
        switch self {
        case .missing(let name): "Missing argument: \(name)"
        case .parameter(let block, let model, let name):
            "Unknown parameter \(block).\(model).\(name)"
        case .exists(let path): "Output already exists: \(path)"
        case .unknownTool(let name): "Unknown tool: \(name)"
        }
    }
}

private func executeFileCommand(_ command: DomainCommand) throws -> String {
    switch command {
    case .setParameter(let target, let blockID, let parameterName, let value):
        guard case .file(let input, let output) = target else { throw MCPToolError.missing("Target") }
        var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
        let block = try profile.block(blockID)
        let modelID = patch.modelID(for: block)
        guard let model = catalog.model(block: blockID, id: modelID),
              let parameter = model.parameters.first(where: { $0.name == parameterName })
        else {
            throw MCPToolError.parameter(blockID, modelID, parameterName)
        }
        try patch.setParameter(parameter, value: value)
        try writePatch(patch, output: output)
        return "Wrote \(blockID).\(parameterName)=\(value) to \(output)"
        
    case .setModel(let target, let blockID, let modelID, let values, let bypassed):
        guard case .file(let input, let output) = target else { throw MCPToolError.missing("Target") }
        let block = try profile.block(blockID)
        guard let model = catalog.model(block: blockID, id: modelID) else {
            throw ProfileError.unknownModel(blockID, modelID)
        }
        var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
        try patch.setModel(model, block: block, values: values, bypassed: bypassed)
        try writePatch(patch, output: output)
        return "Wrote \(blockID) model \(modelID) to \(output)"
        
    case .setBypass(let target, let blockID, let bypassed):
        guard case .file(let input, let output) = target else { throw MCPToolError.missing("Target") }
        var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
        patch.setBypass(bypassed, block: try profile.block(blockID))
        try writePatch(patch, output: output)
        return "Wrote bypass state for \(blockID) to \(output)"
        
    case .setName(let target, let name):
        guard case .file(let input, let output) = target else { throw MCPToolError.missing("Target") }
        var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
        try patch.setName(name)
        try writePatch(patch, output: output)
        return "Wrote patch name to \(output)"
        
    case .setBPM(let target, let bpm):
        guard case .file(let input, let output) = target else { throw MCPToolError.missing("Target") }
        var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
        try patch.setBPM(bpm)
        try writePatch(patch, output: output)
        return "Wrote BPM to \(output)"
        
    case .setNamedField(let target, let field, let value):
        guard case .file(let input, let output) = target else { throw MCPToolError.missing("Target") }
        var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
        try patch.setNamedField(field, value: value)
        try writePatch(patch, output: output)
        return "Wrote \(field) to \(output)"
        
    case .setIR(let target, let wavPath, let name):
        guard case .file(let input, let output) = target else { throw MCPToolError.missing("Target") }
        var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
        try patch.setIR(wav: try Data(contentsOf: URL(fileURLWithPath: wavPath)), name: name)
        try writePatch(patch, output: output)
        return "Embedded IR in \(output)"
        
    case .clearIR(let target):
        guard case .file(let input, let output) = target else { throw MCPToolError.missing("Target") }
        var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
        patch.clearIR()
        try writePatch(patch, output: output)
        return "Cleared embedded IR in \(output)"
        
    default:
        throw MCPToolError.unknownTool(String(describing: command))
    }
}

private func inspect(path: String) throws -> String {
    let patch = try PatchRecord(contentsOf: URL(fileURLWithPath: path), profile: profile)
    let blocks = profile.blocks.map { block in
        let modelID = patch.modelID(for: block)
        let model = catalog.model(block: block.id, id: modelID)
        return [
            "block": block.id,
            "model_id": modelID,
            "model": model?.displayName ?? "unknown",
            "bypassed": patch.isBypassed(block),
        ] as [String: Any]
    }
    let value: [String: Any] = [
        "path": path,
        "size": patch.data.count,
        "slot": patch.slotIndex,
        "name": patch.name,
        "bpm": patch.bpm,
        "ir": [
            "present": patch.irPresent,
            "name": patch.irName,
        ],
        "named_fields": profile.namedFields.mapValues {
            patch.value(at: $0.offset)
        },
        "blocks": blocks,
    ]
    let data = try JSONSerialization.data(
        withJSONObject: value,
        options: [.prettyPrinted, .sortedKeys]
    )
    return String(decoding: data, as: UTF8.self)
}

await server.withMethodHandler(ListTools.self) { _ in
    do {
        let tools = try ToolDefinition.allDefinitions(variant: .file).map { toolDef in
            let schemaVal = try toMCPValue(toolDef.inputSchema)
            return Tool(
                name: toolDef.name,
                description: toolDef.description,
                inputSchema: schemaVal,
                annotations: .init(
                    readOnlyHint: toolDef.kind == .read,
                    destructiveHint: toolDef.kind == .destructive,
                    idempotentHint: false,
                    openWorldHint: toolDef.kind == .filesystem
                )
            )
        }
        return .init(tools: tools)
    } catch {
        return .init(tools: [])
    }
}

await server.withMethodHandler(CallTool.self) { params in
    do {
        if params.name == "inspect_patch" {
            let args = try toArguments(params)
            guard let path = args["path"]?.stringValue else {
                throw MCPToolError.missing("path")
            }
            return text(try inspect(path: path))
        }
        
        if params.name == "list_models" {
            let args = try toArguments(params)
            guard let block = args["block"]?.stringValue else {
                throw MCPToolError.missing("block")
            }
            let models = catalog.models(for: block)
            let data = try JSONEncoder().encode(models)
            return text(String(decoding: data, as: UTF8.self))
        }
        
        let args = try toArguments(params)
        let cmd = try DomainCommand.parse(name: params.name, arguments: args)
        let outputMessage = try executeFileCommand(cmd)
        return text(outputMessage)
    } catch {
        return text(error.localizedDescription, error: true)
    }
}

let transport = StdioTransport()
try await server.start(transport: transport)
while !Task.isCancelled {
    try await Task.sleep(for: .seconds(86_400))
}
