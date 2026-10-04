// SPDX-License-Identifier: GPL-3.0-only
import Foundation

enum LoginItemTests {
    static func run() {
        var current = CompanionLoginItemState.notRegistered
        var registrations = 0
        var removals = 0
        var registrationError: NSError?
        var removalError: NSError?
        var registrationResult = CompanionLoginItemState.enabled
        let service = CompanionLoginItem(
            isAppBundle: { true }, readState: { current },
            register: {
                registrations += 1
                if let error = registrationError { throw error }
                current = registrationResult
            },
            unregister: {
                removals += 1
                if let error = removalError { throw error }
                current = .notRegistered
            })

        precondition(!service.snapshot().enabled)
        precondition(registrations == 0 && removals == 0, "Reading settings must not mutate login items")
        let enabled = service.setEnabled(true)
        precondition(enabled.enabled && !enabled.error && registrations == 1)
        precondition(service.setEnabled(true).enabled && registrations == 1, "Already-enabled items must not be registered again")
        let disabled = service.setEnabled(false)
        precondition(!disabled.enabled && !disabled.error && removals == 1)
        _ = service.setEnabled(false)
        precondition(removals == 1, "Disabling an unregistered item should be a no-op")

        registrationError = NSError(domain: "SMAppServiceErrorDomain", code: 1,
                                    userInfo: [NSLocalizedDescriptionKey: "Operation not permitted"])
        let denied = service.setEnabled(true)
        precondition(!denied.enabled && denied.error && denied.message.contains("Operation not permitted"))
        precondition(service.snapshot().error, "Polling must preserve the last operation error")
        // System Settings can grant/revoke consent while our settings stay open.
        current = .enabled
        precondition(service.snapshot().enabled && !service.snapshot().error)
        current = .requiresApproval
        let pending = service.snapshot()
        precondition(!pending.enabled && pending.requiresApproval && !pending.error)
        let previousRegistrations = registrations
        _ = service.setEnabled(true)
        precondition(registrations == previousRegistrations, "Revoked consent must not trigger re-registration")
        let cancelled = service.setEnabled(false)
        precondition(!cancelled.enabled && !cancelled.requiresApproval && removals == 2)

        registrationError = nil
        registrationResult = .requiresApproval
        let registeredPending = service.setEnabled(true)
        precondition(!registeredPending.enabled && registeredPending.requiresApproval && !registeredPending.error,
                     "Successful registration still needs actual OS approval")
        current = .enabled
        removalError = NSError(domain: "SMAppServiceErrorDomain", code: 5)
        let failedRemoval = service.setEnabled(false)
        precondition(failedRemoval.enabled && failedRemoval.error,
                     "Failed disabling must retain the actual enabled state")

        current = .notRegistered
        registrationResult = .notRegistered
        let unconfirmed = service.setEnabled(true)
        precondition(!unconfirmed.enabled && unconfirmed.error,
                     "A successful API return without the requested state must not claim success")
        current = .notFound
        let fresh = service.snapshot()
        precondition(!fresh.enabled && fresh.available && !fresh.error,
                     "A freshly installed app can have no known login item and still register normally")
        let previousRemovals = removals
        _ = service.setEnabled(false)
        precondition(removals == previousRemovals, "An absent login item needs no removal")
        registrationResult = .enabled
        let firstRegistration = service.setEnabled(true)
        precondition(firstRegistration.enabled && !firstRegistration.error)
        current = .unknown
        precondition(!service.snapshot().enabled && !service.snapshot().available && service.snapshot().error)

        let unbundled = CompanionLoginItem(
            isAppBundle: { false },
            readState: { preconditionFailure("An unbundled executable must not read mainApp") },
            register: { preconditionFailure("An unbundled executable must not register") },
            unregister: { preconditionFailure("An unbundled executable must not unregister") })
        for snapshot in [unbundled.snapshot(), unbundled.setEnabled(true), unbundled.setEnabled(false)] {
            precondition(!snapshot.available && !snapshot.enabled)
            precondition(snapshot.message.contains("Agent Companion.app"))
        }

        // This test executable has no app bundle, exercising the actual native
        // FFI serialization and paired free without reading real OS registration.
        let pointer = agentCompanionLoginItemSnapshotJSON()!
        let data = Data(String(cString: pointer).utf8)
        agentCompanionReleaseLoginItemJSON(pointer)
        let json = try! JSONSerialization.jsonObject(with: data) as! [String: Any]
        precondition(json["enabled"] as? Bool == false && json["available"] as? Bool == false)
        precondition(json["requiresApproval"] as? Bool == false)
        print("Login items: OS state, idempotence, approval/revocation, errors, cancellation, bundle guard and native JSON passed")
    }
}
