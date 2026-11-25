pub fn fmt_size(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", b)
    } else {
        format!("{:.1} {}", v, U[i])
    }
}

pub fn fmt_eta(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 || secs > 359_999.0 {
        return "--:--".into();
    }
    let s = secs as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub fn date_stamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}{:02}{:02}", y, m, d)
}

pub fn parse_drive_map(s: &str) -> std::collections::HashMap<char, char> {
    let mut map = std::collections::HashMap::new();
    for part in s.split(|c| c == ',' || c == ';' || c == ' ').filter(|p| !p.is_empty()) {
        let mut it = part.split('=');
        if let (Some(a), Some(b)) = (it.next(), it.next()) {
            if let (Some(a), Some(b)) = (a.trim().chars().next(), b.trim().chars().next()) {
                if a.is_ascii_alphabetic() && b.is_ascii_alphabetic() {
                    map.insert(a.to_ascii_uppercase(), b.to_ascii_uppercase());
                }
            }
        }
    }
    map
}

pub fn split_rules(s: &str) -> Vec<String> {
    s.split(|c| c == ',' || c == '\n' || c == ';').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}
