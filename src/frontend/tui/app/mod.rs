//! The interactive session: one loop over terminal input, agent events, the turn in flight and a
//! frame tick. The loop owns the transcript and lends it to each turn, which hands it back, so a
//! turn runs alongside input without an engine thread.

mod account;
mod conversation;
mod draw;
mod effects;
mod input;
mod turn;

use std::collections::VecDeque;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::ExitCode;
use std::time::Instant;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::FuturesUnordered;
use tokio::sync::mpsc;

use crate::agent::setup::{Continue, Setup};
use crate::api::models::Model;
use crate::app::App;
use crate::commands::Session;
use crate::config::limits;
use crate::frontend::tui::card::{self, Card};
use crate::frontend::tui::command::Work;
use crate::frontend::tui::editor::Editor;
use crate::frontend::tui::feed::Feed;
use crate::frontend::tui::palette::Palette;
use crate::frontend::tui::paste::Burst;
use crate::frontend::tui::progress::Progress;
use crate::frontend::tui::queue::Kind;
use crate::frontend::tui::screen::Screen;
use crate::frontend::tui::toast::Toast;
use crate::frontend::tui::view::View;
use crate::frontend::tui::{ui, welcome};
use crate::io::clock;
use crate::io::term;
use crate::live::event::{AgentEvent, Ended};
use crate::live::state::Shared;
use crate::models::Catalog;
use crate::release::{self, update};
use crate::session::transcript::Transcript;
use crate::snapshot::Shadow;
use crate::text::template;
use crate::text::theme;
use crate::text::units;
use crate::tools::Registry;
use account::AccountView;
use conversation::Switch;
use turn::NextTurn;

type Turn<'a> = Pin<Box<dyn Future<Output = (Transcript, Ended)> + 'a>>;

/// What every turn shares. The model and system prompt are not here: the session can switch
/// them between turns.
struct Turns<'a> {
    tools: &'a Registry,
    emit: &'a (dyn Fn(AgentEvent) + Send + Sync),
}

/// What the loop should do after an input.
enum Step<'a> {
    Continue,
    Send(String),
    Quit,
    /// Start network work for the card on screen.
    Job(BoxFuture<'a, card::Done>),
    /// Put the conversation away and start another.
    Session(Switch),
    /// Take back the last prompt, and send it `again` or put it in the editor.
    Undo {
        again: bool,
    },
    /// Run beside input: a session command, or what one of them asked for.
    Work(BoxFuture<'a, Work>),
}

/// The session; going on with the one in `resume` when given, its mode included.
pub async fn run(
    app: &App,
    initial: Option<String>,
    resume: Option<&Path>,
) -> anyhow::Result<ExitCode> {
    let from = resume.map(|path| Continue { path, mode: true });
    let Setup {
        catalog,
        model,
        effort,
        tools,
        shared,
        transcript,
        resumed,
        gone,
        shadow,
    } = Setup::load(app, from).await?;
    let session_file = transcript.file().display().to_string();
    let (tx, rx) = mpsc::unbounded_channel();
    let emit = move |event: AgentEvent| {
        let _ = tx.send(event);
    };
    let turns = Turns {
        tools: &tools,
        emit: &emit,
    };

    let raw = term::Raw::fullscreen()?;
    let mut tui = Tui::new(
        app,
        catalog,
        (model, effort),
        (&shared, &shadow),
        initial,
        session_file,
    );
    if let Some(resumed) = &resumed {
        tui.resumed(&transcript.messages, resumed, gone.as_deref());
    }
    if let Some(change) = update::on_start(&app.http, &app.paths, app.settings.auto_update) {
        tui.feed.notice(&change.say(), "done");
    }
    let (spent, transcript) = tui.run(turns, transcript, rx).await;
    drop(raw);

    let text = &ui::get().text;
    // Only a file that still holds the whole conversation counts as saved; a turn still
    // holding the transcript at exit has already written its prompt there.
    let last = match &transcript {
        Some(transcript) => transcript.path().map(|p| p.display().to_string()),
        None => Some(tui.session_file.clone()),
    };
    for path in tui.earlier.iter().chain(&last) {
        term::out(&format!(
            "{}\n",
            template::fill(&text.saved, &[("path", path)])
        ));
    }
    if spent > 0 {
        let cost = units::usd_micros(spent);
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
    next: NextTurn,
    shared: &'a Shared,
    /// Keeps the files as they were before each prompt changed them, for /undo.
    shadow: &'a Shadow,
    screen: Screen,
    feed: Feed,
    view: View,
    /// The transcript block on each screen row of the last frame, for clicks.
    blocks: Vec<Option<usize>>,
    editor: Editor,
    palette: Palette,
    burst: Burst,
    queue: Vec<(Kind, String)>,
    /// Prompts waiting on the user, each with its id; the first is on screen in place of the
    /// editor.
    cards: VecDeque<(u64, Box<dyn Card>)>,
    /// The session card (`/login`, `/forums`, `/models`), on screen over any prompt while open.
    panel: Option<Box<dyn Card>>,
    /// The turn in flight, for the working line.
    progress: Option<Progress>,
    last_verb: Option<usize>,
    rng: fastrand::Rng,
    braille: bool,
    last_ctrl_c: Option<Instant>,
    toast: Option<Toast>,
    cost_micros: u64,
    /// The current conversation's session file, and those of conversations `/new` put away.
    session_file: String,
    earlier: Vec<String>,
    /// Input tokens of the last request; the bar divides by the current model's window, so a
    /// switch of model shows its own share at once.
    context_tokens: Option<u64>,
    account: AccountView,
}

impl<'a> Tui<'a> {
    fn new(
        app: &'a App,
        catalog: Catalog,
        (model, effort): (Model, Option<String>),
        (shared, shadow): (&'a Shared, &'a Shadow),
        initial: Option<String>,
        session_file: String,
    ) -> Self {
        let limits = &limits::get().tui;
        let (width, height) = term::size();
        let mut editor = Editor::with_history(limits.history_max.value);
        if let Some(text) = initial {
            editor.set_text(&text);
        }
        let mut tui = Self {
            app,
            catalog,
            next: NextTurn::new(&app.paths, model, effort),
            shared,
            shadow,
            screen: Screen::new(width, height, theme::get(), term::color_depth()),
            feed: Feed::new(),
            view: View::default(),
            blocks: Vec::new(),
            editor,
            palette: Palette::default(),
            burst: Burst::new(limits.paste_gap_ms.ms(), limits.paste_min_keys.value),
            queue: Vec::new(),
            cards: VecDeque::new(),
            panel: None,
            progress: None,
            last_verb: None,
            rng: fastrand::Rng::new(),
            braille: term::braille(),
            last_ctrl_c: None,
            toast: None,
            cost_micros: 0,
            session_file,
            earlier: Vec::new(),
            context_tokens: None,
            account: AccountView::new(),
        };
        tui.feed.set_welcome(tui.welcome());
        tui
    }

    /// Share of the current model's window the last request used.
    fn context_pct(&self) -> Option<u64> {
        self.context_tokens
            .map(|tokens| tokens * 100 / self.next.model.context_window.max(1))
    }

    /// What this session reports about itself to a command.
    fn session(&self) -> Session {
        Session {
            model: self.next.model.id.clone(),
            effort: self.next.effort().map(str::to_owned),
            mode: self.shared.mode().label.clone(),
            context_pct: self.context_pct(),
            cost_micros: self.cost_micros,
            file: self.session_file.clone(),
            snapshots_off: self.shadow.off(),
        }
    }

    fn welcome(&self) -> welcome::Info {
        let mode = self.shared.mode();
        welcome::Info {
            version: release::shown(),
            cwd: self.app.paths.project.display().to_string(),
            model: self.next.model.id.clone(),
            effort: self.next.effort().map(str::to_owned),
            mode_label: mode.label.clone(),
            mode_color: mode.color.clone(),
            warning: self.account_warning(),
            braille: self.braille,
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
        let mut job: Option<BoxFuture<'a, card::Done>> = None;
        let mut work: FuturesUnordered<BoxFuture<'a, Work>> = FuturesUnordered::new();
        let mut inputs = Box::pin(term::inputs());
        let mut tick = clock::ticks(limits::get().tui.frame_ms.ms());
        let _shared = self.shared;
        loop {
            if let Some(run) = self.account_work() {
                work.push(run);
            }
            self.draw(turn.is_some());
            let step = tokio::select! {
                input = inputs.next() => {
                    let Some(input) = input else { break };
                    self.input(input, turn.is_some(), job.is_some())
                }
                Some(event) = events.recv() => {
                    self.agent(event);
                    while let Ok(event) = events.try_recv() {
                        self.agent(event);
                    }
                    Step::Continue
                }
                (owned, _) = async { turn.as_mut().expect("guarded by the branch condition").await }, if turn.is_some() => {
                    turn = None;
                    transcript = Some(owned);
                    self.progress = None;
                    self.queue.clear();
                    while let Ok(event) = events.try_recv() {
                        self.agent(event);
                    }
                    Step::Continue
                }
                done = async { job.as_mut().expect("guarded by the branch condition").await }, if job.is_some() => {
                    job = None;
                    self.job_done(done)
                }
                Some(done) = work.next(), if !work.is_empty() => self.finish(done),
                // Frames tick while something moves: a turn, or a toast that has to go away.
                _ = tick.tick(), if turn.is_some() || self.toast.is_some() => Step::Continue,
            };
            match step {
                Step::Continue => {}
                Step::Quit => break,
                Step::Job(run) => job = Some(run),
                Step::Work(run) => work.push(run),
                // Typed text queues while a turn runs; only a command's message gets here.
                Step::Send(text) => match transcript.take() {
                    Some(owned) => turn = Some(self.send(text, owned, &turns)),
                    None => self.feed.notice(&ui::get().text.busy, "warn"),
                },
                // CEILING: the restore runs in the loop, so a huge one holds the screen up to
                // `snapshot.timeout_secs`. Upgrade: run it as `Work` and hold sends meanwhile.
                Step::Undo { again } => match transcript.take() {
                    Some(mut owned) => match self.take_back(&mut owned, again).await {
                        Some(prompt) => turn = Some(self.send(prompt, owned, &turns)),
                        None => transcript = Some(owned),
                    },
                    None => self.feed.notice(&ui::get().text.busy, "warn"),
                },
                // A turn holds the transcript while it runs.
                Step::Session(to) => match transcript.take() {
                    Some(old) => transcript = Some(self.switch_to(old, to)),
                    None => self.feed.notice(&ui::get().text.busy, "warn"),
                },
            }
            // Closing a card drops whatever it was waiting on.
            if self.panel.is_none() {
                job = None;
            }
        }
        (self.cost_micros, transcript)
    }

    fn agent(&mut self, event: AgentEvent) {
        let now = clock::instant();
        match &event {
            AgentEvent::MessageEnd { message } => {
                if let Some(usage) = message.usage {
                    self.cost_micros += usage.cost_micros;
                    self.context_tokens = Some(usage.input);
                }
                if let Some(error) = &message.error {
                    self.account_follow(error);
                }
                self.account_reply_ended();
            }
            AgentEvent::RunEnd(_) => self.account_run_ended(),
            AgentEvent::SnapshotOff { why } => {
                let text = template::fill(&ui::get().undo.off, &[("why", why)]);
                self.feed.notice(&text, "warn");
            }
            AgentEvent::Delivered { text } => {
                if let Some(i) = self.queue.iter().position(|(_, t)| t == text) {
                    self.queue.remove(i);
                }
            }
            AgentEvent::Prompt { id, prompt, .. } => {
                let card = card::from_prompt(prompt, self.draw_width());
                self.cards.push_back((*id, card));
            }
            _ => {}
        }
        if let Some(progress) = &mut self.progress {
            progress.event(&event, now);
        }
        self.feed.event(&event, now);
    }
}
