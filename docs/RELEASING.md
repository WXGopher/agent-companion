# Release process

1. Set the workspace version in `Cargo.toml` / `Cargo.lock`, update the bilingual README and add `docs/releases/v<version>.md`. Record completed and unverified manual scenarios in the platform documentation.
2. Require successful Windows MSVC and macOS arm64 CI on the final PR revision. Merge it, then tag the merged revision as `v<version>`; never overwrite a published tag.
3. The Release workflow checks the tag against the workspace version, builds, runs Clippy and tests, packages both platforms, creates provenance attestations and uploads a **draft** release. Native macOS layout/bridge tests run in this workflow too.
4. Download the draft assets into a fresh directory. Confirm both ZIPs and `SHA256SUMS.txt` are present and all SHA-256 digests match. Check ZIP contents, executable architectures and versions. On macOS, extract with `ditto` and run `codesign --verify --strict 'Agent Companion.app'`; check the minimum OS in both the Mach-O and `Info.plist`. Use an isolated `CODEX_HOME` for configuration acceptance checks.
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
