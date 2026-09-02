pub fn part() {
    regexish::top_fn();         // fully qualified external (rule R9)
    crate::query::run();        // crate:: (rule R1)
}

pub fn uses_fn_body_use() {
    use regexish::Regex;        // fn-body overlay scope (rule S4)
    Regex::new();               // -> regexish::Regex::new, only via the overlay
}
