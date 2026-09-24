// SPDX-License-Identifier: GPL-3.0-only
import Foundation

/// The menu and both Usage surfaces share one read per exact source. Background
/// demand cannot switch or cancel a foreground reader, and closing a surface
/// does not stop a read still needed by the visible menu.
final class SubscriptionUsageCoordinator {
    private final class Entry {
        let monitor: SubscriptionMonitor
        var weekly: (usage: WeeklyUsage, readAt: Date)?
        var local: WeeklyUsage?
        var displayed: WeeklyUsage?
        var failed = false
        var loading = false
        init(monitor: SubscriptionMonitor) { self.monitor = monitor }
    }
    private let makeReader: (SubscriptionSource) -> SubscriptionReading
    private let clock: () -> Date
    private var entries: [SubscriptionSource: Entry] = [:]
    private var foreground: [ObjectIdentifier: SubscriptionSource] = [:]
    private var backgroundSources: Set<SubscriptionSource> = []
    var onChange: ((SubscriptionSource, SubscriptionUsage?, Bool) -> Void)?

    init(makeReader: @escaping (SubscriptionSource) -> SubscriptionReading = { _ in CodexSubscriptionReader() },
         clock: @escaping () -> Date = Date.init) {
        self.makeReader = makeReader
        self.clock = clock
    }

    private func entry(for source: SubscriptionSource) -> Entry {
        if let entry = entries[source] { return entry }
        let entry = Entry(monitor: SubscriptionMonitor(reader: makeReader(source), clock: clock))
        entries[source] = entry
        entry.monitor.onChange = { [weak self, weak entry] value, loading in
            guard let self, let entry else { return }
            entry.loading = loading
            if let value, value.readAt != nil || value.error != nil || value.limitsError != nil {
                if let weekly = Self.weekly(value) {
                    entry.weekly = weekly
                    entry.failed = false
                } else {
                    // Retain the last success for an explicitly stale menu
                    // value; a failure must never renew its timestamp.
                    entry.failed = true
                }
            }
            self.onChange?(source, value, loading)
        }
        return entry
    }

    static func weekly(_ value: SubscriptionUsage) -> (usage: WeeklyUsage, readAt: Date)? {
        guard value.error == nil, value.limitsError == nil, let readAt = value.readAt,
              let bucket = value.limits?.buckets.first(where: { $0.id == "codex" }),
              let window = [bucket.value.primary, bucket.value.secondary].compactMap({ $0 })
                .first(where: { $0.windowDurationMins == 10080 }) else { return nil }
        return (WeeklyUsage(usedPercent: window.usedPercent, resetsAt: window.resetsAt, expired: false), readAt)
    }

    func synchronize(instances: [CodexInstance], backgroundEnabled: Bool) {
        let sources = Set(instances.map(\.usageSource))
        foreground = foreground.filter { sources.contains($0.value) }
        // Empty/loading snapshots may still support an explicit foreground
        // read, but must not silently discover a CLI in the background.
        backgroundSources = backgroundEnabled
            ? Set(sources.filter { !$0.codexHome.isEmpty }) : []
        let demanded = backgroundSources.union(foreground.values)
        for source in Array(entries.keys) {
            if !sources.contains(source) {
                entries.removeValue(forKey: source)?.monitor.stop()
            } else if !demanded.contains(source) {
                stopReading(source)
            }
        }
        for instance in instances where demanded.contains(instance.usageSource) {
            if let local = instance.weekly { entry(for: instance.usageSource).local = local }
        }
    }

    func refreshBackground() {
        for source in backgroundSources {
            let entry = entry(for: source)
            if entry.monitor.needsRefresh(for: source) { entry.monitor.refresh(source: source) }
        }
    }

    func refresh(source: SubscriptionSource, owner: AnyObject, force: Bool = false) {
        let id = ObjectIdentifier(owner)
        if let previous = foreground[id], previous != source { release(owner: owner) }
        foreground[id] = source
        let entry = entry(for: source)
        if entry.loading {
            onChange?(source, entry.monitor.cachedUsage(for: source), true)
        } else {
            entry.monitor.refresh(source: source, force: force)
        }
    }

    func release(owner: AnyObject) {
        guard let source = foreground.removeValue(forKey: ObjectIdentifier(owner)),
              !backgroundSources.contains(source), !foreground.values.contains(source) else { return }
        stopReading(source)
    }

    private func stopReading(_ source: SubscriptionSource) {
        guard let entry = entries[source], entry.loading else { return }
        entry.monitor.stop()
        entry.loading = false
    }

    func cachedWeeklyUsage(for source: SubscriptionSource) -> (usage: WeeklyUsage, readAt: Date)? {
        guard let entry = entries[source], !entry.failed else { return nil }
        return entry.weekly
    }

    func reading(for instance: CodexInstance) -> MenuBarUsage.Reading? {
        let now = clock()
        let entry = entries[instance.usageSource]
        if let account = entry?.weekly {
            let age = now.timeIntervalSince(account.readAt)
            if entry?.failed == false, age >= 0, age < SubscriptionMonitor.refreshInterval,
               MenuBarUsage.remaining(account.usage, at: now) != nil {
                entry?.displayed = account.usage
                return .init(usage: account.usage)
            }
        }
        if let local = instance.weekly, MenuBarUsage.remaining(local, at: now) != nil {
            entry?.displayed = local
            return .init(usage: local, stale: entry?.failed == true)
        }
        if let known = entry?.displayed ?? entry?.weekly?.usage ?? instance.weekly ?? entry?.local {
            return .init(usage: known, stale: true)
        }
        return nil
    }

    func stop() {
        foreground.removeAll()
        backgroundSources.removeAll()
        let previous = entries
        entries.removeAll()
        for entry in previous.values { entry.monitor.stop() }
    }
}
