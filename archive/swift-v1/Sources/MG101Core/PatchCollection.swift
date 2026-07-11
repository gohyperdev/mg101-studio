import Foundation

public enum PatchCollection {
    public static let deviceSetCount = 36

    public static func records(
        from data: Data,
        profile: DeviceProfile
    ) throws -> [PatchRecord] {
        let count: Int
        switch data.count {
        case profile.recordSize:
            count = 1
        case profile.recordSize * deviceSetCount:
            count = deviceSetCount
        default:
            throw PatchCollectionError.invalidSize(
                single: profile.recordSize,
                set: profile.recordSize * deviceSetCount,
                actual: data.count
            )
        }

        return try (0..<count).map { index in
            let start = index * profile.recordSize
            return try PatchRecord(
                data: data.subdata(in: start..<(start + profile.recordSize)),
                profile: profile
            )
        }
    }

    public static func records(
        from url: URL,
        profile: DeviceProfile
    ) throws -> [PatchRecord] {
        try records(from: Data(contentsOf: url), profile: profile)
    }

    public static func deviceSet(
        from records: [PatchRecord],
        profile: DeviceProfile
    ) throws -> Data {
        guard records.count == deviceSetCount else {
            throw PatchCollectionError.recordCount(
                expected: deviceSetCount,
                actual: records.count
            )
        }
        var output = Data()
        output.reserveCapacity(profile.recordSize * deviceSetCount)
        for (index, record) in records.enumerated() {
            guard record.data.count == profile.recordSize else {
                throw PatchCollectionError.profileMismatch
            }
            var positioned = record
            positioned.setSlotIndex(UInt32(index))
            output.append(positioned.data)
        }
        return output
    }

    public static func bundledFactoryRecords(
        profile: DeviceProfile
    ) throws -> [PatchRecord] {
        guard let url = ProfileLoader.bundledResourceURL(
            forResource: "factory-patches",
            withExtension: "mg101patch"
        ) else {
            throw ProfileError.missingResource("factory-patches.mg101patch")
        }
        return try records(from: url, profile: profile)
    }
}

public enum PatchCollectionError: LocalizedError {
    case invalidSize(single: Int, set: Int, actual: Int)
    case recordCount(expected: Int, actual: Int)
    case profileMismatch

    public var errorDescription: String? {
        switch self {
        case .invalidSize(let single, let set, let actual):
            "Unsupported patch file size \(actual). Expected \(single) bytes for one patch or \(set) bytes for a 36-patch set."
        case .recordCount(let expected, let actual):
            "A device set requires exactly \(expected) patches; selected \(actual)."
        case .profileMismatch:
            "A selected patch does not match the active device profile."
        }
    }
}
