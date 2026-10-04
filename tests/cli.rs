use std::path::Path;
use std::process::Command;

fn irongrp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_irongrp"))
}

#[test]
fn shell_completions_contain_only_the_completion_script() {
    for (shell, expected_start) in [
        ("bash",       "_irongrp()"),
        ("zsh",        "#compdef irongrp"),
        ("fish",       "complete -c irongrp"),
        ("elvish",     "\nuse builtin;"),
        ("powershell", "\nusing namespace System.Management.Automation"),
    ] {
        let output = irongrp()
            .args(["--generate-shell-completions", shell])
            .output()
            .expect("failed to run irongrp");

        assert!(output.status.success(), "for {}: {}", shell, String::from_utf8_lossy(&output.stderr));
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.starts_with(expected_start),
            "expected the {} completion script to start with {:?}, but it starts with {:?}",
            shell, expected_start, &stdout[..stdout.len().min(80)],
        );
        assert!(!stdout.contains("[INFO]"), "for {}", shell);
        assert!(!stdout.contains("Generating completions"), "for {}", shell);

        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("Generating completions for"), "for {}: {:?}", shell, stderr);
    }
}

/// Writes an Uncompressed GRP with `frame_count` 1x1 frames, and returns its path
fn write_test_grp(dir: &Path, frame_count: u8) -> String {
    let mut data = vec![frame_count, 0, 1, 0, 1, 0]; // Frame count, max width 1, max height 1
    let first_offset = 6 + 8 * frame_count as u32;
    for i in 0..frame_count {
        data.extend([0, 0, 1, 1]); // x and y offsets, width and height
        data.extend((first_offset + i as u32).to_le_bytes());
    }
    data.extend((0..frame_count).map(|i| i + 1)); // One pixel per frame
    let path = dir.join("test.grp");
    std::fs::write(&path, data).unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn grp_to_png_rejects_frame_number_out_of_range() {
    let temp_dir = tempfile::tempdir().unwrap();
    let grp = write_test_grp(temp_dir.path(), 3);
    let out_dir = temp_dir.path().join("out");

    let output = irongrp()
        .args(["--mode", "grp-to-png", "--input-path", &grp, "--frame-number", "3"])
        .arg("--output-path").arg(&out_dir)
        .output()
        .expect("failed to run irongrp");

    assert!(!output.status.success(), "expected a failure exit status");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Frame number 3 is out of range; the GRP has 3 frame(s)"), "{:?}", stderr);
    assert_eq!(std::fs::read_dir(&out_dir).unwrap().count(), 0, "expected no PNGs to be written");
}

#[test]
fn grp_to_png_writes_only_the_requested_frame() {
    let temp_dir = tempfile::tempdir().unwrap();
    let grp = write_test_grp(temp_dir.path(), 3);
    let out_dir = temp_dir.path().join("out");

    let output = irongrp()
        .args(["--mode", "grp-to-png", "--input-path", &grp, "--frame-number", "2"])
        .arg("--output-path").arg(&out_dir)
        .output()
        .expect("failed to run irongrp");

    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let files: Vec<String> = std::fs::read_dir(&out_dir).unwrap()
        .map(|e| e.unwrap().file_name().to_str().unwrap().to_string())
        .collect();
    assert_eq!(files, vec!["uncompressed_frame_002.png"]);
}

#[test]
fn grp_without_frames_is_rejected() {
    let temp_dir = tempfile::tempdir().unwrap();
    let grp = temp_dir.path().join("empty.grp");
    std::fs::write(&grp, [0, 0, 16, 0, 16, 0]).unwrap(); // 0 frames, max size 16x16
    let grp = grp.to_str().unwrap();
    let out_dir = temp_dir.path().join("out");

    for mode_args in [
        vec!["--mode", "grp-to-png", "--output-path", out_dir.to_str().unwrap()],
        vec!["--mode", "grp-to-png", "--output-path", out_dir.to_str().unwrap(), "--tiled"],
        vec!["--mode", "analyse-grp"],
    ] {
        let output = irongrp()
            .args(["--input-path", grp])
            .args(&mode_args)
            .output()
            .expect("failed to run irongrp");

        assert!(!output.status.success(), "expected a failure exit status for {:?}", mode_args);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains(&format!("{}: invalid GRP: The GRP has no frames", grp)),
            "for {:?}: {:?}", mode_args, stderr,
        );
    }
}

#[test]
fn help_describes_how_the_compression_type_is_detected() {
    let output = irongrp().arg("--help").output().expect("failed to run irongrp");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    // clap may wrap the help text, so compare it with whitespace collapsed
    let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(help.contains(r#"contains "uncompressed_" or "war1_""#), "{}", help);
}
