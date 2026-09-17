// SPDX-License-Identifier: GPL-3.0-only
import SwiftUI

struct SubscriptionUsageView: View {
    let usage: SubscriptionUsage
    let loading: Bool
    let scale: CGFloat
    let refresh: () -> Void
    private func s(_ value: CGFloat) -> CGFloat { value * scale }

    var body: some View {
        VStack(alignment: .leading, spacing: s(10)) {
            HStack {
                Text(loading ? "Refreshing…" : updatedText)
                    .font(.system(size: s(10))).foregroundStyle(.secondary).lineLimit(1)
                Spacer(minLength: s(4))
                Button(action: refresh) {
                    Image(systemName: "arrow.clockwise").frame(width: s(26), height: s(24))
                }
                .buttonStyle(.plain).disabled(loading).help("Refresh subscription usage")
                .accessibilityLabel("Refresh subscription usage").accessibilityIdentifier("refresh-subscription-usage")
            }
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        Color.clear.frame(height: 0).id("subscription-top")
                        content
                    }
                }
                .scrollIndicators(.visible)
                .onChange(of: usage.readAt != nil) { _, hasReading in
                    // Anchor the first asynchronous result; subsequent refreshes
                    // retain the user's place in the same scroll view.
                    if hasReading { proxy.scrollTo("subscription-top", anchor: .top) }
                }
            }
            // Loading, errors and successful refreshes share one viewport.
            // Values can change without stretching the panel beneath the cursor.
            .frame(height: s(330))
        }
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: s(14)) {
            if let error = usage.error {
                guidance(error, symbol: "person.crop.circle.badge.exclamationmark")
            } else if usage.readAt == nil {
                guidance(loading ? "Reading your Codex subscription…" : "Refresh to read your Codex subscription.", symbol: "chart.bar.xaxis")
            } else {
                limits
                tokens
                Text("Codex account activity may be delayed. Token totals do not measure remaining allowance.")
                    .font(.system(size: s(10))).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.trailing, s(2))
    }

    private var updatedText: String {
        guard let date = usage.readAt else { return "Subscription account" }
        return "Updated \(date.formatted(date: .omitted, time: .shortened))"
    }

    private var limits: some View {
        VStack(alignment: .leading, spacing: s(8)) {
            sectionTitle("SUBSCRIPTION LIMITS")
            if let error = usage.limitsError { guidance(error, symbol: "wifi.exclamationmark") }
            if let limits = usage.limits {
                ForEach(limits.buckets, id: \.id) { bucket in
                    VStack(alignment: .leading, spacing: s(10)) {
                        Text(bucketTitle(bucket.id, bucket.value))
                            .font(.system(size: s(11), weight: .medium)).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        if let primary = bucket.value.primary { quota(primary, fallback: "Primary") }
                        if let secondary = bucket.value.secondary { quota(secondary, fallback: "Secondary") }
                        if bucket.value.primary == nil && bucket.value.secondary == nil {
                            Text("No allowance window reported.").font(.system(size: s(11))).foregroundStyle(.secondary)
                        }
                    }
                    .padding(s(11)).background(.white.opacity(0.055), in: RoundedRectangle(cornerRadius: s(12)))
                }
            }
        }
    }

    private func bucketTitle(_ id: String, _ bucket: SubscriptionLimits.Bucket) -> String {
        let name = bucket.limitName ?? (id == "codex" ? "Codex" : id)
        guard let plan = bucket.planType, plan != "unknown" else { return name }
        return name + " · " + plan.replacingOccurrences(of: "_", with: " ").capitalized
    }

    private func quota(_ window: SubscriptionLimits.Window, fallback: String) -> some View {
        let remaining = window.remaining()
        return VStack(alignment: .leading, spacing: s(5)) {
            HStack(spacing: s(5)) {
                Text(window.title(fallback: fallback)).foregroundStyle(.secondary)
                Spacer(minLength: 0)
                Text(remaining.map { "\($0)% left" } ?? "—").monospacedDigit()
                    .foregroundStyle(remaining.map { $0 <= 10 } == true ? .orange : CompanionModel.accent)
            }
            .font(.system(size: s(11), weight: .medium))
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    Capsule().fill(.white.opacity(0.10))
                    Capsule().fill(remaining.map { $0 <= 10 } == true ? .orange : CompanionModel.accent)
                        .frame(width: geometry.size.width * CGFloat(remaining ?? 0) / 100)
                }
            }.frame(height: s(3)).accessibilityHidden(true)
            if remaining == nil {
                Text("Reset passed · refresh for a new reading")
                    .font(.system(size: s(9))).foregroundStyle(.secondary)
            } else if let resets = window.resetsAt {
                Text("Resets \(Date(timeIntervalSince1970: resets).formatted(date: .abbreviated, time: .shortened))")
                    .font(.system(size: s(9))).foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
    }

    private var tokens: some View {
        VStack(alignment: .leading, spacing: s(8)) {
            sectionTitle("TOKEN ACTIVITY")
            if let error = usage.tokenError { guidance(error, symbol: "exclamationmark.circle") }
            if let tokens = usage.tokens {
                HStack(spacing: s(7)) {
                    metric("All time", value: tokens.summary.lifetimeTokens)
                    metric("Daily peak", value: tokens.summary.peakDailyTokens)
                }
                if let latest = tokens.recentDays.first {
                    VStack(alignment: .leading, spacing: s(8)) {
                        HStack {
                            Text("Latest day").foregroundStyle(.secondary)
                            Spacer(minLength: s(2))
                            Text(latest.startDate).foregroundStyle(.secondary)
                        }.font(.system(size: s(10)))
                        Text(UsageNumber.short(latest.tokens) + " tokens")
                            .font(.system(size: s(18), weight: .semibold)).monospacedDigit()
                            .help(UsageNumber.full(latest.tokens) + " tokens")
                            .accessibilityLabel(UsageNumber.full(latest.tokens) + " tokens on " + latest.startDate)
                        dailyChart(tokens.recentDays)
                        Text("Last \(tokens.recentDays.count) reported days")
                            .font(.system(size: s(9))).foregroundStyle(.secondary)
                    }
                    .padding(s(11)).background(.white.opacity(0.055), in: RoundedRectangle(cornerRadius: s(12)))
                } else {
                    Text("Daily history is not available.").font(.system(size: s(10))).foregroundStyle(.secondary)
                }
                VStack(spacing: s(8)) {
                    detail("Current streak", value: dayCount(tokens.summary.currentStreakDays))
                    detail("Longest streak", value: dayCount(tokens.summary.longestStreakDays))
                    detail("Longest turn", value: UsageNumber.duration(tokens.summary.longestRunningTurnSec))
                }
                .padding(s(11)).background(.white.opacity(0.055), in: RoundedRectangle(cornerRadius: s(12)))
                Text("— = not reported. Account input/output breakdown unavailable.")
                    .font(.system(size: s(9))).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func sectionTitle(_ title: String) -> some View {
        Text(title).font(.system(size: s(9), weight: .semibold)).tracking(s(0.7)).foregroundStyle(.secondary)
    }

    private func metric(_ title: String, value: Int64?) -> some View {
        VStack(alignment: .leading, spacing: s(6)) {
            Text(title).font(.system(size: s(10))).foregroundStyle(.secondary)
            Text(UsageNumber.short(value)).font(.system(size: s(18), weight: .semibold))
                .monospacedDigit().lineLimit(1).minimumScaleFactor(0.7)
                .foregroundStyle(CompanionModel.accent)
        }
        .frame(maxWidth: .infinity, alignment: .leading).padding(s(10))
        .background(CompanionModel.accent.opacity(0.08), in: RoundedRectangle(cornerRadius: s(11)))
        .help(title + ": " + UsageNumber.full(value) + " tokens")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title + ": " + UsageNumber.full(value) + " tokens")
    }

    private func dailyChart(_ days: [SubscriptionTokens.Day]) -> some View {
        let peak = Double(max(1, days.map(\.tokens).max() ?? 1))
        return HStack(alignment: .bottom, spacing: s(5)) {
            ForEach(days.reversed()) { day in
                VStack(spacing: s(4)) {
                    ZStack(alignment: .bottom) {
                        RoundedRectangle(cornerRadius: s(2)).fill(.white.opacity(0.05))
                        RoundedRectangle(cornerRadius: s(2)).fill(CompanionModel.accent.opacity(0.75))
                            .frame(height: s(38) * CGFloat(Double(day.tokens) / peak))
                    }.frame(height: s(38))
                    Text(String(day.startDate.suffix(2))).font(.system(size: s(8))).foregroundStyle(.secondary)
                }
                .help(day.startDate + ": " + UsageNumber.full(day.tokens) + " tokens")
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(day.startDate + ": " + UsageNumber.full(day.tokens) + " tokens")
            }
        }
    }

    private func dayCount(_ value: Int64?) -> String {
        guard let value, value >= 0 else { return "—" }
        return UsageNumber.full(value) + (value == 1 ? " day" : " days")
    }

    private func detail(_ title: String, value: String) -> some View {
        HStack(spacing: s(6)) {
            Text(title).foregroundStyle(.secondary)
            Spacer(minLength: 0)
            Text(value).monospacedDigit().multilineTextAlignment(.trailing)
        }.font(.system(size: s(10))).accessibilityElement(children: .combine)
    }

    private func guidance(_ text: String, symbol: String) -> some View {
        VStack(alignment: .leading, spacing: s(10)) {
            Image(systemName: symbol).font(.system(size: s(20))).foregroundStyle(CompanionModel.accent)
            Text(text).font(.system(size: s(11))).fixedSize(horizontal: false, vertical: true)
        }
        .padding(s(12)).frame(maxWidth: .infinity, alignment: .leading)
        .background(.white.opacity(0.055), in: RoundedRectangle(cornerRadius: s(12)))
    }
}
