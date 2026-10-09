//! Terminal screens for install progress and version lists.
//!
//! Ratatui 0.30 draws these with the default Crossterm backend (`crossterm` feature,
//! Crossterm 0.29). `Gauge` is the install progress bar. `List` / `ListState` is the
//! scrollable version list. Callers keep the plain-text path for `--quiet`, a
//! non-terminal, `TERM=dumb`, and `--output json`.

use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Gauge, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

use crate::error::Error;

const INSTALL_ROW_HEIGHT: u16 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPhase {
    Waiting,
    Downloading,
    Extracting,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub struct InstallRow {
    pub version: String,
    pub filename: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Time since this row started downloading. Snapshots freeze this so a frame
    /// and a test see the same speed.
    pub elapsed: Duration,
    started: Option<Instant>,
    pub cached: bool,
    pub phase: InstallPhase,
}

impl InstallRow {
    fn bytes_per_sec(&self) -> Option<u64> {
        let secs = self.elapsed.as_secs_f64();
        if self.downloaded > 0 && secs > 0.0 {
            Some((self.downloaded as f64 / secs).round() as u64)
        } else {
            None
        }
    }
}

struct InstallBoardInner {
    rows: Vec<InstallRow>,
}

pub struct InstallBoard {
    inner: Mutex<InstallBoardInner>,
    done: AtomicBool,
}

impl InstallBoard {
    pub fn new(plans: impl IntoIterator<Item = (String, String)>) -> Arc<Self> {
        let rows = plans
            .into_iter()
            .map(|(version, filename)| InstallRow {
                version,
                filename,
                downloaded: 0,
                total: None,
                elapsed: Duration::ZERO,
                started: None,
                cached: false,
                phase: InstallPhase::Waiting,
            })
            .collect();
        Arc::new(Self {
            inner: Mutex::new(InstallBoardInner { rows }),
            done: AtomicBool::new(false),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, InstallBoardInner> {
        self.inner.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub fn set_phase(&self, index: usize, phase: InstallPhase) {
        let mut inner = self.lock();
        if let Some(row) = inner.rows.get_mut(index) {
            if phase == InstallPhase::Downloading && row.started.is_none() {
                row.started = Some(Instant::now());
            }
            row.phase = phase;
        }
    }

    pub fn set_downloaded(&self, index: usize, downloaded: u64) {
        let mut inner = self.lock();
        if let Some(row) = inner.rows.get_mut(index) {
            row.downloaded = downloaded;
            if row.started.is_none() {
                row.started = Some(Instant::now());
            }
        }
    }

    pub fn set_total(&self, index: usize, total: Option<u64>) {
        let mut inner = self.lock();
        if let Some(row) = inner.rows.get_mut(index) {
            row.total = total;
        }
    }

    pub fn mark_cached(&self, index: usize) {
        let mut inner = self.lock();
        if let Some(row) = inner.rows.get_mut(index) {
            row.cached = true;
        }
    }

    pub fn mark_done(&self) {
        self.done.store(true, Ordering::Relaxed);
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> Vec<InstallRow> {
        let mut rows = self.lock().rows.clone();
        for row in &mut rows {
            row.elapsed = row
                .started
                .map(|started| started.elapsed())
                .unwrap_or_default();
        }
        rows
    }
}

/// Owns the stderr alternate screen until downloads finish.
pub struct InstallScreen {
    board: Arc<InstallBoard>,
    handle: Option<JoinHandle<()>>,
}

impl InstallScreen {
    pub fn start(plans: &[(String, String)]) -> io::Result<Self> {
        let board = InstallBoard::new(plans.iter().cloned());
        let shared = Arc::clone(&board);
        let mut session = StderrSession::enter()?;
        let handle = thread::spawn(move || {
            let cancel = session.drive(&shared);
            session.restore();
            if cancel {
                std::process::exit(130);
            }
        });
        Ok(Self {
            board,
            handle: Some(handle),
        })
    }

    pub fn board(&self) -> Arc<InstallBoard> {
        Arc::clone(&self.board)
    }
}

impl Drop for InstallScreen {
    fn drop(&mut self) {
        self.board.mark_done();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Clone)]
pub struct ProgressSlot {
    board: Arc<InstallBoard>,
    index: usize,
}

impl ProgressSlot {
    pub fn new(board: Arc<InstallBoard>, index: usize) -> Self {
        Self { board, index }
    }

    pub fn set_phase(&self, phase: InstallPhase) {
        self.board.set_phase(self.index, phase);
    }

    pub fn set_downloaded(&self, downloaded: u64) {
        self.board.set_downloaded(self.index, downloaded);
    }

    pub fn set_total(&self, total: Option<u64>) {
        self.board.set_total(self.index, total);
    }

    pub fn mark_cached(&self) {
        self.board.mark_cached(self.index);
    }
}

struct StderrSession {
    terminal: Terminal<CrosstermBackend<io::Stderr>>,
    active: bool,
}

impl StderrSession {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(err) = execute!(io::stderr(), EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(err);
        }
        match Terminal::new(CrosstermBackend::new(io::stderr())) {
            Ok(terminal) => Ok(Self {
                terminal,
                active: true,
            }),
            Err(err) => {
                restore_stderr();
                Err(err)
            }
        }
    }

    fn drive(&mut self, board: &InstallBoard) -> bool {
        let listen = io::stdin().is_terminal();
        loop {
            let rows = board.snapshot();
            if self
                .terminal
                .draw(|frame| render_install(frame, &rows))
                .is_err()
            {
                return false;
            }
            if board.is_done() {
                break;
            }
            if listen && cancel_requested() {
                return true;
            }
            if !listen {
                thread::sleep(Duration::from_millis(80));
            }
        }
        let rows = board.snapshot();
        let _ = self.terminal.draw(|frame| render_install(frame, &rows));
        false
    }

    fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        restore_stderr();
    }
}

impl Drop for StderrSession {
    fn drop(&mut self) {
        self.restore();
    }
}

fn restore_stderr() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stderr(), LeaveAlternateScreen, Show);
}

/// Raw mode turns Ctrl-C into a key. Treat that key as cancel so the download
/// screen does not trap the terminal.
fn cancel_requested() -> bool {
    let ready = event::poll(Duration::from_millis(80)).unwrap_or(false);
    if !ready {
        return false;
    }
    match event::read() {
        Ok(Event::Key(key)) => {
            key.kind == KeyEventKind::Press
                && key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        }
        _ => false,
    }
}

pub fn render_install(frame: &mut Frame, rows: &[InstallRow]) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);
    frame.render_widget(Paragraph::new("Installing Go"), chunks[0]);

    let (start, end) = visible_install_rows(rows, chunks[1].height);
    let visible = &rows[start..end];
    let constraints = vec![Constraint::Length(INSTALL_ROW_HEIGHT); visible.len()];
    let gauges = Layout::vertical(constraints).split(chunks[1]);
    for (row, gauge_area) in visible.iter().zip(gauges.iter().copied()) {
        if gauge_area.height == 0 {
            continue;
        }
        frame.render_widget(install_gauge(row), gauge_area);
    }

    let footer = if rows.is_empty() {
        "No downloads".to_string()
    } else if end - start == rows.len() {
        format!("{} version(s)", rows.len())
    } else {
        format!("showing {}-{} of {}", start + 1, end, rows.len())
    };
    frame.render_widget(Paragraph::new(footer), chunks[2]);
}

fn visible_install_rows(rows: &[InstallRow], height: u16) -> (usize, usize) {
    if rows.is_empty() {
        return (0, 0);
    }
    let capacity = (height / INSTALL_ROW_HEIGHT).max(1) as usize;
    if rows.len() <= capacity {
        return (0, rows.len());
    }
    let focus = rows
        .iter()
        .position(|row| {
            matches!(
                row.phase,
                InstallPhase::Waiting | InstallPhase::Downloading | InstallPhase::Extracting
            )
        })
        .unwrap_or(rows.len() - 1);
    let start = focus
        .saturating_sub(capacity / 2)
        .min(rows.len() - capacity);
    (start, start + capacity)
}

fn install_gauge(row: &InstallRow) -> Gauge<'_> {
    let ratio = match row.phase {
        InstallPhase::Done | InstallPhase::Extracting => 1.0,
        InstallPhase::Failed | InstallPhase::Waiting | InstallPhase::Downloading => {
            ratio_of(row.downloaded, row.total)
        }
    };
    let style = match row.phase {
        InstallPhase::Failed => Style::new().red().on_black(),
        InstallPhase::Done => Style::new().green().on_black(),
        InstallPhase::Extracting => Style::new().yellow().on_black(),
        InstallPhase::Waiting | InstallPhase::Downloading => Style::new().cyan().on_black(),
    };
    Gauge::default()
        .block(Block::bordered().title(format!(" {} ", row.filename)))
        .gauge_style(style)
        .ratio(ratio)
        .label(progress_label(row))
        .use_unicode(true)
}

fn ratio_of(downloaded: u64, total: Option<u64>) -> f64 {
    match total {
        Some(total) if total > 0 => (downloaded as f64 / total as f64).clamp(0.0, 1.0),
        Some(_) => 1.0,
        None => 0.0,
    }
}

fn progress_label(row: &InstallRow) -> String {
    match row.phase {
        InstallPhase::Waiting => "Waiting".to_string(),
        InstallPhase::Extracting if row.cached => {
            format!("Extracting Go {} (cached archive)", row.version)
        }
        InstallPhase::Extracting => format!("Extracting Go {}", row.version),
        InstallPhase::Done if row.cached => format!("Done Go {} (cached archive)", row.version),
        InstallPhase::Done => format!("Done {}", byte_progress(row)),
        InstallPhase::Failed => format!("Failed {}", byte_progress(row)),
        InstallPhase::Downloading => byte_progress(row),
    }
}

fn byte_progress(row: &InstallRow) -> String {
    let amount = match row.total {
        Some(total) => format!("{}/{} bytes", row.downloaded, total),
        None => format!("{} bytes", row.downloaded),
    };
    let Some(speed) = row.bytes_per_sec() else {
        return amount;
    };
    let mut text = format!("{amount} ({speed} bytes/sec)");
    if let Some(total) = row.total {
        if speed > 0 && row.downloaded < total {
            let eta = (total - row.downloaded) / speed;
            text.push_str(&format!(" eta {eta}s"));
        }
    }
    text
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRow {
    pub version: String,
    /// Same star the plain text uses: the resolved version, or an installed remote version.
    pub starred: bool,
    /// The version `gv` would run in this directory.
    pub resolved: bool,
}

pub fn browse_versions(title: &str, rows: &[VersionRow]) -> Result<(), Error> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut session = StdoutSession::enter()?;
    let mut state = ListState::default();
    let initial = rows
        .iter()
        .position(|row| row.resolved)
        .or_else(|| rows.iter().position(|row| row.starred))
        .unwrap_or(0);
    state.select(Some(initial));
    let result = browse_loop(&mut session, title, rows, &mut state);
    session.restore();
    result
}

fn browse_loop(
    session: &mut StdoutSession,
    title: &str,
    rows: &[VersionRow],
    state: &mut ListState,
) -> Result<(), Error> {
    loop {
        session
            .terminal
            .draw(|frame| render_versions(frame, title, rows, state))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let event = event::read()?;
        let Event::Key(key) = event else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(());
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => return Ok(()),
            KeyCode::Down | KeyCode::Char('j') => move_selection(state, rows.len(), 1),
            KeyCode::Up | KeyCode::Char('k') => move_selection(state, rows.len(), -1),
            KeyCode::PageDown => move_selection(state, rows.len(), 10),
            KeyCode::PageUp => move_selection(state, rows.len(), -10),
            KeyCode::Home => state.select(Some(0)),
            KeyCode::End => {
                if !rows.is_empty() {
                    state.select(Some(rows.len() - 1));
                }
            }
            _ => {}
        }
    }
}

fn move_selection(state: &mut ListState, len: usize, delta: isize) {
    if len == 0 {
        return;
    }
    let current = state.selected().unwrap_or(0) as isize;
    let next = (current + delta).clamp(0, len as isize - 1) as usize;
    state.select(Some(next));
}

struct StdoutSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    active: bool,
}

impl StdoutSession {
    fn enter() -> Result<Self, Error> {
        enable_raw_mode()?;
        if let Err(err) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(err.into());
        }
        match Terminal::new(CrosstermBackend::new(io::stdout())) {
            Ok(terminal) => Ok(Self {
                terminal,
                active: true,
            }),
            Err(err) => {
                restore_stdout();
                Err(err.into())
            }
        }
    }

    fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        restore_stdout();
    }
}

impl Drop for StdoutSession {
    fn drop(&mut self) {
        self.restore();
    }
}

fn restore_stdout() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
}

pub fn render_versions(frame: &mut Frame, title: &str, rows: &[VersionRow], state: &mut ListState) {
    let area = frame.area();
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let text = version_line(row);
            let line = if row.resolved {
                Line::from(text).green().bold()
            } else if row.starred {
                Line::from(text).green()
            } else {
                Line::from(text)
            };
            ListItem::new(line)
        })
        .collect();
    let list = List::new(items)
        .block(Block::bordered().title(format!(" {title} ")))
        .highlight_style(Style::new().reversed())
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, chunks[0], state);
    frame.render_widget(
        Paragraph::new("↑↓/jk scroll  q quit    * resolved"),
        chunks[1],
    );
}

fn version_line(row: &VersionRow) -> String {
    let star = if row.starred || row.resolved {
        "* "
    } else {
        "  "
    };
    if row.resolved {
        format!("{star}{}  current", row.version)
    } else {
        format!("{star}{}", row.version)
    }
}

#[cfg(test)]
fn render_to_string(width: u16, height: u16, draw: impl FnOnce(&mut Frame)) -> String {
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(draw).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let area = buffer.area;
    let mut lines = Vec::new();
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            line.push_str(buffer[(x, y)].symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row() -> InstallRow {
        InstallRow {
            version: "1.23.4".into(),
            filename: "go1.23.4.linux-amd64.tar.gz".into(),
            downloaded: 500,
            total: Some(1000),
            elapsed: Duration::from_secs(2),
            started: None,
            cached: false,
            phase: InstallPhase::Downloading,
        }
    }

    #[test]
    fn install_screen_shows_progress_bytes_and_speed() {
        let row = sample_row();
        let text = render_to_string(80, 8, |frame| render_install(frame, &[row]));
        assert!(text.contains("Installing Go"), "{text}");
        assert!(text.contains("go1.23.4.linux-amd64.tar.gz"), "{text}");
        assert!(text.contains("500/1000 bytes"), "{text}");
        assert!(text.contains("250 bytes/sec"), "{text}");
        assert!(text.contains("eta "), "{text}");
        assert!(!text.contains('\u{1b}'), "{text}");
    }

    #[test]
    fn install_screen_stacks_one_gauge_per_version() {
        let mut first = sample_row();
        first.version = "1.22.5".into();
        first.filename = "go1.22.5.linux-amd64.tar.gz".into();
        first.phase = InstallPhase::Done;
        first.downloaded = 10;
        first.total = Some(10);
        let second = sample_row();
        let text = render_to_string(80, 12, |frame| render_install(frame, &[first, second]));
        assert!(text.contains("go1.22.5.linux-amd64.tar.gz"), "{text}");
        assert!(text.contains("go1.23.4.linux-amd64.tar.gz"), "{text}");
        assert!(text.contains("500/1000 bytes"), "{text}");
        assert!(text.contains("2 version(s)"), "{text}");
    }

    #[test]
    fn plain_progress_label_has_no_terminal_escapes() {
        let row = sample_row();
        let label = progress_label(&row);
        assert_eq!(label, "500/1000 bytes (250 bytes/sec) eta 2s");
        assert!(!label.contains('\u{1b}'));
        let unknown = InstallRow {
            total: None,
            downloaded: 40,
            elapsed: Duration::ZERO,
            started: None,
            phase: InstallPhase::Downloading,
            ..sample_row()
        };
        assert_eq!(progress_label(&unknown), "40 bytes");
    }

    #[test]
    fn version_list_scrolls_to_the_resolved_row() {
        let rows: Vec<VersionRow> = (0..30)
            .map(|index| VersionRow {
                version: format!("1.0.{index}"),
                starred: index == 25,
                resolved: index == 25,
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(25));
        let text = render_to_string(40, 10, |frame| {
            render_versions(frame, "Installed versions", &rows, &mut state);
        });
        assert!(text.contains("* 1.0.25"), "{text}");
        assert!(text.contains("current"), "{text}");
        assert!(!text.contains("1.0.0"), "{text}");
        assert!(text.contains("scroll"), "{text}");

        state.select(Some(0));
        let top = render_to_string(40, 10, |frame| {
            render_versions(frame, "Installed versions", &rows, &mut state);
        });
        assert!(top.contains("1.0.0"), "{top}");
        assert!(!top.contains("1.0.25"), "{top}");
    }

    #[test]
    fn remote_rows_mark_installed_and_the_resolved_version() {
        let rows = vec![
            VersionRow {
                version: "1.24.0".into(),
                starred: true,
                resolved: false,
            },
            VersionRow {
                version: "1.22.5".into(),
                starred: false,
                resolved: true,
            },
        ];
        let mut state = ListState::default();
        state.select(Some(1));
        let text = render_to_string(48, 10, |frame| {
            render_versions(frame, "Remote versions", &rows, &mut state);
        });
        assert!(text.contains("* 1.24.0"), "{text}");
        assert!(text.contains("* 1.22.5"), "{text}");
        assert!(text.contains("current"), "{text}");
    }

    #[test]
    fn move_selection_stays_inside_the_list() {
        let mut state = ListState::default();
        state.select(Some(0));
        move_selection(&mut state, 3, -1);
        assert_eq!(state.selected(), Some(0));
        move_selection(&mut state, 3, 10);
        assert_eq!(state.selected(), Some(2));
    }
}
