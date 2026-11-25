use std::process::{Command, Output};

fn run(args: &[&str], settings: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_soq-indexer"))
        .env_clear()
        .envs(settings.iter().copied())
        .args(args)
        .output()
        .expect("launch indexer")
}

fn assert_error(output: Output, expected: &str) {
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(expected), "{stderr}");
}

#[test]
fn help_lists_the_cli_and_configuration_without_credentials() {
    let output = run(&["--help"], &[]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for setting in [
        "--follow",
        "--help",
        "SOQ_RPC_URL",
        "SOQ_RPC_USER",
        "SOQ_RPC_PASSWORD",
        "SOQ_DATABASE",
        "SOQ_NETWORK",
        "SOQ_GENESIS",
    ] {
        assert!(help.contains(setting), "{setting}");
    }
}

#[test]
fn unsupported_arguments_and_combined_flags_are_rejected() {
    for args in [&["--start-height", "100"][..], &["--help", "--follow"][..]] {
        assert_error(run(args, &[]), "usage: soq-indexer");
    }
}

#[test]
fn configuration_is_checked_before_contacting_the_node() {
    assert_error(run(&[], &[]), "set SOQ_RPC_USER");
    assert_error(
        run(&[], &[("SOQ_RPC_USER", "fixture-user")]),
        "set SOQ_RPC_PASSWORD",
    );
    assert_error(
        run(&["--follow"], &[("SOQ_NETWORK", "unknown")]),
        "unsupported network",
    );
    assert_error(
        run(
            &[],
            &[
                ("SOQ_NETWORK", "regtest"),
                ("SOQ_RPC_USER", "fixture-user"),
                ("SOQ_RPC_PASSWORD", "fixture-password"),
                ("SOQ_RPC_URL", "http://192.0.2.1:1"),
            ],
        ),
        "RPC must use loopback",
    );
}
