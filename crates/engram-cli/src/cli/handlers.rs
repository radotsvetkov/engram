//! Implementations for every `engram` subcommand.

use super::output::{self as out, accent, bad, bold, dim, good, kv, table, warn};
use super::{
    AgentsCmd, AutonomyCmd, Cmd, ConfigCmd, LedgerCmd, MemoryCmd, ScheduleCmd, SkillsCmd, TasksCmd,
};
use crate::api::{ChatEvent, Client, TaskEvent};
use crate::ui::format::{cost, human_count, one_line, rel_time, stamp};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::Read;

fn print_json<T: serde::Serialize>(v: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(v).unwrap_or_else(|_| "null".into())
    );
}

fn join(parts: &[String]) -> String {
    parts.join(" ").trim().to_string()
}

fn read_stdin() -> String {
    let mut s = String::new();
    let _ = std::io::stdin().read_to_string(&mut s);
    s.trim().to_string()
}

/// Route a non-special command to its handler.
pub async fn dispatch(client: &Client, cmd: Cmd, json: bool) -> Result<i32> {
    match cmd {
        Cmd::Ask { prompt, quiet } => ask(client, join(&prompt), quiet, json).await,
        Cmd::Run { task, max_steps } => run_agent(client, join(&task), max_steps, json).await,
        Cmd::Status => status(client, json).await,
        Cmd::Doctor => doctor(client, json).await,
        Cmd::Tasks { cmd } => tasks(client, cmd, json).await,
        Cmd::Memory { cmd } => memory(client, cmd, json).await,
        Cmd::Skills { cmd } => skills(client, cmd, json).await,
        Cmd::Schedule { cmd } => schedule(client, cmd, json).await,
        Cmd::Autonomy { cmd } => autonomy(client, cmd, json).await,
        Cmd::Ledger { cmd } => ledger(client, cmd, json).await,
        Cmd::Config { cmd } => config(client, cmd, json).await,
        Cmd::Agents { cmd } => agents(client, cmd, json).await,
        Cmd::Tools => tools(client, json).await,
        Cmd::Events => events(client).await,
        // These are handled in `run()` before dispatch.
        Cmd::Tui | Cmd::Serve { .. } | Cmd::Completions { .. } => Ok(0),
    }
}

// ---- chat / agent ---------------------------------------------------------

async fn ask(client: &Client, prompt: String, quiet: bool, json: bool) -> Result<i32> {
    let prompt = if prompt.is_empty() {
        read_stdin()
    } else {
        prompt
    };
    if prompt.is_empty() {
        eprintln!(
            "{}",
            warn("nothing to ask (provide a prompt or pipe stdin)")
        );
        return Ok(2);
    }
    // Measure this turn's token/cost from the meter delta (the stream carries none).
    let before = client.meter().await.unwrap_or_default();
    let mut rx = client.converse_stream(prompt, None, vec![]);
    let mut final_done = None;
    while let Some(ev) = rx.recv().await {
        match ev {
            ChatEvent::Narration(text) => {
                if !quiet && !json {
                    println!("{}", dim(&format!("· {}", one_line(&text))));
                }
            }
            ChatEvent::Step {
                tool,
                ok,
                observation,
                ..
            } => {
                if !quiet && !json {
                    let mark = if ok { good("✓") } else { bad("✗") };
                    let obs = one_line(&observation);
                    let obs = crate::ui::format::ellipsize(&obs, out::term_width() as usize - 8);
                    println!("  {} {} {}", mark, out::tool(&tool), dim(&obs));
                }
            }
            ChatEvent::Done(done) => {
                final_done = Some(*done);
                break;
            }
            ChatEvent::Error(e) => {
                eprintln!("{}", bad(&format!("error: {e}")));
                return Ok(1);
            }
            ChatEvent::Disconnected(e) => {
                eprintln!("{}", bad(&format!("disconnected: {e}")));
                return Ok(1);
            }
        }
    }
    let Some(done) = final_done else {
        eprintln!("{}", bad("the run ended without an answer"));
        return Ok(1);
    };
    if json {
        let after = client.meter().await.unwrap_or_default();
        // Mirror `run`'s object shape so `ask` and `run` are scriptable the same way.
        print_json(&json!({
            "reply": done.reply,
            "recalled_refs": done.recalled_refs,
            "learned": done.learned,
            "steps": done.steps.len(),
            "tokens_in": after.tokens_in.saturating_sub(before.tokens_in),
            "tokens_out": after.tokens_out.saturating_sub(before.tokens_out),
            "cost_usd": (after.cost_usd - before.cost_usd).max(0.0),
        }));
        return Ok(0);
    }
    if !quiet {
        println!();
    }
    out::print_markdown(&done.reply);
    if !done.recalled_refs.is_empty() {
        let chips: Vec<String> = done
            .recalled_refs
            .iter()
            .map(|r| format!("{}:{}", r.region.chars().next().unwrap_or('?'), r.id))
            .collect();
        println!("\n{} {}", dim("grounded on"), dim(&chips.join("  ")));
    }
    if !done.learned.is_empty() {
        println!("{} {}", dim("learned"), dim(&done.learned.join("; ")));
    }
    Ok(0)
}

async fn run_agent(
    client: &Client,
    task: String,
    max_steps: Option<usize>,
    json: bool,
) -> Result<i32> {
    let task = if task.is_empty() { read_stdin() } else { task };
    if task.is_empty() {
        eprintln!("{}", warn("no task given"));
        return Ok(2);
    }
    // /v1/agent doesn't return per-run token/cost, so measure it from the meter delta.
    let before = client.meter().await.unwrap_or_default();
    let resp = client.agent(&task, max_steps).await?;
    let after = client.meter().await.unwrap_or_default();
    let din = after.tokens_in.saturating_sub(before.tokens_in);
    let dout = after.tokens_out.saturating_sub(before.tokens_out);
    let dcost = (after.cost_usd - before.cost_usd).max(0.0);
    if json {
        print_json(&serde_json::json!({
            "answer": resp.answer,
            "stopped": resp.stopped,
            "steps": resp.steps.len(),
            "tokens_in": din,
            "tokens_out": dout,
            "cost_usd": dcost,
        }));
        return Ok(0);
    }
    for s in &resp.steps {
        let mark = if s.ok { good("✓") } else { bad("✗") };
        println!(
            "  {} {} {}",
            mark,
            out::tool(&s.tool),
            dim(&crate::ui::format::ellipsize(&one_line(&s.observation), 80))
        );
    }
    println!();
    out::print_markdown(&resp.answer);
    println!(
        "\n{} {} · {} steps · {} in / {} out · {}",
        dim("stopped:"),
        resp.stopped,
        resp.steps.len(),
        human_count(din),
        human_count(dout),
        cost(dcost)
    );
    Ok(0)
}

// ---- status / doctor ------------------------------------------------------

async fn status(client: &Client, json: bool) -> Result<i32> {
    let health = client.health().await?;
    let meter = client.meter().await.unwrap_or_default();
    let ledger = client.ledger_verify().await.unwrap_or_default();
    let mem = client.memory_stats().await.unwrap_or_default();
    let cfg = client.config().await.ok();

    if json {
        print_json(&json!({
            "health": { "ok": health.ok, "version": health.version, "offline": health.offline },
            "meter": { "calls": meter.calls, "tokens_in": meter.tokens_in, "tokens_out": meter.tokens_out, "cost_usd": meter.cost_usd },
            "ledger": { "ok": ledger.ok, "entries": ledger.entries },
            "memory_total": mem.total,
            "model": cfg.as_ref().map(|c| c.model_in_use.clone()),
        }));
        return Ok(0);
    }

    out::header("Engram");
    kv("daemon", &format!("{} (v{})", good("up"), health.version));
    kv(
        "model",
        &cfg.as_ref()
            .map(|c| {
                if health.offline {
                    format!("{} {}", c.model_in_use, warn("(offline / mock)"))
                } else {
                    c.model_in_use.clone()
                }
            })
            .unwrap_or_else(|| "—".into()),
    );
    let trust = if ledger.ok {
        good(&format!("verified · {}", ledger.entries))
    } else {
        bad("TAMPER DETECTED")
    };
    kv("ledger", &trust);
    kv(
        "today",
        &format!(
            "{} · {} calls · {} in / {} out",
            cost(meter.cost_usd),
            meter.calls,
            human_count(meter.tokens_in),
            human_count(meter.tokens_out)
        ),
    );
    let regions: Vec<String> = mem
        .by_region
        .iter()
        .map(|(r, n)| format!("{r} {n}"))
        .collect();
    kv(
        "memory",
        &format!("{} total · {}", mem.total, regions.join("  ")),
    );
    Ok(0)
}

async fn doctor(client: &Client, json: bool) -> Result<i32> {
    let health = client.health().await.ok();
    let cfg = client.config_raw().await.ok();
    let ledger = client.ledger_verify().await.ok();
    let tools = client.tools().await.ok();

    if json {
        print_json(&json!({
            "health": health.map(|h| json!({"ok": h.ok, "version": h.version, "offline": h.offline})),
            "config": cfg,
            "ledger": ledger.map(|l| json!({"ok": l.ok, "entries": l.entries})),
            "tools": tools.as_ref().map(|t| t.tools.len()),
        }));
        return Ok(0);
    }

    out::header("Diagnostics");
    match &health {
        Some(h) if h.ok => kv("daemon", &good(&format!("healthy (v{})", h.version))),
        _ => kv("daemon", &bad("unreachable")),
    }
    if let Some(c) = &cfg {
        let provider = c.get("provider").cloned().unwrap_or(Value::Null);
        let kind = provider.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
        let key_set = c
            .get("provider")
            .and_then(|p| p.get("api_key"))
            .map(|k| k != "" && !k.is_null())
            .unwrap_or(false);
        kv(
            "provider",
            &format!(
                "{kind} · key {}",
                if key_set {
                    good("set")
                } else {
                    warn("missing")
                }
            ),
        );
        if let Some(model) = c.get("model_in_use").and_then(|v| v.as_str()) {
            kv("model", model);
        }
        if let Some(sec) = c.get("security") {
            let shell = sec
                .get("allow_shell")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let shell_label = if shell {
                warn("enabled")
            } else {
                "off (safe default)".to_string()
            };
            kv("shell", &shell_label);
        }
        let web = c.get("web").cloned().unwrap_or(Value::Null);
        let providers: Vec<&str> = [
            ("tavily", "tavily_key_set"),
            ("brave", "brave_key_set"),
            ("searxng", "searxng_url"),
        ]
        .iter()
        .filter(|(_, k)| {
            web.get(*k)
                .map(|v| {
                    v.as_bool().unwrap_or(false)
                        || v.as_str().map(|s| !s.is_empty()).unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .map(|(n, _)| *n)
        .collect();
        kv(
            "web search",
            &if providers.is_empty() {
                "DuckDuckGo (keyless)".to_string()
            } else {
                format!("DuckDuckGo + {}", providers.join(", "))
            },
        );
    }
    match &ledger {
        Some(l) if l.ok => kv("ledger", &good(&format!("intact · {} entries", l.entries))),
        Some(_) => kv("ledger", &bad("TAMPERED")),
        None => kv("ledger", &dim("unknown")),
    }
    if let Some(t) = &tools {
        let enabled = t.tools.iter().filter(|x| !x.disabled).count();
        kv("tools", &format!("{enabled}/{} enabled", t.tools.len()));
    }
    Ok(0)
}

// ---- tasks ----------------------------------------------------------------

async fn tasks(client: &Client, cmd: TasksCmd, json: bool) -> Result<i32> {
    match cmd {
        TasksCmd::List => {
            let tasks = client.tasks().await?;
            if json {
                print_json(&tasks.iter().map(|t| json!({
                    "id": t.id, "title": t.title, "status": t.status_or_todo(), "origin": t.origin,
                })).collect::<Vec<_>>());
                return Ok(0);
            }
            for col in ["todo", "doing", "done", "scheduled", "failed"] {
                let items: Vec<&_> = tasks.iter().filter(|t| t.status_or_todo() == col).collect();
                if items.is_empty() {
                    continue;
                }
                out::header(&format!("{} ({})", col.to_uppercase(), items.len()));
                for t in items {
                    let prog = t
                        .progress
                        .as_deref()
                        .map(|p| dim(&format!(" — {p}")))
                        .unwrap_or_default();
                    println!(
                        "  {} {}{}\n    {} {}",
                        accent("•"),
                        bold(&crate::ui::format::ellipsize(&t.title, 70)),
                        prog,
                        dim(&t.id),
                        dim(&rel_time(t.created_ms))
                    );
                }
            }
            Ok(0)
        }
        TasksCmd::Show { id } => {
            let tasks = client.tasks().await?;
            let Some(t) = tasks.into_iter().find(|t| t.id == id) else {
                eprintln!("{}", bad(&format!("no task {id}")));
                return Ok(1);
            };
            if json {
                print_json(&serde_json::to_value(&t).unwrap_or(Value::Null));
                return Ok(0);
            }
            out::header(&t.title);
            kv("id", &t.id);
            kv("status", t.status_or_todo());
            kv("origin", &t.origin);
            if let Some(run) = &t.run {
                kv("stopped", &run.stopped);
                kv(
                    "cost",
                    &format!(
                        "{} · {} in / {} out",
                        cost(run.cost_usd),
                        human_count(run.tokens_in),
                        human_count(run.tokens_out)
                    ),
                );
                println!();
                out::print_markdown(&run.answer);
                if !run.steps.is_empty() {
                    out::header("Audit trail");
                    for s in &run.steps {
                        println!(
                            "  {} {} {} {}",
                            dim(&format!("#{}", s.ledger_seq)),
                            if s.ok { good("✓") } else { bad("✗") },
                            out::tool(&s.tool),
                            dim(&s.ledger_hash.chars().take(12).collect::<String>())
                        );
                    }
                }
            }
            Ok(0)
        }
        TasksCmd::New { title, detail, run } => {
            let title = join(&title);
            if title.is_empty() {
                eprintln!("{}", warn("a task needs a title"));
                return Ok(2);
            }
            let t = client
                .task_create(&title, detail.as_deref(), Some("manual"))
                .await?;
            if run {
                stream_task(client, &t.id, json).await
            } else {
                if json {
                    print_json(&serde_json::to_value(&t).unwrap_or(Value::Null));
                } else {
                    println!("{} {}", good("created"), bold(&t.title));
                    kv("id", &t.id);
                }
                Ok(0)
            }
        }
        TasksCmd::Run { id } => stream_task(client, &id, json).await,
        TasksCmd::Receipt { id } => {
            let r = client.task_receipt(&id).await?;
            print_json(&r);
            Ok(0)
        }
    }
}

async fn stream_task(client: &Client, id: &str, json: bool) -> Result<i32> {
    let mut rx = client.task_run_stream(id);
    while let Some(ev) = rx.recv().await {
        match ev {
            TaskEvent::Step(v) => {
                if !json {
                    let tool = v.get("tool").and_then(|t| t.as_str()).unwrap_or("step");
                    let obs = v
                        .get("observation")
                        .and_then(|o| o.as_str())
                        .map(one_line)
                        .unwrap_or_default();
                    println!(
                        "  {} {}",
                        out::tool(tool),
                        dim(&crate::ui::format::ellipsize(&obs, 80))
                    );
                }
            }
            TaskEvent::Done(t) => {
                let stopped = t
                    .run
                    .as_ref()
                    .map(|r| r.stopped.clone())
                    .unwrap_or_default();
                let failed = t.status_or_todo() == "failed"
                    || matches!(stopped.as_str(), "error" | "budget" | "loop");
                if json {
                    print_json(&serde_json::to_value(&*t).unwrap_or(Value::Null));
                } else if let Some(run) = &t.run {
                    println!();
                    out::print_markdown(&run.answer);
                }
                if failed {
                    if !json {
                        eprintln!(
                            "{}",
                            bad(&format!("task did not succeed (stopped: {stopped})"))
                        );
                    }
                    return Ok(1);
                }
                return Ok(0);
            }
            TaskEvent::Error(e) | TaskEvent::Disconnected(e) => {
                eprintln!("{}", bad(&format!("error: {e}")));
                return Ok(1);
            }
        }
    }
    Ok(0)
}

// ---- memory ---------------------------------------------------------------

async fn memory(client: &Client, cmd: MemoryCmd, json: bool) -> Result<i32> {
    match cmd {
        MemoryCmd::Stats => {
            let s = client.memory_stats().await?;
            if json {
                print_json(
                    &json!({"total": s.total, "by_region": s.by_region, "by_tier": s.by_tier}),
                );
                return Ok(0);
            }
            out::header(&format!("Memory · {} total", s.total));
            for (r, n) in &s.by_region {
                kv(r, &n.to_string());
            }
            let tiers: Vec<String> = s.by_tier.iter().map(|(t, n)| format!("{t} {n}")).collect();
            kv("tiers", &tiers.join("  "));
            Ok(0)
        }
        MemoryCmd::Recent { region, n } => {
            let recs = client.memory_recent(region.as_deref(), n).await?;
            if json {
                print_json(
                    &recs
                        .iter()
                        .map(|r| json!({"id": r.id, "region": r.region, "text": r.text}))
                        .collect::<Vec<_>>(),
                );
                return Ok(0);
            }
            let rows: Vec<Vec<String>> = recs
                .iter()
                .map(|r| {
                    vec![
                        r.id.to_string(),
                        r.region.clone(),
                        r.tier.clone(),
                        one_line(&r.text),
                    ]
                })
                .collect();
            out::header("Recent memories");
            table(&["id", "region", "tier", "text"], &rows);
            Ok(0)
        }
        MemoryCmd::Recall { query, k } => {
            let q = join(&query);
            let hits = client.recall(&q, k, None).await?;
            if json {
                print_json(
                    &hits
                        .iter()
                        .map(|h| {
                            json!({
                                "id": h.record.id, "region": h.record.region, "score": h.score,
                                "arm": h.arm(), "text": h.record.text
                            })
                        })
                        .collect::<Vec<_>>(),
                );
                return Ok(0);
            }
            out::header(&format!("Recall · “{q}”"));
            for h in &hits {
                println!(
                    "  {} {} {}",
                    dim(&format!("{:.3}", h.score)),
                    out::tool(&format!("[{}/{}]", h.record.region, h.arm())),
                    one_line(&h.record.text)
                );
            }
            Ok(0)
        }
        MemoryCmd::Remember {
            text,
            region,
            importance,
        } => {
            let text = join(&text);
            let r = client.remember(&region, &text, importance).await?;
            if json {
                print_json(&r);
            } else {
                println!("{} {}", good("remembered"), dim(&one_line(&text)));
            }
            Ok(0)
        }
        MemoryCmd::Forget { id } => {
            let r = client.forget(id).await?;
            if json {
                print_json(&r);
            } else {
                println!("{} memory {id}", good("forgot"));
            }
            Ok(0)
        }
        MemoryCmd::Identity { distill } => {
            if distill {
                let _ = client.consciousness_distill().await;
            }
            let c = client.consciousness().await?;
            if json {
                print_json(&serde_json::to_value(&c).unwrap_or(Value::Null));
                return Ok(0);
            }
            out::header(&format!("Self-model · v{}", c.version));
            for l in &c.lines {
                println!(
                    "  {} {}",
                    out::tool(&format!("[{}]", l.region.chars().next().unwrap_or('?'))),
                    l.text
                );
            }
            Ok(0)
        }
    }
}

// ---- skills ---------------------------------------------------------------

async fn skills(client: &Client, cmd: SkillsCmd, json: bool) -> Result<i32> {
    match cmd {
        SkillsCmd::List { filter } => {
            let resp = client.skills().await?;
            let f = filter.unwrap_or_default().to_lowercase();
            let mut skills: Vec<_> = resp
                .skills
                .into_iter()
                .filter(|s| {
                    f.is_empty()
                        || s.id.to_lowercase().contains(&f)
                        || s.category.to_lowercase().contains(&f)
                        || s.description.to_lowercase().contains(&f)
                })
                .collect();
            skills.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
            if json {
                print_json(&skills.iter().map(|s| json!({
                    "id": s.id, "category": s.category, "enabled": s.enabled, "runs": s.runs
                })).collect::<Vec<_>>());
                return Ok(0);
            }
            out::header(&format!("Skills ({})", skills.len()));
            let rows: Vec<Vec<String>> = skills
                .iter()
                .map(|s| {
                    vec![
                        if s.enabled {
                            "●".into()
                        } else {
                            "○".into()
                        },
                        s.id.clone(),
                        s.category.clone(),
                        one_line(&s.description),
                    ]
                })
                .collect();
            table(&["", "id", "category", "description"], &rows);
            Ok(0)
        }
        SkillsCmd::Run { id, input } => {
            let input = join(&input);
            let r = client.skill_run(&id, &input).await?;
            if json {
                print_json(&serde_json::to_value(&r).unwrap_or(Value::Null));
            } else {
                println!("{}", r.output);
                if r.duration_us > 0 {
                    println!("{}", dim(&format!("· {}µs", r.duration_us)));
                }
            }
            Ok(0)
        }
        SkillsCmd::Enable { id } => {
            client.skill_set_enabled(&id, true).await?;
            println!("{} {id}", good("enabled"));
            Ok(0)
        }
        SkillsCmd::Disable { id } => {
            client.skill_set_enabled(&id, false).await?;
            println!("{} {id}", warn("disabled"));
            Ok(0)
        }
    }
}

// ---- schedule -------------------------------------------------------------

async fn schedule(client: &Client, cmd: ScheduleCmd, json: bool) -> Result<i32> {
    match cmd {
        ScheduleCmd::List => {
            let jobs = client.schedule().await?;
            if json {
                print_json(
                    &jobs
                        .iter()
                        .map(|j| serde_json::to_value(j).unwrap())
                        .collect::<Vec<_>>(),
                );
                return Ok(0);
            }
            out::header(&format!("Schedule ({})", jobs.len()));
            for j in &jobs {
                let next = j.next_fire_ms.map(stamp).unwrap_or_else(|| "—".into());
                println!(
                    "  {} {}\n    {} next {} · {}",
                    accent("◷"),
                    bold(&j.name),
                    dim(&j.id),
                    next,
                    dim(&describe_recurrence(&j.recurrence))
                );
            }
            Ok(0)
        }
        ScheduleCmd::Add { name, when, title } => {
            // The daemon falls back to the job name when no title is given; send an
            // empty object rather than a bare-string sentinel.
            let payload = title.map(|t| json!({ "title": t })).unwrap_or(json!({}));
            let r = client.schedule_add(&name, &when, payload).await?;
            if json {
                print_json(&r);
            } else {
                println!("{} {name}", good("scheduled"));
            }
            Ok(0)
        }
        ScheduleCmd::Preview { when } => {
            let when = join(&when);
            let p = client.schedule_preview(&when).await?;
            if json {
                print_json(&serde_json::to_value(&p).unwrap_or(Value::Null));
                return Ok(0);
            }
            if p.ok {
                let next = p.next_fire_ms.map(stamp).unwrap_or_else(|| "—".into());
                println!("{} {} → next fire {}", good("ok"), bold(&when), next);
            } else {
                println!(
                    "{} {}",
                    bad("can't parse"),
                    dim(&p.error.unwrap_or_default())
                );
            }
            Ok(0)
        }
        ScheduleCmd::Run { id } => {
            let r = client.schedule_run(&id).await?;
            if json {
                print_json(&r);
            } else {
                println!("{} {id}", good("fired"));
            }
            Ok(0)
        }
        ScheduleCmd::Delete { id } => {
            let r = client.schedule_remove(&id).await?;
            if json {
                print_json(&r);
            } else {
                println!("{} {id}", warn("deleted"));
            }
            Ok(0)
        }
    }
}

fn describe_recurrence(v: &Value) -> String {
    let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    match kind {
        "daily" => {
            let h = v.get("hour").and_then(|x| x.as_i64()).unwrap_or(0);
            let m = v.get("min").and_then(|x| x.as_i64()).unwrap_or(0);
            format!("daily at {h:02}:{m:02}")
        }
        "" => one_line(&v.to_string()),
        other => other.to_string(),
    }
}

// ---- autonomy -------------------------------------------------------------

async fn autonomy(client: &Client, cmd: AutonomyCmd, json: bool) -> Result<i32> {
    match cmd {
        AutonomyCmd::Report => {
            let r = client.autonomy_report().await?;
            if json {
                print_json(&serde_json::to_value(&r).unwrap_or(Value::Null));
                return Ok(0);
            }
            out::header("Autonomy");
            let t = &r.totals;
            kv("autonomous sends", &t.autonomous_sends.to_string());
            kv("staged", &t.staged.to_string());
            kv("allowlisted", &t.allowlisted.to_string());
            kv("refused", &t.refused.to_string());
            kv("denied", &t.denied.to_string());
            kv("one-time approvals", &r.one_time_approvals.to_string());
            Ok(0)
        }
        AutonomyCmd::Pending => {
            let p = client.egress_pending().await?;
            if json {
                print_json(&serde_json::to_value(&p).unwrap_or(Value::Null));
                return Ok(0);
            }
            if p.pending.is_empty() {
                println!("{}", dim("no pending egress approvals"));
                return Ok(0);
            }
            out::header(&format!("Pending egress ({})", p.pending.len()));
            for e in &p.pending {
                println!(
                    "  {} {} → {}\n    {} {}",
                    warn("⚠"),
                    out::tool(&e.tool),
                    bold(&e.dest),
                    dim(&e.scope),
                    dim(&e.reason)
                );
            }
            println!(
                "\n{}",
                dim("approve with: engram autonomy approve <scope> <dest>")
            );
            Ok(0)
        }
        AutonomyCmd::Approve { scope, dest } => {
            client.egress_approve(&scope, &dest).await?;
            println!("{} {dest}", good("approved"));
            Ok(0)
        }
        AutonomyCmd::Deny { scope, dest } => {
            client.egress_deny(&scope, &dest).await?;
            println!("{} {dest}", warn("denied"));
            Ok(0)
        }
    }
}

// ---- ledger ---------------------------------------------------------------

async fn ledger(client: &Client, cmd: LedgerCmd, json: bool) -> Result<i32> {
    match cmd {
        LedgerCmd::Tail { n } => {
            let entries = client.ledger_tail(n).await?;
            if json {
                print_json(
                    &entries
                        .iter()
                        .map(|e| serde_json::to_value(e).unwrap())
                        .collect::<Vec<_>>(),
                );
                return Ok(0);
            }
            out::header(&format!("Ledger · last {}", entries.len()));
            for e in &entries {
                println!(
                    "  {} {} {} {}",
                    dim(&format!("#{:<6}", e.seq)),
                    out::tool(&format!(
                        "{:<22}",
                        crate::ui::format::ellipsize(&e.kind, 22)
                    )),
                    dim(&format!("{:<8}", e.actor)),
                    dim(&e.hash.chars().take(12).collect::<String>())
                );
            }
            Ok(0)
        }
        LedgerCmd::Verify => {
            let v = client.ledger_verify().await?;
            if json {
                print_json(&json!({"ok": v.ok, "entries": v.entries}));
                return Ok(0);
            }
            if v.ok {
                println!("{} {} entries, chain intact", good("✓ verified"), v.entries);
                Ok(0)
            } else {
                println!(
                    "{}",
                    bad("✗ TAMPER DETECTED — the audit chain failed to verify")
                );
                Ok(1)
            }
        }
        LedgerCmd::Pubkey => {
            let k = client.ledger_pubkey().await?;
            print_json(&k);
            Ok(0)
        }
    }
}

// ---- config / agents / tools ----------------------------------------------

async fn config(client: &Client, cmd: ConfigCmd, json: bool) -> Result<i32> {
    match cmd {
        ConfigCmd::Show => {
            let c = client.config_raw().await?;
            if json {
                print_json(&c);
            } else {
                print_json(&c); // config is best shown as JSON
            }
            Ok(0)
        }
        ConfigCmd::Set { key, value } => {
            // Build a nested patch from a dotted key.
            let parsed: Value =
                serde_json::from_str(&value).unwrap_or(Value::String(value.clone()));
            let mut patch = parsed;
            for part in key.split('.').rev() {
                patch = json!({ part: patch });
            }
            let r = client.config_set(patch).await?;
            if json {
                print_json(&r);
            } else {
                println!("{} {key}", good("set"));
                if r.get("restart_needed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    println!("{}", warn("· restart the daemon for this to take effect"));
                }
            }
            Ok(0)
        }
        ConfigCmd::Test => {
            let r = client.config_test(serde_json::json!({})).await?;
            let ok = r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
            if json {
                print_json(&r);
                return Ok(if ok { 0 } else { 1 });
            }
            if ok {
                let model = r.get("model").and_then(|v| v.as_str()).unwrap_or("?");
                let reply = r.get("reply").and_then(|v| v.as_str()).unwrap_or("");
                println!(
                    "{} {} replied “{}”",
                    good("✓ provider ok —"),
                    model,
                    one_line(reply)
                );
                Ok(0)
            } else {
                let err = r.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
                println!("{} {}", bad("✗ provider error:"), err);
                Ok(1)
            }
        }
    }
}

async fn agents(client: &Client, cmd: Option<AgentsCmd>, json: bool) -> Result<i32> {
    match cmd.unwrap_or(AgentsCmd::List) {
        AgentsCmd::List => {
            let arr = client.agents_list().await?;
            if json {
                print_json(&arr);
                return Ok(0);
            }
            if arr.is_empty() {
                println!("{}", dim("no named agents yet"));
                return Ok(0);
            }
            out::header(&format!("Agents ({})", arr.len()));
            for a in &arr {
                let name = a.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                let role = a.get("role").and_then(|x| x.as_str()).unwrap_or("");
                let model = a.get("model").and_then(|x| x.as_str()).unwrap_or("");
                let emoji = a.get("emoji").and_then(|x| x.as_str()).unwrap_or("•");
                let id = a.get("id").and_then(|x| x.as_str()).unwrap_or("");
                println!(
                    "  {emoji} {}  {}  {}\n    {}",
                    bold(name),
                    out::tool(model),
                    dim(&one_line(role)),
                    dim(id)
                );
            }
            Ok(0)
        }
        AgentsCmd::Create {
            name,
            role,
            model,
            provider,
            emoji,
        } => {
            let body = json!({
                "name": name,
                "role": role.unwrap_or_default(),
                "model": model.unwrap_or_default(),
                "provider": provider.unwrap_or_default(),
                "emoji": emoji.unwrap_or_default(),
            });
            let r = client.agents_create(body).await?;
            if json {
                print_json(&r);
            } else {
                println!("{} {name}", good("created"));
            }
            Ok(0)
        }
        AgentsCmd::Edit {
            id,
            role,
            model,
            provider,
            emoji,
        } => {
            // Only send the fields that were provided.
            let mut m = serde_json::Map::new();
            for (k, v) in [
                ("role", role),
                ("model", model),
                ("provider", provider),
                ("emoji", emoji),
            ] {
                if let Some(v) = v {
                    m.insert(k.into(), json!(v));
                }
            }
            let r = client.agents_update(&id, Value::Object(m)).await?;
            if json {
                print_json(&r);
            } else {
                println!("{} {id}", good("updated"));
            }
            Ok(0)
        }
        AgentsCmd::Delete { id } => {
            client.agents_delete(&id).await?;
            println!("{} {id}", warn("deleted"));
            Ok(0)
        }
        AgentsCmd::Policy {
            id,
            egress,
            actions,
            max_actions,
            max_spend_cents,
            expires_days,
        } => {
            let csv = |s: Option<String>| -> Vec<String> {
                s.unwrap_or_default()
                    .split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            };
            let egress_v = csv(egress);
            // The daemon revokes the policy unless there's an allowlist or a
            // positive action cap — warn rather than silently no-op.
            if egress_v.is_empty() && max_actions == 0 {
                eprintln!(
                    "{}",
                    warn("policy revoked: set --egress or --max-actions to enable autonomy")
                );
                return Ok(2);
            }
            let mut body = json!({
                "enabled": true,
                "allowed_egress": egress_v,
                "allowed_actions": csv(actions),
                "max_actions": max_actions,
            });
            if let Some(m) = max_spend_cents {
                body["max_spend_cents"] = json!(m);
            }
            if let Some(e) = expires_days {
                body["expires_days"] = json!(e);
            }
            let r = client.agent_set_policy(&id, body).await?;
            if json {
                print_json(&r);
            } else {
                println!("{} {id}", good("policy set"));
            }
            Ok(0)
        }
    }
}

async fn tools(client: &Client, json: bool) -> Result<i32> {
    let t = client.tools().await?;
    if json {
        print_json(&serde_json::to_value(&t.tools).unwrap_or(Value::Null));
        return Ok(0);
    }
    out::header(&format!("Tools ({})", t.tools.len()));
    for tool in &t.tools {
        let mark = if tool.disabled {
            dim("○")
        } else {
            good("●")
        };
        println!(
            "  {} {}\n    {}",
            mark,
            bold(&tool.name),
            dim(&one_line(&tool.description))
        );
    }
    Ok(0)
}

// ---- events ---------------------------------------------------------------

async fn events(client: &Client) -> Result<i32> {
    out::header("Live events (Ctrl-C to stop)");
    let mut rx = client.events_stream();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            ev = rx.recv() => match ev {
                Some(s) if s.topic == "__disconnected" => break,
                Some(s) => println!("  {} {}", out::tool(&s.topic), dim(&one_line(&s.payload.to_string()))),
                None => break,
            }
        }
    }
    Ok(0)
}

// ---- serve / completions --------------------------------------------------

pub async fn serve(client: &Client, detach: bool) -> Result<i32> {
    if super::daemon::is_up(client).await {
        println!("{} already running at {}", good("✓"), client.base_url());
        return Ok(0);
    }
    super::daemon::spawn_and_wait(client, false).await?;
    if detach {
        return Ok(0);
    }
    println!("{}", dim("· attached — Ctrl-C to leave the daemon running"));
    let _ = tokio::signal::ctrl_c().await;
    println!("\n{}", dim("· detached (daemon keeps running until idle)"));
    Ok(0)
}

pub fn completions(shell: clap_complete::Shell) {
    use clap::CommandFactory;
    let mut cmd = super::Cli::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
}
