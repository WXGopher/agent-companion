// SPDX-License-Identifier: GPL-3.0-only
import SwiftUI

struct SubscriptionUsageView: View {
    @Environment(\.companionTheme) private var theme
    private var palette: CompanionPalette { theme.palette }
    let usage: SubscriptionUsage
    let loading: Bool
    let scale: CGFloat
    let refresh: () -> Void
    var instances: [CodexInstance] = []
    var selectedInstanceID = "codex"
    var selectInstance: (String) -> Void = { _ in }
    private func s(_ value: CGFloat) -> CGFloat { (value * scale).rounded() }
    private func f(_ value: CGFloat) -> CGFloat { max(9, value * scale) }

    var body: some View {
        VStack(alignment: .leading, spacing: s(10)) {
            if instances.count > 1 {
                HStack(spacing: s(3)) {
                    ForEach(instances) { instance in
                        Button { selectInstance(instance.id) } label: {
                            Text(instance.label).font(.system(size: f(11), weight: .medium))
                                .foregroundStyle(instance.id == selectedInstanceID ? palette.accent : palette.secondaryText)
                                .frame(maxWidth: .infinity, minHeight: s(25))
                                .contentShape(Rectangle())
                                .background(instance.id == selectedInstanceID ? palette.selectedControl : palette.inactiveControl,
                                            in: RoundedRectangle(cornerRadius: s(6)))
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("\(instance.label) subscription usage")
                        .accessibilityAddTraits(instance.id == selectedInstanceID ? .isSelected : [])
                    }
                }
                .accessibilityIdentifier("usage-instance-picker")
            }
            HStack {
                Text(loading ? "Refreshing…" : updatedText)
                    .font(.system(size: f(10))).foregroundStyle(palette.secondaryText).lineLimit(1)
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
                    .font(.system(size: f(10))).foregroundStyle(palette.secondaryText)
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
                            .font(.system(size: f(11), weight: .medium)).foregroundStyle(palette.secondaryText)
                            .fixedSize(horizontal: false, vertical: true)
                        if let primary = bucket.value.primary { quota(primary, fallback: "Primary") }
                        if let secondary = bucket.value.secondary { quota(secondary, fallback: "Secondary") }
                        if bucket.value.primary == nil && bucket.value.secondary == nil {
                            Text("No allowance window reported.").font(.system(size: f(11))).foregroundStyle(palette.secondaryText)
                        }
                    }
                    .padding(s(11)).background(palette.surface, in: RoundedRectangle(cornerRadius: s(12)))
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
                Text(window.title(fallback: fallback)).foregroundStyle(palette.secondaryText)
                Spacer(minLength: 0)
                Text(remaining.map { "\($0)% left" } ?? "—").monospacedDigit()
                    .foregroundStyle(remaining.map { $0 <= 10 } == true ? palette.warning : palette.accent)
            }
            .font(.system(size: f(11), weight: .medium))
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    Capsule().fill(palette.quotaTrack)
                    Capsule().fill(remaining.map { $0 <= 10 } == true ? palette.warning : palette.accent)
                        .frame(width: geometry.size.width * CGFloat(remaining ?? 0) / 100)
                }
            }.frame(height: s(3)).accessibilityHidden(true)
            if remaining == nil {
                Text("Reset passed · refresh for a new reading")
                    .font(.system(size: f(9))).foregroundStyle(palette.secondaryText)
            } else if let resets = window.resetsAt {
                Text("Resets \(Date(timeIntervalSince1970: resets).formatted(date: .abbreviated, time: .shortened))")
                    .font(.system(size: f(9))).foregroundStyle(palette.secondaryText)
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
                            Text("Latest day").foregroundStyle(palette.secondaryText)
                            Spacer(minLength: s(2))
                            Text(latest.startDate).foregroundStyle(palette.secondaryText)
                        }.font(.system(size: f(10)))
                        Text(UsageNumber.short(latest.tokens) + " tokens")
                            .font(.system(size: f(15), weight: .semibold)).monospacedDigit()
                            .help(UsageNumber.full(latest.tokens) + " tokens")
                            .accessibilityLabel(UsageNumber.full(latest.tokens) + " tokens on " + latest.startDate)
                        dailyChart(tokens.recentDays)
                        Text("Last \(tokens.recentDays.count) reported days")
                            .font(.system(size: f(9))).foregroundStyle(palette.secondaryText)
                    }
                    .padding(s(11)).background(palette.surface, in: RoundedRectangle(cornerRadius: s(12)))
                } else {
                    Text("Daily history is not available.").font(.system(size: f(10))).foregroundStyle(palette.secondaryText)
                }
                VStack(spacing: s(8)) {
                    detail("Current streak", value: dayCount(tokens.summary.currentStreakDays))
                    detail("Longest streak", value: dayCount(tokens.summary.longestStreakDays))
                    detail("Longest turn", value: UsageNumber.duration(tokens.summary.longestRunningTurnSec))
                }
                .padding(s(11)).background(palette.surface, in: RoundedRectangle(cornerRadius: s(12)))
                Text("— = not reported. Account input/output breakdown unavailable.")
                    .font(.system(size: f(9))).foregroundStyle(palette.secondaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func sectionTitle(_ title: String) -> some View {
        Text(title).font(.system(size: f(9), weight: .semibold)).tracking(s(0.7)).foregroundStyle(palette.secondaryText)
    }

    private func metric(_ title: String, value: Int64?) -> some View {
        let amount = UsageNumber.short(value)
        let label = title + ": " + UsageNumber.full(value) + " tokens"
        let valueText = Text(amount).font(.system(size: f(15), weight: .semibold))
            .monospacedDigit().lineLimit(1).minimumScaleFactor(0.7)
            .foregroundStyle(palette.accent)
        return VStack(alignment: .leading, spacing: s(6)) {
            Text(title).font(.system(size: f(10))).foregroundStyle(palette.secondaryText)
            valueText
        }
        .frame(maxWidth: .infinity, alignment: .leading).padding(s(10))
        .background(palette.accentSurface, in: RoundedRectangle(cornerRadius: s(11)))
        .help(label)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
    }

    private func dailyChart(_ days: [SubscriptionTokens.Day]) -> some View {
        let peak = Double(max(1, days.map(\.tokens).max() ?? 1))
        let chronologicalDays = Array(days.reversed())
        return HStack(alignment: .bottom, spacing: s(5)) {
            ForEach(chronologicalDays) { day in
                dailyBar(day, peak: peak)
            }
        }
    }

    // Keep each bar separately type-checked for the macOS 14 CI toolchain.
    private func dailyBar(_ day: SubscriptionTokens.Day, peak: Double) -> some View {
        let height: CGFloat = s(38) * CGFloat(Double(day.tokens) / peak)
        let label = day.startDate + ": " + UsageNumber.full(day.tokens) + " tokens"
        return VStack(spacing: s(4)) {
            ZStack(alignment: .bottom) {
                RoundedRectangle(cornerRadius: s(2)).fill(palette.chartTrack)
                RoundedRectangle(cornerRadius: s(2)).fill(palette.accent.opacity(0.75))
                    .frame(height: height)
            }.frame(height: s(38))
            Text(String(day.startDate.suffix(2))).font(.system(size: f(8))).foregroundStyle(palette.secondaryText)
        }
        .help(label)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
    }

    private func dayCount(_ value: Int64?) -> String {
        guard let value, value >= 0 else { return "—" }
        return UsageNumber.full(value) + (value == 1 ? " day" : " days")
    }

    private func detail(_ title: String, value: String) -> some View {
        HStack(spacing: s(6)) {
            Text(title).foregroundStyle(palette.secondaryText)
            Spacer(minLength: 0)
            Text(value).monospacedDigit().multilineTextAlignment(.trailing)
        }.font(.system(size: f(10))).accessibilityElement(children: .combine)
    }

    private func guidance(_ text: String, symbol: String) -> some View {
        VStack(alignment: .leading, spacing: s(10)) {
            Image(systemName: symbol).font(.system(size: f(20))).foregroundStyle(palette.accent)
            Text(text).font(.system(size: f(11))).fixedSize(horizontal: false, vertical: true)
        }
        .padding(s(12)).frame(maxWidth: .infinity, alignment: .leading)
        .background(palette.surface, in: RoundedRectangle(cornerRadius: s(12)))
    }
}
