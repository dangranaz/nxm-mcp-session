//! End-to-end test: drive the built binary over stdio like a real MCP client.
//!
//! The binary now reads product config from `~/.config/nxm/nxm-session.toml`
//! (`nxm-config`, `[session]` block via `Config.extra`), so each helper writes
//! that file into a per-call isolated `HOME` before spawning the binary.
//! `NXM_SESSION_PATH` / `NXM_SESSION_ARCHIVE_ROOT` are translated into the
//! `sessions_dir` / `archive_root` keys of that file.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use tempfile::TempDir;

/// Crea una HOME isolata (per il logging/data-dir) e una `XDG_CONFIG_HOME`
/// isolata (per `nxm-config`, che usa `dirs::home_dir()` a meno che
/// `XDG_CONFIG_HOME` non sia impostata). Scrive il product config sotto
/// `XDG_CONFIG_HOME/nxm/nxm-session.toml` puntando `sessions_dir` a
/// `sessions`. Restituisce entrambe le `TempDir` (vive finché in scope).
fn home_with_config(sessions: &Path) -> (TempDir, TempDir) {
    let home = TempDir::new().expect("tempdir for HOME");
    let xdg = TempDir::new().expect("tempdir for XDG_CONFIG_HOME");
    let config_dir = xdg.path().join("nxm");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    let toml = format!(
        "[session]\nsessions_dir = {}\narchive_root = {}\n",
        serde_json::to_string(sessions).expect("serialize sessions_dir"),
        serde_json::to_string(&format!("{}/archive", sessions.display()))
            .expect("serialize archive_root")
    );
    std::fs::write(config_dir.join("nxm-session.toml"), &toml).expect("write product config");
    (home, xdg)
}

fn run_session(input: &str) -> String {
    run_session_env(input, &[])
}

/// Like [`run_session`], but sets extra environment variables on the child so
/// tests can point `NXM_SESSION_PATH` at a scratch directory. `HOME` is a
/// fresh temp dir per call so parallel tests never share config or the
/// global registry.
fn run_session_env(input: &str, env: &[(&str, &str)]) -> String {
    // `HOME` drives `product_data_dir` (logging) and `env.home`; `XDG_CONFIG_HOME`
    // drives `nxm-config`'s `product_config_path` (it uses `dirs::home_dir()`,
    // i.e. the real home unless `XDG_CONFIG_HOME` is set), so the product config
    // is written under `XDG_CONFIG_HOME` and never touches the real home.
    let home = TempDir::new().expect("tempdir for HOME");
    let xdg = TempDir::new().expect("tempdir for XDG_CONFIG_HOME");
    let config_dir = xdg.path().join("nxm");
    std::fs::create_dir_all(&config_dir).expect("config dir");

    let mut sessions: Option<String> = None;
    let mut archive_root: Option<String> = None;
    for (k, v) in env {
        match *k {
            "NXM_SESSION_PATH" => sessions = Some(v.to_string()),
            "NXM_SESSION_ARCHIVE_ROOT" => archive_root = Some(v.to_string()),
            _ => {}
        }
    }
    let sessions_dir = sessions.unwrap_or_else(|| {
        let d = TempDir::new().expect("tempdir for sessions");
        let p = d.path().to_string_lossy().into_owned();
        std::mem::forget(d);
        p
    });
    // `archive_root` defaults to `<sessions>/archive` so archive tests land
    // next to the sessions dir; an explicit `NXM_SESSION_ARCHIVE_ROOT` wins.
    let archive_root = archive_root.unwrap_or_else(|| format!("{}/archive", sessions_dir));

    let mut toml = String::from("[session]\n");
    toml.push_str(&format!(
        "sessions_dir = {}\n",
        serde_json::to_string(&sessions_dir).expect("serialize sessions_dir")
    ));
    toml.push_str(&format!(
        "archive_root = {}\n",
        serde_json::to_string(&archive_root).expect("serialize archive_root")
    ));
    std::fs::write(config_dir.join("nxm-session.toml"), &toml).expect("write product config");

    // `nxm-config` resolves the product config via `dirs::home_dir()` unless
    // `XDG_CONFIG_HOME` is set, so we pin both `HOME` and `XDG_CONFIG_HOME`
    // into the child environment (the shared logging entry point may reset
    // the process env).
    // Isolate the child working directory so `repo_root` walk-up and any
    // relative path defaults land in a temp dir without `.git`, never in the
    // repo root. The TempDir lives until `wait_with_output()` returns, i.e.
    // for the entire lifetime of the child process.
    let cwd = TempDir::new().expect("tempdir for child cwd");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nxm-mcp-session"));
    cmd.current_dir(cwd.path());
    cmd.env("HOME", home.path());
    cmd.env("XDG_CONFIG_HOME", xdg.path());
    for (k, v) in env {
        match *k {
            "NXM_SESSION_PATH" | "NXM_SESSION_ARCHIVE_ROOT" => {} // already in config toml
            _ => {
                cmd.env(k, v);
            }
        }
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn server");

    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");
    String::from_utf8(output.stdout).expect("utf8 stdout")
}

#[test]
fn initialize_and_list_all_tools() {
    let input = "\
{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}
";
    let out = run_session(input);
    assert!(
        out.contains("\"nxm-mcp-session\""),
        "serverInfo missing: {out}"
    );

    // Parse the tools/list response (id 2) and count the advertised tools.
    let mut names: Vec<String> = Vec::new();
    for line in out.lines() {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("id") == Some(&serde_json::json!(2)) {
            names = v["result"]["tools"]
                .as_array()
                .expect("tools array")
                .iter()
                .map(|t| t["name"].as_str().expect("tool name").to_string())
                .collect();
        }
    }
    let mut expected: Vec<String> = nxm_session_core::handler::TOOL_NAMES
        .iter()
        .map(|s| s.to_string())
        .collect();
    names.sort();
    expected.sort();
    assert_eq!(names, expected, "tools/list must match TOOL_NAMES exactly");
}

#[test]
fn update_session_upserts_existing_session_over_stdio() {
    // No capabilities declared → no Sampling, no Elicitation; the explicit
    // `conversation` argument is the material source (trigger "manual").
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();

    // Seed the registry with one existing session and its handoff stub —
    // the state a previous generate_handoff leaves behind.
    let registry = r#"{
  "schema": "nxm-session-registry/v1",
  "sessions": [
    {
      "id": "ses_update_stdio",
      "short_description": "seeded",
      "status": "unconsumed",
      "created": "2026-10-02T09:00:00+02:00",
      "consumed_at": null,
      "consumed_by": null,
      "file": "ses_update_stdio.md"
    }
  ]
}"#;
    std::fs::write(sessions.join("registry.json"), registry).unwrap();
    std::fs::write(sessions.join("ses_update_stdio.md"), "original v0 body\n").unwrap();

    let input = "\
{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{}}}
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"update_session\",\"arguments\":{\"id\":\"ses_update_stdio\",\"goal_next\":\"finish the update wiring\",\"conversation\":\"we moved to the update path; new state v1\"}}}
";
    let out = run_session_env(input, &[("NXM_SESSION_PATH", sessions.to_str().unwrap())]);
    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    // The result payload reports the bumped revision.
    assert!(
        out.contains("\\\"revision\\\":1"),
        "expected revision 1: {out}"
    );

    // Upsert-per-id: the SAME file was rewritten, no new entry created.
    let reg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sessions.join("registry.json")).unwrap())
            .unwrap();
    let entries = reg["sessions"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "registry must stay at one entry: {reg}");
    assert_eq!(entries[0]["id"], serde_json::json!("ses_update_stdio"));
    assert_eq!(entries[0]["revision"], serde_json::json!(1));
    assert!(
        entries[0]["updated_at"].is_string(),
        "updated_at not set: {reg}"
    );

    let md = std::fs::read_to_string(sessions.join("ses_update_stdio.md")).unwrap();
    assert!(md.contains("new state v1"), "md: {md}");
    assert!(!md.contains("original v0 body"), "md not rewritten: {md}");
    assert!(md.contains("revision: 1"), "md: {md}");
    // Exactly one handoff document on disk.
    let md_count = std::fs::read_dir(&sessions)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
        .count();
    assert_eq!(md_count, 1, "expected one handoff .md");
}

#[test]
fn audit_last_task_fallback_returns_structured_audit_over_stdio() {
    // No capabilities declared → no Sampling. With material (task_ref + diff)
    // the tool returns a minimal, structured four-section audit (source =
    // "fallback"), NOT an error and NOT "not implemented yet".
    let input = "\
{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{}}}
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"audit_last_task\",\"arguments\":{\"task_ref\":\"refactor auth\",\"diff\":\"@@ -1 +1 @@\"}}}
";
    let out = run_session(input);
    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    assert!(
        !out.contains("not implemented yet"),
        "must not be stubbed: {out}"
    );
    assert!(
        out.contains("\\\"source\\\":\\\"fallback\\\""),
        "expected fallback source: {out}"
    );
    assert!(
        out.contains("code-review"),
        "expected code-review suggestion: {out}"
    );
    // The four §4 headings are present in the audit body.
    assert!(out.contains("Fragilit"), "missing fragility section: {out}");
    assert!(
        out.contains("Disallineamenti dai requisiti"),
        "missing mismatch section: {out}"
    );
    assert!(
        out.contains("Debito tecnico introdotto"),
        "missing debt section: {out}"
    );
    assert!(
        out.contains("Verifica suggerita"),
        "missing verify section: {out}"
    );
}

#[test]
fn audit_last_task_no_material_no_sampling_is_error_over_stdio() {
    // No capabilities and no material → cannot audit → isError (no panic).
    let input = "\
{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{}}}
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"audit_last_task\",\"arguments\":{}}}
";
    let out = run_session(input);
    assert!(out.contains("\"isError\":true"), "expected error: {out}");
    assert!(out.contains("needs material"), "expected message: {out}");
    assert!(
        !out.contains("not implemented yet"),
        "must not be stubbed: {out}"
    );
}

#[test]
fn audit_last_task_sampling_returns_model_audit_over_stdio() {
    // Declare sampling, then answer the server's sampling/createMessage request
    // interactively. The model's four-section audit comes back with source =
    // "sampling".
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    let dir = tempfile::tempdir().unwrap();
    let (home, xdg) = home_with_config(dir.path());
    let cwd = TempDir::new().expect("tempdir for child cwd");
    let mut child = Command::new(env!("CARGO_BIN_EXE_nxm-mcp-session"))
        .current_dir(cwd.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");

    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    writeln!(
        stdin,
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"capabilities\":{{\"sampling\":{{}}}}}}}}"
    )
    .unwrap();
    writeln!(
        stdin,
        "{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{{\"name\":\"audit_last_task\",\"arguments\":{{\"diff\":\"@@ -1 +1 @@\"}}}}}}"
    )
    .unwrap();
    stdin.flush().unwrap();

    let mut final_result: Option<String> = None;
    let mut line = String::new();
    while final_result.is_none() {
        line.clear();
        let n = reader.read_line(&mut line).expect("read");
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(trimmed).expect("json line");
        if v.get("id") == Some(&serde_json::json!(1)) && v.get("result").is_some() {
            continue;
        }
        if v.get("id") == Some(&serde_json::json!(2)) && v.get("result").is_some() {
            final_result = Some(trimmed.to_string());
            continue;
        }
        if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
            assert_eq!(
                method, "sampling/createMessage",
                "unexpected request: {method}"
            );
            let id = v.get("id").cloned().unwrap();
            let reply = serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "result": {
                    "role": "assistant",
                    "content": { "type": "text",
                        "text": "## Fragilità potenziali\n- timeout non gestito\n## Disallineamenti dai requisiti\n- nessuno rilevato\n## Debito tecnico introdotto\n- TODO in x.ts:1\n## Verifica suggerita\n- Eseguire la skill `code-review` sul diff." },
                    "model": "test", "stopReason": "endTurn"
                }
            });
            writeln!(stdin, "{}", serde_json::to_string(&reply).unwrap()).unwrap();
            stdin.flush().unwrap();
        }
    }

    drop(stdin);
    let _ = child.wait();

    let out = final_result.expect("final tools/call result");
    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    assert!(
        out.contains("\\\"source\\\":\\\"sampling\\\""),
        "expected sampling source: {out}"
    );
    assert!(
        out.contains("Fragilit"),
        "expected model audit surfaced: {out}"
    );
    assert!(
        out.contains("code-review"),
        "expected code-review skill: {out}"
    );
}

#[test]
fn generate_handoff_fallback_writes_document_over_stdio() {
    // No client capabilities declared (empty initialize) → no Sampling, no
    // Elicitation. With an explicit `conversation` the tool falls back to it,
    // writing the handoff without any server→client request.
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");

    let input = "\
{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{}}}
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"generate_handoff\",\"arguments\":{\"goal_next\":\"finish the wiring\",\"conversation\":\"we decided to go async; middleware skeleton done\"}}}
";
    let out = run_session_env(input, &[("NXM_SESSION_PATH", sessions.to_str().unwrap())]);
    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    assert!(
        out.contains("\\\"status\\\":\\\"active\\\""),
        "expected status: {out}"
    );

    // A handoff file and a registry entry now exist.
    let registry = std::fs::read_to_string(sessions.join("registry.json")).unwrap();
    assert!(
        registry.contains("\"status\": \"active\""),
        "registry: {registry}"
    );
    // Exactly one .md handoff document was written.
    let md_count = std::fs::read_dir(&sessions)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
        .count();
    assert_eq!(md_count, 1, "expected one handoff .md");
}

#[test]
fn generate_handoff_no_material_is_error_and_writes_nothing() {
    // No capabilities and no conversation → cannot gather material → isError.
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");

    let input = "\
{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{}}}
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"generate_handoff\",\"arguments\":{}}}
";
    let out = run_session_env(input, &[("NXM_SESSION_PATH", sessions.to_str().unwrap())]);
    assert!(out.contains("\"isError\":true"), "expected error: {out}");
    assert!(out.contains("needs material"), "expected message: {out}");
    // Nothing written.
    assert!(
        !sessions.join("registry.json").exists(),
        "registry should not exist"
    );
}

#[test]
fn generate_handoff_sampling_and_elicitation_accept_over_stdio() {
    // Drive the full server→client round-trip interactively: declare sampling +
    // elicitation, then answer the server's requests as they arrive on stdout.
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let (home, xdg) = home_with_config(&sessions);
    let cwd = TempDir::new().expect("tempdir for child cwd");
    let mut child = Command::new(env!("CARGO_BIN_EXE_nxm-mcp-session"))
        .current_dir(cwd.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .env("NXM_SESSION_PATH", sessions.to_str().unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");

    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    // initialize (declares sampling + elicitation), then call generate_handoff.
    writeln!(
        stdin,
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"capabilities\":{{\"sampling\":{{}},\"elicitation\":{{}}}}}}}}"
    )
    .unwrap();
    writeln!(
        stdin,
        "{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{{\"name\":\"generate_handoff\",\"arguments\":{{\"goal_next\":\"wire async validation\"}}}}}}"
    )
    .unwrap();
    stdin.flush().unwrap();

    // Read server output lines; answer server→client requests by their id.
    let mut final_result: Option<String> = None;
    let mut line = String::new();
    while final_result.is_none() {
        line.clear();
        let n = reader.read_line(&mut line).expect("read");
        if n == 0 {
            break; // EOF
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(trimmed).expect("json line");

        // The initialize response (id 1) — ignore.
        if v.get("id") == Some(&serde_json::json!(1)) && v.get("result").is_some() {
            continue;
        }
        // The final tools/call response (id 2).
        if v.get("id") == Some(&serde_json::json!(2)) && v.get("result").is_some() {
            final_result = Some(trimmed.to_string());
            continue;
        }
        // A server→client request: reply by method.
        if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
            let id = v.get("id").cloned().unwrap();
            let reply = match method {
                "sampling/createMessage" => serde_json::json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": {
                        "role": "assistant",
                        "content": { "type": "text",
                            "text": "## Goal\nwire async\n## Decisions\n- async\n## Failed approaches\n- cache\n## Last task\ndone skeleton\n## Relevant files\n1. src/x.ts\n## Next steps\n1. [ ] finish" },
                        "model": "test", "stopReason": "endTurn"
                    }
                }),
                "elicitation/create" => serde_json::json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": { "action": "accept", "content": { "confirm": true } }
                }),
                other => panic!("unexpected server request: {other}"),
            };
            writeln!(stdin, "{}", serde_json::to_string(&reply).unwrap()).unwrap();
            stdin.flush().unwrap();
        }
    }

    // Close stdin so the server loop ends, then reap.
    drop(stdin);
    let _ = child.wait();

    let out = final_result.expect("final tools/call result");
    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    assert!(
        out.contains("wire async"),
        "expected material surfaced: {out}"
    );

    // The document and registry entry exist.
    let registry = std::fs::read_to_string(sessions.join("registry.json")).unwrap();
    assert!(
        registry.contains("\"status\": \"active\""),
        "registry: {registry}"
    );
}

#[test]
fn list_sessions_on_empty_dir_returns_empty_list() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let call = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"list_sessions\",\"arguments\":{}}}\n";
    let out = run_session_env(call, &[("NXM_SESSION_PATH", sessions.to_str().unwrap())]);
    // Success envelope with an empty sessions array.
    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    assert!(
        out.contains("\\\"sessions\\\":[]"),
        "expected empty list: {out}"
    );
}

#[test]
fn get_session_unknown_id_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let call = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"get_session\",\"arguments\":{\"id\":\"ses_missing\"}}}\n";
    let out = run_session_env(call, &[("NXM_SESSION_PATH", sessions.to_str().unwrap())]);
    assert!(
        out.contains("\"isError\":true"),
        "expected error envelope: {out}"
    );
    assert!(
        out.contains("not found"),
        "expected not-found message: {out}"
    );
}

#[test]
fn get_session_missing_id_arg_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let call = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"get_session\",\"arguments\":{}}}\n";
    let out = run_session_env(call, &[("NXM_SESSION_PATH", sessions.to_str().unwrap())]);
    assert!(
        out.contains("\"isError\":true"),
        "expected error envelope: {out}"
    );
    assert!(
        out.contains("missing required argument"),
        "expected arg error: {out}"
    );
}

#[test]
fn mark_consumed_updates_registry_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();

    // Seed a registry with one unconsumed session and its handoff file.
    let registry = r#"{
  "schema": "nxm-session-registry/v1",
  "sessions": [
    {
      "id": "ses_seed",
      "short_description": "seeded",
      "status": "unconsumed",
      "created": "2026-09-29T09:20:00+02:00",
      "consumed_at": null,
      "consumed_by": null,
      "file": "ses_seed.md"
    }
  ]
}"#;
    std::fs::write(sessions.join("registry.json"), registry).unwrap();
    std::fs::write(
        sessions.join("ses_seed.md"),
        "---\nid: ses_seed\nstatus: unconsumed\nconsumed_at: null\nconsumed_by: null\n---\n\n# body\n",
    )
    .unwrap();

    let call = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"mark_consumed\",\"arguments\":{\"id\":\"ses_seed\",\"consumed_by\":\"ses_new\"}}}\n";
    let out = run_session_env(call, &[("NXM_SESSION_PATH", sessions.to_str().unwrap())]);
    assert!(out.contains("\"isError\":false"), "expected success: {out}");

    // Registry and frontmatter both reflect the consumption.
    let reg = std::fs::read_to_string(sessions.join("registry.json")).unwrap();
    assert!(
        reg.contains("\"status\": \"superseded\""),
        "registry: {reg}"
    );
    assert!(
        reg.contains("\"consumed_by\": \"ses_new\""),
        "registry: {reg}"
    );

    let md = std::fs::read_to_string(sessions.join("ses_seed.md")).unwrap();
    assert!(md.contains("status: superseded"), "md: {md}");
    assert!(md.contains("consumed_by: \"ses_new\""), "md: {md}");
}

#[test]
fn find_resumable_session_hint_matches_unconsumed_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();

    // Two entries: a consumed one (strong match) and an unconsumed one.
    let registry = r#"{
  "schema": "nxm-session-registry/v1",
  "sessions": [
    {
      "id": "ses_auth_done",
      "short_description": "refactor auth jwt middleware",
      "status": "consumed",
      "created": "2026-09-29T12:00:00+02:00",
      "consumed_at": "2026-09-29T12:30:00+02:00",
      "consumed_by": "ses_x",
      "file": "ses_auth_done.md"
    },
    {
      "id": "ses_auth_open",
      "short_description": "refactor auth jwt validation",
      "status": "unconsumed",
      "created": "2026-09-29T09:00:00+02:00",
      "consumed_at": null,
      "consumed_by": null,
      "file": "ses_auth_open.md"
    }
  ]
}"#;
    std::fs::write(sessions.join("registry.json"), registry).unwrap();

    let call = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"find_resumable_session\",\"arguments\":{\"hint\":\"continue the auth jwt work\"}}}\n";
    let out = run_session_env(call, &[("NXM_SESSION_PATH", sessions.to_str().unwrap())]);

    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    // The unconsumed match is returned, not the consumed one.
    assert!(
        out.contains("ses_auth_open"),
        "expected open session: {out}"
    );
    assert!(!out.contains("ses_auth_done"), "consumed leaked in: {out}");
    assert!(
        out.contains("\\\"found\\\":true"),
        "expected found flag: {out}"
    );
}

#[test]
fn find_resumable_session_empty_registry_is_non_error_message() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");

    let call = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"find_resumable_session\",\"arguments\":{}}}\n";
    let out = run_session_env(call, &[("NXM_SESSION_PATH", sessions.to_str().unwrap())]);

    // Absence of candidates is not an error.
    assert!(
        out.contains("\"isError\":false"),
        "expected non-error: {out}"
    );
    assert!(
        out.contains("nessuna sessione riprendibile"),
        "expected 'no resumable session' message: {out}"
    );
}

/// Run the server with an explicit, *clean* environment for the session-reader
/// path: `sessions_path` is written into the product config
/// (`$HOME/.config/nxm/nxm-session.toml`), and `$AGENT_SESSION_ID` /
/// `$KIRO_SESSION_ID` / `HOME` controlled (removed unless provided) so tests
/// never inherit the runner's real values.
fn run_session_reader_env(
    input: &str,
    sessions_path: &str,
    agent_session_id: Option<&str>,
    kiro_session_id: Option<&str>,
    home: Option<&str>,
) -> String {
    // `home` esplicito quando il test vuole controllarlo; altrimenti una
    // TempDir interna (viva finché la funzione è in scope, quindi per tutta
    // la durata del child process).
    let home = match home {
        Some(h) => h.to_string(),
        None => {
            let d = TempDir::new().expect("tempdir for HOME");
            let p = d.path().to_string_lossy().into_owned();
            std::mem::forget(d);
            p
        }
    };
    let xdg = TempDir::new().expect("tempdir for XDG_CONFIG_HOME");
    let config_dir = xdg.path().join("nxm");
    std::fs::create_dir_all(&config_dir).expect("config dir");
    let toml = format!(
        "[session]\nsessions_dir = {}\narchive_root = {}\n",
        serde_json::to_string(sessions_path).expect("serialize sessions_path"),
        serde_json::to_string(&format!("{}/archive", sessions_path))
            .expect("serialize archive_root")
    );
    std::fs::write(config_dir.join("nxm-session.toml"), &toml).expect("write product config");

    let cwd = TempDir::new().expect("tempdir for child cwd");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nxm-mcp-session"));
    cmd.current_dir(cwd.path());
    cmd.env("HOME", &home);
    cmd.env("XDG_CONFIG_HOME", xdg.path());
    cmd.env_remove("AGENT_SESSION_ID");
    cmd.env_remove("KIRO_SESSION_ID");
    if let Some(id) = agent_session_id {
        cmd.env("AGENT_SESSION_ID", id);
    }
    if let Some(id) = kiro_session_id {
        cmd.env("KIRO_SESSION_ID", id);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn server");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    String::from_utf8(output.stdout).expect("utf8 stdout")
}

// (d) archive_session with session_file: the raw is saved IDENTICAL to source
// in sessions/archive/, 0600 on unix, archive_file recorded; source =
// "session_file".
#[test]
fn archive_session_via_session_file_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");

    // A raw Kiro-style .jsonl session log on disk.
    let raw = "{\"version\":\"v1\",\"kind\":\"Prompt\",\"data\":{\"text\":\"hi\"}}\n\
               {\"version\":\"v1\",\"kind\":\"AssistantMessage\",\"data\":{\"text\":\"yo\"}}\n";
    let session_file = dir.path().join("some_session.jsonl");
    std::fs::write(&session_file, raw).unwrap();

    let call = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"archive_session\",\"arguments\":{{\"session_file\":{}}}}}}}\n",
        serde_json::to_string(session_file.to_str().unwrap()).unwrap()
    );
    // No KIRO_SESSION_ID: session_file must still win (it is highest priority).
    let out = run_session_reader_env(&call, sessions.to_str().unwrap(), None, None, None);

    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    assert!(
        out.contains("\\\"source\\\":\\\"session_file\\\""),
        "expected session_file source: {out}"
    );

    // Archive file exists with byte-identical content.
    let archive = sessions.join("archive").join("ses_some_session.raw.jsonl");
    assert!(archive.exists(), "archive missing: {}", archive.display());
    assert_eq!(std::fs::read_to_string(&archive).unwrap(), raw);

    // Registry records the archive_file on a (created) entry.
    let reg = std::fs::read_to_string(sessions.join("registry.json")).unwrap();
    assert!(
        reg.contains("archive/ses_some_session.raw.jsonl"),
        "archive_file not recorded: {reg}"
    );

    // 0600 on unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&archive).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "expected 0600, got {:o}", mode & 0o777);
    }
}

// KIRO_SESSION_ID path: reader resolves <home>/.kiro/sessions/cli/<id>.jsonl.
#[test]
fn archive_session_via_kiro_env_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let home = dir.path().join("home");
    let cli_dir = home.join(".kiro").join("sessions").join("cli");
    std::fs::create_dir_all(&cli_dir).unwrap();

    let session_id = "ses_kiroenv";
    let raw = "{\"version\":\"v1\",\"kind\":\"Prompt\",\"data\":{}}\n";
    std::fs::write(cli_dir.join(format!("{session_id}.jsonl")), raw).unwrap();

    let call = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"archive_session\",\"arguments\":{}}}\n";
    let out = run_session_reader_env(
        call,
        sessions.to_str().unwrap(),
        None,             // agent_session_id
        Some(session_id), // kiro_session_id
        Some(home.to_str().unwrap()),
    );

    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    assert!(
        out.contains("\\\"source\\\":\\\"kiro\\\""),
        "expected kiro source: {out}"
    );
    let archive = sessions
        .join("archive")
        .join(format!("{session_id}.raw.jsonl"));
    assert!(archive.exists(), "archive missing: {}", archive.display());
    assert_eq!(std::fs::read_to_string(&archive).unwrap(), raw);
}

// (e) fallback conversation (STEP 7a) still works: source = "conversation".
#[test]
fn archive_session_conversation_fallback_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();

    // Seed an existing registry entry so this mirrors the STEP 7a flow exactly.
    let registry = r#"{
  "schema": "nxm-session-registry/v1",
  "sessions": [
    {
      "id": "ses_conv",
      "short_description": "seeded",
      "status": "unconsumed",
      "created": "2026-09-29T09:20:00+02:00",
      "consumed_at": null,
      "consumed_by": null,
      "file": "ses_conv.md"
    }
  ]
}"#;
    std::fs::write(sessions.join("registry.json"), registry).unwrap();

    let raw = "{\"role\":\"user\"}\n{\"role\":\"assistant\"}\n";
    let call = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"archive_session\",\"arguments\":{{\"id\":\"ses_conv\",\"conversation\":{}}}}}}}\n",
        serde_json::to_string(raw).unwrap()
    );
    // No KIRO_SESSION_ID → the Kiro reader is skipped → conversation fallback.
    let out = run_session_reader_env(&call, sessions.to_str().unwrap(), None, None, None);

    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    assert!(
        out.contains("\\\"source\\\":\\\"conversation\\\""),
        "expected conversation source: {out}"
    );
    let archive = sessions.join("archive").join("ses_conv.raw.jsonl");
    assert!(archive.exists(), "archive missing: {}", archive.display());
    assert_eq!(std::fs::read_to_string(&archive).unwrap(), raw);
}

// (f) AGENT_SESSION_ID as primary env var works over stdio.
#[test]
fn archive_session_via_agent_env_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let home = dir.path().join("home");
    let cli_dir = home.join(".kiro").join("sessions").join("cli");
    std::fs::create_dir_all(&cli_dir).unwrap();
    let session_id = "ses_agentenv";
    let raw = "{\"version\":\"v1\",\"kind\":\"Prompt\",\"data\":{}}\n";
    std::fs::write(cli_dir.join(format!("{session_id}.jsonl")), raw).unwrap();
    let call = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"archive_session\",\"arguments\":{}}}\n";
    let out = run_session_reader_env(
        call,
        sessions.to_str().unwrap(),
        Some(session_id), // AGENT_SESSION_ID primary
        None,             // no KIRO
        Some(home.to_str().unwrap()),
    );
    assert!(out.contains("\"isError\":false"), "{out}");
    assert!(out.contains("\\\"source\\\":\\\"kiro\\\""), "{out}");
    assert!(sessions
        .join("archive")
        .join(format!("{session_id}.raw.jsonl"))
        .exists());
}

#[test]
fn create_session_mints_key_and_registers_active_entry_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");

    let input = "\
{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{}}}
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"create_session\",\"arguments\":{\"slug\":\"specdec\"}}}
";
    let out = run_session_env(input, &[("NXM_SESSION_PATH", sessions.to_str().unwrap())]);
    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    // The minted key and the initial status come back to the caller.
    assert!(
        out.contains("\\\"key\\\":\\\"specdec-01\\\""),
        "expected minted key: {out}"
    );
    assert!(
        out.contains("\\\"status\\\":\\\"active\\\""),
        "expected status: {out}"
    );

    // The registry now holds the Active entry under the key…
    let reg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sessions.join("registry.json")).unwrap())
            .unwrap();
    let entries = reg["sessions"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "registry: {reg}");
    assert_eq!(entries[0]["id"], serde_json::json!("specdec-01"));
    assert_eq!(entries[0]["status"], serde_json::json!("active"));
    assert_eq!(entries[0]["file"], serde_json::json!("specdec-01.md"));

    // …and the minimal handoff document was written next to it.
    assert!(
        sessions.join("specdec-01.md").exists(),
        "handoff <key>.md missing"
    );
}

#[test]
fn register_prompt_indexes_prompt_on_session_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();

    // 1) create_session mints the key, 2) register_prompt indexes a brief on it.
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_session","arguments":{"slug":"specdec"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"register_prompt","arguments":{"key":"specdec-01","task_no":"01","slug":"multirow","lang":"ita","path":"specdec-01_01-multirow_ITA.md"}}}"#,
        "\n",
    );
    let out = run_session_env(input, &[("NXM_SESSION_PATH", sessions.to_str().unwrap())]);

    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    assert!(
        out.contains("specdec-01"),
        "expected minted key in output: {out}"
    );

    // The registry now carries the prompt metadata on the session entry.
    let registry = std::fs::read_to_string(sessions.join("registry.json")).unwrap();
    assert!(
        registry.contains("\"prompts\""),
        "prompts field missing: {registry}"
    );
    assert!(
        registry.contains("multirow"),
        "prompt slug missing: {registry}"
    );
    assert!(
        registry.contains("specdec-01_01-multirow_ITA.md"),
        "prompt path missing: {registry}"
    );
}

#[test]
fn set_status_done_archives_and_drops_from_list_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let archive = dir.path().join("archive_root");
    std::fs::create_dir_all(&sessions).unwrap();

    // create_session → set_status done → list_sessions (default view).
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_session","arguments":{"slug":"closed"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"set_status","arguments":{"key":"closed-01","status":"done"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_sessions","arguments":{}}}"#,
        "\n",
    );
    let out = run_session_env(
        input,
        &[
            ("NXM_SESSION_PATH", sessions.to_str().unwrap()),
            ("NXM_SESSION_ARCHIVE_ROOT", archive.to_str().unwrap()),
        ],
    );

    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    // The done session is gone from the active registry (default list empty of it).
    let registry = std::fs::read_to_string(sessions.join("registry.json")).unwrap();
    assert!(
        !registry.contains("closed-01"),
        "archived session still in registry: {registry}"
    );
    // The handoff was physically moved under the archive root.
    assert!(
        archive.join("closed-01").join("closed-01.md").exists()
            || archive
                .read_dir()
                .map(|mut d| d.next().is_some())
                .unwrap_or(false),
        "archive root should contain the moved session"
    );
}

#[test]
fn reopen_superseded_returns_session_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let archive = dir.path().join("archive_root");
    std::fs::create_dir_all(&sessions).unwrap();

    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_session","arguments":{"slug":"revive"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"set_status","arguments":{"key":"revive-01","status":"superseded","consumed_by":"revive-02"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"reopen","arguments":{"key":"revive-01"}}}"#,
        "\n",
    );
    let out = run_session_env(
        input,
        &[
            ("NXM_SESSION_PATH", sessions.to_str().unwrap()),
            ("NXM_SESSION_ARCHIVE_ROOT", archive.to_str().unwrap()),
        ],
    );

    assert!(out.contains("\"isError\":false"), "expected success: {out}");
    // After reopen the session is back in the active registry as active, and
    // its handoff file is back in the live sessions dir.
    let registry = std::fs::read_to_string(sessions.join("registry.json")).unwrap();
    assert!(
        registry.contains("revive-01"),
        "reopened session missing: {registry}"
    );
    assert!(
        sessions.join("revive-01.md").exists(),
        "handoff not returned to live tree"
    );
}

// R1 — dopo UN giro tipico (initialize + create_session) nessun artefatto
// di sessione/registry compare nella root del repo: tutto resta nella
// sessions dir isolata e nella cwd temporanea del figlio. Copre un singolo
// giro, non l'intera suite.
#[test]
fn repo_root_stays_clean_after_stdio_suite() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();

    let input = "\
{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{}}}
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"create_session\",\"arguments\":{\"slug\":\"specdec\"}}}
";
    let out = run_session_env(input, &[("NXM_SESSION_PATH", sessions.to_str().unwrap())]);
    assert!(out.contains("\"isError\":false"), "expected success: {out}");

    // L'artefatto deve comparire solo nella sessions dir isolata.
    assert!(
        sessions.join("specdec-01.md").exists(),
        "handoff missing in sessions dir"
    );
    assert!(
        sessions.join("registry.json").exists(),
        "registry missing in sessions dir"
    );

    // E mai nella root del repo.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(
        !root.join("registry.json").exists(),
        "registry.json leaked to repo root"
    );
    assert!(
        !root.join("registry.jsonl").exists(),
        "registry.jsonl leaked to repo root"
    );
    assert!(
        !root.join("specdec-01.md").exists(),
        "specdec-01.md leaked to repo root"
    );
}
