import Foundation

struct StickyWidgetNote: Codable, Identifiable {
    let id: String
    let kind: String
    let content: String
}

struct StickyWidgetSession: Codable, Equatable {
    let serverAddress: String
    let sessionToken: String

    func endpoint() throws -> URL {
        guard let base = URLComponents(string: serverAddress),
              ["http", "https"].contains(base.scheme?.lowercased() ?? ""),
              base.host != nil, base.user == nil, base.password == nil,
              base.query == nil, base.fragment == nil,
              let url = URL(string: serverAddress.trimmingCharacters(in: CharacterSet(charactersIn: "/")) + "/entries/sticky")
        else { throw StickyWidgetStore.failure("服务器地址无效，请打开 ClipRoam 重新登录") }
        return url
    }
}

struct StickyWidgetState: Codable {
    var session: StickyWidgetSession?
    var notes: [StickyWidgetNote] = []
    var updatedAt: Date?
    var requiresLogin = false
}

// Shared by the app's native plugin and the WidgetKit extension. Keep credentials
// and snapshots in one coordinated file so logout cannot restore another account's data.
enum StickyWidgetStore {
    static let kind = "ClipRoamSticky"

    static func failure(_ message: String) -> NSError {
        NSError(domain: "ClipRoam.Widget", code: 1, userInfo: [NSLocalizedDescriptionKey: AppBrand.message(message)])
    }

    private static func fileURL() throws -> URL {
        let group = "group.com.whilecode.cliproam"
        var groups = [group]
        // Match ShareInbox's SideStore provisioning-profile group resolution.
        if let profile = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision"),
           let data = try? Data(contentsOf: profile),
           let start = data.range(of: Data("<plist".utf8)),
           let end = data.range(of: Data("</plist>".utf8), in: start.lowerBound..<data.endIndex),
           let plist = try? PropertyListSerialization.propertyList(
               from: data.subdata(in: start.lowerBound..<end.upperBound), options: [], format: nil
           ) as? [String: Any],
           let entitlements = plist["Entitlements"] as? [String: Any],
           let authorized = entitlements["com.apple.security.application-groups"] as? [String] {
            let matching = authorized.filter { $0 == group || $0.hasPrefix(group + ".") }.sorted()
            if !matching.isEmpty { groups = matching }
        }
        guard let container = groups.lazy.compactMap({
            FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: $0)
        }).first else { throw failure("无法访问便签小组件，请检查 App Group 签名配置") }
        var directory = container.appendingPathComponent("StickyWidget", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
            attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication])
        var resources = URLResourceValues()
        resources.isExcludedFromBackup = true
        try directory.setResourceValues(resources)
        return directory.appendingPathComponent("state.json")
    }

    private static func read(_ url: URL) throws -> StickyWidgetState {
        guard FileManager.default.fileExists(atPath: url.path) else { return StickyWidgetState() }
        return try JSONDecoder().decode(StickyWidgetState.self, from: Data(contentsOf: url))
    }

    static func load() throws -> StickyWidgetState {
        let url = try fileURL()
        guard FileManager.default.fileExists(atPath: url.path) else { return StickyWidgetState() }
        var error: NSError?
        var result: Result<StickyWidgetState, Error>?
        NSFileCoordinator().coordinate(readingItemAt: url, options: [], error: &error) { coordinated in
            result = Result { try read(coordinated) }
        }
        if let error = error { throw error }
        guard let result = result else { throw failure("无法读取便签缓存") }
        return try result.get()
    }

    @discardableResult
    private static func update(_ change: (inout StickyWidgetState) throws -> Void) throws -> StickyWidgetState {
        let url = try fileURL()
        var error: NSError?
        var result: Result<StickyWidgetState, Error>?
        NSFileCoordinator().coordinate(writingItemAt: url, options: .forMerging, error: &error) { coordinated in
            result = Result {
                var state = try read(coordinated)
                try change(&state)
                try JSONEncoder().encode(state).write(to: coordinated,
                    options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
                return state
            }
        }
        if let error = error { throw error }
        guard let result = result else { throw failure("无法保存便签缓存") }
        return try result.get()
    }

    static func configure(serverAddress: String?, sessionToken: String?) throws {
        let session: StickyWidgetSession?
        if let address = serverAddress, let token = sessionToken, !token.isEmpty {
            session = StickyWidgetSession(serverAddress: address, sessionToken: token)
            _ = try session?.endpoint()
        } else {
            guard serverAddress == nil && sessionToken == nil else { throw failure("小组件登录配置不完整") }
            session = nil
        }
        try update { state in
            if session == nil || state.session != session { state = StickyWidgetState(session: session) }
        }
    }

    static func save(notes: [StickyWidgetNote], for session: StickyWidgetSession) throws -> StickyWidgetState {
        try update { state in
            guard state.session == session else { return }
            state.notes = notes.map { StickyWidgetNote(id: $0.id, kind: $0.kind, content: String($0.content.prefix(1024))) }
            state.updatedAt = Date()
            state.requiresLogin = false
        }
    }

    static func expire(_ session: StickyWidgetSession) throws -> StickyWidgetState {
        try update { state in
            guard state.session == session else { return }
            state = StickyWidgetState(requiresLogin: true)
        }
    }
}
