use std::ops::{BitXor, BitXorAssign};
use roaring::bitmap::RoaringBitmap;

fn main() {
    // let a: RoaringBitmap = RoaringBitmap::full();
    // let b: RoaringBitmap = RoaringBitmap::full();
    let a: RoaringBitmap = RoaringBitmap::from_iter([1,2,3]);
    let b: RoaringBitmap = RoaringBitmap::from_iter([5,32432,432]);
    for _ in 0..50 {
        let mut c = a.clone();
        BitXorAssign::bitxor_assign(&mut c, b.clone());
    }
}
