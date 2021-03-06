/*
 * Copyright (c) 2018 Adgear
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy of
 * this software and associated documentation files (the "Software"), to deal in
 * the Software without restriction, including without limitation the rights to
 * use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
 * the Software, and to permit persons to whom the Software is furnished to do so,
 * subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
 * FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
 * COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
 * IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
 * CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */

//! Random number generator based of the PCG paper (http://www.pcg-random.org/paper.html).

use std::u32;
use std::time::*;

pub struct Rng {
    state: u64,
    inc: u64,
}

impl Default for Rng {
    fn default() -> Self {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(res) => Self::seed_with(res.as_secs() + u64::from(res.subsec_nanos())),
            Err(_) => Self::seed_with(6_364_136_223_846_793_005)
        }
    }
}

impl Rng {
    fn pcg32(&mut self) -> u32 {
        let oldstate = self.state;
        // Advance internal state
        self.state = u64::wrapping_add(u64::wrapping_mul(oldstate, 6_364_136_223_846_793_005u64), self.inc | 1);
        // Calculate output function (XSH RR), uses old state for max ILP
        let xorshifted = (((oldstate >> 18) ^ oldstate) >> 27) & 0xFFFF_FFFF;
        let rot = (oldstate >> 59) & 0xFFFF_FFFF;
        let v = (xorshifted >> rot) | (xorshifted << (u64::wrapping_sub(0, rot) & 31));
        v as u32
    }

    /// Creates a new pseudo-random with a custom seed.
    pub fn seed_with(seed: u64) -> Self {
        // We xor the seed with a randomly chosen number to avoid ending up with
        // a 0 state which would be bad.
        Self {
            state: seed ^ 0xedef_335f_00e1_70b3,
            inc: 12345,
        }
    }

    /// Creates a new pseudo-random number generator with default seed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Generates an integer.
    pub fn gen_int(&mut self) -> u32 {
        self.pcg32()
    }

    /// Generates an integer between `min` (included) and `max` (excluded), i.e. [min, max).
    pub fn gen_int_interval(&mut self, min: u32, max: u32) -> u32 {
        (self.pcg32() % (max - min)) + min
    }

    /// Generates a floating-point number between 0.0 and 1.0, both included.
    pub fn gen_double_interval_unit(&mut self) -> f64 {
        let max = f64::from(u32::MAX);
        let n = f64::from(self.gen_int());
        n / max
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::Rng;

    #[test]
    fn avg_median() {
        let mut rng = Rng::new();
        let mut numbers = vec![];
        for _ in 0..1_000_000 {
            numbers.push(rng.gen_int());
        }
        let (min, max) = {
            (numbers.iter().cloned().min().expect("minimum"), numbers.iter().cloned().max().expect("maximum"))
        };
        let median = f64::from((max - min) / 2);
        let len = numbers.len();
        let avg = numbers.into_iter().map(u64::from).sum::<u64>() / len as u64;
        let ratio = median / avg as f64;
        // Check that the ratio of the median over the average is close to one.
        assert!((ratio - 1.0).abs() < 0.01);
    }

    fn distribution_with_capacity(capacity: usize) {
        let mut rng = Rng::new();
        let mut values = vec![0; capacity];
        let end = capacity * 25;
        for _ in 0..end {
            let index = rng.gen_int() as usize % values.len();
            values[index] += 1;
        }

        let mut occurences = BTreeMap::<u32, u64>::new();
        for &val in &values {
            *occurences.entry(val).or_insert(0) += 1;
        }

        let min = *occurences.iter().next().expect("first element").0;
        let max = *occurences.iter().next_back().expect("last element").0;

        // There's not much difference between the generation occurences of the different numbers.
        assert!(max - min < 100);
        assert!(occurences.len() < 100);
        // We generated at least once every numbers in the range.
        assert!(!values.iter().any(|&v| v == 0));
    }

    #[test]
    fn distribution_small() {
        distribution_with_capacity(400_000);
    }

    #[test]
    #[ignore]
    fn distribution_big() {
        distribution_with_capacity(4_000_000);
    }
}
