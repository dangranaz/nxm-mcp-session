//! Integration tests for wiring the write-side MCP tools onto the global
//! session registry (`~/nxm/sessions/registry.jsonl`).
//!
//! Unlike the old version these tests no longer mutate the process-global
//! `HOME`/cwd/`NXM_SESSION_PATH`: they build [`SessionPaths`],
//! [`SessionContext`] and [`ResolveEnv`] from temp dirs and call
//! `nxm-session-core` directly. Each test is therefore hermetic and the
//! suite runs without a global mutex.

use std::io::Write;
use std::sync::OnceLock;
use tempfile::TempDir;

use nxm_session_core::{
    global_registry, handler, handoff, protocol, registry, sampling::Capabilities,
    session_reader::ResolveEnv, transport::Transport, SessionContext, SessionPaths,
};
use serde_json::{json, Value};

/// Ensures the tracing subscriber is installed at most once per test binary.
static LOGGER: OnceLock<()> = OnceLock::new();

fn init_logger_once() {
    LOGGER.get_or_init(|| {
        nxm_shared::logging::init("");
    });
}

/// An isolated fixture: temp `HOME` (global registry lands under
/// `<home>/nxm/sessions`), temp repo root with a `.git` (repo root resolution
/// + project identity), and the resolved structs handed to core.
struct Fixture {
    _home: TempDir,
    _repo: TempDir,
    paths: SessionPaths,
    ctx: SessionContext,
    env: ResolveEnv,
}

fn isolated() -> Fixture {
    init_logger_once();

    let home = TempDir::new().expect("home");
    let repo = TempDir::new().expect("repo");
    let sessions = repo.path().join("sessions");
    let archive = repo.path().join("archive");
    std::fs::create_dir_all(repo.path().join(".git")).expect(".git");
    std::fs::create_dir_all(&sessions).expect("sessions");

    let paths = SessionPaths {
        sessions_dir: sessions.clone(),
        archive_root: archive.clone(),
        repo_root: repo.path().to_path_buf(),
        global_sessions_dir: home.path().join("nxm").join("sessions"),
        archived_sessions_dir: home.path().join("nxm").join("sessions").join("archived"),
    };
    let ctx = SessionContext {
        no_elicit: false,
        workspace: Some(repo.path().to_string_lossy().into_owned()),
    };
    let env = ResolveEnv {
        home: Some(home.path().to_string_lossy().into_owned()),
        agent_session_id: None,
    };

    Fixture {
        _home: home,
        _repo: repo,
        paths,
        ctx,
        env,
    }
}

/// A transport whose reader yields a single `accept` elicitation response.
fn accept_transport(
) -> Transport<std::io::BufReader<std::io::Cursor<Vec<u8>>>, std::io::BufWriter<Vec<u8>>> {
    let response = br#"{"jsonrpc":"2.0","id":"srv-1","result":{"action":"accept"}}"#.to_vec();
    let mut bytes = response;
    bytes.push(b'\n');
    Transport::new(
        std::io::BufReader::new(std::io::Cursor::new(bytes)),
        std::io::BufWriter::new(Vec::new()),
    )
}

fn caps() -> Capabilities {
    Capabilities {
        sampling: false,
        elicitation: true,
    }
}

/// Extract the inner JSON object from the tool's MCP content envelope (for Value results).
fn inner(result: &Value) -> Value {
    let text = result
        .get("content")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|b| b.get("text"))
        .and_then(|t| t.as_str())
        .expect("content[0].text");
    serde_json::from_str(text).expect("inner json")
}

/// Extract the inner JSON object from the tool's MCP content envelope (for JsonRpcResponse).
fn inner_response(result: &protocol::JsonRpcResponse) -> Value {
    let result_value = serde_json::to_value(result).expect("serialize response");
    let text = result_value
        .get("result")
        .and_then(|v| v.get("content"))
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|b| b.get("text"))
        .and_then(|t| t.as_str())
        .expect("content[0].text");
    serde_json::from_str(text).expect("inner json")
}

fn write_test_handoff(fx: &Fixture) -> String {
    let dir = &fx.paths.sessions_dir;
    let meta = registry::HandoffMeta {
        title: "Test Session".to_string(),
        short_description: "desc".to_string(),
        goal_next: Some("goal".to_string()),
        created: registry::now_iso(),
        source_archive: None,
        workspace: fx.ctx.workspace.clone(),
        context_usage_pct: None,
        trigger: "manual".to_string(),
        skills_involved: vec![],
        updated_at: None,
        revision: 0,
    };
    let entry = registry::write_handoff(dir, &meta, "# Test\n").expect("write_handoff");
    entry.id
}

#[test]
fn generate_handoff_appends_unconsumed_global_record() {
    let fx = isolated();
    let args = json!({ "goal_next": "Test goal", "conversation": "line one\nline two" });
    let now = registry::now_iso();
    let mut transport = accept_transport();

    let result = handoff::generate_handoff(&mut transport, caps(), &args, &now, &fx.paths, &fx.ctx);
    assert!(
        result.get("error").is_none(),
        "unexpected error: {result:?}"
    );
    let id = inner(&result)
        .get("id")
        .and_then(|v| v.as_str())
        .expect("id")
        .to_string();

    let records = global_registry::read_records(&fx.paths.global_sessions_dir).expect("read");
    assert_eq!(records.len(), 1, "one global record expected");
    assert_eq!(records[0].id, id);
    assert_eq!(records[0].stato, global_registry::Status::Unconsumed);
    assert_eq!(
        records[0].path_sessioni_locali,
        fx.paths.sessions_dir.to_string_lossy()
    );
}

#[test]
fn update_session_appends_unconsumed_global_record() {
    let fx = isolated();
    let id = write_test_handoff(&fx);
    let args = json!({ "id": id, "goal_next": "Updated", "conversation": "a\nb" });
    let now = registry::now_iso();
    let mut transport = accept_transport();

    let result = handoff::update_session(&mut transport, caps(), &args, &now, &fx.paths, &fx.ctx);
    assert!(
        result.get("error").is_none(),
        "unexpected error: {result:?}"
    );

    let records = global_registry::read_records(&fx.paths.global_sessions_dir).expect("read");
    assert_eq!(records.len(), 1, "update appends one global record");
    assert_eq!(records[0].id, id);
    assert_eq!(records[0].stato, global_registry::Status::Unconsumed);
}

#[test]
fn mark_consumed_appends_consumed_global_record() {
    let fx = isolated();
    let id = write_test_handoff(&fx);
    let params =
        json!({ "name": "mark_consumed", "arguments": { "id": id, "consumed_by": "test" } });
    let response = handler::handle_tools_call(
        Some(Value::String("t".to_string())),
        &params,
        &fx.paths,
        &fx.env,
    );
    assert!(
        response.error.is_none(),
        "mark_consumed error: {:?}",
        response.error
    );

    let records = global_registry::read_records(&fx.paths.global_sessions_dir).expect("read");
    assert_eq!(records.len(), 1, "mark_consumed appends one global record");
    assert_eq!(records[0].id, id);
    assert_eq!(records[0].stato, global_registry::Status::Consumed);
}

// ===== Read-side tests (task 05) =====

#[test]
fn list_global_sessions_returns_empty_when_no_registry() {
    let fx = isolated();
    let params = json!({ "name": "list_global_sessions", "arguments": {} });
    let response = handler::handle_tools_call(
        Some(Value::String("t".to_string())),
        &params,
        &fx.paths,
        &fx.env,
    );
    assert!(
        response.error.is_none(),
        "list_global_sessions error: {:?}",
        response.error
    );
    let inner_obj = inner_response(&response);
    let sessions = inner_obj
        .get("sessions")
        .and_then(|v| v.as_array())
        .expect("sessions array");
    assert_eq!(sessions.len(), 0, "empty sessions when no registry");
    let session_number = inner_obj
        .get("session_number")
        .and_then(|v| v.as_u64())
        .expect("session_number");
    assert_eq!(session_number, 0, "session_number is 0 when empty");
}

#[test]
fn list_global_sessions_returns_pointers_with_session_number() {
    let fx = isolated();
    // Create two global records via the write-side tools
    let now = registry::now_iso();
    let mut transport = accept_transport();
    let args1 = json!({ "goal_next": "Test goal 1", "conversation": "line one" });
    let result1 =
        handoff::generate_handoff(&mut transport, caps(), &args1, &now, &fx.paths, &fx.ctx);
    assert!(result1.get("error").is_none());
    let id1 = inner(&result1)
        .get("id")
        .and_then(|v| v.as_str())
        .expect("id1")
        .to_string();

    let mut transport = accept_transport();
    let args2 = json!({ "goal_next": "Test goal 2", "conversation": "line two" });
    let result2 =
        handoff::generate_handoff(&mut transport, caps(), &args2, &now, &fx.paths, &fx.ctx);
    assert!(result2.get("error").is_none());
    let _id2 = inner(&result2)
        .get("id")
        .and_then(|v| v.as_str())
        .expect("id2")
        .to_string();

    // Mark one as consumed (adds a third record)
    let params =
        json!({ "name": "mark_consumed", "arguments": { "id": id1, "consumed_by": "test" } });
    let response = handler::handle_tools_call(
        Some(Value::String("t".to_string())),
        &params,
        &fx.paths,
        &fx.env,
    );
    assert!(response.error.is_none());

    // Now call list_global_sessions - should see 3 records (2 unconsumed + 1 consumed)
    let params = json!({ "name": "list_global_sessions", "arguments": {} });
    let response = handler::handle_tools_call(
        Some(Value::String("t".to_string())),
        &params,
        &fx.paths,
        &fx.env,
    );
    assert!(
        response.error.is_none(),
        "list_global_sessions error: {:?}",
        response.error
    );
    let inner_obj = inner_response(&response);
    let sessions = inner_obj
        .get("sessions")
        .and_then(|v| v.as_array())
        .expect("sessions array");
    assert_eq!(
        sessions.len(),
        3,
        "three global records expected (2 unconsumed + 1 consumed)"
    );

    // Check structure of returned pointers
    for s in sessions {
        assert!(s.get("id").is_some(), "has id");
        assert!(s.get("progetto").is_some(), "has progetto");
        assert!(s.get("repository").is_some(), "has repository");
        assert!(
            s.get("path_sessioni_locali").is_some(),
            "has path_sessioni_locali"
        );
        assert!(s.get("stato").is_some(), "has stato");
        assert!(s.get("timestamp").is_some(), "has timestamp");
    }

    // Check session_number = count of records
    let session_number = inner_obj
        .get("session_number")
        .and_then(|v| v.as_u64())
        .expect("session_number");
    assert_eq!(session_number, 3, "session_number equals record count");
}

#[test]
fn list_global_sessions_skips_corrupt_line() {
    let fx = isolated();
    // Create one valid record first using write-side tool
    let now = registry::now_iso();
    let mut transport = accept_transport();
    let args = json!({ "goal_next": "Test goal", "conversation": "line one" });
    let result = handoff::generate_handoff(&mut transport, caps(), &args, &now, &fx.paths, &fx.ctx);
    assert!(result.get("error").is_none());
    let id = inner(&result)
        .get("id")
        .and_then(|v| v.as_str())
        .expect("id")
        .to_string();

    // Corrupt the global registry file by appending a bad line
    let file_path = fx.paths.global_sessions_dir.join("registry.jsonl");
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
        .expect("open")
        .write_all(b"not valid json\n")
        .expect("write corrupt");

    // list_global_sessions should still work and skip the corrupt line
    let params = json!({ "name": "list_global_sessions", "arguments": {} });
    let response = handler::handle_tools_call(
        Some(Value::String("t".to_string())),
        &params,
        &fx.paths,
        &fx.env,
    );
    assert!(
        response.error.is_none(),
        "list_global_sessions error: {:?}",
        response.error
    );
    let inner_obj = inner_response(&response);
    let sessions = inner_obj
        .get("sessions")
        .and_then(|v| v.as_array())
        .expect("sessions array");
    assert_eq!(
        sessions.len(),
        1,
        "only valid record returned, corrupt line skipped"
    );
    assert_eq!(
        sessions[0].get("id").and_then(|v| v.as_str()),
        Some(id.as_str())
    );
    let session_number = inner_obj
        .get("session_number")
        .and_then(|v| v.as_u64())
        .expect("session_number");
    assert_eq!(
        session_number, 1,
        "session_number counts only valid records"
    );
}
