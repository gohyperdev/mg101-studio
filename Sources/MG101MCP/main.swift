import Foundation
import MCP
import MG101Core

private let server = Server(
    name: "mg101-studio",
    version: "1.0.0",
    capabilities: .init(tools: .init(listChanged: false))
)

private let activeProfile = try ProfileLoader.active()
private let profile = activeProfile.profile
private let catalog = activeProfile.catalog

private func schema(
    properties: [String: Value],
    required: [String] = []
) -> Value {
    .object([
        "type": .string("object"),
        "properties": .object(properties),
        "required": .array(required.map(Value.string)),
        "additionalProperties": .bool(false),
    ])
}

private func stringProperty(_ description: String) -> Value {
    .object(["type": .string("string"), "description": .string(description)])
}

private func integerProperty(_ description: String) -> Value {
    .object(["type": .string("integer"), "description": .string(description)])
}

private func text(_ value: String, error: Bool = false) -> CallTool.Result {
    .init(
        content: [.text(text: value, annotations: nil, _meta: nil)],
        isError: error
    )
}

private func argument(
    _ params: CallTool.Parameters,
    _ name: String
) throws -> String {
    guard let value = params.arguments?[name]?.stringValue, !value.isEmpty else {
        throw MCPToolError.missing(name)
    }
    return value
}

private func integerArgument(
    _ params: CallTool.Parameters,
    _ name: String
) throws -> Int {
    guard let value = params.arguments?[name]?.intValue else {
        throw MCPToolError.missing(name)
    }
    return value
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

private func writeParameter(
    input: String,
    output: String,
    blockID: String,
    parameterName: String,
    value: Int
) throws -> String {
    var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
    let block = try profile.block(blockID)
    let modelID = patch.modelID(for: block)
    guard let model = catalog.model(block: blockID, id: modelID),
          let parameter = model.parameters.first(where: { $0.name == parameterName })
    else {
        throw MCPToolError.parameter(blockID, modelID, parameterName)
    }
    try patch.setParameter(parameter, value: value)
    let outputURL = URL(fileURLWithPath: output)
    guard !FileManager.default.fileExists(atPath: outputURL.path) else {
        throw MCPToolError.exists(output)
    }
    try PatchFileWriter.writeNew(patch.data, to: outputURL)
    return "Wrote \(blockID).\(parameterName)=\(value) to \(output)"
}

private func writePatch(_ patch: PatchRecord, output: String) throws {
    let outputURL = URL(fileURLWithPath: output)
    guard !FileManager.default.fileExists(atPath: outputURL.path) else {
        throw MCPToolError.exists(output)
    }
    try PatchFileWriter.writeNew(patch.data, to: outputURL)
}

private func boolArgument(
    _ params: CallTool.Parameters,
    _ name: String,
    default fallback: Bool? = nil
) throws -> Bool {
    if let value = params.arguments?[name]?.boolValue { return value }
    if let fallback { return fallback }
    throw MCPToolError.missing(name)
}

private func integerArrayArgument(
    _ params: CallTool.Parameters,
    _ name: String
) throws -> [Int] {
    guard let values = params.arguments?[name]?.arrayValue else {
        throw MCPToolError.missing(name)
    }
    return try values.map {
        guard let value = $0.intValue else { throw MCPToolError.missing(name) }
        return value
    }
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

await server.withMethodHandler(ListTools.self) { _ in
    .init(tools: [
        Tool(
            name: "inspect_patch",
            description: "Read a MG-101 patch without modifying it.",
            inputSchema: schema(
                properties: ["path": stringProperty("Absolute path to a .mg101patch file.")],
                required: ["path"]
            ),
            annotations: .init(readOnlyHint: true, openWorldHint: false)
        ),
        Tool(
            name: "list_models",
            description: "List models and parameter ranges from the active device profile.",
            inputSchema: schema(
                properties: ["block": stringProperty("Block id such as amp, efx, dly or cab.")],
                required: ["block"]
            ),
            annotations: .init(readOnlyHint: true, openWorldHint: false)
        ),
        Tool(
            name: "set_parameter",
            description: "Create a new patch file with one model-aware parameter changed. Existing files are never overwritten.",
            inputSchema: schema(
                properties: [
                    "input": stringProperty("Absolute source patch path."),
                    "output": stringProperty("Absolute destination path."),
                    "block": stringProperty("Block id."),
                    "parameter": stringProperty("Parameter name for the currently selected model."),
                    "value": integerProperty("Raw value validated against the active model."),
                ],
                required: ["input", "output", "block", "parameter", "value"]
            ),
            annotations: .init(
                readOnlyHint: false,
                destructiveHint: false,
                idempotentHint: false,
                openWorldHint: false
            )
        ),
        Tool(
            name: "set_bypass",
            description: "Create a new patch with one block enabled or bypassed.",
            inputSchema: schema(
                properties: [
                    "input": stringProperty("Absolute source patch path."),
                    "output": stringProperty("Absolute destination path."),
                    "block": stringProperty("Block id."),
                    "bypassed": .object(["type": .string("boolean")]),
                ],
                required: ["input", "output", "block", "bypassed"]
            ),
            annotations: .init(readOnlyHint: false, destructiveHint: false, openWorldHint: false)
        ),
        Tool(
            name: "set_model",
            description: "Create a new patch with a model and its full active parameter set.",
            inputSchema: schema(
                properties: [
                    "input": stringProperty("Absolute source patch path."),
                    "output": stringProperty("Absolute destination path."),
                    "block": stringProperty("Block id."),
                    "model": integerProperty("Model id from list_models."),
                    "values": .object([
                        "type": .string("array"),
                        "items": .object(["type": .string("integer")]),
                    ]),
                    "bypassed": .object(["type": .string("boolean")]),
                ],
                required: ["input", "output", "block", "model", "values"]
            ),
            annotations: .init(readOnlyHint: false, destructiveHint: false, openWorldHint: false)
        ),
        Tool(
            name: "set_bpm",
            description: "Create a new patch with tempo changed.",
            inputSchema: schema(
                properties: [
                    "input": stringProperty("Absolute source patch path."),
                    "output": stringProperty("Absolute destination path."),
                    "bpm": integerProperty("Tempo in the profile-supported range."),
                ],
                required: ["input", "output", "bpm"]
            ),
            annotations: .init(readOnlyHint: false, destructiveHint: false, openWorldHint: false)
        ),
        Tool(
            name: "set_name",
            description: "Create a new patch with its display name changed.",
            inputSchema: schema(
                properties: [
                    "input": stringProperty("Absolute source patch path."),
                    "output": stringProperty("Absolute destination path."),
                    "name": stringProperty("UTF-8 patch name within the configured byte limit."),
                ],
                required: ["input", "output", "name"]
            ),
            annotations: .init(readOnlyHint: false, destructiveHint: false, openWorldHint: false)
        ),
        Tool(
            name: "set_named_field",
            description: "Create a new patch with a profile-defined global field changed, such as send, return or patch.position.",
            inputSchema: schema(
                properties: [
                    "input": stringProperty("Absolute source patch path."),
                    "output": stringProperty("Absolute destination path."),
                    "field": stringProperty("Key from device-profile.json namedFields."),
                    "value": integerProperty("Value validated against the field range."),
                ],
                required: ["input", "output", "field", "value"]
            ),
            annotations: .init(readOnlyHint: false, destructiveHint: false, openWorldHint: false)
        ),
        Tool(
            name: "set_ir",
            description: "Create a new patch with a device-format local IR embedded.",
            inputSchema: schema(
                properties: [
                    "input": stringProperty("Absolute source patch path."),
                    "output": stringProperty("Absolute destination path."),
                    "wav": stringProperty("Absolute path to the 8236-byte RIFF/WAVE."),
                    "name": stringProperty("IR name, at most 32 UTF-8 bytes."),
                ],
                required: ["input", "output", "wav", "name"]
            ),
            annotations: .init(readOnlyHint: false, destructiveHint: false, openWorldHint: false)
        ),
        Tool(
            name: "clear_ir",
            description: "Create a new patch with the embedded local IR removed.",
            inputSchema: schema(
                properties: [
                    "input": stringProperty("Absolute source patch path."),
                    "output": stringProperty("Absolute destination path."),
                ],
                required: ["input", "output"]
            ),
            annotations: .init(readOnlyHint: false, destructiveHint: false, openWorldHint: false)
        ),
    ])
}

await server.withMethodHandler(CallTool.self) { params in
    do {
        switch params.name {
        case "inspect_patch":
            return text(try inspect(path: argument(params, "path")))
        case "list_models":
            let block = try argument(params, "block")
            let models = catalog.models(for: block)
            let data = try JSONEncoder().encode(models)
            return text(String(decoding: data, as: UTF8.self))
        case "set_parameter":
            return text(
                try writeParameter(
                    input: argument(params, "input"),
                    output: argument(params, "output"),
                    blockID: argument(params, "block"),
                    parameterName: argument(params, "parameter"),
                    value: integerArgument(params, "value")
                )
            )
        case "set_bypass":
            let input = try argument(params, "input")
            let output = try argument(params, "output")
            let blockID = try argument(params, "block")
            var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
            patch.setBypass(
                try boolArgument(params, "bypassed"),
                block: try profile.block(blockID)
            )
            try writePatch(patch, output: output)
            return text("Wrote bypass state for \(blockID) to \(output)")
        case "set_model":
            let input = try argument(params, "input")
            let output = try argument(params, "output")
            let blockID = try argument(params, "block")
            let modelID = try integerArgument(params, "model")
            let block = try profile.block(blockID)
            guard let model = catalog.model(block: blockID, id: modelID) else {
                throw ProfileError.unknownModel(blockID, modelID)
            }
            var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
            try patch.setModel(
                model,
                block: block,
                values: try integerArrayArgument(params, "values"),
                bypassed: try boolArgument(params, "bypassed", default: false)
            )
            try writePatch(patch, output: output)
            return text("Wrote \(blockID) model \(modelID) to \(output)")
        case "set_bpm":
            let input = try argument(params, "input")
            let output = try argument(params, "output")
            var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
            try patch.setBPM(try integerArgument(params, "bpm"))
            try writePatch(patch, output: output)
            return text("Wrote BPM to \(output)")
        case "set_name":
            let input = try argument(params, "input")
            let output = try argument(params, "output")
            var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
            try patch.setName(try argument(params, "name"))
            try writePatch(patch, output: output)
            return text("Wrote patch name to \(output)")
        case "set_named_field":
            let input = try argument(params, "input")
            let output = try argument(params, "output")
            let field = try argument(params, "field")
            var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
            try patch.setNamedField(field, value: try integerArgument(params, "value"))
            try writePatch(patch, output: output)
            return text("Wrote \(field) to \(output)")
        case "set_ir":
            let input = try argument(params, "input")
            let output = try argument(params, "output")
            let wav = try argument(params, "wav")
            var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
            try patch.setIR(
                wav: Data(contentsOf: URL(fileURLWithPath: wav)),
                name: try argument(params, "name")
            )
            try writePatch(patch, output: output)
            return text("Embedded IR in \(output)")
        case "clear_ir":
            let input = try argument(params, "input")
            let output = try argument(params, "output")
            var patch = try PatchRecord(contentsOf: URL(fileURLWithPath: input), profile: profile)
            patch.clearIR()
            try writePatch(patch, output: output)
            return text("Cleared embedded IR in \(output)")
        default:
            throw MCPToolError.unknownTool(params.name)
        }
    } catch {
        return text(error.localizedDescription, error: true)
    }
}

let transport = StdioTransport()
try await server.start(transport: transport)
while !Task.isCancelled {
    try await Task.sleep(for: .seconds(86_400))
}
