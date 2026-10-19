pub const ENV_SOCKET: &str = "APP_SOCKET";

pub fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

pub fn read() -> String {
    let a = std::env::var("APP_KEY").unwrap_or_default();
    let b = env_or(ENV_SOCKET, "/run/app.sock");
    println!("{} {}", a, "APP_PRINTED");
    format!("{a}{b}")
}

/// "APP_IN_DOC" is a doc comment, not code.
pub fn names() -> [&'static str; 1] {
    ["APP_NAMED"]
}

pub enum Mode {
    Serving,
    ReadOnly,
    Maintenance,
}

pub fn label(m: &Mode) -> &'static str {
    match m {
        Mode::Serving => "serving",
        Mode::ReadOnly | Mode::Maintenance => "not serving",
    }
}

pub fn serving(m: &Mode) -> bool {
    match m {
        Mode::Serving => true,
        _ => false,
    }
}

#[deprecated(note = "use serving")]
pub fn old_serving() -> bool {
    false
}

pub struct Config {
    pub name: String,
}

impl Config {
    pub fn named(name: &str) -> Self {
        Config { name: name.to_string() }
    }
}

pub fn configs(c: &Config) -> Option<Config> {
    let Config { name } = c;
    Some(Config { name: name.clone() })
}
