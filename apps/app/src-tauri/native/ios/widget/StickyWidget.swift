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

    private func row(_ note: StickyWidgetNote) -> some View {
        let lines = note.content.components(separatedBy: .newlines).filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
        return VStack(alignment: .leading, spacing: 4) {
            Text(lines.first ?? note.kind).font(.body).foregroundColor(.primary).lineLimit(1)
            if lines.count > 1 {
                Text(lines.dropFirst().joined(separator: " ")).font(.caption).foregroundColor(.secondary).lineLimit(1)
            } else {
                Text("无更多文本").font(.caption).foregroundColor(.secondary).lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
        .padding(.vertical, 6).contentShape(Rectangle())
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "folder")
                Text("便签").font(.headline)
                Spacer(minLength: 0)
                Text("\(entry.state.notes.count)").font(.caption)
            }
            .foregroundColor(.primary)
            .padding(.horizontal, 12).padding(.vertical, 10)
            VStack(alignment: .leading, spacing: 0) {
                if visibleNotes.isEmpty {
                    Text(emptyMessage).font(.subheadline).foregroundColor(.secondary)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
                } else {
                    ForEach(visibleNotes) { note in
                        if family == .systemSmall { row(note) }
                        else { Link(destination: entryURL(note)) { row(note) } }
                        Divider()
                    }
                    Spacer(minLength: 0)
                }
                if entry.offline {
                    Label("缓存", systemImage: "wifi.slash").font(.caption2).foregroundColor(.secondary)
                }
            }.padding(.horizontal, 12)
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
            protectedContent.padding().background(Color(UIColor.systemBackground))
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
