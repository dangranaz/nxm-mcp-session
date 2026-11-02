//! nxm-mcp-session — thin binary wrapper around `nxm-session-core`.
//!
//! The binary owns the process: product config/env resolution (`wiring`),
//! logging setup via `nxm-shared` (never in core), and the stdio JSON-RPC
//! loop. All session logic lives in `nxm-session-core`; this target has no
//! library target and no duplicated core modules.

mod wiring;

use std::io::{self, BufRead, Write};

use nxm_session_core::{
    audit, handler, handoff,
    protocol::{codes, JsonRpcRequest, JsonRpcResponse},
    registry,
    sampling::Capabilities,
    transport::{InboundLine, Transport},
};
use wiring::Wiring;

fn main() {
    // Capture the ambient environment BEFORE `init_product_logging`, which may
    // reset the process environment (e.g. `HOME`). The binary owns config/env
    // resolution; core receives the final, already-resolved values.
    let home = std::env::var("HOME").ok().filter(|s| !s.is_empty());
    let agent_session_id = std::env::var("AGENT_SESSION_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("KIRO_SESSION_ID")
                .ok()
                .filter(|s| !s.is_empty())
        });
    let workspace = std::env::var("PWD").ok().filter(|s| !s.is_empty());

    // Mandatory shared logging (RULES.md § Logging): persist to the product
    // data dir (`<data_dir>/logs/nxm-session.<YYYY-MM-DD>.log`, daily rolling)
    // AND stderr, via the shared `nxm-shared` utilities — never roll our own.
    // Data-dir resolution mirrors `nxm-webtomd` / `nxm-memory`:
    // `NXM_SESSION_WORKSPACE/.nxm` wins when set, else `$HOME/.nxm/nxm-session`,
    // else `.nxm/nxm-session` relative to the CWD. The returned `WorkerGuard`
    // MUST stay alive for the whole process (it flushes the non-blocking
    // writer on drop), so we bind it here in `main`.
    let data_dir = nxm_shared::logging::product_data_dir("nxm-session", "NXM_SESSION_WORKSPACE");
    let _log_guard = match nxm_shared::logging::init_product_logging(
        "nxm-session",
        &data_dir,
        "nxm_mcp_session=info,nxm=info,nexum=info",
    ) {
        Ok(g) => Some(g),
        Err(e) => {
            // Degraded path: logging must never abort the process. Keep the
            // human-readable warning, then install the structured subscriber
            // via the shared `init` with the same env filter, so every
            // subsequent event (starting with the breadcrumb below) is still
            // captured in structured form.
            eprintln!("warning: file logging unavailable ({e}); continuing with stderr only");
            nxm_shared::logging::init("nxm_mcp_session=info,nxm=info,nexum=info");
            None
        }
    };

    let wiring =
        match wiring::load_config_with(home.clone(), agent_session_id.clone(), workspace.clone()) {
            Ok(w) => w,
            Err(e) => {
                // A missing/unreadable product config must not abort the server
                // (degraded path, mirroring the logging degraded path): fall back
                // to empty/default paths and keep serving so the process stays
                // alive. Core receives final values; the binary owns the defaults.
                eprintln!(
                    "warning: failed to load product config ({e}); continuing with default paths"
                );
                Wiring::default()
            }
        };

    nxm_shared::tracing::info!(
        target: "nxm::session::mcp",
        "nxm-mcp-session starting (JSON-RPC over stdio)"
    );

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut transport = Transport::new(stdin.lock(), stdout.lock());
    run(&mut transport, &wiring);
}

/// The main loop, generic over the transport so it can be driven in tests.
///
/// Drains any messages buffered by an in-flight server→client request first,
/// then reads fresh client messages. Each parsed request is dispatched and its
/// response written back; parse failures produce a parse-error response with a
/// null id, exactly as before.
fn run<R: BufRead, W: Write>(transport: &mut Transport<R, W>, wiring: &Wiring) {
    let mut caps = Capabilities::default();
    loop {
        let inbound = match transport.next_inbound() {
            Ok(Some(inbound)) => inbound,
            Ok(None) => break, // EOF
            Err(e) => {
                eprintln!("{e}");
                break;
            }
        };

        let response = match inbound {
            InboundLine::ParseError { error, .. } => Some(JsonRpcResponse::error(
                None,
                codes::PARSE_ERROR,
                format!("parse error: {error}"),
            )),
            InboundLine::Parsed(value) => match serde_json::from_value::<JsonRpcRequest>(value) {
                Ok(request) => handle_request(transport, &mut caps, &request, wiring),
                Err(e) => Some(JsonRpcResponse::error(
                    None,
                    codes::PARSE_ERROR,
                    format!("parse error: {e}"),
                )),
            },
        };

        // Notifications return None and must not be answered.
        if let Some(response) = response {
            if let Err(e) = transport.send_response(&response) {
                eprintln!("{e}");
            }
        }
    }
}

/// Route one request: capture capabilities on `initialize`, hand the
/// transport-driven `generate_handoff`, `update_session` and `audit_last_task`
/// tools to [`handoff`] / [`audit`], and defer everything else to the pure
/// [`handler::dispatch`], injecting the resolved [`SessionPaths`],
/// [`SessionContext`] and [`ResolveEnv`].
fn handle_request<R: BufRead, W: Write>(
    transport: &mut Transport<R, W>,
    caps: &mut Capabilities,
    request: &JsonRpcRequest,
    wiring: &Wiring,
) -> Option<JsonRpcResponse> {
    // Capture the client's capabilities as soon as they arrive.
    if request.method == "initialize" {
        if let Some(c) = request.params.get("capabilities") {
            *caps = Capabilities::from_client_capabilities(c);
        }
    }

    // Intercept `tools/call` for the three transport-driven tools; they need
    // the transport (Sampling/Elicitation). Notifications carry no id.
    if request.method == "tools/call" && request.id.is_some() {
        let name = request
            .params
            .get("name")
            .and_then(serde_json::Value::as_str);
        if name == Some("generate_handoff") {
            let args = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let now = registry::now_iso();
            let result = handoff::generate_handoff(
                transport,
                *caps,
                &args,
                &now,
                &wiring.paths,
                &wiring.ctx,
            );
            return Some(JsonRpcResponse::ok(request.id.clone(), result));
        }
        if name == Some("update_session") {
            let args = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let now = registry::now_iso();
            let result =
                handoff::update_session(transport, *caps, &args, &now, &wiring.paths, &wiring.ctx);
            return Some(JsonRpcResponse::ok(request.id.clone(), result));
        }
        if name == Some("audit_last_task") {
            let args = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let result = audit::audit_last_task(transport, *caps, &args);
            return Some(JsonRpcResponse::ok(request.id.clone(), result));
        }
    }

    handler::dispatch(request, &wiring.paths, &wiring.env)
}
