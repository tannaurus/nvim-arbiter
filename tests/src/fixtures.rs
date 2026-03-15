//! Embedded test data for unit and nvim tests.
//!
//! Const strings for diffs, JSON responses, thread state, and other fixtures.
//! Single source of truth for test data formats.

/// Simple single-file, single-hunk diff.
pub const SIMPLE_DIFF: &str = "\
diff --git a/src/main.rs b/src/main.rs
index abc..def 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!(\"hello\");
+    println!(\"hello world\");
+    println!(\"goodbye\");
 }
";

/// Multi-hunk diff for a single file.
pub const MULTI_HUNK_DIFF: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 000..111 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,3 @@
 pub fn foo() {}
+pub fn bar() {}
@@ -5,4 +6,5 @@
 pub fn baz() {
-    let x = 1;
+    let x = 2;
+    let y = 3;
 }
";

/// Multi-file diff output (git diff --name-status format).
pub const MULTI_FILE_DIFF_NAMES: &str = "M\tsrc/main.rs\nA\tsrc/lib.rs\nD\tsrc/old.rs\n";

/// JSON backend response with session_id and result.
pub const JSON_BACKEND_RESPONSE: &str = r#"{"session_id": "sess-123", "result": "Hello"}"#;

/// Streaming JSON lines (one event per line).
pub const STREAMING_JSON_LINES: &str = r#"{"event":"assistant","text":"Hello "}
{"event":"assistant","text":"world"}
{"event":"result","text":"Hello world"}
"#;

/// Self-review text response (THREAD|file|line|message format).
pub const SELF_REVIEW_TEXT: &str = "THREAD|src/main.rs|22|Should this return 401 or 403?
THREAD|src/auth.rs|15|Consider caching the token lookup.
";

/// Thread state JSON for persistence tests.
pub const THREAD_STATE_JSON: &str = r#"{
  "threads": [
    {
      "id": "t-001",
      "origin": "user",
      "file": "src/main.rs",
      "line": 22,
      "anchor_content": "let x = 1;",
      "anchor_context": ["  fn foo() {", "  }"],
      "status": "open",
      "auto_resolve": false,
      "auto_resolve_at": null,
      "context": "review",
      "session_id": "sess-abc",
      "messages": [
        {"role": "user", "text": "fix this", "ts": 12345},
        {"role": "agent", "text": "Done.", "ts": 12346}
      ],
      "pending": false
    }
  ]
}"#;

/// Review state JSON for persistence tests.
pub const REVIEW_STATE_JSON: &str = r#"{
  "files": {
    "src/main.rs": {
      "status": "approved",
      "content_hash": "a1b2c3",
      "updated_at": 1710000000
    }
  }
}"#;
