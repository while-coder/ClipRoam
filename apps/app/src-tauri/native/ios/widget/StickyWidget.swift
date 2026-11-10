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
        Array(entry.state.notes.prefix(family == .systemSmall ? 1 : family == .systemMedium ? 3 : 5))
    }

    private var emptyMessage: LocalizedStringKey {
        if entry.unavailable { return "请解锁设备并打开 ClipRoam" }
        if entry.state.requiresLogin { return "登录已失效，请打开 ClipRoam" }
        if entry.state.session == nil { return "打开 ClipRoam 登录后查看便签" }
        if entry.offline && entry.state.updatedAt == nil { return "暂时无法连接，稍后自动重试" }
        return "还没有便签，在历史记录中添加"
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: "note.text").foregroundColor(.accentColor)
                Text("便签").font(.headline)
                Spacer(minLength: 0)
                if !entry.state.notes.isEmpty {
                    Text("\(entry.state.notes.count)").font(.caption).foregroundColor(.secondary)
                }
            }
            if visibleNotes.isEmpty {
                Text(emptyMessage).font(.subheadline).foregroundColor(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
            } else {
                ForEach(visibleNotes) { note in
                    HStack(alignment: .top, spacing: 6) {
                        if note.kind != "text" {
                            Image(systemName: note.kind == "image" ? "photo" : "doc")
                                .foregroundColor(.secondary).accessibilityHidden(true)
                        }
                        Text(note.content).font(.body)
                            .lineLimit(family == .systemSmall ? 5 : family == .systemMedium ? 1 : 2)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                Spacer(minLength: 0)
            }
            if let updated = entry.state.updatedAt {
                HStack(spacing: 4) {
                    if entry.offline { Image(systemName: "wifi.slash"); Text("缓存") }
                    else { Text("更新于") }
                    Text(updated, style: .time)
                }.font(.caption2).foregroundColor(.secondary)
            }
        }
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
