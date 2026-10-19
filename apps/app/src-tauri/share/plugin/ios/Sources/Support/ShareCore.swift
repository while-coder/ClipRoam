import Foundation
import ShareUploadCore

public enum ShareCore {
    public static func directories() throws -> (files: URL, inbox: URL) {
        guard let root = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.com.whilecode.cliproam") else {
            throw failure("无法访问共享容器，请检查 App Group 签名权限")
        }
        return (root.appendingPathComponent("files", isDirectory: true), root.appendingPathComponent("ShareInbox", isDirectory: true))
    }

    // The sole Swift -> Rust interface. Paths and native HTTP results are data.
    public static func call(_ input: [String: Any]) throws -> Any {
        var input = input
        let paths = try directories()
        input["files"] = paths.files.path
        input["inbox"] = paths.inbox.path
        let data = try JSONSerialization.data(withJSONObject: input)
        let text = String(decoding: data, as: UTF8.self)
        guard let output = text.withCString({ cliproam_share_call($0) }) else { throw failure("Rust 分享入口未返回结果") }
        defer { cliproam_share_free(output) }
        let response = try JSONSerialization.jsonObject(with: Data(String(cString: output).utf8)) as? [String: Any]
        guard response?["ok"] as? Bool == true else { throw failure(response?["error"] as? String ?? "分享处理失败") }
        return response?["value"] ?? NSNull()
    }

    public static func failure(_ message: String) -> NSError {
        NSError(domain: "ClipRoamShare", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
}
