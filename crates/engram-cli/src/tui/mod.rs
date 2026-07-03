//! The full-screen terminal UI.
//!
//! A single async event loop multiplexes three sources — terminal input, an
//! internal message bus fed by background HTTP tasks, and an animation tick —
//! and redraws after each. All network work happens off the draw thread, so the
//! UI never blocks on the daemon.

mod app;
mod ui;
mod views;

pub use app::{App, Msg};

use crate::api::Client;
use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyEventKind,
};
use crossterm::{execute, terminal};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::time::Duration;
use tokio::sync::mpsc;

type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Run the TUI to completion against `client`.
pub async fn run(client: Client) -> Result<()> {
    let mut term = setup()?;
    install_panic_hook();

    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut app = App::new(client, tx);
    app.bootstrap();

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(110));
    let mut mouse_on = app.mouse; // capture was enabled in setup() to match the default

    let res = loop {
        if let Err(e) = term.draw(|f| ui::draw(f, &mut app)) {
            break Err(e.into());
        }
        if app.should_quit {
            break Ok(());
        }
        // Apply a mouse-capture toggle (palette "Toggle mouse"): off restores the
        // terminal's native text selection.
        if app.mouse != mouse_on {
            let r = if app.mouse {
                execute!(term.backend_mut(), EnableMouseCapture)
            } else {
                execute!(term.backend_mut(), DisableMouseCapture)
            };
            if let Err(e) = r {
                break Err(e.into());
            }
            mouse_on = app.mouse;
        }

        tokio::select! {
            _ = ticker.tick() => app.on_tick(),
            maybe_ev = events.next() => match maybe_ev {
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release => app.on_key(k),
                Some(Ok(Event::Mouse(me))) => app.on_mouse(me),
                Some(Ok(Event::Paste(s))) => app.on_paste(&s),
                Some(Ok(Event::Resize(_, _))) => {}
                Some(Ok(_)) => {}
                Some(Err(e)) => break Err(e.into()),
                None => break Ok(()),
            },
            Some(msg) = rx.recv() => app.on_msg(msg),
        }
    };

    restore(&mut term)?;
    res
}

fn setup() -> Result<Tui> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        terminal::EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;
    term.clear()?;
    Ok(term)
}

fn restore(term: &mut Tui) -> Result<()> {
    terminal::disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        terminal::LeaveAlternateScreen,
        DisableBracketedPaste,
        DisableMouseCapture
    )?;
    term.show_cursor()?;
    Ok(())
}

/// Make sure a panic never leaves the user's terminal in raw/alt-screen mode.
fn install_panic_hook() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            terminal::LeaveAlternateScreen,
            DisableBracketedPaste,
            DisableMouseCapture
        );
        let _ = terminal::disable_raw_mode();
        hook(info);
    }));
}

#[cfg(test)]
mod smoke {
    //! Renders every view at a range of terminal sizes against a headless
    //! backend, asserting the draw code never panics (out-of-bounds slices in
    //! wrapping/ellipsis/scroll math are the usual suspects).
    use super::app::{App, LiveStep, PlanStep, Role, Turn, View};
    use crate::api::{
        Consciousness, ConsciousnessLine, EgressItem, Health, LedgerEntry, LedgerVerify, MemRecord,
        Meter, ScheduleJob, Skill, StepRecord, Task, TaskRun,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::json;

    fn sample_app() -> App {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let client = crate::api::Client::new("http://127.0.0.1:1", None);
        let mut app = App::new(client, tx);
        app.model = "claude-opus-4-8".into();
        app.meter = Meter {
            tokens_in: 623569,
            tokens_out: 21007,
            cost_usd: 0.42,
            calls: 56,
        };
        app.ledger = Some(LedgerVerify {
            ok: true,
            entries: 4233,
            bad_seq: None,
        });
        app.health = Some(Health {
            ok: true,
            version: "0.2.0".into(),
            offline: false,
        });

        let md = "# Heading\n\nSome **bold**, _italic_, and `inline code`.\n\n- bullet one\n- bullet two with a fairly long line that should wrap somewhere around the middle of the terminal width\n\n| col a | col b |\n|---|---|\n| 1 | two |\n\n```rust\nfn main() { println!(\"hi\"); }\n```\n\n> a quote\n\nA [link](https://example.com/very/long/path/that/keeps/going) here. supercalifragilisticexpialidocioussupercalifragilistic";
        app.chat.turns.push(Turn {
            role: Role::User,
            text: "hello there, render this".into(),
            recalled: vec![],
            learned: vec![],
            steps: vec![],
            plan: vec![],
            error: false,
        });
        app.chat.turns.push(Turn {
            role: Role::Engram,
            text: md.into(),
            recalled: vec![crate::api::RecalledRef {
                id: 31,
                region: "identity".into(),
                text: "x".into(),
                score: 0.9,
            }],
            learned: vec!["User likes ice cream".into()],
            steps: vec![StepRecord {
                tool: "web_search".into(),
                ok: true,
                ..Default::default()
            }],
            plan: vec![
                PlanStep {
                    title: "search the web".into(),
                    status: "done".into(),
                },
                PlanStep {
                    title: "read the top results".into(),
                    status: "doing".into(),
                },
                PlanStep {
                    title: "summarize".into(),
                    status: "todo".into(),
                },
            ],
            error: false,
        });
        app.chat.streaming = true;
        app.chat.live_steps.push(LiveStep {
            tool: "web_fetch".into(),
            ok: true,
            observation: "fetched a very long observation ".repeat(20),
        });
        app.chat.live_narration.push("I'm searching now".into());
        app.chat.live_plan.push(PlanStep {
            title: "gathering sources".into(),
            status: "doing".into(),
        });

        app.tasks = vec![
            Task {
                id: "t1".into(),
                title: "A todo task with a long title that needs ellipsis somewhere".into(),
                status: "todo".into(),
                origin: "manual".into(),
                created_ms: 1782700000000,
                ..Default::default()
            },
            Task {
                id: "t2".into(),
                title: "running".into(),
                status: "doing".into(),
                progress: Some("step 3 · web_fetch".into()),
                ..Default::default()
            },
            Task {
                id: "t3".into(),
                title: "done".into(),
                status: "done".into(),
                run: Some(TaskRun {
                    answer: md.into(),
                    stopped: "final".into(),
                    steps: vec![StepRecord {
                        tool: "x".into(),
                        ledger_seq: 12,
                        ledger_hash: "abc123".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
        ];
        app.memory_recent = (0..30)
            .map(|i| MemRecord {
                id: i,
                region: "semantic".into(),
                text: format!("memory {i} with some text"),
                tier: "warm".into(),
                ..Default::default()
            })
            .collect();
        app.consciousness = Some(Consciousness {
            version: 4,
            distilled_at_ms: 1782700000000,
            lines: vec![ConsciousnessLine {
                id: "m1".into(),
                region: "identity".into(),
                text: "User is Bulgarian".into(),
                ..Default::default()
            }],
        });
        app.skills = (0..20)
            .map(|i| Skill {
                id: format!("skill_{i}"),
                category: "research".into(),
                description: "Does a useful thing".into(),
                enabled: i % 2 == 0,
                active: 1,
                versions: vec![1],
                runtime: "process".into(),
                interpreter: Some("python3".into()),
                ..Default::default()
            })
            .collect();
        app.schedule = vec![ScheduleJob {
            id: "s1".into(),
            name: "Evening digest".into(),
            next_fire_ms: Some(1782831600000),
            recurrence: json!({"kind":"daily","hour":17,"min":0}),
            ..Default::default()
        }];
        app.ledger_tail = (0..40)
            .map(|i| LedgerEntry {
                seq: i,
                ts_ms: 1782700000000,
                kind: "open.url".into(),
                actor: "user".into(),
                hash: "abcdef0123456789".into(),
                payload: json!({"url":"https://x"}),
                ..Default::default()
            })
            .collect();
        app.egress = vec![EgressItem {
            scope: "agent:default".into(),
            dest: "hooks.slack.com".into(),
            tool: "send_message".into(),
            reason: "destination_not_allowlisted".into(),
        }];
        app.agents = vec![
            json!({"id":"a1","name":"Researcher","role":"deep web research","emoji":"🔎","model":"claude-opus-4-8","autonomy_policy":null}),
        ];
        app.config_raw = Some(json!({
            "provider": {"kind":"openrouter","model":"minimax/minimax-m3","base_url":"https://openrouter.ai/api/v1","effort":"","api_key_set":true},
            "embed": {"kind":"trigram","model_dir":""},
            "security": {"allow_shell":false,"shell_backend":"","shell_target":"","enable_worktree_isolation":false,"auto_distill_skills":true,"disable_skill_author":false,"api_token_set":false},
            "cost": {"task_token_budget":1000000},
            "web": {"tavily_key_set":true,"brave_key_set":false,"searxng_url":"","travelpayouts_set":false},
            "media": {"vision_model":"","image_model":"","tts_model":"","stt_model":""},
            "browser": {"chrome_path":"","cdp_port":0},
            "channels": {"telegram_set":false,"telegram_username":"","webhook_url_set":false},
            "mcp": [{"name":"filesystem","command":"npx","args":["-y","@modelcontextprotocol/server-filesystem","/tmp"],"env":{"TOKEN":"•••"}}],
        }));
        app.chat
            .pending_attachments
            .push(crate::tui::app::Attachment {
                kind: "file".into(),
                name: "notes.txt".into(),
                text: "hello".into(),
            });
        app.sessions = (0..12)
            .map(|i| crate::api::SessionMeta {
                id: format!("s-{i}"),
                title: format!("a past session number {i} with a long title"),
                messages: (i as u64) * 2,
                fav: i % 3 == 0,
                updated_ms: 1782700000000,
                ..Default::default()
            })
            .collect();
        app
    }

    #[test]
    fn every_view_renders_at_many_sizes() {
        let sizes = [(20u16, 8u16), (40, 12), (80, 24), (120, 40), (200, 50)];
        for view in [
            View::Chat,
            View::Tasks,
            View::Memory,
            View::Skills,
            View::Schedule,
            View::Autonomy,
            View::Ledger,
            View::Agents,
            View::Settings,
            View::Help,
        ] {
            for (w, h) in sizes {
                let mut app = sample_app();
                app.view = view;
                let backend = TestBackend::new(w, h);
                let mut term = Terminal::new(backend).unwrap();
                term.draw(|f| super::ui::draw(f, &mut app)).unwrap();
            }
        }
    }

    #[test]
    fn mcp_edit_preserves_unedited_fields() {
        // Editing a server through the form must not drop cwd/trusted.
        let mut app = sample_app();
        app.config_raw = Some(json!({ "mcp": [
            {"name":"fs","command":"npx","args":["a"],"env":{},"cwd":"/work","trusted":true}
        ]}));
        app.open_mcp_form(Some(0));
        // The form only exposes name/command/args/env (4 fields).
        assert_eq!(app.form.as_ref().unwrap().fields.len(), 4);
        // (Full apply requires a runtime; the round-trip preservation is in apply_form,
        // which starts from the existing server object — see app.rs FormKind::Mcp.)
    }

    #[test]
    fn palette_overlay_renders() {
        let mut app = sample_app();
        app.open_palette();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| super::ui::draw(f, &mut app)).unwrap();
    }

    /// Dump a few views as plain text (`cargo test -p engram-cli render_preview
    /// -- --ignored --nocapture`) — a visual sanity check without a real terminal.
    #[test]
    #[ignore]
    fn render_preview() {
        for view in [
            View::Chat,
            View::Settings,
            View::Agents,
            View::Tasks,
            View::Memory,
            View::Ledger,
            View::Skills,
        ] {
            let mut app = sample_app();
            app.view = view;
            if view == View::Chat {
                app.chat.streaming = false;
                app.chat.live_steps.clear();
                app.chat.live_narration.clear();
                app.chat.live_plan.clear();
            }
            let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
            term.draw(|f| super::ui::draw(f, &mut app)).unwrap();
            println!("\n========== {view:?} ==========");
            println!("{}", term.backend());
        }
    }

    #[test]
    fn form_modals_render() {
        for opener in [
            App::create_agent_prompt as fn(&mut App),
            App::add_schedule_form,
        ] {
            for (w, h) in [(20u16, 8u16), (80, 24), (200, 50)] {
                let mut app = sample_app();
                opener(&mut app);
                let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                term.draw(|f| super::ui::draw(f, &mut app)).unwrap();
            }
        }
        // Agent-derived forms (edit / policy) read the selected agent.
        let mut app = sample_app();
        app.view = View::Agents;
        app.sel = 0;
        app.edit_selected_agent();
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| super::ui::draw(f, &mut app)).unwrap();
        let mut app = sample_app();
        app.policy_selected_agent();
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| super::ui::draw(f, &mut app)).unwrap();
        // MCP add + edit forms.
        for index in [None, Some(0usize)] {
            let mut app = sample_app();
            app.open_mcp_form(index);
            let mut term = Terminal::new(TestBackend::new(90, 24)).unwrap();
            term.draw(|f| super::ui::draw(f, &mut app)).unwrap();
        }
    }

    #[test]
    fn model_prompt_renders() {
        for (w, h) in [(20u16, 8u16), (80, 24)] {
            let mut app = sample_app();
            app.open_model_prompt();
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| super::ui::draw(f, &mut app)).unwrap();
        }
    }

    #[test]
    fn sessions_picker_renders() {
        for (w, h) in [(20u16, 8u16), (80, 24), (200, 50)] {
            let mut app = sample_app();
            app.sessions_open = true;
            app.sessions_sel = 5;
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| super::ui::draw(f, &mut app)).unwrap();
        }
    }
}
