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
use futures_util::stream::FuturesUnordered;
use tokio::sync::mpsc;

use crate::agent::event::{AgentEvent, Outcome};
use crate::agent::message::Part;
use crate::agent::run::{self, RunCtx};
use crate::agent::state::Shared;
use crate::agent::system_prompt;
use crate::api::models::{self, Catalog, Model};
use crate::app::App;
use crate::commands::{self, Effect, Outcome as CommandOutcome, Scope};
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
use crate::tools::{self, Registry};
use crate::tui::choice::{self, Choice};
use crate::tui::command::{self, Work};
use crate::tui::editor::Editor;
use crate::tui::feed::{Block, Feed};
use crate::tui::input::Burst;
use crate::tui::keymap::{self, Action};
use crate::tui::login::{self, Login};
use crate::tui::palette::{self, Palette};
use crate::tui::picker::{self, Picker};
use crate::tui::queue::{self, Kind};
use crate::tui::screen::Screen;
use crate::tui::status::{self, Progress};
use crate::tui::view::View;
use crate::tui::{frame, layout, ui, welcome};

type Turn<'a> = Pin<Box<dyn Future<Output = (Transcript, Outcome)> + 'a>>;

/// What every turn shares. The model and system prompt are not here: the session can switch
/// them between turns.
struct Turns<'a> {
    tools: &'a Registry,
    emit: &'a (dyn Fn(AgentEvent) + Send + Sync),
    plan_file: String,
}

/// What the loop should do after an input.
enum Step<'a> {
    Continue,
    Send(String),
    Quit,
    /// Start network work for the login card.
    Login(login::Job),
    /// Run beside input: a session command, or what one of them asked for.
    Work(BoxFuture<'a, Work>),
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
    let registry = Registry::builtin(app);
    let transcript = Transcript::new(Some(Store::create(&app.paths)?));
    let session_file = transcript.path().map(|p| p.display().to_string());
    let plan_file = tools::plan_file(&app.paths, &transcript.id());
    let shared = Shared::new(mode);
    let (tx, rx) = mpsc::unbounded_channel();
    let emit = move |event: AgentEvent| {
        let _ = tx.send(event);
    };
    let turns = Turns {
        tools: &registry,
        emit: &emit,
        plan_file,
    };

    let raw = term::Raw::fullscreen()?;
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        term::restore_fullscreen();
        default_hook(info);
    }));
    let mut tui = Tui::new(app, catalog, model, &shared, initial);
    let (spent, transcript) = tui.run(turns, transcript, rx).await;
    drop(raw);

    let text = &ui::get().text;
    // Only a file that still holds the whole conversation counts as saved.
    let saved = match &transcript {
        Some(transcript) => transcript.path().map(|p| p.display().to_string()),
        None => session_file,
    };
    if let Some(path) = saved {
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
    catalog: Catalog,
    /// The model for the next turn, and the system prompt written for it.
    model: Model,
    system: String,
    shared: &'a Shared,
    screen: Screen,
    feed: Feed,
    view: View,
    /// The transcript block on each screen row of the last frame, for clicks.
    blocks: Vec<Option<usize>>,
    editor: Editor,
    palette: Palette,
    burst: Burst,
    queue: Vec<(Kind, String)>,
    /// Prompts waiting on the user; the first is on screen in place of the editor.
    cards: VecDeque<Choice>,
    /// The `/login` card, on screen in place of the editor while open.
    login: Option<Login>,
    /// The `/models` card, likewise.
    picker: Option<Picker>,
    /// The turn in flight, for the working line.
    progress: Option<Progress>,
    last_verb: Option<usize>,
    rng: fastrand::Rng,
    braille: bool,
    last_ctrl_c: Option<Instant>,
    cost_micros: u64,
    context_pct: Option<u64>,
}

impl<'a> Tui<'a> {
    fn new(
        app: &'a App,
        catalog: Catalog,
        model: Model,
        shared: &'a Shared,
        initial: Option<String>,
    ) -> Self {
        let limits = &limits::get().tui;
        let (width, height) = term::size();
        let mut feed = Feed::new();
        let mode = shared.mode();
        feed.push(Block::Welcome(welcome::Info {
            version: env!("CARGO_PKG_VERSION"),
            cwd: app.paths.project.display().to_string(),
            model: model.id.clone(),
            effort: app.settings.effort.clone(),
            mode_label: mode.label.clone(),
            mode_color: mode.color.clone(),
            logged_in: app.api.has_key(),
            braille: term::braille(),
        }));
        let mut editor = Editor::new(limits.history_max.value);
        if let Some(text) = initial {
            editor.set_text(&text);
        }
        Self {
            app,
            catalog,
            system: system_prompt::build(&app.paths, &model),
            model,
            shared,
            screen: Screen::new(width, height, theme::get(), term::color_depth()),
            feed,
            view: View::default(),
            blocks: Vec::new(),
            editor,
            palette: Palette::default(),
            burst: Burst::new(limits.paste_gap_ms.ms(), limits.paste_min_keys.value),
            queue: Vec::new(),
            cards: VecDeque::new(),
            login: None,
            picker: None,
            progress: None,
            last_verb: None,
            rng: fastrand::Rng::new(),
            braille: term::braille(),
            last_ctrl_c: None,
            cost_micros: 0,
            context_pct: None,
        }
    }

    /// Runs until the user quits; returns what the session spent, in micro-dollars, and the
    /// transcript unless a turn still held it.
    async fn run(
        &mut self,
        turns: Turns<'a>,
        transcript: Transcript,
        mut events: mpsc::UnboundedReceiver<AgentEvent>,
    ) -> (u64, Option<Transcript>) {
        let mut transcript = Some(transcript);
        let mut turn: Option<Turn<'a>> = None;
        let mut job: Option<BoxFuture<'a, login::Done>> = None;
        let mut work: FuturesUnordered<BoxFuture<'a, Work>> = FuturesUnordered::new();
        let mut inputs = Box::pin(term::inputs());
        let mut tick = tokio::time::interval(limits::get().tui.frame_ms.ms());
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let shared = self.shared;
        loop {
            self.draw(turn.is_some());
            tokio::select! {
                input = inputs.next() => {
                    let Some(input) = input else { break };
                    match self.input(input, turn.is_some()) {
                        Step::Continue => {}
                        Step::Quit => break,
                        Step::Login(next) => job = Some(login::run(self.app, next)),
                        Step::Work(run) => work.push(run),
                        Step::Send(text) => {
                            let Some(mut owned) = transcript.take() else { continue };
                            let progress = Progress::start(clock::instant(), self.last_verb, &mut self.rng);
                            self.last_verb = Some(progress.verb());
                            self.progress = Some(progress);
                            let parts = vec![Part::Text { text }];
                            let (app, tools, emit) = (self.app, turns.tools, turns.emit);
                            let plan_file = turns.plan_file.clone();
                            let (model, system) = (self.model.clone(), self.system.clone());
                            turn = Some(Box::pin(async move {
                                let cx = RunCtx {
                                    app,
                                    model: &model,
                                    effort: app.settings.effort.as_deref(),
                                    system: &system,
                                    tools,
                                    plan_file,
                                    emit,
                                };
                                let outcome = run::run(&cx, &mut owned, shared, parts).await;
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
                (owned, _) = async { turn.as_mut().expect("guarded by the branch condition").await }, if turn.is_some() => {
                    turn = None;
                    transcript = Some(owned);
                    self.progress = None;
                    self.queue.clear();
                    while let Ok(event) = events.try_recv() {
                        self.agent(event);
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
                Some(done) = work.next(), if !work.is_empty() => {
                    match self.finish(done) {
                        Step::Quit => break,
                        Step::Work(run) => work.push(run),
                        _ => {}
                    }
                }
                _ = tick.tick(), if turn.is_some() => {}
            }
            // Closing the login card drops whatever it was waiting on.
            if self.login.is_none() {
                job = None;
            }
        }
        (self.cost_micros, transcript)
    }

    fn input(&mut self, input: Input, running: bool) -> Step<'a> {
        match input {
            Input::Resize(width, height) => {
                self.screen.resize(width, height);
                Step::Continue
            }
            Input::Scroll(notches) => {
                let lines = limits::get().tui.scroll_lines.value as isize;
                self.view.scroll(notches * lines);
                Step::Continue
            }
            Input::Click { row } => {
                if let Some(Some(block)) = self.blocks.get(row) {
                    self.feed.toggle(*block);
                }
                Step::Continue
            }
            Input::Key(key)
                if matches!(
                    keymap::get().action(&key),
                    Some(Action::PageUp | Action::PageDown)
                ) =>
            {
                let page = self.view.page();
                let up = keymap::get().action(&key) == Some(Action::PageUp);
                self.view.scroll(if up { -page } else { page });
                Step::Continue
            }
            Input::Paste(_) if self.picker.is_some() => Step::Continue,
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
            Input::Key(key) if self.picker.is_some() => {
                self.picker_key(&key);
                Step::Continue
            }
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
                                    self.editor.set_text(&Scope::Session.invoke(&name));
                                    return self.submit(running);
                                }
                            }
                        }
                        self.action(action, running)
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

    fn login_key(&mut self, key: &crate::io::term::KeyEvent) -> Step<'a> {
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

    fn picker_key(&mut self, key: &crate::io::term::KeyEvent) {
        let action = keymap::get().action(key);
        if action == Some(Action::CycleMode) {
            return self.cycle_mode();
        }
        let Some(picker) = &mut self.picker else {
            return;
        };
        match picker.key(action) {
            picker::Step::Stay => {}
            picker::Step::Close => self.picker = None,
            picker::Step::Pick(id) => {
                self.picker = None;
                self.switch_model(&id);
            }
        }
    }

    /// Uses `id` from the next turn on, for this session only.
    fn switch_model(&mut self, id: &str) {
        let Some(model) = self.catalog.get(id).cloned() else {
            return;
        };
        self.system = system_prompt::build(&self.app.paths, &model);
        self.model = model;
        let text = template::fill(&ui::get().picker.switched, &[("model", id)]);
        self.feed.notice(&text, "done");
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

    fn action(&mut self, action: Action, running: bool) -> Step<'a> {
        match action {
            Action::Submit => return self.submit(running),
            // A command is never steering: it would reach the model as text.
            Action::Tab if running && self.is_command() => return self.submit(running),
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
            Action::ToggleReasoning => self.feed.toggle_all(),
            editing => {
                self.editor.apply(editing);
            }
        }
        Step::Continue
    }

    fn submit(&mut self, running: bool) -> Step<'a> {
        let text = self.editor.text();
        // A trailing backslash asks for a new line, for terminals that cannot send Shift+Enter.
        if let Some(stripped) = text.strip_suffix('\\') {
            self.editor.set_text(&format!("{stripped}\n"));
            return Step::Continue;
        }
        if text.trim().is_empty() {
            return Step::Continue;
        }
        // Checked before a run gets the text: a command queued as a message would send its
        // arguments (a `--key`) to the model.
        if let Some(line) = self
            .editor
            .text()
            .trim()
            .strip_prefix(Scope::Session.prefix())
        {
            let line = line.to_owned();
            let shown = Scope::Session.invoke(&commands::redact_line(&line));
            self.editor.take_remembering(shown);
            return self.command(&line);
        }
        if running {
            self.enqueue(Kind::Queued);
            return Step::Continue;
        }
        let text = self.editor.take();
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
        let Some(prefix) = text
            .strip_prefix(Scope::Session.prefix())
            .filter(|p| !p.contains(' '))
        else {
            return;
        };
        let matches: Vec<&str> = commands::available(Scope::Session)
            .map(|c| c.spec().name.as_str())
            .filter(|name| name.starts_with(prefix))
            .collect();
        if let [only] = matches.as_slice() {
            self.editor
                .set_text(&Scope::Session.invoke(&format!("{only} ")));
        }
    }

    fn is_command(&self) -> bool {
        self.editor
            .text()
            .trim_start()
            .starts_with(Scope::Session.prefix())
    }

    fn command(&mut self, line: &str) -> Step<'a> {
        match command::start(self.app, line) {
            Ok(started) => {
                self.feed.user(&started.echo);
                Step::Work(started.run)
            }
            Err(note) => {
                if !note.is_empty() {
                    self.feed.notice(&note, "warn");
                }
                Step::Continue
            }
        }
    }

    /// Applies what a command finished with.
    fn finish(&mut self, done: Work) -> Step<'a> {
        let outcome: CommandOutcome = match done {
            Work::Catalog(catalog) => {
                self.catalog = catalog;
                self.picker = Some(Picker::new(&self.catalog, &self.model.id));
                return Step::Continue;
            }
            Work::Command(Err(e)) => {
                self.feed.notice(&format!("{e:#}"), "error");
                return Step::Continue;
            }
            Work::Command(Ok(outcome)) => outcome,
        };
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
                Effect::PickModel { refresh: true } => {
                    return Step::Work(command::refresh_catalog(self.app));
                }
                Effect::PickModel { refresh: false } => {
                    self.picker = Some(Picker::new(&self.catalog, &self.model.id));
                }
            }
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
                    .push_back(Choice::from_prompt(*id, prompt, self.screen.width()));
            }
            _ => {}
        }
        if let Some(progress) = &mut self.progress {
            progress.event(&event);
        }
        self.feed.event(&event, now);
    }

    fn draw(&mut self, running: bool) {
        let now = clock::instant();
        let (width, height) = (self.screen.width(), self.screen.height());
        let mode = self.shared.mode();
        // The bar, pinned to the bottom of the screen.
        let mut live = Vec::new();
        if running {
            live.push(self.status(now, width));
        }
        live.extend(queue::render(&self.queue, width));
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
        // The command popup floats over the conversation instead of growing the bar.
        let mut floating = Vec::new();
        match (&self.login, &self.picker, self.cards.front()) {
            (Some(login), _, _) => live.extend(login.render(width)),
            (None, Some(picker), _) => live.extend(picker.render(width)),
            (None, None, Some(card)) => live.extend(card.render(width)),
            (None, None, None) => {
                live.push(frame::top(mode, width));
                if !running {
                    let found = self.palette.open(&self.editor.text());
                    if !found.is_empty() {
                        floating = self.palette.render(&found, width);
                    }
                }
                let ui = ui::get();
                let prompt = Line::styled(&ui.prompt, Style::fg(&mode.color).bold());
                let rows = (height * limits::get().tui.editor_max_rows_pct.value / 100).max(3);
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
        let frame = layout::compose(
            &mut self.feed,
            &mut self.view,
            &live,
            &floating,
            (width, height),
            now,
        );
        self.blocks = frame.blocks;
        let bytes = self.screen.frame(&frame.rows);
        if !bytes.is_empty() {
            term::out(&bytes);
        }
    }

    fn status(&self, now: Instant, width: usize) -> Line {
        if let Some((secs, why)) = self.feed.retry(now) {
            return status::retrying(secs, why, width);
        }
        let color = &self.shared.mode().color;
        self.progress
            .as_ref()
            .map(|p| p.render(now, color, self.braille, width))
            .unwrap_or_default()
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
