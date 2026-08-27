/// Rust keywords that can be used as raw identifiers (`r#...`).
const RAW_OK: [&str; 47] = [
    "as", "break", "const", "continue", "else", "enum", "false", "fn", "for", "if", "impl", "in", "let", "loop",
    "match", "mod", "move", "mut", "pub", "ref", "return", "static", "struct", "trait", "true", "type", "unsafe",
    "use", "where", "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro", "override",
    "priv", "typeof", "unsized", "virtual", "yield", "try", "gen",
];

/// Keywords that cannot be raw identifiers.
const RAW_FORBIDDEN: [&str; 5] = ["self", "Self", "super", "crate", "extern"];

/// Convert an arbitrary segment to snake_case.
pub fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() {
                let prev_lower_or_digit = i > 0
                    && (chars[i - 1].is_ascii_lowercase()
                        || (chars[i - 1].is_ascii_digit() && c.is_ascii_alphabetic()));
                let next_lower = i + 1 < chars.len() && chars[i + 1].is_ascii_lowercase();
                if i > 0 && (prev_lower_or_digit || (chars[i - 1].is_ascii_uppercase() && next_lower)) {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    while out.starts_with('_') {
        out.remove(0);
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if out.is_empty() {
        out.push_str("anon");
    }
    out
}

/// Convert an arbitrary segment to UpperCamelCase.
pub fn to_upper_camel_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut upper_next = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if upper_next {
                out.extend(c.to_uppercase());
                upper_next = false;
            } else {
                out.push(c);
            }
        } else {
            upper_next = true;
        }
    }
    if out.is_empty() {
        out.push_str("Anon");
    }
    out
}

/// Sanitize a string into a valid Rust identifier, resolving keywords.
pub fn sanitize_ident(s: &str) -> String {
    let base = to_snake_case(s);
    if RAW_FORBIDDEN.contains(&base.as_str()) || RAW_FORBIDDEN.contains(&s) {
        format!("{base}_")
    } else if RAW_OK.contains(&base.as_str()) {
        format!("r#{base}")
    } else {
        base
    }
}

/// Sanitize a module name (no raw identifiers for modules in `use` paths we emit).
pub fn sanitize_mod_name(s: &str) -> String {
    let base = to_snake_case(s);
    if RAW_FORBIDDEN.contains(&base.as_str()) || RAW_OK.contains(&base.as_str()) {
        format!("{base}_")
    } else {
        base
    }
}

/// Sanitize a type name into UpperCamelCase and ensure it is not a keyword.
pub fn sanitize_type_name(s: &str) -> String {
    let mut base = to_upper_camel_case(s);
    if RAW_OK.contains(&base.as_str()) || RAW_FORBIDDEN.contains(&base.as_str()) {
        base.push('_');
    }
    base
}

/// Escape a string for use in a Rust string literal (`"..."`).
pub(crate) fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake() {
        assert_eq!(to_snake_case("deviceId"), "device_id");
        assert_eq!(to_snake_case("DeviceGroups"), "device_groups");
        assert_eq!(to_snake_case("device-groups"), "device_groups");
        assert_eq!(to_snake_case("X-Token"), "x_token");
        assert_eq!(to_snake_case("v1"), "v1");
        assert_eq!(to_snake_case("2fa"), "_2fa");
    }

    #[test]
    fn camel() {
        assert_eq!(to_upper_camel_case("device-groups"), "DeviceGroups");
        assert_eq!(to_upper_camel_case("device_groups"), "DeviceGroups");
    }

    #[test]
    fn keywords() {
        assert_eq!(sanitize_ident("type"), "r#type");
        assert_eq!(sanitize_ident("self"), "self_");
        assert_eq!(sanitize_mod_name("type"), "type_");
    }
}
