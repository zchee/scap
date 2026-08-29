use std::io;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

use super::*;

/// A `MakeWriter` that appends everything written to it into a shared
/// buffer, so a test can read back exactly what a subscriber wrote.
///
/// `tracing_subscriber::fmt::TestWriter` writes to stdout, which `cargo
/// test`/`nextest` capture per-test as a whole but do not expose back to the
/// test body, so it cannot stand in for a per-test assertion target here.
#[derive(Clone)]
struct BufWriter(Arc<Mutex<Vec<u8>>>);

impl BufWriter {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    fn captured(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

struct BufWriterGuard(Arc<Mutex<Vec<u8>>>);

impl io::Write for BufWriterGuard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'w> MakeWriter<'w> for BufWriter {
    type Writer = BufWriterGuard;

    fn make_writer(&'w self) -> Self::Writer {
        BufWriterGuard(self.0.clone())
    }
}

#[test]
fn no_log_env_admits_warn_only() {
    let writer = BufWriter::new();
    tracing::subscriber::with_default(build_subscriber(None, writer.clone()), || {
        tracing::warn!("w1");
        tracing::info!("i1");
        tracing::debug!("d1");
    });
    let out = writer.captured();
    assert!(out.contains("w1"), "expected warn to reach the writer with no log env; got: {out}");
    assert!(!out.contains("i1"), "info must be filtered out with no log env; got: {out}");
    assert!(!out.contains("d1"), "debug must be filtered out with no log env; got: {out}");
}

#[test]
fn debug_directive_admits_every_level() {
    let writer = BufWriter::new();
    tracing::subscriber::with_default(build_subscriber(Some("debug"), writer.clone()), || {
        tracing::warn!("w2");
        tracing::info!("i2");
        tracing::debug!("d2");
    });
    let out = writer.captured();
    assert!(out.contains("w2"), "expected warn to reach the writer under debug; got: {out}");
    assert!(out.contains("i2"), "expected info to reach the writer under debug; got: {out}");
    assert!(out.contains("d2"), "expected debug to reach the writer under debug; got: {out}");
}

#[test]
fn target_scoped_directive_is_respected() {
    // `tracing::info!`/`debug!` below default their target to the calling
    // module path, which is `scap::tests` here -- a `scap`-prefixed target,
    // so the `scap=info` directive applies to it.
    let writer = BufWriter::new();
    tracing::subscriber::with_default(build_subscriber(Some("scap=info"), writer.clone()), || {
        tracing::info!("i3");
        tracing::debug!("d3");
    });
    let out = writer.captured();
    assert!(out.contains("i3"), "expected info to reach the writer under scap=info; got: {out}");
    assert!(!out.contains("d3"), "expected debug to stay filtered under scap=info; got: {out}");
}

#[test]
fn invalid_directive_falls_back_to_warn_without_panicking() {
    let writer = BufWriter::new();
    tracing::subscriber::with_default(build_subscriber(Some("=[="), writer.clone()), || {
        tracing::warn!("w4");
        tracing::info!("i4");
    });
    let out = writer.captured();
    assert!(
        out.contains("w4"),
        "an invalid directive must still let warnings reach the writer; got: {out}"
    );
    assert!(
        !out.contains("i4"),
        "an invalid directive must fall back to WARN, not admit info; got: {out}"
    );
}

#[test]
fn span_close_line_survives_the_filter_split() {
    // Proves `FmtSpan::CLOSE` (src/lib.rs) is set on both branches of
    // `build_subscriber`, not just the `EnvFilter` one -- ADR-9's read-back
    // of `SCAP_LOG=debug` close lines depends on it.
    let writer = BufWriter::new();
    tracing::subscriber::with_default(build_subscriber(Some("debug"), writer.clone()), || {
        let span = tracing::debug_span!("w1_close_probe");
        let entered = span.enter();
        drop(entered);
        drop(span);
    });
    let out = writer.captured();
    assert!(
        out.contains("w1_close_probe") && out.contains("close"),
        "expected a FmtSpan::CLOSE line for the debug span; got: {out}"
    );
}
