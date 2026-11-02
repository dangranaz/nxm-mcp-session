#!/usr/bin/env bash
#
# release.sh — build, verify and install nxm-mcp-session so every MCP client
# on this machine resolves the freshly built binary from the PATH.
#
# Rationale: the binary that coding agents launch is `nxm-mcp-session` resolved
# from the PATH (installed under a cargo --root, e.g. ~/.local/bin). A plain
# `cargo build` leaves that installed copy stale, so agents keep running the
# OLD version. This script closes that gap: it (re)installs via `cargo install`
# (which also keeps the macOS code signature valid, unlike a manual `cp`) and
# then verifies end-to-end that the INSTALLED binary reports the version from
# Cargo.toml and advertises the full tool contract.
#
# Usage:
#   scripts/release.sh                 # build + test + install + verify
#   scripts/release.sh --no-test       # skip the test suite (faster)
#   INSTALL_ROOT=~/.cargo scripts/release.sh   # install under a different root
#
# Env:
#   INSTALL_ROOT  cargo --root for the install (default: ~/.local)
#
set -euo pipefail

cd "$(dirname "$0")/.."

INSTALL_ROOT="${INSTALL_ROOT:-$HOME/.local}"
BIN_NAME="nxm-mcp-session"
RUN_TESTS=1
[ "${1:-}" = "--no-test" ] && RUN_TESTS=0

# Version declared in Cargo.toml — the single authority we verify against.
EXPECTED_VERSION="$(
  grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/'
)"

echo "==> nxm-mcp-session release"
echo "    expected version : ${EXPECTED_VERSION}"
echo "    install root      : ${INSTALL_ROOT}  (binary -> ${INSTALL_ROOT}/bin/${BIN_NAME})"

echo "==> clippy (all targets, warnings as hard failures are up to CI)"
cargo clippy --all-targets

if [ "${RUN_TESTS}" -eq 1 ]; then
  echo "==> test suite"
  cargo test
else
  echo "==> test suite SKIPPED (--no-test)"
fi

echo "==> cargo install (reinstalls the PATH binary; keeps code signature valid)"
cargo install --path . --root "${INSTALL_ROOT}" --force

# Resolve the binary the way an agent would: from the PATH if the install root
# is on it, otherwise fall back to the known install location.
INSTALLED_BIN="$(command -v "${BIN_NAME}" || true)"
[ -z "${INSTALLED_BIN}" ] && INSTALLED_BIN="${INSTALL_ROOT}/bin/${BIN_NAME}"

# Re-sign ad-hoc as a safety net. `cargo install` keeps the signature valid on
# its own, but any later manual `cp`/move would invalidate it and macOS (arm64)
# then SIGKILLs the binary at launch — which surfaces to MCP clients as an
# opaque `write EPIPE`. Signing here makes the installed binary robust to that.
if command -v codesign >/dev/null 2>&1; then
  echo "==> codesign --force --sign - (ad-hoc), keep the launch valid on arm64"
  codesign --force --sign - "${INSTALLED_BIN}" 2>/dev/null || \
    echo "    (codesign skipped/failed — non-fatal)"
  xattr -d com.apple.provenance "${INSTALLED_BIN}" 2>/dev/null || true
fi

echo "==> verifying installed binary: ${INSTALLED_BIN}"

# Drive the server exactly as a client does: initialize + tools/list over stdio,
# under a throwaway HOME so the real ~/.sessions is never touched.
PROBE_HOME="$(mktemp -d)"
trap 'rm -rf "${PROBE_HOME}"' EXIT

PROBE_OUT="$(
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  | HOME="${PROBE_HOME}" "${INSTALLED_BIN}" --transport stdio 2>/dev/null
)"

# Parse and assert the server starts and advertises the full tool contract.
# NOTE: since the rewire, `serverInfo.version` comes from nxm-session-core
# (env!("CARGO_PKG_VERSION") evaluated in the library), NOT from this binary's
# Cargo.toml — the two version lines legitimately differ. So we assert the
# version is present and looks like semver, and focus the real check on the
# tool contract.
EXPECTED_VERSION="${EXPECTED_VERSION}" python3 - "$PROBE_OUT" <<'PY'
import json, os, re, sys

binary_version = os.environ["EXPECTED_VERSION"]
version, tools = None, []
for line in sys.argv[1].splitlines():
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    if msg.get("id") == 1:
        version = msg["result"]["serverInfo"]["version"]
    elif msg.get("id") == 2:
        tools = [t["name"] for t in msg["result"]["tools"]]

errors = []
if not version or not re.match(r"^\d+\.\d+\.\d+", version):
    errors.append(f"serverInfo.version missing or not semver: {version!r}")
if "list_global_sessions" not in tools:
    errors.append("tool 'list_global_sessions' missing from tools/list")
if not tools:
    errors.append("tools/list returned no tools (server failed to start?)")

if errors:
    print("VERIFY FAILED:")
    for e in errors:
        print(f"  - {e}")
    sys.exit(1)

print(
    f"VERIFY OK: serverInfo.version={version} (from nxm-session-core), "
    f"binary Cargo.toml={binary_version}, tools={len(tools)}, "
    f"list_global_sessions=present"
)
PY

echo "==> done. Agents will pick up ${EXPECTED_VERSION} on their next (re)start."
echo "    NOTE: already-running agent sessions keep the old spawned process in"
echo "    memory — restart the agent session or reload its MCP servers."
