use std::{
    env, fs,
    io::{self, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use rodio::Source;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize)]
struct Requirement {
    title: String,
    body: String,
    status: Status,
    notes: String,
    evidence: Vec<usize>,
    expanded: bool,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Unseen,
    Strong,
    Weak,
    Missing,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Unseen => "UNSEEN",
            Status::Strong => "STRONG",
            Status::Weak => "WEAK",
            Status::Missing => "MISSING",
        }
    }

    fn color(self) -> Color {
        match self {
            Status::Unseen => Color::Gray,
            Status::Strong => Color::Green,
            Status::Weak => Color::Yellow,
            Status::Missing => Color::Red,
        }
    }
}

#[derive(Clone, Serialize)]
struct Segment {
    start: f64,
    end: f64,
    label: String,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Requirements,
    Transcript,
    Notes,
    Feedback,
}

struct App {
    audio_path: Option<PathBuf>,
    audio_player: Option<AudioPlayer>,
    requirements_path: Option<PathBuf>,
    srt_path: Option<PathBuf>,
    requirements: Vec<Requirement>,
    segments: Vec<Segment>,
    active_requirement: usize,
    active_segment: usize,
    pane: Pane,
    editing_note: bool,
    message: String,
    model: String,
    feedback_markdown: String,
    feedback_scroll: u16,
}

struct AudioPlayer {
    stream: rodio::OutputStream,
    sink: Option<rodio::Sink>,
    current_label: Option<String>,
}

#[derive(Default)]
struct Args {
    audio: Option<PathBuf>,
    srt: Option<PathBuf>,
    requirements: Option<PathBuf>,
    model: String,
    auto_assess: bool,
    language: Option<String>,
    prompt: Option<String>,
}

#[derive(Serialize)]
struct AutoAssessRequest {
    requirements: Vec<AutoAssessRequirement>,
    segments: Vec<AutoAssessSegment>,
}

#[derive(Serialize)]
struct AutoAssessRequirement {
    index: usize,
    title: String,
    body: String,
}

#[derive(Serialize)]
struct AutoAssessSegment {
    index: usize,
    label: String,
    text: String,
}

#[derive(Deserialize)]
struct AutoAssessResponse {
    requirements: Vec<AutoAssessRequirementResponse>,
}

#[derive(Deserialize)]
struct AutoAssessRequirementResponse {
    requirement_index: usize,
    status: Status,
    note: String,
    evidence_segment_indices: Vec<usize>,
}

#[derive(Serialize)]
struct FeedbackRequest {
    assignment_instructions: String,
    assessment_notes_markdown: String,
}

#[derive(Deserialize)]
struct FeedbackResponse {
    grade: String,
    feedback_markdown: String,
}

fn main() -> io::Result<()> {
    let mut args = parse_args();
    normalize_args(&mut args)?;
    ensure_transcript(&mut args)?;
    let auto_assess = args.auto_assess;
    let mut app = App::load(args);
    if auto_assess {
        app.auto_assess();
    }
    run_terminal(&mut app)
}

fn parse_args() -> Args {
    let mut args = Args {
        model: String::from("gpt-4.1-mini"),
        ..Args::default()
    };
    let mut iter = env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--requirements" | "-r" => args.requirements = iter.next().map(PathBuf::from),
            "--auto" | "-a" => args.auto_assess = true,
            "--model" | "-m" => {
                if let Some(model) = iter.next() {
                    args.model = model;
                }
            }
            "--language" | "-l" => args.language = iter.next(),
            "--prompt" | "-p" => args.prompt = iter.next(),
            value if !value.starts_with('-') => {
                let path = PathBuf::from(&value);
                let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
                match ext.as_deref() {
                    Some("srt") => {
                        if args.srt.is_none() {
                            args.srt = Some(path);
                        } else if args.audio.is_none() {
                            args.audio = Some(path);
                        }
                    }
                    Some("mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "wma") => {
                        if args.audio.is_none() {
                            args.audio = Some(path);
                        } else if args.srt.is_none() {
                            args.srt = Some(path);
                        }
                    }
                    _ => {
                        if args.requirements.is_none() {
                            args.requirements = Some(path);
                        } else if args.audio.is_none() {
                            args.audio = Some(path);
                        } else if args.srt.is_none() {
                            args.srt = Some(path);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    args
}

fn normalize_args(args: &mut Args) -> io::Result<()> {
    args.audio = normalize_optional_path(args.audio.take())?;
    args.srt = normalize_optional_path(args.srt.take())?;
    args.requirements = normalize_optional_path(args.requirements.take())?;
    Ok(())
}

fn normalize_optional_path(path: Option<PathBuf>) -> io::Result<Option<PathBuf>> {
    path.map(normalize_input_path).transpose()
}

fn normalize_input_path(path: PathBuf) -> io::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }

    if path.exists() {
        return path.canonicalize();
    }

    let manifest_relative = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(&path);
    if manifest_relative.exists() {
        return manifest_relative.canonicalize();
    }

    Ok(env::current_dir()?.join(path))
}

fn ensure_transcript(args: &mut Args) -> io::Result<()> {
    if args.srt.is_some() {
        return Ok(());
    }

    let Some(audio_path) = args.audio.as_ref() else {
        return Ok(());
    };

    let srt_path = audio_path.with_extension("srt");
    if srt_path.exists() {
        args.srt = Some(srt_path);
        return Ok(());
    }

    let script = transcribe_script_path();
    if !script.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Could not find transcribe.py at {}", script.display()),
        ));
    }

    eprintln!(
        "No SRT found at {}. Transcribing with {}...",
        srt_path.display(),
        script.display()
    );

    let mut command = Command::new("python3");
    command
        .arg(&script)
        .arg(audio_path)
        .arg("--format")
        .arg("srt")
        .arg("--output")
        .arg(&srt_path)
        .arg("--model")
        .arg("whisper-1");

    if let Some(language) = args.language.as_ref() {
        command.arg("--language").arg(language);
    }

    if let Some(prompt) = args.prompt.as_ref() {
        command.arg("--prompt").arg(prompt);
    }

    let status = command.status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "Transcription failed with status {status}"
        )));
    }

    args.srt = Some(srt_path);
    Ok(())
}

fn transcribe_script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("transcribe.py")
}

fn auto_assess_script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("auto_assess.py")
}

fn generate_feedback_script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("generate_feedback.py")
}

impl App {
    fn load(args: Args) -> Self {
        let mut message = String::from("Ready");
        let audio_player = if args.audio.is_some() {
            match AudioPlayer::new() {
                Ok(player) => Some(player),
                Err(error) => {
                    message = format!("Audio output unavailable: {error}");
                    None
                }
            }
        } else {
            None
        };

        let requirements = args
            .requirements
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .map(|text| parse_assignment(&text))
            .unwrap_or_else(|| {
                vec![Requirement {
                    title: String::from("Load requirements with -r file.md"),
                    body: String::from(
                        "Each heading, bullet, or numbered line becomes a requirement.",
                    ),
                    status: Status::Unseen,
                    notes: String::new(),
                    evidence: Vec::new(),
                    expanded: true,
                }]
            });

        let segments = args
            .srt
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .map(|text| parse_srt(&text))
            .unwrap_or_default();

        if segments.is_empty() {
            message =
                String::from("No transcript loaded. Provide an audio file to auto-transcribe.");
        }

        Self {
            audio_path: args.audio,
            audio_player,
            requirements_path: args.requirements,
            srt_path: args.srt,
            requirements,
            segments,
            active_requirement: 0,
            active_segment: 0,
            pane: Pane::Requirements,
            editing_note: false,
            message,
            model: args.model,
            feedback_markdown: String::new(),
            feedback_scroll: 0,
        }
    }

    fn active_requirement(&self) -> Option<&Requirement> {
        self.requirements.get(self.active_requirement)
    }

    fn active_requirement_mut(&mut self) -> Option<&mut Requirement> {
        self.requirements.get_mut(self.active_requirement)
    }

    fn move_up(&mut self) {
        match self.pane {
            Pane::Requirements => {
                self.active_requirement = self.active_requirement.saturating_sub(1);
            }
            Pane::Transcript => {
                self.active_segment = self.active_segment.saturating_sub(1);
            }
            Pane::Notes => {}
            Pane::Feedback => {
                self.feedback_scroll = self.feedback_scroll.saturating_sub(1);
            }
        }
    }

    fn move_down(&mut self) {
        match self.pane {
            Pane::Requirements => {
                if !self.requirements.is_empty() {
                    self.active_requirement =
                        (self.active_requirement + 1).min(self.requirements.len() - 1);
                }
            }
            Pane::Transcript => {
                if !self.segments.is_empty() {
                    self.active_segment = (self.active_segment + 1).min(self.segments.len() - 1);
                }
            }
            Pane::Notes => {}
            Pane::Feedback => {
                self.feedback_scroll = self.feedback_scroll.saturating_add(1);
            }
        }
    }

    fn next_pane(&mut self) {
        self.editing_note = false;
        self.pane = match self.pane {
            Pane::Requirements => Pane::Transcript,
            Pane::Transcript => Pane::Notes,
            Pane::Notes => Pane::Feedback,
            Pane::Feedback => Pane::Requirements,
        };
    }

    fn set_status(&mut self, status: Status) {
        if let Some(requirement) = self.active_requirement_mut() {
            requirement.status = status;
            self.message = format!("Marked requirement {}", status.label());
        }
    }

    fn toggle_expanded(&mut self) {
        if let Some(requirement) = self.active_requirement_mut() {
            requirement.expanded = !requirement.expanded;
        }
    }

    fn toggle_evidence(&mut self) {
        let segment_index = self.active_segment;
        if self.segments.get(segment_index).is_none() {
            self.message = String::from("No transcript segment selected");
            return;
        }

        if let Some(requirement) = self.active_requirement_mut() {
            if let Some(index) = requirement
                .evidence
                .iter()
                .position(|candidate| *candidate == segment_index)
            {
                requirement.evidence.remove(index);
                self.message = String::from("Evidence unlinked");
            } else {
                requirement.evidence.push(segment_index);
                requirement.expanded = true;
                self.message = String::from("Evidence linked");
            }
        }
    }

    fn play_selected(&mut self) {
        let Some(audio_path) = self.audio_path.clone() else {
            self.message = String::from("No audio loaded. Start with --audio file.mp3");
            return;
        };
        let Some(segment) = self.segments.get(self.active_segment).cloned() else {
            self.message = String::from("No transcript segment selected");
            return;
        };
        let Some(player) = self.audio_player.as_mut() else {
            self.message = String::from("Audio output unavailable");
            return;
        };

        self.message = match player.play_from(&audio_path, segment.start, &segment.label) {
            Ok(()) => format!("Playing from {}", segment.label),
            Err(error) => format!("Playback failed: {error}"),
        };
    }

    fn toggle_playback(&mut self) {
        let Some(player) = self.audio_player.as_mut() else {
            self.message = String::from("Audio output unavailable");
            return;
        };

        self.message = player
            .toggle_pause()
            .unwrap_or("No active playback. Select a transcript line and press Enter.")
            .to_string();
    }

    fn auto_assess(&mut self) {
        if self.requirements.is_empty() || self.segments.is_empty() {
            self.message = String::from("Auto-assess needs requirements and transcript segments");
            return;
        }

        let payload = AutoAssessRequest {
            requirements: self
                .requirements
                .iter()
                .enumerate()
                .map(|(index, requirement)| AutoAssessRequirement {
                    index,
                    title: requirement.title.clone(),
                    body: requirement.body.clone(),
                })
                .collect(),
            segments: self
                .segments
                .iter()
                .enumerate()
                .map(|(index, segment)| AutoAssessSegment {
                    index,
                    label: segment.label.clone(),
                    text: segment.text.clone(),
                })
                .collect(),
        };

        let Ok(input) = serde_json::to_string(&payload) else {
            self.message = String::from("Could not serialize auto-assess payload");
            return;
        };

        let mut child = match Command::new("python3")
            .arg(auto_assess_script_path())
            .arg("--model")
            .arg(&self.model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                self.message = format!("Could not run auto-assess helper: {error}");
                return;
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            if let Err(error) = stdin.write_all(input.as_bytes()) {
                self.message = format!("Could not send auto-assess payload: {error}");
                return;
            }
        }

        let output = match child.wait_with_output() {
            Ok(output) => output,
            Err(error) => {
                self.message = format!("Could not read auto-assess response: {error}");
                return;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            self.message = format!("Auto-assess failed: {}", stderr.trim());
            return;
        }

        let parsed = serde_json::from_slice::<AutoAssessResponse>(&output.stdout);
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                self.message = format!("Could not parse auto-assess response: {error}");
                return;
            }
        };

        let mut applied = 0;
        for suggestion in parsed.requirements {
            if suggestion.requirement_index >= self.requirements.len() {
                continue;
            }
            let evidence = suggestion
                .evidence_segment_indices
                .into_iter()
                .filter(|index| *index < self.segments.len())
                .collect::<Vec<_>>();
            let requirement = &mut self.requirements[suggestion.requirement_index];
            requirement.status = suggestion.status;
            requirement.notes = suggestion.note;
            requirement.evidence = evidence;
            requirement.expanded = true;
            applied += 1;
        }

        self.message = format!("Auto-assess applied {applied} requirement suggestions");
    }

    fn generate_feedback(&mut self) {
        if self.requirements.is_empty() {
            self.message = String::from("Feedback generation needs requirements");
            return;
        }

        let assignment_instructions = self
            .requirements_path
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .unwrap_or_else(|| String::from("No requirements file was loaded."));

        let payload = FeedbackRequest {
            assignment_instructions,
            assessment_notes_markdown: self.build_assessment_notes_markdown(),
        };

        let Ok(input) = serde_json::to_string(&payload) else {
            self.message = String::from("Could not serialize feedback payload");
            return;
        };

        let mut child = match Command::new("python3")
            .arg(generate_feedback_script_path())
            .arg("--model")
            .arg(&self.model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                self.message = format!("Could not run feedback helper: {error}");
                return;
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            if let Err(error) = stdin.write_all(input.as_bytes()) {
                self.message = format!("Could not send feedback payload: {error}");
                return;
            }
        }

        let output = match child.wait_with_output() {
            Ok(output) => output,
            Err(error) => {
                self.message = format!("Could not read feedback response: {error}");
                return;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            self.message = format!("Feedback generation failed: {}", stderr.trim());
            return;
        }

        let parsed = serde_json::from_slice::<FeedbackResponse>(&output.stdout);
        match parsed {
            Ok(parsed) => {
                let mut markdown = parsed.feedback_markdown;
                if !markdown.starts_with(&format!("Grade: {}", parsed.grade)) {
                    markdown = format!("Grade: {}\n\n{}", parsed.grade, markdown);
                }
                self.feedback_markdown = markdown;
                self.feedback_scroll = 0;
                self.pane = Pane::Feedback;
                self.message = format!("Generated feedback — Grade: {}", parsed.grade);
            }
            Err(error) => {
                self.message = format!("Could not parse feedback response: {error}");
            }
        }
    }

    fn export_markdown(&mut self) {
        match fs::write("verbalstudio-feedback.md", self.build_markdown()) {
            Ok(()) => self.message = String::from("Exported verbalstudio-feedback.md"),
            Err(error) => self.message = format!("Markdown export failed: {error}"),
        }
    }

    fn export_json(&mut self) {
        let payload = AssessmentExport {
            audio_path: self
                .audio_path
                .as_ref()
                .map(|path| path.display().to_string()),
            requirements_path: self
                .requirements_path
                .as_ref()
                .map(|path| path.display().to_string()),
            srt_path: self
                .srt_path
                .as_ref()
                .map(|path| path.display().to_string()),
            requirements: self.requirements.clone(),
            segments: self.segments.clone(),
            feedback_markdown: self.feedback_markdown.clone(),
        };

        match serde_json::to_string_pretty(&payload)
            .ok()
            .and_then(|json| fs::write("verbalstudio-assessment.json", json).ok())
        {
            Some(()) => self.message = String::from("Exported verbalstudio-assessment.json"),
            None => self.message = String::from("JSON export failed"),
        }
    }

    fn build_markdown(&self) -> String {
        let mut sections = Vec::new();
        if !self.feedback_markdown.trim().is_empty() {
            sections.push(self.feedback_markdown.trim().to_string());
        }
        sections.push(self.build_assessment_notes_markdown());
        sections.join("\n\n---\n\n")
    }

    fn build_assessment_notes_markdown(&self) -> String {
        self.requirements
            .iter()
            .map(|requirement| {
                let evidence = requirement
                    .evidence
                    .iter()
                    .filter_map(|index| self.segments.get(*index))
                    .map(|segment| format!("- {}: {}", segment.label, segment.text))
                    .collect::<Vec<_>>()
                    .join("\n");

                format!(
                    "## {}\n\nStatus: {}\n\n{}\n\nEvidence:\n{}",
                    requirement.title,
                    requirement.status.label(),
                    requirement.notes,
                    if evidence.is_empty() {
                        String::from("No evidence linked.")
                    } else {
                        evidence
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

impl AudioPlayer {
    fn new() -> Result<Self, String> {
        let mut stream =
            rodio::OutputStreamBuilder::open_default_stream().map_err(|error| error.to_string())?;
        stream.log_on_drop(false);

        Ok(Self {
            stream,
            sink: None,
            current_label: None,
        })
    }

    fn play_from(&mut self, audio_path: &PathBuf, start: f64, label: &str) -> Result<(), String> {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }

        let file = fs::File::open(audio_path).map_err(|error| error.to_string())?;
        let source = rodio::Decoder::try_from(file)
            .map_err(|error| error.to_string())?
            .skip_duration(Duration::from_secs_f64(start.max(0.0)));
        let sink = rodio::Sink::connect_new(self.stream.mixer());
        sink.append(source);

        self.current_label = Some(label.to_string());
        self.sink = Some(sink);
        Ok(())
    }

    fn toggle_pause(&mut self) -> Option<&'static str> {
        let sink = self.sink.as_ref()?;
        if sink.is_paused() {
            sink.play();
            Some("Playback resumed")
        } else {
            sink.pause();
            Some("Playback paused")
        }
    }

    fn status(&self) -> String {
        let Some(sink) = self.sink.as_ref() else {
            return String::from("idle");
        };
        let label = self.current_label.as_deref().unwrap_or("audio");
        if sink.empty() {
            format!("finished {label}")
        } else if sink.is_paused() {
            format!("paused {label}")
        } else {
            format!("playing {label}")
        }
    }
}

#[derive(Serialize)]
struct AssessmentExport {
    audio_path: Option<String>,
    requirements_path: Option<String>,
    srt_path: Option<String>,
    requirements: Vec<Requirement>,
    segments: Vec<Segment>,
    feedback_markdown: String,
}

fn run_terminal(app: &mut App) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, app))?;

        if !event::poll(Duration::from_millis(80))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };

        if handle_key(app, key) {
            return Ok(());
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    if app.editing_note {
        match key.code {
            KeyCode::Esc => app.editing_note = false,
            KeyCode::Enter => {
                if let Some(requirement) = app.active_requirement_mut() {
                    requirement.notes.push('\n');
                }
            }
            KeyCode::Backspace => {
                if let Some(requirement) = app.active_requirement_mut() {
                    requirement.notes.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(requirement) = app.active_requirement_mut() {
                    requirement.notes.push(c);
                }
            }
            _ => {}
        }
        return false;
    }

    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
        KeyCode::Tab => app.next_pane(),
        KeyCode::Up | KeyCode::Char('k') => app.move_up(),
        KeyCode::Down | KeyCode::Char('j') => app.move_down(),
        KeyCode::Enter => match app.pane {
            Pane::Requirements => app.toggle_expanded(),
            Pane::Transcript => app.play_selected(),
            Pane::Notes => app.editing_note = true,
            Pane::Feedback => {}
        },
        KeyCode::Char(' ') => app.toggle_playback(),
        KeyCode::Char('l') => app.toggle_evidence(),
        KeyCode::Char('a') => app.auto_assess(),
        KeyCode::Char('f') => app.generate_feedback(),
        KeyCode::Char('n') => {
            app.pane = Pane::Notes;
            app.editing_note = true;
        }
        KeyCode::Char('0') => app.set_status(Status::Unseen),
        KeyCode::Char('1') | KeyCode::Char('+') => app.set_status(Status::Strong),
        KeyCode::Char('2') | KeyCode::Char('w') => app.set_status(Status::Weak),
        KeyCode::Char('3') | KeyCode::Char('m') => app.set_status(Status::Missing),
        KeyCode::Char('e') => app.export_markdown(),
        KeyCode::Char('s') => app.export_json(),
        _ => {}
    }
    false
}

fn draw(frame: &mut Frame, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(8),
            Constraint::Length(2),
        ])
        .split(frame.area());

    draw_header(frame, root[0], app);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(44),
            Constraint::Percentage(26),
        ])
        .split(root[1]);

    draw_requirements(frame, panes[0], app);
    draw_transcript(frame, panes[1], app);
    draw_notes(frame, panes[2], app);
    draw_feedback(frame, root[2], app);
    draw_footer(frame, root[3], app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let audio = app
        .audio_path
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "no audio".into());
    let srt = app
        .srt_path
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "no srt".into());
    let requirements = app
        .requirements_path
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "no requirements".into());

    let line = Line::from(vec![
        Span::styled(
            " VerbalStudio ",
            Style::default().fg(Color::Black).bg(Color::Green),
        ),
        Span::raw(" "),
        Span::styled(audio, Style::default().fg(Color::White)),
        Span::raw(" | "),
        Span::styled(srt, Style::default().fg(Color::White)),
        Span::raw(" | "),
        Span::styled(requirements, Style::default().fg(Color::White)),
    ]);

    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_requirements(frame: &mut Frame, area: Rect, app: &App) {
    let text_width = area.width.saturating_sub(14).max(12) as usize;
    let items = app
        .requirements
        .iter()
        .map(|requirement| {
            let marker = if requirement.expanded { "v" } else { ">" };
            let wrapped = wrap_words(&requirement.title, text_width);
            let mut lines = Vec::new();
            for (index, part) in wrapped.iter().enumerate() {
                if index == 0 {
                    lines.push(Line::from(vec![
                        Span::raw(format!("{marker} ")),
                        Span::styled(part.clone(), Style::default().fg(Color::White)),
                        Span::raw(" "),
                        Span::styled(
                            requirement.status.label(),
                            Style::default().fg(requirement.status.color()),
                        ),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(part.clone(), Style::default().fg(Color::White)),
                    ]));
                }
            }
            ListItem::new(lines)
        })
        .collect::<Vec<_>>();

    let mut state = ListState::default();
    state.select(Some(app.active_requirement));
    let block = focused_block("Requirements", app.pane == Pane::Requirements);
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_transcript(frame: &mut Frame, area: Rect, app: &App) {
    let active_evidence = app
        .active_requirement()
        .map(|requirement| requirement.evidence.as_slice())
        .unwrap_or(&[]);
    let text_width = area.width.saturating_sub(12).max(12) as usize;

    let items = app
        .segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let linked = active_evidence.contains(&index);
            let style = if linked {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };
            let wrapped = wrap_words(&segment.text, text_width);
            let mut lines = Vec::new();
            for (line_index, part) in wrapped.iter().enumerate() {
                if line_index == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{:>7} ", segment.label),
                            Style::default().fg(Color::Gray),
                        ),
                        Span::styled(part.clone(), style),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw("        "),
                        Span::styled(part.clone(), style),
                    ]));
                }
            }
            ListItem::new(lines)
        })
        .collect::<Vec<_>>();

    let mut state = ListState::default();
    if !app.segments.is_empty() {
        state.select(Some(app.active_segment));
    }

    let list = List::new(items)
        .block(focused_block(
            "Evidence / Transcript",
            app.pane == Pane::Transcript,
        ))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_notes(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = Vec::new();
    if let Some(requirement) = app.active_requirement() {
        lines.push(Line::styled(
            requirement.title.as_str(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::raw(""));
        lines.push(Line::styled("Notes:", Style::default().fg(Color::Gray)));
        if requirement.notes.is_empty() {
            lines.push(Line::styled(
                "Press n or Enter in this pane to type.",
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            lines.extend(requirement.notes.lines().map(Line::raw));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Linked evidence:",
            Style::default().fg(Color::Gray),
        ));
        for index in &requirement.evidence {
            if let Some(segment) = app.segments.get(*index) {
                lines.push(Line::raw(format!("{} {}", segment.label, segment.text)));
            }
        }
    }

    let title = if app.editing_note {
        "Teacher Review INSERT"
    } else {
        "Teacher Review"
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(focused_block(title, app.pane == Pane::Notes))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_feedback(frame: &mut Frame, area: Rect, app: &App) {
    let (title, preview) = if app.feedback_markdown.trim().is_empty() {
        let preview = app
            .requirements
            .iter()
            .map(|requirement| {
                format!(
                    "{} [{}] note:{} evidence:{}",
                    requirement.title,
                    requirement.status.label(),
                    if requirement.notes.is_empty() {
                        "no"
                    } else {
                        "yes"
                    },
                    requirement.evidence.len()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        ("Assessment Notes Preview", preview)
    } else {
        ("Generated Feedback", app.feedback_markdown.clone())
    };

    frame.render_widget(
        Paragraph::new(preview)
            .block(focused_block(title, app.pane == Pane::Feedback))
            .scroll((app.feedback_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let help = " Tab pane | j/k move | Enter open/play/edit | Space pause/resume | l link | a auto | f feedback | n note | 1/2/3/+ /w/m status | e md | s json | q / Ctrl+c quit | -r/-a/-m ";
    let audio_status = app
        .audio_player
        .as_ref()
        .map(AudioPlayer::status)
        .unwrap_or_else(|| String::from("no audio"));
    let line = Line::from(vec![
        Span::styled(help, Style::default().fg(Color::Gray)),
        Span::styled(
            format!(" audio:{audio_status} "),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(&app.message, Style::default().fg(Color::Green)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn focused_block(title: &'static str, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::default().fg(Color::LightBlue)
    } else {
        Style::default().fg(Color::Gray)
    };
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(style)
}

fn parse_assignment(text: &str) -> Vec<Requirement> {
    let lines = assignment_requirement_lines(text);
    let mut requirements = lines
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let cleaned = clean_assignment_line(line);
            if cleaned.is_empty() {
                return None;
            }
            Some(Requirement {
                title: truncate(&cleaned, 78),
                body: cleaned,
                status: Status::Unseen,
                notes: String::new(),
                evidence: Vec::new(),
                expanded: false,
            })
        })
        .collect::<Vec<_>>();

    if requirements.is_empty() {
        requirements.push(Requirement {
            title: String::from("No requirements found"),
            body: String::from("Add headings or bullet lines to the assignment file."),
            status: Status::Unseen,
            notes: String::new(),
            evidence: Vec::new(),
            expanded: true,
        });
    }

    requirements
}

fn assignment_requirement_lines(text: &str) -> String {
    let mut in_checklist = false;
    let mut found_checklist = false;
    let mut checklist_lines = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        let heading = trimmed.trim_start_matches('#').trim().to_lowercase();
        if trimmed.starts_with("##") && heading == "requirements checklist" {
            in_checklist = true;
            found_checklist = true;
            continue;
        }
        if found_checklist && in_checklist && trimmed.starts_with("##") {
            break;
        }
        if in_checklist {
            checklist_lines.push(line);
        }
    }

    if found_checklist {
        checklist_lines.join("\n")
    } else {
        text.to_string()
    }
}

fn clean_assignment_line(line: &str) -> String {
    let cleaned = line
        .trim_start_matches('#')
        .trim()
        .trim_start_matches("- ")
        .trim_start_matches("* ")
        .trim();
    let Some((prefix, rest)) = cleaned.split_once(['.', ')']) else {
        return cleaned.to_string();
    };
    if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
        return rest.trim().to_string();
    }
    cleaned.to_string()
}

fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let current_len = current.chars().count();
        let word_len = word.chars().count();
        let next_len = if current.is_empty() {
            word_len
        } else {
            current_len + 1 + word_len
        };

        if next_len <= width {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            continue;
        }

        if !current.is_empty() {
            lines.push(current);
            current = String::new();
        }

        if word_len <= width {
            current.push_str(word);
            continue;
        }

        let mut chunk = String::new();
        for ch in word.chars() {
            if chunk.chars().count() == width {
                lines.push(chunk);
                chunk = String::new();
            }
            chunk.push(ch);
        }
        current = chunk;
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn parse_srt(text: &str) -> Vec<Segment> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split("\n\n")
        .filter_map(|block| {
            let lines = block
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>();
            let timing_index = lines.iter().position(|line| line.contains("-->"))?;
            let (start_raw, end_raw) = lines[timing_index].split_once("-->")?;
            let start = parse_timestamp(start_raw.trim())?;
            let end = parse_timestamp(end_raw.trim())?;
            let text = lines[timing_index + 1..].join(" ");
            if text.is_empty() {
                return None;
            }
            Some(Segment {
                start,
                end,
                label: short_time(start),
                text,
            })
        })
        .collect()
}

fn parse_timestamp(value: &str) -> Option<f64> {
    let normalized = value.replace(',', ".");
    let parts = normalized.split(':').collect::<Vec<_>>();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [h, m, s] => (
            h.parse::<f64>().ok()?,
            m.parse::<f64>().ok()?,
            s.parse::<f64>().ok()?,
        ),
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        _ => return None,
    };
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

fn short_time(seconds: f64) -> String {
    let seconds = seconds.floor() as u64;
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_srt_segments() {
        let srt = "1\n00:00:02,000 --> 00:00:06,500\nFirst line.\n\n2\n00:01:01,250 --> 00:01:03,000\nSecond line.";
        let segments = parse_srt(srt);

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].label, "0:02");
        assert_eq!(segments[0].text, "First line.");
        assert!((segments[1].start - 61.25).abs() < 0.001);
    }

    #[test]
    fn parses_assignment_lines() {
        let assignment = "# Main task\n\n- Use two texts\n2. Give a conclusion";
        let requirements = parse_assignment(assignment);

        assert_eq!(requirements.len(), 3);
        assert_eq!(requirements[0].title, "Main task");
        assert_eq!(requirements[1].title, "Use two texts");
        assert_eq!(requirements[2].title, "Give a conclusion");
    }

    #[test]
    fn prefers_requirements_checklist_section() {
        let assignment = "# Guidance\n\nThis should not become a row.\n\n## Requirements Checklist\n\n- Cover Don Draper\n- Use two theme texts\n\n## Reference Guidance\n\n- Ignore this detail";
        let requirements = parse_assignment(assignment);

        assert_eq!(requirements.len(), 2);
        assert_eq!(requirements[0].title, "Cover Don Draper");
        assert_eq!(requirements[1].title, "Use two theme texts");
    }
}
