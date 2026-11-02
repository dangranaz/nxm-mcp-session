//! Wiring between the product config/env and `nxm-session-core`.
//!
//! The single place where the binary reads `nxm-config` (`[session]` via
//! `Config.extra` — schema deliberately NOT extended) and the process
//! environment (`HOME`, `PWD`, `AGENT_SESSION_ID`/`KIRO_SESSION_ID`,
//! walk-up `.git` from the cwd), applies the missing-key defaults itself, and
//! hands core the final, already-resolved [`SessionPaths`],
//! [`SessionContext`] and [`ResolveEnv`]. Core never reads the environment.

use nxm_config::{load_from_path_with_env, product_config_path, ProcessEnv};
use nxm_session_core::{session_reader::ResolveEnv, SessionContext, SessionPaths};
use std::path::{Path, PathBuf};

/// The resolved config handed to core: paths, runtime flags, reader env.
pub struct Wiring {
    pub paths: SessionPaths,
    pub ctx: SessionContext,
    pub env: ResolveEnv,
}

impl Default for Wiring {
    /// Degraded fallback when the product config cannot be read.
    ///
    /// Emits an `eprintln!` warning so the operator knows the server is running
    /// with explicit default paths instead of the configured ones. Paths default
    /// to `sessions/` and `archive/` relative to the current working directory,
    /// global registries fall back to `.nxm/nxm-session` under the cwd.
    ///
    /// Uses `eprintln!` for the fallback warning because in the degraded path
    /// the logging subsystem may not yet be initialized (see the degraded-path
    /// handling in `main.rs`).
    fn default() -> Self {
        eprintln!(
            "warning: using degraded wiring fallback (product config unavailable); \
            sessions_dir=sessions/, archive_root=archive/, global_dir=.nxm/nxm-session"
        );
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            paths: SessionPaths {
                sessions_dir: cwd.join("sessions"),
                archive_root: cwd.join("archive"),
                repo_root: cwd.clone(),
                global_sessions_dir: cwd.join(".nxm").join("nxm-session"),
                archived_sessions_dir: cwd.join(".nxm").join("nxm-session").join("archived"),
            },
            ctx: SessionContext::default(),
            env: ResolveEnv::default(),
        }
    }
}

/// Pure resolver: builds `Wiring` from already-parsed config and injected
/// environment values. No I/O, no `std::env`, no filesystem access beyond
/// pure path manipulation on the provided inputs.
///
/// This is the single authority for the config→struct mapping.
/// `load_config_with` is a thin wrapper that performs the ambient I/O
/// (`current_dir`, `product_config_path`, `load_from_path_with_env`)
/// and then delegates here.
pub fn build_wiring(
    session_extra: Option<&toml::Value>,
    cwd: &Path,
    home: Option<String>,
    agent_session_id: Option<String>,
    workspace: Option<String>,
) -> anyhow::Result<Wiring> {
    let sessions_dir_raw = session_extra
        .and_then(|v| v.get("sessions_dir"))
        .and_then(|v| v.as_str());
    let archive_root_raw = session_extra
        .and_then(|v| v.get("archive_root"))
        .and_then(|v| v.as_str());
    let no_elicit = session_extra
        .and_then(|v| v.get("no_elicit"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Ok(Wiring {
        paths: SessionPaths {
            sessions_dir: resolve_path(sessions_dir_raw, cwd.join("sessions"), cwd)?,
            archive_root: resolve_path(archive_root_raw, cwd.join("archive"), cwd)?,
            repo_root: find_repo_root(cwd).unwrap_or_else(|| cwd.to_path_buf()),
            global_sessions_dir: global_dir(cwd, home.as_deref()),
            archived_sessions_dir: global_dir(cwd, home.as_deref()).join("archived"),
        },
        ctx: SessionContext {
            no_elicit,
            workspace,
        },
        env: ResolveEnv {
            home,
            agent_session_id,
        },
    })
}

/// Resolve the `nxm-session` product config from the process environment and
/// return the structs core expects. Entry point used by `main`.
///
/// `home`, `agent_session_id` and `workspace` are resolved by the caller
/// (the binary) *before* `init_product_logging` runs, because that shared
/// logging entry point may reset the process environment. Core receives the
/// final, already-resolved values.
pub fn load_config_with(
    home: Option<String>,
    agent_session_id: Option<String>,
    workspace: Option<String>,
) -> anyhow::Result<Wiring> {
    let cwd = std::env::current_dir()?;
    let config_path = product_config_path("nxm-session")?;
    let (config, _provenance) = load_from_path_with_env(&config_path, &ProcessEnv)?;
    let session_extra = config.extra.get("session");
    build_wiring(session_extra, &cwd, home, agent_session_id, workspace)
}

fn resolve_path(raw: Option<&str>, default: PathBuf, cwd: &Path) -> anyhow::Result<PathBuf> {
    let path = match raw {
        Some(s) if !s.is_empty() => PathBuf::from(s),
        _ => default,
    };
    Ok(if path.is_relative() {
        cwd.join(path)
    } else {
        path
    })
}

/// Walk up from `dir` looking for a `.git` directory; the first dir that has
/// one is the repo root. Returns `None` if none is found up to the filesystem
/// root.
fn find_repo_root(mut dir: &Path) -> Option<PathBuf> {
    loop {
        if dir.join(".git").is_dir() {
            return Some(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent,
            _ => return None,
        }
    }
}

/// Base dir for the cross-repo (global) registries: `~/nxm/sessions` when
/// `HOME` is known, else a `.nxm/nxm-session` fallback under the cwd.
fn global_dir(cwd: &Path, home: Option<&str>) -> PathBuf {
    home.map(|h| Path::new(h).join("nxm").join("sessions"))
        .unwrap_or_else(|| cwd.join(".nxm").join("nxm-session"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nxm_config::{load_from_path_with_env, EnvLookup};

    struct FixedEnv(std::collections::HashMap<String, String>);
    impl EnvLookup for FixedEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    #[test]
    fn build_wiring_populates_structs_from_toml() {
        let home = tempfile::TempDir::new().expect("tempdir for HOME");
        let cwd = tempfile::TempDir::new().expect("tempdir for cwd");
        std::fs::create_dir_all(cwd.path().join(".git")).expect(".git fixture");
        let sessions = cwd.path().join("sessions");
        let archive = cwd.path().join("archive");
        let toml = format!(
            r#"[session]
sessions_dir = "{}"
archive_root = "{}"
no_elicit = true
"#,
            sessions.display(),
            archive.display()
        );
        let config_path = cwd.path().join("nxm-session.toml");
        std::fs::write(&config_path, &toml).expect("write fixture toml");

        let mut env_map = std::collections::HashMap::new();
        env_map.insert(
            "HOME".to_string(),
            home.path().to_string_lossy().into_owned(),
        );
        env_map.insert("AGENT_SESSION_ID".to_string(), "ses_agent_1".to_string());

        let (config, _) =
            load_from_path_with_env(&config_path, &FixedEnv(env_map)).expect("load fixture config");
        let session_extra = config.extra.get("session");

        let wiring = build_wiring(
            session_extra,
            cwd.path(),
            Some(home.path().to_string_lossy().into_owned()),
            Some("ses_agent_1".to_string()),
            Some(cwd.path().to_string_lossy().into_owned()),
        )
        .expect("build_wiring");

        assert_eq!(wiring.paths.sessions_dir, sessions);
        assert_eq!(wiring.paths.archive_root, archive);
        assert_eq!(wiring.paths.repo_root, cwd.path());
        assert_eq!(
            wiring.paths.global_sessions_dir,
            home.path().join("nxm").join("sessions")
        );
        assert_eq!(
            wiring.paths.archived_sessions_dir,
            home.path().join("nxm").join("sessions").join("archived")
        );
        assert_eq!(wiring.paths.prompts_dir(), cwd.path().join("prompts"));
        assert!(wiring.ctx.no_elicit);
        assert_eq!(
            wiring.ctx.workspace,
            Some(cwd.path().to_string_lossy().into_owned())
        );
        assert_eq!(
            wiring.env.home,
            Some(home.path().to_string_lossy().into_owned())
        );
        assert_eq!(wiring.env.agent_session_id, Some("ses_agent_1".to_string()));
    }

    #[test]
    fn build_wiring_applies_defaults_when_session_keys_missing() {
        let home = tempfile::TempDir::new().expect("tempdir for HOME");
        let cwd = tempfile::TempDir::new().expect("tempdir for cwd");
        std::fs::create_dir_all(cwd.path().join(".git")).expect(".git fixture");
        std::fs::write(
            cwd.path().join("nxm-session.toml"),
            "[session]\nno_elicit = false\n",
        )
        .expect("write fixture toml");

        let (config, _) = load_from_path_with_env(
            &cwd.path().join("nxm-session.toml"),
            &FixedEnv(std::collections::HashMap::new()),
        )
        .expect("load fixture config");
        let session_extra = config.extra.get("session");

        let wiring = build_wiring(
            session_extra,
            cwd.path(),
            Some(home.path().to_string_lossy().into_owned()),
            None,
            Some(cwd.path().to_string_lossy().into_owned()),
        )
        .expect("build_wiring");

        assert_eq!(wiring.paths.sessions_dir, cwd.path().join("sessions"));
        assert_eq!(wiring.paths.archive_root, cwd.path().join("archive"));
        assert!(!wiring.ctx.no_elicit);
        assert_eq!(
            wiring.paths.global_sessions_dir,
            home.path().join("nxm").join("sessions")
        );
        assert_eq!(
            wiring.env.home,
            Some(home.path().to_string_lossy().into_owned())
        );
        assert_eq!(wiring.env.agent_session_id, None);
    }

    #[test]
    fn degraded_wiring_has_explicit_nonempty_defaults() {
        // The fallback Wiring::default() must not be silent and must have
        // explicit, non-empty paths (not PathBuf::default()).
        let wiring = Wiring::default();
        assert!(!wiring.paths.sessions_dir.as_os_str().is_empty());
        assert!(!wiring.paths.archive_root.as_os_str().is_empty());
        assert!(!wiring.paths.repo_root.as_os_str().is_empty());
        assert!(!wiring.paths.global_sessions_dir.as_os_str().is_empty());
        assert!(!wiring.paths.archived_sessions_dir.as_os_str().is_empty());
        // Defaults point to cwd-relative locations.
        assert!(wiring.paths.sessions_dir.ends_with("sessions"));
        assert!(wiring.paths.archive_root.ends_with("archive"));
        assert!(wiring
            .paths
            .global_sessions_dir
            .ends_with(".nxm/nxm-session"));
    }
}
