//! Terminal presentation: colour, `--quiet` and `--verbose`.
//!
//! Human-readable output is built as plain text with a few *tone markers* (see [`tone`]). The markers are turned
//! into ANSI colour sequences, or removed, only when the text is printed ([`finish`]), after untrusted content
//! has been defanged. So colour can never be produced by file names, commit messages or other data, and a
//! pipe, `NO_COLOR` or `--color never` always yields plain text. JSON output is never coloured.

use std::io::IsTerminal;
use std::sync::OnceLock;

/// `--color` / the `color` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorMode {
    /// Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise.
    Auto,
    /// Always colour.
    Always,
    /// Never colour.
    Never,
}

impl ColorMode {
    /// Parses `auto`, `always` or `never`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    /// The setting's spelling.
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

/// The environment inputs of colour resolution (separate from the process for tests).
#[derive(Clone, Copy, Debug, Default)]
pub struct ColorEnv {
    /// `NO_COLOR` is set to a non-empty value.
    pub no_color: bool,
    /// `CLICOLOR_FORCE` is set to something other than `0`.
    pub force: bool,
    /// `CLICOLOR=0`.
    pub clicolor_off: bool,
    /// `TERM=dumb`.
    pub term_dumb: bool,
}

impl ColorEnv {
    /// Reads the real environment.
    pub fn from_process() -> Self {
        let set = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            no_color: set("NO_COLOR").is_some(),
            force: set("CLICOLOR_FORCE").is_some_and(|v| v != "0"),
            clicolor_off: set("CLICOLOR").is_some_and(|v| v == "0"),
            term_dumb: set("TERM").is_some_and(|v| v == "dumb"),
        }
    }
}

/// Decides whether a stream gets colour. Precedence: the `--color` flag, then the user's `color` setting, then
/// `NO_COLOR`, `CLICOLOR_FORCE`, `CLICOLOR=0` / `TERM=dumb`, and finally whether the stream is a terminal.
/// (The `NO_COLOR` convention lets user-level configuration and per-run flags override it.)
pub fn resolve_color(flag: Option<ColorMode>, config: Option<ColorMode>, env: &ColorEnv, is_tty: bool) -> bool {
    let auto = || {
        if env.no_color {
            false
        } else if env.force {
            true
        } else if env.clicolor_off || env.term_dumb {
            false
        } else {
            is_tty
        }
    };
    match flag.or(config.filter(|c| *c != ColorMode::Auto)) {
        Some(ColorMode::Always) => true,
        Some(ColorMode::Never) => false,
        _ => auto(),
    }
}

/// Process-wide presentation settings, fixed once at start-up.
#[derive(Clone, Copy, Debug, Default)]
pub struct Settings {
    /// Colour on standard output.
    pub color_out: bool,
    /// Colour on standard error.
    pub color_err: bool,
    /// `--quiet`: suppress confirmations and hints.
    pub quiet: bool,
    /// `--verbose`: print diagnostics to standard error.
    pub verbose: bool,
}

static SETTINGS: OnceLock<Settings> = OnceLock::new();

/// Fixes the settings for this process (the first call wins).
pub fn init(s: Settings) {
    let _ = SETTINGS.set(s);
}

/// The settings (plain, chatty defaults before [`init`]).
pub fn settings() -> Settings {
    SETTINGS.get().copied().unwrap_or_default()
}

/// Is this standard stream a terminal?
pub fn is_tty(err: bool) -> bool {
    if err { std::io::stderr().is_terminal() } else { std::io::stdout().is_terminal() }
}

/// What a piece of text means; mapped to a colour only at print time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Success.
    Ok,
    /// Needs attention but nothing failed.
    Warn,
    /// A failure or refusal.
    Bad,
    /// Secondary information.
    Dim,
    /// A heading.
    Strong,
    /// A suggestion for what to do next.
    Hint,
}

const OPEN: char = '\u{E000}';
const CLOSE: char = '\u{E001}';

impl Tone {
    fn code(self) -> char {
        match self {
            Tone::Ok => 'o',
            Tone::Warn => 'w',
            Tone::Bad => 'b',
            Tone::Dim => 'd',
            Tone::Strong => 's',
            Tone::Hint => 'h',
        }
    }

    fn sgr(code: char) -> &'static str {
        match code {
            'o' => "\x1b[32m",
            'w' => "\x1b[33m",
            'b' => "\x1b[31m",
            'd' => "\x1b[2m",
            's' => "\x1b[1m",
            'h' => "\x1b[36m",
            _ => "",
        }
    }
}

/// Marks `s` with a tone. The result is only meaningful to [`finish`].
pub fn tone(t: Tone, s: impl AsRef<str>) -> String {
    format!("{OPEN}{}{}{CLOSE}", t.code(), s.as_ref())
}

/// Turns tone markers into ANSI colour (`color`) or removes them. Must run after
/// [`crate::ctx::sanitize_for_terminal`]; markers that arrive in untrusted data can only select one of the fixed
/// styles above.
pub fn finish(text: &str, color: bool) -> String {
    if !text.contains(OPEN) && !text.contains(CLOSE) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 16);
    let mut chars = text.chars();
    let mut open = false;
    while let Some(c) = chars.next() {
        if c == OPEN {
            let code = chars.next().unwrap_or(' ');
            if color {
                out.push_str(Tone::sgr(code));
                open = true;
            }
        } else if c == CLOSE {
            if color && open {
                out.push_str("\x1b[0m");
                open = false;
            }
        } else {
            out.push(c);
        }
    }
    if open {
        out.push_str("\x1b[0m");
    }
    out
}

/// Writes a line to standard output; a closed pipe (`ag log | head`) ends the process quietly.
pub fn print_line(s: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    if let Err(e) = writeln!(out, "{s}") {
        broken_pipe_or_die(&e);
    }
}

/// Writes raw bytes to standard output (file contents), tolerating a closed pipe.
pub fn print_bytes(b: &[u8]) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    if let Err(e) = out.write_all(b).and_then(|()| out.flush()) {
        broken_pipe_or_die(&e);
    }
}

fn broken_pipe_or_die(e: &std::io::Error) -> ! {
    if e.kind() == std::io::ErrorKind::BrokenPipe {
        // The reader went away on purpose; this is not an error.
        std::process::exit(0);
    }
    eprintln!("error: cannot write to standard output: {e}");
    std::process::exit(1);
}

/// A line on standard error (never part of the data a command prints).
pub fn eprint_line(s: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr().lock(), "{}", finish(s, settings().color_err));
}

/// `--verbose` diagnostics: `[ag] ...` on standard error.
pub fn verbose(msg: impl AsRef<str>) {
    if settings().verbose {
        eprint_line(&format!("{} {}", tone(Tone::Dim, "[ag]"), msg.as_ref()));
    }
}

/// A suggestion for the next step, on standard error, unless `--quiet`.
pub fn hint(msg: impl AsRef<str>) {
    if !settings().quiet {
        eprint_line(&format!("{} {}", tone(Tone::Hint, "hint:"), msg.as_ref()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> ColorEnv {
        ColorEnv::default()
    }

    #[test]
    fn colour_follows_the_terminal_by_default() {
        assert!(resolve_color(None, None, &env(), true));
        assert!(!resolve_color(None, None, &env(), false), "never colour a pipe");
    }

    #[test]
    fn no_color_wins_over_the_terminal_but_not_over_a_flag_or_the_users_setting() {
        let e = ColorEnv { no_color: true, ..env() };
        assert!(!resolve_color(None, None, &e, true));
        assert!(!resolve_color(Some(ColorMode::Auto), None, &e, true));
        assert!(resolve_color(Some(ColorMode::Always), None, &e, false), "--color always is explicit");
        assert!(resolve_color(None, Some(ColorMode::Always), &e, false), "user-level setting overrides NO_COLOR");
        assert!(
            !resolve_color(Some(ColorMode::Never), Some(ColorMode::Always), &e, true),
            "the flag beats the setting"
        );
    }

    #[test]
    fn force_dumb_and_clicolor() {
        assert!(resolve_color(None, None, &ColorEnv { force: true, ..env() }, false));
        assert!(!resolve_color(None, None, &ColorEnv { term_dumb: true, ..env() }, true));
        assert!(!resolve_color(None, None, &ColorEnv { clicolor_off: true, ..env() }, true));
        assert!(
            !resolve_color(None, None, &ColorEnv { force: true, no_color: true, ..env() }, true),
            "NO_COLOR beats force"
        );
    }

    #[test]
    fn markers_become_ansi_or_vanish() {
        let t = format!("a {} b {}", tone(Tone::Ok, "ok"), tone(Tone::Bad, "bad"));
        assert_eq!(finish(&t, false), "a ok b bad");
        assert_eq!(finish(&t, true), "a \x1b[32mok\x1b[0m b \x1b[31mbad\x1b[0m");
        assert_eq!(finish("plain", true), "plain");
    }

    #[test]
    fn a_stray_marker_cannot_leave_the_terminal_coloured() {
        assert_eq!(finish("x\u{E000}b red", true), "x\x1b[31m red\x1b[0m");
        assert_eq!(finish("x\u{E001}", true), "x");
    }
}
