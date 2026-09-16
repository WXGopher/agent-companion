// SPDX-License-Identifier: GPL-3.0-only
import SwiftUI

struct CompanionView: View {
    @ObservedObject var model: CompanionModel
    @State private var hoveredTask: String?

    var body: some View {
        VStack(spacing: 0) {
            compact
            if model.expanded { expanded.transition(.opacity.combined(with: .move(edge: .top))) }
        }
        .frame(width: model.compactWidth)
        .background {
            if model.hasCamera {
                NotchShape(topCornerRadius: 6, bottomCornerRadius: model.expanded ? 24 : 10)
                    .fill(.black)
            } else {
                RoundedRectangle(cornerRadius: model.expanded ? 22 : 14).fill(.black)
            }
        }
        .foregroundStyle(.white)
        .preferredColorScheme(.dark)
        .onExitCommand { model.collapse?() }
    }

    private var compact: some View {
        HStack(spacing: 0) {
            Button { model.expand?() } label: {
                HStack(spacing: 7) {
                    count(model.snapshot.activeCount, symbol: "circle.inset.filled", tint: CompanionModel.accent)
                    count(model.snapshot.completedCount, symbol: "checkmark", tint: .white.opacity(0.7))
                }
                .frame(width: model.sideWidth, alignment: .center)
                .frame(height: model.compactHeight)
                .contentShape(Rectangle())
            }
            .accessibilityLabel("Codex: \(model.snapshot.activeCount) active, \(model.snapshot.completedCount) recently finished")
            if model.hasCamera { Color.clear.frame(width: model.cameraWidth) }
            else { Spacer(minLength: 20) }
            Button { model.expand?() } label: {
                HStack(spacing: 4) {
                    Text(model.weeklyText).monospacedDigit().fontWeight(.semibold).lineLimit(1).fixedSize()
                    Text("wk").font(.system(size: 9)).foregroundStyle(.white.opacity(0.5))
                }
                .frame(width: model.sideWidth)
                .frame(height: model.compactHeight)
                .contentShape(Rectangle())
            }
            .accessibilityLabel("Weekly usage \(model.weeklyText). Open Codex tasks")
        }
        .font(.system(size: 12))
        .padding(.horizontal, 6)
        .frame(width: model.compactWidth, height: model.compactHeight)
        .buttonStyle(.plain)
        .accessibilityHint("Open task list")
        .help("\(model.snapshot.activeCount) active · \(model.snapshot.completedCount) finished in the last 15 minutes · \(model.weeklyText) weekly used")
        .contextMenu {
            Button("Show tasks") { model.expand?() }
            Button("Settings…") { model.openSettings() }
            Divider()
            Button("Quit Agent Companion") { model.quit?() }
        }
    }

    private func count(_ count: Int, symbol: String, tint: Color) -> some View {
        HStack(spacing: 3) {
            Image(systemName: symbol).font(.system(size: 8, weight: .semibold)).foregroundStyle(tint)
            Text(model.countText(count)).monospacedDigit().fontWeight(.semibold).lineLimit(1).fixedSize()
        }
    }

    private var expanded: some View {
        VStack(alignment: .leading, spacing: 13) {
            HStack(alignment: .center) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("Codex").font(.system(size: 20, weight: .semibold))
                    Text("Your tasks, at a glance").font(.system(size: 11)).foregroundStyle(.secondary)
                }
                Spacer()
                Button { model.collapse?() } label: { Image(systemName: "chevron.up").frame(width: 26, height: 26) }
                    .buttonStyle(.plain).foregroundStyle(.secondary).help("Collapse (Esc)").accessibilityLabel("Collapse task list")
            }
            weekly
            HStack(spacing: 8) {
                tab("Active", count: model.snapshot.activeCount, completed: false)
                tab("Finished", count: model.snapshot.completedCount, completed: true)
                Spacer()
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
                    LazyVStack(spacing: 4) {
                        ForEach(model.visibleTasks) { task in taskRow(task) }
                    }
                }
                .frame(height: min(CGFloat(model.visibleTasks.count) * 58, 232))
                .scrollIndicators(.visible)
            }
            if let message = model.message {
                VStack(alignment: .leading, spacing: 7) {
                    HStack(alignment: .top) {
                        Text(message).font(.system(size: 11)).fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: 4)
                        Button { model.message = nil; model.failedTask = nil } label: { Image(systemName: "xmark") }
                            .buttonStyle(.plain).accessibilityLabel("Dismiss message")
                    }
                    if let task = model.failedTask {
                        ViewThatFits(in: .horizontal) {
                            HStack { recoveryButtons(task) }
                            VStack(alignment: .leading, spacing: 6) { recoveryButtons(task) }
                        }
                        .buttonStyle(.bordered).controlSize(.small)
                    }
                }
                .padding(10).background(.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
            }
            Divider().overlay(.white.opacity(0.06))
            HStack {
                Button { model.openSettings() } label: { Label("Settings", systemImage: "gearshape") }
                    .help("Customize the Codex CLI status bar…")
                Spacer()
                Button("Open Codex") { model.openCodex() }
                Button { model.quit?() } label: { Image(systemName: "power") }
                    .help("Quit Agent Companion").accessibilityLabel("Quit Agent Companion")
            }
            .buttonStyle(.plain).font(.system(size: 11)).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 16).padding(.top, 12).padding(.bottom, 16)
    }

    private var weekly: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack {
                Text("Weekly usage").font(.system(size: 11)).foregroundStyle(.secondary)
                Spacer()
                Text(model.weeklyText + (model.weeklyText == "—" ? "" : " used"))
                    .font(.system(size: 12, weight: .medium)).monospacedDigit()
            }
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    Capsule().fill(.white.opacity(0.10))
                    if let usage = model.snapshot.weekly, !usage.expired {
                        Capsule().fill(usage.usedPercent >= 90 ? Color.orange : CompanionModel.accent)
                            .frame(width: geometry.size.width * CGFloat(usage.usedPercent) / 100)
                    }
                }
            }.frame(height: 3).accessibilityHidden(true)
            if let usage = model.snapshot.weekly, !usage.expired, let resets = usage.resetsAt {
                Text("Resets \(Date(timeIntervalSince1970: resets).formatted(date: .abbreviated, time: .shortened))")
                    .font(.system(size: 10)).foregroundStyle(.secondary)
            } else {
                Text(model.snapshot.weekly?.expired == true ? "Waiting for a new Codex usage reading after reset." : "Usage appears after Codex records a rate limit reading.")
                    .font(.system(size: 10)).foregroundStyle(.secondary)
            }
        }
        .padding(12).background(.white.opacity(0.055), in: RoundedRectangle(cornerRadius: 12))
    }

    private func tab(_ title: String, count: Int, completed: Bool) -> some View {
        Button { model.showingCompleted = completed } label: {
            HStack(spacing: 5) {
                Text(title)
                Text("\(count)").monospacedDigit().foregroundStyle(model.showingCompleted == completed ? CompanionModel.accent : .white.opacity(0.55))
            }
            .font(.system(size: 11, weight: .medium)).padding(.horizontal, 11).padding(.vertical, 7)
            .background(.white.opacity(model.showingCompleted == completed ? 0.11 : 0.035), in: Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(title), \(count) tasks")
        .accessibilityAddTraits(model.showingCompleted == completed ? .isSelected : [])
        .keyboardShortcut(completed ? "2" : "1", modifiers: .command)
    }

    private func taskRow(_ task: CodexTask) -> some View {
        Button { model.jump(to: task) } label: {
            HStack(spacing: 10) {
                Image(systemName: task.symbol).foregroundStyle(task.tint).font(.system(size: 13)).frame(width: 18)
                VStack(alignment: .leading, spacing: 5) {
                    Text(task.title).font(.system(size: 12, weight: .medium)).lineLimit(1)
                    HStack(spacing: 5) {
                        if model.compactWidth >= 260 {
                            Text(task.project).lineLimit(1)
                            Text("·")
                        }
                        Text(task.status).lineLimit(1).fixedSize()
                        Spacer(minLength: 4)
                        Text(age(task.updatedAt)).monospacedDigit().fixedSize()
                    }.font(.system(size: 10)).foregroundStyle(.secondary)
                }
                if model.jumpingID == task.id { ProgressView().controlSize(.mini) }
                else { Image(systemName: "arrow.up.right").font(.system(size: 10)).foregroundStyle(.secondary) }
            }
            .padding(.horizontal, 10).frame(height: 54)
            .background(.white.opacity(hoveredTask == task.id ? 0.10 : 0.045), in: RoundedRectangle(cornerRadius: 10))
            .contentShape(RoundedRectangle(cornerRadius: 10))
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
        if task.resumeCommand != nil { Button("Copy resume command") { model.copyResume(task) } }
    }

    private func age(_ timestamp: TimeInterval) -> String {
        let seconds = max(0, Int(Date().timeIntervalSince1970 - timestamp))
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3600 { return "\(seconds / 60)m" }
        return "\(seconds / 3600)h"
    }

    private func empty(_ title: String, detail: String, symbol: String) -> some View {
        VStack(spacing: 9) {
            Image(systemName: symbol).font(.system(size: 22)).foregroundStyle(.white.opacity(0.35))
            Text(title).font(.system(size: 12, weight: .medium))
            Text(detail).font(.system(size: 11)).foregroundStyle(.secondary).multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity).frame(height: 132).padding(.horizontal, 25)
    }

    private func notice(_ text: String, symbol: String) -> some View {
        Label(text, systemImage: symbol).font(.system(size: 11)).foregroundStyle(.orange)
            .fixedSize(horizontal: false, vertical: true)
    }
}
