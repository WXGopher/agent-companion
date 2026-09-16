# Third-party source notices

## Open Island (open-vibe-island)

Copyright Open Island contributors, including Octane0411.
Licensed under the GNU General Public License, version 3 (GPL-3.0-only).
The complete license is included in [LICENSE](LICENSE).

Source: <https://github.com/Octane0411/open-vibe-island>
Pinned revision: `b50f87aa7d58af1478837d48909eb68baa37f9b9`.

Agent Companion's macOS implementation incorporates:

- `crates/agent-companion/macos/NotchShape.swift`: copied from
  `Sources/OpenIslandApp/NotchShape.swift`, with attribution added and unused
  upstream radius defaults removed.
- `crates/agent-companion/macos/NotchController.swift`: adapted window placement,
  nonactivating panel and screen/Space behavior from
  `Sources/OpenIslandApp/OverlayPanelController.swift`; simplified to a single
  compact Codex surface with our own hover, sizing and lifecycle handling.
- `crates/agent-companion/macos/TerminalJump.swift`: adapted Terminal/iTerm tab
  selection and Codex conversation links from
  `Sources/OpenIslandApp/TerminalJumpService.swift`; target discovery is limited
  to local Codex writer processes, and scripts receive data through arguments.

Agent Companion uses its existing Rust session/usage reader and its own SwiftUI
dashboard. It does not bundle Open Island's other agents, pets or plugins.

Other dependencies retain their respective licenses. Rust dependency versions
are recorded in `Cargo.lock`; no third-party Swift packages are bundled.
