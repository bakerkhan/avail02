use dusk_plonk::fft::EvaluationDomain;
use dusk_plonk::prelude::BlsScalar;
use std::convert::TryInto;

// This module is taken from https://gist.github.com/itzmeanjan/4acf9338d9233e79cfbee5d311e7a0b4
// which I wrote few months back when exploring polynomial based erasure coding technique !

pub fn reconstruct_poly(
    // domain I'm working with
    // all (i)ffts to be performed on it
    eval_domain: EvaluationDomain,
    // subset of available data
    subset: Vec<Option<BlsScalar>>,
) -> Result<Vec<BlsScalar>, String> {
    let mut missing_indices = Vec::new();
    for i in 0..subset.len() {
        if let None = subset[i] {
            missing_indices.push(i as u64);
        }
    }
    let (mut zero_poly, zero_eval) =
        zero_poly_fn(eval_domain, &missing_indices[..], subset.len() as u64);
    for i in 0..subset.len() {
        if let None = subset[i] {
            if !(zero_eval[i] == BlsScalar::zero()) {
                return Err("bad zero poly evaluation !".to_owned());
            }
        }
    }
    let mut poly_evals_with_zero: Vec<BlsScalar> = Vec::new();
    for i in 0..subset.len() {
        if let Some(v) = subset[i] {
            poly_evals_with_zero.push(v * zero_eval[i]);
        } else {
            poly_evals_with_zero.push(BlsScalar::zero());
        }
    }
    let mut poly_with_zero = eval_domain.ifft(&poly_evals_with_zero[..]);
    shift_poly(&mut poly_with_zero[..]);
    shift_poly(&mut zero_poly[..]);
    let mut eval_shifted_poly_with_zero = eval_domain.fft(&poly_with_zero[..]);
    let eval_shifted_zero_poly = eval_domain.fft(&zero_poly[..]);
    for i in 0..eval_shifted_poly_with_zero.len() {
        eval_shifted_poly_with_zero[i] *= eval_shifted_zero_poly[i].invert().unwrap();
    }

    let mut shifted_reconstructed_poly = eval_domain.ifft(&eval_shifted_poly_with_zero[..]);
    unshift_poly(&mut shifted_reconstructed_poly[..]);

    let reconstructed_data = eval_domain.fft(&shifted_reconstructed_poly[..]);
    Ok(reconstructed_data)
}

fn expand_root_of_unity(eval_domain: EvaluationDomain) -> Vec<BlsScalar> {
    let root_of_unity = eval_domain.group_gen;
    let mut roots: Vec<BlsScalar> = Vec::new();
    roots.push(BlsScalar::one());
    roots.push(root_of_unity);
    let mut i = 1;
    while roots[i] != BlsScalar::one() {
        roots.push(roots[i] * root_of_unity);
        i += 1;
    }
    return roots;
}

fn zero_poly_fn(
    eval_domain: EvaluationDomain,
    missing_indices: &[u64],
    length: u64,
) -> (Vec<BlsScalar>, Vec<BlsScalar>) {
    let expanded_r_o_u = expand_root_of_unity(eval_domain);
    let domain_stride = eval_domain.size() as u64 / length;
    let mut zero_poly: Vec<BlsScalar> = Vec::with_capacity(length as usize);
    let mut sub: BlsScalar;
    for i in 0..missing_indices.len() {
        let v = missing_indices[i as usize];
        sub = BlsScalar::zero() - expanded_r_o_u[(v * domain_stride) as usize];
        zero_poly.push(sub);
        if i > 0 {
            zero_poly[i] = zero_poly[i] + zero_poly[i - 1];
            for j in (1..i).rev() {
                zero_poly[j] = zero_poly[j] * sub;
                zero_poly[j] = zero_poly[j] + zero_poly[j - 1];
            }
            zero_poly[0] = zero_poly[0] * sub
        }
    }
    zero_poly.push(BlsScalar::one());
    for _ in zero_poly.len()..zero_poly.capacity() {
        zero_poly.push(BlsScalar::zero());
    }
    let zero_eval = eval_domain.fft(&zero_poly[..]);
    (zero_poly, zero_eval)
}

// in-place shifting
fn shift_poly(poly: &mut [BlsScalar]) {
    // primitive root of unity
    let shift_factor = BlsScalar::from(5);
    let mut factor_power = BlsScalar::one();
    // hoping it won't panic, though it should be handled properly
    //
    // this is actually 1/ shift_factor --- multiplicative inverse
    let inv_factor = shift_factor.invert().unwrap();

    for i in 0..poly.len() {
        poly[i] *= factor_power;
        factor_power *= inv_factor;
    }
}

// in-place unshifting
fn unshift_poly(poly: &mut [BlsScalar]) {
    // primitive root of unity
    let shift_factor = BlsScalar::from(5);
    let mut factor_power = BlsScalar::one();

    for i in 0..poly.len() {
        poly[i] *= factor_power;
        factor_power *= shift_factor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn data_reconstruction_success() {
        let domain_size = 1usize << 4;
        let eval_domain = EvaluationDomain::new(domain_size * 2).unwrap();

        // some dummy source data I care about
        let mut src: Vec<BlsScalar> = Vec::with_capacity(domain_size * 2);
        for i in 0..domain_size {
            src.push(BlsScalar::from(1 << (i + 1)));
        }
        // fill extended portion of vector with zeros
        for _ in domain_size..(2 * domain_size) {
            src.push(BlsScalar::zero());
        }

        // erasure code it
        let coded_src = eval_domain.fft(&src);
        // choose random subset of it ( >= 50% )
        let (coded_src_subset, _) = random_subset(&coded_src);
        // reconstruct 100% erasure coded values from random coded subset
        let coded_recovered = reconstruct_poly(eval_domain, coded_src_subset).unwrap();

        for i in 0..(2 * domain_size) {
            assert_eq!(coded_src[i], coded_recovered[i]);
        }

        let dst = eval_domain.ifft(&coded_recovered);

        for i in 0..(2 * domain_size) {
            assert_eq!(src[i].to_bytes(), dst[i].to_bytes());
        }
    }

    #[test]
    fn data_reconstruction_failure_0() {
        let domain_size = 1usize << 4;
        let eval_domain = EvaluationDomain::new(domain_size * 2).unwrap();

        let mut src: Vec<BlsScalar> = Vec::with_capacity(domain_size * 2);
        for i in 0..domain_size {
            src.push(BlsScalar::from(1 << (i + 1)));
        }
        for _ in domain_size..(2 * domain_size) {
            src.push(BlsScalar::zero());
        }

        let coded_src = eval_domain.fft(&src);
        let (mut coded_src_subset, available) = random_subset(&coded_src);
        // intentionally drop a few coded elements such that
        // < 50% is available
        drop_few(&mut coded_src_subset, available);
        // attempt to reconstruct 100% coded data from <50 % coded data
        // I've available
        let coded_recovered = reconstruct_poly(eval_domain, coded_src_subset).unwrap();

        let mut mismatch_count = 0;
        for i in 0..(2 * domain_size) {
            if coded_src[i] != coded_recovered[i] {
                mismatch_count += 1;
            }
        }

        assert!(mismatch_count > 0);
    }

    // Context behind following three test cases, where one failure condition
    // along with one possible solution, is demonstrated
    //
    // Need for writing these test cases originates in a conversation
    // with Prabal<https://github.com/prabal-banerjee> where we were discussing
    // how to ensure input byte chunks to dusk-plonk's `BlsScalar::from_bytes_wide()`
    // is always lesser than prime field modulus ( 255 bits wide ), because in our case
    // we'll get data bytes from arbitrary sources which will be converted into
    // (multiple) field elements, by splitting them into smaller chunks, each of size 48 bytes.
    //
    // Now imagine we got a 48-bytes wide chunk with content like [0xff; 48]
    // When that's attempted to be converted into field element it should be wrapped
    // around and original value will be lost
    //
    // We want to specify a way for `how a large byte string is splitted into field elements
    // such that no values are required to be wrapped around i.e. all values must be lesser
    // than 255-bit prime ?`
    //
    // One way to go about solving this problem is grouping large byte array into 254-bits
    // chunks, then we should not encounter that problem as value is always lesser than
    // prime number which is 255 -bits
    //
    // We'd like to spend some more time on this before we finalise anything
    // i.e. input parsing ( considering input endianness ),
    // bit width of chunks before attempt of conversion to prime field element etc.
    #[test]
    #[should_panic]
    fn data_reconstruction_failure_1() {
        let domain_size = 1usize << 4;
        let eval_domain = EvaluationDomain::new(domain_size * 2).unwrap();

        let input = [0xffu8; 32];

        let input_wide: [u8; 64] = {
            let mut v = vec![];

            v.extend_from_slice(&input.to_vec()[..]);
            v.extend_from_slice(&[0u8; 32].to_vec()[..]);

            v.try_into().unwrap()
        };

        let mut src: Vec<BlsScalar> = Vec::with_capacity(domain_size * 2);
        for _ in 0..domain_size {
            src.push(BlsScalar::from_bytes_wide(&input_wide));
        }
        for _ in domain_size..(2 * domain_size) {
            src.push(BlsScalar::zero());
        }

        // erasure code it
        let coded_src = eval_domain.fft(&src);
        // choose random subset of it ( >= 50% )
        let (coded_src_subset, _) = random_subset(&coded_src);
        // reconstruct 100% erasure coded values from random coded subset
        let coded_recovered = reconstruct_poly(eval_domain, coded_src_subset).unwrap();

        for i in 0..(2 * domain_size) {
            assert_eq!(coded_src[i], coded_recovered[i]);
        }

        let dst = eval_domain.ifft(&coded_recovered);

        for i in 0..domain_size {
            assert_eq!(input, dst[i].to_bytes(), "{}", format!("at i = {}", i));
        }
        // this is redundant here, test should fail in above for-loop, still I'm keeping it here
        // for sake of completeness
        for i in domain_size..(2 * domain_size) {
            assert_eq!([0u8; 32], dst[i].to_bytes(), "{}", format!("at i = {}", i));
        }
    }

    #[test]
    fn data_reconstruction_failure_2() {
        let domain_size = 1usize << 4;
        let eval_domain = EvaluationDomain::new(domain_size * 2).unwrap();

        // here I modify little endian input byte array such that
        // value represented in lesser that 255-bit prime field element,
        // we're working with on BLS12-381 curve
        //
        // and that's the reason why this test case passes !
        //
        // this test case is written to show that we need to make sure
        // we define a proper way for parsing elements from large input byte array
        // in smaller chunks with endianess consideration
        //
        // and following demonstrated way can be a way, where we group consequtive 254-bits
        // making sure it never goes over MOD for this prime field
        //
        // As I can think of now, that will require us to index inside bytes (for last 6 -bits)
        let mut input = [0xffu8; 32];
        input[31] &= 0b0011_1111; // check this line, which is modifying little endian input to bring it below MOD of prime field

        let input_wide: [u8; 64] = {
            let mut v = vec![];

            v.extend_from_slice(&input.to_vec()[..]);
            v.extend_from_slice(&[0u8; 32].to_vec()[..]);

            v.try_into().unwrap()
        };

        let mut src: Vec<BlsScalar> = Vec::with_capacity(domain_size * 2);
        for _ in 0..domain_size {
            src.push(BlsScalar::from_bytes_wide(&input_wide));
        }
        for _ in domain_size..(2 * domain_size) {
            src.push(BlsScalar::zero());
        }

        // erasure code it
        let coded_src = eval_domain.fft(&src);
        // choose random subset of it ( >= 50% )
        let (coded_src_subset, _) = random_subset(&coded_src);
        // reconstruct 100% erasure coded values from random coded subset
        let coded_recovered = reconstruct_poly(eval_domain, coded_src_subset).unwrap();

        for i in 0..(2 * domain_size) {
            assert_eq!(coded_src[i], coded_recovered[i]);
        }

        let dst = eval_domain.ifft(&coded_recovered);

        for i in 0..domain_size {
            assert_eq!(input, dst[i].to_bytes(), "{}", format!("at i = {}", i));
        }
        for i in domain_size..(2 * domain_size) {
            assert_eq!([0u8; 32], dst[i].to_bytes(), "{}", format!("at i = {}", i));
        }
    }

    #[test]
    #[should_panic]
    fn data_reconstruction_failure_3() {
        let domain_size = 1usize << 4;
        let eval_domain = EvaluationDomain::new(domain_size * 2).unwrap();

        // here also I demonstrate that if input byte array is parsed such that each
        // consequtive 255 -bits are taken together, still there's a possibility that
        // it'll produce a number which will be greater than prine field modulus, which
        // is also 255 -bits, but not necessarily highest representable number using 255 -bits
        //
        // so this test case must fail
        let mut input = [0xffu8; 32];
        input[31] &= 0b0111_1111;

        let input_wide: [u8; 64] = {
            let mut v = vec![];

            v.extend_from_slice(&input.to_vec()[..]);
            v.extend_from_slice(&[0u8; 32].to_vec()[..]);

            v.try_into().unwrap()
        };

        let mut src: Vec<BlsScalar> = Vec::with_capacity(domain_size * 2);
        for _ in 0..domain_size {
            src.push(BlsScalar::from_bytes_wide(&input_wide));
        }
        // fill extended portion of vector with zeros
        for _ in domain_size..(2 * domain_size) {
            src.push(BlsScalar::zero());
        }

        // erasure code it
        let coded_src = eval_domain.fft(&src);
        // choose random subset of it ( >= 50% )
        let (coded_src_subset, _) = random_subset(&coded_src);
        // reconstruct 100% erasure coded values from random coded subset
        let coded_recovered = reconstruct_poly(eval_domain, coded_src_subset).unwrap();

        for i in 0..(2 * domain_size) {
            assert_eq!(coded_src[i], coded_recovered[i]);
        }

        let dst = eval_domain.ifft(&coded_recovered);

        for i in 0..domain_size {
            assert_eq!(input, dst[i].to_bytes(), "{}", format!("at i = {}", i));
        }
        // this is redundant here, test should fail in above for-loop, still I'm keeping it here
        // for sake of completeness
        for i in domain_size..(2 * domain_size) {
            assert_eq!([0u8; 32], dst[i].to_bytes(), "{}", format!("at i = {}", i));
        }
    }

    fn drop_few(data: &mut [Option<BlsScalar>], mut available: usize) {
        assert!(available <= data.len());

        let mut idx = 0;
        while available >= data.len() / 2 {
            if let Some(_) = data[idx] {
                data[idx] = None;
                available -= 1;
            }
            idx += 1;
        }
    }

    // select a random subset of coded data to be used for
    // reconstruction purpose
    //
    // @note this is just a helper function for writing test case
    fn random_subset(data: &[BlsScalar]) -> (Vec<Option<BlsScalar>>, usize) {
        let mut rng = StdRng::seed_from_u64(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        let mut subset: Vec<Option<BlsScalar>> = Vec::with_capacity(data.len());
        let mut available = 0;
        for i in 0..data.len() {
            if rng.gen::<u8>() % 2 == 0 {
                subset.push(Some(data[i]));
                available += 1;
            } else {
                subset.push(None);
            }
        }

        // already we've >=50% data available
        // so just return & attempt to reconstruct back
        if available >= data.len() / 2 {
            (subset, available)
        } else {
            for i in 0..data.len() {
                if let None = subset[i] {
                    // enough data added, >=50% needs
                    // to be present
                    if available >= data.len() / 2 {
                        break;
                    }

                    subset[i] = Some(data[i]);
                    available += 1;
                }
            }
            (subset, available)
        }
    }
}
