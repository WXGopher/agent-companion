// SPDX-License-Identifier: GPL-3.0-only
import Foundation

/// Account-wide Codex activity returned by account/usage/read. Optional values
/// mean unavailable; they must not become zero or be inferred from quota %.
struct SubscriptionTokens: Decodable {
    struct Summary: Decodable {
        var lifetimeTokens: Int64?
        var peakDailyTokens: Int64?
        var longestRunningTurnSec: Int64?
        var currentStreakDays: Int64?
        var longestStreakDays: Int64?
    }
    struct Day: Decodable, Identifiable {
        let startDate: String
        let tokens: Int64
        var id: String { startDate }
    }
    let summary: Summary
    let dailyUsageBuckets: [Day]?

    // Dates are the service's calendar labels. Do not shift them into the Mac's
    // time zone or invent zero-valued buckets for days the service omitted.
    var recentDays: [Day] {
        Array((dailyUsageBuckets ?? []).filter { $0.tokens >= 0 }
            .sorted { $0.startDate > $1.startDate }.prefix(7))
    }
}

struct SubscriptionLimits: Decodable {
    struct Window: Decodable {
        let usedPercent: Int
        let windowDurationMins: Int?
        let resetsAt: TimeInterval?

        func remaining(at now: Date = Date()) -> Int? {
            if let resetsAt, resetsAt <= now.timeIntervalSince1970 { return nil }
            return 100 - min(100, max(0, usedPercent))
        }
        func title(fallback: String) -> String {
            guard let minutes = windowDurationMins, minutes > 0 else { return fallback }
            if minutes == 10080 { return "Weekly" }
            if minutes.isMultiple(of: 1440) { return "\(minutes / 1440)-day" }
            if minutes.isMultiple(of: 60) { return "\(minutes / 60)-hour" }
            return "\(minutes)-minute"
        }
    }
    struct Bucket: Decodable {
        let limitId: String?
        let limitName: String?
        let planType: String?
        let primary: Window?
        let secondary: Window?
    }
    let rateLimits: Bucket
    let rateLimitsByLimitId: [String: Bucket]?

    var buckets: [(id: String, value: Bucket)] {
        if let values = rateLimitsByLimitId, !values.isEmpty {
            return values.keys.sorted { left, right in
                if left == "codex" { return right != "codex" }
                if right == "codex" { return false }
                return left < right
            }.map { ($0, values[$0]!) }
        }
        return [(rateLimits.limitId ?? "codex", rateLimits)]
    }
}

struct SubscriptionUsage {
    var tokens: SubscriptionTokens?
    var limits: SubscriptionLimits?
    var tokenError: String?
    var limitsError: String?
    var error: String?
    var readAt: Date?

    static func failure(_ message: String) -> Self { Self(error: message) }
}

enum UsageNumber {
    static func full(_ value: Int64?) -> String {
        guard let value, value >= 0 else { return "—" }
        return value.formatted(.number)
    }
    static func short(_ value: Int64?) -> String {
        guard let value, value >= 0 else { return "—" }
        for (divisor, suffix) in [(1_000_000_000_000.0, "T"), (1_000_000_000.0, "B"), (1_000_000.0, "M"), (1_000.0, "K")] {
            if Double(value) >= divisor {
                return (Double(value) / divisor).formatted(.number.precision(.fractionLength(0...1))) + suffix
            }
        }
        return String(value)
    }
    static func duration(_ seconds: Int64?) -> String {
        guard let seconds, seconds >= 0 else { return "—" }
        if seconds >= 3600 { return "\(seconds / 3600)h \((seconds % 3600) / 60)m" }
        if seconds >= 60 { return "\(seconds / 60)m \(seconds % 60)s" }
        return "\(seconds)s"
    }
}

protocol SubscriptionReading: AnyObject {
    func read(codexHome: String, completion: @escaping (SubscriptionUsage) -> Void)
    func cancel()
}

/// Main-thread coordinator; the CLI is launched only after opening Usage.
/// No persisted account data, no API keys, and no extra model turns.
final class SubscriptionMonitor {
    private let reader: SubscriptionReading
    private let clock: () -> Date
    private var generation = 0
    private var inFlight = false
    private var lastAttempt: Date?
    private var sourceHome: String?
    var onChange: ((SubscriptionUsage?, Bool) -> Void)?

    init(reader: SubscriptionReading = CodexSubscriptionReader(), clock: @escaping () -> Date = Date.init) {
        self.reader = reader
        self.clock = clock
    }

    func refresh(codexHome: String, force: Bool = false) {
        if sourceHome != codexHome {
            stop()
            sourceHome = codexHome
            lastAttempt = nil
            onChange?(SubscriptionUsage(), false)
        }
        guard !inFlight, force || lastAttempt == nil || clock().timeIntervalSince(lastAttempt!) >= 300 else { return }
        inFlight = true
        lastAttempt = clock()
        generation &+= 1
        let request = generation
        onChange?(nil, true)
        reader.read(codexHome: codexHome) { [weak self] result in
            guard let self, request == self.generation else { return }
            self.inFlight = false
            // Replace the entire snapshot, including on failure. This avoids
            // displaying another account's cached data after a login change.
            self.onChange?(result, false)
        }
    }

    func stop() {
        generation &+= 1
        reader.cancel()
        // Reopening a completed read within five minutes uses the existing
        // snapshot. An interrupted request is retried on the next visit.
        if inFlight { lastAttempt = nil }
        inFlight = false
    }
}
