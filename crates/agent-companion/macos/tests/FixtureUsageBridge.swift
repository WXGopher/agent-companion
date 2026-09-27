// SPDX-License-Identifier: GPL-3.0-only
import Foundation

/// UI tests supply completed Rust snapshots; they never duplicate its scheduler.
final class FixtureUsageBridge: SubscriptionUsageBridging {
    var value = SubscriptionSnapshot()
    var events: [SubscriptionEvent] = []
    var snapshotReads = 0
    var onEvent: ((SubscriptionEvent) -> Void)?

    func snapshot() -> SubscriptionSnapshot? { snapshotReads += 1; return value }
    func send(_ event: SubscriptionEvent) -> SubscriptionSnapshot? {
        events.append(event)
        onEvent?(event)
        return value
    }
    func count(_ event: String) -> Int { events.filter { $0.event == event }.count }
    func set(_ usage: SubscriptionUsage, for source: SubscriptionSource = .init(codexHome: ""), loading: Bool = false) {
        let state = SubscriptionInstanceSnapshot(source: source,
            limits: SubscriptionQuery(value: usage.limits, lastSuccessAt: usage.readAt?.timeIntervalSince1970,
                                      loading: loading, error: usage.limitsError ?? usage.error),
            history: SubscriptionQuery(value: usage.tokens, lastSuccessAt: usage.tokensReadAt?.timeIntervalSince1970,
                                       loading: usage.tokensLoading, error: usage.tokenError))
        value.instances.removeAll { $0.source == source }
        value.instances.append(state)
    }
    static func quota(_ used: Int, readAt: Date = Date(timeIntervalSince1970: 2_000_000_000),
                      resetsAt: TimeInterval = 4_000_000_000) -> SubscriptionUsage {
        let window = SubscriptionLimits.Window(usedPercent: Double(used), windowDurationMins: 10080, resetsAt: resetsAt)
        let bucket = SubscriptionLimits.Bucket(limitId: "codex", limitName: nil, planType: nil, primary: nil, secondary: window)
        return SubscriptionUsage(limits: SubscriptionLimits(rateLimits: bucket, rateLimitsByLimitId: nil), readAt: readAt)
    }
}
