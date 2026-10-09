use std::io::{self, IsTerminal, Write};
use std::panic::PanicHookInfo;
use std::path::Path;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
#[cfg(test)]
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{HighlightSpacing, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

use crate::commands::list;
use crate::download::IndexPolicy;
use crate::error::Error;
use crate::resolve;

const VIEWPORT_ROWS: u16 = 12;

pub fn is_interactive() -> bool {
    is_interactive_with(
        io::stderr().is_terminal(),
        std::env::var("TERM").ok().as_deref(),
    )
}

fn is_interactive_with(stderr_is_tty: bool, term: Option<&str>) -> bool {
    stderr_is_tty && !term.is_some_and(|value| value == "dumb")
}

pub fn choose_installed(
    root: &Path,
    cwd: &Path,
    gv_version: Option<&str>,
) -> Result<Option<String>, Error> {
    let items = installed_items(root, cwd, gv_version)?;
    run_ui("installed", items)
}

pub async fn choose_remote(root: &Path) -> Result<Option<String>, Error> {
    let loaded = list::load_remote_rows(root, false, IndexPolicy::RefreshIfStale, None).await?;
    if let Some(warning) = loaded.warning {
        eprintln!("gv: {warning}");
    }
    if loaded.rows.is_empty() {
        return Err(Error::Failed("No matching remote versions".into()));
    }
    let items = loaded
        .rows
        .into_iter()
        .map(|row| PickerItem {
            version: row.version,
            marked: row.installed,
        })
        .collect();
    run_ui("remote", items)
}

fn installed_items(
    root: &Path,
    cwd: &Path,
    gv_version: Option<&str>,
) -> Result<Vec<PickerItem>, Error> {
    let installed = resolve::installed_versions(root)?;
    if installed.is_empty() {
        return Err(Error::Failed("No Go versions are installed".into()));
    }
    let current = match resolve::resolve_using(cwd, root, gv_version, Some(&installed)) {
        Ok(resolved) => Some(resolved.version.to_string()),
        Err(Error::NoVersion) | Err(Error::NotInstalled(_)) => None,
        Err(err) => return Err(err),
    };
    Ok(installed
        .into_iter()
        .map(|version| {
            let name = version.to_string();
            let marked = Some(name.as_str()) == current.as_deref();
            PickerItem {
                version: name,
                marked,
            }
        })
        .collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PickerItem {
    version: String,
    marked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Up,
    Down,
    Enter,
    Cancel,
    Backspace,
    Char(char),
    Ignore,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Continue,
    Select(String),
    Cancel,
}

#[derive(Clone, Debug)]
struct Picker {
    items: Vec<PickerItem>,
    filter: String,
    cursor: usize,
}

impl Picker {
    fn new(items: Vec<PickerItem>) -> Self {
        Self {
            items,
            filter: String::new(),
            cursor: 0,
        }
    }

    fn filter(&self) -> &str {
        &self.filter
    }

    fn cursor(&self) -> usize {
        self.cursor
    }

    fn filtered(&self) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                crate::download::remote_version_matches(&item.version, &self.filter)
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn handle(&mut self, key: Key) -> Step {
        match key {
            Key::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                Step::Continue
            }
            Key::Down => {
                let len = self.filtered().len();
                if len > 0 && self.cursor + 1 < len {
                    self.cursor += 1;
                }
                Step::Continue
            }
            Key::Enter => {
                let filtered = self.filtered();
                let Some(index) = filtered.get(self.cursor).copied() else {
                    return Step::Continue;
                };
                Step::Select(self.items[index].version.clone())
            }
            Key::Cancel => Step::Cancel,
            Key::Backspace => {
                self.filter.pop();
                self.cursor = 0;
                Step::Continue
            }
            Key::Char('q') => {
                if self.filter.is_empty() {
                    Step::Cancel
                } else {
                    Step::Continue
                }
            }
            Key::Char(ch) if ch.is_ascii_digit() || ch == '.' => {
                self.filter.push(ch);
                self.cursor = 0;
                Step::Continue
            }
            Key::Char(_) | Key::Ignore => Step::Continue,
        }
    }
}

fn run_ui(title: &str, items: Vec<PickerItem>) -> Result<Option<String>, Error> {
    let guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(io::stderr()),
        TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_ROWS),
        },
    )?;
    let choice = drive(&mut terminal, title, items);
    let dismissed = dismiss(&mut terminal);
    drop(terminal);
    drop(guard);
    dismissed?;
    choice
}

fn drive(
    terminal: &mut Terminal<CrosstermBackend<io::Stderr>>,
    title: &str,
    items: Vec<PickerItem>,
) -> Result<Option<String>, Error> {
    let mut picker = Picker::new(items);
    loop {
        terminal.draw(|frame| draw(frame, title, &picker))?;
        let Some(key) = read_key()? else {
            continue;
        };
        match picker.handle(key) {
            Step::Continue => {}
            Step::Select(version) => return Ok(Some(version)),
            Step::Cancel => return Ok(None),
        }
    }
}

fn dismiss(terminal: &mut Terminal<CrosstermBackend<io::Stderr>>) -> Result<(), Error> {
    let origin = terminal.get_frame().area().as_position();
    terminal.clear()?;
    terminal.set_cursor_position(origin)?;
    io::stderr().flush()?;
    Ok(())
}

fn draw(frame: &mut Frame, title: &str, picker: &Picker) {
    let [title_area, list_area, filter_area, help_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let filtered = picker.filtered();
    let items: Vec<ListItem> = filtered
        .iter()
        .map(|index| {
            let item = &picker.items[*index];
            let mark = if item.marked { "*" } else { " " };
            ListItem::new(format!("{mark} {}", item.version))
        })
        .collect();
    let mut state = ListState::default();
    if !filtered.is_empty() {
        state.select(Some(picker.cursor()));
    }
    let list = List::new(items)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">")
        .highlight_spacing(HighlightSpacing::Always);

    frame.render_widget(
        Paragraph::new(title).style(Style::new().add_modifier(Modifier::BOLD)),
        title_area,
    );
    frame.render_stateful_widget(list, list_area, &mut state);
    frame.render_widget(
        Paragraph::new(format!("filter: {}", picker.filter())),
        filter_area,
    );
    frame.render_widget(Paragraph::new("enter select · esc cancel"), help_area);
}

fn read_key() -> io::Result<Option<Key>> {
    let event = event::read()?;
    let Event::Key(key) = event else {
        return Ok(None);
    };
    if key.kind != KeyEventKind::Press {
        return Ok(None);
    }
    Ok(Some(translate(key)))
}

fn translate(key: KeyEvent) -> Key {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return if matches!(key.code, KeyCode::Char('c' | 'C')) {
            Key::Cancel
        } else {
            Key::Ignore
        };
    }
    match key.code {
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Cancel,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Char(ch) => Key::Char(ch),
        _ => Key::Ignore,
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stderr(), Show);
}

type PanicHook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

struct TerminalGuard {
    previous_hook: Option<PanicHook>,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(err) = execute!(io::stderr(), Hide) {
            let _ = disable_raw_mode();
            return Err(err);
        }
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_info| {
            restore_terminal();
        }));
        Ok(Self {
            previous_hook: Some(previous_hook),
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
        if let Some(previous_hook) = self.previous_hook.take() {
            std::panic::set_hook(previous_hook);
        }
    }
}

#[cfg(test)]
fn buffer_text(buffer: &Buffer) -> String {
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{touch_sdk, TempDir};
    use ratatui::backend::TestBackend;

    fn sample() -> Picker {
        Picker::new(vec![
            PickerItem {
                version: "1.23.4".into(),
                marked: true,
            },
            PickerItem {
                version: "1.22.5".into(),
                marked: false,
            },
            PickerItem {
                version: "1.22.1".into(),
                marked: false,
            },
        ])
    }

    #[test]
    fn interactive_requires_a_terminal_and_a_real_term() {
        assert!(is_interactive_with(true, Some("xterm")));
        assert!(is_interactive_with(true, None));
        assert!(!is_interactive_with(true, Some("dumb")));
        assert!(!is_interactive_with(false, Some("xterm")));
    }

    #[test]
    fn filter_arrows_and_enter_select_the_highlighted_match() {
        let mut picker = sample();
        assert_eq!(picker.handle(Key::Char('1')), Step::Continue);
        assert_eq!(picker.handle(Key::Char('.')), Step::Continue);
        assert_eq!(picker.handle(Key::Char('2')), Step::Continue);
        assert_eq!(picker.handle(Key::Char('2')), Step::Continue);
        assert_eq!(picker.filter(), "1.22");
        assert_eq!(
            picker
                .filtered()
                .into_iter()
                .map(|index| picker.items[index].version.as_str())
                .collect::<Vec<_>>(),
            vec!["1.22.5", "1.22.1"]
        );
        assert_eq!(picker.handle(Key::Down), Step::Continue);
        assert_eq!(picker.handle(Key::Down), Step::Continue);
        assert_eq!(picker.cursor(), 1);
        assert_eq!(picker.handle(Key::Up), Step::Continue);
        assert_eq!(picker.handle(Key::Up), Step::Continue);
        assert_eq!(picker.cursor(), 0);
        assert_eq!(picker.handle(Key::Enter), Step::Select("1.22.5".into()));
    }

    #[test]
    fn escape_and_empty_filter_q_cancel() {
        let mut picker = sample();
        assert_eq!(picker.handle(Key::Cancel), Step::Cancel);
        let mut picker = sample();
        assert_eq!(picker.handle(Key::Char('q')), Step::Cancel);
    }

    #[test]
    fn q_does_not_enter_a_filter_and_unmatched_enter_does_nothing() {
        let mut picker = sample();
        picker.handle(Key::Char('1'));
        assert_eq!(picker.handle(Key::Char('q')), Step::Continue);
        assert_eq!(picker.filter(), "1");
        picker.handle(Key::Backspace);
        picker.handle(Key::Char('9'));
        assert_eq!(picker.handle(Key::Enter), Step::Continue);
        assert_eq!(picker.handle(Key::Char('a')), Step::Continue);
        assert_eq!(picker.filter(), "9");
    }

    #[test]
    fn exact_filter_selects_that_version() {
        let mut picker = sample();
        for ch in ['1', '.', '2', '3', '.', '4'] {
            picker.handle(Key::Char(ch));
        }
        assert_eq!(picker.handle(Key::Enter), Step::Select("1.23.4".into()));
    }

    #[test]
    fn installed_list_marks_the_current_version_and_rejects_an_empty_root() {
        let root = TempDir::new();
        let cwd = TempDir::new();
        let empty = installed_items(root.path(), cwd.path(), None).unwrap_err();
        assert_eq!(empty.to_string(), "No Go versions are installed");

        touch_sdk(root.path(), "1.22.1");
        touch_sdk(root.path(), "1.23.4");
        std::fs::write(cwd.path().join(".go-version"), "1.23.4\n").unwrap();
        let items = installed_items(root.path(), cwd.path(), None).unwrap();
        assert_eq!(items[0].version, "1.23.4");
        assert!(items[0].marked);
        assert!(!items[1].marked);

        std::fs::write(cwd.path().join(".go-version"), "9.9.9\n").unwrap();
        let missing = installed_items(root.path(), cwd.path(), None).unwrap();
        assert!(missing.iter().all(|item| !item.marked));
    }

    #[test]
    fn frame_shows_the_title_mark_filter_and_help() {
        let mut picker = sample();
        picker.handle(Key::Char('1'));
        picker.handle(Key::Char('.'));
        picker.handle(Key::Char('2'));
        picker.handle(Key::Char('3'));
        let backend = TestBackend::new(32, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw(frame, "installed", &picker))
            .unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("installed"), "{text}");
        assert!(text.contains("* 1.23.4"), "{text}");
        assert!(!text.contains("1.22.5"), "{text}");
        assert!(text.contains("filter: 1.23"), "{text}");
        assert!(text.contains("enter select · esc cancel"), "{text}");
    }
}
