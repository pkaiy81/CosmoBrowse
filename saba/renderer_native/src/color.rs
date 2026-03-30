/// Parse CSS color strings into (r, g, b, a) tuples.

pub fn parse_css_color(s: &str) -> (u8, u8, u8, u8) {
    let s = s.trim();

    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }

    if let Some(inner) = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return parse_rgb_func(inner);
    }

    match s.to_ascii_lowercase().as_str() {
        "black" => (0, 0, 0, 255),
        "white" => (255, 255, 255, 255),
        "red" => (255, 0, 0, 255),
        "green" => (0, 128, 0, 255),
        "blue" => (0, 0, 255, 255),
        "yellow" => (255, 255, 0, 255),
        "cyan" | "aqua" => (0, 255, 255, 255),
        "magenta" | "fuchsia" => (255, 0, 255, 255),
        "gray" | "grey" => (128, 128, 128, 255),
        "silver" => (192, 192, 192, 255),
        "maroon" => (128, 0, 0, 255),
        "olive" => (128, 128, 0, 255),
        "purple" => (128, 0, 128, 255),
        "teal" => (0, 128, 128, 255),
        "navy" => (0, 0, 128, 255),
        "orange" => (255, 165, 0, 255),
        "transparent" => (0, 0, 0, 0),
        _ => (0, 0, 0, 255),
    }
}

fn parse_hex(hex: &str) -> (u8, u8, u8, u8) {
    match hex.len() {
        3 => {
            let r = hex_digit(hex.as_bytes()[0]) * 17;
            let g = hex_digit(hex.as_bytes()[1]) * 17;
            let b = hex_digit(hex.as_bytes()[2]) * 17;
            (r, g, b, 255)
        }
        4 => {
            let r = hex_digit(hex.as_bytes()[0]) * 17;
            let g = hex_digit(hex.as_bytes()[1]) * 17;
            let b = hex_digit(hex.as_bytes()[2]) * 17;
            let a = hex_digit(hex.as_bytes()[3]) * 17;
            (r, g, b, a)
        }
        6 => {
            let r = hex_byte(&hex[0..2]);
            let g = hex_byte(&hex[2..4]);
            let b = hex_byte(&hex[4..6]);
            (r, g, b, 255)
        }
        8 => {
            let r = hex_byte(&hex[0..2]);
            let g = hex_byte(&hex[2..4]);
            let b = hex_byte(&hex[4..6]);
            let a = hex_byte(&hex[6..8]);
            (r, g, b, a)
        }
        _ => (0, 0, 0, 255),
    }
}

fn hex_digit(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

fn hex_byte(s: &str) -> u8 {
    let bytes = s.as_bytes();
    hex_digit(bytes[0]) * 16 + hex_digit(bytes[1])
}

fn parse_rgb_func(inner: &str) -> (u8, u8, u8, u8) {
    let parts: Vec<&str> = inner.split([',', '/']).map(str::trim).collect();
    let parse_component = |s: &str| -> u8 {
        if let Some(pct) = s.strip_suffix('%') {
            let v: f64 = pct.trim().parse().unwrap_or(0.0);
            (v * 2.55).round().clamp(0.0, 255.0) as u8
        } else {
            s.trim().parse::<f64>().unwrap_or(0.0).round().clamp(0.0, 255.0) as u8
        }
    };

    let r = parts.first().map(|s| parse_component(s)).unwrap_or(0);
    let g = parts.get(1).map(|s| parse_component(s)).unwrap_or(0);
    let b = parts.get(2).map(|s| parse_component(s)).unwrap_or(0);
    let a = parts.get(3).map(|s| {
        let s = s.trim();
        if let Some(pct) = s.strip_suffix('%') {
            let v: f64 = pct.trim().parse().unwrap_or(100.0);
            (v * 2.55).round().clamp(0.0, 255.0) as u8
        } else {
            let v: f64 = s.parse().unwrap_or(1.0);
            (v * 255.0).round().clamp(0.0, 255.0) as u8
        }
    }).unwrap_or(255);
    (r, g, b, a)
}
