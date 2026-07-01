# `engram` — the command line & terminal UI

`engram` is the terminal client for the Engram daemon (`engramd`). It is a single
small Rust binary (`crates/engram-cli`) that talks to the daemon over its local
HTTP API. Run it with no arguments for a full-screen TUI, or use a subcommand for
scripting. If the daemon isn't already running, `engram` starts it for you and
waits for it to come up — then gets out of the way (the daemon sleeps itself to
zero after its idle window, exactly as it does for the desktop app).

```
cargo build --release -p engram-cli      # produces target/release/engram
engram                                    # opens the TUI
engram status                             # one-line health / cost / ledger summary
```

The client reads `ENGRAM_ADDR` (default `127.0.0.1:8088`) and `ENGRAM_API_TOKEN`
(only needed when the daemon is exposed off-localhost); both can be overridden with
`--addr` and `--token`. `--json` switches any subcommand to machine-readable output,
and `--no-spawn` makes it fail instead of auto-starting the daemon.

## The TUI

A calm, dark, full-screen control surface with everything the desktop control
center has, in the terminal:

- **A trust spine in the header** — the model in use, today's cost and token flow,
  a live **`✓ ledger N`** chip (which flips to a red **`✗ TAMPER`** banner the moment
  the signed audit chain fails to verify), and a connection dot.
- **Streaming chat** — the same tool-using agent the task board runs. Tool steps and
  the model's interim narration stream live; the finished answer renders as real
  Markdown (headings, tables, code, lists, links). A **recall ribbon** under each
  answer shows exactly which memories grounded it, tinted by region.
- **A command palette** (`Ctrl-P` / `Ctrl-K`, or `/` from an empty composer) for
  jumping between views and running actions (verify ledger, re-distill the
  self-model, new session, toggle theme).
- **A kanban Tasks board** (To do / Running / Done) with glass-box detail cards that
  show a finished run's answer *and* its signed ledger slice (each step paired with
  its sequence number and BLAKE3 hash).
- **Memory** — the brain's regions and tiers, the distilled self-model, and a
  recent-memories list you can forget from.
- **Skills** — the self-improving program library, with enable toggles and per-skill
  detail (capabilities, runtime, version history, learning record).
- **Schedule**, **Agents** (create with `n`, delete with `d`), and an **Autonomy**
  view where staged egress actions wait for your approval (`a` to allowlist, `d` to
  deny) — the graduated-autonomy gate, in the terminal.
- **Ledger** — the signed, append-only audit chain with a live verification chip and
  a payload preview for the selected entry.
- **Settings** — an editable browser over the daemon config: model provider, keys,
  security flags, web-search providers, media models, browser, channels, and the
  **MCP servers** list (Enter to add/edit a server's name·command·args·env, `d` to
  delete). **Enter** edits/toggles/cycles a field, **`x`** clears a secret, **`t`** runs
  a live provider test. Secrets show `● set` / `○ unset` and follow the "blank keeps it" rule.

Answers render with **syntax-highlighted code blocks** and coloured **diffs** (` ```diff `
or unified-diff content) — in both the TUI chat pane and `engram ask` output.

### Keys

| Context | Key | Action |
|---|---|---|
| Global | `Ctrl-P` / `Ctrl-K` | command palette |
| Global | `/` | palette (from an empty chat composer, or any list) |
| Global | `Alt-1` … `Alt-9` | jump to a tab |
| Global | `?` / `F1` | help |
| Global | `Ctrl-C` | close an open overlay, else quit · `Ctrl-Q` quit |
| Chat | `Enter` | send |
| Chat | `Ctrl-T` / `Ctrl-A` / `Ctrl-R` / `Ctrl-Y` | task / attach file / resume session / copy last answer |
| Chat | `Esc` | stop a streaming run |
| Chat | `↑ ↓` / `PgUp` `PgDn` | scroll the transcript |
| Chat | `Ctrl-U` / `Ctrl-W` | clear line / delete word |
| Lists | `↑ ↓` or `j k` | move selection |
| Tasks | `← →` or `h l` · `c` | switch kanban column · cancel a running task |
| Lists | `Enter` | run / open / approve / edit |
| Lists | `r` | refresh |
| Memory | `f` | forget the selected memory (×2 to confirm) |
| Autonomy | `a` / `d` | approve / deny a staged egress |
| Agents | `n` / `e` / `p` / `d` | create / edit / set autonomy policy / delete (×2) |
| Schedule | `a` / `Enter` / `d` | add a job / run now / delete |
| Settings | `Enter` / `x` / `t` | edit/toggle/cycle · clear secret · test provider |
| Mouse | click / wheel | click a tab, wheel-scroll (palette "Toggle mouse" to disable) |

## CLI subcommands

```
engram ask <prompt…>            # chat in one shot; streams tool steps then the answer
engram run <task…>              # run the tool-using agent (one-shot, non-interactive)
engram status                   # health, cost, ledger, memory summary
engram doctor                   # provider, tools, ledger integrity, config

engram tasks list|show <id>|new <title> [--run]|run <id>|receipt <id>
engram memory stats|recent|recall <q>|remember <text>|forget <id>|identity
engram skills list [--filter]|run <id> <input>|enable <id>|disable <id>
engram schedule list|add <name> <when>|preview <when…>|run <id>|delete <id>
engram autonomy report|pending|approve <scope> <dest>|deny <scope> <dest>
engram agents list|create <name> [--role --model --provider --emoji]|edit <id>|delete <id>|policy <id> [--egress --actions --max-actions --max-spend-cents --expires-days]
engram ledger tail|verify|pubkey
engram config show|set <key> <value>|test
engram tools | events

engram serve [--detach]         # start the daemon
engram completions <shell>      # bash | zsh | fish | powershell | elvish
```

Every subcommand honours `--json` for piping into other tools. `ask` and `run` read
the prompt from stdin if you don't pass one, so `echo "…" | engram ask` works.

### Examples

```bash
engram ask "what do you know about me?"
engram recall "favorite watch" --k 5            # hybrid keyword+semantic recall
engram tasks new "draft a weekly digest" --run  # create and stream the run
engram ledger verify                            # exit 0 = chain intact, 1 = tampered
engram schedule preview "every weekday at 9am"  # next-fire preview, no model call
engram status --json | jq .ledger               # scriptable everything
```

## How it fits

The terminal client is a thin, decoupled HTTP consumer — it shares no process with
the daemon and stores nothing itself. The daemon remains the single audited
choke-point: every action the agent takes still lands in the signed ledger first,
whether it was driven from the desktop, a messaging channel, or this CLI. The TUI
just gives you a fast, keyboard-first window onto the same brain.
