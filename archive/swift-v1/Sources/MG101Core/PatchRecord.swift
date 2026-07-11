import Foundation

public struct PatchRecord: Sendable {
    public private(set) var data: Data
    public let profile: DeviceProfile

    public init(data: Data, profile: DeviceProfile) throws {
        guard data.count == profile.recordSize else {
            throw PatchError.invalidSize(expected: profile.recordSize, actual: data.count)
        }
        self.data = data
        self.profile = profile
    }

    public init(contentsOf url: URL, profile: DeviceProfile) throws {
        try self.init(data: Data(contentsOf: url), profile: profile)
    }

    public var slotIndex: UInt32 {
        data.withUnsafeBytes { raw in
            raw.loadUnaligned(fromByteOffset: 0, as: UInt32.self).littleEndian
        }
    }

    public mutating func setSlotIndex(_ value: UInt32) {
        var littleEndian = value.littleEndian
        withUnsafeBytes(of: &littleEndian) {
            data.replaceSubrange(0..<4, with: $0)
        }
    }

    public var name: String {
        let range = profile.patchName.offset..<(profile.patchName.offset + profile.patchName.length)
        let bytes = data[range].prefix { $0 != 0 }
        return String(decoding: bytes, as: UTF8.self)
    }

    public var bpm: Int {
        Int(data[profile.bpm.msbOffset]) << 7 | Int(data[profile.bpm.lsbOffset])
    }

    public var irPresent: Bool {
        data[0x82] != 0 || data[0x83] != 0 || data[0x84] != 0 || data[0x85] != 0
    }

    public var irName: String {
        let bytes = data[0x86..<0xA6].prefix { $0 != 0 }
        return String(decoding: bytes, as: UTF8.self)
    }

    public func selector(for block: DeviceProfile.Block) -> UInt8 {
        data[block.selectorOffset]
    }

    public func modelID(for block: DeviceProfile.Block) -> Int {
        Int(selector(for: block) & 0x3F)
    }

    public func isBypassed(_ block: DeviceProfile.Block) -> Bool {
        selector(for: block) & 0x40 != 0
    }

    public func value(at offset: Int) -> Int {
        Int(data[offset])
    }

    public mutating func setByte(_ value: Int, at offset: Int) throws {
        guard data.indices.contains(offset) else { throw PatchError.offset(offset) }
        guard 0...255 ~= value else { throw PatchError.value(value) }
        data[offset] = UInt8(value)
    }

    public mutating func setBypass(_ bypassed: Bool, block: DeviceProfile.Block) {
        let model = data[block.selectorOffset] & 0x3F
        data[block.selectorOffset] = model | (bypassed ? 0x40 : 0)
    }

    public mutating func setParameter(
        _ parameter: EffectCatalog.Parameter,
        value: Int
    ) throws {
        guard parameter.minimum...parameter.maximum ~= value else {
            throw PatchError.parameterRange(
                parameter.name,
                parameter.minimum,
                parameter.maximum,
                value
            )
        }
        try setByte(value, at: parameter.fileOffset)
    }

    public mutating func setModel(
        _ model: EffectCatalog.Model,
        block: DeviceProfile.Block,
        values: [Int],
        bypassed: Bool,
        clearInactive: Bool = true
    ) throws {
        guard values.count == model.parameters.count else {
            throw PatchError.parameterCount(model.parameters.count, values.count)
        }
        data[block.selectorOffset] = UInt8(model.modelID) | (bypassed ? 0x40 : 0)
        let active = Set(model.parameters.map(\.fileOffset))
        for (parameter, value) in zip(model.parameters, values) {
            try setParameter(parameter, value: value)
        }
        if clearInactive {
            for offset in block.parameterOffsets where !active.contains(offset) {
                data[offset] = 0
            }
        }
    }

    public mutating func setBPM(_ value: Int) throws {
        guard profile.bpm.minimum...profile.bpm.maximum ~= value else {
            throw PatchError.bpm(value)
        }
        data[profile.bpm.msbOffset] = UInt8((value >> 7) & 0x7F)
        data[profile.bpm.lsbOffset] = UInt8(value & 0x7F)
    }

    public mutating func setNamedField(_ name: String, value: Int) throws {
        guard let field = profile.namedFields[name] else {
            throw PatchError.unknownField(name)
        }
        guard field.minimum...field.maximum ~= value else {
            throw PatchError.parameterRange(
                name,
                field.minimum,
                field.maximum,
                value
            )
        }
        try setByte(value, at: field.offset)
    }

    public mutating func setName(_ value: String) throws {
        let encoded = Array(value.utf8)
        guard encoded.count <= profile.patchName.length else {
            throw PatchError.nameTooLong(profile.patchName.length)
        }
        let start = profile.patchName.offset
        for index in 0..<profile.patchName.length {
            data[start + index] = index < encoded.count ? encoded[index] : 0
        }
    }

    public mutating func setIR(wav: Data, name: String) throws {
        let expectedLength = profile.recordSize - 0xA6
        guard wav.count == expectedLength else {
            throw PatchError.ir("Expected \(expectedLength) WAV bytes, received \(wav.count).")
        }
        guard wav.prefix(4) == Data("RIFF".utf8),
              wav[8..<12] == Data("WAVE".utf8)
        else {
            throw PatchError.ir("IR must be a RIFF/WAVE file.")
        }
        let riffSize = wav.withUnsafeBytes {
            $0.loadUnaligned(fromByteOffset: 4, as: UInt32.self).littleEndian
        }
        guard riffSize == UInt32(expectedLength - 8) else {
            throw PatchError.ir("Invalid RIFF size \(riffSize).")
        }
        let encoded = Array(name.utf8)
        guard encoded.count <= 32 else { throw PatchError.ir("IR name exceeds 32 bytes.") }

        data[0x82] = 1
        data[0x83] = 0
        data[0x84] = 0
        data[0x85] = 0
        for index in 0..<32 {
            data[0x86 + index] = index < encoded.count ? encoded[index] : 0
        }
        data.replaceSubrange(0xA6..<profile.recordSize, with: wav)
    }

    public mutating func clearIR() {
        data.replaceSubrange(0x82..<profile.recordSize, with: repeatElement(0, count: profile.recordSize - 0x82))
    }

    public func differences(from original: PatchRecord) -> [ByteDifference] {
        zip(original.data, data).enumerated().compactMap { offset, pair in
            pair.0 == pair.1 ? nil : ByteDifference(
                offset: offset,
                before: pair.0,
                after: pair.1
            )
        }
    }
}

public struct ByteDifference: Identifiable, Hashable, Sendable {
    public let offset: Int
    public let before: UInt8
    public let after: UInt8
    public var id: Int { offset }
}

public enum PatchError: LocalizedError {
    case invalidSize(expected: Int, actual: Int)
    case offset(Int)
    case value(Int)
    case bpm(Int)
    case nameTooLong(Int)
    case parameterRange(String, Int, Int, Int)
    case parameterCount(Int, Int)
    case unknownField(String)
    case ir(String)

    public var errorDescription: String? {
        switch self {
        case .invalidSize(let expected, let actual):
            "Expected \(expected) bytes, received \(actual)."
        case .offset(let value): "Offset outside record: \(value)."
        case .value(let value): "Byte value outside 0...255: \(value)."
        case .bpm(let value): "BPM outside configured range: \(value)."
        case .nameTooLong(let maximum): "Patch name exceeds \(maximum) UTF-8 bytes."
        case .parameterRange(let name, let minimum, let maximum, let value):
            "\(name)=\(value) is outside \(minimum)...\(maximum)."
        case .parameterCount(let expected, let actual):
            "Expected \(expected) parameter values, received \(actual)."
        case .unknownField(let name): "Unknown named field: \(name)."
        case .ir(let message): "Invalid local IR: \(message)"
        }
    }
}
