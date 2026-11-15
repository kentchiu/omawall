use std::path::Path;

/// Colors accepted by the Wallhaven `colors` filter.
const PALETTE: [&str; 29] = [
    "660000", "990000", "cc0000", "cc3333", "ea4c88", "993399", "663399", "333399", "0066cc",
    "0099cc", "66cccc", "77cc33", "669900", "336600", "666600", "999900", "cccc33", "ffff00",
    "ffcc33", "ff9900", "ff6600", "cc6633", "996633", "663300", "000000", "999999", "cccccc",
    "ffffff", "424153",
];

fn parse_rgb(hex: &str) -> Option<[i32; 3]> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| i32::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// Nearest palette entry by squared RGB distance.
pub fn nearest(hex: &str) -> Option<&'static str> {
    let rgb = parse_rgb(hex)?;
    PALETTE.iter().copied().min_by_key(|candidate| {
        let c = parse_rgb(candidate).expect("palette entries are valid");
        (0..3).map(|i| (rgb[i] - c[i]).pow(2)).sum::<i32>()
    })
}

/// Palette color for the current Omarchy theme accent; `None` when unreadable.
pub fn theme_color(colors_toml: &Path) -> Option<&'static str> {
    let text = std::fs::read_to_string(colors_toml).ok()?;
    let table: toml::Table = text.parse().ok()?;
    nearest(table.get("accent")?.as_str()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_matches_contract_example() {
        assert_eq!(nearest("#89b4fa"), Some("66cccc"));
        assert_eq!(nearest("#000001"), Some("000000"));
        assert_eq!(nearest("#zzzzzz"), None);
        assert_eq!(nearest("#fff"), None);
    }

    #[test]
    fn theme_color_reads_accent() {
        let dir = crate::testutil::temp_dir("color");
        let file = dir.join("colors.toml");
        std::fs::write(&file, "accent = \"#89b4fa\"\nforeground = \"#ffffff\"\n").unwrap();
        assert_eq!(theme_color(&file), Some("66cccc"));
        std::fs::write(&file, "accent = 5").unwrap();
        assert_eq!(theme_color(&file), None);
        assert_eq!(theme_color(&dir.join("missing.toml")), None);
    }
}
