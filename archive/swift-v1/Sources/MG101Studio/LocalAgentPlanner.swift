import Foundation
import MG101Core

enum LocalAgentPlanner {
    static func plan(
        prompt: String,
        patch: PatchRecord,
        profile: DeviceProfile,
        catalog: EffectCatalog
    ) -> [PatchOperation] {
        let normalized = prompt.lowercased()
        var result: [PatchOperation] = []

        if let bpm = firstInteger(after: "bpm", in: normalized)
            ?? firstInteger(after: "tempo", in: normalized) {
            result.append(.setBPM(bpm))
        }

        for block in profile.blocks {
            if normalized.contains("bypass \(block.id)")
                || normalized.contains("wyłącz \(block.id)") {
                result.append(.setBypass(block: block.id, value: true))
            }
            if normalized.contains("enable \(block.id)")
                || normalized.contains("włącz \(block.id)") {
                result.append(.setBypass(block: block.id, value: false))
            }
        }

        if let quoted = quotedValue(in: prompt), normalized.contains("name") || normalized.contains("nazwa") {
            result.append(.setName(quoted))
        }

        return result
    }

    private static func firstInteger(after marker: String, in text: String) -> Int? {
        guard let range = text.range(of: marker) else { return nil }
        let suffix = text[range.upperBound...]
        return suffix.split(whereSeparator: { !$0.isNumber }).first.flatMap { Int($0) }
    }

    private static func quotedValue(in text: String) -> String? {
        guard let start = text.firstIndex(of: "\""),
              let end = text[text.index(after: start)...].firstIndex(of: "\"")
        else { return nil }
        return String(text[text.index(after: start)..<end])
    }
}
