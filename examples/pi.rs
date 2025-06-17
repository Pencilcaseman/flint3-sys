use core::f64;
use std::{io::Write, mem, ptr::null_mut, time::Instant};

use flint3_sys::*;

fn main() {
    println!("Hello, World!");

    let digits = 1_000_000;
    let bits = (digits as f64 * f64::consts::LOG2_10) as i64 + 64;

    unsafe {
        let mut pi: arb_struct = mem::MaybeUninit::uninit().assume_init();

        flint_set_num_threads(16);

        arb_init(&mut pi);

        let start = Instant::now();
        arb_const_pi(&mut pi, bits);
        println!("Computed {digits} digits in {:?}", start.elapsed());
        std::io::stdout().flush().unwrap();

        // arb_print(&pi);
        arb_printn(&pi, digits, 20 * 16);
        println!();
        std::io::stdout().flush().unwrap();

        arb_mul(&mut pi, &mut pi, &mut pi, bits);
        arb_printn(&pi, digits, 20 * 16);
        println!();
        std::io::stdout().flush().unwrap();

        arb_clear(&mut pi);
    }
}
