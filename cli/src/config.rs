use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::state;

pub const FAVS: &str = "favs";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub key: String,
    pub label: String,
    pub tag: u64,
}

#[derive(Deserialize)]
struct ConfigFile {
    sources: Vec<Source>,
}

const DEFAULT_SOURCES: [(&str, &str, u64); 10] = [
    ("landscape", "風景", 711),
    ("nature", "自然", 37),
    ("space", "太空", 32),
    ("city", "城市", 17),
    ("mountains", "山岳", 328),
    ("architecture", "建築", 989),
    ("minimalism", "極簡", 2278),
    ("abstract", "抽象", 74),
    ("digital-art", "數位藝術", 479),
    ("cars", "汽車", 314),
];

pub fn default_sources() -> Vec<Source> {
    DEFAULT_SOURCES
        .iter()
        .map(|&(key, label, tag)| Source {
            key: key.into(),
            label: label.into(),
            tag,
        })
        .collect()
}

/// Sources from `config.json`, or the built-in defaults when the file is missing.
pub fn load_sources(path: &Path) -> Result<Vec<Source>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(default_sources()),
        Err(e) => {
            return Err(AppError::io(
                format!("無法讀取設定檔 {}", path.display()),
                e,
            ));
        }
    };
    let file: ConfigFile = serde_json::from_str(&text)
        .map_err(|e| AppError::new("config", format!("設定檔格式錯誤：{e}")))?;
    validate_sources(&file.sources)?;
    Ok(file.sources)
}

fn is_valid_key(key: &str) -> bool {
    (1..=32).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub fn validate_sources(sources: &[Source]) -> Result<()> {
    for (i, source) in sources.iter().enumerate() {
        let fail =
            |why: &str| AppError::new("config", format!("來源「{}」無效：{why}", source.key));
        if !is_valid_key(&source.key) {
            return Err(fail("key 只能使用 a-z、0-9、-，長度 1–32"));
        }
        if source.key == FAVS {
            return Err(fail("favs 為保留字"));
        }
        if sources[..i].iter().any(|s| s.key == source.key) {
            return Err(fail("key 重複"));
        }
        if source.label.trim().is_empty() {
            return Err(fail("label 不可為空"));
        }
        if source.tag == 0 {
            return Err(fail("tag 必須是正整數"));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    None,
    Theme,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Styles {
    pub general: bool,
    pub anime: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timer {
    pub enabled: bool,
    pub minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub sources: Vec<String>,
    pub styles: Styles,
    pub color: ColorMode,
    pub timer: Timer,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            sources: DEFAULT_SOURCES.iter().map(|s| s.0.to_string()).collect(),
            styles: Styles {
                general: true,
                anime: false,
            },
            color: ColorMode::None,
            timer: Timer {
                enabled: false,
                minutes: 60,
            },
        }
    }
}

const MINUTES: std::ops::RangeInclusive<u32> = 5..=1440;

impl Settings {
    /// Reads `settings.json`; unknown or duplicate source keys are dropped.
    pub fn load(path: &Path, sources: &[Source]) -> Result<Self> {
        if !path.exists() {
            // First run selects every configured source, including a custom config's.
            return Ok(Self {
                sources: sources.iter().map(|s| s.key.clone()).collect(),
                ..Self::default()
            });
        }
        let mut settings: Settings = state::read_json(path)?;
        let mut kept: Vec<String> = Vec::new();
        for key in settings.sources.drain(..) {
            let known = key == FAVS || sources.iter().any(|s| s.key == key);
            if known && !kept.contains(&key) {
                kept.push(key);
            }
        }
        settings.sources = kept;
        if !(settings.styles.general || settings.styles.anime)
            || !MINUTES.contains(&settings.timer.minutes)
        {
            return Err(AppError::new(
                "io",
                format!("狀態檔內容無效：{}", path.display()),
            ));
        }
        Ok(settings)
    }

    /// Wallhaven `categories` value for the selected styles.
    pub fn categories(&self) -> &'static str {
        match (self.styles.general, self.styles.anime) {
            (true, true) => "110",
            (false, true) => "010",
            _ => "100",
        }
    }

    pub fn is_selected(&self, key: &str) -> bool {
        self.sources.iter().any(|s| s == key)
    }

    /// Applies `omawall set <name> <value>`; any invalid input is a usage error.
    pub fn set(&mut self, name: &str, value: &str, sources: &[Source]) -> Result<()> {
        match name {
            "sources" => {
                let mut keys: Vec<String> = Vec::new();
                for key in value.split(',').map(str::trim).filter(|k| !k.is_empty()) {
                    if key != FAVS && !sources.iter().any(|s| s.key == key) {
                        return Err(AppError::usage(format!("未知的來源：{key}")));
                    }
                    if !keys.iter().any(|k| k == key) {
                        keys.push(key.to_string());
                    }
                }
                self.sources = keys;
            }
            "style.general" | "style.anime" => {
                let on = parse_bool(value)?;
                let mut styles = self.styles.clone();
                if name == "style.general" {
                    styles.general = on;
                } else {
                    styles.anime = on;
                }
                if !(styles.general || styles.anime) {
                    return Err(AppError::usage("至少要保留一種風格"));
                }
                self.styles = styles;
            }
            "color" => {
                self.color = match value {
                    "none" => ColorMode::None,
                    "theme" => ColorMode::Theme,
                    _ => return Err(AppError::usage("color 只能是 none 或 theme")),
                };
            }
            "timer.enabled" => self.timer.enabled = parse_bool(value)?,
            "timer.minutes" => {
                let minutes = value
                    .parse::<u32>()
                    .ok()
                    .filter(|m| MINUTES.contains(m))
                    .ok_or_else(|| AppError::usage("timer.minutes 必須是 5–1440 的整數"))?;
                self.timer.minutes = minutes;
            }
            _ => return Err(AppError::usage(format!("未知的設定：{name}"))),
        }
        Ok(())
    }
}

fn parse_bool(value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(AppError::usage("值只能是 true 或 false")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(key: &str) -> Source {
        Source {
            key: key.into(),
            label: "x".into(),
            tag: 1,
        }
    }

    #[test]
    fn sources_validation() {
        assert!(validate_sources(&default_sources()).is_ok());
        for bad in [
            vec![source("favs")],
            vec![source("Bad")],
            vec![source("")],
            vec![source(&"a".repeat(33))],
            vec![source("a"), source("a")],
            vec![Source {
                tag: 0,
                ..source("a")
            }],
            vec![Source {
                label: " ".into(),
                ..source("a")
            }],
        ] {
            assert_eq!(validate_sources(&bad).unwrap_err().code, "config");
        }
    }

    #[test]
    fn load_sources_missing_and_invalid() {
        let dir = crate::testutil::temp_dir("config");
        let path = dir.join("config.json");
        assert_eq!(load_sources(&path).unwrap(), default_sources());
        std::fs::write(&path, r#"{"sources":[{"key":"a","label":"A","tag":3}]}"#).unwrap();
        assert_eq!(
            load_sources(&path).unwrap(),
            vec![Source {
                label: "A".into(),
                tag: 3,
                ..source("a")
            }]
        );
        std::fs::write(&path, "{").unwrap();
        assert_eq!(load_sources(&path).unwrap_err().code, "config");
    }

    #[test]
    fn settings_set_validation() {
        let sources = default_sources();
        let mut s = Settings::default();
        assert_eq!(
            s.set("timer.minutes", "3", &sources).unwrap_err().code,
            "usage"
        );
        assert_eq!(
            s.set("timer.minutes", "1441", &sources).unwrap_err().code,
            "usage"
        );
        assert_eq!(
            s.set("timer.minutes", "x", &sources).unwrap_err().code,
            "usage"
        );
        s.set("timer.minutes", "5", &sources).unwrap();
        s.set("timer.minutes", "1440", &sources).unwrap();
        assert_eq!(s.timer.minutes, 1440);

        assert_eq!(
            s.set("style.general", "false", &sources).unwrap_err().code,
            "usage"
        );
        s.set("style.anime", "true", &sources).unwrap();
        s.set("style.general", "false", &sources).unwrap();
        assert_eq!(s.categories(), "010");
        assert_eq!(
            s.set("style.anime", "yes", &sources).unwrap_err().code,
            "usage"
        );

        s.set("sources", "favs,city,city", &sources).unwrap();
        assert_eq!(s.sources, ["favs", "city"]);
        s.set("sources", "", &sources).unwrap();
        assert!(s.sources.is_empty());
        assert_eq!(
            s.set("sources", "nope", &sources).unwrap_err().code,
            "usage"
        );

        assert_eq!(s.set("color", "red", &sources).unwrap_err().code, "usage");
        s.set("color", "theme", &sources).unwrap();
        assert_eq!(s.color, ColorMode::Theme);
        assert_eq!(s.set("nope", "1", &sources).unwrap_err().code, "usage");
    }

    #[test]
    fn settings_load_drops_unknown_sources() {
        let dir = crate::testutil::temp_dir("settings");
        let path = dir.join("settings.json");
        assert_eq!(
            Settings::load(&path, &default_sources()).unwrap(),
            Settings::default()
        );
        // A custom config without settings.json starts with all of its own sources.
        let custom = [source("ocean"), source("forest")];
        assert_eq!(
            Settings::load(&path, &custom).unwrap().sources,
            ["ocean", "forest"]
        );
        std::fs::write(
            &path,
            r#"{"version":1,"sources":["city","gone","favs","city"]}"#,
        )
        .unwrap();
        let s = Settings::load(&path, &default_sources()).unwrap();
        assert_eq!(s.sources, ["city", "favs"]);
        assert_eq!(s.timer.minutes, 60);
        std::fs::write(&path, r#"{"timer":{"enabled":true,"minutes":1}}"#).unwrap();
        assert_eq!(
            Settings::load(&path, &default_sources()).unwrap_err().code,
            "io"
        );
    }
}
