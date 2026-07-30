use std::io::IsTerminal;
use std::sync::OnceLock;

/// Colors are on only when stdout is a real terminal and the user hasn't
/// opted out via the standard `NO_COLOR` convention (https://no-color.org) —
/// so piping output to a file or another program never gets escape codes
/// mixed into it.
fn colors_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal())
}

fn wrap(code: &str, s: &str) -> String {
    if colors_enabled() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn dim(s: &str) -> String {
    wrap("2", s)
}
pub fn bold(s: &str) -> String {
    wrap("1", s)
}
pub fn cyan(s: &str) -> String {
    wrap("36", s)
}
pub fn green(s: &str) -> String {
    wrap("32", s)
}
pub fn red(s: &str) -> String {
    wrap("31", s)
}
pub fn yellow(s: &str) -> String {
    wrap("33", s)
}
