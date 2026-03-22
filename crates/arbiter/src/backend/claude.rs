//! Claude Code CLI adapter (`claude` binary).
//!
//! Builds args from BackendOpts, spawns process, parses JSON response.
//! Uses --add-dir for workspace, --permission-mode plan for ask mode.

use super::Adapter;
use crate::types::{BackendOp, BackendOpts, BackendResult, OnComplete, OnStream};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

/// Claude Code CLI adapter. Binary name: `claude`.
#[derive(Debug)]
pub(crate) struct ClaudeAdapter {
    config: crate::backend::BackendConfig,
}

impl ClaudeAdapter {
    /// Creates a new Claude adapter with the given config.
    pub(crate) fn new(config: crate::backend::BackendConfig) -> Self {
        Self { config }
    }

    /// Builds CLI args from BackendOpts.
    fn build_args(&self, opts: &BackendOpts) -> Vec<String> {
        let fmt = if opts.stream {
            "stream-json".to_string()
        } else {
            "json".to_string()
        };
        let mut args = vec![
            "-p".to_string(),
            opts.prompt.clone(),
            "--output-format".to_string(),
            fmt,
        ];

        match &opts.op {
            BackendOp::Resume(sid) => {
                args.push("--resume".to_string());
                args.push(sid.clone());
            }
            BackendOp::ContinueLatest => args.push("--continue".to_string()),
            BackendOp::NewSession => {}
        }
        if opts.ask_mode {
            args.push("--permission-mode".to_string());
            args.push("plan".to_string());
        }
        if opts.stream {
            args.push("--include-partial-messages".to_string());
        }
        if let Some(schema) = opts.json_schema.as_ref() {
            args.push("--json-schema".to_string());
            args.push(schema.clone());
        }
        if let Some(model) = self.config.model.as_ref() {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        if let Some(dir) = self.config.workspace.as_ref() {
            args.push("--add-dir".to_string());
            args.push(dir.clone());
        }
        args.extend(self.config.extra_args.iter().cloned());
        args
    }
}

use super::response::parse_json_response;

#[derive(Debug, serde::Deserialize)]
struct StreamEvent {
    #[serde(default)]
    event: String,
    text: Option<String>,
    #[serde(default)]
    session_id: String,
}

impl Adapter for ClaudeAdapter {
    fn execute(&self, opts: BackendOpts, on_stream: Option<OnStream>, callback: OnComplete) {
        let args = self.build_args(&opts);
        let config = self.config.clone();

        thread::spawn(move || {
            let mut cmd = Command::new("claude");
            cmd.args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            if let Some(dir) = config.workspace.as_ref() {
                cmd.current_dir(dir);
            }

            if opts.stream {
                let child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        let err = format!("claude binary not found on PATH: {e}");
                        crate::dispatch::schedule(move || {
                            (callback)(BackendResult {
                                text: String::new(),
                                session_id: String::new(),
                                error: Some(err),
                            });
                        });
                        return;
                    }
                };

                let handle: crate::backend::SharedChild =
                    std::sync::Arc::new(std::sync::Mutex::new(child));
                crate::backend::track_child(&handle);

                let mut text = String::new();
                let mut session_id = String::new();

                let pipe = handle.lock().expect("child lock").stdout.take();
                if let Some(pipe) = pipe {
                    let reader = BufReader::new(pipe);
                    for line in reader.lines().map_while(Result::ok) {
                        if crate::backend::is_shutdown() {
                            break;
                        }
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if let Ok(ev) = serde_json::from_str::<StreamEvent>(trimmed) {
                            if !ev.session_id.is_empty() {
                                session_id = ev.session_id;
                            }
                            if ev.event == "assistant" {
                                if let Some(t) = ev.text {
                                    text.push_str(&t);
                                    if let Some(cb) = on_stream.as_ref() {
                                        let chunk = t;
                                        let cb = Arc::clone(cb);
                                        crate::dispatch::schedule(move || cb(&chunk));
                                    }
                                }
                            } else if ev.event == "result" {
                                if let Some(t) = ev.text {
                                    text = t;
                                }
                            }
                        }
                    }
                }

                let (exit, stderr) = {
                    let child = &mut *handle.lock().expect("child lock");
                    let exit = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
                    let stderr = child
                        .stderr
                        .take()
                        .map(|mut pipe| {
                            let mut s = String::new();
                            pipe.read_to_string(&mut s).ok();
                            s
                        })
                        .unwrap_or_default();
                    (exit, stderr)
                };
                crate::backend::untrack_child(&handle);

                if crate::backend::is_shutdown() {
                    return;
                }

                let result = if exit != 0 {
                    BackendResult {
                        text: text.clone(),
                        session_id,
                        error: Some(format!(
                            "exit code {}: {}",
                            exit,
                            stderr.trim().lines().last().unwrap_or("")
                        )),
                    }
                } else {
                    BackendResult {
                        text,
                        session_id,
                        error: None,
                    }
                };

                let needs_retry = exit != 0
                    && matches!(&opts.op, BackendOp::Resume(_))
                    && (stderr.contains("session")
                        || stderr.contains("expired")
                        || stderr.contains("not found"));

                if needs_retry {
                    let retry_opts = BackendOpts {
                        op: BackendOp::NewSession,
                        ..opts
                    };
                    let retry = ClaudeAdapter::new(config);
                    retry.execute(retry_opts, on_stream, callback);
                } else {
                    crate::dispatch::schedule(move || (callback)(result));
                }
            } else {
                let child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        let err = format!("claude binary not found on PATH: {e}");
                        crate::dispatch::schedule(move || {
                            (callback)(BackendResult {
                                text: String::new(),
                                session_id: String::new(),
                                error: Some(err),
                            });
                        });
                        return;
                    }
                };

                let handle: crate::backend::SharedChild =
                    std::sync::Arc::new(std::sync::Mutex::new(child));
                crate::backend::track_child(&handle);

                let (exit, stdout, stderr) = {
                    let child = &mut *handle.lock().expect("child lock");
                    let mut stdout_buf = String::new();
                    let mut stderr_buf = String::new();
                    if let Some(pipe) = child.stdout.as_mut() {
                        let _ = pipe.read_to_string(&mut stdout_buf);
                    }
                    if let Some(pipe) = child.stderr.as_mut() {
                        let _ = pipe.read_to_string(&mut stderr_buf);
                    }
                    let exit = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
                    (exit, stdout_buf, stderr_buf)
                };
                crate::backend::untrack_child(&handle);

                if crate::backend::is_shutdown() {
                    return;
                }

                let result = if exit != 0 {
                    BackendResult {
                        text: stdout.clone(),
                        session_id: String::new(),
                        error: Some(format!(
                            "exit code {}: {}",
                            exit,
                            stderr.trim().lines().last().unwrap_or("")
                        )),
                    }
                } else {
                    match parse_json_response(&stdout) {
                        Ok(r) => r,
                        Err(e) => BackendResult {
                            text: stdout,
                            session_id: String::new(),
                            error: Some(e),
                        },
                    }
                };

                let needs_retry = result.error.is_some()
                    && matches!(&opts.op, BackendOp::Resume(_))
                    && (stderr.contains("session")
                        || stderr.contains("expired")
                        || stderr.contains("not found"));

                if needs_retry {
                    let retry_opts = BackendOpts {
                        op: BackendOp::NewSession,
                        ..opts
                    };
                    let retry = ClaudeAdapter::new(config);
                    retry.execute(retry_opts, on_stream, callback);
                } else {
                    crate::dispatch::schedule(move || (callback)(result));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::BackendConfig;

    fn default_config() -> BackendConfig {
        BackendConfig {
            backend: "claude".to_string(),
            model: None,
            workspace: None,
            extra_args: Vec::new(),
        }
    }

    fn new_session_opts() -> BackendOpts {
        BackendOpts {
            op: BackendOp::NewSession,
            prompt: "hi".to_string(),
            ask_mode: false,
            stream: false,
            json_schema: None,
        }
    }

    fn assert_arg_value(args: &[String], flag: &str, expected: &str) {
        let i = args.iter().position(|a| a == flag).unwrap();
        assert_eq!(
            args.get(i + 1).map(String::as_str),
            Some(expected),
            "after {flag}: {args:?}"
        );
    }

    #[test]
    fn build_args_new_session() {
        let a = ClaudeAdapter::new(default_config());
        let args = a.build_args(&new_session_opts());
        assert_eq!(args.first().map(String::as_str), Some("-p"));
        assert_eq!(args.get(1).map(String::as_str), Some("hi"));
        assert_arg_value(&args, "--output-format", "json");
        assert!(!args.iter().any(|a| a == "--resume"));
        assert!(!args.iter().any(|a| a == "--continue"));
    }

    #[test]
    fn build_args_continue_latest() {
        let a = ClaudeAdapter::new(default_config());
        let opts = BackendOpts {
            op: BackendOp::ContinueLatest,
            ..new_session_opts()
        };
        let args = a.build_args(&opts);
        assert!(args.iter().any(|a| a == "--continue"));
        assert!(!args.iter().any(|a| a == "--resume"));
    }

    #[test]
    fn build_args_resume() {
        let a = ClaudeAdapter::new(default_config());
        let opts = BackendOpts {
            op: BackendOp::Resume("sess-123".to_string()),
            ..new_session_opts()
        };
        let args = a.build_args(&opts);
        assert_arg_value(&args, "--resume", "sess-123");
        assert!(!args.iter().any(|a| a == "--continue"));
    }

    #[test]
    fn build_args_stream() {
        let a = ClaudeAdapter::new(default_config());
        let opts = BackendOpts {
            stream: true,
            ..new_session_opts()
        };
        let args = a.build_args(&opts);
        assert_arg_value(&args, "--output-format", "stream-json");
        assert!(args.iter().any(|a| a == "--include-partial-messages"));
    }

    #[test]
    fn build_args_ask_mode() {
        let a = ClaudeAdapter::new(default_config());
        let opts = BackendOpts {
            ask_mode: true,
            ..new_session_opts()
        };
        let args = a.build_args(&opts);
        assert_arg_value(&args, "--permission-mode", "plan");
    }

    #[test]
    fn build_args_model() {
        let a = ClaudeAdapter::new(BackendConfig {
            model: Some("claude-3".to_string()),
            ..default_config()
        });
        let args = a.build_args(&new_session_opts());
        assert_arg_value(&args, "--model", "claude-3");
    }

    #[test]
    fn build_args_workspace() {
        let a = ClaudeAdapter::new(BackendConfig {
            workspace: Some("/tmp/ws".to_string()),
            ..default_config()
        });
        let args = a.build_args(&new_session_opts());
        assert_arg_value(&args, "--add-dir", "/tmp/ws");
    }

    #[test]
    fn build_args_extra_args() {
        let a = ClaudeAdapter::new(BackendConfig {
            extra_args: vec!["--verbose".to_string(), "--foo".to_string()],
            ..default_config()
        });
        let args = a.build_args(&new_session_opts());
        assert!(args.iter().any(|a| a == "--verbose"));
        assert!(args.iter().any(|a| a == "--foo"));
    }

    #[test]
    fn build_args_json_schema() {
        let schema = r#"{"type":"object"}"#;
        let a = ClaudeAdapter::new(default_config());
        let opts = BackendOpts {
            json_schema: Some(schema.to_string()),
            ..new_session_opts()
        };
        let args = a.build_args(&opts);
        assert_arg_value(&args, "--json-schema", schema);
    }
}
