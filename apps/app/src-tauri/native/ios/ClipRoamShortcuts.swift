import AppIntents
import Foundation

@available(iOS 16.0, *)
struct SyncTextToClipRoam: AppIntent {
    static var title: LocalizedStringResource = "同步文本到 ClipRoam"
    static var description = IntentDescription("将传入的文本直接同步到当前登录的 ClipRoam 账号，可用于收到信息时的个人自动化。")
    static var openAppWhenRun: Bool = false
    static var authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed

    @Parameter(title: "文本")
    var text: String

    static var parameterSummary: some ParameterSummary {
        Summary("同步 \(\.$text) 到 ClipRoam")
    }

    func perform() async throws -> some IntentResult {
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw ShortcutSession.failure("没有收到文本，请将自动化的“快捷指令输入”连接到“文本”参数")
        }
        let account = try ShortcutSession.load()
        let payload: [String: Any] = ["text": text]
        let body = try JSONSerialization.data(withJSONObject: payload)
        guard body.count <= 16 * 1024 * 1024 else { throw ShortcutSession.failure("文本超过上传大小限制") }
        let data = try await ShortcutSession.request(url: account.endpoint(), token: account.uploadToken, method: "POST", body: body)
        try ShortcutSession.checkUploadResult(data)
        return .result()
    }
}

@available(iOS 16.0, *)
struct SyncFilesToClipRoam: AppIntent {
    static var title: LocalizedStringResource = "同步文件到 ClipRoam"
    static var description = IntentDescription("上传文件或文件夹并保留目录结构，同步到当前登录的 ClipRoam 账号。")
    static var openAppWhenRun: Bool = false
    static var authenticationPolicy: IntentAuthenticationPolicy = .alwaysAllowed

    @Parameter(title: "文件", supportedTypeIdentifiers: ["public.item"])
    var files: [IntentFile]

    static var parameterSummary: some ParameterSummary { Summary("同步 \(\.$files) 到 ClipRoam") }

    func perform() async throws -> some IntentResult {
        try await ShortcutMultipartUpload.send(files)
        return .result()
    }
}

@available(iOS 16.0, *)
private enum ShortcutMultipartUpload {
    static func send(_ files: [IntentFile]) async throws {
        guard !files.isEmpty else { throw ShortcutSession.failure("没有收到上传内容") }
        let account = try ShortcutSession.load()
        let boundary = "ClipRoam-\(UUID().uuidString)"
        let bodyURL = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        guard FileManager.default.createFile(atPath: bodyURL.path, contents: nil) else {
            throw ShortcutSession.failure("无法准备上传文件")
        }
        defer { try? FileManager.default.removeItem(at: bodyURL) }
        let output = try FileHandle(forWritingTo: bodyURL)
        defer { try? output.close() }
        func write(_ text: String) throws { try output.write(contentsOf: Data(text.utf8)) }
        func header(_ name: String) throws {
            guard !name.contains("\r"), !name.contains("\n"), !name.contains("\0") else {
                throw ShortcutSession.failure("文件名无效")
            }
            let escaped = name.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
            try write("--\(boundary)\r\nContent-Disposition: form-data; name=\"files\"; filename=\"\(escaped)\"\r\nContent-Type: application/octet-stream\r\n\r\n")
        }
        func append(_ url: URL, name: String) throws {
            try header(name)
            let input = try FileHandle(forReadingFrom: url)
            defer { try? input.close() }
            while let data = try input.read(upToCount: 1024 * 1024), !data.isEmpty {
                try output.write(contentsOf: data)
            }
            try write("\r\n")
        }
        var directories: [String] = []
        for file in files {
            let scoped = file.fileURL?.startAccessingSecurityScopedResource() ?? false
            defer { if scoped { file.fileURL?.stopAccessingSecurityScopedResource() } }
            if let url = file.fileURL {
                let values = try url.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
                guard values.isSymbolicLink != true else { throw ShortcutSession.failure("暂不支持上传符号链接") }
                if values.isDirectory == true {
                    let rootName = url.lastPathComponent
                    directories.append(rootName)
                    var enumerationError: Error?
                    guard let iterator = FileManager.default.enumerator(at: url,
                        includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey, .isRegularFileKey],
                        errorHandler: { _, error in enumerationError = error; return false }) else {
                        throw ShortcutSession.failure("无法读取文件夹")
                    }
                    while let entry = iterator.nextObject() {
                        guard let child = entry as? URL else { continue }
                        let values = try child.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey, .isRegularFileKey])
                        guard values.isSymbolicLink != true else { throw ShortcutSession.failure("暂不支持上传符号链接") }
                        let relative = rootName + "/" + String(child.path.dropFirst(url.path.count + 1))
                        if values.isDirectory == true { directories.append(relative) }
                        else if values.isRegularFile == true { try append(child, name: relative) }
                        else { throw ShortcutSession.failure("文件夹包含不支持的文件类型") }
                    }
                    if let error = enumerationError { throw error }
                } else { try append(url, name: file.filename) }
            } else {
                try header(file.filename)
                try output.write(contentsOf: file.data)
                try write("\r\n")
            }
        }
        if !directories.isEmpty {
            try write("--\(boundary)\r\nContent-Disposition: form-data; name=\"directories\"\r\n\r\n")
            try output.write(contentsOf: JSONSerialization.data(withJSONObject: directories))
            try write("\r\n")
        }
        try write("--\(boundary)--\r\n")
        try output.close()
        let data = try await ShortcutSession.request(url: account.endpoint("files"),
            token: account.uploadToken, method: "POST", bodyFile: bodyURL,
            contentType: "multipart/form-data; boundary=\(boundary)", timeout: 300)
        try ShortcutSession.checkUploadResult(data)
    }
}

@available(iOS 16.0, *)
struct ClipRoamAppShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(intent: SyncTextToClipRoam(), phrases: [
            "同步文本到 \(.applicationName)",
        ], shortTitle: "同步文本", systemImageName: "text.badge.plus")
        AppShortcut(intent: SyncFilesToClipRoam(), phrases: [
            "同步文件到 \(.applicationName)",
        ], shortTitle: "同步文件", systemImageName: "doc")
    }
}

