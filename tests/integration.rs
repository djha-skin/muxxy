//! End-to-end tests that run the `muxxy` binary against a real tmux server
//! hosting a Python REPL. Each test gets its own isolated server on its own
//! socket, so nothing touches the developer's tmux sessions. Tests are
//! skipped (pass) when tmux or python3 is unavailable.

use serde_yaml::Value;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TestServer {
    name: String,
    socket: String,
}

impl TestServer {
    fn start() -> Option<TestServer> {
        if !tmux_available() {
            eprintln!("tmux unavailable; skipping");
            return None;
        }
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let name = format!("muxxy-it-{}-{}", std::process::id(), n);
        let _ = Command::new("tmux").args(["-L", &name, "kill-server"]).status();

        let started = Command::new("tmux")
            .args(["-L", &name, "new-session", "-d", "-s", "muxxytest", "python3 -i"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !started {
            eprintln!("could not start python REPL session; skipping");
            return None;
        }

        let socket = Command::new("tmux")
            .args(["-L", &name, "display-message", "-p", "#{socket_path}"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())?;

        let server = TestServer { name, socket };
        // Wait for the Python REPL prompt to appear.
        for _ in 0..100 {
            if let Some(text) = server.tmux(&["capture-pane", "-t", "0", "-p"])
                && text.contains(">>>")
            {
                return Some(server);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        eprintln!("python REPL never became ready; skipping");
        None
    }

    fn tmux(&self, args: &[&str]) -> Option<String> {
        let mut full = vec!["-L", &self.name];
        full.extend_from_slice(args);
        let out = Command::new("tmux").args(&full).output().ok()?;
        if out.status.success() {
            Some(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            None
        }
    }

    fn muxxy(&self, args: &[&str]) -> Output {
        let mut full = vec!["--socket", &self.socket];
        full.extend_from_slice(args);
        Command::new(env!("CARGO_BIN_EXE_muxxy"))
            .args(&full)
            .output()
            .expect("failed to run muxxy binary")
    }

    fn parse_yaml(output: &Output) -> Value {
        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_yaml::from_str(&stdout).unwrap_or_else(|e| {
            panic!("muxxy output was not valid YAML: {e}\n---\n{stdout}")
        })
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = Command::new("tmux").args(["-L", &self.name, "kill-server"]).status();
    }
}

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn is_repl_ready_true_when_idle() {
    let Some(server) = TestServer::start() else { return };
    let out = server.muxxy(&["--prompt", "^>>> ", "is-repl-ready"]);
    assert!(out.status.success());
    let v = TestServer::parse_yaml(&out);
    assert_eq!(v["is_ready"].as_bool(), Some(true));
    assert!(v["kind"].is_string());
}

#[test]
fn execute_command_returns_output() {
    let Some(server) = TestServer::start() else { return };
    let out = server.muxxy(&["--prompt", "^>>> ", "execute-command", "2 + 3", "--check", "0.1"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v = TestServer::parse_yaml(&out);
    assert_eq!(v["status"].as_str(), Some("ok"));
    assert_eq!(v["last_command"].as_str(), Some("2 + 3"));
    assert_eq!(v["output"].as_str(), Some("5"));
}

#[test]
fn get_last_command_after_execute() {
    let Some(server) = TestServer::start() else { return };
    let _ = server.muxxy(&["--prompt", "^>>> ", "execute-command", "2 + 3", "--check", "0.1"]);
    let out = server.muxxy(&["--prompt", "^>>> ", "get-last-command"]);
    assert!(out.status.success());
    let v = TestServer::parse_yaml(&out);
    assert_eq!(v["last_command"].as_str(), Some("2 + 3"));
    assert_eq!(v["output"].as_str(), Some("5"));
}

#[test]
fn execute_command_waits_for_slow_command() {
    let Some(server) = TestServer::start() else { return };
    let out = server.muxxy(&[
        "--prompt",
        "^>>> ",
        "execute-command",
        "import time; time.sleep(1.5); print('slow-done')",
        "--check",
        "0.1",
    ]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v = TestServer::parse_yaml(&out);
    assert_eq!(v["status"].as_str(), Some("ok"));
    assert_eq!(v["output"].as_str(), Some("slow-done"));
}

#[test]
fn busy_repl_reports_not_ready() {
    let Some(server) = TestServer::start() else { return };
    let _ = server.tmux(&[
        "send-keys",
        "-t",
        "0",
        "import time; time.sleep(4); print('x')",
        "Enter",
    ]);
    std::thread::sleep(Duration::from_millis(400));
    let out = server.muxxy(&["--prompt", "^>>> ", "is-repl-ready"]);
    let v = TestServer::parse_yaml(&out);
    assert_eq!(v["is_ready"].as_bool(), Some(false));
    assert!(v["kind"].is_null());
}

#[test]
fn execute_command_errors_when_busy() {
    let Some(server) = TestServer::start() else { return };
    let _ = server.tmux(&[
        "send-keys",
        "-t",
        "0",
        "import time; time.sleep(4); print('x')",
        "Enter",
    ]);
    std::thread::sleep(Duration::from_millis(400));
    let out = server.muxxy(&[
        "--prompt",
        "^>>> ",
        "execute-command",
        "print(1)",
        "--check",
        "0.1",
    ]);
    let v = TestServer::parse_yaml(&out);
    assert_eq!(v["status"].as_str(), Some("error"));
}

#[test]
fn split_pane_and_send_keys_setup_flow() {
    let Some(server) = TestServer::start() else { return };
    // Split the REPL pane, creating a fresh shell pane beside it.
    let out = server.muxxy(&["split-pane"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v = TestServer::parse_yaml(&out);
    let pane = v["pane"].as_str().expect("split-pane must print the new pane id").to_string();
    assert!(!pane.is_empty());

    // Start a Python REPL in the new pane and wait a moment for it to boot.
    let out = server.muxxy(&["--pane", &pane, "send-keys", "python3 -i", "--sleep", "1"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    // The new pane should become ready.
    let mut ready = false;
    for _ in 0..40 {
        let out = server.muxxy(&["--pane", &pane, "--prompt", "^>>> ", "is-repl-ready"]);
        let v = TestServer::parse_yaml(&out);
        if v["is_ready"].as_bool() == Some(true) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(ready, "new pane never became ready");

    // And we can run commands in the new pane.
    let out = server.muxxy(&[
        "--pane",
        &pane,
        "--prompt",
        "^>>> ",
        "execute-command",
        "1 + 1",
        "--check",
        "0.1",
    ]);
    let v = TestServer::parse_yaml(&out);
    assert_eq!(v["status"].as_str(), Some("ok"));
    assert_eq!(v["output"].as_str(), Some("2"));
}

#[test]
fn kill_pane_destroys_the_pane() {
    let Some(server) = TestServer::start() else { return };
    let out = server.muxxy(&["split-pane"]);
    let v = TestServer::parse_yaml(&out);
    let pane = v["pane"].as_str().expect("split-pane must print the new pane id").to_string();

    let pane_exists = |server: &TestServer| {
        server
            .tmux(&["list-panes", "-F", "#{pane_id}"])
            .is_some_and(|out| out.lines().any(|l| l.trim() == pane))
    };

    // The new pane exists...
    assert!(pane_exists(&server));

    // ...until we kill it with the tool.
    let out = server.muxxy(&["--pane", &pane, "kill-pane"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(!pane_exists(&server));
}

#[test]
fn multiline_input_with_continuation_prompt() {
    let Some(server) = TestServer::start() else { return };
    // A multi-line python block, sent with embedded newlines plus a final
    // Enter to close the block. The continuation (...) and top-level (>>>)
    // prompts are both in the prompt set.
    let out = server.muxxy(&[
        "--pane",
        "0",
        "send-keys",
        "for i in range(3):\n    print('row', i)\n",
    ]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    // Wait for the block to finish executing.
    let mut done = false;
    for _ in 0..40 {
        let out = server.muxxy(&[
            "--prompt",
            "^>>> ",
            "--prompt",
            r"^\.\.\. ",
            "get-last-command",
        ]);
        let v = TestServer::parse_yaml(&out);
        if let Some(cmd) = v["last_command"].as_str() {
            if cmd.contains("print('row', i)") {
                assert_eq!(v["output"].as_str(), Some("row 0\nrow 1\nrow 2"));
                done = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(done, "multi-line block output never appeared");
}
