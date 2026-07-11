import Foundation
import Testing
@testable import MG101Core

struct PatchRecordTests {
    private let profile: DeviceProfile
    private let catalog: EffectCatalog

    init() throws {
        (profile, catalog) = try ProfileLoader.bundled()
    }

    private func fixture() throws -> PatchRecord {
        var data = Data(repeating: 0, count: profile.recordSize)
        data[0] = 17
        data[profile.patchName.offset] = 84
        data[profile.patchName.offset + 1] = 69
        data[profile.patchName.offset + 2] = 83
        data[profile.patchName.offset + 3] = 84
        data[profile.bpm.lsbOffset] = 120
        for block in profile.blocks {
            data[block.selectorOffset] = 1
        }
        return try PatchRecord(data: data, profile: profile)
    }

    @Test func bundledProfileLoadsCompleteCatalog() throws {
        #expect(profile.recordSize == 8402)
        #expect(profile.blocks.count == 11)
        #expect(catalog.models(for: "amp").count == 25)
        #expect(catalog.models(for: "cab").contains { $0.modelID == 27 })
    }

    @Test func recordDecodesNameSlotAndBPM() throws {
        let patch = try fixture()
        #expect(patch.slotIndex == 17)
        #expect(patch.name == "TEST")
        #expect(patch.bpm == 120)
    }

    @Test func modelAwareParameterChecksRange() throws {
        var patch = try fixture()
        let amp = try profile.block("amp")
        let jazz = try #require(catalog.model(block: "amp", id: 1))
        try patch.setModel(
            jazz,
            block: amp,
            values: [35, 0, 83, 65, 65, 60],
            bypassed: false
        )
        #expect(patch.modelID(for: amp) == 1)
        #expect(patch.value(at: 0x21) == 0)
        #expect(throws: PatchError.self) {
            var invalid = patch
            try invalid.setParameter(jazz.parameters[1], value: 2)
        }
    }

    @Test func diffReportsOnlyChangedBytes() throws {
        let original = try fixture()
        var changed = original
        try changed.setBPM(175)
        let offsets = changed.differences(from: original).map(\.offset)
        #expect(Set(offsets).isSubset(of: [profile.bpm.msbOffset, profile.bpm.lsbOffset]))
    }

    @Test func unknownBytesSurviveNamedEdit() throws {
        var patch = try fixture()
        try patch.setByte(0xA5, at: 0x13)
        try patch.setBPM(140)
        #expect(patch.data[0x13] == 0xA5)
    }

    @Test func embeddedIRCanBeClearedAndRestored() throws {
        var patch = try fixture()
        var wav = Data(repeating: 0, count: profile.recordSize - 0xA6)
        wav.replaceSubrange(0..<4, with: Data("RIFF".utf8))
        let size = UInt32(wav.count - 8).littleEndian
        withUnsafeBytes(of: size) { wav.replaceSubrange(4..<8, with: $0) }
        wav.replaceSubrange(8..<12, with: Data("WAVE".utf8))
        try patch.setIR(wav: wav, name: "TEST IR")
        #expect(patch.irPresent)
        #expect(patch.irName == "TEST IR")
        patch.clearIR()
        #expect(!patch.irPresent)
        #expect(Set(patch.data[0x82...]) == [0])
    }

    @Test func externalProfilePairLoadsAndValidates() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let encodedProfile = try JSONEncoder().encode(profile)
        let encodedCatalog = try JSONEncoder().encode(catalog)
        try encodedProfile.write(
            to: directory.appendingPathComponent(ProfileLoader.profileFileName)
        )
        try encodedCatalog.write(
            to: directory.appendingPathComponent(ProfileLoader.catalogFileName)
        )
        let loaded = try ProfileLoader.load(from: directory)
        #expect(loaded.0.id == "nux.mg101")
        #expect(loaded.1.models(for: "cab").contains { $0.modelID == 27 })
    }

    @Test func invalidExternalProfileIsRejected() throws {
        let profileObject = try #require(
            JSONSerialization.jsonObject(
                with: JSONEncoder().encode(profile)
            ) as? [String: Any]
        )
        var invalid = profileObject
        invalid["recordSize"] = 10
        let invalidProfile = try JSONDecoder().decode(
            DeviceProfile.self,
            from: JSONSerialization.data(withJSONObject: invalid)
        )
        #expect(throws: ProfileError.self) {
            try ProfileLoader.validate(profile: invalidProfile, catalog: catalog)
        }
    }

    @Test func safeWriterCreatesButNeverOverwrites() throws {
        let destination = FileManager.default.temporaryDirectory
            .appendingPathComponent("\(UUID().uuidString).mg101patch")
        defer { try? FileManager.default.removeItem(at: destination) }
        let first = Data([1, 2, 3])
        try PatchFileWriter.writeNew(first, to: destination)
        #expect(try Data(contentsOf: destination) == first)
        #expect(throws: (any Error).self) {
            try PatchFileWriter.writeNew(Data([9]), to: destination)
        }
        #expect(try Data(contentsOf: destination) == first)
    }

    @Test func bundledFactoryLibraryContainsCompleteDeviceSet() throws {
        let records = try PatchCollection.bundledFactoryRecords(profile: profile)
        #expect(records.count == 36)
        #expect(records.first?.name == "EuroLead")
        #expect(records[17].name == "Mayer Clean")
        #expect(records.enumerated().allSatisfy { index, patch in
            patch.slotIndex == UInt32(index)
        })
    }

    @Test func deviceSetExportReassignsSlotsAndRoundTrips() throws {
        let records = try PatchCollection.bundledFactoryRecords(profile: profile)
            .reversed()
        let data = try PatchCollection.deviceSet(
            from: Array(records),
            profile: profile
        )
        #expect(data.count == profile.recordSize * 36)
        let decoded = try PatchCollection.records(from: data, profile: profile)
        #expect(decoded.count == 36)
        #expect(decoded.first?.name == "Uber")
        #expect(decoded.last?.name == "EuroLead")
        #expect(decoded.enumerated().allSatisfy { index, patch in
            patch.slotIndex == UInt32(index)
        })
    }

    @Test func deviceSetRejectsWrongSelectionCount() throws {
        let records = try PatchCollection.bundledFactoryRecords(profile: profile)
        #expect(throws: PatchCollectionError.self) {
            _ = try PatchCollection.deviceSet(
                from: Array(records.prefix(35)),
                profile: profile
            )
        }
    }
}
