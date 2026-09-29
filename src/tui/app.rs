//! The interactive session: one loop over terminal input, agent events, the turn in flight and a
//! frame tick. The loop owns the transcript and lends it to each turn, which hands it back, so a
//! turn runs alongside input without an engine thread.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::anyhow;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use tokio::sync::mpsc;

use crate::agent::event::{AgentEvent, Outcome};
use crate::agent::message::Part;
use crate::agent::run::{self, RunCtx};
use crate::agent::state::Shared;
use crate::agent::system_prompt;
use crate::api::models::{self, Model};
use crate::app::App;
use crate::commands::{self, Ctx, Effect, Scope};
use crate::io::clock;
use crate::io::term::{self, Input, KeyCode, KeyModifiers};
use crate::limits;
use crate::mode;
use crate::session::store::Store;
use crate::session::transcript::Transcript;
use crate::text::styled::{Line, Style};
use crate::text::template;
use crate::text::theme;
use crate::text::units;
use crate::tools::Registry;
use crate::tui::choice::{self, Choice};
use crate::tui::editor::Editor;
use crate::tui::feed::Feed;
use crate::tui::input::Burst;
use crate::tui::keymap::{self, Action};
use crate::tui::login::{self, Login};
use crate::tui::palette::{self, Palette};
use crate::tui::queue::{self, Kind};
use crate::tui::screen::Screen;
use crate::tui::status::{self, Status};
use crate::tui::{frame, ui, welcome};

type Turn<'a> = Pin<Box<dyn Future<Output = (Transcript, anyhow::Result<Outcome>)> + 'a>>;

/// What the loop should do after an input.
enum Step {
    Continue,
    Send(String),
    Quit,
    /// Start network work for the login card.
    Login(login::Job),
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
    palette: Palette,
    burst: Burst,
    queue: Vec<(Kind, String)>,
    /// Prompts waiting on the user; the first is on screen in place of the editor.
    cards: VecDeque<Choice>,
    /// The `/login` card, on screen in place of the editor while open.
    login: Option<Login>,
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
            screen: Screen::new(
                width,
                height,
                term::cursor_row(),
                theme::get(),
                term::color_depth(),
            ),
            feed,
            editor,
            palette: Palette::default(),
            burst: Burst::new(limits.paste_gap_ms.ms(), limits.paste_min_keys.value),
            queue: Vec::new(),
            cards: VecDeque::new(),
            login: None,
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
        let mut job: Option<BoxFuture<'a, login::Done>> = None;
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
                        Step::Login(next) => job = Some(login::run(self.app, next)),
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
                done = async { job.as_mut().expect("guarded by the branch condition").await }, if job.is_some() => {
                    job = self
                        .login
                        .as_mut()
                        .map(|card| card.finished(done))
                        .and_then(|next| self.login_next(next))
                        .map(|next| login::run(self.app, next));
                }
                _ = tick.tick(), if turn.is_some() => {}
            }
            // Closing the login card drops whatever it was waiting on.
            if self.login.is_none() {
                job = None;
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
                if let Some(card) = &mut self.login {
                    card.insert(&text);
                } else if let Some(card) = self.cards.front_mut() {
                    card.insert(&text);
                } else {
                    self.editor.insert(&text);
                }
                Step::Continue
            }
            Input::Key(key) if self.login.is_some() => self.login_key(&key),
            Input::Key(key) if !self.cards.is_empty() => {
                self.card_key(&key, running);
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
                    Some(action) => {
                        // The command popup takes its keys first; it is closed during a run,
                        // where Tab steers.
                        if !running {
                            match self.palette.key(action, &self.editor.text()) {
                                palette::Step::Ignored => {}
                                palette::Step::Handled => return Step::Continue,
                                palette::Step::Complete(text) => {
                                    self.editor.set_text(&text);
                                    return Step::Continue;
                                }
                                palette::Step::Run(name) => {
                                    self.editor.set_text(&format!("/{name}"));
                                    return self.submit(running).await;
                                }
                            }
                        }
                        self.action(action, running).await
                    }
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

    /// Keys while a prompt card is up: the card answers most, the session keeps a few.
    fn card_key(&mut self, key: &crate::io::term::KeyEvent, running: bool) {
        let action = keymap::get().action(key);
        match action {
            Some(Action::CycleMode) => return self.cycle_mode(),
            Some(Action::CtrlC) if running => return self.interrupt(),
            _ => {}
        }
        let Some(card) = self.cards.front_mut() else {
            return;
        };
        if let choice::Step::Answer(reply) = card.key(action, key) {
            self.shared.answer(card.id, reply);
            self.cards.pop_front();
        }
    }

    fn login_key(&mut self, key: &crate::io::term::KeyEvent) -> Step {
        let action = keymap::get().action(key);
        if action == Some(Action::CycleMode) {
            self.cycle_mode();
            return Step::Continue;
        }
        let next = match &mut self.login {
            Some(card) => card.key(action, key),
            None => return Step::Continue,
        };
        self.login_next(next).map_or(Step::Continue, Step::Login)
    }

    fn login_next(&mut self, next: login::Next) -> Option<login::Job> {
        match next {
            login::Next::Stay => None,
            login::Next::Run(job) => Some(job),
            login::Next::Error(error) => {
                self.feed.notice(&error, "error");
                None
            }
            login::Next::Close(message) => {
                self.login = None;
                if let Some(message) = message {
                    self.feed.notice(&message, "done");
                }
                None
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
                        Effect::Login => self.login = Some(Login::new()),
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
            AgentEvent::Prompt { id, prompt, .. } => {
                self.cards
                    .push_back(Choice::from_prompt(*id, prompt, term::size().0));
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
        // Prompts answered elsewhere (AUTO approved them, the run was interrupted) go away.
        let shared = self.shared;
        self.cards.retain(|card| shared.waiting(card.id));
        match (&self.login, self.cards.front()) {
            (Some(login), _) => live.extend(login.render(width)),
            (None, Some(card)) => live.extend(card.render(width)),
            (None, None) => {
                live.push(frame::top(mode, width));
                if !running {
                    let found = self.palette.open(&self.editor.text());
                    if !found.is_empty() {
                        live.extend(self.palette.render(&found, width));
                    }
                }
                let ui = ui::get();
                let prompt = Line::styled(&ui.prompt, Style::fg(&mode.color).bold());
                let rows = (self.height * limits::get().tui.editor_max_rows_pct.value / 100).max(3);
                live.extend(self.editor.render(width, rows, &prompt, &ui.placeholder));
            }
        }
        live.push(frame::bottom(
            mode,
            &frame::Info {
                model: &self.model.id,
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
