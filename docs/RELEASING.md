# Release process

Local builds default to version `100.0` in the app and `acomp`, including `cargo build --release`. They never check for Companion releases, display cached update notices or enable Windows one-click updates. To build a distributable using the workspace version and enable release checks and updates, set `AGENT_COMPANION_RELEASE_BUILD=1` for the build and packaging commands. CI and Release workflows set this explicitly. The workspace version remains the published release version.

Published Windows x64 builds offer **一键更新** in the main task panel after detecting a newer stable release. Only a click downloads the matching official ZIP and `SHA256SUMS.txt`; the updater verifies the archive and stages its package files before restarting the current installation. The extracted installation directory must be writable. Download and preparation errors remain in the panel; **错误详情** opens their complete scrollable explanation, and **重试更新** retries. Settings keeps its read-only check and GitHub Release link, and macOS keeps its release-page update behavior. Codex and Dodex TUI installations remain separately updated.

Reported replacement or startup failures trigger rollback. A hard crash or power loss is not repaired automatically; the transaction journal, backups and copied helper are retained for manual recovery. Successful updates may also retain the helper staging directory and backups of old files that are still in use.

Before publishing an updater change, exercise success and recovery in a disposable extracted-installation directory. Check the restarted version and package hashes, verify a failed replacement or startup restores a runnable prior version, and confirm another Companion process is never shut down by an updater restart. Record these checks separately from synthetic controller/render tests and manual desktop acceptance.

1. Set the workspace version in `Cargo.toml` / `Cargo.lock`, update the bilingual README and add `docs/releases/v<version>.md`. Record completed and unverified manual scenarios in the platform documentation.
2. Require successful Windows MSVC and macOS arm64 CI on the final PR revision. Merge it, then tag the merged revision as `v<version>`; never overwrite a published tag.
3. The Release workflow checks the tag against the workspace version, builds, runs Clippy and tests, packages both platforms, creates provenance attestations and uploads a **draft** release. Native macOS layout/bridge tests run in this workflow too.
4. Download the draft assets into a fresh directory. Confirm both ZIPs and `SHA256SUMS.txt` are present and all SHA-256 digests match. Check ZIP contents, executable architectures and versions. Both platforms must include the Companion GUI, `acomp` console and native `dodex` console entry; they must not include a Dodex desktop launcher. On macOS, extract with `ditto` and run `codesign --verify --strict 'Agent Companion.app'`; check the minimum OS in both the Mach-O and `Info.plist`. Use an isolated `CODEX_HOME` for configuration acceptance checks. Run `scripts/test-tui-instances.py` against the packaged console to verify real installation, independent updating and two native daemons.
5. Verify each archive's attestation against the repository, tag, merged commit and release workflow, requiring GitHub-hosted runners. For example, replace the version, platform and commit below with the release being checked:

```sh
gh attestation verify agent-companion-v0.3.0-macos-arm64.zip \
  --repo WXGopher/agent-companion \
  --source-ref refs/tags/v0.3.0 \
  --source-digest <tag-commit> \
  --signer-workflow WXGopher/agent-companion/.github/workflows/release.yml \
  --deny-self-hosted-runners
```

6. Publish only after the download checks pass, using `gh release edit v<version> --draft=false --latest`. Verify the public release and asset links, and record the build run and source revision in the release notes.

The macOS bundle uses an ad-hoc integrity signature. It does not claim Developer ID signing or Apple notarization. Windows desktop checks require a Windows environment; successful compilation and automated tests are reported separately from manual GUI verification.
