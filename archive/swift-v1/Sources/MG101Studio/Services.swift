import Foundation
import MG101Core
import MG101Tools

public struct PatchImportService {
    public static func importPatch(from url: URL, profile: DeviceProfile) throws -> [PatchRecord] {
        return try PatchCollection.records(from: url, profile: profile)
    }
}

public struct PatchExportService {
    public static func exportPatch(_ patch: PatchRecord, to url: URL) throws {
        try PatchFileWriter.writeNew(patch.data, to: url)
    }

    public static func exportSet(_ patches: [PatchRecord], profile: DeviceProfile, to url: URL) throws {
        let data = try PatchCollection.deviceSet(from: patches, profile: profile)
        try PatchFileWriter.writeNew(data, to: url)
    }
}

public struct IRService {
    public static func embedIR(wavData: Data, name: String, into patch: inout PatchRecord) throws {
        try patch.setIR(wav: wavData, name: name)
    }

    public static func clearIR(in patch: inout PatchRecord) {
        patch.clearIR()
    }
}
