
use alloc::string::{String, ToString};
use sp_std::ops::Range;
use sp_std::convert::TryInto;
use rand::prelude::*;
use rand::distributions::{ Distribution, Uniform };
use crate::utils::{ uninit_vector };
use sp_std::vec::Vec;
use wasm_bindgen_test::console_log;
use sha2::{Digest, Sha256};
use crypto::hashes::{
    blake2b::Blake2b256
};
use b2sum_rs::Blake2bSum;

// CONSTANTS
// ================================================================================================

// Field modulus = 2^128 - 45 * 2^40 + 1
pub const M: u128 = 340282366920938463463374557953744961537;

// 2^40 root of unity
pub const G: u128 = 23953097886125630542083529559205016746;

// public constants
pub const MODULUS: u128 = M;
pub const RANGE: Range<u128> = Range { start: 0, end: M };

pub const ZERO: u128 = 0;
pub const ONE: u128 = 1;

// BASIC ARITHMETIC
// --------------------------------------------------------------------------------------------

/// Computes (a + b) % m; a and b are assumed to be valid field elements.
pub fn add(a: u128, b: u128) -> u128 {
    let z = M - b;
    return if a < z { M - z + a } else { a - z};
}

/// Computes (a - b) % m; a and b are assumed to be valid field elements.
pub fn sub(a: u128, b: u128) -> u128 {
    return if a < b { M - b + a } else { a - b };
}

/// Computes (a * b) % m; a and b are assumed to be valid field elements.
pub fn mul(a: u128, b: u128) -> u128 {

    let (x0, x1, x2) = mul_128x64(a, (b >> 64) as u64);         // x = a * b_hi
    let (mut x0, mut x1, x2) = mul_reduce(x0, x1, x2);          // x = x - (x >> 128) * m
    if x2 == 1 {
        // if there was an overflow beyond 128 bits, subtract
        // modulus from the result to make sure it fits into 
        // 128 bits; this can potentially be removed in favor
        // of checking overflow later
        let (t0, t1) = sub_modulus(x0, x1);                     // x = x - m
        x0 = t0; x1 = t1;
    }

    let (y0, y1, y2) = mul_128x64(a, b as u64);                 // y = a * b_lo

    let (mut y1, carry) = add64_with_carry(y1, x0, 0);          // y = y + (x << 64)
    let (mut y2, y3) = add64_with_carry(y2, x1, carry);
    if y3 == 1 {
        // if there was an overflow beyond 192 bits, subtract
        // modulus * 2^64 from the result to make sure it fits
        // into 192 bits; this can potentially replace the
        // previous overflow check (but needs to be proven)
        let (t0, t1) = sub_modulus(y1, y2);                     // y = y - (m << 64)
        y1 = t0; y2 = t1;
    }
    
    let (mut z0, mut z1, z2) = mul_reduce(y0, y1, y2);          // z = y - (y >> 128) * m

    // make sure z is smaller than m
    if z2 == 1 || (z1 == (M >> 64) as u64 && z0 >= (M as u64)) {
        let (t0, t1) = sub_modulus(z0, z1);                     // z = z - m
        z0 = t0; z1 = t1;
    }

    return ((z1 as u128) << 64) + (z0 as u128);
}

/// Computes a[i] + b[i] * c for all i and saves result into a.
pub fn mul_acc(a: &mut [u128], b: &[u128], c: u128) {
    for i in 0..a.len() {
        a[i] = add(a[i], mul(b[i], c));
    }
}

/// Computes y such that (x * y) % m = 1; x is assumed to be a valid field element.
pub fn inv(x: u128) -> u128 {
    if x == 0 { return 0 };

    // initialize v, a, u, and d variables
    let mut v = M;
    let (mut a0, mut a1, mut a2) = (0, 0, 0);
    let (mut u0, mut u1, mut u2) = if x & 1 == 1 {
        // u = x
        (x as u64, (x >> 64) as u64, 0)
    }
    else {
        // u = x + m
        add_192x192(x as u64, (x >> 64) as u64, 0, M as u64, (M >> 64) as u64, 0)
    };
    // d = m - 1
    let (mut d0, mut d1, mut d2) = ((M as u64) - 1, (M >> 64) as u64, 0);

    // compute the inverse
    while v != 1 {
        while u2 > 0 || ((u0 as u128) + ((u1 as u128) << 64)) > v { // u > v
            // u = u - v
            let (t0, t1, t2) = sub_192x192(u0, u1, u2, v as u64, (v >> 64) as u64, 0);
            u0 = t0; u1 = t1; u2 = t2;
            
            // d = d + a
            let (t0, t1, t2) = add_192x192(d0, d1, d2, a0, a1, a2);
            d0 = t0; d1 = t1; d2 = t2;
            
            while u0 & 1 == 0 {
                if d0 & 1 == 1 {
                    // d = d + m
                    let (t0, t1, t2) = add_192x192(d0, d1, d2, M as u64, (M >> 64) as u64, 0);
                    d0 = t0; d1 = t1; d2 = t2;
                }

                // u = u >> 1
                u0 = (u0 >> 1) | ((u1 & 1) << 63);
                u1 = (u1 >> 1) | ((u2 & 1) << 63);
                u2 = u2 >> 1;

                // d = d >> 1
                d0 = (d0 >> 1) | ((d1 & 1) << 63);
                d1 = (d1 >> 1) | ((d2 & 1) << 63);
                d2 = d2 >> 1;
            }
        }

        // v = v - u (u is less than v at this point)
        v = v - ((u0 as u128) + ((u1 as u128) << 64));
        
        // a = a + d
        let (t0, t1, t2) = add_192x192(a0, a1, a2, d0, d1, d2);
        a0 = t0; a1 = t1; a2 = t2;

        while v & 1 == 0 {
            if a0 & 1 == 1 {
                // a = a + m
                let (t0, t1, t2) = add_192x192(a0, a1, a2, M as u64, (M >> 64) as u64, 0);
                a0 = t0; a1 = t1; a2 = t2;
            }

            v = v >> 1;

            // a = a >> 1
            a0 = (a0 >> 1) | ((a1 & 1) << 63);
            a1 = (a1 >> 1) | ((a2 & 1) << 63);
            a2 = a2 >> 1;
        }
    }

    // a = a mod m
    let mut a = (a0 as u128) + ((a1 as u128) << 64);
    while a2 > 0 || a >= M {
        let (t0, t1, t2) = sub_192x192(a0, a1, a2, M as u64, (M >> 64) as u64, 0);
        a0 = t0; a1 = t1; a2 = t2;
        a = (a0 as u128) + ((a1 as u128) << 64);
    }

    return a;
}

/// Computes multiplicative inverses of all slice elements using batch inversion method.
pub fn inv_many(values: &[u128]) -> Vec<u128> {
    let mut result = uninit_vector(values.len());
    inv_many_fill(values, &mut result);
    return result;
}

/// Computes multiplicative inverses of all slice elements using batch inversion method
/// and stores the result into the provided slice.
pub fn inv_many_fill(values: &[u128], result: &mut [u128]) {
    let mut last = ONE;
    for i in 0..values.len() {
        result[i] = last;
        if values[i] != ZERO {
            last = mul(last, values[i]);
        }
    }

    last = inv(last);
    for i in (0..values.len()).rev() {
        if values[i] == ZERO {
            result[i] = ZERO;
        }
        else {
            result[i] = mul(last, result[i]);
            last = mul(last, values[i]);
        }
    }
}

/// Computes y = (a / b) such that (b * y) % m = a; a and b are assumed to be valid field elements.
pub fn div(a: u128, b: u128) -> u128 {
    let b = inv(b);
    return mul(a, b);
}

/// Computes (b^p) % m; b and p are assumed to be valid field elements.
pub fn exp(b: u128, p: u128) -> u128 {
    if b == 0 { return 0; }
    else if p == 0 { return 1; }

    let mut r = 1;
    let mut b = b;
    let mut p = p;

    // TODO: optimize
    while p > 0 {
        if p & 1 == 1 {
            r = mul(r, b);
        }
        p = p >> 1;
        b = mul(b, b);
    }

    return r;
}

/// Computes (0 - x) % m; x is assumed to be a valid field element.
pub fn neg(x: u128) -> u128 {
    return sub(ZERO, x);
}

// ROOT OF UNITY
// --------------------------------------------------------------------------------------------
pub fn get_root_of_unity(order: usize) -> u128 {
    assert!(order != 0, "cannot get root of unity for order 0");
    assert!(order.is_power_of_two(), "order must be a power of 2");
    assert!(order.trailing_zeros() <= 40, "order cannot exceed 2^40");
    let p = 1u128 << (40 - order.trailing_zeros());
    return exp(G, p);
}

/// Generates a vector with values [1, b, b^2, b^3, b^4, ..., b^length].
pub fn get_power_series(b: u128, length: usize) -> Vec<u128> {
    // console_log!("b is{:?},length is {:?}",b,length);

    let mut result = uninit_vector(length);
    // console_log!("get_power_series result is {:?},len is {:?}",result,result.len());


    result[0] = ONE;
    for i in 1..result.len() {
        result[i] = mul(result[i - 1], b);

    }    
    // console_log!("aftfer for result is {:?}",result);
    return result;
}

// pub fn sha256_a(x1: u128, x2:u128, y1:u128, y2:u128) -> u128 {
//     let x1 = format!("{:x}", x1);
//     let x2 = format!("{:x}", x2);
//     let y1= format!("{:x}", y1);
//     let y2 = format!("{:x}", y2);
//     let x3 = x1 + &x2;
//     let y3 = y1 + &y2;
//     let x4 :Vec<u8> = hex::decode(x3.clone()).unwrap();
//     let y4 :Vec<u8> = hex::decode(y3.clone()).unwrap();

//     let mut hasher = Sha256::new();
//     hasher.update(x4);
//     hasher.update(y4);
//     let mut result = hasher.finalize();

//     let whole = format!("{:x}", result.clone());
//     let head = &whole[0..32];
//     let i = u128::from_str_radix(head,16).unwrap();
//     return i;
// }
// pub fn sha256_b(x1: u128, x2:u128, y1:u128, y2:u128) -> u128 {
//     let x1 = format!("{:x}", x1);
//     let x2 = format!("{:x}", x2);
//     let y1= format!("{:x}", y1);
//     let y2 = format!("{:x}", y2);
//     let x3 = x1 + &x2;
//     let y3 = y1 + &y2;
//     let x4 :Vec<u8> = hex::decode(x3.clone()).unwrap();
//     let y4 :Vec<u8> = hex::decode(y3.clone()).unwrap();

//     let mut hasher = Sha256::new();
//     hasher.update(x4);
//     hasher.update(y4);
//     let mut result = hasher.finalize();


//     let whole = format!("{:x}", result.clone());
//     let head = &whole[32..];
//     let i = u128::from_str_radix(head,16).unwrap();
//     return i;
// }

// pub fn blake_a(x1: u128, x2:u128, x3: u128, x4:u128,x5: u128, y1:u128, y2:u128) -> u128 {
//     let zero = String::from("0");

//     let mut x1 = format!("{:x}", x1);
//     let mut x2 = format!("{:x}", x2);
//     let mut x3 = format!("{:x}", x3);
//     let mut x4 = format!("{:x}", x4);    
//     let mut x5 = format!("{:x}", x5);
//     if x1.len() % 2 !=0{
//         x1 = zero.clone() + &x1;
//     };
//     while x1.len() < 8{
//         x1 = zero.clone() + &x1;
//     }
//     if x2.len() % 2 !=0{
//         x2 = zero.clone() + &x2;
//     };
//     while x2.len() < 4{
//         x2 = zero.clone() + &x2;
//     }
//     if x3.len() % 2 !=0{
//         x3 = zero.clone() + &x3;
//     };
//     while x3.len() < 4{
//         x3 = zero.clone() + &x3;
//     }
//     if x4.len() % 2 !=0{
//         x4 = zero.clone() + &x4;
//     };    
//     while x4.len() < 4{
//         x4 = zero.clone() + &x4;
//     }
//     if x5.len() % 2 !=0{
//         x5 = zero.clone() + &x5;
//     };
//     while x5.len() < 12{
//         x5 = zero.clone() + &x5;
//     }
    

//     let hyphen = String::from('-');
//     let nonce = x1 + &hyphen + &x2 + &hyphen + &x3 + &hyphen + &x4 + &hyphen + &x5;
//     console_log!("hi im in blake2b nonce  is {:?}",nonce);

//     let y0 = String::from("0x");
//     let mut y1= format!("{:x}", y1);
//     let mut y2 = format!("{:x}", y2);
//     let mut i1 = 32 - y1.len();
//     while i1 > 0{
//         y1 = zero.clone() + &y1;
//         i1 -=1;
//     } 
//     let mut i2 = 32 - y2.len();
//     while i2 > 0{
//         y2 = zero.clone() + &y2;
//         i2 -=1;
//     } 
    
//     let hash_before = y0 + &y1 + &y2;
//     let hash = nonce+ &hash_before;
//     let hasher = Blake2b256::digest(hash.as_bytes());
//     let whole = format!("{:x}", hasher.clone());
//     let head = &whole[0..32];
//     let i = u128::from_str_radix(head,16).unwrap();
//     return i;
// }
// pub fn blake_b(x1: u128, x2:u128, x3: u128, x4:u128,x5: u128, y1:u128, y2:u128) -> u128 {
//     let zero = String::from("0");

//     let mut x1 = format!("{:x}", x1);
//     let mut x2 = format!("{:x}", x2);
//     let mut x3 = format!("{:x}", x3);
//     let mut x4 = format!("{:x}", x4);    
//     let mut x5 = format!("{:x}", x5);
//     if x1.len() % 2 !=0{
//         x1 = zero.clone() + &x1;
//     };
//     while x1.len() < 8{
//         x1 = zero.clone() + &x1;
//     }
//     if x2.len() % 2 !=0{
//         x2 = zero.clone() + &x2;
//     };
//     while x2.len() < 4{
//         x2 = zero.clone() + &x2;
//     }
//     if x3.len() % 2 !=0{
//         x3 = zero.clone() + &x3;
//     };
//     while x3.len() < 4{
//         x3 = zero.clone() + &x3;
//     }
//     if x4.len() % 2 !=0{
//         x4 = zero.clone() + &x4;
//     };    
//     while x4.len() < 4{
//         x4 = zero.clone() + &x4;
//     }
//     if x5.len() % 2 !=0{
//         x5 = zero.clone() + &x5;
//     };
//     while x5.len() < 12{
//         x5 = zero.clone() + &x5;
//     }
    
//     let hyphen = String::from('-');
//     let nonce = x1 + &hyphen + &x2 + &hyphen + &x3 + &hyphen + &x4 + &hyphen + &x5;
//     console_log!("hi im in blake2b nonce  is {:?}",nonce);

//     let y0 = String::from("0x");
//     let mut y1= format!("{:x}", y1);
//     let mut y2 = format!("{:x}", y2);
//     let mut i1 = 32 - y1.len();
//     while i1 > 0{
//         y1 = zero.clone() + &y1;
//         i1 -=1;
//     } 
//     let mut i2 = 32 - y2.len();
//     while i2 > 0{
//         y2 = zero.clone() + &y2;
//         i2 -=1;
//     } 
    
//     let hash_before = y0 + &y1 + &y2;
//     let hash = nonce+ &hash_before;
//     let hasher = Blake2b256::digest(hash.as_bytes());
//     let whole = format!("{:x}", hasher.clone());
//     let head = &whole[32..];
//     let i = u128::from_str_radix(head,16).unwrap();
//     return i;
// }

pub fn kvalid_a(x1:u128, x2:u128, x3:u128, x4:u128, x5:u128, content:u128, ctype_1:u128, ctype_2:u128, ascii:u128) -> u128{
    let prefix = String::from("{\"kilt:ctype:");
    let joint_1 = String::from('#');
    let joint_2 = String::from("\":");
    let suffice = String::from('}');

    let zero = String::from("0");
    let mut ctype_1= format!("{:x}", ctype_1);
    let mut i1 = 32 - ctype_1.len();
    while i1 > 0 {
        ctype_1 = zero.clone() + &ctype_1;
        i1 -=1;
    } 
    let mut ctype_2 = format!("{:x}", ctype_2);
    let mut i2 = 32 - ctype_2.len();
    while i2 > 0 {
        ctype_2 = zero.clone() + &ctype_2;
        i2 -=1;
    } 
    let y0 = String::from("0x");

    let ctype_hash = y0+ &ctype_1 + &ctype_2;  

    let ascii_string = format!("{:x}",ascii);
    let ascii_vec = hex::decode(ascii_string.clone()).unwrap();
    let ascii_final = String::from_utf8(ascii_vec).unwrap();


    let origin_string = prefix + &ctype_hash + &joint_1 + &ascii_final + &joint_2 + &content.to_string() + &suffice;

    let mut x1 = format!("{:x}", x1);
    let mut x2 = format!("{:x}", x2);
    let mut x3 = format!("{:x}", x3);
    let mut x4 = format!("{:x}", x4);    
    let mut x5 = format!("{:x}", x5);
    let y0 = String::from("0x");

    if x1.len() % 2 !=0{
            x1 = zero.clone() + &x1;
    };
    while x1.len() < 8{
        x1 = zero.clone() + &x1;
    }
    if x2.len() % 2 !=0{
        x2 = zero.clone() + &x2;
    };
    while x2.len() < 4{
        x2 = zero.clone() + &x2;
    }
    if x3.len() % 2 !=0{
        x3 = zero.clone() + &x3;
    };
    while x3.len() < 4{
        x3 = zero.clone() + &x3;
    }
    if x4.len() % 2 !=0{
        x4 = zero.clone() + &x4;
    };    
    while x4.len() < 4{
        x4 = zero.clone() + &x4;
    }
    if x5.len() % 2 !=0{
        x5 = zero.clone() + &x5;
    };
    while x5.len() < 12{
        x5 = zero.clone() + &x5;
    }

    let hyphen = String::from('-');
    let y4 = x1 + &hyphen + &x2 + &hyphen + &x3 + &hyphen + &x4 + &hyphen + &x5;
    let mut hasher = Blake2b256::digest(origin_string.as_bytes());
    let mut whole = format!("{:x}", hasher.clone());

    let go_to_hash = y4 + &y0 + &whole;
    let mut hasher = Blake2b256::digest(go_to_hash.as_bytes());
    let mut whole = format!("{:x}", hasher.clone());

    let head = &whole[0..32];
    let rear = &whole[32..];
    let i = u128::from_str_radix(head,16).unwrap();
    return i;    
}

pub fn kvalid_b(x1:u128, x2:u128, x3:u128, x4:u128, x5:u128, content:u128, ctype_1:u128, ctype_2:u128, ascii:u128) -> u128{
    let prefix = String::from("{\"kilt:ctype:");
    let joint_1 = String::from('#');
    let joint_2 = String::from("\":");
    let suffice = String::from('}');

    let zero = String::from("0");
    let mut ctype_1= format!("{:x}", ctype_1);
    let mut i1 = 32 - ctype_1.len();
    while i1 > 0 {
        ctype_1 = zero.clone() + &ctype_1;
        i1 -=1;
    } 
    let mut ctype_2 = format!("{:x}", ctype_2);
    let mut i2 = 32 - ctype_2.len();
    while i2 > 0 {
        ctype_2 = zero.clone() + &ctype_2;
        i2 -=1;
    } 
    let y0 = String::from("0x");

    let ctype_hash = y0+ &ctype_1 + &ctype_2;  

    let ascii_string = format!("{:x}",ascii);
    let ascii_vec = hex::decode(ascii_string.clone()).unwrap();
    let ascii_final = String::from_utf8(ascii_vec).unwrap();
    
    let origin_string = prefix + &ctype_hash + &joint_1 + &ascii_final + &joint_2 + &content.to_string() + &suffice;

    let mut x1 = format!("{:x}", x1);
    let mut x2 = format!("{:x}", x2);
    let mut x3 = format!("{:x}", x3);
    let mut x4 = format!("{:x}", x4);    
    let mut x5 = format!("{:x}", x5);
    let y0 = String::from("0x");

    if x1.len() % 2 !=0{
            x1 = zero.clone() + &x1;
    };
    while x1.len() < 8{
        x1 = zero.clone() + &x1;
    }

    if x2.len() % 2 !=0{
        x2 = zero.clone() + &x2;
    };

    while x2.len() < 4{
        x2 = zero.clone() + &x2;
    }
    if x3.len() % 2 !=0{
        x3 = zero.clone() + &x3;
    };
    while x3.len() < 4{
        x3 = zero.clone() + &x3;
    }
    if x4.len() % 2 !=0{
        x4 = zero.clone() + &x4;
    };    
    while x4.len() < 4{
        x4 = zero.clone() + &x4;
    }
    if x5.len() % 2 !=0{
        x5 = zero.clone() + &x5;
    };
    while x5.len() < 12{
        x5 = zero.clone() + &x5;
    }

    let hyphen = String::from('-');
    let y4 = x1 + &hyphen + &x2 + &hyphen + &x3 + &hyphen + &x4 + &hyphen + &x5;


    let mut hasher = Blake2b256::digest(origin_string.as_bytes());
    let mut whole = format!("{:x}", hasher.clone());

    let go_to_hash = y4 + &y0 + &whole;
    let mut hasher = Blake2b256::digest(go_to_hash.as_bytes());
    let mut whole = format!("{:x}", hasher.clone());

    let head = &whole[0..32];
    let rear = &whole[32..];

    // let i = u128::from_str_radix(head,16).unwrap();
    let i2 = u128::from_str_radix(rear,16).unwrap();

    return i2;    
}

pub fn khash_a(hash_in_khash: &Vec<u128>, n:u32) -> u128{
    let zero = String::from("0");
    let mut hex_list: Vec<Vec<u8>> = Vec::new();
    let mut concat_saltedhash: Vec<u8> = Vec::new();
    assert_eq!(hash_in_khash.len() / 2, n as usize);
    for i in 0..n{
        let mut x1 = format!("{:x}", hash_in_khash[(i * 2) as usize]);
        let mut y1 = format!("{:x}", hash_in_khash[(i * 2 + 1) as usize]);

        let mut i = 32 - x1.len();
        while i > 0 {
            x1 = zero.clone() + &x1;
            i -=1;
        }; 

        let mut i = 32 - y1.len();
        while i > 0 {
            y1 = zero.clone() + &y1;
            i -=1;
        }; 
        hex_list.push(hex::decode(x1 + &y1).unwrap());
    };
    console_log!("hex_list is {:?}",hex_list);

    hex_list.sort();
    console_log!("hex_list is {:?}",hex_list);


    for i in 0..n{
        concat_saltedhash.append(&mut hex_list[i as usize]);
    } 
    console_log!("concat is {:?}",concat_saltedhash);

    let context = Blake2bSum::new(32);
    let hash = context.read_bytes(&concat_saltedhash);
    let bytes = Blake2bSum::as_bytes(&hash);
    let whole = hex::encode(bytes);

    let head = &whole[0..32];
    console_log!("head is {:?}",head);

    let i = u128::from_str_radix(head,16).unwrap();
    return i;
}


pub fn khash_b(hash_in_khash: &Vec<u128>, n:u32) -> u128{
    let zero = String::from("0");
    let mut hex_list: Vec<Vec<u8>> = Vec::new();
    let mut concat_saltedhash: Vec<u8> = Vec::new();
    assert_eq!(hash_in_khash.len() / 2, n as usize);
    for i in 0..n{
        let mut x1 = format!("{:x}", hash_in_khash[(i * 2) as usize]);
        let mut y1 = format!("{:x}", hash_in_khash[(i * 2 + 1) as usize]);

        let mut i = 32 - x1.len();
        while i > 0 {
            x1 = zero.clone() + &x1;
            i -=1;
        }; 

        let mut i = 32 - y1.len();
        while i > 0 {
            y1 = zero.clone() + &y1;
            i -=1;
        }; 
        hex_list.push(hex::decode(x1 + &y1).unwrap());
    };
    hex_list.sort();

    for i in 0..n{
        concat_saltedhash.append(&mut hex_list[i as usize]);
    } 

    let context = Blake2bSum::new(32);
    let hash = context.read_bytes(&concat_saltedhash);
    let bytes = Blake2bSum::as_bytes(&hash);
    let whole = hex::encode(bytes);
    let rear = &whole[32..];
    console_log!("rear is {:?}",rear);

    let i = u128::from_str_radix(rear,16).unwrap();
    return i;



}
// RANDOMNESS
// --------------------------------------------------------------------------------------------

/// Generates a random field element.
pub fn rand() -> u128 {
    let range = Uniform::from(RANGE);
    let mut g = rand::thread_rng();
    return g.sample(range);
}

/// Generates a vector of random field elements.
pub fn rand_vector(length: usize) -> Vec<u128> {
    let range = Uniform::from(RANGE);
    let g = rand::thread_rng();
    return g.sample_iter(range).take(length).collect();
}

/// Generates a pseudo-random field element from a given `seed`.
pub fn prng(seed: [u8; 32]) -> u128 {
    let range = Uniform::from(RANGE);
    let mut g = StdRng::from_seed(seed);
    return range.sample(&mut g);
}

/// Generates a vector of pseudo-random field elements from a given `seed`.
pub fn prng_vector(seed: [u8; 32], length: usize) -> Vec<u128> {
    let range = Uniform::from(RANGE);
    let g = StdRng::from_seed(seed);
    return g.sample_iter(range).take(length).collect();
}

// TYPE CONVERSIONS
// --------------------------------------------------------------------------------------------
pub fn from_bytes(bytes: &[u8]) -> u128 { 
    return u128::from_le_bytes(bytes.try_into().unwrap());
}

// HELPER FUNCTIONS
// ================================================================================================

#[inline(always)]
fn mul_128x64(a: u128, b: u64) -> (u64, u64, u64) {
    let z_lo = ((a as u64) as u128) * (b as u128);
    let z_hi = (a >> 64) * (b as u128);
    let z_hi = z_hi + (z_lo >> 64);
    return (z_lo as u64, z_hi as u64, (z_hi >> 64) as u64);
}

#[inline(always)]
fn mul_reduce(z0: u64, z1: u64, z2: u64) -> (u64, u64, u64) {
    let (q0, q1, q2) = mul_by_modulus(z2);
    let (z0, z1, z2) = sub_192x192(z0, z1, z2, q0, q1, q2);
    return (z0, z1, z2);
}

#[inline(always)]
fn mul_by_modulus(a: u64) -> (u64, u64, u64) {
    let a_lo = (a as u128).wrapping_mul(M);
    let a_hi = if a == 0 { 0 } else { a - 1 };
    return (a_lo as u64, (a_lo >> 64) as u64, a_hi);
}

#[inline(always)]
fn sub_modulus(a_lo: u64, a_hi: u64) -> (u64, u64) {
    let mut z = 0u128.wrapping_sub(M);
    z = z.wrapping_add(a_lo as u128);
    z = z.wrapping_add((a_hi as u128) << 64);
    return (z as u64, (z >> 64) as u64);
}

#[inline(always)]
fn sub_192x192(a0: u64, a1: u64, a2: u64, b0: u64, b1: u64, b2: u64) -> (u64, u64, u64) {
    let z0 = (a0 as u128).wrapping_sub(b0 as u128);
    let z1 = (a1 as u128).wrapping_sub((b1 as u128) + (z0 >> 127));
    let z2 = (a2 as u128).wrapping_sub((b2 as u128) + (z1 >> 127));
    return (z0 as u64, z1 as u64, z2 as u64);
}

#[inline(always)]
fn add_192x192(a0: u64, a1: u64, a2: u64, b0: u64, b1: u64, b2: u64) -> (u64, u64, u64) {
    let z0 = (a0 as u128) + (b0 as u128);
    let z1 = (a1 as u128) + (b1 as u128) + (z0 >> 64);
    let z2 = (a2 as u128) + (b2 as u128) + (z1 >> 64);
    return (z0 as u64, z1 as u64, z2 as u64);
}

#[inline(always)]
pub const fn add64_with_carry(a: u64, b: u64, carry: u64) -> (u64, u64) {
    let ret = (a as u128) + (b as u128) + (carry as u128);
    return (ret as u64, (ret >> 64) as u64);
}

// TESTS
// ================================================================================================
#[cfg(test)]
mod tests {

    use sp_std::convert::TryInto;
    use num_bigint::{ BigUint };

    #[test]
    fn add() {
        // identity
        let r: u128 = super::rand();
        assert_eq!(r, super::add(r, 0));

        // test addition within bounds
        assert_eq!(5, super::add(2, 3));

        // test overflow
        let m: u128 = super::MODULUS;
        let t = m - 1;
        assert_eq!(0, super::add(t, 1));
        assert_eq!(1, super::add(t, 2));

        // test random values
        let r1: u128 = super::rand();
        let r2: u128 = super::rand();

        let expected = (BigUint::from(r1) + BigUint::from(r2)) % BigUint::from(super::M);
        let expected = u128::from_le_bytes((expected.to_bytes_le()[..]).try_into().unwrap());
        assert_eq!(expected, super::add(r1, r2));
    }

    #[test]
    fn sub() {
        // identity
        let r: u128 = super::rand();
        assert_eq!(r, super::sub(r, 0));

        // test subtraction within bounds
        assert_eq!(2, super::sub(5u128, 3));

        // test underflow
        let m: u128 = super::MODULUS;
        assert_eq!(m - 2, super::sub(3u128, 5));
    }

    #[test]
    fn mul() {
        // identity
        let r: u128 = super::rand();
        assert_eq!(0, super::mul(r, 0));
        assert_eq!(r, super::mul(r, 1));

        // test multiplication within bounds
        assert_eq!(15, super::mul(5u128, 3));

        // test overflow
        let m: u128 = super::MODULUS;
        let t = m - 1;
        assert_eq!(1, super::mul(t, t));
        assert_eq!(m - 2, super::mul(t, 2));
        assert_eq!(m - 4, super::mul(t, 4));

        let t = (m + 1) / 2;
        assert_eq!(1, super::mul(t, 2));

        // test random values
        let v1: Vec<u128> = super::rand_vector(1000);
        let v2: Vec<u128> = super::rand_vector(1000);
        for i in 0..v1.len() {
            let r1 = v1[i];
            let r2 = v2[i];

            let result = (BigUint::from(r1) * BigUint::from(r2)) % BigUint::from(super::M);
            let result = result.to_bytes_le();
            let mut expected = [0u8; 16];
            expected[0..result.len()].copy_from_slice(&result);
            let expected = u128::from_le_bytes(expected);

            if expected != super::mul(r1, 32) {
                println!("failed for: {} * {}", r1, r2);
                assert_eq!(expected, super::mul(r1, r2));
            }
        }
    }

    #[test]
    fn inv() {
        // identity
        assert_eq!(1, super::inv(1));
        assert_eq!(0, super::inv(0));

        // test random values
        let x: Vec<u128> = super::rand_vector(1000);
        for i in 0..x.len() {
            let y = super::inv(x[i]);
            assert_eq!(1, super::mul(x[i], y));
        }
    }

    #[test]
    fn get_root_of_unity() {
        let root_40: u128 = super::get_root_of_unity(usize::pow(2, 40));
        assert_eq!(23953097886125630542083529559205016746, root_40);
        assert_eq!(1, super::exp(root_40, u128::pow(2, 40)));

        let root_39: u128 = super::get_root_of_unity(usize::pow(2, 39));
        let expected = super::exp(root_40, 2);
        assert_eq!(expected, root_39);
        assert_eq!(1, super::exp(root_39, u128::pow(2, 39)));
    }
}