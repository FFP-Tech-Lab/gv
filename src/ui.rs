use std::io::{self, IsTerminal, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

const SPINNER_DELAY: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Done,
    Notice,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiMode {
    pub animated: bool,
    pub color: bool,
}

pub fn decide(
    quiet: bool,
    stderr_is_tty: bool,
    term: Option<&str>,
    no_color: Option<&str>,
) -> UiMode {
    let dumb = term.is_some_and(|value| value == "dumb");
    let animated = !quiet && stderr_is_tty && !dumb;
    let color = animated && no_color.map(|value| value.is_empty()).unwrap_or(true);
    UiMode { animated, color }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ui {
    mode: UiMode,
}

impl Ui {
    pub fn detect(quiet: bool) -> Self {
        let term = std::env::var("TERM").ok();
        let no_color = std::env::var("NO_COLOR").ok();
        Self {
            mode: decide(
                quiet,
                io::stderr().is_terminal(),
                term.as_deref(),
                no_color.as_deref(),
            ),
        }
    }

    pub fn plain() -> Self {
        Self {
            mode: UiMode {
                animated: false,
                color: false,
            },
        }
    }

    pub fn animated(&self) -> bool {
        self.mode.animated
    }

    pub fn color(&self) -> bool {
        self.mode.color
    }

    pub fn start(&self, message: &str) -> Activity {
        Activity::begin(self.mode, message)
    }

    pub fn finish(&self, tone: Tone, message: &str) {
        if tone == Tone::Failed {
            return;
        }
        write_result(self.mode, tone, message);
    }
}

pub fn format_result_line(tone: Tone, message: &str, color: bool) -> String {
    let message = message.trim_end_matches(['\r', '\n']);
    let glyph = match tone {
        Tone::Done => "✔",
        Tone::Notice => "•",
        Tone::Failed => "✖",
    };
    let painted = match (color, tone) {
        (true, Tone::Done) => format!("\u{1b}[32m{glyph}\u{1b}[0m"),
        (true, Tone::Failed) => format!("\u{1b}[31m{glyph}\u{1b}[0m"),
        _ => glyph.to_string(),
    };
    format!("{painted} {message}")
}

pub fn format_result(tone: Tone, message: &str, color: bool) -> String {
    format!("{}\n", format_result_line(tone, message, color))
}

fn plain_sentence(message: &str) -> String {
    if message.ends_with('\n') {
        message.to_string()
    } else {
        format!("{message}\n")
    }
}

fn write_result(mode: UiMode, tone: Tone, message: &str) {
    if mode.animated {
        let line = format_result(tone, message, mode.color);
        let mut stderr = io::stderr().lock();
        let _ = stderr.write_all(line.as_bytes());
        return;
    }
    if tone == Tone::Failed {
        return;
    }
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(plain_sentence(message).as_bytes());
}

fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner} {msg}").expect("spinner template")
}

struct Shared {
    done: bool,
    message: String,
    bar: Option<ProgressBar>,
}

pub struct Activity {
    mode: UiMode,
    shared: Arc<(Mutex<Shared>, Condvar)>,
    worker: Option<thread::JoinHandle<()>>,
    finished: bool,
}

impl Activity {
    fn begin(mode: UiMode, message: &str) -> Self {
        let shared = Arc::new((
            Mutex::new(Shared {
                done: !mode.animated,
                message: message.to_string(),
                bar: None,
            }),
            Condvar::new(),
        ));
        let worker = if mode.animated {
            let thread_shared = Arc::clone(&shared);
            Some(thread::spawn(move || {
                let (lock, cv) = &*thread_shared;
                let guard = lock.lock().expect("activity lock");
                let (mut guard, _) = cv
                    .wait_timeout_while(guard, SPINNER_DELAY, |shared| !shared.done)
                    .expect("activity wait");
                if guard.done {
                    return;
                }
                let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
                bar.set_style(spinner_style());
                bar.set_message(guard.message.clone());
                bar.enable_steady_tick(SPINNER_DELAY);
                guard.bar = Some(bar);
            }))
        } else {
            None
        };
        Self {
            mode,
            shared,
            worker,
            finished: false,
        }
    }

    pub fn set_message(&self, message: &str) {
        if !self.mode.animated {
            return;
        }
        let (lock, _) = &*self.shared;
        let mut guard = lock.lock().expect("activity lock");
        guard.message = message.to_string();
        if let Some(bar) = &guard.bar {
            bar.set_message(message.to_string());
        }
    }

    pub fn finish(mut self, tone: Tone, message: &str) {
        self.stop_worker();
        self.clear_bar();
        if tone != Tone::Failed {
            write_result(self.mode, tone, message);
        }
        self.finished = true;
    }

    fn stop_worker(&mut self) {
        let worker = self.worker.take();
        {
            let (lock, cv) = &*self.shared;
            let mut guard = lock.lock().expect("activity lock");
            guard.done = true;
            cv.notify_all();
        }
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }

    fn clear_bar(&self) {
        if !self.mode.animated {
            return;
        }
        let (lock, _) = &*self.shared;
        let mut guard = lock.lock().expect("activity lock");
        if let Some(bar) = guard.bar.take() {
            bar.finish_and_clear();
        }
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.stop_worker();
        self.clear_bar();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn decide_requires_a_terminal_and_a_real_term() {
        assert_eq!(
            decide(false, true, Some("xterm"), None),
            UiMode {
                animated: true,
                color: true
            }
        );
        assert_eq!(
            decide(false, true, Some("xterm"), Some("")),
            UiMode {
                animated: true,
                color: true
            }
        );
        assert_eq!(
            decide(false, true, Some("xterm"), Some("1")),
            UiMode {
                animated: true,
                color: false
            }
        );
        assert_eq!(
            decide(true, true, Some("xterm"), None),
            UiMode {
                animated: false,
                color: false
            }
        );
        assert_eq!(
            decide(false, false, Some("xterm"), None),
            UiMode {
                animated: false,
                color: false
            }
        );
        assert_eq!(
            decide(false, true, Some("dumb"), None),
            UiMode {
                animated: false,
                color: false
            }
        );
        assert_eq!(
            decide(false, true, None, None),
            UiMode {
                animated: true,
                color: true
            }
        );
    }

    #[test]
    fn format_result_line_uses_glyphs_and_optional_color() {
        assert_eq!(
            format_result_line(Tone::Done, "Installed Go 1.23.4", false),
            "✔ Installed Go 1.23.4"
        );
        assert_eq!(
            format_result_line(Tone::Notice, "Go 1.23.4 is already installed\n", false),
            "• Go 1.23.4 is already installed"
        );
        assert_eq!(format_result_line(Tone::Failed, "missing", false), "✖ missing");
        assert_eq!(
            format_result_line(Tone::Done, "ok", true),
            "\u{1b}[32m✔\u{1b}[0m ok"
        );
        assert_eq!(
            format_result_line(Tone::Failed, "bad", true),
            "\u{1b}[31m✖\u{1b}[0m bad"
        );
        assert_eq!(format_result_line(Tone::Notice, "skip", true), "• skip");
        assert_eq!(format_result(Tone::Done, "ok", false), "✔ ok\n");
    }

    #[test]
    fn finish_before_spinner_returns_immediately() {
        let started = Instant::now();
        let activity = Activity::begin(
            UiMode {
                animated: true,
                color: false,
            },
            "working",
        );
        activity.finish(Tone::Failed, "unused");
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "early finish took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn plain_sentence_keeps_one_trailing_newline() {
        assert_eq!(plain_sentence("Installed Go 1.23.4"), "Installed Go 1.23.4\n");
        assert_eq!(
            plain_sentence("Removed 1 cached archive\n"),
            "Removed 1 cached archive\n"
        );
    }

    #[test]
    fn error_prefix_follows_animation() {
        let animated = decide(false, true, Some("xterm"), None);
        let quiet = decide(true, true, Some("xterm"), None);
        assert!(animated.animated);
        assert!(!quiet.animated);
        assert_eq!(
            format_result(Tone::Failed, "Go 1.2.3 is not installed. Run gv install 1.2.3", false),
            "✖ Go 1.2.3 is not installed. Run gv install 1.2.3\n"
        );
    }
}
