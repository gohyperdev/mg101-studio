import Foundation
import LocalAuthentication
import Security

enum KeychainStore {
    private static let service = "dev.mos.mg101studio.ai"

    static func read(
        account: String,
        allowInteraction: Bool = false
    ) throws -> String {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        let context = LAContext()
        context.interactionNotAllowed = !allowInteraction
        query[kSecUseAuthenticationContext as String] = context
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return "" }
        if status == errSecInteractionNotAllowed, !allowInteraction { return "" }
        guard status == errSecSuccess, let data = result as? Data else {
            throw KeychainError.status(status)
        }
        return String(decoding: data, as: UTF8.self)
    }

    static func write(_ value: String, account: String) throws {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        if value.isEmpty {
            let status = SecItemDelete(query as CFDictionary)
            guard status == errSecSuccess || status == errSecItemNotFound else {
                throw KeychainError.status(status)
            }
            return
        }

        let attributes: [String: Any] = [
            kSecValueData as String: Data(value.utf8),
            kSecAttrLabel as String: "MG101 Studio \(account) API key",
        ]
        let updateStatus = SecItemUpdate(
            query as CFDictionary,
            attributes as CFDictionary
        )
        if updateStatus == errSecItemNotFound {
            var addition = query
            addition.merge(attributes) { _, new in new }
            let status = SecItemAdd(addition as CFDictionary, nil)
            guard status == errSecSuccess else {
                throw KeychainError.status(status)
            }
        } else if updateStatus != errSecSuccess {
            throw KeychainError.status(updateStatus)
        }
    }
}

enum KeychainError: LocalizedError {
    case status(OSStatus)

    var errorDescription: String? {
        switch self {
        case .status(let status):
            let message = SecCopyErrorMessageString(status, nil) as String?
            return "Keychain error \(status): \(message ?? "unknown error")"
        }
    }
}
