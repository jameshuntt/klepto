// S5 amendment fixture: generics and `Self` of impl blocks and types are
// resolution contexts outside fn bodies (impl headers, associated items).

pub trait Body {
    type Data;
    type Error;
}

pub struct Wrap<B>(pub B);

impl<B: Body> Wrap<B> {
    /// `B::Data` in a signature: `B` is the impl's generic parameter.
    pub fn data(&self) -> Option<B::Data> { None }
}

pub trait Framed {
    type Out;
    fn frame(&self) -> Self::Out;
}

impl<B: Body> Framed for Wrap<B> {
    /// `B::Error` in an associated type: still the impl's generic.
    type Out = Option<B::Error>;
    fn frame(&self) -> Self::Out { None }
}

/// A type's own generic in a field type.
pub struct Node<T> {
    pub next: Option<Box<Node<T>>>,
}
