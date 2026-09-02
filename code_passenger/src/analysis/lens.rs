pub struct Lens<S, A> {
    pub get: fn(&S) -> &A,
    pub get_mut: fn(&mut S) -> &mut A,
}

impl<S, A> Lens<S, A> {
    pub fn over(&self, s: &mut S, f: impl FnOnce(&mut A)) {
        f((self.get_mut)(s));
    }
}
