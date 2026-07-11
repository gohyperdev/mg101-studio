import Foundation
import Testing
@testable import MG101Tools

struct ToolsContractTests {
    @Test func testParseLibrarySetParameter() throws {
        let args: [String: JSONValue] = [
            "patchID": .string("factory-01"),
            "expectedRevision": .number(12),
            "block": .string("amp"),
            "parameter": .string("gain"),
            "value": .number(55)
        ]
        let command = try DomainCommand.parse(name: "set_parameter", arguments: args)
        
        guard case .setParameter(let target, let block, let parameter, let value) = command else {
            Issue.record("Expected setParameter command")
            return
        }
        
        #expect(target == .library(patchID: "factory-01", expectedRevision: 12))
        #expect(block == "amp")
        #expect(parameter == "gain")
        #expect(value == 55)
    }

    @Test func testParseFileSetParameter() throws {
        let args: [String: JSONValue] = [
            "input": .string("/tmp/in.mg101patch"),
            "output": .string("/tmp/out.mg101patch"),
            "block": .string("amp"),
            "parameter": .string("gain"),
            "value": .number(55)
        ]
        let command = try DomainCommand.parse(name: "set_parameter", arguments: args)
        
        guard case .setParameter(let target, let block, let parameter, let value) = command else {
            Issue.record("Expected setParameter command")
            return
        }
        
        #expect(target == .file(input: "/tmp/in.mg101patch", output: "/tmp/out.mg101patch"))
        #expect(block == "amp")
        #expect(parameter == "gain")
        #expect(value == 55)
    }

    @Test func testParseSetBypassAlternateKey() throws {
        // Test compatibility with "bool_value" and "bypassed"
        let argsBoolVal: [String: JSONValue] = [
            "patchID": .string("factory-01"),
            "expectedRevision": .number(1),
            "block": .string("dly"),
            "bool_value": .bool(true)
        ]
        let cmd1 = try DomainCommand.parse(name: "set_bypass", arguments: argsBoolVal)
        guard case .setBypass(_, _, let bypassed1) = cmd1 else {
            Issue.record()
            return
        }
        #expect(bypassed1 == true)

        let argsBypassed: [String: JSONValue] = [
            "patchID": .string("factory-01"),
            "expectedRevision": .number(1),
            "block": .string("dly"),
            "bypassed": .bool(false)
        ]
        let cmd2 = try DomainCommand.parse(name: "set_bypass", arguments: argsBypassed)
        guard case .setBypass(_, _, let bypassed2) = cmd2 else {
            Issue.record()
            return
        }
        #expect(bypassed2 == false)
    }

    @Test func testParseListFiles() throws {
        let args: [String: JSONValue] = [
            "path": .string("/tmp/some-dir")
        ]
        let command = try DomainCommand.parse(name: "list_files", arguments: args)
        guard case .listFiles(let path) = command else {
            Issue.record("Expected listFiles command")
            return
        }
        #expect(path == "/tmp/some-dir")
    }
}
