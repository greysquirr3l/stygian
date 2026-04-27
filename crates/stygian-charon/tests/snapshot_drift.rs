use std::fs;
use std::path::{Path, PathBuf};

use stygian_charon::{
    NormalizedFingerprintSnapshot, SnapshotDeterminismOptions, compare_snapshot_signal_drift,
};

#[derive(Debug, serde::Deserialize)]
struct FixtureManifest {
    fixtures: Vec<String>,
}

fn parse_snapshot_file(path: &Path) -> NormalizedFingerprintSnapshot {
    let raw = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read snapshot file {}: {error}", path.display()));
    serde_json::from_str::<NormalizedFingerprintSnapshot>(&raw)
        .unwrap_or_else(|error| panic!("failed to parse snapshot file {}: {error}", path.display()))
}

fn source_path_for_fixture(crate_root: &Path, fixture_name: &str) -> PathBuf {
    let source_name = fixture_name.strip_suffix(".fixture.json").map_or_else(
        || fixture_name.to_string(),
        |prefix| format!("{prefix}.json"),
    );

    crate_root.join("docs").join("examples").join(source_name)
}

#[test]
fn snapshot_fixtures_match_latest_sources() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixtures_root = crate_root.join("docs").join("examples").join("fixtures");
    let manifest_path = fixtures_root.join("manifest.json");

    let manifest_raw = fs::read_to_string(&manifest_path).unwrap_or_else(|error| {
        panic!(
            "failed to read fixture manifest {}: {error}",
            manifest_path.display()
        )
    });
    let manifest = serde_json::from_str::<FixtureManifest>(&manifest_raw).unwrap_or_else(|error| {
        panic!(
            "failed to parse fixture manifest {}: {error}",
            manifest_path.display()
        )
    });

    let options = SnapshotDeterminismOptions::default();
    for fixture_name in manifest.fixtures {
        let fixture_path = fixtures_root.join(&fixture_name);
        let source_path = source_path_for_fixture(&crate_root, &fixture_name);

        assert!(
            source_path.exists(),
            "source snapshot for fixture {fixture_name} is missing: {}",
            source_path.display()
        );

        let baseline = parse_snapshot_file(&fixture_path);
        let latest = parse_snapshot_file(&source_path);

        let report =
            compare_snapshot_signal_drift(&baseline, &latest, &options).unwrap_or_else(|error| {
                panic!(
                    "failed to compare fixture {} against source {}: {error}",
                    fixture_path.display(),
                    source_path.display()
                )
            });

        assert!(
            !report.has_drift(),
            "unapproved signal drift for fixture {}:\n{}",
            fixture_name,
            report.render_focused_diff()
        );
    }
}
