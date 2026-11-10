import Foundation

enum AppBrand {
    static var name: String {
        (Locale.preferredLanguages.first ?? "en").lowercased().hasPrefix("zh") ? "随心贴" : "ClipRoam"
    }

    static func message(_ value: String) -> String {
        value.replacingOccurrences(of: "ClipRoam", with: name)
    }
}
