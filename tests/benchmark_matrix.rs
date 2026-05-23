use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn write_fake_binary(path: &Path, version_text: &str) -> io::Result<()> {
    let mut f = File::create(path)?;
    writeln!(f, "#!/usr/bin/env bash")?;
    writeln!(f, "set -euo pipefail")?;
    writeln!(f, "if [[ \"${{1:-}}\" == --version ]]; then")?;
    writeln!(f, "  echo \"{}\"", version_text)?;
    writeln!(f, "  exit 0")?;
    writeln!(f, "fi")?;
    writeln!(f, "printf ''")?;

    #[cfg(unix)]
    {
        let mut p = fs::metadata(path)?.permissions();
        p.set_mode(0o755);
        fs::set_permissions(path, p)?;
    }

    Ok(())
}

#[test]
fn benchmark_matrix_script_generates_matrix_artifacts_in_dry_run() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = repo_root.join("scripts/bench-list-matrix.sh");
    assert!(
        script.exists(),
        "bench script should exist: {}",
        script.display()
    );

    let workspace = TempDir::new().unwrap();
    let fake_bin = workspace.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();

    for tool in ["ghq", "fd", "find", "hyperfine", "scap"] {
        write_fake_binary(&fake_bin.join(tool), &format!("{} 1.2.3", tool)).unwrap();
    }

    let real_a = TempDir::new().unwrap();
    let real_b = TempDir::new().unwrap();

    let run_id = "task1-dryrun-matrix";
    let output_root = repo_root
        .join(".omx/assets/scap-list-oss-fastest")
        .join(run_id);
    let _ = fs::remove_dir_all(&output_root);

    let original_path = env::var("PATH").unwrap_or_default();
    let path = format!("{}:{}", fake_bin.display(), original_path);

    let out = Command::new("bash")
        .arg(&script)
        .env("PATH", path)
        .env("SCAP_BENCH_DRY_RUN", "1")
        .env("SCAP_BENCH_RUN_ID", run_id)
        .env(
            "SCAP_BENCH_ROOTS",
            format!("{}:{}", real_a.path().display(), real_b.path().display()),
        )
        .env("SCAP_BENCH_SYNTH_HOSTS", "1")
        .env("SCAP_BENCH_SYNTH_USERS", "1")
        .env("SCAP_BENCH_SYNTH_REPOS", "1")
        .env("SCAP_BENCH_SYNTH_NOISE", "2")
        .env("SCAP_BIN", fake_bin.join("scap"))
        .env("GHQ_BIN", fake_bin.join("ghq"))
        .env("FD_BIN", fake_bin.join("fd"))
        .env("FIND_BIN", fake_bin.join("find"))
        .env("HYPERFINE_BIN", fake_bin.join("hyperfine"))
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "benchmark matrix script failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let metadata = output_root.join("metadata.json");
    assert!(metadata.exists(), "metadata.json must be created");
    let metadata_body = fs::read_to_string(&metadata).unwrap();
    assert!(
        metadata_body.contains(&format!("\"run_id\": \"{}\"", run_id)),
        "metadata must include run id"
    );
    assert!(
        metadata_body.contains("\"runs\": 20"),
        "metadata must include hyperfine run count"
    );

    let expected_artifacts = [
        "metadata.json",
        ".commands/real_ghq_sort.sh",
        ".commands/real_ghq_devnull.sh",
        ".commands/real_scap_sort.sh",
        ".commands/real_scap_devnull.sh",
        ".commands/real_fd_raw_sort.sh",
        ".commands/real_fd_raw_devnull.sh",
        ".commands/real_find_raw_sort.sh",
        ".commands/real_find_raw_devnull.sh",
        ".commands/synthetic_fd_raw_sort.sh",
        ".commands/synthetic_find_raw_sort.sh",
        ".commands/synthetic_ghq_sort.sh",
        ".commands/synthetic_scap_sort.sh",
    ];

    for rel in expected_artifacts {
        assert!(
            output_root.join(rel).exists(),
            "missing artifact: {}",
            output_root.join(rel).display()
        );
    }

    for rel in [
        "real/ghq-sort.json",
        "real/scap-sort.json",
        "synthetic/ghq-sort.json",
        "synthetic/scap-sort.json",
    ] {
        let artifact = output_root.join(rel);
        assert!(
            artifact.exists(),
            "missing artifact json: {}",
            artifact.display()
        );
        let body = fs::read_to_string(artifact).unwrap();
        assert!(
            body.contains("\"command\": "),
            "hyperfine placeholder should include command metadata"
        );
    }

    let real_ghq_cmd = fs::read_to_string(output_root.join(".commands/real_ghq_sort.sh")).unwrap();
    assert!(
        real_ghq_cmd.contains(&format!(
            "GHQ_ROOT=\"{}:{}\"",
            real_a.path().display(),
            real_b.path().display()
        )),
        "real_ghq_sort should use benchmark root override"
    );

    let real_find_cmd =
        fs::read_to_string(output_root.join(".commands/real_find_raw_sort.sh")).unwrap();
    assert!(
        real_find_cmd.contains("|| true"),
        "real_find_raw_sort command should tolerate non-fatal finder failures"
    );

    let synthetic_find_cmd =
        fs::read_to_string(output_root.join(".commands/synthetic_find_raw_sort.sh")).unwrap();
    assert!(
        synthetic_find_cmd.contains("|| true"),
        "synthetic_find_raw_sort command should tolerate non-fatal finder failures"
    );

    let summary = fs::read_to_string(output_root.join("matrix-summary.md")).unwrap();
    assert!(
        summary.contains("- Real corpus: `real/*.json`")
            && summary.contains("- Synthetic corpus: `synthetic/*.json`"),
        "matrix summary should include artifact sections for real and synthetic outputs"
    );

    assert!(
        summary.contains(&format!(
            "Real-row commands use GHQ_ROOT={}",
            real_a.path().display(),
        )) && summary.contains(&format!(":{}", real_b.path().display()))
            || summary.contains(&format!(
                "Real-row commands use GHQ_ROOT=\"{}:{}\"",
                real_a.path().display(),
                real_b.path().display()
            )),
        "matrix summary should include expanded GHQ_ROOT for real roots"
    );
}
