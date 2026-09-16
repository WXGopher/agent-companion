#!/bin/sh
# Compile and render the real SwiftUI dashboard using synthetic fixtures.
set -eu
cd "$(dirname "$0")/.."
mkdir -p target/native-ui-tests
xcrun swiftc -swift-version 5 -target arm64-apple-macosx14.0 \
  -sdk "$(xcrun --sdk macosx --show-sdk-path)" \
  crates/agent-companion/macos/*.swift \
  crates/agent-companion/macos/tests/LayoutTests.swift \
  -o target/native-ui-tests/layout-tests
target/native-ui-tests/layout-tests target/native-ui-tests/renders
