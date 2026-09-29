//! The interactive session: one loop over terminal input, agent events, the turn in flight and a
//! frame tick. The loop owns the transcript and lends it to each turn, which hands it back, so a
//! turn runs alongside input without an engine thread.

use std::future::Future;
use std::pin::Pin;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::anyhow;
use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::agent::event::{AgentEvent, Outcome};
use crate::agent::message::Part;
use crate::agent::run::{self, RunCtx};
use crate::agent::state::{Reply, Shared};
use crate::agent::system_prompt;
use crate::api::models::{self, Model};
use crate::app::App;
use crate::commands::{self, Ctx, Effect, Scope};
use crate::io::clock;
use crate::io::term::{self, Input, KeyCode, KeyModifiers};
use crate::limits;
use crate::mode::{self, Mode};
use crate::session::store::Store;
use crate::session::transcript::Transcript;
use crate::text::styled::{Line, Style};
use crate::text::template;
use crate::text::theme;
use crate::text::units;
use crate::tools::Registry;
use crate::tui::editor::Editor;
use crate::tui::feed::Feed;
use crate::tui::input::Burst;
use crate::tui::keymap::{self, Action};
use crate::tui::queue::{self, Kind};
use crate::tui::screen::Screen;
use crate::tui::status::{self, Status};
use crate::tui::{footer, ui, welcome};

type Turn<'a> = Pin<Box<dyn Future<Output = (Transcript, anyhow::Result<Outcome>)> + 'a>>;

/// What the loop should do after an input.
enum Step {
    Continue,
    Send(String),
    Quit,
}

pub async fn run(app: &App, initial: Option<String>) -> anyhow::Result<ExitCode> {
    let catalog = models::load(app, false).await;
    // crowbot silently swaps an unknown model id for its default, so refuse it here instead.
    let model = catalog.get(&app.settings.model).cloned().ok_or_else(|| {
        anyhow!(
            "unknown model `{}`; see `crowbot models`",
            app.settings.model
        )
    })?;
    let mode = mode::get(&app.settings.mode).ok_or_else(|| anyhow!("mode was validated"))?;
    let system = system_prompt::build(&app.paths, &model);
    let registry = Registry::builtin(app);
    let transcript = Transcript::new(Some(Store::create(&app.paths)?));
    let session_file = transcript.path().map(|p| p.display().to_string());
    let plan_file = app
        .paths
        .plans_dir()
        .join(format!("{}.md", transcript.id()))
        .to_string_lossy()
        .replace('\\', "/");
    let shared = Shared::new(mode);
    let (tx, rx) = mpsc::unbounded_channel();
    let emit = move |event: AgentEvent| {
        let _ = tx.send(event);
    };
    let cx = RunCtx {
        app,
        model: &model,
        effort: app.settings.effort.as_deref(),
        system: &system,
        tools: &registry,
        plan_file,
        emit: &emit,
    };

    let raw = term::Raw::enter()?;
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        term::restore();
        default_hook(info);
    }));
    let mut tui = Tui::new(app, &model, &shared, initial);
    let spent = tui.run(&cx, transcript, rx).await;
    drop(raw);

    let text = &ui::get().text;
    if let Some(path) = session_file {
        term::out(&format!(
            "{}\n",
            template::fill(&text.saved, &[("path", &path)])
        ));
    }
    if spent > 0 {
        let cost = units::usd(spent as f64 / 1e6);
        term::out(&format!(
            "{}\n",
            template::fill(&text.spent, &[("cost", &cost)])
        ));
    }
    Ok(ExitCode::SUCCESS)
}

struct Tui<'a> {
    app: &'a App,
    model: &'a Model,
    shared: &'a Shared,
    screen: Screen,
    feed: Feed,
    editor: Editor,
    burst: Burst,
    queue: Vec<(Kind, String)>,
    height: usize,
    turn_started: Option<Instant>,
    last_ctrl_c: Option<Instant>,
    cost_micros: u64,
    context_pct: Option<u64>,
}

impl<'a> Tui<'a> {
    fn new(app: &'a App, model: &'a Model, shared: &'a Shared, initial: Option<String>) -> Self {
        let limits = &limits::get().tui;
        let (width, height) = term::size();
        let mut feed = Feed::new(width);
        let mode = shared.mode();
        let cwd = app.paths.project.display().to_string();
        feed.raw(welcome::render(
            &welcome::Info {
                version: env!("CARGO_PKG_VERSION"),
                cwd: &cwd,
                model: &model.id,
                effort: app.settings.effort.as_deref(),
                mode_label: &mode.label,
                mode_color: &mode.color,
                logged_in: app.api.has_key(),
                braille: term::braille(),
            },
            width,
        ));
        let mut editor = Editor::new(limits.history_max.value);
        if let Some(text) = initial {
            editor.set_text(&text);
        }
        Self {
            app,
            model,
            shared,
            screen: Screen::new(width, height, theme::get(), term::color_depth()),
            feed,
            editor,
            burst: Burst::new(limits.paste_gap_ms.ms(), limits.paste_min_keys.value),
            queue: Vec::new(),
            height,
            turn_started: None,
            last_ctrl_c: None,
            cost_micros: 0,
            context_pct: None,
        }
    }

    /// Runs until the user quits; returns what the session spent, in micro-dollars.
    async fn run(
        &mut self,
        cx: &'a RunCtx<'a>,
        transcript: Transcript,
        mut events: mpsc::UnboundedReceiver<AgentEvent>,
    ) -> u64 {
        let mut transcript = Some(transcript);
        let mut turn: Option<Turn<'a>> = None;
        let mut inputs = Box::pin(term::inputs());
        let mut tick = tokio::time::interval(limits::get().tui.frame_ms.ms());
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let shared = self.shared;
        loop {
            self.draw(turn.is_some());
            tokio::select! {
                input = inputs.next() => {
                    let Some(input) = input else { break };
                    match self.input(input, turn.is_some()).await {
                        Step::Continue => {}
                        Step::Quit => break,
                        Step::Send(text) => {
                            let Some(mut owned) = transcript.take() else { continue };
                            self.turn_started = Some(clock::instant());
                            let parts = vec![Part::Text { text }];
                            turn = Some(Box::pin(async move {
                                let outcome = run::run(cx, &mut owned, shared, parts).await;
                                (owned, outcome)
                            }));
                        }
                    }
                }
                Some(event) = events.recv() => {
                    self.agent(event);
                    while let Ok(event) = events.try_recv() {
                        self.agent(event);
                    }
                }
                (owned, outcome) = async { turn.as_mut().expect("guarded by the branch condition").await }, if turn.is_some() => {
                    turn = None;
                    transcript = Some(owned);
                    self.turn_started = None;
                    self.queue.clear();
                    while let Ok(event) = events.try_recv() {
                        self.agent(event);
                    }
                    if let Err(e) = outcome {
                        self.feed.notice(&format!("{e:#}"), "error");
                    }
                }
                _ = tick.tick(), if turn.is_some() => {}
            }
        }
        // Leave nothing half-drawn: clear the live region before the terminal is handed back.
        let bytes = self.screen.frame(&self.feed.take_commits(), &[]);
        term::out(&bytes);
        self.cost_micros
    }

    async fn input(&mut self, input: Input, running: bool) -> Step {
        match input {
            Input::Resize(width, height) => {
                self.screen.resize(width, height);
                self.feed.set_width(width);
                self.height = height;
                Step::Continue
            }
            Input::Paste(text) => {
                self.editor.insert(&text);
                Step::Continue
            }
            Input::Key(key) => {
                let pasting = self.burst.is_paste(clock::instant());
                // Inside a paste, Enter and Tab are text, never commands.
                if pasting {
                    match key.code {
                        KeyCode::Enter => {
                            self.editor.insert("\n");
                            return Step::Continue;
                        }
                        KeyCode::Tab => {
                            self.editor.insert("    ");
                            return Step::Continue;
                        }
                        _ => {}
                    }
                }
                match keymap::get().action(&key) {
                    Some(action) => self.action(action, running).await,
                    None => {
                        let plain = !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
                        if let (KeyCode::Char(c), true) = (key.code, plain) {
                            self.editor.insert(&c.to_string());
                        }
                        Step::Continue
                    }
                }
            }
        }
    }

    async fn action(&mut self, action: Action, running: bool) -> Step {
        match action {
            Action::Submit => return self.submit(running).await,
            Action::Tab if running => self.enqueue(Kind::Steering),
            Action::Tab => self.complete(),
            Action::CycleMode => self.cycle_mode(),
            Action::Escape if running => self.interrupt(),
            Action::Escape => {}
            Action::CtrlC => {
                if !self.editor.is_empty() {
                    self.editor.clear();
                } else if running {
                    self.interrupt();
                } else {
                    let now = clock::instant();
                    let window = limits::get().tui.quit_window_ms.ms();
                    if self
                        .last_ctrl_c
                        .is_some_and(|t| now.duration_since(t) <= window)
                    {
                        return Step::Quit;
                    }
                    self.last_ctrl_c = Some(now);
                }
            }
            Action::Quit if self.editor.is_empty() => return Step::Quit,
            Action::Quit => {}
            Action::ToggleReasoning => {
                let text = &ui::get().text;
                let note = if self.feed.toggle_reasoning() {
                    &text.reasoning_shown
                } else {
                    &text.reasoning_hidden
                };
                self.feed.notice(note, "muted");
            }
            editing => {
                self.editor.apply(editing);
            }
        }
        Step::Continue
    }

    async fn submit(&mut self, running: bool) -> Step {
        let text = self.editor.text();
        // A trailing backslash asks for a new line, for terminals that cannot send Shift+Enter.
        if let Some(stripped) = text.strip_suffix('\\') {
            self.editor.set_text(&format!("{stripped}\n"));
            return Step::Continue;
        }
        if text.trim().is_empty() {
            return Step::Continue;
        }
        if running {
            self.enqueue(Kind::Queued);
            return Step::Continue;
        }
        let text = self.editor.take();
        if let Some(command) = text.trim().strip_prefix('/') {
            return self.command(command).await;
        }
        self.feed.user(&text);
        Step::Send(text)
    }

    /// Hands typed text to the run: steering lands after the current tools, a queued message
    /// when the run would otherwise stop.
    fn enqueue(&mut self, kind: Kind) {
        let text = self.editor.take();
        if text.trim().is_empty() {
            return;
        }
        let parts = vec![Part::Text { text: text.clone() }];
        match kind {
            Kind::Steering => self.shared.steer(parts),
            Kind::Queued => self.shared.follow_up(parts),
        }
        self.queue.push((kind, text));
    }

    /// Stops the run and puts anything still queued back in the editor.
    fn interrupt(&mut self) {
        self.shared.interrupt();
        let mut pulled: Vec<String> = self
            .shared
            .drain_steer()
            .into_iter()
            .chain(self.shared.drain_follow())
            .map(|parts| text_of(&parts))
            .collect();
        self.queue.clear();
        if !pulled.is_empty() {
            if !self.editor.is_empty() {
                pulled.push(self.editor.text());
            }
            self.editor.set_text(&pulled.join("\n"));
        }
    }

    fn cycle_mode(&mut self) {
        let modes = mode::all();
        let current = self.shared.mode();
        let at = modes.iter().position(|m| m.id == current.id).unwrap_or(0);
        self.shared.set_mode(&modes[(at + 1) % modes.len()]);
    }

    /// Completes a slash command name when only one fits.
    fn complete(&mut self) {
        let text = self.editor.text();
        let Some(prefix) = text.strip_prefix('/').filter(|p| !p.contains(' ')) else {
            return;
        };
        let matches: Vec<&str> = commands::available(Scope::Session)
            .map(|c| c.spec().name.as_str())
            .filter(|name| name.starts_with(prefix))
            .collect();
        if let [only] = matches.as_slice() {
            self.editor.set_text(&format!("/{only} "));
        }
    }

    async fn command(&mut self, line: &str) -> Step {
        let words: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        let Some((name, args)) = words.split_first() else {
            return Step::Continue;
        };
        let found = commands::find(name).filter(|c| c.spec().scope.contains(&Scope::Session));
        let Some(command) = found else {
            let text = template::fill(&ui::get().text.unknown_command, &[("name", name)]);
            self.feed.notice(&text, "warn");
            return Step::Continue;
        };
        self.feed.user(&format!("/{line}"));
        let cx = Ctx {
            app: self.app,
            scope: Scope::Session,
        };
        match command.run(&cx, args).await {
            Ok(outcome) => {
                if !outcome.text.is_empty() {
                    self.feed.markdown(&outcome.text);
                }
                for effect in outcome.effects {
                    match effect {
                        Effect::Quit => return Step::Quit,
                        Effect::CycleMode => self.cycle_mode(),
                        Effect::SetMode(id) => {
                            if let Some(mode) = mode::get(&id) {
                                self.shared.set_mode(mode);
                            }
                        }
                    }
                }
            }
            Err(e) => self.feed.notice(&format!("{e:#}"), "error"),
        }
        Step::Continue
    }

    fn agent(&mut self, event: AgentEvent) {
        let now = clock::instant();
        match &event {
            AgentEvent::MessageEnd { message } => {
                if let Some(usage) = message.usage {
                    self.cost_micros += usage.cost_micros;
                    let window = self.model.context_window.max(1);
                    self.context_pct = Some(usage.input * 100 / window);
                }
            }
            AgentEvent::Delivered { text } => {
                if let Some(i) = self.queue.iter().position(|(_, t)| t == text) {
                    self.queue.remove(i);
                }
            }
            // CEILING: until the prompt views land (M3 step 3.6), prompts are declined so the
            // model hears why; AUTO mode never asks.
            AgentEvent::Ask { id, .. } => {
                self.shared.answer(*id, Reply::Unavailable);
            }
            _ => {}
        }
        self.feed.event(&event, now);
    }

    fn draw(&mut self, running: bool) {
        let now = clock::instant();
        let width = self.screen_width();
        let mode = self.shared.mode();
        let commits = self.feed.take_commits();
        let mut live = self.feed.live(now);
        live.extend(queue::render(&self.queue, width));
        if running {
            live.push(self.status(now, width));
        }
        if let Some(at) = self.last_ctrl_c
            && now.duration_since(at) <= limits::get().tui.quit_window_ms.ms()
            && !running
        {
            live.push(Line::styled(
                &ui::get().text.ctrl_c_again,
                Style::fg("muted"),
            ));
        }
        live.push(border(mode, width));
        let ui = ui::get();
        let prompt = Line::styled(&ui.prompt, Style::fg(&mode.color).bold());
        let rows = (self.height * limits::get().tui.editor_max_rows_pct.value / 100).max(3);
        live.extend(self.editor.render(width, rows, &prompt, &ui.placeholder));
        live.push(footer::render(
            &footer::State {
                mode_label: &mode.label,
                mode_color: &mode.color,
                model: &self.model.id,
                crow: self.model.id.starts_with("crow"),
                effort: self.app.settings.effort.as_deref(),
                context_pct: self.context_pct,
                cost_micros: self.cost_micros,
            },
            width,
        ));
        let bytes = self.screen.frame(&commits, &live);
        if !bytes.is_empty() {
            term::out(&bytes);
        }
    }

    fn status(&self, now: Instant, width: usize) -> Line {
        if let Some((secs, why)) = self.feed.retry(now) {
            return status::render(&Status::Retrying { secs, why }, width);
        }
        let elapsed = self
            .turn_started
            .map_or(0, |t| now.duration_since(t).as_millis());
        let frame = ui::get().spinner_frame(elapsed, limits::get().tui.spinner_ms.value);
        let secs = (elapsed / 1000) as u64;
        status::render(&Status::Working { secs, frame }, width)
    }

    fn screen_width(&self) -> usize {
        term::size().0
    }
}

/// The editor's top border, in the mode's colour with its label.
fn border(mode: &Mode, width: usize) -> Line {
    let style = Style::fg(&mode.color);
    let mut line = Line::styled("── ", style.clone());
    line.push(&mode.label, style.clone().bold());
    line.push(" ", style.clone());
    let used = line.width();
    line.push("─".repeat(width.saturating_sub(used)), style);
    line
}

fn text_of(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
