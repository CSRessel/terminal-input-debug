use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    Dark,
    Light,
    Unknown,
}

impl Default for Preference {
    fn default() -> Self {
        Preference::Unknown
    }
}

/// Attempt to detect whether the current terminal prefers a light or dark palette.
pub fn detect_terminal_color_preference() -> Preference {
    if let Some(pref) = detect_from_background_env() {
        return pref;
    }

    if let Some(pref) = detect_from_colorfgbg() {
        return pref;
    }

    Preference::Unknown
}

fn detect_from_background_env() -> Option<Preference> {
    let value = env::var("BACKGROUND").ok()?;
    match value.to_ascii_lowercase().as_str() {
        "light" => Some(Preference::Light),
        "dark" => Some(Preference::Dark),
        _ => None,
    }
}

fn detect_from_colorfgbg() -> Option<Preference> {
    let value = env::var("COLORFGBG").ok()?;
    let mut parts = value.split(';');
    let bg = parts.next_back()?;
    let bg_index = bg.parse::<u8>().ok()?;
    // Bright colors generally map to lighter backgrounds.
    if matches!(bg_index, 7 | 15 | 231) {
        Some(Preference::Light)
    } else if bg_index <= 6 || (8..=16).contains(&bg_index) {
        Some(Preference::Dark)
    } else {
        None
    }
}

