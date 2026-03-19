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
pub(crate) struct CursorAdapter {
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
        if let Some(model) = self.config.model.as_ref() {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        if let Some(dir) = self.config.workspace.as_ref() {
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

            if let Some(dir) = config.workspace.as_ref() {
                cmd.current_dir(dir);
            }

            if opts.stream {
                let child = match cmd.spawn() {
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

                let handle: crate::backend::SharedChild =
                    std::sync::Arc::new(std::sync::Mutex::new(child));
                crate::backend::track_child(&handle);

                let mut text = String::new();
                let mut session_id = String::new();
                let mut streamed = String::new();

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
                                if combined.starts_with(&streamed)
                                    && combined.len() > streamed.len()
                                {
                                    let delta = combined[streamed.len()..].to_string();
                                    if let Some(cb) = on_stream.as_ref() {
                                        let cb = Arc::clone(cb);
                                        crate::dispatch::schedule(move || cb(&delta));
                                    }
                                    streamed = combined.clone();
                                } else if !combined.is_empty() && combined != streamed {
                                    let delta = combined.clone();
                                    if let Some(cb) = on_stream.as_ref() {
                                        let cb = Arc::clone(cb);
                                        crate::dispatch::schedule(move || cb(&delta));
                                    }
                                    streamed.push_str(&combined);
                                }
                                if !combined.is_empty() {
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
                    let retry = CursorAdapter::new(config);
                    retry.execute(retry_opts, on_stream, callback);
                } else {
                    crate::dispatch::schedule(move || (callback)(result));
                }
            } else {
                let child = match cmd.spawn() {
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
    #[serde(default)]
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

    #[test]
    fn stream_event_missing_content_defaults_empty() {
        let j = r#"{"type":"assistant","message":{"role":"assistant"},"session_id":"s1"}"#;
        let ev: StreamEvent = serde_json::from_str(j).unwrap();
        assert_eq!(ev.kind, "assistant");
        let texts: Vec<String> = ev
            .message
            .into_iter()
            .flat_map(|m| m.content)
            .filter_map(|c| c.text)
            .collect();
        assert!(texts.is_empty());
    }

    /// Spawns the real `agent` CLI with streaming, captures every delta our
    /// parsing logic produces, and asserts that concatenating them reproduces
    /// the final cumulative text exactly. Run with:
    ///
    /// ```sh
    /// cargo test -p arbiter streaming_deltas_match -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "requires `agent` binary on PATH and valid auth"]
    fn streaming_deltas_match_final_text() {
        use std::process::{Command, Stdio};

        let workspace = std::env::var("ARBITER_TEST_WORKSPACE").unwrap_or_else(|_| ".".to_string());

        let mut child = Command::new("agent")
            .args([
                "-p",
                "--trust",
                "--approve-mcps",
                "Respond with exactly two paragraphs. First: 'Hello from arbiter.' \
                 Second: 'Streaming works.'",
                "--output-format",
                "stream-json",
                "--stream-partial-output",
                "--mode",
                "ask",
                "--workspace",
                &workspace,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("agent binary must be on PATH");

        let mut deltas: Vec<String> = Vec::new();
        let mut streamed = String::new();
        let mut final_text = String::new();
        let mut result_text: Option<String> = None;
        let mut event_count = 0usize;
        let mut skipped_lines = 0usize;

        if let Some(pipe) = child.stdout.take() {
            let reader = BufReader::new(pipe);
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(ev) = serde_json::from_str::<StreamEvent>(trimmed) else {
                    skipped_lines += 1;
                    eprintln!("[skip] {trimmed}");
                    continue;
                };
                event_count += 1;
                match ev.kind.as_str() {
                    "assistant" => {
                        let combined: String = ev
                            .message
                            .into_iter()
                            .flat_map(|m| m.content)
                            .filter_map(|c| c.text)
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        if combined.starts_with(&streamed) && combined.len() > streamed.len() {
                            let d = combined[streamed.len()..].to_string();
                            eprintln!("[delta:cumulative] {:?}", d);
                            deltas.push(d);
                            streamed = combined.clone();
                        } else if !combined.is_empty() && combined != streamed {
                            eprintln!("[delta:incremental] {:?}", combined);
                            deltas.push(combined.clone());
                            streamed.push_str(&combined);
                        }
                        if !combined.is_empty() {
                            final_text = combined;
                        }
                    }
                    "result" => {
                        result_text = ev.result.clone();
                    }
                    other => {
                        eprintln!("[event] type={other}");
                    }
                }
            }
        }

        let exit = child.wait().expect("wait for agent");
        let stderr = child
            .stderr
            .take()
            .map(|mut p| {
                let mut s = String::new();
                p.read_to_string(&mut s).ok();
                s
            })
            .unwrap_or_default();

        eprintln!("\n--- summary ---");
        eprintln!("events parsed : {event_count}");
        eprintln!("lines skipped : {skipped_lines}");
        eprintln!("deltas emitted: {}", deltas.len());
        eprintln!("exit code     : {:?}", exit.code());
        if !stderr.trim().is_empty() {
            eprintln!("stderr        : {}", stderr.trim());
        }

        assert!(exit.success(), "agent exited with {exit}");
        assert!(!final_text.is_empty(), "no text received from agent");

        let streamed = deltas.join("");

        eprintln!("\n--- streamed (concatenated deltas) ---");
        eprintln!("{streamed}");
        eprintln!("\n--- final_text (last cumulative) ---");
        eprintln!("{final_text}");

        if let Some(rt) = result_text.as_ref() {
            eprintln!("\n--- result event text ---");
            eprintln!("{rt}");
        }

        assert_eq!(
            streamed, final_text,
            "concatenated streaming deltas must equal final cumulative text"
        );
    }
}
