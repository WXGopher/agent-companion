// SPDX-License-Identifier: GPL-3.0-only
import SwiftUI

struct CompanionView: View {
    @ObservedObject var model: CompanionModel
    var showsDetails: Bool? = nil
    var drawsBackground = true
    var detailsHeight: CGFloat? = nil
    @State private var hoveredTask: String?

    private func scaled(_ value: CGFloat) -> CGFloat { value * model.metrics.contentScale }
    private func compactScaled(_ value: CGFloat) -> CGFloat { value * min(1, model.compactWidth / 179) }

    var body: some View {
        VStack(spacing: 0) {
            compact
            if showsDetails ?? model.expanded {
                expanded
                    .allowsHitTesting(model.expanded)
                    .accessibilityHidden(!model.expanded)
            }
        }
        .frame(width: model.compactWidth)
        .background {
            if drawsBackground {
                NotchOutline(hasCamera: model.hasCamera, expansion: model.expanded ? 1 : 0).fill(.black)
            }
        }
        .frame(maxHeight: .infinity, alignment: .top)
        .foregroundStyle(.white)
        .preferredColorScheme(.dark)
        .onExitCommand { model.collapse?() }
    }

    private var compact: some View {
        Button { model.expand?() } label: {
            VStack(spacing: 0) {
                // Keep the menu-bar band entirely inside the physical camera gap.
                // Counters sit just below it instead of growing wings over menus.
                if model.hasCamera { Color.clear.frame(height: model.metrics.cameraHeight) }
                HStack(spacing: 0) {
                    HStack(spacing: compactScaled(7)) {
                        count(model.snapshot.activeCount, symbol: "circle.inset.filled", tint: CompanionModel.accent)
                        count(model.snapshot.completedCount, symbol: "checkmark", tint: .white.opacity(0.7))
                    }
                    Spacer(minLength: compactScaled(8))
                    HStack(spacing: compactScaled(3)) {
                        Text(model.weeklyText).monospacedDigit().fontWeight(.semibold).lineLimit(1).fixedSize()
                        if model.weeklyRemainingPercent != nil {
                            Text("left").font(.system(size: compactScaled(9))).foregroundStyle(.white.opacity(0.5)).fixedSize()
                        }
                    }
                }
                .frame(height: model.metrics.statsHeight)
                .padding(.horizontal, compactScaled(10))
            }
            .font(.system(size: compactScaled(12)))
            .frame(width: model.compactWidth, height: model.compactHeight)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("notch-summary")
        .accessibilityLabel("Codex: \(model.snapshot.activeCount) active, \(model.snapshot.completedCount) recently finished. Weekly quota remaining: \(model.weeklyText)")
        .accessibilityHint("Open task list")
        .help("\(model.snapshot.activeCount) active · \(model.snapshot.completedCount) finished in the last 15 minutes · Weekly quota remaining: \(model.weeklyText)")
        .contextMenu {
            Button("Show tasks") { model.expand?() }
            Button("Settings…") { model.openSettings() }
            Divider()
            Button("Quit Agent Companion") { model.quit?() }
        }
    }

    private func count(_ count: Int, symbol: String, tint: Color) -> some View {
        HStack(spacing: compactScaled(3)) {
            Image(systemName: symbol).font(.system(size: compactScaled(8), weight: .semibold)).foregroundStyle(tint)
            Text(model.countText(count)).monospacedDigit().fontWeight(.semibold).lineLimit(1).fixedSize()
        }
    }

    private var expanded: some View {
        VStack(alignment: .leading, spacing: scaled(13)) {
            if detailsHeight != nil {
                ScrollView {
                    expandedContent
                }
                .scrollIndicators(.visible)
                .frame(maxHeight: .infinity)
            } else {
                expandedContent
            }
            Divider().overlay(.white.opacity(0.06))
            footer
        }
        .padding(.horizontal, scaled(16)).padding(.top, scaled(12)).padding(.bottom, scaled(16))
        .frame(height: detailsHeight)
    }

    private var expandedContent: some View {
        VStack(alignment: .leading, spacing: scaled(13)) {
            HStack(alignment: .center, spacing: scaled(8)) {
                VStack(alignment: .leading, spacing: scaled(3)) {
                    Text("Codex").font(.system(size: scaled(20), weight: .semibold))
                    Text("Your tasks, at a glance").font(.system(size: scaled(11))).foregroundStyle(.secondary)
                }
                Spacer()
                Button { model.collapse?() } label: { Image(systemName: "chevron.up").frame(width: scaled(26), height: scaled(26)) }
                    .buttonStyle(.plain).foregroundStyle(.secondary).help("Collapse (Esc)").accessibilityLabel("Collapse task list")
            }
            weekly
            HStack(spacing: scaled(8)) {
                tab("Active", count: model.snapshot.activeCount, completed: false)
                tab("Finished", count: model.snapshot.completedCount, completed: true)
                Spacer(minLength: 0)
            }
            if let error = model.snapshot.error { notice(error, symbol: "exclamationmark.triangle") }
            if model.snapshot.loading {
                empty("Reading Codex…", detail: "Checking local sessions and usage.", symbol: "ellipsis")
            } else if model.visibleTasks.isEmpty {
                empty(model.showingCompleted ? "No recently finished tasks" : "No active tasks",
                      detail: model.showingCompleted ? "Finished, stopped and failed tasks stay here for 15 minutes." : "Start a task in Codex.app or Codex CLI. It will appear here automatically.",
                      symbol: model.showingCompleted ? "checkmark.circle" : "terminal")
            } else {
                ScrollView {
                    LazyVStack(spacing: scaled(4)) {
                        ForEach(model.visibleTasks) { task in taskRow(task) }
                    }
                }
                .frame(height: scaled(min(CGFloat(model.visibleTasks.count) * 58, 232)))
                .scrollIndicators(.visible)
            }
            if let message = model.message {
                VStack(alignment: .leading, spacing: scaled(7)) {
                    HStack(alignment: .top, spacing: scaled(8)) {
                        Text(message).font(.system(size: scaled(11))).fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: scaled(4))
                        Button { model.message = nil; model.failedTask = nil } label: { Image(systemName: "xmark") }
                            .buttonStyle(.plain).accessibilityLabel("Dismiss message")
                    }
                    if let task = model.failedTask {
                        ViewThatFits(in: .horizontal) {
                            HStack(spacing: scaled(8)) { recoveryButtons(task) }
                            VStack(alignment: .leading, spacing: scaled(6)) { recoveryButtons(task) }
                        }
                        .buttonStyle(.bordered).controlSize(.small).font(.system(size: scaled(11)))
                    }
                }
                .padding(scaled(10)).background(.white.opacity(0.08), in: RoundedRectangle(cornerRadius: scaled(10)))
            }
        }
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: scaled(10)) {
            HStack {
                Button { model.openSettings() } label: { Label("Settings", systemImage: "gearshape").fixedSize() }
                    .help("Customize the Codex CLI status bar…")
                Spacer(minLength: 0)
                Button { model.quit?() } label: { Image(systemName: "power") }
                    .help("Quit Agent Companion").accessibilityLabel("Quit Agent Companion")
            }
            Button { model.openCodex() } label: { Label("Open Codex", systemImage: "arrow.up.forward.app").fixedSize() }
        }
        .buttonStyle(.plain).font(.system(size: scaled(11))).foregroundStyle(.secondary)
    }

    private var weekly: some View {
        VStack(alignment: .leading, spacing: scaled(7)) {
            HStack(spacing: scaled(8)) {
                Text("Weekly quota").font(.system(size: scaled(11))).foregroundStyle(.secondary)
                Spacer()
                Text(model.weeklyText + (model.weeklyRemainingPercent == nil ? "" : " left"))
                    .font(.system(size: scaled(12), weight: .medium)).monospacedDigit()
            }
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    Capsule().fill(.white.opacity(0.10))
                    if let remaining = model.weeklyRemainingPercent {
                        Capsule().fill(remaining <= 10 ? Color.orange : CompanionModel.accent)
                            .frame(width: geometry.size.width * CGFloat(remaining) / 100)
                    }
                }
            }.frame(height: scaled(3)).accessibilityHidden(true)
            if let usage = model.snapshot.weekly, !usage.expired, let resets = usage.resetsAt {
                Text("Resets \(Date(timeIntervalSince1970: resets).formatted(date: .abbreviated, time: .shortened))")
                    .font(.system(size: scaled(10))).foregroundStyle(.secondary)
            } else {
                Text(model.snapshot.weekly?.expired == true ? "Waiting for a new Codex usage reading after reset." : "Usage appears after Codex records a rate limit reading.")
                    .font(.system(size: scaled(10))).foregroundStyle(.secondary)
            }
        }
        .padding(scaled(12)).background(.white.opacity(0.055), in: RoundedRectangle(cornerRadius: scaled(12)))
    }

    private func tab(_ title: String, count: Int, completed: Bool) -> some View {
        Button { model.showingCompleted = completed } label: {
            HStack(spacing: scaled(5)) {
                Text(title)
                Text(model.countText(count)).monospacedDigit().foregroundStyle(model.showingCompleted == completed ? CompanionModel.accent : .white.opacity(0.55))
            }
            .font(.system(size: scaled(11), weight: .medium)).fixedSize()
            .padding(.horizontal, scaled(8)).padding(.vertical, scaled(7))
            .background(.white.opacity(model.showingCompleted == completed ? 0.11 : 0.035), in: Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(title), \(count) tasks")
        .accessibilityAddTraits(model.showingCompleted == completed ? .isSelected : [])
        .keyboardShortcut(completed ? "2" : "1", modifiers: .command)
    }

    private func taskRow(_ task: CodexTask) -> some View {
        Button { model.jump(to: task) } label: {
            HStack(spacing: scaled(10)) {
                Image(systemName: task.symbol).foregroundStyle(task.tint).font(.system(size: scaled(13))).frame(width: scaled(18))
                VStack(alignment: .leading, spacing: scaled(5)) {
                    Text(task.title).font(.system(size: scaled(12), weight: .medium)).lineLimit(1)
                    HStack(spacing: scaled(5)) {
                        if model.compactWidth >= 260 {
                            Text(task.project).lineLimit(1)
                            Text("·")
                        }
                        Text(task.status).lineLimit(1).fixedSize()
                        Spacer(minLength: scaled(4))
                        Text(age(task.updatedAt)).monospacedDigit().fixedSize()
                    }.font(.system(size: scaled(10))).foregroundStyle(.secondary)
                }
                if model.jumpingID == task.id { ProgressView().controlSize(.mini) }
                else { Image(systemName: "arrow.up.right").font(.system(size: scaled(10))).foregroundStyle(.secondary) }
            }
            .padding(.horizontal, scaled(10)).frame(height: scaled(54))
            .background(.white.opacity(hoveredTask == task.id ? 0.10 : 0.045), in: RoundedRectangle(cornerRadius: scaled(10)))
            .contentShape(RoundedRectangle(cornerRadius: scaled(10)))
        }
        .buttonStyle(.plain)
        .disabled(model.jumpingID != nil)
        .onHover { inside in
            if inside { hoveredTask = task.id }
            else if hoveredTask == task.id { hoveredTask = nil }
        }
        .help("\(task.title)\n\(task.cwd ?? task.project)\nOpen this \(task.client == "desktop" ? "Codex conversation" : "session")")
        .accessibilityLabel("\(task.title), \(task.status), \(task.project). Open session")
        .contextMenu {
            Button("Open session") { model.jump(to: task) }
            if task.resumeCommand != nil { Button("Copy resume command") { model.copyResume(task) } }
        }
    }

    @ViewBuilder private func recoveryButtons(_ task: CodexTask) -> some View {
        Button("Try again") { model.jump(to: task) }
        if task.resumeCommand != nil {
            Button("Copy command") { model.copyResume(task) }
                .accessibilityLabel("Copy resume command")
                .help("Copy the command to resume this Codex session")
        }
    }

    private func age(_ timestamp: TimeInterval) -> String {
        let seconds = max(0, Int(Date().timeIntervalSince1970 - timestamp))
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3600 { return "\(seconds / 60)m" }
        return "\(seconds / 3600)h"
    }

    private func empty(_ title: String, detail: String, symbol: String) -> some View {
        VStack(spacing: scaled(9)) {
            Image(systemName: symbol).font(.system(size: scaled(22))).foregroundStyle(.white.opacity(0.35))
            Text(title).font(.system(size: scaled(12), weight: .medium))
            Text(detail).font(.system(size: scaled(11))).foregroundStyle(.secondary).multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity).frame(height: scaled(132)).padding(.horizontal, scaled(25))
    }

    private func notice(_ text: String, symbol: String) -> some View {
        Label(text, systemImage: symbol).font(.system(size: scaled(11))).foregroundStyle(.orange)
            .fixedSize(horizontal: false, vertical: true)
    }
}
