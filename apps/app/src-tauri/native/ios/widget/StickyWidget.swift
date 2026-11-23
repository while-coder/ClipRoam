import Foundation
import SwiftUI
import UIKit
import WidgetKit

struct StickyEntry: TimelineEntry {
    let date: Date
    let state: StickyWidgetState
    var offline = false
    var unavailable = false
}

struct StickyProvider: TimelineProvider {
    private let refreshInterval: TimeInterval = 30 * 60

    func placeholder(in context: Context) -> StickyEntry {
        StickyEntry(date: Date(), state: StickyWidgetState(notes: [
            StickyWidgetNote(id: "preview", kind: "text", content: "把常用的内容留在手边。"),
        ]))
    }

    func getSnapshot(in context: Context, completion: @escaping (StickyEntry) -> Void) {
        if context.isPreview { completion(placeholder(in: context)); return }
        completion(cached())
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<StickyEntry>) -> Void) {
        func finish(_ entry: StickyEntry) {
            completion(Timeline(entries: [entry], policy: .after(Date().addingTimeInterval(refreshInterval))))
        }
        let previous = cached()
        guard let session = previous.state.session, let url = try? session.endpoint() else {
            finish(previous)
            return
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 15
        configuration.timeoutIntervalForResource = 20
        let network = URLSession(configuration: configuration)
        var request = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData)
        request.setValue("Bearer \(session.sessionToken)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        network.dataTask(with: request) { data, response, error in
            defer { network.finishTasksAndInvalidate() }
            do {
                if let error = error { throw error }
                guard let response = response as? HTTPURLResponse else { throw URLError(.badServerResponse) }
                if response.statusCode == 401 || response.statusCode == 403 {
                    let state = try StickyWidgetStore.expire(session)
                    finish(StickyEntry(date: Date(), state: state))
                    return
                }
                guard response.statusCode == 200, let data = data else { throw URLError(.badServerResponse) }
                struct Response: Decodable { let entries: [StickyWidgetNote] }
                let notes = try JSONDecoder().decode(Response.self, from: data).entries
                let state = try StickyWidgetStore.save(notes: notes, for: session)
                finish(StickyEntry(date: Date(), state: state))
            } catch {
                // Reload the current account's cache, rather than a snapshot captured before logout.
                var entry = self.cached()
                entry.offline = true
                finish(entry)
            }
        }.resume()
    }

    private func cached() -> StickyEntry {
        do { return StickyEntry(date: Date(), state: try StickyWidgetStore.load()) }
        catch { return StickyEntry(date: Date(), state: StickyWidgetState(), unavailable: true) }
    }
}

struct StickyWidgetView: View {
    let entry: StickyEntry
    @Environment(\.widgetFamily) private var family

    private var visibleNotes: [StickyWidgetNote] {
        Array(entry.state.notes.prefix(family == .systemSmall ? 1 : family == .systemMedium ? 2 : 5))
    }

    private var emptyMessage: LocalizedStringKey {
        if entry.unavailable { return "请解锁设备并打开 ClipRoam" }
        if entry.state.requiresLogin { return "登录已失效，请打开 ClipRoam" }
        if entry.state.session == nil { return "打开 ClipRoam 登录后查看便签" }
        if entry.offline && entry.state.updatedAt == nil { return "暂时无法连接，稍后自动重试" }
        return "还没有便签，在历史记录中添加"
    }

    private func entryURL(_ note: StickyWidgetNote) -> URL {
        var url = URLComponents()
        url.scheme = "cliproam"
        url.host = "widget"
        url.queryItems = [URLQueryItem(name: "id", value: note.id)]
        return url.url!
    }

    private func thumbnail(_ note: StickyWidgetNote) -> UIImage? {
        guard note.kind == "image", let encoded = note.imageInfo?.thumbnail,
              let data = Data(base64Encoded: encoded) else { return nil }
        return UIImage(data: data)
    }

    @ViewBuilder
    private func row(_ note: StickyWidgetNote) -> some View {
        if family == .systemSmall {
            VStack(alignment: .leading, spacing: 4) {
                if let image = thumbnail(note) {
                    Image(uiImage: image).resizable().scaledToFit()
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .accessibilityLabel(Text(note.content))
                } else {
                    Text(note.content).font(.system(size: 13)).foregroundColor(.primary)
                        .lineLimit(nil)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .clipped()
        } else {
            HStack(alignment: .top, spacing: 6) {
                if let image = thumbnail(note) {
                    Image(uiImage: image).resizable().scaledToFit()
                        .frame(width: 40, height: 40)
                        .accessibilityLabel(Text(note.content))
                }
                Text(note.content).font(.system(size: 13)).foregroundColor(.primary)
                    .lineLimit(2).multilineTextAlignment(.leading)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.vertical, 5).contentShape(Rectangle())
        }
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 5) {
                Image(systemName: "folder")
                Text("便签").font(.system(size: 12, weight: .semibold))
                Spacer(minLength: 0)
                Text("\(entry.state.notes.count)").font(.caption)
            }
            .font(.system(size: 11))
            .foregroundColor(.primary)
            .padding(.horizontal, 8).padding(.vertical, 5)
            VStack(alignment: .leading, spacing: 0) {
                if visibleNotes.isEmpty {
                    Text(emptyMessage).font(.subheadline).foregroundColor(.secondary)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
                } else {
                    ForEach(visibleNotes) { note in
                        if family == .systemSmall { row(note) }
                        else { Link(destination: entryURL(note)) { row(note) } }
                        if family != .systemSmall && note.id != visibleNotes.last?.id { Divider() }
                    }
                    if family != .systemSmall { Spacer(minLength: 0) }
                }
                if entry.offline {
                    Label("缓存", systemImage: "wifi.slash").font(.caption2).foregroundColor(.secondary)
                }
            }.padding(.horizontal, 8).padding(.bottom, 6)
        }
        .widgetURL(visibleNotes.first.map { entryURL($0) } ?? URL(string: "cliproam://widget"))
    }

    @ViewBuilder
    private var protectedContent: some View {
        if #available(iOS 15.0, *) { content.privacySensitive() }
        else { content }
    }

    var body: some View {
        if #available(iOS 17.0, *) {
            protectedContent.containerBackground(.background, for: .widget)
        } else {
            protectedContent.padding(8).background(Color(UIColor.systemBackground))
        }
    }
}

@main
struct ClipRoamStickyWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: StickyWidgetStore.kind, provider: StickyProvider()) { entry in
            StickyWidgetView(entry: entry)
        }
        .configurationDisplayName("ClipRoam 便签")
        .description("查看最近的便签，自动联网更新。")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
    }
}
