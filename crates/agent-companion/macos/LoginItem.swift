// SPDX-License-Identifier: GPL-3.0-only
import Foundation
import ServiceManagement

enum CompanionLoginItemState {
    case unavailable, notRegistered, enabled, requiresApproval, notFound, unknown
}

struct CompanionLoginItemSnapshot: Encodable {
    let enabled: Bool
    let available: Bool
    let requiresApproval: Bool
    let error: Bool
    let message: String
}

/// Reads macOS every time; no preference file can drift from the login item.
/// Injected operations keep tests away from the user's actual login settings.
final class CompanionLoginItem {
    private let isAppBundle: () -> Bool
    private let readState: () -> CompanionLoginItemState
    private let register: () throws -> Void
    private let unregister: () throws -> Void
    private var failure: (state: CompanionLoginItemState, message: String)?

    init(isAppBundle: @escaping () -> Bool,
         readState: @escaping () -> CompanionLoginItemState,
         register: @escaping () throws -> Void,
         unregister: @escaping () throws -> Void) {
        self.isAppBundle = isAppBundle
        self.readState = readState
        self.register = register
        self.unregister = unregister
    }

    private var state: CompanionLoginItemState {
        isAppBundle() ? readState() : .unavailable
    }

    func snapshot() -> CompanionLoginItemSnapshot {
        let current = state
        if failure?.state != current { failure = nil }
        let message: String
        switch current {
        case .unavailable:
            message = "请从已安装的 Agent Companion.app 打开设置后再修改。"
        case .notRegistered:
            message = "未开启；开启后会在登录此 Mac 时启动菜单栏应用。"
        case .enabled:
            message = "已开启；登录此 Mac 时自动启动 Agent Companion。"
        case .requiresApproval:
            message = "尚未生效；请在系统登录项设置中允许 Agent Companion。"
        case .notFound:
            message = "尚未注册登录项；开启后会在登录此 Mac 时启动菜单栏应用。"
        case .unknown:
            message = "无法识别系统登录项状态，请在系统登录项设置中检查。"
        }
        return CompanionLoginItemSnapshot(
            enabled: current == .enabled,
            available: current != .unavailable && current != .unknown,
            requiresApproval: current == .requiresApproval,
            error: failure != nil || current == .unknown,
            message: failure.map { "\($0.message)\n\(message)" } ?? message)
    }

    func setEnabled(_ enabled: Bool) -> CompanionLoginItemSnapshot {
        failure = nil
        let before = state
        guard before != .unavailable && before != .unknown else { return snapshot() }
        // Avoid re-registering an enabled or denied item. In the latter case,
        // only the user can grant consent in System Settings.
        if enabled && (before == .enabled || before == .requiresApproval) { return snapshot() }
        if !enabled && (before == .notRegistered || before == .notFound) { return snapshot() }
        do {
            if enabled { try register() } else { try unregister() }
            let after = state
            if (enabled && after != .enabled && after != .requiresApproval)
                || (!enabled && after != .notRegistered && after != .notFound) {
                failure = (after, "系统尚未确认更改，请重试或检查系统登录项设置。")
            }
        } catch {
            let error = error as NSError
            failure = (state, "无法\(enabled ? "开启" : "关闭")登录时自动启动：\(error.localizedDescription)（\(error.domain) / \(error.code)）。")
        }
        return snapshot()
    }
}

private let companionLoginItem = CompanionLoginItem(
    isAppBundle: {
        let bundle = Bundle.main
        return bundle.bundleIdentifier == "com.wxgopher.agent-companion"
            && bundle.bundleURL.pathExtension == "app"
            && bundle.executableURL?.lastPathComponent == "agent-companion"
    },
    readState: {
        switch SMAppService.mainApp.status {
        case .notRegistered: return .notRegistered
        case .enabled: return .enabled
        case .requiresApproval: return .requiresApproval
        case .notFound: return .notFound
        @unknown default: return .unknown
        }
    },
    register: { try SMAppService.mainApp.register() },
    unregister: { try SMAppService.mainApp.unregister() })

private func loginItemJSON(_ snapshot: CompanionLoginItemSnapshot) -> UnsafeMutablePointer<CChar>? {
    guard let data = try? JSONEncoder().encode(snapshot),
          let json = String(data: data, encoding: .utf8) else { return nil }
    return strdup(json)
}

@_cdecl("agent_companion_login_item_snapshot_json")
public func agentCompanionLoginItemSnapshotJSON() -> UnsafeMutablePointer<CChar>? {
    precondition(Thread.isMainThread)
    return loginItemJSON(companionLoginItem.snapshot())
}

@_cdecl("agent_companion_set_login_item")
public func agentCompanionSetLoginItem(_ enabled: Bool) -> UnsafeMutablePointer<CChar>? {
    precondition(Thread.isMainThread)
    return loginItemJSON(companionLoginItem.setEnabled(enabled))
}

@_cdecl("agent_companion_release_login_item_json")
public func agentCompanionReleaseLoginItemJSON(_ pointer: UnsafeMutablePointer<CChar>?) {
    free(pointer)
}

@_cdecl("agent_companion_open_login_item_settings")
public func agentCompanionOpenLoginItemSettings() {
    precondition(Thread.isMainThread)
    SMAppService.openSystemSettingsLoginItems()
}
