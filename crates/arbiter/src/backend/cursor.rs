//! Cursor CLI adapter (`agent` binary).
//!
//! Builds args from BackendOpts, spawns process, parses JSON response.
//! Streaming via BufReader::lines for assistant events.

use super::Adapter;
use crate::types::{BackendOp, BackendOpts, BackendResult, OnComplete, OnStream};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

/// Cursor CLI adapter. Binary name: `agent`.
#[derive(Debug)]
pub struct CursorAdapter {
    config: crate::backend::BackendConfig,
}

impl CursorAdapter {
    /// Creates a new Cursor adapter with the given config.
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
            "--trust".to_string(),
            "--approve-mcps".to_string(),
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
            args.push("--mode".to_string());
            args.push("ask".to_string());
        }
        if opts.stream {
            args.push("--stream-partial-output".to_string());
        }
        if let Some(ref model) = self.config.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        if let Some(ref dir) = self.config.workspace {
            args.push("--workspace".to_string());
            args.push(dir.clone());
        }
        args.extend(self.config.extra_args.iter().cloned());
        args
    }
}

impl Adapter for CursorAdapter {
    fn execute(&self, opts: BackendOpts, on_stream: Option<OnStream>, callback: OnComplete) {
        let args = self.build_args(&opts);
        let config = self.config.clone();

        thread::spawn(move || {
            let mut cmd = Command::new("agent");
            cmd.args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            if let Some(ref dir) = config.workspace {
                cmd.current_dir(dir);
            }

            if opts.stream {
                let mut child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        let err = format!("agent binary not found on PATH: {e}");
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

                let mut text = String::new();
                let mut session_id = String::new();
                let mut prev_combined = String::new();

                if let Some(pipe) = child.stdout.take() {
                    let reader = BufReader::new(pipe);
                    for line in reader.lines().map_while(Result::ok) {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let Ok(ev) = serde_json::from_str::<StreamEvent>(trimmed) else {
                            continue;
                        };
                        if let Some(sid) = &ev.session_id {
                            if !sid.is_empty() {
                                session_id = sid.clone();
                            }
                        }
                        match ev.kind.as_str() {
                            "assistant" => {
                                let combined: String = ev
                                    .message
                                    .into_iter()
                                    .flat_map(|m| m.content)
                                    .filter_map(|c| c.text)
                                    .collect::<Vec<_>>()
                                    .join("\n\n");
                                if combined.len() > prev_combined.len()
                                    && combined.starts_with(&prev_combined)
                                {
                                    let delta = &combined[prev_combined.len()..];
                                    if let Some(ref cb) = on_stream {
                                        let cb = Arc::clone(cb);
                                        let d = delta.to_string();
                                        crate::dispatch::schedule(move || cb(&d));
                                    }
                                }
                                if !combined.is_empty() {
                                    prev_combined = combined.clone();
                                    text = combined;
                                }
                            }
                            "result" => {
                                if on_stream.is_none() {
                                    if let Some(t) = ev.result {
                                        text = t;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }

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
                    let retry = CursorAdapter::new(config);
                    retry.execute(retry_opts, on_stream, callback);
                } else {
                    crate::dispatch::schedule(move || (callback)(result));
                }
            } else {
                let output = match cmd.output() {
                    Ok(o) => o,
                    Err(e) => {
                        let err = format!("agent binary not found on PATH: {e}");
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

                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let exit = output.status.code().unwrap_or(-1);

                let result = if exit != 0 {
                    BackendResult {
                        text: stdout.to_string(),
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
                            text: stdout.to_string(),
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
                    let retry = CursorAdapter::new(config);
                    retry.execute(retry_opts, on_stream, callback);
                } else {
                    crate::dispatch::schedule(move || (callback)(result));
                }
            }
        });
    }
}

#[derive(Debug, serde::Deserialize)]
struct StreamEvent {
    #[serde(rename = "type", default)]
    kind: String,
    message: Option<StreamMessage>,
    result: Option<String>,
    session_id: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct StreamMessage {
    content: Vec<StreamContent>,
}

#[derive(Debug, serde::Deserialize)]
struct StreamContent {
    text: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct JsonResponse {
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    result: String,
}

fn parse_json_response(stdout: &str) -> Result<BackendResult, String> {
    let parsed: JsonResponse =
        serde_json::from_str(stdout).map_err(|e| format!("malformed JSON: {e}"))?;
    Ok(BackendResult {
        text: parsed.result,
        session_id: parsed.session_id,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::BackendConfig;

    fn default_config() -> BackendConfig {
        BackendConfig {
            backend: "cursor".to_string(),
            model: None,
            workspace: None,
            extra_args: Vec::new(),
        }
    }

    #[test]
    fn build_args_resume() {
        let a = CursorAdapter::new(default_config());
        let opts = BackendOpts {
            op: BackendOp::Resume("s-1".to_string()),
            prompt: "hi".to_string(),
            ask_mode: false,
            stream: false,
            json_schema: None,
        };
        let args = a.build_args(&opts);
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"s-1".to_string()));
    }

    #[test]
    fn build_args_continue_latest() {
        let a = CursorAdapter::new(default_config());
        let opts = BackendOpts {
            op: BackendOp::ContinueLatest,
            prompt: "hi".to_string(),
            ask_mode: false,
            stream: false,
            json_schema: None,
        };
        let args = a.build_args(&opts);
        assert!(args.contains(&"--continue".to_string()));
    }

    #[test]
    fn build_args_new_session() {
        let a = CursorAdapter::new(default_config());
        let opts = BackendOpts {
            op: BackendOp::NewSession,
            prompt: "hi".to_string(),
            ask_mode: false,
            stream: false,
            json_schema: None,
        };
        let args = a.build_args(&opts);
        assert!(!args.iter().any(|s| s == "--resume"));
        assert!(!args.iter().any(|s| s == "--continue"));
    }

    #[test]
    fn build_args_stream() {
        let a = CursorAdapter::new(default_config());
        let opts = BackendOpts {
            op: BackendOp::NewSession,
            prompt: "hi".to_string(),
            ask_mode: false,
            stream: true,
            json_schema: None,
        };
        let args = a.build_args(&opts);
        assert!(args.contains(&"stream-json".to_string()));
        assert!(args.contains(&"--stream-partial-output".to_string()));
    }

    #[test]
    fn build_args_ask_mode() {
        let a = CursorAdapter::new(default_config());
        let opts = BackendOpts {
            op: BackendOp::NewSession,
            prompt: "hi".to_string(),
            ask_mode: true,
            stream: false,
            json_schema: None,
        };
        let args = a.build_args(&opts);
        assert!(args.contains(&"--mode".to_string()));
        assert!(args.contains(&"ask".to_string()));
    }

    #[test]
    fn build_args_model() {
        let a = CursorAdapter::new(BackendConfig {
            model: Some("gpt-4".to_string()),
            ..default_config()
        });
        let opts = BackendOpts {
            op: BackendOp::NewSession,
            prompt: "hi".to_string(),
            ask_mode: false,
            stream: false,
            json_schema: None,
        };
        let args = a.build_args(&opts);
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gpt-4".to_string()));
    }

    #[test]
    fn build_args_workspace() {
        let a = CursorAdapter::new(BackendConfig {
            workspace: Some("/tmp/ws".to_string()),
            ..default_config()
        });
        let opts = BackendOpts {
            op: BackendOp::NewSession,
            prompt: "hi".to_string(),
            ask_mode: false,
            stream: false,
            json_schema: None,
        };
        let args = a.build_args(&opts);
        assert!(args.contains(&"--workspace".to_string()));
        assert!(args.contains(&"/tmp/ws".to_string()));
    }

    #[test]
    fn build_args_extra_args() {
        let a = CursorAdapter::new(BackendConfig {
            extra_args: vec!["--yolo".to_string(), "--custom-flag".to_string()],
            ..default_config()
        });
        let opts = BackendOpts {
            op: BackendOp::NewSession,
            prompt: "hi".to_string(),
            ask_mode: false,
            stream: false,
            json_schema: None,
        };
        let args = a.build_args(&opts);
        assert!(args.contains(&"--yolo".to_string()));
        assert!(args.contains(&"--custom-flag".to_string()));
    }

    #[test]
    fn parse_json_response_valid() {
        let j = r#"{"session_id": "sess-123", "result": "Hello"}"#;
        let r = parse_json_response(j).unwrap();
        assert_eq!(r.session_id, "sess-123");
        assert_eq!(r.text, "Hello");
        assert!(r.error.is_none());
    }

    #[test]
    fn parse_json_response_actual_cli_format() {
        let j = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":2438,"result":"Hello!","session_id":"641faf9d-ffae-43cc-a6d7-a546c686fb31","request_id":"abc","usage":{"inputTokens":3}}"#;
        let r = parse_json_response(j).unwrap();
        assert_eq!(r.session_id, "641faf9d-ffae-43cc-a6d7-a546c686fb31");
        assert_eq!(r.text, "Hello!");
        assert!(r.error.is_none());
    }

    #[test]
    fn parse_json_response_malformed() {
        let j = "not json";
        let r = parse_json_response(j);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("malformed"));
    }

    #[test]
    fn stream_event_assistant_parses() {
        let j = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello"}]},"session_id":"abc-123","timestamp_ms":1773535570730}"#;
        let ev: StreamEvent = serde_json::from_str(j).unwrap();
        assert_eq!(ev.kind, "assistant");
        assert_eq!(ev.session_id.as_deref(), Some("abc-123"));
        let text = ev
            .message
            .and_then(|m| m.content.into_iter().next())
            .and_then(|c| c.text);
        assert_eq!(text.as_deref(), Some("Hello"));
    }

    #[test]
    fn stream_event_result_parses() {
        let j = r#"{"type":"result","subtype":"success","result":"Full response","session_id":"abc-123"}"#;
        let ev: StreamEvent = serde_json::from_str(j).unwrap();
        assert_eq!(ev.kind, "result");
        assert_eq!(ev.result.as_deref(), Some("Full response"));
    }

    #[test]
    fn stream_event_system_ignored() {
        let j = r#"{"type":"system","subtype":"init","apiKeySource":"login"}"#;
        let ev: StreamEvent = serde_json::from_str(j).unwrap();
        assert_eq!(ev.kind, "system");
        assert!(ev.message.is_none());
        assert!(ev.result.is_none());
    }
}
