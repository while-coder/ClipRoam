import Foundation

struct SharedItem: Codable {
    let path: String
    let name: String
    let mimeType: String
}

struct PendingShare: Codable {
    let id: String
    let text: String?
    let html: String?
    let items: [SharedItem]
}

// Compiled into both the native plugin and the Share Extension.
enum ShareInbox {
    static let group = "group.com.whilecode.cliproam"

    static func root() throws -> URL {
        guard let container = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: group
        ) else {
            throw NSError(domain: "ClipRoam.Share", code: 1, userInfo: [
                NSLocalizedDescriptionKey: "无法访问分享收件箱，请检查 App Groups 签名配置"
            ])
        }
        let root = container.appendingPathComponent("ShareInbox", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    static func directory(_ id: String) throws -> URL {
        guard UUID(uuidString: id) != nil else {
            throw NSError(domain: "ClipRoam.Share", code: 2, userInfo: [
                NSLocalizedDescriptionKey: "分享标识不合法"
            ])
        }
        return try root().appendingPathComponent(id, isDirectory: true)
    }

    static func pending() throws -> [PendingShare] {
        let directories = try FileManager.default.contentsOfDirectory(
            at: root(), includingPropertiesForKeys: nil
        ).sorted { $0.lastPathComponent < $1.lastPathComponent }
        return try directories.compactMap { directory in
            // A share becomes visible only after its manifest is atomically committed.
            let manifest = directory.appendingPathComponent("share.json")
            guard FileManager.default.fileExists(atPath: manifest.path) else { return nil }
            return try JSONDecoder().decode(PendingShare.self, from: Data(contentsOf: manifest))
        }
    }

    static func acknowledge(_ id: String) throws {
        let directory = try directory(id)
        if FileManager.default.fileExists(atPath: directory.path) {
            try FileManager.default.removeItem(at: directory)
        }
    }
}
