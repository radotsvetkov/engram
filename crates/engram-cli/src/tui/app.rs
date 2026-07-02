//! TUI application state and the update logic that drives it.

use crate::api::{
    AutonomyReport, ChatEvent, Client, Consciousness, EgressItem, Health, LedgerEntry,
    LedgerVerify, MemRecord, MemoryStats, Meter, Project, RecalledRef, ScheduleJob, SessionMeta,
    SessionMsg, Skill, StepRecord, Task,
};
use crate::ui::format::now_ms;
use crate::ui::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use serde_json::{json, Value};
use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;

/// The top-level views, reachable from the command palette or hotkeys.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Chat,
    Tasks,
    Memory,
    Skills,
    Schedule,
    Autonomy,
    Ledger,
    Agents,
    Settings,
    Help,
}

impl View {
    pub const TABS: [View; 9] = [
        View::Chat,
        View::Tasks,
        View::Memory,
        View::Skills,
        View::Schedule,
        View::Autonomy,
        View::Ledger,
        View::Agents,
        View::Settings,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            View::Chat => "Chat",
            View::Tasks => "Tasks",
            View::Memory => "Memory",
            View::Skills => "Skills",
            View::Schedule => "Schedule",
            View::Autonomy => "Autonomy",
            View::Ledger => "Ledger",
            View::Agents => "Agents",
            View::Settings => "Settings",
            View::Help => "Help",
        }
    }
}

/// Messages background tasks push back into the UI loop.
pub enum Msg {
    Spine {
        health: Option<Health>,
        meter: Option<Meter>,
        ledger: Option<LedgerVerify>,
        model: Option<String>,
    },
    Tasks(Vec<Task>),
    MemoryRecent(Vec<MemRecord>),
    MemoryStats(MemoryStats),
    Consciousness(Consciousness),
    Skills(Vec<Skill>),
    Schedule(Vec<ScheduleJob>),
    LedgerTail(Vec<LedgerEntry>),
    Autonomy(AutonomyReport),
    Egress(Vec<EgressItem>),
    Agents(Vec<Value>),
    Config(Value),
    Sessions(Vec<SessionMeta>),
    /// The workspace project list (for the switcher + status bar).
    Projects(Vec<Project>),
    /// A newly-created project (switch to it, start a session in it).
    ProjectCreated(Project),
    /// A real server-side session was created under the active project (chat is now scoped to it).
    SessionStarted(String),
    SessionLoaded {
        id: String,
        msgs: Vec<SessionMsg>,
        /// The resumed session's project, so the active scope + footer badge follow the chat.
        project_id: Option<String>,
    },
    /// A Ctrl-T task creation failed; restore the user's text and tell them.
    TaskCreateFailed {
        title: String,
        err: String,
    },
    /// A daemon reconnect attempt finished (true = the daemon is back up).
    Reconnected(bool),
    /// The task-cancel path finished releasing the daemon-wide kill switch; clear the halted state.
    TaskHaltReleased,
    /// Model-only spine update (so it doesn't clobber the health probe).
    Model(String),
    Chat(ChatEvent),
    Toast(String),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Role {
    User,
    Engram,
}

/// One rendered turn in the transcript.
pub struct Turn {
    pub role: Role,
    pub text: String,
    pub recalled: Vec<RecalledRef>,
    pub learned: Vec<String>,
    pub steps: Vec<StepRecord>,
    pub plan: Vec<PlanStep>,
    pub error: bool,
}

/// One item of the agent's live plan (from the `update_plan` tool).
#[derive(Clone)]
pub struct PlanStep {
    pub title: String,
    pub status: String, // "todo" | "doing" | "done"
}

/// A single step observed live while a turn streams.
pub struct LiveStep {
    pub tool: String,
    pub ok: bool,
    pub observation: String,
}

/// The chat transcript + composer state.
#[derive(Default)]
pub struct ChatState {
    pub turns: Vec<Turn>,
    pub session: Option<String>,
    pub streaming: bool,
    pub live_steps: Vec<LiveStep>,
    pub live_narration: Vec<String>,
    pub live_plan: Vec<PlanStep>,
    pub composer: String,
    pub cursor: usize, // char index into composer
    pub pending_attachments: Vec<Attachment>,
    pub scroll: u16,
    pub stick: bool,
    /// Total rendered transcript lines from the last draw (for scroll clamping).
    pub last_total: u16,
    pub last_viewport: u16,
}

/// A palette action.
#[derive(Clone)]
pub enum Action {
    Go(View),
    ClearChat,
    NewSession,
    NewProject,
    SwitchProject,
    ResumeSession,
    SwitchModel,
    Verify,
    Distill,
    Refresh,
    ToggleTheme,
    ToggleMouse,
    CopyAnswer,
    Quit,
}

/// A modal text prompt (e.g. switching the model).
pub struct PromptModal {
    pub title: String,
    pub buffer: String,
    pub cursor: usize,
    pub kind: PromptKind,
}

pub enum PromptKind {
    /// Quick model switch from the palette.
    SetModel,
    /// Edit a config field; `secret` masks input and uses the "blank keeps it"
    /// rule, `number` parses a u64.
    SetConfig {
        section: String,
        field: String,
        secret: bool,
        number: bool,
    },
    /// Attach a file by path to the next chat message.
    AddAttachment,
}

/// A file/url pinned to the next chat message.
#[derive(Clone)]
pub struct Attachment {
    pub kind: String,
    pub name: String,
    pub text: String,
}

/// One labelled field in a multi-field form modal.
pub struct FormField {
    pub label: &'static str,
    pub value: String,
    pub hint: &'static str,
    pub number: bool,
}

impl FormField {
    fn new(label: &'static str, value: impl Into<String>, hint: &'static str) -> Self {
        FormField {
            label,
            value: value.into(),
            hint,
            number: false,
        }
    }
    fn num(label: &'static str, value: impl Into<String>, hint: &'static str) -> Self {
        FormField {
            label,
            value: value.into(),
            hint,
            number: true,
        }
    }
}

/// A multi-field form overlay (create/edit agent, autonomy policy, add schedule).
pub struct FormModal {
    pub title: String,
    pub fields: Vec<FormField>,
    pub sel: usize,
    pub cursor: usize, // char cursor in the focused field
    pub kind: FormKind,
}

pub enum FormKind {
    CreateAgent,
    NewProject,
    EditAgent {
        id: String,
    },
    SetPolicy {
        id: String,
    },
    AddSchedule,
    /// Add (None) or edit (Some(index)) an MCP server.
    Mcp {
        index: Option<usize>,
    },
}

pub struct PaletteItem {
    pub label: &'static str,
    pub hint: &'static str,
    pub action: Action,
}

pub struct Palette {
    pub query: String,
    pub sel: usize,
}

pub struct App {
    pub client: Client,
    pub tx: UnboundedSender<Msg>,
    pub should_quit: bool,
    pub view: View,
    pub theme: Theme,
    pub light: bool,
    pub tick: usize,

    // trust spine
    pub health: Option<Health>,
    pub meter: Meter,
    pub ledger: Option<LedgerVerify>,
    pub model: String,

    // chat
    pub chat: ChatState,

    // view data
    pub tasks: Vec<Task>,
    pub memory_recent: Vec<MemRecord>,
    pub memory_stats: MemoryStats,
    pub consciousness: Option<Consciousness>,
    pub skills: Vec<Skill>,
    pub schedule: Vec<ScheduleJob>,
    pub ledger_tail: Vec<LedgerEntry>,
    pub autonomy: Option<AutonomyReport>,
    pub egress: Vec<EgressItem>,
    pub agents: Vec<Value>,
    pub sessions: Vec<SessionMeta>,
    /// The workspace projects, and which one is active. Chats run under the active project, so their
    /// memory and working directory are scoped to it.
    pub projects: Vec<Project>,
    pub active_project: Option<String>,
    /// The project switcher overlay.
    pub project_picker_open: bool,
    pub project_sel: usize,
    /// The full (redacted) daemon config, for the Settings view.
    pub config_raw: Option<Value>,

    // selection per list view
    pub sel: usize,
    pub board_col: usize,
    pub detail_open: bool,
    pub detail_scroll: u16,
    /// True while a task cancel has latched the daemon-wide kill switch and the client is about to
    /// release it. Surfaces a visible "halted — press c to release" state; `c` releases immediately.
    pub task_halted: bool,
    /// The view to return to from Help (Esc).
    pub prev_view: View,
    /// A memory id armed for deletion — a second `f` within the window confirms.
    pub confirm_forget: Option<i64>,
    /// An agent id armed for deletion — a second `d` confirms.
    pub confirm_agent: Option<String>,

    // command palette
    pub palette: Option<Palette>,

    // session resume picker
    pub sessions_open: bool,
    pub sessions_sel: usize,

    // modal text prompt (model switch, …)
    pub prompt_modal: Option<PromptModal>,
    // multi-field form (agent create/edit, policy, schedule add)
    pub form: Option<FormModal>,

    // daemon reconnect (the idle daemon sleeps to zero; bring it back)
    pub reconnecting: bool,
    pub was_up: bool,
    pub mouse: bool,
    /// Clickable tab regions `(view, x_start, x_end)`, recorded during header draw.
    pub tab_hits: Vec<(View, u16, u16)>,
    /// Row count of the active list view (set during render; for wheel scrolling).
    pub list_len: usize,

    // ephemeral status toast
    pub toast: Option<(String, Instant)>,
}

pub const PALETTE: &[PaletteItem] = &[
    PaletteItem {
        label: "Chat",
        hint: "talk to your agent",
        action: Action::Go(View::Chat),
    },
    PaletteItem {
        label: "Tasks",
        hint: "the kanban board",
        action: Action::Go(View::Tasks),
    },
    PaletteItem {
        label: "Memory",
        hint: "brain regions & recall",
        action: Action::Go(View::Memory),
    },
    PaletteItem {
        label: "Skills",
        hint: "self-improving programs",
        action: Action::Go(View::Skills),
    },
    PaletteItem {
        label: "Schedule",
        hint: "recurring jobs",
        action: Action::Go(View::Schedule),
    },
    PaletteItem {
        label: "Autonomy",
        hint: "egress policy & approvals",
        action: Action::Go(View::Autonomy),
    },
    PaletteItem {
        label: "Ledger",
        hint: "signed audit chain",
        action: Action::Go(View::Ledger),
    },
    PaletteItem {
        label: "Agents",
        hint: "named agents",
        action: Action::Go(View::Agents),
    },
    PaletteItem {
        label: "Settings",
        hint: "provider, security, web, media…",
        action: Action::Go(View::Settings),
    },
    PaletteItem {
        label: "Help",
        hint: "keys & commands",
        action: Action::Go(View::Help),
    },
    PaletteItem {
        label: "New session",
        hint: "start a fresh chat",
        action: Action::NewSession,
    },
    PaletteItem {
        label: "New project",
        hint: "a named world with its own memory + working directory",
        action: Action::NewProject,
    },
    PaletteItem {
        label: "Switch project",
        hint: "change the active project (scopes the chat's memory + files)",
        action: Action::SwitchProject,
    },
    PaletteItem {
        label: "Resume session",
        hint: "reopen a past conversation",
        action: Action::ResumeSession,
    },
    PaletteItem {
        label: "Switch model",
        hint: "change the model the agent uses",
        action: Action::SwitchModel,
    },
    PaletteItem {
        label: "Clear chat",
        hint: "empty the transcript",
        action: Action::ClearChat,
    },
    PaletteItem {
        label: "Verify ledger",
        hint: "re-check the audit chain",
        action: Action::Verify,
    },
    PaletteItem {
        label: "Distill self-model",
        hint: "re-derive consciousness",
        action: Action::Distill,
    },
    PaletteItem {
        label: "Refresh",
        hint: "reload the current view",
        action: Action::Refresh,
    },
    PaletteItem {
        label: "Toggle theme",
        hint: "dark / light",
        action: Action::ToggleTheme,
    },
    PaletteItem {
        label: "Copy last answer",
        hint: "yank the latest reply to the clipboard",
        action: Action::CopyAnswer,
    },
    PaletteItem {
        label: "Toggle mouse",
        hint: "mouse on lets you click & wheel-scroll; off restores text selection",
        action: Action::ToggleMouse,
    },
    PaletteItem {
        label: "Quit",
        hint: "exit engram",
        action: Action::Quit,
    },
];

impl App {
    pub fn new(client: Client, tx: UnboundedSender<Msg>) -> Self {
        App {
            client,
            tx,
            should_quit: false,
            view: View::Chat,
            theme: Theme::dark(),
            light: false,
            tick: 0,
            health: None,
            meter: Meter::default(),
            ledger: None,
            model: String::new(),
            chat: ChatState {
                stick: true,
                ..Default::default()
            },
            tasks: vec![],
            memory_recent: vec![],
            memory_stats: MemoryStats::default(),
            consciousness: None,
            skills: vec![],
            schedule: vec![],
            ledger_tail: vec![],
            autonomy: None,
            egress: vec![],
            agents: vec![],
            sessions: vec![],
            projects: vec![],
            active_project: None,
            project_picker_open: false,
            project_sel: 0,
            config_raw: None,
            sel: 0,
            board_col: 0,
            detail_open: false,
            detail_scroll: 0,
            task_halted: false,
            prev_view: View::Chat,
            confirm_forget: None,
            confirm_agent: None,
            palette: None,
            sessions_open: false,
            sessions_sel: 0,
            prompt_modal: None,
            form: None,
            reconnecting: false,
            was_up: false,
            mouse: true,
            tab_hits: Vec::new(),
            list_len: 0,
            toast: None,
        }
    }

    // ---- background fetch plumbing ----------------------------------------

    fn fetch<F, Fut>(&self, f: F)
    where
        F: FnOnce(Client) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Option<Msg>> + Send + 'static,
    {
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Some(msg) = f(client).await {
                let _ = tx.send(msg);
            }
        });
    }

    pub fn bootstrap(&mut self) {
        self.refresh_spine();
        self.load_view(self.view);
        // Tasks power chat-side counts too; pull once up front.
        self.refresh_tasks();
        // Load the projects so the switcher + status bar work, and the first chat runs scoped.
        self.refetch_projects();
    }

    fn refetch_projects(&self) {
        self.fetch(|c| async move { c.projects().await.ok().map(Msg::Projects) });
    }

    /// Start a REAL server-side session under `project_id`, so the chat's memory + working directory
    /// are scoped to that project (unlike the client-only fallback id used before any project loads).
    fn start_session_in(&mut self, project_id: String) {
        self.chat.turns.clear();
        self.chat.session = None;
        self.chat.scroll = 0;
        self.chat.stick = true;
        self.view = View::Chat;
        self.fetch(move |c| async move {
            c.session_create(&project_id, None)
                .await
                .ok()
                .map(|s| Msg::SessionStarted(s.id))
        });
    }

    /// The active project's display name (for the status bar / switcher).
    pub fn active_project_name(&self) -> String {
        match &self.active_project {
            Some(id) => self
                .projects
                .iter()
                .find(|p| &p.id == id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| id.clone()),
            None => "—".into(),
        }
    }

    pub fn open_project_picker(&mut self) {
        self.project_picker_open = true;
        self.project_sel = self
            .projects
            .iter()
            .position(|p| Some(&p.id) == self.active_project.as_ref())
            .unwrap_or(0);
        self.refetch_projects();
    }

    fn project_picker_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => self.project_picker_open = false,
            KeyCode::Up | KeyCode::Char('k') => {
                self.project_sel = self.project_sel.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.project_sel + 1 < self.projects.len() {
                    self.project_sel += 1;
                }
            }
            KeyCode::Char('n') => {
                // Quick "new project" straight from the switcher.
                self.project_picker_open = false;
                self.create_project_prompt();
            }
            KeyCode::Enter => {
                if let Some(p) = self.projects.get(self.project_sel) {
                    let (id, name) = (p.id.clone(), p.name.clone());
                    self.project_picker_open = false;
                    self.active_project = Some(id.clone());
                    self.toast(&format!("· project · {name}"));
                    self.start_session_in(id);
                }
            }
            _ => {}
        }
    }

    pub fn refresh_spine(&self) {
        self.fetch(|c| async move {
            let (health, meter, ledger, cfg) =
                tokio::join!(c.health(), c.meter(), c.ledger_verify(), c.config());
            Some(Msg::Spine {
                health: health.ok(),
                meter: meter.ok(),
                ledger: ledger.ok(),
                model: cfg.ok().map(|c| c.model_in_use),
            })
        });
    }

    fn refresh_tasks(&self) {
        self.fetch(|c| async move { c.tasks().await.ok().map(Msg::Tasks) });
    }

    pub fn load_view(&self, view: View) {
        match view {
            View::Chat => {
                // Scope the session list to the active project, else the daemon defaults to
                // "personal" and project-scoped chats are invisible in the resume picker.
                let project = self.active_project.clone();
                self.fetch(move |c| async move {
                    c.sessions(project.as_deref())
                        .await
                        .ok()
                        .map(Msg::Sessions)
                });
            }
            View::Tasks => self.refresh_tasks(),
            View::Memory => {
                self.fetch(|c| async move {
                    c.memory_recent(None, 40).await.ok().map(Msg::MemoryRecent)
                });
                self.fetch(|c| async move { c.memory_stats().await.ok().map(Msg::MemoryStats) });
                self.fetch(|c| async move { c.consciousness().await.ok().map(Msg::Consciousness) });
            }
            View::Skills => {
                self.fetch(|c| async move { c.skills().await.ok().map(|s| Msg::Skills(s.skills)) });
            }
            View::Schedule => {
                self.fetch(|c| async move { c.schedule().await.ok().map(Msg::Schedule) });
            }
            View::Autonomy => {
                self.fetch(|c| async move { c.autonomy_report().await.ok().map(Msg::Autonomy) });
                self.fetch(|c| async move {
                    c.egress_pending()
                        .await
                        .ok()
                        .map(|e| Msg::Egress(e.pending))
                });
            }
            View::Ledger => {
                self.fetch(|c| async move { c.ledger_tail(60).await.ok().map(Msg::LedgerTail) });
            }
            View::Agents => {
                self.fetch(|c| async move { c.agents_list().await.ok().map(Msg::Agents) });
            }
            View::Settings => {
                self.fetch(|c| async move { c.config_raw().await.ok().map(Msg::Config) });
            }
            View::Help => {}
        }
    }

    // ---- tick -------------------------------------------------------------

    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        // Expire toast after ~4s.
        if let Some((_, when)) = &self.toast {
            if when.elapsed().as_secs() >= 4 {
                self.toast = None;
            }
        }
        // Periodic refresh: spine every ~3s; the live data view every ~3s.
        if self.tick % 28 == 0 {
            // If the daemon was up and has since gone away (zero-idle exit), bring
            // it back before refreshing.
            let down = !self.health.as_ref().map(|h| h.ok).unwrap_or(false);
            if self.was_up && down && !self.reconnecting {
                self.try_reconnect();
            } else {
                self.refresh_spine();
                if matches!(self.view, View::Tasks | View::Autonomy) || self.chat.streaming {
                    self.load_view(self.view);
                    if self.chat.streaming {
                        self.refresh_tasks();
                    }
                }
            }
        }
    }

    pub fn toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), Instant::now()));
    }

    // ---- message handling -------------------------------------------------

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Spine {
                health,
                meter,
                ledger,
                model,
            } => {
                // Health always reflects the latest probe (None = the daemon is
                // down); this is the only Spine sender, so it never gets clobbered.
                if self.health.as_ref().map(|h| h.ok).unwrap_or(false) {
                    self.was_up = true;
                }
                self.health = health;
                if let Some(m) = meter {
                    self.meter = m;
                }
                if ledger.is_some() {
                    self.ledger = ledger;
                }
                if let Some(m) = model {
                    self.model = m;
                }
            }
            Msg::Tasks(t) => {
                self.tasks = t;
                self.clamp_sel(self.tasks.len());
            }
            Msg::MemoryRecent(m) => {
                self.memory_recent = m;
                if self.view == View::Memory {
                    self.clamp_sel(self.memory_recent.len());
                }
            }
            Msg::MemoryStats(s) => self.memory_stats = s,
            Msg::Consciousness(c) => self.consciousness = Some(c),
            Msg::Skills(s) => {
                self.skills = s;
                if self.view == View::Skills {
                    self.clamp_sel(self.skills.len());
                }
            }
            Msg::Schedule(s) => {
                self.schedule = s;
                if self.view == View::Schedule {
                    self.clamp_sel(self.schedule.len());
                }
            }
            Msg::LedgerTail(l) => self.ledger_tail = l,
            Msg::Autonomy(a) => self.autonomy = Some(a),
            Msg::Egress(e) => {
                self.egress = e;
                if self.view == View::Autonomy {
                    self.clamp_sel(self.egress.len());
                }
            }
            Msg::Agents(a) => {
                self.agents = a;
                if self.view == View::Agents {
                    self.clamp_sel(self.agents.len());
                    // The armed-delete target may have shifted/vanished on refresh.
                    self.confirm_agent = None;
                }
            }
            Msg::Config(c) => self.config_raw = Some(c),
            Msg::Sessions(s) => {
                self.sessions = s;
                // A background refresh can shrink the list while the picker is open.
                if self.sessions_open {
                    self.sessions_sel =
                        self.sessions_sel.min(self.sessions.len().saturating_sub(1));
                }
            }
            Msg::Projects(ps) => {
                self.projects = ps;
                if self.project_picker_open {
                    self.project_sel = self.project_sel.min(self.projects.len().saturating_sub(1));
                }
                // First load: adopt a default project and start the chat scoped to it, so the very
                // first message already runs under a real project (memory + workdir).
                if self.active_project.is_none() {
                    if let Some(first) = self.projects.first() {
                        let id = first.id.clone();
                        self.active_project = Some(id.clone());
                        if self.chat.session.is_none() {
                            self.start_session_in(id);
                        }
                    }
                }
            }
            Msg::ProjectCreated(p) => {
                self.toast(&format!(
                    "· project · {}{}",
                    p.name,
                    p.workdir.as_deref().map(|w| format!(" · {w}")).unwrap_or_default()
                ));
                self.active_project = Some(p.id.clone());
                self.refetch_projects();
                self.start_session_in(p.id);
            }
            Msg::SessionStarted(id) => {
                // The real server-side session id has arrived. If the user already started chatting
                // before it landed (a wide window during cold-start/auto-spawn), DON'T wipe their
                // visible transcript or retarget a streaming turn — that stranded the in-flight turn
                // in an unpersisted phantom session and lost the ability to halt it. Only swap the id
                // for future turns; clear the transcript only when it's genuinely empty and idle.
                if self.chat.streaming || !self.chat.turns.is_empty() {
                    // Keep the id already driving the streaming turn (so Esc-halt still targets it);
                    // otherwise adopt the real id for the next turn.
                    if !self.chat.streaming {
                        self.chat.session = Some(id);
                    }
                } else {
                    self.chat.session = Some(id);
                    self.chat.turns.clear();
                }
                self.chat.stick = true;
            }
            Msg::Reconnected(ok) => {
                self.reconnecting = false;
                if ok {
                    self.toast("· reconnected");
                    self.refresh_spine();
                    self.load_view(self.view);
                }
            }
            Msg::Model(m) => self.model = m,
            Msg::TaskHaltReleased => {
                self.task_halted = false;
            }
            Msg::TaskCreateFailed { title, err } => {
                // Restore the lost composer text and route back to chat.
                self.chat.composer = title;
                self.chat.cursor = self.chat.composer.chars().count();
                self.set_view(View::Chat);
                self.toast(format!("· couldn't create task: {err}"));
            }
            Msg::SessionLoaded {
                id,
                msgs,
                project_id,
            } => {
                self.chat.turns = msgs
                    .into_iter()
                    .map(|m| Turn {
                        role: if m.role == "user" {
                            Role::User
                        } else {
                            Role::Engram
                        },
                        text: m.text,
                        recalled: m.recalled_refs,
                        learned: m.learned,
                        steps: vec![],
                        plan: vec![],
                        error: false,
                    })
                    .collect();
                self.chat.session = Some(id);
                self.chat.scroll = 0;
                self.chat.stick = true;
                // Follow the resumed chat's scope so the footer badge + future turns use the right
                // project (and stay consistent with the session the daemon persists under).
                if let Some(pid) = project_id {
                    self.active_project = Some(pid);
                }
                self.view = View::Chat;
            }
            Msg::Toast(s) => self.toast(s),
            Msg::Chat(ev) => self.on_chat_event(ev),
        }
    }

    fn on_chat_event(&mut self, ev: ChatEvent) {
        match ev {
            ChatEvent::Narration(t) => {
                self.chat.live_narration.push(t);
                self.chat.stick = true;
            }
            ChatEvent::Step {
                tool,
                ok,
                observation,
                args,
                ..
            } => {
                // The plan tool drives the live checklist rather than the step feed.
                if tool == "update_plan" {
                    self.chat.live_plan = parse_plan(&args);
                } else {
                    self.chat.live_steps.push(LiveStep {
                        tool,
                        ok,
                        observation,
                    });
                }
                self.chat.stick = true;
            }
            ChatEvent::Done(done) => {
                // Recover the plan from the run's steps if it never streamed.
                let plan = if self.chat.live_plan.is_empty() {
                    done.steps
                        .iter()
                        .rev()
                        .find(|s| s.tool == "update_plan")
                        .map(|s| parse_plan(&s.args))
                        .unwrap_or_default()
                } else {
                    std::mem::take(&mut self.chat.live_plan)
                };
                let steps = done.steps.clone();
                self.chat.turns.push(Turn {
                    role: Role::Engram,
                    text: done.reply.clone(),
                    recalled: done.recalled_refs.clone(),
                    learned: done.learned.clone(),
                    steps,
                    plan,
                    error: false,
                });
                self.chat.streaming = false;
                self.chat.live_steps.clear();
                self.chat.live_narration.clear();
                self.chat.live_plan.clear();
                self.chat.stick = true;
            }
            ChatEvent::Disconnected(ref e) => {
                // The stream dropped — likely the idle daemon exited. Try to bring
                // it back so the next message works.
                let msg = e.clone();
                self.chat.turns.push(Turn {
                    role: Role::Engram,
                    text: format!("⚠ {msg} — reconnecting…"),
                    recalled: vec![],
                    learned: vec![],
                    steps: vec![],
                    plan: vec![],
                    error: true,
                });
                self.chat.streaming = false;
                self.chat.live_steps.clear();
                self.chat.live_narration.clear();
                self.chat.live_plan.clear();
                self.chat.stick = true;
                if self.was_up {
                    self.try_reconnect();
                }
            }
            ChatEvent::Error(e) => {
                self.chat.turns.push(Turn {
                    role: Role::Engram,
                    text: format!("⚠ {e}"),
                    recalled: vec![],
                    learned: vec![],
                    steps: vec![],
                    plan: vec![],
                    error: true,
                });
                self.chat.streaming = false;
                self.chat.live_steps.clear();
                self.chat.live_narration.clear();
                self.chat.live_plan.clear();
                self.chat.stick = true;
            }
        }
    }

    // ---- input ------------------------------------------------------------

    pub fn on_mouse(&mut self, me: MouseEvent) {
        if !self.mouse {
            return;
        }
        // Don't let clicks/scroll leak through an open overlay.
        if self.form.is_some()
            || self.prompt_modal.is_some()
            || self.palette.is_some()
            || self.sessions_open
            || self.project_picker_open
        {
            return;
        }
        match me.kind {
            MouseEventKind::ScrollUp => self.scroll_action(-3),
            MouseEventKind::ScrollDown => self.scroll_action(3),
            MouseEventKind::Down(MouseButton::Left) => {
                // Click a header tab.
                if me.row == 0 {
                    let hit = self
                        .tab_hits
                        .iter()
                        .find(|(_, x0, x1)| me.column >= *x0 && me.column < *x1)
                        .map(|(v, _, _)| *v);
                    if let Some(v) = hit {
                        self.set_view(v);
                    }
                }
            }
            _ => {}
        }
    }

    fn scroll_action(&mut self, delta: i32) {
        if self.detail_open {
            self.detail_scroll = if delta < 0 {
                self.detail_scroll.saturating_sub((-delta) as u16)
            } else {
                self.detail_scroll.saturating_add(delta as u16)
            };
        } else if self.view == View::Chat {
            self.scroll_chat(delta);
        } else {
            let len = self.list_len;
            self.move_sel(if delta < 0 { -1 } else { 1 }, len);
        }
    }

    pub fn on_paste(&mut self, s: &str) {
        if self.view == View::Chat && self.palette.is_none() {
            // Preserve newlines as a separator instead of stripping them — dropping them silently
            // concatenated pasted lines ("line1\nline2" → "line1line2"), corrupting pasted code /
            // logs / lists. The single-line composer renders one line, so collapse any CRLF/CR/LF
            // run to a single space (a full multiline composer with Shift-Enter is a separate,
            // render-side change). A trailing newline (common when copying a whole line) is dropped.
            let mut prev_was_newline = false;
            for ch in s.chars() {
                if ch == '\n' || ch == '\r' {
                    if !prev_was_newline {
                        self.insert_char(' ');
                        prev_was_newline = true;
                    }
                } else {
                    self.insert_char(ch);
                    prev_was_newline = false;
                }
            }
            // Trim a lone trailing separator introduced by a final newline.
            if self.chat.composer.ends_with(' ') && s.ends_with(['\n', '\r']) {
                self.backspace();
            }
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        // The quit contract is universal — it must win over any open overlay.
        // Ctrl-C first closes an open overlay (a gentle bail-out); a second
        // Ctrl-C with nothing open quits. Ctrl-Q always quits outright.
        if ctrl && matches!(k.code, KeyCode::Char('q')) {
            self.should_quit = true;
            return;
        }
        if ctrl && matches!(k.code, KeyCode::Char('c')) {
            if self.form.take().is_some() {
                return;
            }
            if self.prompt_modal.take().is_some() {
                return;
            }
            if self.sessions_open {
                self.sessions_open = false;
                return;
            }
            if self.project_picker_open {
                self.project_picker_open = false;
                return;
            }
            if self.palette.take().is_some() {
                return;
            }
            self.should_quit = true;
            return;
        }
        // Other modal overlays intercept everything else while open.
        if self.form.is_some() {
            self.form_key(k);
            return;
        }
        if self.prompt_modal.is_some() {
            self.prompt_key(k);
            return;
        }
        if self.sessions_open {
            self.sessions_key(k);
            return;
        }
        if self.project_picker_open {
            self.project_picker_key(k);
            return;
        }
        if self.palette.is_some() {
            self.palette_key(k);
            return;
        }
        match (ctrl, k.code) {
            (true, KeyCode::Char('p')) | (true, KeyCode::Char('k')) => {
                self.open_palette();
                return;
            }
            (true, KeyCode::Char('r')) => {
                self.open_sessions();
                return;
            }
            (true, KeyCode::Char('o')) => {
                self.open_project_picker();
                return;
            }
            (true, KeyCode::Char('t')) => {
                self.new_task_from_composer();
                return;
            }
            (true, KeyCode::Char('a')) => {
                self.add_attachment_prompt();
                return;
            }
            (true, KeyCode::Char('y')) => {
                self.yank_last_answer();
                return;
            }
            _ => {}
        }
        if k.code == KeyCode::Esc {
            if self.detail_open {
                self.detail_open = false;
                return;
            }
            if self.view == View::Help {
                let back = if self.prev_view == View::Help {
                    View::Chat
                } else {
                    self.prev_view
                };
                self.set_view(back);
                return;
            }
            if self.chat.streaming {
                self.halt();
                return;
            }
        }
        if k.code == KeyCode::F(1) {
            self.set_view(View::Help);
            return;
        }
        // Alt+1..8 jump to a tab.
        if k.modifiers.contains(KeyModifiers::ALT) {
            if let KeyCode::Char(c) = k.code {
                if let Some(d) = c.to_digit(10) {
                    if (1..=View::TABS.len() as u32).contains(&d) {
                        self.set_view(View::TABS[(d - 1) as usize]);
                        return;
                    }
                }
            }
        }

        if self.view == View::Chat {
            self.composer_key(k);
        } else {
            crate::tui::views::handle_key(self, k);
        }
    }

    fn composer_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Enter => self.submit(),
            KeyCode::Char('/') if self.chat.composer.is_empty() => self.open_palette(),
            KeyCode::Char(c) if !ctrl => self.insert_char(c),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Left => self.cursor_left(),
            KeyCode::Right => self.cursor_right(),
            KeyCode::Home => self.chat.cursor = 0,
            KeyCode::End => self.chat.cursor = self.chat.composer.chars().count(),
            KeyCode::Up => self.scroll_chat(-1),
            KeyCode::Down => self.scroll_chat(1),
            KeyCode::PageUp => self.scroll_chat(-10),
            KeyCode::PageDown => self.scroll_chat(10),
            KeyCode::Char('u') if ctrl => {
                self.chat.composer.clear();
                self.chat.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => self.delete_word(),
            _ => {}
        }
    }

    fn insert_char(&mut self, c: char) {
        let byte = self.byte_at(self.chat.cursor);
        self.chat.composer.insert(byte, c);
        self.chat.cursor += 1;
    }
    fn backspace(&mut self) {
        if self.chat.cursor == 0 {
            return;
        }
        let prev = self.chat.cursor - 1;
        let start = self.byte_at(prev);
        let end = self.byte_at(self.chat.cursor);
        self.chat.composer.replace_range(start..end, "");
        self.chat.cursor = prev;
    }
    fn delete_word(&mut self) {
        let chars: Vec<char> = self.chat.composer.chars().collect();
        let cursor = self.chat.cursor.min(chars.len());
        let mut i = cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let start = self.byte_at(i);
        let end = self.byte_at(self.chat.cursor);
        self.chat.composer.replace_range(start..end, "");
        self.chat.cursor = i;
    }
    fn cursor_left(&mut self) {
        self.chat.cursor = self.chat.cursor.saturating_sub(1);
    }
    fn cursor_right(&mut self) {
        let n = self.chat.composer.chars().count();
        if self.chat.cursor < n {
            self.chat.cursor += 1;
        }
    }
    fn byte_at(&self, char_idx: usize) -> usize {
        self.chat
            .composer
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.chat.composer.len())
    }

    fn scroll_chat(&mut self, delta: i32) {
        let new = (self.chat.scroll as i32 + delta).max(0) as u16;
        self.chat.scroll = new;
        // If the user scrolls to the bottom, re-stick.
        let max = self.chat.last_total.saturating_sub(self.chat.last_viewport);
        self.chat.stick = self.chat.scroll >= max;
    }

    fn session_id(&mut self) -> String {
        if self.chat.session.is_none() {
            self.chat.session = Some(format!("s-cli-{}", now_ms()));
        }
        self.chat.session.clone().unwrap()
    }

    fn submit(&mut self) {
        let text = self.chat.composer.trim().to_string();
        if text.is_empty() || self.chat.streaming {
            return;
        }
        self.chat.composer.clear();
        self.chat.cursor = 0;
        self.chat.turns.push(Turn {
            role: Role::User,
            text: text.clone(),
            recalled: vec![],
            learned: vec![],
            steps: vec![],
            plan: vec![],
            error: false,
        });
        self.chat.streaming = true;
        self.chat.live_steps.clear();
        self.chat.live_narration.clear();
        self.chat.live_plan.clear();
        self.chat.stick = true;
        // Pinned files travel with this turn as untrusted reference material.
        let attachments: Vec<Value> = self
            .chat
            .pending_attachments
            .drain(..)
            .map(|a| json!({ "kind": a.kind, "name": a.name, "text": a.text }))
            .collect();
        let session = self.session_id();
        let mut rx = self
            .client
            .converse_stream(text, Some(session), attachments);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                if tx.send(Msg::Chat(ev)).is_err() {
                    break;
                }
            }
        });
    }

    /// Turn the current composer text into a task and run it (Ctrl-T) — the
    /// keyboard equivalent of the desktop's ⌘+Enter "create & run".
    fn new_task_from_composer(&mut self) {
        let title = self.chat.composer.trim().to_string();
        if title.is_empty() {
            return;
        }
        self.chat.composer.clear();
        self.chat.cursor = 0;
        self.set_view(View::Tasks);
        self.toast("· creating task…");
        self.fetch(move |c| async move {
            match c.task_create(&title, None, Some("manual")).await {
                Ok(task) => {
                    // Run it; the daemon streams progress server-side and the Tasks
                    // view's periodic refresh reflects it.
                    let id = task.id.clone();
                    let cc = c.clone();
                    tokio::spawn(async move {
                        let _ = cc.task_run(&id).await;
                    });
                    // Surface the new card immediately.
                    c.tasks().await.ok().map(Msg::Tasks)
                }
                // Don't lose the user's text on failure — hand it back.
                Err(e) => Some(Msg::TaskCreateFailed {
                    title,
                    err: e.to_string(),
                }),
            }
        });
    }

    fn halt(&mut self) {
        let session = self.chat.session.clone();
        self.fetch(move |c| async move {
            let _ = c.halt(session.as_deref(), true).await;
            Some(Msg::Toast("· stopped".into()))
        });
    }

    // ---- palette ----------------------------------------------------------

    pub fn open_palette(&mut self) {
        self.palette = Some(Palette {
            query: String::new(),
            sel: 0,
        });
    }

    // ---- modal text prompt ------------------------------------------------

    pub fn open_model_prompt(&mut self) {
        self.prompt_modal = Some(PromptModal {
            title: "Switch model — provider/model id".into(),
            buffer: self.model.clone(),
            cursor: self.model.chars().count(),
            kind: PromptKind::SetModel,
        });
    }

    pub fn open_prompt(&mut self, title: impl Into<String>, prefill: String, kind: PromptKind) {
        let cursor = prefill.chars().count();
        self.prompt_modal = Some(PromptModal {
            title: title.into(),
            buffer: prefill,
            cursor,
            kind,
        });
    }

    fn prompt_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => {
                self.prompt_modal = None;
                return;
            }
            KeyCode::Enter => {
                self.apply_prompt();
                return;
            }
            _ => {}
        }
        let Some(p) = self.prompt_modal.as_mut() else {
            return;
        };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('u') if ctrl => {
                p.buffer.clear();
                p.cursor = 0;
            }
            KeyCode::Char(c) if !ctrl => {
                let mut chars: Vec<char> = p.buffer.chars().collect();
                let at = p.cursor.min(chars.len());
                chars.insert(at, c);
                p.buffer = chars.into_iter().collect();
                p.cursor = at + 1;
            }
            KeyCode::Backspace => {
                if p.cursor > 0 {
                    let mut chars: Vec<char> = p.buffer.chars().collect();
                    chars.remove(p.cursor - 1);
                    p.buffer = chars.into_iter().collect();
                    p.cursor -= 1;
                }
            }
            KeyCode::Left => p.cursor = p.cursor.saturating_sub(1),
            KeyCode::Right => {
                let n = p.buffer.chars().count();
                if p.cursor < n {
                    p.cursor += 1;
                }
            }
            KeyCode::Home => p.cursor = 0,
            KeyCode::End => p.cursor = p.buffer.chars().count(),
            _ => {}
        }
    }

    fn apply_prompt(&mut self) {
        let Some(p) = self.prompt_modal.take() else {
            return;
        };
        match p.kind {
            PromptKind::SetModel => {
                let model = p.buffer.trim().to_string();
                if model.is_empty() {
                    return;
                }
                self.model = model.clone(); // optimistic; the spine refresh confirms it
                self.fetch(move |c| async move {
                    let _ = c
                        .config_set(json!({ "provider": { "model": model } }))
                        .await;
                    c.config()
                        .await
                        .ok()
                        .map(|cfg| Msg::Model(cfg.model_in_use))
                });
                self.toast("· updating model…");
            }
            PromptKind::SetConfig {
                section,
                field,
                secret,
                number,
            } => {
                let raw = p.buffer.trim().to_string();
                // For a secret, an empty buffer means "keep the current value".
                if secret && raw.is_empty() {
                    self.toast("· unchanged");
                    return;
                }
                let value = if number {
                    match raw.parse::<u64>() {
                        Ok(n) => json!(n),
                        Err(_) => {
                            self.toast("· not a number");
                            return;
                        }
                    }
                } else {
                    json!(raw)
                };
                self.config_set_field(&section, &field, value);
                self.toast("· saving…");
            }
            PromptKind::AddAttachment => {
                let path = p.buffer.trim().to_string();
                if path.is_empty() {
                    return;
                }
                // Cap the read so a huge file can't be slurped wholesale into the
                // turn (the daemon also bounds attachment text it forwards).
                const MAX: usize = 256 * 1024;
                match std::fs::read_to_string(&path) {
                    Ok(mut text) => {
                        let name = std::path::Path::new(&path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(&path)
                            .to_string();
                        let truncated = text.len() > MAX;
                        if truncated {
                            crate::ui::format::truncate_bytes(&mut text, MAX);
                        }
                        let kb = text.len() / 1024;
                        self.chat.pending_attachments.push(Attachment {
                            kind: "file".into(),
                            name: name.clone(),
                            text,
                        });
                        self.toast(if truncated {
                            format!("· attached {name} (truncated to {kb}KB)")
                        } else {
                            format!("· attached {name} ({kb}KB)")
                        });
                    }
                    Err(e) => self.toast(format!("· can't read file: {e}")),
                }
            }
        }
    }

    /// Apply a config patch and refresh the cached config + spine. On success the
    /// updated value is what confirms it landed; on failure we surface the error
    /// (so a save is never silently a no-op).
    pub fn config_set_patch(&self, patch: Value) {
        self.fetch(move |c| async move {
            match c.config_set(patch).await {
                Ok(_) => c.config_raw().await.ok().map(Msg::Config),
                Err(e) => Some(Msg::Toast(format!("· save failed: {e}"))),
            }
        });
        // The model display lives on the spine — refresh it too.
        self.refresh_spine();
    }

    /// Patch a single config field (`{section:{field:value}}`).
    pub fn config_set_field(&self, section: &str, field: &str, value: Value) {
        self.config_set_patch(json!({ section: { field: value } }));
    }

    pub fn toggle_config(&self, section: &str, field: &str, current: bool) {
        self.config_set_field(section, field, json!(!current));
    }

    /// Clear a secret config field via its `clear_<field>` flag.
    pub fn clear_config_secret(&self, section: &str, field: &str) {
        let mut inner = serde_json::Map::new();
        inner.insert(format!("clear_{field}"), json!(true));
        let mut outer = serde_json::Map::new();
        outer.insert(section.to_string(), Value::Object(inner));
        self.config_set_patch(Value::Object(outer));
    }

    /// Open the edit modal for a config field.
    pub fn edit_config(
        &mut self,
        section: &str,
        field: &str,
        secret: bool,
        number: bool,
        current: &str,
    ) {
        let title = if secret {
            format!("{section}.{field} — new value (blank keeps current)")
        } else {
            format!("{section}.{field}")
        };
        let prefill = if secret {
            String::new()
        } else {
            current.to_string()
        };
        self.open_prompt(
            title,
            prefill,
            PromptKind::SetConfig {
                section: section.to_string(),
                field: field.to_string(),
                secret,
                number,
            },
        );
    }

    /// Test the live provider with a tiny completion and toast the result.
    pub fn test_provider(&mut self) {
        self.fetch(|c| async move {
            let r = c.config_test(json!({})).await.ok();
            Some(Msg::Toast(match r {
                Some(v) if v.get("ok").and_then(|x| x.as_bool()) == Some(true) => {
                    let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("?");
                    let reply: String = v
                        .get("reply")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .chars()
                        .take(40)
                        .collect();
                    format!("✓ provider ok — {model} replied “{}”", reply.trim())
                }
                Some(v) => format!(
                    "✗ provider error: {}",
                    v.get("error").and_then(|m| m.as_str()).unwrap_or("unknown")
                ),
                None => "✗ provider test failed".into(),
            }))
        });
        self.toast("· testing provider…");
    }

    // ---- agents -----------------------------------------------------------

    pub fn create_project_prompt(&mut self) {
        self.form = Some(FormModal {
            title: "New project".into(),
            fields: vec![
                FormField::new("Name", "", "required"),
                FormField::new(
                    "Directory",
                    "",
                    "optional — the folder its agent works in (created if missing)",
                ),
            ],
            sel: 0,
            cursor: 0,
            kind: FormKind::NewProject,
        });
    }

    pub fn create_agent_prompt(&mut self) {
        self.form = Some(FormModal {
            title: "New agent".into(),
            fields: vec![
                FormField::new("Name", "", "required"),
                FormField::new("Role", "", "system prompt / specialty"),
                FormField::new("Model", "", "blank = global default"),
                FormField::new("Provider", "", "blank = global"),
                FormField::new("Emoji", "", "e.g. 🔎"),
            ],
            sel: 0,
            cursor: 0,
            kind: FormKind::CreateAgent,
        });
    }

    pub fn edit_selected_agent(&mut self) {
        let Some(a) = self.agents.get(self.sel) else {
            return;
        };
        let s = |k: &str| a.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let Some(id) = a.get("id").and_then(|v| v.as_str()).map(str::to_string) else {
            return;
        };
        self.form = Some(FormModal {
            title: format!("Edit agent · {}", s("name")),
            fields: vec![
                FormField::new("Name", s("name"), "required"),
                FormField::new("Role", s("role"), "system prompt / specialty"),
                FormField::new("Model", s("model"), "blank = global default"),
                FormField::new("Provider", s("provider"), "blank = global"),
                FormField::new("Emoji", s("emoji"), ""),
            ],
            sel: 0,
            cursor: 0,
            kind: FormKind::EditAgent { id },
        });
    }

    pub fn policy_selected_agent(&mut self) {
        let Some(a) = self.agents.get(self.sel) else {
            return;
        };
        let name = a
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let Some(id) = a.get("id").and_then(|v| v.as_str()).map(str::to_string) else {
            return;
        };
        self.form = Some(FormModal {
            title: format!("Autonomy policy · {name}"),
            fields: vec![
                FormField::new("Allowed egress", "", "comma-sep destinations (host/url)"),
                FormField::new("Allowed actions", "send", "comma-sep: send,post,pay,…"),
                FormField::num("Max actions", "", "0 = none"),
                FormField::num("Max spend (cents)", "", "optional"),
                FormField::num("Expires (days)", "", "optional"),
            ],
            sel: 0,
            cursor: 0,
            kind: FormKind::SetPolicy { id },
        });
    }

    /// Delete the selected agent, requiring a confirming second `d`.
    pub fn delete_selected_agent(&mut self) {
        let Some(id) = self
            .agents
            .get(self.sel)
            .and_then(|a| a.get("id").and_then(|v| v.as_str()))
            .map(str::to_string)
        else {
            return;
        };
        if self.confirm_agent.as_deref() == Some(id.as_str()) {
            self.confirm_agent = None;
            self.fetch(move |c| async move {
                let _ = c.agents_delete(&id).await;
                c.agents_list().await.ok().map(Msg::Agents)
            });
            self.toast("· agent deleted");
        } else {
            self.confirm_agent = Some(id);
            self.toast("press d again to delete this agent");
        }
    }

    // ---- schedule ---------------------------------------------------------

    pub fn add_schedule_form(&mut self) {
        self.form = Some(FormModal {
            title: "New scheduled job".into(),
            fields: vec![
                FormField::new("Name", "", "required"),
                FormField::new("When", "", "e.g. every weekday at 9am"),
                FormField::new("Task title", "", "what to run on each fire"),
            ],
            sel: 0,
            cursor: 0,
            kind: FormKind::AddSchedule,
        });
    }

    pub fn delete_selected_schedule(&mut self) {
        if let Some(j) = self.schedule.get(self.sel) {
            let id = j.id.clone();
            self.fetch(move |c| async move {
                let _ = c.schedule_remove(&id).await;
                c.schedule().await.ok().map(Msg::Schedule)
            });
            self.toast("· job deleted");
        }
    }

    // ---- MCP servers (in Settings) ---------------------------------------

    fn mcp_array(&self) -> Vec<Value> {
        self.config_raw
            .as_ref()
            .and_then(|c| c.get("mcp"))
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default()
    }

    pub fn open_mcp_form(&mut self, index: Option<usize>) {
        let (name, command, args, env) = index
            .and_then(|i| self.mcp_array().get(i).cloned())
            .map(|s| {
                let g = |k: &str| s.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                let args = s
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        shell_join(
                            &a.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect::<Vec<_>>(),
                        )
                    })
                    .unwrap_or_default();
                // Env values come back masked; show keys so the user can keep them.
                let env = s
                    .get("env")
                    .and_then(|v| v.as_object())
                    .map(|o| {
                        o.keys()
                            .map(|k| format!("{k}=•••"))
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                (g("name"), g("command"), args, env)
            })
            .unwrap_or_default();
        self.form = Some(FormModal {
            title: if index.is_some() {
                "Edit MCP server".into()
            } else {
                "New MCP server".into()
            },
            fields: vec![
                FormField::new("Name", name, "unique id"),
                FormField::new("Command", command, "e.g. npx, uvx, /path/to/bin"),
                FormField::new("Args", args, "space-separated"),
                FormField::new("Env", env, "K=V,K2=V2  (••• keeps current)"),
            ],
            sel: 0,
            cursor: 0,
            kind: FormKind::Mcp { index },
        });
    }

    pub fn delete_mcp(&mut self, index: usize) {
        let mut arr = self.mcp_array();
        if index < arr.len() {
            arr.remove(index);
            self.config_set_patch(json!({ "mcp": arr }));
            self.toast("· MCP server removed");
        }
    }

    // ---- form modal -------------------------------------------------------

    fn form_field(&mut self) -> Option<&mut FormField> {
        let f = self.form.as_mut()?;
        let sel = f.sel.min(f.fields.len().saturating_sub(1));
        f.fields.get_mut(sel)
    }

    fn form_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => {
                self.form = None;
                return;
            }
            KeyCode::Enter => {
                self.apply_form();
                return;
            }
            KeyCode::Tab | KeyCode::Down => {
                if let Some(f) = self.form.as_mut() {
                    f.sel = (f.sel + 1) % f.fields.len().max(1);
                    f.cursor = f
                        .fields
                        .get(f.sel)
                        .map(|x| x.value.chars().count())
                        .unwrap_or(0);
                }
                return;
            }
            KeyCode::BackTab | KeyCode::Up => {
                if let Some(f) = self.form.as_mut() {
                    let n = f.fields.len().max(1);
                    f.sel = (f.sel + n - 1) % n;
                    f.cursor = f
                        .fields
                        .get(f.sel)
                        .map(|x| x.value.chars().count())
                        .unwrap_or(0);
                }
                return;
            }
            _ => {}
        }
        let cursor = self.form.as_ref().map(|f| f.cursor).unwrap_or(0);
        match k.code {
            KeyCode::Char('u') if ctrl => {
                if let Some(field) = self.form_field() {
                    field.value.clear();
                }
                if let Some(f) = self.form.as_mut() {
                    f.cursor = 0;
                }
            }
            KeyCode::Char(c) if !ctrl => {
                // Numeric fields accept digits only.
                let is_num = self
                    .form
                    .as_ref()
                    .and_then(|f| f.fields.get(f.sel))
                    .map(|x| x.number)
                    .unwrap_or(false);
                if is_num && !c.is_ascii_digit() {
                    return;
                }
                if let Some(field) = self.form_field() {
                    let mut chars: Vec<char> = field.value.chars().collect();
                    let at = cursor.min(chars.len());
                    chars.insert(at, c);
                    field.value = chars.into_iter().collect();
                }
                if let Some(f) = self.form.as_mut() {
                    f.cursor = cursor + 1;
                }
            }
            KeyCode::Backspace => {
                if cursor > 0 {
                    if let Some(field) = self.form_field() {
                        let mut chars: Vec<char> = field.value.chars().collect();
                        if cursor <= chars.len() {
                            chars.remove(cursor - 1);
                            field.value = chars.into_iter().collect();
                        }
                    }
                    if let Some(f) = self.form.as_mut() {
                        f.cursor = cursor - 1;
                    }
                }
            }
            KeyCode::Left => {
                if let Some(f) = self.form.as_mut() {
                    f.cursor = f.cursor.saturating_sub(1);
                }
            }
            KeyCode::Right => {
                if let Some(f) = self.form.as_mut() {
                    let n = f
                        .fields
                        .get(f.sel)
                        .map(|x| x.value.chars().count())
                        .unwrap_or(0);
                    if f.cursor < n {
                        f.cursor += 1;
                    }
                }
            }
            KeyCode::Home => {
                if let Some(f) = self.form.as_mut() {
                    f.cursor = 0;
                }
            }
            KeyCode::End => {
                if let Some(f) = self.form.as_mut() {
                    f.cursor = f
                        .fields
                        .get(f.sel)
                        .map(|x| x.value.chars().count())
                        .unwrap_or(0);
                }
            }
            _ => {}
        }
    }

    fn apply_form(&mut self) {
        let Some(form) = self.form.take() else {
            return;
        };
        let get = |i: usize| {
            form.fields
                .get(i)
                .map(|f| f.value.trim().to_string())
                .unwrap_or_default()
        };
        let csv = |s: &str| -> Vec<String> {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        };
        match form.kind {
            FormKind::NewProject => {
                let name = get(0);
                if name.is_empty() {
                    self.toast("· a project needs a name");
                    return;
                }
                let dir = get(1);
                let dir = if dir.is_empty() { None } else { Some(dir) };
                self.fetch(move |c| async move {
                    match c.project_create(&name, dir.as_deref()).await {
                        // Switch to the new project + start a session in it (see Msg::ProjectCreated).
                        Ok(p) => Some(Msg::ProjectCreated(p)),
                        Err(e) => Some(Msg::Toast(format!("· project create failed: {e}"))),
                    }
                });
                self.toast("· creating project…");
            }
            FormKind::CreateAgent | FormKind::EditAgent { .. } => {
                let name = get(0);
                if name.is_empty() {
                    self.toast("· an agent needs a name");
                    return;
                }
                let edit = matches!(form.kind, FormKind::EditAgent { .. });
                let id = if let FormKind::EditAgent { id } = &form.kind {
                    Some(id.clone())
                } else {
                    None
                };
                // On edit, omit blank fields so the daemon leaves them unchanged
                // (a present empty string would clobber the stored value). On
                // create, send blanks (the daemon treats them as global defaults).
                let mut m = serde_json::Map::new();
                m.insert("name".into(), json!(name));
                for (key, idx) in [("role", 1), ("model", 2), ("provider", 3), ("emoji", 4)] {
                    let v = get(idx);
                    if !edit || !v.is_empty() {
                        m.insert(key.into(), json!(v));
                    }
                }
                let body = Value::Object(m);
                self.fetch(move |c| async move {
                    let r = if let Some(id) = id {
                        c.agents_update(&id, body).await
                    } else {
                        c.agents_create(body).await
                    };
                    match r {
                        Ok(_) => c.agents_list().await.ok().map(Msg::Agents),
                        Err(e) => Some(Msg::Toast(format!("· agent save failed: {e}"))),
                    }
                });
                self.toast(if edit {
                    "· updating agent…"
                } else {
                    "· creating agent…"
                });
            }
            FormKind::SetPolicy { id } => {
                let egress = csv(&get(0));
                let actions = csv(&get(1));
                let max_actions: u64 = get(2).parse().unwrap_or(0);
                let max_spend = get(3).parse::<u64>().ok();
                let expires = get(4).parse::<u64>().ok();
                // The daemon revokes unless there's an allowlist or a positive
                // action cap — don't let the user think they set a live policy.
                if egress.is_empty() && max_actions == 0 {
                    self.toast("· need allowed egress or max-actions to enable a policy");
                    return;
                }
                let mut body = json!({
                    "enabled": true,
                    "allowed_egress": egress,
                    "allowed_actions": actions,
                    "max_actions": max_actions,
                });
                if let Some(m) = max_spend {
                    body["max_spend_cents"] = json!(m);
                }
                if let Some(e) = expires {
                    body["expires_days"] = json!(e);
                }
                self.fetch(move |c| async move {
                    match c.agent_set_policy(&id, body).await {
                        Ok(_) => c.agents_list().await.ok().map(Msg::Agents),
                        Err(e) => Some(Msg::Toast(format!("· policy failed: {e}"))),
                    }
                });
                self.toast("· setting policy…");
            }
            FormKind::AddSchedule => {
                let name = get(0);
                let when = get(1);
                let title = get(2);
                if name.is_empty() || when.is_empty() {
                    self.toast("· name and when are required");
                    return;
                }
                let payload = if title.is_empty() {
                    json!({})
                } else {
                    json!({ "title": title })
                };
                self.fetch(move |c| async move {
                    match c.schedule_add(&name, &when, payload).await {
                        Ok(_) => c.schedule().await.ok().map(Msg::Schedule),
                        Err(e) => Some(Msg::Toast(format!("· couldn't schedule: {e}"))),
                    }
                });
                self.toast("· scheduling…");
            }
            FormKind::Mcp { index } => {
                let name = get(0);
                let command = get(1);
                if name.is_empty() || command.is_empty() {
                    self.toast("· name and command are required");
                    return;
                }
                let args: Vec<String> = shell_split(&get(2));
                let mut env = serde_json::Map::new();
                for pair in get(3).split(',') {
                    if let Some((k, v)) = pair.split_once('=') {
                        let k = k.trim();
                        if !k.is_empty() {
                            env.insert(k.to_string(), json!(v.trim()));
                        }
                    }
                }
                let mut arr = self.mcp_array();
                // Names key the daemon's env-mask restoration, so they must be unique.
                if arr.iter().enumerate().any(|(i, s)| {
                    Some(i) != index
                        && s.get("name").and_then(|v| v.as_str()) == Some(name.as_str())
                }) {
                    self.toast("· an MCP server with that name already exists");
                    return;
                }
                // Start from the existing server so its `cwd`/`trusted` (and any other
                // fields the TUI doesn't edit) survive — the daemon would otherwise
                // default `trusted` back to false, re-sensitising the server.
                let base = index.and_then(|i| arr.get(i).cloned()).unwrap_or(json!({}));
                let mut obj = base.as_object().cloned().unwrap_or_default();
                obj.insert("name".into(), json!(name));
                obj.insert("command".into(), json!(command));
                obj.insert("args".into(), json!(args));
                obj.insert("env".into(), Value::Object(env));
                let server = Value::Object(obj);
                match index {
                    Some(i) if i < arr.len() => arr[i] = server,
                    _ => arr.push(server),
                }
                self.config_set_patch(json!({ "mcp": arr }));
                self.toast("· saving MCP server…");
            }
        }
    }

    // ---- daemon reconnect -------------------------------------------------

    /// If the (zero-idle) daemon has gone away, try to bring it back. Rate-limited
    /// by the `reconnecting` flag so we never spawn-storm.
    pub fn try_reconnect(&mut self) {
        if self.reconnecting {
            return;
        }
        self.reconnecting = true;
        self.toast("· reconnecting…");
        self.fetch(|c| async move {
            let ok = crate::cli::daemon::ensure(&c, true, true).await.is_ok();
            Some(Msg::Reconnected(ok))
        });
    }

    // ---- clipboard --------------------------------------------------------

    /// Copy the last assistant answer to the system clipboard.
    pub fn yank_last_answer(&mut self) {
        let Some(turn) = self
            .chat
            .turns
            .iter()
            .rev()
            .find(|t| matches!(t.role, Role::Engram) && !t.error)
        else {
            self.toast("· nothing to copy");
            return;
        };
        match copy_to_clipboard(&turn.text) {
            Ok(()) => self.toast("· copied the last answer"),
            Err(e) => self.toast(format!("· copy failed: {e}")),
        }
    }

    // ---- attachments ------------------------------------------------------

    pub fn add_attachment_prompt(&mut self) {
        self.open_prompt(
            "Attach a file — path",
            String::new(),
            PromptKind::AddAttachment,
        );
    }

    // ---- session resume picker -------------------------------------------

    pub fn open_sessions(&mut self) {
        self.sessions_open = true;
        self.sessions_sel = 0;
        // Refresh the list so it reflects any sessions created since launch. Scope to the active
        // project — the daemon defaults an absent filter to "personal", which would hide every
        // project-scoped chat from the resume picker.
        let project = self.active_project.clone();
        self.fetch(move |c| async move {
            c.sessions(project.as_deref())
                .await
                .ok()
                .map(Msg::Sessions)
        });
    }

    fn sessions_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => self.sessions_open = false,
            KeyCode::Up | KeyCode::Char('k') => {
                self.sessions_sel = self.sessions_sel.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.sessions_sel + 1 < self.sessions.len() {
                    self.sessions_sel += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(s) = self.sessions.get(self.sessions_sel) {
                    let id = s.id.clone();
                    // Carry the picker row's project so the resumed chat adopts the right scope
                    // (SessionDetail itself doesn't echo the project_id back).
                    let project_id = if s.project_id.is_empty() {
                        None
                    } else {
                        Some(s.project_id.clone())
                    };
                    self.sessions_open = false;
                    self.fetch(move |c| async move {
                        c.session_detail(&id)
                            .await
                            .ok()
                            .map(|d| Msg::SessionLoaded {
                                id: d.id,
                                msgs: d.messages,
                                project_id,
                            })
                    });
                    self.toast("· resuming session");
                }
            }
            _ => {}
        }
    }

    pub fn palette_items(&self) -> Vec<usize> {
        let q = self
            .palette
            .as_ref()
            .map(|p| p.query.to_lowercase())
            .unwrap_or_default();
        PALETTE
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                q.is_empty()
                    || it.label.to_lowercase().contains(&q)
                    || it.hint.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn palette_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let items = self.palette_items();
        let Some(p) = self.palette.as_mut() else {
            return;
        };
        match k.code {
            KeyCode::Esc => self.palette = None,
            KeyCode::Up => p.sel = p.sel.saturating_sub(1),
            KeyCode::Down => {
                if p.sel + 1 < items.len() {
                    p.sel += 1;
                }
            }
            KeyCode::Backspace => {
                p.query.pop();
                p.sel = 0;
            }
            KeyCode::Enter | KeyCode::Tab => {
                if let Some(&idx) = items.get(p.sel) {
                    let action = PALETTE[idx].action.clone();
                    self.palette = None;
                    self.run_action(action);
                }
            }
            // Only plain characters filter; ctrl-combos must not pollute the query.
            KeyCode::Char(c) if !ctrl => {
                p.query.push(c);
                p.sel = 0;
            }
            _ => {}
        }
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::Go(v) => self.set_view(v),
            Action::ClearChat => {
                self.chat.turns.clear();
                self.chat.scroll = 0;
                self.chat.stick = true;
            }
            Action::NewSession => {
                // Start a fresh session UNDER the active project, so the new chat's memory and
                // working directory are scoped to it (falls back to a client-only chat if no
                // project has loaded yet).
                self.toast("· new session");
                match self.active_project.clone() {
                    Some(pid) => self.start_session_in(pid),
                    None => {
                        self.chat.turns.clear();
                        self.chat.session = None;
                        self.chat.scroll = 0;
                        self.chat.stick = true;
                        self.view = View::Chat;
                    }
                }
            }
            Action::NewProject => self.create_project_prompt(),
            Action::SwitchProject => self.open_project_picker(),
            Action::ResumeSession => self.open_sessions(),
            Action::SwitchModel => self.open_model_prompt(),
            Action::Verify => {
                self.refresh_spine();
                self.fetch(|c| async move {
                    let v = c.ledger_verify().await.ok();
                    Some(Msg::Toast(match v {
                        Some(v) if v.ok => format!("✓ ledger intact · {} entries", v.entries),
                        Some(_) => "✗ TAMPER DETECTED".into(),
                        None => "could not verify".into(),
                    }))
                });
            }
            Action::Distill => {
                self.fetch(|c| async move {
                    let _ = c.consciousness_distill().await;
                    c.consciousness().await.ok().map(Msg::Consciousness)
                });
                self.toast("· re-distilling self-model");
            }
            Action::Refresh => {
                self.refresh_spine();
                self.load_view(self.view);
                self.toast("· refreshed");
            }
            Action::ToggleTheme => {
                self.light = !self.light;
                self.theme = if self.light {
                    Theme::light()
                } else {
                    Theme::dark()
                };
            }
            Action::ToggleMouse => {
                self.mouse = !self.mouse;
                self.toast(if self.mouse {
                    "· mouse on (click & scroll)"
                } else {
                    "· mouse off (text selection)"
                });
            }
            Action::CopyAnswer => self.yank_last_answer(),
            Action::Quit => self.should_quit = true,
        }
    }

    pub fn set_view(&mut self, v: View) {
        if self.view != v {
            self.sel = 0;
            self.detail_open = false;
            self.confirm_forget = None;
            self.confirm_agent = None;
            if self.view != View::Help {
                self.prev_view = self.view;
            }
        }
        self.view = v;
        self.load_view(v);
    }

    // ---- selection helpers (used by list views) --------------------------

    pub fn clamp_sel(&mut self, len: usize) {
        if len == 0 {
            self.sel = 0;
        } else if self.sel >= len {
            self.sel = len - 1;
        }
    }

    pub fn move_sel(&mut self, delta: i32, len: usize) {
        if len == 0 {
            return;
        }
        let cur = self.sel as i32 + delta;
        self.sel = cur.clamp(0, len as i32 - 1) as usize;
    }
}

/// Extract `[{title,status}]` from an `update_plan` tool call's args.
pub fn parse_plan(args: &Value) -> Vec<PlanStep> {
    args.get("steps")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let title = s.get("title").and_then(|t| t.as_str())?.to_string();
                    let status = s
                        .get("status")
                        .and_then(|t| t.as_str())
                        .unwrap_or("todo")
                        .to_string();
                    Some(PlanStep { title, status })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Split a command line into args with shell-like quoting (`'…'`, `"…"`, `\`),
/// so an MCP arg containing spaces survives an edit round-trip.
fn shell_split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '"' => {
                started = true;
                while let Some(q) = chars.next() {
                    if q == '\\' {
                        if let Some(n) = chars.next() {
                            cur.push(n);
                        }
                    } else if q == '"' {
                        break;
                    } else {
                        cur.push(q);
                    }
                }
            }
            '\\' => {
                started = true;
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            _ => {
                started = true;
                cur.push(c);
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

/// Join args back into a command line, quoting any that contain whitespace/quotes
/// (the inverse of [`shell_split`] for the common cases).
fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.is_empty()
                || a.chars()
                    .any(|c| c.is_whitespace() || c == '"' || c == '\'')
            {
                format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Copy `text` to the system clipboard by shelling out to the platform tool
/// (pbcopy / wl-copy / xclip / xsel / clip). No extra dependency.
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let candidates: &[(&str, &[&str])] = &[
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
        ("clip", &[]),
    ];
    let mut last_err: Option<std::io::Error> = None;
    for (cmd, args) in candidates {
        match Command::new(cmd)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(text.as_bytes())?;
                }
                let _ = child.wait();
                return Ok(());
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no clipboard tool found")
    }))
}
