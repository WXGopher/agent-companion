// SPDX-License-Identifier: GPL-3.0-only
import Foundation

@_silgen_name("agent_companion_usage_snapshot_json")
private func readUsageSnapshot() -> UnsafeMutablePointer<CChar>?
@_silgen_name("agent_companion_usage_event")
private func sendUsageEvent(_ json: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
@_silgen_name("agent_companion_release_json")
private func releaseUsageSnapshot(_ pointer: UnsafeMutablePointer<CChar>?)

struct SubscriptionQuery<Value: Codable & Equatable>: Codable, Equatable {
    var value: Value?
    var lastSuccessAt: TimeInterval?
    var completedAt: TimeInterval?
    var loading = false
    var error: String?
    var nextQueryAt: TimeInterval?
    var elapsedMs: UInt64?
}

struct SubscriptionInstanceSnapshot: Codable, Equatable {
    var source: SubscriptionSource
    var limits = SubscriptionQuery<SubscriptionLimits>()
    var history = SubscriptionQuery<SubscriptionTokens>()

    var usage: SubscriptionUsage {
        SubscriptionUsage(tokens: history.value, limits: limits.value,
                          tokenError: history.error, limitsError: limits.error,
                          readAt: limits.lastSuccessAt.map(Date.init(timeIntervalSince1970:)),
                          tokensReadAt: history.lastSuccessAt.map(Date.init(timeIntervalSince1970:)),
                          tokensLoading: history.loading)
    }
}

struct SubscriptionSnapshot: Codable, Equatable {
    var intervalMinutes = 5
    var instances: [SubscriptionInstanceSnapshot] = []
}

struct SubscriptionEvent: Codable, Equatable {
    let event: String
    var instanceId: String?

    static let panelOpen = Self(event: "panelOpen")
    static func refresh(_ id: String) -> Self { Self(event: "refresh", instanceId: id) }
    static func history(_ id: String) -> Self { Self(event: "history", instanceId: id) }
}

protocol SubscriptionUsageBridging: AnyObject {
    func snapshot() -> SubscriptionSnapshot?
    func send(_ event: SubscriptionEvent) -> SubscriptionSnapshot?
}

final class RustSubscriptionUsageBridge: SubscriptionUsageBridging {
    func snapshot() -> SubscriptionSnapshot? { decode(readUsageSnapshot()) }

    func send(_ event: SubscriptionEvent) -> SubscriptionSnapshot? {
        guard let bytes = try? JSONEncoder().encode(event), let json = String(data: bytes, encoding: .utf8) else { return nil }
        return json.withCString { decode(sendUsageEvent($0)) }
    }

    private func decode(_ pointer: UnsafeMutablePointer<CChar>?) -> SubscriptionSnapshot? {
        guard let pointer else { return nil }
        defer { releaseUsageSnapshot(pointer) }
        return try? JSONDecoder().decode(SubscriptionSnapshot.self, from: Data(String(cString: pointer).utf8))
    }
}

/// A presentation adapter only. Rust owns source synchronization, scheduling,
/// concurrent requests, cancellation, caching and all account subprocesses.
final class SubscriptionUsageCoordinator {
    private let bridge: SubscriptionUsageBridging
    private(set) var snapshot = SubscriptionSnapshot()
    var onChange: (() -> Void)?

    init(bridge: SubscriptionUsageBridging = RustSubscriptionUsageBridge()) { self.bridge = bridge }

    func refreshSnapshot() { receive(bridge.snapshot()) }
    func panelOpened() { receive(bridge.send(.panelOpen)) }
    func refreshQuota(instanceID: String) { receive(bridge.send(.refresh(instanceID))) }
    func loadHistory(instanceID: String) { receive(bridge.send(.history(instanceID))) }

    private func receive(_ value: SubscriptionSnapshot?) {
        guard let value, value != snapshot else { return }
        snapshot = value
        onChange?()
    }

    func state(for source: SubscriptionSource) -> SubscriptionInstanceSnapshot? {
        snapshot.instances.first { $0.source.matches(source) }
    }

    static func weekly(_ value: SubscriptionUsage) -> (usage: WeeklyUsage, readAt: Date)? {
        guard let readAt = value.readAt,
              let bucket = value.limits?.buckets.first(where: { $0.id == "codex" })?.value else { return nil }
        let explicit = [bucket.secondary, bucket.primary].compactMap { $0 }.first { $0.windowDurationMins == 10080 }
        let legacy = bucket.secondary.flatMap { $0.windowDurationMins == nil ? $0 : nil }
        guard let window = explicit ?? legacy else { return nil }
        return (WeeklyUsage(usedPercent: Int(min(100, max(0, window.usedPercent)).rounded()), resetsAt: window.resetsAt, expired: false), readAt)
    }

    func cachedWeeklyUsage(for source: SubscriptionSource) -> (usage: WeeklyUsage, readAt: Date)? {
        state(for: source).flatMap { Self.weekly($0.usage) }
    }

    func reading(for instance: CodexInstance) -> MenuBarUsage.Reading? {
        guard let state = state(for: instance.usageSource) else { return nil }
        return .init(usage: Self.weekly(state.usage)?.usage, stale: state.limits.error != nil,
                     readAt: state.usage.readAt, error: state.limits.error)
    }
}
