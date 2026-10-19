import Tauri
import UIKit
import WebKit

private struct AcknowledgeArgs: Decodable {
    let id: String
}

class ShareReceiverPlugin: Plugin {
    private var observer: NSObjectProtocol?

    override func load(webview: WKWebView) {
        observer = NotificationCenter.default.addObserver(
            forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main
        ) { [weak self] _ in
            self?.trigger("received", data: [:] as JSObject)
        }
    }

    deinit {
        if let observer = observer { NotificationCenter.default.removeObserver(observer) }
    }

    @objc public func pending(_ invoke: Invoke) throws {
        try invoke.resolve(ShareInbox.pending())
    }

    @objc public func acknowledge(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(AcknowledgeArgs.self)
        try ShareInbox.acknowledge(args.id)
        invoke.resolve()
    }
}

@_cdecl("init_plugin_cliproam_share_receiver")
func initPlugin() -> Plugin {
    ShareReceiverPlugin()
}
