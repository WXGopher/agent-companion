//! Exercise the Windows Settings markup, controller and pointer interactions on macOS.
//! The Fluent style matches the Windows build instead of inheriting Cupertino.
#![cfg(target_os = "macos")]
#![allow(dead_code, unused_imports)]

#[path = "../src/codex_tui.rs"]
mod codex_tui;
#[path = "../src/macos.rs"]
mod macos;
#[path = "../src/macos_deployment.rs"]
mod macos_deployment;
#[path = "../src/macos_primary_app.rs"]
mod macos_primary_app;
#[path = "../src/managed_tui.rs"]
mod managed_tui;
#[path = "../src/settings_update.rs"]
mod settings_update;
#[path = "../src/software_updates.rs"]
mod software_updates;
#[path = "../src/update_service.rs"]
mod update_service;
#[path = "../src/usage_service.rs"]
mod usage_service;

mod ui {
    slint::slint! {
        #[style = "fluent"]
        import { CodexTuiWindow, StatusComponent, SoftwareVersion } from "../ui/codex-tui.slint";
        export { CodexTuiWindow, StatusComponent, SoftwareVersion }
        export { Palette } from "std-widgets.slint";
    }
}

#[path = "../src/app/panel_render_tests.rs"]
mod panel_render_tests;
