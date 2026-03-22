//! Shared JSON response parsing for CLI backend adapters.

use crate::types::BackendResult;

#[derive(Debug, serde::Deserialize)]
struct JsonResponse {
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    result: String,
}

pub(super) fn parse_json_response(stdout: &str) -> Result<BackendResult, String> {
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

    #[test]
    fn parse_valid() {
        let stdout = r#"{"session_id":"abc","result":"hello"}"#;
        let r = parse_json_response(stdout).unwrap();
        assert_eq!(r.text, "hello");
        assert_eq!(r.session_id, "abc");
        assert!(r.error.is_none());
    }

    #[test]
    fn parse_malformed() {
        let err = parse_json_response("not json").unwrap_err();
        assert!(err.contains("malformed JSON"));
    }

    #[test]
    fn parse_actual_cli_format() {
        let j = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":2438,"result":"Hello!","session_id":"641faf9d-ffae-43cc-a6d7-a546c686fb31","request_id":"abc","usage":{"inputTokens":3}}"#;
        let r = parse_json_response(j).unwrap();
        assert_eq!(r.session_id, "641faf9d-ffae-43cc-a6d7-a546c686fb31");
        assert_eq!(r.text, "Hello!");
        assert!(r.error.is_none());
    }
}
