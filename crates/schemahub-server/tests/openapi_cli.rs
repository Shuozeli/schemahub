//! Byte-level regression coverage for the generated OpenAPI release artifact.

use std::process::{Command, Output};

use schemahub_server::http;

fn render_openapi_in_a_fresh_process() -> Output {
    Command::new(env!("CARGO_BIN_EXE_schemahub-server"))
        .arg("--print-openapi")
        .output()
        .expect("schemahub-server should start")
}

#[test]
fn generated_openapi_is_byte_stable_across_processes() {
    // Arrange
    const PROCESS_COUNT: usize = 8;

    // Act
    let outputs = (0..PROCESS_COUNT)
        .map(|_| render_openapi_in_a_fresh_process())
        .collect::<Vec<_>>();

    // Assert
    for output in &outputs {
        assert!(
            output.status.success(),
            "schemahub-server failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "OpenAPI command wrote to stderr");
        assert_eq!(output.stdout, http::openapi_json_bytes());
    }
    assert!(
        outputs
            .windows(2)
            .all(|pair| pair[0].stdout == pair[1].stdout),
        "OpenAPI bytes changed across identical processes"
    );
    assert!(http::openapi_json_bytes().ends_with(b"\n"));
}
