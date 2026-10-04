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
