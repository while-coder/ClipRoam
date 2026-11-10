import Foundation
import Security
import CryptoKit

struct ShortcutSession: Codable {
    let serverAddress: String
    let uploadToken: String
    let expiresAt: String
    let sessionFingerprint: String

    private static let key: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "com.whilecode.cliproam.shortcuts",
        kSecAttrAccount as String: "active-session",
    ]

    static func save(_ session: ShortcutSession?) throws {
        guard let session = session else {
            let status = SecItemDelete(key as CFDictionary)
            guard status == errSecSuccess || status == errSecItemNotFound else {
                throw failure("无法清除快捷指令登录信息（\(status)）")
            }
            return
        }
        _ = try session.endpoint()
        guard !session.uploadToken.isEmpty else { throw failure("请先登录 ClipRoam") }
        let attributes: [String: Any] = [
            kSecValueData as String: try JSONEncoder().encode(session),
            // Automations may run while locked, after the first unlock since boot.
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        var status = SecItemUpdate(key as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound {
            status = SecItemAdd(key.merging(attributes) { _, value in value } as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw failure("无法保存快捷指令登录信息（\(status)）") }
    }

    static func load() throws -> ShortcutSession {
        let query = key.merging([
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]) { _, value in value }
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess, let data = result as? Data else {
            throw failure("请先解锁 iPhone 并打开 ClipRoam 登录，然后重试快捷指令")
        }
        return try JSONDecoder().decode(ShortcutSession.self, from: data)
    }

    func endpoint(_ path: String = "/shortcuts/text") throws -> URL {
        try Self.endpoint(serverAddress, path)
    }

    private static func endpoint(_ address: String, _ path: String) throws -> URL {
        guard let base = URLComponents(string: address),
              ["http", "https"].contains(base.scheme?.lowercased() ?? ""),
              base.host != nil, base.user == nil, base.password == nil,
              base.query == nil, base.fragment == nil,
              let url = URL(string: address.trimmingCharacters(in: CharacterSet(charactersIn: "/")) + path)
        else { throw Self.failure("ClipRoam 服务器地址无效，请在 App 中重新登录") }
        return url
    }

    @available(iOS 15.0, *)
    static func configure(serverAddress: String?, sessionToken: String?) async throws {
        guard let address = serverAddress, let loginToken = sessionToken else {
            guard serverAddress == nil && sessionToken == nil else { throw failure("快捷指令配置不完整") }
            try await clear()
            return
        }
        let fingerprint = SHA256.hash(data: Data(loginToken.utf8)).map { String(format: "%02x", $0) }.joined()
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let cached = try? load(), cached.serverAddress == address, cached.sessionFingerprint == fingerprint,
           let expiry = formatter.date(from: cached.expiresAt), expiry > Date() { return }
        try await clear()
        let data = try await request(url: endpoint(address, "/shortcuts/token"), token: loginToken, method: "POST")
        struct Credential: Decodable { let token: String; let expiresAt: String }
        let credential = try JSONDecoder().decode(Credential.self, from: data)
        try save(ShortcutSession(serverAddress: address, uploadToken: credential.token,
                                 expiresAt: credential.expiresAt, sessionFingerprint: fingerprint))
    }

    @available(iOS 15.0, *)
    private static func clear() async throws {
        let cached = try? load()
        // Always clear locally, even when the server cannot be reached at logout.
        try save(nil)
        if let cached = cached, let url = try? cached.endpoint("/shortcuts/token") {
            _ = try? await request(url: url, token: cached.uploadToken, method: "DELETE", timeout: 5)
        }
    }

    @available(iOS 15.0, *)
    static func request(url: URL, token: String, method: String, body: Data? = nil,
                        bodyFile: URL? = nil, contentType: String = "application/json", timeout: TimeInterval = 30) async throws -> Data {
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if body != nil || bodyFile != nil {
            request.setValue(contentType, forHTTPHeaderField: "Content-Type")
        }
        if let body = body {
            request.httpBody = body
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = timeout
        configuration.timeoutIntervalForResource = timeout
        let session = URLSession(configuration: configuration, delegate: ShortcutNoRedirect(), delegateQueue: nil)
        defer { session.invalidateAndCancel() }
        let data: Data
        let response: URLResponse
        if let bodyFile = bodyFile {
            (data, response) = try await session.upload(for: request, fromFile: bodyFile)
        } else {
            (data, response) = try await session.data(for: request)
        }
        guard let response = response as? HTTPURLResponse else { throw failure("服务器响应无效") }
        if response.statusCode == 401 || response.statusCode == 403 {
            throw failure("同步凭证已失效，请在 ClipRoam 设置中重新配置快捷指令")
        }
        guard (200..<300).contains(response.statusCode) else {
            if let error = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let message = error["message"] as? String {
                throw failure(message)
            }
            throw failure("同步失败（HTTP \(response.statusCode)），请检查服务器和网络后重试")
        }
        return data
    }

    static func checkUploadResult(_ data: Data) throws {
        guard let result = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let entry = result["entry"] as? [String: Any],
              let id = entry["id"] as? String, !id.isEmpty else {
            throw failure("服务器返回了不兼容的同步结果")
        }
    }

    static func failure(_ message: String) -> NSError {
        NSError(domain: "ClipRoam.Shortcuts", code: 1, userInfo: [NSLocalizedDescriptionKey: AppBrand.message(message)])
    }
}

// Never forward text or credentials to a redirected endpoint.
private final class ShortcutNoRedirect: NSObject, URLSessionTaskDelegate {
    func urlSession(_ session: URLSession, task: URLSessionTask,
                    willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        completionHandler(nil)
    }
}
