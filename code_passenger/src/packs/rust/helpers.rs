



pub(super) fn to_type_name(snake: &str) -> String {
    let mut out = String::new();
    let mut up = true;
    for c in snake.chars() {
        if c == '_' { up = true; continue; }
        if up { out.extend(c.to_uppercase()); up = false; }
        else { out.push(c); }
    }
    if out.is_empty() { "Thing".into() } else { out }
}

pub(super) fn norm_ident(s: &str) -> String {
    s.replace('-', "_")
}
