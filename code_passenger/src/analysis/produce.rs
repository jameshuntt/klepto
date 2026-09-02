pub fn produce<S: Clone>(base: &S, f: impl FnOnce(&mut S)) -> S {
    let mut draft = base.clone();
    f(&mut draft);
    draft
}