import UIKit
import UniformTypeIdentifiers

class ShareViewController: UIViewController {
    private let status = UILabel()
    private var started = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        status.text = "正在保存到 ClipRoam…"
        status.numberOfLines = 0
        status.textAlignment = .center
        status.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(status)
        NSLayoutConstraint.activate([
            status.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            status.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            status.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: 24),
            status.trailingAnchor.constraint(lessThanOrEqualTo: view.trailingAnchor, constant: -24)
        ])
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        guard !started else { return }
        started = true
        let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? [])
            .flatMap { $0.attachments ?? [] }
        do {
            let id = UUID().uuidString.lowercased()
            let directory = try ShareInbox.directory(id)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            receive(providers, index: 0, id: id, directory: directory, texts: [], items: [])
        } catch { fail(error) }
    }

    // Load sequentially to retain ordering and avoid holding several large images in memory.
    private func receive(_ providers: [NSItemProvider], index: Int, id: String,
                         directory: URL, texts: [String], items: [SharedItem]) {
        guard index < providers.count else {
            do {
                guard !texts.isEmpty || !items.isEmpty else {
                    throw NSError(domain: "ClipRoam.Share", code: 3, userInfo: [
                        NSLocalizedDescriptionKey: "没有可接收的文字、链接、图片或文件"
                    ])
                }
                let share = PendingShare(id: id, text: texts.isEmpty ? nil : texts.joined(separator: "\n"),
                                         html: nil, items: items)
                try JSONEncoder().encode(share).write(
                    to: directory.appendingPathComponent("share.json"), options: .atomic
                )
                status.text = "已保存，打开 ClipRoam 后自动导入"
                let done = UIAlertController(title: "已保存到 ClipRoam",
                    message: "打开 ClipRoam 后将自动导入。", preferredStyle: .alert)
                done.addAction(UIAlertAction(title: "完成", style: .default) { [weak self] _ in
                    self?.extensionContext?.completeRequest(returningItems: nil)
                })
                present(done, animated: true)
            } catch { fail(error, directory: directory) }
            return
        }
        let provider = providers[index]
        // Prefer a real file/image over its alternate text representation.
        if let type = provider.registeredTypeIdentifiers.first(where: {
            guard let type = UTType($0) else { return false }
            return type.conforms(to: .image) || type.conforms(to: .data)
                && !type.conforms(to: .text) && !type.conforms(to: .url)
        }) {
            provider.loadFileRepresentation(forTypeIdentifier: type) { url, error in
                do {
                    if let error = error { throw error }
                    guard let url = url else { throw self.missingContent() }
                    let itemDirectory = directory.appendingPathComponent(String(index), isDirectory: true)
                    try FileManager.default.createDirectory(at: itemDirectory, withIntermediateDirectories: true)
                    var target = itemDirectory.appendingPathComponent(url.lastPathComponent)
                    // The provider URL is temporary and must be copied inside this callback.
                    var mime = UTType(type)?.preferredMIMEType ?? "application/octet-stream"
                    if mime.hasPrefix("image/") && !["image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp"].contains(mime) {
                        // The shared Rust image decoder does not read HEIC/TIFF from Photos.
                        guard let data = UIImage(contentsOfFile: url.path)?.pngData() else {
                            throw self.missingContent()
                        }
                        target = target.deletingPathExtension().appendingPathExtension("png")
                        try data.write(to: target)
                        mime = "image/png"
                    } else {
                        try FileManager.default.copyItem(at: url, to: target)
                    }
                    let item = SharedItem(path: target.path, name: target.lastPathComponent,
                                          mimeType: mime)
                    DispatchQueue.main.async {
                        self.receive(providers, index: index + 1, id: id, directory: directory,
                                     texts: texts, items: items + [item])
                    }
                } catch { DispatchQueue.main.async { self.fail(error, directory: directory) } }
            }
        } else {
            let type = provider.hasItemConformingToTypeIdentifier(UTType.url.identifier)
                ? UTType.url.identifier : UTType.plainText.identifier
            provider.loadItem(forTypeIdentifier: type, options: nil) { value, error in
                // File-provider URLs may expire when this callback returns.
                if error == nil, let url = value as? URL, url.isFileURL {
                    do {
                        let access = url.startAccessingSecurityScopedResource()
                        defer { if access { url.stopAccessingSecurityScopedResource() } }
                        let itemDirectory = directory.appendingPathComponent(String(index), isDirectory: true)
                        try FileManager.default.createDirectory(at: itemDirectory, withIntermediateDirectories: true)
                        let target = itemDirectory.appendingPathComponent(url.lastPathComponent)
                        try FileManager.default.copyItem(at: url, to: target)
                        let mime = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType
                            ?? "application/octet-stream"
                        DispatchQueue.main.async {
                            self.receive(providers, index: index + 1, id: id, directory: directory,
                                texts: texts, items: items + [SharedItem(path: target.path,
                                    name: target.lastPathComponent, mimeType: mime)])
                        }
                    } catch { DispatchQueue.main.async { self.fail(error, directory: directory) } }
                    return
                }
                DispatchQueue.main.async {
                    do {
                        if let error = error { throw error }
                        let text: String
                        if let url = value as? URL {
                            text = url.absoluteString
                        } else if let string = value as? String { text = string }
                        else if let data = value as? Data, let string = String(data: data, encoding: .utf8) { text = string }
                        else { throw self.missingContent() }
                        self.receive(providers, index: index + 1, id: id, directory: directory,
                                     texts: texts + [text], items: items)
                    } catch { self.fail(error, directory: directory) }
                }
            }
        }
    }

    private func missingContent() -> Error {
        NSError(domain: "ClipRoam.Share", code: 4,
                userInfo: [NSLocalizedDescriptionKey: "无法读取分享内容，请重新分享"])
    }

    private func fail(_ error: Error, directory: URL? = nil) {
        if let directory = directory { try? FileManager.default.removeItem(at: directory) }
        let alert = UIAlertController(title: "接收分享失败", message: error.localizedDescription,
                                      preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "关闭", style: .default) { [weak self] _ in
            self?.extensionContext?.cancelRequest(withError: error)
        })
        present(alert, animated: true)
    }
}
