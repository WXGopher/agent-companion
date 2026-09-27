// SPDX-License-Identifier: GPL-3.0-only
import Foundation

/// Account-wide Codex activity returned by account/usage/read. Optional values
/// mean unavailable; they must not become zero or be inferred from quota %.
struct SubscriptionTokens: Codable, Equatable {
    struct Summary: Codable, Equatable {
        var lifetimeTokens: Int64?
        var peakDailyTokens: Int64?
        var longestRunningTurnSec: Int64?
        var currentStreakDays: Int64?
        var longestStreakDays: Int64?
    }
    struct Day: Codable, Equatable, Identifiable {
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

struct SubscriptionLimits: Codable, Equatable {
    struct Window: Codable, Equatable {
        let usedPercent: Double
        let windowDurationMins: Int?
        let resetsAt: TimeInterval?

        func remaining(at now: Date = Date()) -> Int? {
            return 100 - Int(min(100, max(0, usedPercent)).rounded())
        }
        func title(fallback: String) -> String {
            guard let minutes = windowDurationMins, minutes > 0 else { return fallback }
            if minutes == 10080 { return "Weekly" }
            if minutes.isMultiple(of: 1440) { return "\(minutes / 1440)-day" }
            if minutes.isMultiple(of: 60) { return "\(minutes / 60)-hour" }
            return "\(minutes)-minute"
        }
    }
    struct Bucket: Codable, Equatable {
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
    var tokensReadAt: Date?
    var tokensLoading = false

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

/// All routing inputs participate in cache identity. A redeployed runtime must
/// never inherit a prior runtime's account reading, even at the same home.
struct SubscriptionSource: Hashable, Codable {
    var instanceID = "codex"
    var codexHome: String
    var executablePath: String?
    var databasePath: String?

    func matches(_ requested: SubscriptionSource) -> Bool {
        func path(_ value: String) -> String {
            value.isEmpty ? value : URL(fileURLWithPath: value).standardizedFileURL.resolvingSymlinksInPath().path
        }
        func samePath(_ left: String, _ right: String) -> Bool { left == right || path(left) == path(right) }
        guard instanceID == requested.instanceID, samePath(codexHome, requested.codexHome),
              samePath(databasePath ?? codexHome, requested.databasePath ?? requested.codexHome) else { return false }
        if requested.instanceID == "codex" && requested.executablePath == nil { return true }
        if executablePath == requested.executablePath { return true }
        return executablePath.map(path) == requested.executablePath.map(path)
    }

    enum CodingKeys: String, CodingKey {
        case instanceID = "instanceId"
        case codexHome, executablePath, databasePath
    }
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
