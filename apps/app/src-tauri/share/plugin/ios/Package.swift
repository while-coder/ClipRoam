// swift-tools-version:5.3
import PackageDescription

let package = Package(
    name: "tauri-plugin-cliproam-share-receiver",
    platforms: [.iOS(.v14)],
    products: [.library(name: "tauri-plugin-cliproam-share-receiver", type: .static, targets: ["ClipRoamSharePlugin"])],
    dependencies: [.package(name: "Tauri", path: "../.tauri/tauri-api")],
    targets: [
        .target(name: "ClipRoamSharePlugin", dependencies: [.byName(name: "Tauri")], path: "Sources"),
    ]
)
