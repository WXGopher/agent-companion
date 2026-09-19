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
    func read(source: SubscriptionSource, completion: @escaping (SubscriptionUsage) -> Void)
    func cancel()
}

extension SubscriptionReading {
    func read(source: SubscriptionSource, completion: @escaping (SubscriptionUsage) -> Void) {
        read(codexHome: source.codexHome, completion: completion)
    }
}

/// All routing inputs participate in cache identity. A redeployed runtime must
/// never inherit a prior runtime's account reading, even at the same home.
struct SubscriptionSource: Hashable {
    var instanceID = "codex"
    var codexHome: String
    var executablePath: String?
    var databasePath: String?
}

enum InstanceEnvironment {
    static let clearedPrefixes = ["CODEX_", "OPENAI_", "CHATGPT_", "ELECTRON_", "DYLD_"]
    static let clearedNames = ["NODE_OPTIONS", "NODE_PATH"]
    static func shouldClear(_ key: String) -> Bool {
        clearedNames.contains(key) || clearedPrefixes.contains { key.hasPrefix($0) }
    }
    static func isolated(_ inherited: [String: String], source: SubscriptionSource) -> [String: String] {
        var result = inherited.filter { !shouldClear($0.key) }
        result["CODEX_HOME"] = source.codexHome
        if let database = source.databasePath { result["CODEX_SQLITE_HOME"] = database }
        return result
    }
    static func configurationArguments(_ source: SubscriptionSource) -> [String] {
        var arguments: [String] = []
        if source.instanceID != "codex" { arguments += ["-c", "cli_auth_credentials_store=\"file\""] }
        if let database = source.databasePath,
           let encoded = try? JSONEncoder().encode(database), let value = String(data: encoded, encoding: .utf8) {
            arguments += ["-c", "sqlite_home=\(value)"]
        }
        return arguments
    }
}

/// Main-thread coordinator; the CLI is launched only after opening Usage.
/// No persisted account data, no API keys, and no extra model turns.
final class SubscriptionMonitor {
    private let reader: SubscriptionReading
    private let clock: () -> Date
    private var generation = 0
    private var inFlight = false
    private struct Cache { let completedAt: Date; let usage: SubscriptionUsage }
    private var cache: [SubscriptionSource: Cache] = [:]
    private var source: SubscriptionSource?
    var onChange: ((SubscriptionUsage?, Bool) -> Void)?

    init(reader: SubscriptionReading = CodexSubscriptionReader(), clock: @escaping () -> Date = Date.init) {
        self.reader = reader
        self.clock = clock
    }

    func refresh(codexHome: String, force: Bool = false) {
        refresh(source: SubscriptionSource(codexHome: codexHome), force: force)
    }

    func refresh(source: SubscriptionSource, force: Bool = false) {
        let changedSource = self.source != source
        if changedSource {
            stop()
            self.source = source
        }
        guard !inFlight else { return }
        if !force, let cached = cache[source] {
            let age = clock().timeIntervalSince(cached.completedAt)
            if age >= 0 && age < 300 {
                // Restore the result immediately even when reopening the same
                // source. No CLI or loading state is needed for a cache hit.
                onChange?(cached.usage, false)
                return
            }
        }
        if changedSource { onChange?(cache[source]?.usage ?? SubscriptionUsage(), false) }
        inFlight = true
        generation &+= 1
        let request = generation
        onChange?(nil, true)
        reader.read(source: source) { [weak self] result in
            guard let self, request == self.generation else { return }
            self.inFlight = false
            // A slow request must not consume any of the result's five-minute
            // lifetime. Cache only completed reads, independently per source.
            self.cache[source] = Cache(completedAt: self.clock(), usage: result)
            // Replace the entire snapshot, including on failure. This avoids
            // displaying another account's cached data after a login change.
            self.onChange?(result, false)
        }
    }

    func stop() {
        generation &+= 1
        reader.cancel()
        // Cancellation creates no cache entry. A previous completed result
        // remains usable for the rest of its own five-minute lifetime.
        inFlight = false
    }

    func cachedUsage(for source: SubscriptionSource) -> SubscriptionUsage? {
        cache[source]?.usage
    }

    func retainSources(_ sources: Set<SubscriptionSource>) {
        if let source, !sources.contains(source) { stop(); self.source = nil }
        cache = cache.filter { sources.contains($0.key) }
    }
}
