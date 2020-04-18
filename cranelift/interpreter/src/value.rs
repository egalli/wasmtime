// //! The values operated on by the interpreter.
// //!
// //! Each variant of `Value` may implement traits of `std::ops` for use in `interpreter`; e.g.,
// //! `Value::Int` has an implementation (a match arm) of `Add`.
//
// use cranelift_reader::DataValue;
// use std::ops::{Add, Sub};
//
// impl Add for DataValue {
//     type Output = Self;
//     fn add(self, rhs: Self) -> Self::Output {
//         use Value::*;
//         match (self, rhs) {
//             (Int(a), Int(b)) => Int(a + b),
//             _ => unimplemented!(),
//         }
//     }
// }
//
// impl Sub for DataValue {
//     type Output = Self;
//     fn sub(self, rhs: Self) -> Self::Output {
//         use Value::*;
//         match (self, rhs) {
//             (Int(a), Int(b)) => Int(a - b),
//             _ => unimplemented!(),
//         }
//     }
// }
//
// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn value_size() {
//         assert_eq!(core::mem::size_of::<DataValue>(), 16);
//     }
// }
