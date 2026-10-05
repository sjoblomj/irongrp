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
        ("fish",       "# Print an optspec for argparse"),
        ("elvish",     "\nuse builtin;"),
        ("powershell", "\nusing namespace System.Management.Automation"),
    ] {
        let output = irongrp()
            .args(["completions", shell])
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
        .args(["grp-to-png", &grp]).arg(&out_dir)
        .args(["--frame", "3"])
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
        .args(["grp-to-png", &grp]).arg(&out_dir)
        .args(["--frame", "2"])
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

    let out_dir = out_dir.to_str().unwrap();
    for mode_args in [
        vec!["grp-to-png", grp, out_dir],
        vec!["grp-to-png", grp, out_dir, "--tiled"],
        vec!["analyse", grp],
    ] {
        let output = irongrp()
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
    let output = irongrp().args(["png-to-grp", "--help"]).output().expect("failed to run irongrp");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    // clap may wrap the help text, so compare it with whitespace collapsed
    let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(help.contains(r#"contains "uncompressed_" or "war1_""#), "{}", help);
}

#[test]
fn old_flag_names_are_accepted_as_aliases() {
    let temp_dir = tempfile::tempdir().unwrap();
    let grp = write_test_grp(temp_dir.path(), 3);
    let out_dir = temp_dir.path().join("out");

    let output = irongrp()
        .args(["grp-to-png", &grp]).arg(&out_dir)
        .args(["--frame-number", "1", "--use-transparency"])
        .output()
        .expect("failed to run irongrp");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let output = irongrp()
        .args(["analyse-grp", &grp, "--frame-number", "0", "--analyse-row-number", "0"])
        .output()
        .expect("failed to run irongrp");
    // Row analysis is only supported for Normal GRPs, but the flags themselves must be accepted
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--row is only supported for GRPs of type Normal"), "{:?}", stderr);
}

#[test]
fn flags_of_other_subcommands_are_rejected() {
    let temp_dir = tempfile::tempdir().unwrap();
    let grp = write_test_grp(temp_dir.path(), 3);
    let out_dir = temp_dir.path().join("out");
    let out_dir = out_dir.to_str().unwrap();

    for args in [
        vec!["png-to-grp", out_dir, &grp, "--tiled"],
        vec!["png-to-grp", out_dir, &grp, "--frame", "0"],
        vec!["grp-to-png", &grp, out_dir, "--compression", "normal"],
        vec!["grp-to-png", &grp, out_dir, "--row", "0"],
        vec!["analyse", &grp, "--palette", "units.pal"],
        vec!["analyse", &grp, "--row", "0"], // --row requires --frame
        vec!["grp-to-png", &grp, out_dir, "--tiled", "--frame", "0"],
        vec!["grp-to-png", &grp, out_dir, "--max-width", "100"], // --max-width requires --tiled
    ] {
        let output = irongrp().args(&args).output().expect("failed to run irongrp");
        assert_eq!(output.status.code(), Some(2), "expected a usage error for {:?}", args);
    }
}

#[test]
fn input_and_output_are_required() {
    for args in [vec!["grp-to-png", "a.grp"], vec!["png-to-grp", "dir"], vec!["analyse"], vec![]] {
        let output = irongrp().args(&args).output().expect("failed to run irongrp");
        assert_eq!(output.status.code(), Some(2), "expected a usage error for {:?}", args);
    }
}
