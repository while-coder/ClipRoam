// swift-tools-version:5.3
import PackageDescription

let package = Package(
    name: "tauri-plugin-cliproam-share-receiver",
    platforms: [.iOS(.v14)],
    products: [.library(name: "tauri-plugin-cliproam-share-receiver", type: .static, targets: ["ClipRoamSharePlugin"])],
    dependencies: [.package(name: "Tauri", path: "../.tauri/tauri-api")],
    targets: [
        .target(name: "ShareUploadCore", path: "FFI", publicHeadersPath: "."),
        .target(name: "ClipRoamShareSupport", dependencies: ["ShareUploadCore"], path: "Sources/Support"),
        .target(name: "ClipRoamSharePlugin", dependencies: ["ClipRoamShareSupport", .byName(name: "Tauri")], path: "Sources/Plugin"),
    ]
)
