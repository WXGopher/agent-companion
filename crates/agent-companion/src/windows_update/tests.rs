use super::*;

use serde_json::{Value, json};
use std::io::Cursor;

const TEST_VERSION: &str = "2.4.0";
const TEST_TAG: &str = "v2.4.0";
const TEST_ARCHIVE: &str = "agent-companion-v2.4.0-windows-x86_64.zip";
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn update(version: &str, url: &str) -> crate::update_service::UpdateSnapshot {
    crate::update_service::UpdateSnapshot {
        latest_version: Some(version.into()),
        release_url: Some(url.into()),
    }
}

fn selected() -> Selection {
    Selection {
        tag: TEST_TAG.into(),
        version: TEST_VERSION.into(),
        archive: TEST_ARCHIVE.into(),
    }
}

fn release_asset(name: &str, id: u64, size: u64) -> Value {
    json!({
        "id": id,
        "name": name,
        "size": size,
        "browser_download_url": format!(
            "https://github.com/WXGopher/agent-companion/releases/download/{TEST_TAG}/{name}"
        ),
        "digest": null,
    })
}

fn release() -> Value {
    json!({
        "tag_name": TEST_TAG,
        "draft": false,
        "prerelease": false,
        "assets": [
            release_asset(TEST_ARCHIVE, 101, 4096),
            release_asset("SHA256SUMS.txt", 102, 256),
        ],
    })
}

fn select_release(value: &Value) -> io::Result<(Asset, Asset)> {
    select_assets(&serde_json::to_vec(value).unwrap(), &selected())
}

#[test]
fn selection_binds_the_version_and_package_to_the_official_release() {
    let snapshot = update(TEST_VERSION, &format!("{RELEASE_PREFIX}{TEST_TAG}"));
    let selection = Selection::from_snapshot(&snapshot, "2.3.9", true, true).unwrap();

    assert_eq!(selection.tag, TEST_TAG);
    assert_eq!(selection.version, TEST_VERSION);
    assert_eq!(selection.archive, TEST_ARCHIVE);
}

#[test]
fn selection_rejects_untrusted_urls_and_tag_version_mismatches() {
    for url in [
        "http://github.com/WXGopher/agent-companion/releases/tag/v2.4.0",
        "https://github.com.evil.invalid/WXGopher/agent-companion/releases/tag/v2.4.0",
        "https://github.com/another-owner/agent-companion/releases/tag/v2.4.0",
        "https://github.com/WXGopher/agent-companion/releases/tag/v2.4.1",
        "https://github.com/WXGopher/agent-companion/releases/tag/v2.4.0?download=1",
        "https://github.com/WXGopher/agent-companion/releases/tag/v2.4.0/other",
    ] {
        assert!(
            Selection::from_snapshot(&update(TEST_VERSION, url), "2.3.9", true, true).is_err(),
            "accepted release URL {url}"
        );
    }
}

#[test]
fn selection_requires_a_supported_newer_release_and_complete_snapshot() {
    let snapshot = update(TEST_VERSION, &format!("{RELEASE_PREFIX}{TEST_TAG}"));
    for (current, enabled, supported_arch) in [
        ("2.3.9", false, true),
        ("2.3.9", true, false),
        ("2.4.0", true, true),
        ("2.5.0", true, true),
        ("invalid", true, true),
    ] {
        assert!(
            Selection::from_snapshot(&snapshot, current, enabled, supported_arch).is_err(),
            "accepted current={current}, enabled={enabled}, arch={supported_arch}"
        );
    }
    // Build metadata cannot turn the same semantic version into an upgrade.
    let build_metadata = update("2.4.0+release", &format!("{RELEASE_PREFIX}v2.4.0+release"));
    assert!(Selection::from_snapshot(&build_metadata, "2.4.0+local", true, true).is_err());

    for incomplete in [
        crate::update_service::UpdateSnapshot::default(),
        crate::update_service::UpdateSnapshot {
            latest_version: None,
            ..snapshot.clone()
        },
        crate::update_service::UpdateSnapshot {
            release_url: None,
            ..snapshot
        },
    ] {
        assert!(Selection::from_snapshot(&incomplete, "2.3.9", true, true).is_err());
    }
}

#[test]
fn malformed_prerelease_and_path_like_versions_are_not_installable() {
    let overlong = format!("2.4.0+{}", "a".repeat(128));
    for version in [
        "",
        "2.4",
        "02.4.0",
        "v2.4.0-rc.1",
        "2.4.0/other",
        "2.4.0%2fother",
        "2.4.0\n",
        "2.4.0+é",
        overlong.as_str(),
    ] {
        assert!(stable_version(version).is_none(), "accepted {version:?}");
        assert!(
            Selection::from_snapshot(
                &update(version, &format!("{RELEASE_PREFIX}{version}")),
                "2.3.9",
                true,
                true,
            )
            .is_err(),
            "selected {version:?}"
        );
    }
}

#[test]
fn metadata_selects_exact_package_and_checksums_among_other_release_assets() {
    let mut metadata = release();
    metadata["assets"].as_array_mut().unwrap().insert(
        0,
        release_asset("agent-companion-v2.4.0-linux-x86_64.tar.gz", 103, 512),
    );
    metadata["assets"][1]["digest"] = format!("sha256:{ABC_SHA256}").into();
    let (archive, sums) = select_release(&metadata).unwrap();

    assert_eq!(
        (archive.id, archive.name.as_str(), archive.size),
        (101, TEST_ARCHIVE, 4096)
    );
    assert_eq!(
        archive.digest.as_deref(),
        Some(format!("sha256:{ABC_SHA256}").as_str())
    );
    assert_eq!((sums.id, sums.name.as_str()), (102, "SHA256SUMS.txt"));
    assert!(sums.digest.is_none());
}

#[test]
fn metadata_rejects_a_different_tag_draft_or_prerelease() {
    for (field, value) in [
        ("tag_name", json!("v2.4.1")),
        ("tag_name", json!("2.4.0")),
        ("draft", json!(true)),
        ("prerelease", json!(true)),
    ] {
        let mut metadata = release();
        metadata[field] = value;
        assert!(
            select_release(&metadata).is_err(),
            "accepted invalid {field}"
        );
    }
}

#[test]
fn metadata_requires_one_of_each_selected_asset() {
    for index in [0, 1] {
        let mut missing = release();
        missing["assets"].as_array_mut().unwrap().remove(index);
        assert!(
            select_release(&missing).is_err(),
            "accepted missing asset {index}"
        );

        let mut duplicate = release();
        let extra = duplicate["assets"][index].clone();
        duplicate["assets"].as_array_mut().unwrap().push(extra);
        assert!(
            select_release(&duplicate).is_err(),
            "accepted duplicate asset {index}"
        );
    }
}

#[test]
fn metadata_rejects_replaced_download_urls_and_asset_id_collisions() {
    for url in [
        format!(
            "https://github.com.evil.invalid/WXGopher/agent-companion/releases/download/{TEST_TAG}/{TEST_ARCHIVE}"
        ),
        format!(
            "https://github.com/another-owner/agent-companion/releases/download/{TEST_TAG}/{TEST_ARCHIVE}"
        ),
        format!(
            "https://github.com/WXGopher/agent-companion/releases/download/v2.4.1/{TEST_ARCHIVE}"
        ),
        format!("https://release-assets.githubusercontent.com/{TEST_ARCHIVE}"),
    ] {
        let mut metadata = release();
        metadata["assets"][0]["browser_download_url"] = url.clone().into();
        assert!(
            select_release(&metadata).is_err(),
            "accepted substituted asset URL {url}"
        );
    }

    let mut collision = release();
    collision["assets"][1]["id"] = collision["assets"][0]["id"].clone();
    assert!(select_release(&collision).is_err());
}

#[test]
fn metadata_rejects_empty_oversized_and_unidentified_assets() {
    for (index, field, value) in [
        (0, "id", 0),
        (1, "id", 0),
        (0, "size", 0),
        (1, "size", 0),
        (0, "size", MAX_ARCHIVE + 1),
        (1, "size", MAX_CHECKSUMS + 1),
    ] {
        let mut metadata = release();
        metadata["assets"][index][field] = value.into();
        assert!(
            select_release(&metadata).is_err(),
            "accepted asset {index} {field}={value}"
        );
    }

    let mut at_limit = release();
    at_limit["assets"][0]["size"] = MAX_ARCHIVE.into();
    at_limit["assets"][1]["size"] = MAX_CHECKSUMS.into();
    assert!(select_release(&at_limit).is_ok());
}

#[test]
fn metadata_rejects_unsupported_or_malformed_digests() {
    for digest in [
        format!("md5:{ABC_SHA256}"),
        format!("sha256:{}", &ABC_SHA256[..63]),
        format!("sha256:{}g", &ABC_SHA256[..63]),
        ABC_SHA256.into(),
    ] {
        for index in [0, 1] {
            let mut metadata = release();
            metadata["assets"][index]["digest"] = digest.clone().into();
            assert!(
                select_release(&metadata).is_err(),
                "accepted digest {digest:?} on asset {index}"
            );
        }
    }
}

#[test]
fn metadata_checks_the_byte_limit_before_parsing() {
    let mut body = serde_json::to_vec(&release()).unwrap();
    body.resize(MAX_METADATA as usize, b' ');
    assert!(select_assets(&body, &selected()).is_ok());
    body.push(b' ');
    assert!(
        select_assets(&body, &selected())
            .err()
            .unwrap()
            .to_string()
            .contains("too large")
    );

    for malformed in [b"{not json}".as_slice(), b"[]", b"{}"] {
        assert!(select_assets(malformed, &selected()).is_err());
    }
}

#[test]
fn checksum_selection_accepts_standard_text_and_binary_entries_and_normalizes_hex() {
    let checksums = format!(
        "\u{feff}\n{ABC_SHA256}  other.zip\r\n{} *{TEST_ARCHIVE}\r\n\n",
        ABC_SHA256.to_ascii_uppercase()
    );
    assert_eq!(
        checksum_for(checksums.as_bytes(), TEST_ARCHIVE).unwrap(),
        ABC_SHA256
    );
}

#[test]
fn checksum_selection_rejects_missing_duplicate_and_malformed_entries() {
    let duplicate = format!(
        "{ABC_SHA256}  {TEST_ARCHIVE}\n{} *{TEST_ARCHIVE}\n",
        "0".repeat(64)
    );
    let missing = format!("{ABC_SHA256}  other.zip\n");
    let short = format!("{}  {TEST_ARCHIVE}\n", &ABC_SHA256[..63]);
    let non_hex = format!("{}g  {TEST_ARCHIVE}\n", &ABC_SHA256[..63]);
    let extra_field = format!("{ABC_SHA256} {TEST_ARCHIVE} ignored\n");
    // A bad unrelated entry invalidates the manifest rather than being ignored.
    let bad_other_entry = format!("{ABC_SHA256} {TEST_ARCHIVE}\ninvalid other.zip\n");
    for manifest in [
        duplicate.as_bytes(),
        missing.as_bytes(),
        short.as_bytes(),
        non_hex.as_bytes(),
        extra_field.as_bytes(),
        bad_other_entry.as_bytes(),
        ABC_SHA256.as_bytes(),
        b"\xff\xfe",
        b"",
    ] {
        assert!(
            checksum_for(manifest, TEST_ARCHIVE).is_err(),
            "accepted {manifest:?}"
        );
    }
}

#[test]
fn checksum_asset_digest_accepts_uppercase_hex_and_rejects_changed_bytes() {
    let mut metadata = release();
    metadata["assets"][1]["digest"] = format!("sha256:{}", ABC_SHA256.to_ascii_uppercase()).into();
    let (_, sums) = select_release(&metadata).unwrap();

    verify_asset_digest(b"abc", &sums).unwrap();
    assert!(verify_asset_digest(b"abd", &sums).is_err());

    let (_, unsigned_sums) = select_release(&release()).unwrap();
    assert!(verify_asset_digest(b"abc", &unsigned_sums).is_ok());
}

#[test]
fn asset_redirects_accept_only_https_github_download_hosts() {
    for host in [
        "github.com",
        "release-assets.githubusercontent.com",
        "objects.githubusercontent.com",
        "github-releases.githubusercontent.com",
    ] {
        let url = Url::parse(&format!("https://{host}/asset?token=signed&download=1")).unwrap();
        assert!(safe_asset_url(&url), "rejected allowed host {host}");
    }

    for value in [
        "http://github.com/asset",
        "https://github.com.evil.invalid/asset",
        "https://release-assets.githubusercontent.com.evil.invalid/asset",
        "https://evil.github.com/asset",
        "https://githubusercontent.com/asset",
        "https://user@github.com/asset",
        "https://user:password@github.com/asset",
        "https://github.com@evil.invalid/asset",
        "https://github.com:8443/asset",
        "https://github.com/asset#fragment",
        "https://127.0.0.1/asset",
    ] {
        assert!(
            !safe_asset_url(&Url::parse(value).unwrap()),
            "accepted {value}"
        );
    }
}

/// Simulates network reads that split data independently of the copy buffer.
struct Chunks<R> {
    reader: R,
    maximum: usize,
}

impl<R: Read> Read for Chunks<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let length = buffer.len().min(self.maximum);
        self.reader.read(&mut buffer[..length])
    }
}

struct ReadFailure;

impl Read for ReadFailure {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "connection reset",
        ))
    }
}

#[test]
fn bounded_copy_accepts_the_exact_limit_and_reports_actual_progress() {
    let mut reader = Chunks {
        reader: Cursor::new(b"abcdef"),
        maximum: 2,
    };
    let mut output = Vec::new();
    let mut progress = Vec::new();

    assert_eq!(
        copy_bounded(&mut reader, &mut output, 6, |count| progress.push(count)).unwrap(),
        6
    );
    assert_eq!(output, b"abcdef");
    assert_eq!(progress, [2, 4, 6]);
    assert_eq!(
        copy_bounded(&mut io::empty(), &mut Vec::new(), 0, |_| {}).unwrap(),
        0
    );
}

#[test]
fn bounded_copy_stops_before_writing_a_chunk_that_would_exceed_the_limit() {
    let mut reader = Chunks {
        reader: Cursor::new(b"abcdefgh"),
        maximum: 3,
    };
    let mut output = Vec::new();
    let mut progress = Vec::new();

    assert!(copy_bounded(&mut reader, &mut output, 6, |count| progress.push(count)).is_err());
    assert_eq!(output, b"abcdef");
    assert_eq!(progress, [3, 6]);

    let mut no_bytes_allowed = Vec::new();
    assert!(copy_bounded(&mut Cursor::new(b"x"), &mut no_bytes_allowed, 0, |_| {}).is_err());
    assert!(no_bytes_allowed.is_empty());
}

#[test]
fn bounded_copy_and_hashing_propagate_read_failure_instead_of_accepting_a_prefix() {
    let mut reader = Cursor::new(b"abc").chain(ReadFailure);
    let mut output = Vec::new();
    let mut progress = Vec::new();
    let error =
        copy_bounded(&mut reader, &mut output, 100, |count| progress.push(count)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert_eq!(output, b"abc");
    assert_eq!(progress, [3]);

    let error = hash_reader(&mut Cursor::new(b"abc").chain(ReadFailure)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
}

struct WriteFailure {
    bytes: Vec<u8>,
}

impl Write for WriteFailure {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.is_empty() {
            let count = bytes.len().min(2);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        } else {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "writer stopped"))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn bounded_copy_propagates_partial_write_failure_without_reporting_the_chunk_complete() {
    let mut writer = WriteFailure { bytes: Vec::new() };
    let mut progress = Vec::new();
    let error = copy_bounded(&mut Cursor::new(b"abcdef"), &mut writer, 6, |count| {
        progress.push(count)
    })
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(writer.bytes, b"ab");
    assert!(progress.is_empty());
}

#[test]
fn streaming_hash_matches_known_sha256_vectors_across_buffer_boundaries() {
    assert_eq!(
        hash_reader(&mut io::empty()).unwrap(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(hash_reader(&mut Cursor::new(b"abc")).unwrap(), ABC_SHA256);
    let mut million_a = Chunks {
        reader: Cursor::new(vec![b'a'; 1_000_000]),
        maximum: 8191,
    };
    assert_eq!(
        hash_reader(&mut million_a).unwrap(),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn readiness_without_a_helper_fails_once_without_starting_or_stopping_a_process() {
    let manager = Manager {
        inner: Arc::new(Inner {
            state: Mutex::new(State {
                snapshot: Snapshot {
                    stage: Stage::Restarting,
                    ..Snapshot::default()
                },
                helper: None,
                restart_ready: true,
            }),
            result_path: None,
        }),
    };

    assert!(!manager.take_restart_ready());
    let failed = manager.snapshot();
    assert_eq!(failed.stage, Stage::Failed);
    assert!(!failed.message.is_empty());
    assert!(!manager.inner.state().restart_ready);
    assert!(!manager.take_restart_ready());
    assert_eq!(manager.snapshot(), failed);
}
