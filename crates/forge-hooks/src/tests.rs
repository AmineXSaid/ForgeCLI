use super::*;

fn runner(hooks: Value) -> (HookRunner, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    let (cfg, errs) = HooksConfig::from_settings(Some(&hooks));
    assert!(errs.is_empty(), "{errs:?}");
    let base =
        HookBase { session_id: "s".into(), transcript_path: None, cwd: d.path().into(), project_dir: d.path().into() };
    (HookRunner::new(cfg, base), d)
}

fn cmd(event: &str, matcher: Option<&str>, command: &str) -> Value {
    let mut m = json!({"hooks": [{"type": "command", "command": command}]});
    if let Some(p) = matcher {
        m["matcher"] = json!(p);
    }
    json!({ event: [m] })
}

#[tokio::test]
async fn c11_exit2_semantics_per_event() {
    // The runner reports the block; each event's effect is fixed by exit2_effect().
    let (r, _d) = runner(cmd("PreToolUse", Some("Bash"), "echo 'no rm' >&2; exit 2"));
    let c = CancellationToken::new();
    let o = r.run(HookEvent::PreToolUse, Some("Bash"), "default", json!({"tool_name": "Bash"}), &c).await;
    assert_eq!(o.blocked.as_deref(), Some("no rm"));
    let o = r.run(HookEvent::PreToolUse, Some("Edit"), "default", json!({}), &c).await;
    assert_eq!(o.ran, 0, "matcher must not match Edit");

    assert_eq!(HookEvent::PreToolUse.exit2_effect(), Exit2Effect::BlockToModel);
    assert_eq!(HookEvent::PostToolUse.exit2_effect(), Exit2Effect::FeedbackToModel);
    assert_eq!(HookEvent::PostToolUseFailure.exit2_effect(), Exit2Effect::FeedbackToModel);
    assert_eq!(HookEvent::UserPromptSubmit.exit2_effect(), Exit2Effect::BlockToUser);
    assert_eq!(HookEvent::Stop.exit2_effect(), Exit2Effect::BlockToModel);
    assert_eq!(HookEvent::SubagentStop.exit2_effect(), Exit2Effect::BlockToModel);
    for e in [HookEvent::PreCompact, HookEvent::Notification, HookEvent::SessionStart, HookEvent::SessionEnd] {
        assert_eq!(e.exit2_effect(), Exit2Effect::UserOnly);
    }

    // Other non-zero exit codes are non-blocking errors shown to the user.
    let (r, _d) = runner(cmd("Stop", None, "echo oops >&2; exit 1"));
    let o = r.run(HookEvent::Stop, None, "default", json!({}), &c).await;
    assert!(o.blocked.is_none());
    assert!(o.user_messages[0].contains("oops"));
}

#[tokio::test]
async fn c11_json_permission_decision() {
    let script = r#"cat >/dev/null; echo '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"ok","updatedInput":{"command":"ls -la"}}}'"#;
    let hooks = json!({"PreToolUse": [
        {"matcher": "Bash", "hooks": [{"type": "command", "command": script}]},
        {"matcher": "Bash", "hooks": [{"type": "command", "command": "echo '{\"hookSpecificOutput\":{\"permissionDecision\":\"deny\",\"permissionDecisionReason\":\"policy\"}}'"}]}
    ]});
    let (r, _d) = runner(hooks);
    let o = r.run(HookEvent::PreToolUse, Some("Bash"), "default", json!({}), &CancellationToken::new()).await;
    assert_eq!(o.ran, 2);
    assert_eq!(o.permission, Some(("deny".into(), "policy".into())), "deny outranks allow");
    assert_eq!(o.updated_input, Some(json!({"command": "ls -la"})));
}

#[tokio::test]
async fn stdin_carries_event_and_stdout_becomes_context() {
    let (r, _d) = runner(cmd(
        "UserPromptSubmit",
        None,
        r#"python3 -c 'import sys,json; d=json.load(sys.stdin); print("event="+d["hook_event_name"]+" prompt="+d["prompt"])'"#,
    ));
    let o = r.run(HookEvent::UserPromptSubmit, None, "plan", json!({"prompt": "hi"}), &CancellationToken::new()).await;
    assert_eq!(o.additional_context, vec!["event=UserPromptSubmit prompt=hi".to_string()]);
}

#[tokio::test]
async fn continue_false_and_block_decision() {
    let (r, _d) = runner(cmd(
        "PostToolUse",
        None,
        r#"echo '{"continue": false, "stopReason": "enough", "decision": "block", "reason": "lint failed"}'"#,
    ));
    let o = r.run(HookEvent::PostToolUse, Some("Edit"), "default", json!({}), &CancellationToken::new()).await;
    assert_eq!(o.stop.as_deref(), Some("enough"));
    assert_eq!(o.blocked.as_deref(), Some("lint failed"));
}

#[tokio::test]
async fn timeouts_are_non_blocking() {
    let hooks = json!({"Notification": [{"hooks": [{"type": "command", "command": "sleep 5", "timeout": 0.2}]}]});
    let (r, _d) = runner(hooks);
    let start = std::time::Instant::now();
    let o = r.run(HookEvent::Notification, None, "default", json!({}), &CancellationToken::new()).await;
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(o.blocked.is_none());
    assert!(o.user_messages[0].contains("timed out"));
}

#[test]
fn matchers() {
    let m = Matcher { pattern: Some("Edit|Write".into()), hooks: vec![] };
    assert!(
        m.matches(Some("Edit"))
            && m.matches(Some("Write"))
            && !m.matches(Some("Bash"))
            && !m.matches(Some("MultiEditX"))
    );
    let mcp = Matcher { pattern: Some("mcp__github__.*".into()), hooks: vec![] };
    assert!(mcp.matches(Some("mcp__github__create_issue")));
    assert!(Matcher { pattern: None, hooks: vec![] }.matches(None));
    let (_, errs) = HooksConfig::from_settings(Some(&json!({"Bogus": []})));
    assert_eq!(errs.len(), 1);
}
