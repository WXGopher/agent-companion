//! Run the Windows allowance presentation and navigation tests on other hosts.
//! The real Slint data shape, Rust projection and core scheduler are reused;
//! platform I/O is inert because these tests feed only synthetic snapshots.
#![cfg(not(windows))]
#![allow(dead_code)]

mod ui {
    slint::slint! {
        import { UsageRow } from "../ui/common.slint";
        export component UsageFixture inherits Window {
            in property <[UsageRow]> rows;
        }
    }
}

#[path = "../src/app/subscription.rs"]
mod subscription;
#[path = "../src/usage_cache.rs"]
mod usage_cache;

mod util {
    pub fn home_dir() -> Option<std::path::PathBuf> {
        None
    }
    pub fn debug_log(_: &str) {}
}

mod app {
    pub mod net {
        pub fn get_json(_: &str, _: &str, _: &[(&str, &str)]) -> std::io::Result<String> {
            panic!("presentation tests must not query a network endpoint")
        }
    }
}
