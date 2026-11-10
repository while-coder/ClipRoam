import Foundation
import UIKit

#if CLIPROAM_SHARE_EXTENSION
import UniformTypeIdentifiers
#else
import Tauri
import WidgetKit
#endif

struct SharedItem: Codable {
    let path: String
    let name: String
}

struct PendingShare: Codable {
    let id: String
    let text: String?
    let html: String?
    let items: [SharedItem]
    // Preserve the destination of shares staged by the previous uploader.
    let ownerScope: String?
}

// Compiled into both the main-app plugin and the Share Extension.
enum ShareInbox {
    private static let appGroup = "group.com.whilecode.cliproam"

    private static let appGroups: [String] = {
        // SideStore appends the signing team's ID to App Groups. Read each
        // target's profile so the app and extension use the authorized name.
        guard let profile = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision"),
              let data = try? Data(contentsOf: profile),
              let start = data.range(of: Data("<plist".utf8)),
              let end = data.range(of: Data("</plist>".utf8), in: start.lowerBound..<data.endIndex),
              let plist = try? PropertyListSerialization.propertyList(
                from: data.subdata(in: start.lowerBound..<end.upperBound), options: [], format: nil
              ) as? [String: Any],
              let entitlements = plist["Entitlements"] as? [String: Any],
              let groups = entitlements["com.apple.security.application-groups"] as? [String]
        else { return [appGroup] }
        let matching = groups.filter { $0 == appGroup || $0.hasPrefix(appGroup + ".") }.sorted()
        return matching.isEmpty ? [appGroup] : matching
    }()

    static func failure(_ message: String) -> NSError {
        NSError(domain: "ClipRoam.Share", code: 1, userInfo: [NSLocalizedDescriptionKey: AppBrand.message(message)])
    }

    static func root() throws -> URL {
        guard let container = appGroups.lazy.compactMap({
            FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: $0)
        }).first else { throw failure("无法访问分享收件箱，请检查 App Group 签名配置") }
        let root = container.appendingPathComponent("ShareInbox", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    static func directory(_ id: String) throws -> URL {
        guard UUID(uuidString: id) != nil else { throw failure("分享标识不合法") }
        return try root().appendingPathComponent(id, isDirectory: true)
    }

    static func pending(scope: String) throws -> [PendingShare] {
        let directories = try FileManager.default.contentsOfDirectory(
            at: root(), includingPropertiesForKeys: nil
        ).sorted { $0.lastPathComponent < $1.lastPathComponent }
        return try directories.compactMap { directory in
            let current = directory.appendingPathComponent("manifest.json")
            let legacy = directory.appendingPathComponent("share.json")
            let manifest = FileManager.default.fileExists(atPath: current.path) ? current : legacy
            // Ignore incomplete receipts; the manifest is committed atomically.
            guard FileManager.default.fileExists(atPath: manifest.path) else { return nil }
            let share = try JSONDecoder().decode(PendingShare.self, from: Data(contentsOf: manifest))
            guard share.ownerScope == nil || share.ownerScope == scope else { return nil }
            return share
        }
    }

    static func acknowledge(_ id: String) throws {
        let directory = try directory(id)
        if FileManager.default.fileExists(atPath: directory.path) {
            try FileManager.default.removeItem(at: directory)
        }
    }
}

#if CLIPROAM_SHARE_EXTENSION
class ShareViewController: UIViewController {
    private var started = false
    private let id = UUID().uuidString.lowercased()
    private var texts: [String] = []
    private var files: [SharedItem] = []

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        let status = UILabel()
        status.text = AppBrand.message("正在保存到 ClipRoam…")
        status.textAlignment = .center
        status.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(status)
        NSLayoutConstraint.activate([
            status.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            status.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        ])
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        guard !started else { return }
        started = true
        let items = extensionContext?.inputItems as? [NSExtensionItem] ?? []
        let providers = items.flatMap { $0.attachments ?? [] }
        // Some hosts put the shared text in the extension item itself.
        for item in items where item.attachments?.isEmpty != false {
            if let text = item.attributedContentText?.string, !text.isEmpty { texts.append(text) }
        }
        receive(providers, index: 0)
    }

    private func receive(_ providers: [NSItemProvider], index: Int) {
        guard index < providers.count else { commit(); return }
        let provider = providers[index]
        let identifier = provider.registeredTypeIdentifiers.first {
            guard let type = UTType($0) else { return false }
            return type.conforms(to: .data) && !type.conforms(to: .text) && !type.conforms(to: .url)
        }
        if let identifier = identifier {
            provider.loadFileRepresentation(forTypeIdentifier: identifier) { url, error in
                do {
                    if let error = error { throw error }
                    guard let url = url else { throw ShareInbox.failure("无法读取分享文件") }
                    try self.copy(url, index: index)
                    self.receive(providers, index: index + 1)
                } catch { self.fail(error) }
            }
        } else {
            // Prefer URLs and plain text, then try the host's concrete text
            // types. A provider need not vend public.utf8-plain-text.
            var types = [UTType.url.identifier, UTType.plainText.identifier].filter {
                provider.hasItemConformingToTypeIdentifier($0)
            }
            for identifier in provider.registeredTypeIdentifiers {
                if let type = UTType(identifier), type.conforms(to: .text), !types.contains(identifier) {
                    types.append(identifier)
                }
            }
            receiveText(provider, types: types, position: 0, lastError: nil) { result in
                switch result {
                case .success(let value):
                    do {
                        if let url = value as? URL, url.isFileURL { try self.copy(url, index: index) }
                        else if let url = value as? URL { self.texts.append(url.absoluteString) }
                        else if let text = value as? String { self.texts.append(text) }
                        self.receive(providers, index: index + 1)
                    } catch { self.fail(error) }
                case .failure(let error): self.fail(error)
                }
            }
        }
    }

    private func receiveText(_ provider: NSItemProvider, types: [String], position: Int,
                             lastError: Error?, completion: @escaping (Result<Any, Error>) -> Void) {
        guard position < types.count else {
            completion(.failure(lastError ?? ShareInbox.failure(
                "无法读取分享文字或链接（类型：\(provider.registeredTypeIdentifiers.joined(separator: ", "))）"
            )))
            return
        }
        let identifier = types[position]
        provider.loadItem(forTypeIdentifier: identifier, options: nil) { value, error in
            if error == nil {
                if let url = value as? URL { completion(.success(url)); return }
                if let text = value as? String, !text.isEmpty { completion(.success(text)); return }
                if let text = value as? NSAttributedString, !text.string.isEmpty {
                    completion(.success(text.string)); return
                }
                if let data = value as? Data {
                    let type = UTType(identifier)
                    // Decode rich text with its document format, not as markup
                    // or RTF source. Plain text can be UTF-8 or UTF-16.
                    var text: String?
                    if type?.conforms(to: .rtf) == true || type?.conforms(to: .html) == true {
                        let format: NSAttributedString.DocumentType = type?.conforms(to: .rtf) == true ? .rtf : .html
                        // The HTML importer must run on the main thread.
                        DispatchQueue.main.async {
                            let decoded = (try? NSAttributedString(data: data, options: [.documentType: format],
                                                                  documentAttributes: nil))?.string
                            if let decoded = decoded, !decoded.isEmpty { completion(.success(decoded)) }
                            else {
                                self.receiveText(provider, types: types, position: position + 1,
                                                 lastError: lastError, completion: completion)
                            }
                        }
                        return
                    } else {
                        text = String(data: data, encoding: .utf8) ?? String(data: data, encoding: .utf16)
                    }
                    if let text = text, !text.isEmpty { completion(.success(text)); return }
                }
            }
            self.receiveText(provider, types: types, position: position + 1,
                             lastError: error ?? lastError, completion: completion)
        }
    }

    private func copy(_ source: URL, index: Int) throws {
        let access = source.startAccessingSecurityScopedResource()
        defer { if access { source.stopAccessingSecurityScopedResource() } }
        let directory = try ShareInbox.directory(id).appendingPathComponent(String(index), isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let target = directory.appendingPathComponent(source.lastPathComponent)
        try FileManager.default.copyItem(at: source, to: target)
        files.append(SharedItem(path: target.path, name: target.lastPathComponent))
    }

    private func commit() {
        DispatchQueue.global(qos: .utility).async {
            do {
                guard !self.texts.isEmpty || !self.files.isEmpty else { throw ShareInbox.failure("没有可接收的分享内容") }
                let directory = try ShareInbox.directory(self.id)
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                let share = PendingShare(id: self.id, text: self.texts.joined(separator: "\n"),
                                         html: nil, items: self.files, ownerScope: nil)
                try JSONEncoder().encode(share).write(
                    to: directory.appendingPathComponent("manifest.json"), options: .atomic
                )
                DispatchQueue.main.async { self.openMainApp() }
            } catch { self.fail(error) }
        }
    }

    private func openMainApp() {
        let url = URL(string: "cliproam://share")!
        // The extension context cannot open the containing app for this extension
        // point. Use the application's modern URL opener through the responder chain.
        var responder: UIResponder? = self
        while let current = responder {
            if let application = current as? UIApplication {
                application.open(url, options: [:]) { opened in
                    self.finish(opened ? nil : AppBrand.message("内容已保存，请重新从分享菜单打开 ClipRoam 以导入。"))
                }
                return
            }
            responder = current.next
        }
        finish(AppBrand.message("内容已保存，请重新从分享菜单打开 ClipRoam 以导入。"))
    }

    private func fail(_ error: Error) {
        // No manifest was committed, so partial attachments are not importable.
        if let directory = try? ShareInbox.directory(id) { try? FileManager.default.removeItem(at: directory) }
        finish("接收分享失败：\(error.localizedDescription)，请重新分享。")
    }
    private func finish(_ message: String?) {
        DispatchQueue.main.async {
            guard let message = message else { self.extensionContext?.completeRequest(returningItems: nil); return }
            let alert = UIAlertController(title: AppBrand.message("分享到 ClipRoam"), message: message, preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "关闭", style: .default) { _ in self.extensionContext?.completeRequest(returningItems: nil) })
            self.present(alert, animated: true)
        }
    }
}
#else
private struct PendingArgs: Decodable {
    let scope: String
}

private struct AcknowledgeArgs: Decodable {
    let id: String
}

private struct ShortcutArgs: Decodable {
    let serverAddress: String?
    let sessionToken: String?
}

private struct ShortcutLinkArgs: Decodable {
    let url: String
}

private final class SharePlugin: Plugin {
    @objc func configureWidget(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(ShortcutArgs.self)
        try StickyWidgetStore.configure(serverAddress: args.serverAddress, sessionToken: args.sessionToken)
        WidgetCenter.shared.reloadTimelines(ofKind: StickyWidgetStore.kind)
        invoke.resolve()
    }

    @objc func addSmsShortcut(_ invoke: Invoke) throws {
        guard #available(iOS 27.0, *) else {
            invoke.reject("快速添加短信自动同步需要 iOS 27 或更新版本")
            return
        }
        let args = try invoke.parseArgs(ShortcutLinkArgs.self)
        guard let parts = URLComponents(string: args.url), parts.scheme == "https",
              parts.host?.lowercased() == "www.icloud.com", parts.port == nil,
              parts.user == nil, parts.password == nil, parts.query == nil, parts.fragment == nil,
              parts.path.range(of: "^/shortcuts/[a-fA-F0-9]{32}/?$", options: .regularExpression) != nil,
              let url = parts.url else { throw ShortcutSession.failure("快捷指令分享链接无效") }
        DispatchQueue.main.async {
            UIApplication.shared.open(url, options: [:]) { opened in
                if opened { invoke.resolve() }
                else { invoke.reject("无法打开快捷指令，请确认已安装系统“快捷指令”App") }
            }
        }
    }

    @objc func configureShortcuts(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(ShortcutArgs.self)
        if #available(iOS 16.0, *) {
            Task {
                do {
                    try await ShortcutSession.configure(serverAddress: args.serverAddress, sessionToken: args.sessionToken)
                    invoke.resolve()
                } catch { invoke.reject(error.localizedDescription) }
            }
        } else { invoke.reject("快捷指令需要 iOS 16 或更新版本") }
    }

    @objc func deviceMetadata(_ invoke: Invoke) {
        var result: JSObject = ["deviceBrand": "Apple"]
        if let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String,
           !build.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            result["appBuildNumber"] = build
        }
        invoke.resolve(result)
    }

    @objc func pending(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(PendingArgs.self)
        try invoke.resolve(ShareInbox.pending(scope: args.scope))
    }

    @objc func acknowledge(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(AcknowledgeArgs.self)
        try ShareInbox.acknowledge(args.id)
        invoke.resolve()
    }
}

@_cdecl("init_plugin_cliproam_share_receiver")
func initPlugin() -> Plugin { SharePlugin() }
#endif
