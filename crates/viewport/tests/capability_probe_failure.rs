use std::io::{self, Write};

use threeterm_viewport::{
    CapabilityProbe, CapabilityProbeIo, TerminalEnvironment, ViewportDiagnosticCode,
};

#[derive(Debug)]
struct ProbeIo {
    writes: Vec<u8>,
    response: Vec<u8>,
}

impl Write for ProbeIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl CapabilityProbeIo for ProbeIo {
    fn read_probe_response(&mut self, _max: usize) -> io::Result<Vec<u8>> {
        Ok(std::mem::take(&mut self.response))
    }
}

fn base_env() -> TerminalEnvironment {
    TerminalEnvironment {
        term: Some("xterm-ghostty".to_string()),
        term_program: Some("ghostty".to_string()),
        in_tmux: false,
        over_ssh: false,
        foreground_tty: true,
        utf8: true,
        width: 80,
        height: 24,
    }
}

#[test]
fn probe_env_denials_are_fail_closed_with_recovery() {
    let cases: Vec<(TerminalEnvironment, &str, &str)> = vec![
        (
            TerminalEnvironment {
                term: Some("xterm-256color".to_string()),
                ..base_env()
            },
            "direct Ghostty identity is missing",
            "supported direct Ghostty attachment",
        ),
        (
            TerminalEnvironment {
                term_program: None,
                ..base_env()
            },
            "direct Ghostty identity is missing",
            "supported direct Ghostty attachment",
        ),
        (
            TerminalEnvironment {
                in_tmux: true,
                ..base_env()
            },
            "transport is indirect",
            "direct local Ghostty window",
        ),
        (
            TerminalEnvironment {
                over_ssh: true,
                ..base_env()
            },
            "transport is indirect",
            "direct local Ghostty window",
        ),
        (
            TerminalEnvironment {
                foreground_tty: false,
                ..base_env()
            },
            "foreground UTF-8 TTY baseline is unavailable",
            "foreground UTF-8 TTY",
        ),
        (
            TerminalEnvironment {
                utf8: false,
                ..base_env()
            },
            "foreground UTF-8 TTY baseline is unavailable",
            "foreground UTF-8 TTY",
        ),
        (
            TerminalEnvironment {
                width: 0,
                ..base_env()
            },
            "terminal dimensions are unavailable",
            "positive dimensions",
        ),
        (
            TerminalEnvironment {
                height: 0,
                ..base_env()
            },
            "terminal dimensions are unavailable",
            "positive dimensions",
        ),
    ];
    for (env, detail_substr, recovery_substr) in cases {
        let mut io = ProbeIo {
            writes: Vec::new(),
            response: Vec::new(),
        };
        let diagnostic = CapabilityProbe::new(1)
            .probe(&mut io, env)
            .expect_err("env denial must fail closed");
        assert_eq!(diagnostic.code, ViewportDiagnosticCode::CapabilityDenied);
        assert_eq!(diagnostic.source_revision, "capability-probe");
        assert_eq!(diagnostic.schema_version, "threeterm.viewport/1");
        assert!(
            diagnostic.detail.contains(detail_substr),
            "detail {} does not contain {}",
            diagnostic.detail,
            detail_substr
        );
        assert!(
            diagnostic.recovery.contains(recovery_substr),
            "recovery {} does not contain {}",
            diagnostic.recovery,
            recovery_substr
        );
        assert!(
            io.writes.is_empty(),
            "denied probe must not touch terminal wire"
        );
    }
}

#[test]
fn probe_wire_and_transcript_fail_closed_with_structured_diagnostics() {
    // Zero nonce
    let mut io = ProbeIo {
        writes: Vec::new(),
        response: Vec::new(),
    };
    let diag = CapabilityProbe::new(0)
        .probe(&mut io, base_env())
        .expect_err("zero nonce must be malformed");
    assert_eq!(diag.code, ViewportDiagnosticCode::CapabilityMalformed);
    assert_eq!(diag.source_revision, "capability-probe");
    assert_eq!(diag.schema_version, "threeterm.viewport/1");
    assert!(diag.detail.contains("nonce must be non-zero"));
    assert!(diag.recovery.contains("fresh nonce"));

    // Nonce overflow (MAX) — first emit exhausts Kitty image identity before replacement check
    let mut io = ProbeIo {
        writes: Vec::new(),
        response: Vec::new(),
    };
    let diag = CapabilityProbe::new(u64::MAX)
        .probe(&mut io, base_env())
        .expect_err("nonce overflow must be fail-closed");
    assert!(
        diag.code == ViewportDiagnosticCode::TransportWriteFailed
            || diag.code == ViewportDiagnosticCode::CapabilityMalformed
    );
    assert!(
        diag.detail
            .contains("cannot produce a replacement identity")
            || diag.detail.contains("Kitty image identity exhausted")
    );

    // Wire missing kitty sequences — empty response with valid nonce but no wire evidence
    // We inject a writer that doesn't capture required sequences by using a custom Io that pretends to write but we check wire check
    // Actually base probe writes ENTER_SEQUENCE and two images; use response with valid wire but missing ack
    let mut io = ProbeIo {
        writes: Vec::new(),
        response: b"some response without acks\x1b[?3u\x1b[97;1:1u\x1b[97;1:2u\x1b[<0;1;1M\x1b[<32;2;1M\x1b[<0;2;1m\x1b[<0;101;101M\x1b[<32;102;101M\x1b[<0;102;101m\x1b[I\x1b[8;24;80t".to_vec(),
    };
    // Need to feed response that will fail because missing ack, but wire itself should pass (wire is produced by probe, not response)
    // The wire check is on captured writes, which will always contain a=T,t=d etc, so this case tests missing ack not wire.
    let diag = CapabilityProbe::new(77)
        .probe(&mut io, base_env())
        .expect_err("missing ack must be malformed or timeout");
    assert!(matches!(
        diag.code,
        ViewportDiagnosticCode::CapabilityMalformed | ViewportDiagnosticCode::CapabilityTimeout
    ));
    assert_eq!(diag.source_revision, "capability-probe");

    // Oversize response (>64KiB)
    struct OversizeIo {
        writes: Vec<u8>,
    }
    impl Write for OversizeIo {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl CapabilityProbeIo for OversizeIo {
        fn read_probe_response(&mut self, _max: usize) -> io::Result<Vec<u8>> {
            Ok(vec![b'x'; 65 * 1024])
        }
    }
    let mut io = OversizeIo { writes: Vec::new() };
    let diag = CapabilityProbe::new(5)
        .probe(&mut io, base_env())
        .expect_err("oversize response must be malformed");
    assert_eq!(diag.code, ViewportDiagnosticCode::CapabilityMalformed);
    assert!(diag.detail.contains("exceeds the bounded read size"));

    // IO error => CapabilityTimeout
    struct FailingReadIo {
        writes: Vec<u8>,
    }
    impl Write for FailingReadIo {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl CapabilityProbeIo for FailingReadIo {
        fn read_probe_response(&mut self, _max: usize) -> io::Result<Vec<u8>> {
            Err(io::Error::other("injected read failure"))
        }
    }
    let mut io = FailingReadIo { writes: Vec::new() };
    let diag = CapabilityProbe::new(6)
        .probe(&mut io, base_env())
        .expect_err("read error must be timeout");
    assert_eq!(diag.code, ViewportDiagnosticCode::CapabilityTimeout);
    assert_eq!(diag.source_revision, "capability-probe");

    // Empty response => CapabilityTimeout
    let mut io = ProbeIo {
        writes: Vec::new(),
        response: Vec::new(),
    };
    let diag = CapabilityProbe::new(7)
        .probe(&mut io, base_env())
        .expect_err("empty response must be timeout");
    assert_eq!(diag.code, ViewportDiagnosticCode::CapabilityTimeout);
    assert!(diag.detail.contains("did not acknowledge"));

    // Missing replacement ack (only first ack present)
    let nonce = 88;
    let response = format!(
        "\x1b_Gi={nonce};OK\x1b\\\x1b[?u\x1b[97;1:1u\x1b[97;1:2u\x1b[<0;1;1M\x1b[<32;2;1M\x1b[<0;2;1m\x1b[<0;101;101M\x1b[<32;102;101M\x1b[<0;102;101m\x1b[I\x1b[8;24;80t"
    );
    let mut io = ProbeIo {
        writes: Vec::new(),
        response: response.into_bytes(),
    };
    let diag = CapabilityProbe::new(nonce)
        .probe(&mut io, base_env())
        .expect_err("missing replacement ack must be malformed");
    assert_eq!(diag.code, ViewportDiagnosticCode::CapabilityMalformed);
    assert!(diag.detail.contains("replacement"));

    // Incomplete transcript (missing focus/alternate-screen etc) — provide both acks but missing transcript evidence
    let nonce = 90;
    let response = format!("\x1b_Gi={nonce};OK\x1b\\\x1b_Gi={};OK\x1b\\", nonce + 1);
    let mut io = ProbeIo {
        writes: Vec::new(),
        response: response.into_bytes(),
    };
    let diag = CapabilityProbe::new(nonce)
        .probe(&mut io, base_env())
        .expect_err("incomplete transcript must be malformed");
    assert_eq!(diag.code, ViewportDiagnosticCode::CapabilityMalformed);
    assert!(diag.detail.contains("observations are incomplete"));
}
