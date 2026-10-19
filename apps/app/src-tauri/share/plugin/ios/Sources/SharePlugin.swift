import Foundation
import UIKit

#if CLIPROAM_SHARE_EXTENSION
import UniformTypeIdentifiers
#else
import Tauri
import WebKit
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
    static func failure(_ message: String) -> NSError {
        NSError(domain: "ClipRoam.Share", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }

    static func root() throws -> URL {
        guard let container = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: "group.com.whilecode.cliproam"
        ) else { throw failure("无法访问分享收件箱，请检查 App Group 签名配置") }
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
        status.text = "正在保存到 ClipRoam…"
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
        let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? []).flatMap { $0.attachments ?? [] }
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
            let type = provider.hasItemConformingToTypeIdentifier(UTType.url.identifier) ? UTType.url.identifier : UTType.plainText.identifier
            provider.loadItem(forTypeIdentifier: type, options: nil) { value, error in
                do {
                    if let error = error { throw error }
                    if let url = value as? URL {
                        if url.isFileURL { try self.copy(url, index: index) }
                        else { self.texts.append(url.absoluteString) }
                    } else if let text = value as? String { self.texts.append(text) }
                    else if let data = value as? Data, let text = String(data: data, encoding: .utf8) { self.texts.append(text) }
                    else { throw ShareInbox.failure("无法读取分享文字或链接") }
                    self.receive(providers, index: index + 1)
                } catch { self.fail(error) }
            }
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
                    self.finish(opened ? nil : "内容已保存，请打开 ClipRoam 后自动导入。")
                }
                return
            }
            responder = current.next
        }
        finish("内容已保存，请打开 ClipRoam 后自动导入。")
    }

    private func fail(_ error: Error) {
        // No manifest was committed, so partial attachments are not importable.
        if let directory = try? ShareInbox.directory(id) { try? FileManager.default.removeItem(at: directory) }
        finish("接收分享失败：\(error.localizedDescription)，请重新分享。")
    }
    private func finish(_ message: String?) {
        DispatchQueue.main.async {
            guard let message = message else { self.extensionContext?.completeRequest(returningItems: nil); return }
            let alert = UIAlertController(title: "分享到 ClipRoam", message: message, preferredStyle: .alert)
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

private final class SharePlugin: Plugin {
    private var observer: NSObjectProtocol?

    override func load(webview: WKWebView) {
        observer = NotificationCenter.default.addObserver(
            forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main
        ) { [weak self] _ in
            self?.trigger("share-received", data: [:] as JSObject)
        }
    }

    deinit {
        if let observer = observer { NotificationCenter.default.removeObserver(observer) }
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
