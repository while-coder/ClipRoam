import UIKit
import UniformTypeIdentifiers

class ShareViewController: UIViewController {
    private var started = false
    private let id = UUID().uuidString.lowercased()
    private var texts: [String] = []
    private var files: [[String: Any]] = []

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
                    guard let url = url else { throw ShareCore.failure("无法读取分享文件") }
                    try self.copy(url, index: index)
                    self.receive(providers, index: index + 1)
                } catch { self.finish(error.localizedDescription) }
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
                    else { throw ShareCore.failure("无法读取分享文字或链接") }
                    self.receive(providers, index: index + 1)
                } catch { self.finish(error.localizedDescription) }
            }
        }
    }

    private func copy(_ source: URL, index: Int) throws {
        let access = source.startAccessingSecurityScopedResource()
        defer { if access { source.stopAccessingSecurityScopedResource() } }
        let directory = try ShareCore.directories().inbox.appendingPathComponent(id).appendingPathComponent(String(index), isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let target = directory.appendingPathComponent(source.lastPathComponent)
        try FileManager.default.copyItem(at: source, to: target)
        files.append(["path": target.path, "name": target.lastPathComponent, "mimeType": "application/octet-stream"])
    }

    private func commit() {
        DispatchQueue.global(qos: .utility).async {
            do {
                guard !self.texts.isEmpty || !self.files.isEmpty else { throw ShareCore.failure("没有可接收的分享内容") }
                let result = try ShareCore.call(["command": "stage", "share": ["id": self.id, "text": self.texts.joined(separator: "\n"), "html": NSNull(), "items": self.files]])
                let uploading = (result as? [String: Any])?["uploading"] as? Bool == true
                ShareHTTP.shared.startJobs(result) { error in
                    self.finish(error?.localizedDescription ?? (uploading ? nil : "内容已保存，请打开 ClipRoam 登录后上传。"))
                }
            } catch { self.finish("内容已保留。\(error.localizedDescription)") }
        }
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
