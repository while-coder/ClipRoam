import UIKit
import ShareUploadCore

class ShareViewController: UIViewController {
    private var started = false

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        guard !started else { return }
        started = true
        let pointer = UInt(bitPattern: Unmanaged.passUnretained(self).toOpaque())
        let input = "{\"command\":\"ios.start\",\"controller\":\(pointer)}"
        guard let output = input.withCString({ cliproam_share_call($0) }) else { return }
        defer { cliproam_share_free(output) }
        if let data = String(cString: output).data(using: .utf8),
           let result = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           result["ok"] as? Bool != true {
            let alert = UIAlertController(title: "接收分享失败", message: result["error"] as? String, preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "关闭", style: .default) { [weak self] _ in
                self?.extensionContext?.completeRequest(returningItems: nil)
            })
            present(alert, animated: true)
        }
    }
}
