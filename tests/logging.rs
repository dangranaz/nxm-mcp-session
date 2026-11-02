//! Integration tests for the shared logging adoption in `main()`.
//!
//! Spawns the real `nxm-mcp-session` binary (like a real MCP client
//! with immediate stdin EOF) and verifies the `product_data_dir`
//! precedence (global-first, from nxm-shared):
//!
//! 1. `$NXM_SESSION_WORKSPACE/.nxm` when the env var is set (explicit
//!    workspace scoping);
//! 2. `$HOME/.nxm/nxm-session` when `HOME` is set and the env var isn't;
//! 3. `.nxm/nxm-session` relative to the CWD when `HOME` is unset too.
//!
//! NOTE: this is GLOBAL-first, not cwd-first. A server launched from a
//! workspace logs into `~/.nxm/nxm-session` unless `NXM_SESSION_WORKSPACE`
//! points it at a workspace. (Changed 2026-10-07 when adopting
//! `product_data_dir` from the promoted nxm-shared rev.)
//!
//! Plus the degraded path: when the file sink cannot be created, the
//! process must keep running and still emit structured logs via
//! `nxm_shared::logging::init` (RULES.md § Logging — logging must
//! never abort the process).
//!
//! NOTE for reviewers: since the promoted nxm-shared rev (2e18023),
//! the shared `init()` degraded sink is **stderr** (not stdout) — the
//! "send structured records to stderr" fix. Correct for a stdio MCP
//! server (stdout is the JSON-RPC stream). The degraded-path assertions
//! therefore check stderr for the structured event.
//!
//! Tests are isolated: `HOME` is pinned to `TempDir`s (or removed)
//! per child process, so tests never touch the real `~/.nxm`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// Spawn the binary with an isolated environment (no `RUST_LOG`, pinned
/// or removed `HOME`), immediate EOF on stdin,
/// and wait for exit. Returns `(stdout, stderr)` so tests can assert on
/// the degraded path's structured output (the shared `init()` sink is
/// stdout).
fn spawn_isolated(home: Option<&Path>, cwd: &Path) -> (String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nxm-mcp-session"));
    cmd.env_remove("RUST_LOG"); // let the default env filter drive
    cmd.current_dir(cwd);
    match home {
        Some(h) => {
            cmd.env("HOME", h);
        }
        None => {
            cmd.env_remove("HOME");
        }
    }
    let output = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn nxm-mcp-session");
    assert!(
        output.status.success(),
        "nxm-mcp-session exited with {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Assert that `log_dir` holds at least one daily `.log` file
/// containing both the `init_product_logging` startup breadcrumb and
/// the process startup event.
fn assert_log_populated(log_dir: &Path) {
    let entries: Vec<PathBuf> = fs::read_dir(log_dir)
        .unwrap_or_else(|e| panic!("log dir {log_dir:?} not created: {e}"))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "log"))
        .collect();
    assert!(!entries.is_empty(), "no .log files in {log_dir:?}");
    let joined = entries
        .iter()
        .map(|p| fs::read_to_string(p).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("logging initialised"),
        "init breadcrumb not found in {log_dir:?}"
    );
    assert!(
        joined.contains("nxm-mcp-session starting"),
        "startup event not found in {log_dir:?}"
    );
}

#[test]
fn logs_land_in_workspace_env_when_set() {
    // Precedence 1: $NXM_SESSION_WORKSPACE/.nxm wins when the env var is set,
    // regardless of HOME. (product_data_dir is global-first; the workspace env
    // is the explicit opt-in to workspace-scoped logs.)
    let home = TempDir::new().expect("tempdir for HOME");
    let ws = TempDir::new().expect("tempdir for workspace");
    let cwd = TempDir::new().expect("tempdir for cwd");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nxm-mcp-session"));
    cmd.env_remove("RUST_LOG");
    cmd.current_dir(cwd.path());
    cmd.env("HOME", home.path());
    cmd.env("NXM_SESSION_WORKSPACE", ws.path());
    let output = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn nxm-mcp-session");
    assert!(output.status.success(), "exited with {:?}", output.status);

    let log_dir = ws.path().join(".nxm").join("logs");
    assert_log_populated(&log_dir);
    assert!(
        !home.path().join(".nxm").join("nxm-session").exists(),
        "HOME fallback used despite NXM_SESSION_WORKSPACE being set"
    );
}

#[test]
fn logs_land_in_home_fallback_when_no_cwd_nxm() {
    // Precedence 2: no cwd/.nxm → $HOME/.nxm/nxm-session
    let home = TempDir::new().expect("tempdir for HOME");
    let cwd = TempDir::new().expect("tempdir for cwd");
    // Ensure cwd/.nxm does NOT exist
    let (_stdout, _stderr) = spawn_isolated(Some(home.path()), cwd.path());

    let log_dir = home.path().join(".nxm").join("nxm-session").join("logs");
    assert_log_populated(&log_dir);
    assert!(
        !cwd.path().join(".nxm").join("logs").exists(),
        "cwd-based logging used despite HOME being set and no cwd/.nxm"
    );
}

#[test]
fn logs_land_in_cwd_nxm_session_when_no_home() {
    // Precedence 3: no cwd/.nxm and no HOME → `.nxm/nxm-session`
    // relative to the CWD.
    let cwd = TempDir::new().expect("tempdir for cwd");
    let (_stdout, _stderr) = spawn_isolated(None, cwd.path());

    let log_dir = cwd.path().join(".nxm").join("nxm-session").join("logs");
    assert_log_populated(&log_dir);
}

#[test]
fn degraded_path_keeps_structured_logging() {
    // Force `init_product_logging` to fail: HOME points to a path
    // whose parent is a regular file, so the log directory cannot be
    // created. The process must still exit 0, warn on stderr,
    // AND emit the structured startup event.
    //
    // Since the promoted nxm-shared rev (2e18023, "send structured records
    // to stderr, not stdout"), the degraded `init()` sink is STDERR — correct
    // for a stdio MCP server, where stdout is reserved for the JSON-RPC
    // stream. So we assert the structured event on stderr, not stdout.
    let tmp = TempDir::new().expect("tempdir");
    let blocker = tmp.path().join("blocker");
    fs::write(&blocker, "not a directory").expect("write blocker file");
    let bad_home = blocker.join("home");
    let cwd = TempDir::new().expect("tempdir for cwd");
    let (_stdout, stderr) = spawn_isolated(Some(&bad_home), cwd.path());

    assert!(
        stderr.contains("file logging unavailable"),
        "degradation warning missing: {stderr}"
    );
    assert!(
        stderr.contains("nxm::session::mcp"),
        "structured startup event missing on stderr: {stderr}"
    );
    assert!(
        stderr.contains("nxm-mcp-session starting"),
        "startup message missing on stderr: {stderr}"
    );
}
