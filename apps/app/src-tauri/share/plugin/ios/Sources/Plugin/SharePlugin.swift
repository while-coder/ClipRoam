import Foundation
import CoreFoundation
import ObjectiveC
import Tauri
import UIKit
import WebKit
import ClipRoamShareSupport

private final class ShareHost: NSObject {
    static var original: IMP?
    static var installed = false

    static func install() {
        guard !installed, let delegate = UIApplication.shared.delegate, let type = object_getClass(delegate),
              let replacement = class_getInstanceMethod(ShareHost.self, #selector(receive(_:handleEventsForBackgroundURLSession:completionHandler:))) else { return }
        let selector = NSSelectorFromString("application:handleEventsForBackgroundURLSession:completionHandler:")
        if let method = class_getInstanceMethod(type, selector) { original = method_getImplementation(method) }
        if !class_addMethod(type, selector, method_getImplementation(replacement), method_getTypeEncoding(replacement)) {
            if let method = class_getInstanceMethod(type, selector) { method_setImplementation(method, method_getImplementation(replacement)) }
        }
        // Refresh UIKit's cached optional delegate methods.
        UIApplication.shared.delegate = delegate
        installed = true
    }

    @objc func receive(_ application: UIApplication, handleEventsForBackgroundURLSession identifier: String, completionHandler: @escaping () -> Void) {
        if identifier.hasPrefix(ShareHTTP.prefix) {
            ShareHTTP.shared.reconnect(identifier, completion: completionHandler)
        } else if let original = ShareHost.original {
            typealias Callback = @convention(c) (AnyObject, Selector, UIApplication, NSString, @convention(block) () -> Void) -> Void
            let callback = unsafeBitCast(original, to: Callback.self)
            callback(self, NSSelectorFromString("application:handleEventsForBackgroundURLSession:completionHandler:"), application, identifier as NSString, completionHandler)
        } else { completionHandler() }
    }
}

private final class SharePlugin: Plugin {
    private var observer: NSObjectProtocol?
    override func load(webview: WKWebView) {
        ShareHost.install()
        ShareHTTP.shared.onChanged = { [weak self] in self?.trigger("share-received", data: [:] as JSObject) }
        observer = NotificationCenter.default.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in self?.resume() }
        resume()
    }
    deinit { if let observer = observer { NotificationCenter.default.removeObserver(observer) } }
    private func resume() {
        DispatchQueue.global(qos: .utility).async {
            if let result = try? ShareCore.call(["command": "resume"]) { ShareHTTP.shared.startJobs(result) }
            DispatchQueue.main.async { self.trigger("share-received", data: [:] as JSObject) }
        }
    }
    @objc func call(_ invoke: Invoke) {
        DispatchQueue.global(qos: .utility).async {
            do {
                let arguments = try JSONSerialization.jsonObject(with: Data(invoke.getRawArgs().utf8)) as? [String: Any]
                guard let input = arguments?["input"] as? [String: Any] else { throw ShareCore.failure("分享参数无效") }
                let result = try ShareCore.call(input)
                ShareHTTP.shared.startJobs(result) { error in
                    if let error = error { invoke.reject(error.localizedDescription) }
                    else { invoke.resolve(EncodableJSON(result)) }
                }
            } catch { invoke.reject(error.localizedDescription) }
        }
    }
}

// Invoke.resolve accepts Encodable; retain JSON primitives without stringifying.
private struct EncodableJSON: Encodable {
    let value: Any
    init(_ value: Any) { self.value = value }
    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch value {
        case is NSNull: try container.encodeNil()
        case let value as String: try container.encode(value)
        case let value as NSNumber:
            if CFGetTypeID(value) == CFBooleanGetTypeID() { try container.encode(value.boolValue) }
            else { try container.encode(value.doubleValue) }
        case let value as [Any]: try container.encode(value.map(EncodableJSON.init))
        case let value as [String: Any]: try container.encode(value.mapValues(EncodableJSON.init))
        default: throw ShareCore.failure("无法编码分享响应")
        }
    }
}

@_cdecl("init_plugin_cliproam_share_receiver")
func initPlugin() -> Plugin { SharePlugin() }
