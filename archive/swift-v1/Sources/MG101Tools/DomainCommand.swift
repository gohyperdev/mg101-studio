import Foundation
import MG101Core

public typealias PatchID = String

public enum TargetRef: Sendable, Equatable {
    case library(patchID: PatchID, expectedRevision: Int)
    case file(input: String, output: String)
}

public enum DomainCommand: Sendable, Equatable {
    // Read
    case listPatches
    case getPatch(patchID: PatchID)
    case getSelection
    case listModels(block: String)
    case getProfile
    case getDiff(patchID: PatchID)

    // Write
    case setParameter(target: TargetRef, block: String, parameter: String, value: Int)
    case setModel(target: TargetRef, block: String, model: Int, values: [Int], bypassed: Bool)
    case setBypass(target: TargetRef, block: String, bypassed: Bool)
    case setName(target: TargetRef, name: String)
    case setBPM(target: TargetRef, bpm: Int)
    case setNamedField(target: TargetRef, field: String, value: Int)
    case clearIR(target: TargetRef)
    case duplicatePatch(target: TargetRef)
    case revertLastAgentAction

    // UI
    case selectPatch(patchID: PatchID)

    // Filesystem
    case setIR(target: TargetRef, wavPath: String, name: String)
    case importPatch(path: String)
    case exportPatch(target: TargetRef, destinationPath: String)
    case listFiles(path: String)

    // Destructive
    case deletePatch(target: TargetRef)
    case revertSession
}

public enum ToolParseError: Error, LocalizedError {
    case unknownTool(String)
    case missingArgument(String)
    case invalidArgumentType(String, expected: String)

    public var errorDescription: String? {
        switch self {
        case .unknownTool(let name): "Unknown tool: \(name)"
        case .missingArgument(let name): "Missing required argument: \(name)"
        case .invalidArgumentType(let name, let expected): "Invalid type for argument \(name), expected \(expected)"
        }
    }
}

extension DomainCommand {
    public static func parse(name: String, arguments: [String: JSONValue]) throws -> DomainCommand {
        func stringArg(_ key: String) throws -> String {
            guard let val = arguments[key] else { throw ToolParseError.missingArgument(key) }
            if case .string(let str) = val { return str }
            throw ToolParseError.invalidArgumentType(key, expected: "string")
        }

        func intArg(_ key: String) throws -> Int {
            guard let val = arguments[key] else { throw ToolParseError.missingArgument(key) }
            if case .number(let num) = val { return Int(num) }
            throw ToolParseError.invalidArgumentType(key, expected: "integer")
        }

        func boolArg(_ key: String) throws -> Bool {
            guard let val = arguments[key] else { throw ToolParseError.missingArgument(key) }
            if case .bool(let b) = val { return b }
            throw ToolParseError.invalidArgumentType(key, expected: "boolean")
        }

        func intArrayArg(_ key: String) throws -> [Int] {
            guard let val = arguments[key] else { throw ToolParseError.missingArgument(key) }
            if case .array(let arr) = val {
                return try arr.map {
                    if case .number(let num) = $0 { return Int(num) }
                    throw ToolParseError.invalidArgumentType(key, expected: "array of integers")
                }
            }
            throw ToolParseError.invalidArgumentType(key, expected: "array")
        }

        func targetRef() throws -> TargetRef {
            if arguments["input"] != nil || arguments["output"] != nil {
                return .file(input: try stringArg("input"), output: try stringArg("output"))
            } else {
                return .library(patchID: try stringArg("patchID"), expectedRevision: try intArg("expectedRevision"))
            }
        }

        switch name {
        case "list_patches":
            return .listPatches
        case "get_patch":
            return .getPatch(patchID: try stringArg("patchID"))
        case "get_selection":
            return .getSelection
        case "list_models":
            return .listModels(block: try stringArg("block"))
        case "get_profile":
            return .getProfile
        case "get_diff":
            return .getDiff(patchID: try stringArg("patchID"))

        case "set_parameter":
            return .setParameter(
                target: try targetRef(),
                block: try stringArg("block"),
                parameter: try stringArg("parameter"),
                value: try intArg("value")
            )
        case "set_model":
            return .setModel(
                target: try targetRef(),
                block: try stringArg("block"),
                model: try intArg("model"),
                values: try intArrayArg("values"),
                bypassed: try boolArg("bypassed")
            )
        case "set_bypass":
            let bypassed: Bool
            if let val = arguments["bypassed"] {
                if case .bool(let b) = val { bypassed = b }
                else { throw ToolParseError.invalidArgumentType("bypassed", expected: "boolean") }
            } else if let val = arguments["bool_value"] {
                if case .bool(let b) = val { bypassed = b }
                else { throw ToolParseError.invalidArgumentType("bool_value", expected: "boolean") }
            } else {
                throw ToolParseError.missingArgument("bypassed")
            }
            return .setBypass(
                target: try targetRef(),
                block: try stringArg("block"),
                bypassed: bypassed
            )
        case "set_name":
            return .setName(
                target: try targetRef(),
                name: try stringArg("name")
            )
        case "set_bpm":
            return .setBPM(
                target: try targetRef(),
                bpm: try intArg("bpm")
            )
        case "set_named_field":
            let field = try (arguments["field"] != nil ? stringArg("field") : stringArg("parameter"))
            return .setNamedField(
                target: try targetRef(),
                field: field,
                value: try intArg("value")
            )
        case "clear_ir":
            return .clearIR(target: try targetRef())
        case "duplicate_patch":
            return .duplicatePatch(target: try targetRef())
        case "revert_last_agent_action":
            return .revertLastAgentAction

        case "select_patch":
            return .selectPatch(patchID: try stringArg("patchID"))

        case "set_ir":
            let wavPath = try (arguments["wav"] != nil ? stringArg("wav") : stringArg("wavPath"))
            return .setIR(
                target: try targetRef(),
                wavPath: wavPath,
                name: try stringArg("name")
            )
        case "import_patch":
            return .importPatch(path: try stringArg("path"))
        case "export_patch":
            return .exportPatch(
                target: try targetRef(),
                destinationPath: try stringArg("destinationPath")
            )
        case "list_files":
            return .listFiles(path: try stringArg("path"))

        case "delete_patch":
            return .deletePatch(target: try targetRef())
        case "revert_session":
            return .revertSession

        default:
            throw ToolParseError.unknownTool(name)
        }
    }
}
