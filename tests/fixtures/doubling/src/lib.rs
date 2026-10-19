//! Forty `let`s, each a method call on the one before with that one as its argument too:
//! a local's descriptor repeats the one it is bound from twice, 2^40 nodes uncapped.
pub struct S;
impl S {
    pub fn f(&self, _other: &S) -> S { S }
}

pub fn doubling() -> S {
    let a0 = S;
    let a1 = a0.f(&a0);
    let a2 = a1.f(&a1);
    let a3 = a2.f(&a2);
    let a4 = a3.f(&a3);
    let a5 = a4.f(&a4);
    let a6 = a5.f(&a5);
    let a7 = a6.f(&a6);
    let a8 = a7.f(&a7);
    let a9 = a8.f(&a8);
    let a10 = a9.f(&a9);
    let a11 = a10.f(&a10);
    let a12 = a11.f(&a11);
    let a13 = a12.f(&a12);
    let a14 = a13.f(&a13);
    let a15 = a14.f(&a14);
    let a16 = a15.f(&a15);
    let a17 = a16.f(&a16);
    let a18 = a17.f(&a17);
    let a19 = a18.f(&a18);
    let a20 = a19.f(&a19);
    let a21 = a20.f(&a20);
    let a22 = a21.f(&a21);
    let a23 = a22.f(&a22);
    let a24 = a23.f(&a23);
    let a25 = a24.f(&a24);
    let a26 = a25.f(&a25);
    let a27 = a26.f(&a26);
    let a28 = a27.f(&a27);
    let a29 = a28.f(&a28);
    let a30 = a29.f(&a29);
    let a31 = a30.f(&a30);
    let a32 = a31.f(&a31);
    let a33 = a32.f(&a32);
    let a34 = a33.f(&a33);
    let a35 = a34.f(&a34);
    let a36 = a35.f(&a35);
    let a37 = a36.f(&a36);
    let a38 = a37.f(&a37);
    let a39 = a38.f(&a38);
    let a40 = a39.f(&a39);
    a40
}
