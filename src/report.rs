use crate::model::*;

pub fn findings_to_json(findings: &[Finding]) -> String {
    serde_json::to_string_pretty(findings).unwrap()
}

pub fn findings_to_markdown(findings: &[Finding]) -> String {
    let mut s = String::new();
    s.push_str("# Klepto Report\n\n");
    for f in findings {
        s.push_str(&format!(
            "- **{:?} {}**: {} (`{}`:{}:{})\n",
            f.severity,
            f.code,
            f.message,
            f.location.path.display(),
            f.location.line.unwrap_or(0),
            f.location.column.unwrap_or(0),
        ));
    }
    s
}

// simple ASCII table (no deps)
pub fn findings_to_table(findings: &[Finding]) -> String {
    let mut out = String::new();
    out.push_str("SEV  CODE    LOCATION                         MESSAGE\n");
    out.push_str("---- ------- -------------------------------  ------------------------------\n");
    for f in findings {
        let loc = format!(
            "{}:{}:{}",
            f.location.path.display(),
            f.location.line.unwrap_or(0),
            f.location.column.unwrap_or(0)
        );
        out.push_str(&format!(
            "{:<4} {:<7} {:<31}  {}\n",
            format!("{:?}", f.severity),
            f.code,
            truncate(&loc, 31),
            f.message
        ));
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { return s.to_string(); }
    let mut t = s[..max.saturating_sub(3)].to_string();
    t.push_str("...");
    t
}
