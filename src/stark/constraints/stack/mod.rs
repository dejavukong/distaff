use crate::{
    math::{ field, polynom },
    processor::OpCode,
    stark::TraceState,
    utils::hasher::ARK,
    BASE_CYCLE_LENGTH, HASH_STATE_WIDTH
};
use super::utils::{
    are_equal, is_zero, is_binary, binary_not, extend_constants, EvaluationResult,
    enforce_stack_copy, enforce_left_shift, enforce_right_shift,
};

mod input;
use input::{ enforce_push, enforce_read, enforce_read2 };

mod arithmetic;
use arithmetic::{
    enforce_add, enforce_mul, enforce_inv, enforce_neg,
    enforce_not, enforce_and, enforce_or, enforce_khash
};

mod manipulation;
use manipulation::{
    enforce_dup, enforce_dup2, enforce_dup4, enforce_pad2, enforce_drop, enforce_drop4,
    enforce_swap, enforce_swap2, enforce_swap4, enforce_roll4, enforce_roll8,
};

mod comparison;
use comparison::{ enforce_assert, enforce_asserteq, enforce_eq, enforce_cmp, enforce_binacc };

mod conditional;
use conditional::{ enforce_choose, enforce_choose2, enforce_cswap2 };

mod hash;
use hash::{ enforce_rescr };

use sp_std::{vec, vec::Vec};
use wasm_bindgen_test::*;

// CONSTANTS
// ================================================================================================
pub const NUM_AUX_CONSTRAINTS: usize = 2;
const AUX_CONSTRAINT_DEGREES: [usize; NUM_AUX_CONSTRAINTS] = [7, 7];
const STACK_TRANSITION_DEGREE: usize = 7; // degree for all stack register transition constraints

// TYPES AND INTERFACES
// ================================================================================================
#[derive(Debug)]

pub struct Stack {
    trace_length        : usize,
    cycle_length        : usize,
    ark_values          : Vec<[u128; 2 * HASH_STATE_WIDTH]>,
    ark_polys           : Vec<Vec<u128>>,
    constraint_degrees  : Vec<usize>,
}

// STACK CONSTRAINT EVALUATOR IMPLEMENTATION
// ================================================================================================
impl Stack {

    pub fn new(trace_length: usize, extension_factor: usize, stack_depth: usize) -> Stack 
    {
        // 参数为 trace_length:256, extension_factor:8, stack_depth:10
    
        // build an array of constraint degrees for the stack
        // 先在degrees里面插两个[7,7]
        let mut degrees = Vec::from(&AUX_CONSTRAINT_DEGREES[..]);
        
        // 再给degrees里面补7，即补到 12 个 7
        degrees.resize(stack_depth + NUM_AUX_CONSTRAINTS, STACK_TRANSITION_DEGREE);

        // determine extended cycle length
        // cycle_length = 16 * 8 = 128
        let cycle_length = BASE_CYCLE_LENGTH * extension_factor;


        // 这里用的ARK是hasher里的ark 有12行的[每行16个]
        // 先12个插值出多项式，再在 128个点上进行插值，插值出多项式，及128个点值
        // extend rounds constants by the specified extension factor
        let (ark_polys, ark_evaluations) = extend_constants(&ARK, extension_factor);
        // 将12个[128个点值]转置，转为128个[12个点值]

        let ark_values = transpose_ark_constants(ark_evaluations, cycle_length);
        return Stack {
            // 256    ,  128
            trace_length, cycle_length,
            // 12个多项式在128个点上的点值转置，变成128个[12个点值]，12个多项式
            ark_values, ark_polys,
            // 12个7
            constraint_degrees: degrees,
        };
    }

    pub fn constraint_degrees(&self) -> &[usize] {
        return &self.constraint_degrees;
    }

    // EVALUATOR FUNCTIONS
    // --------------------------------------------------------------------------------------------

    /// Evaluates stack transition constraints at the specified step of the evaluation domain and
    /// saves the evaluations into `result`.
    pub fn evaluate(&self, current: &TraceState, next: &TraceState, step: usize, result: &mut [u128])
    {
        // determine round constants at the specified step
        let ark = self.ark_values[step % self.cycle_length];

        // evaluate transition constraints for the stack
        enforce_constraints(current, next, &ark, result);
    }

    /// Evaluates stack transition constraints at the specified x coordinate and saves the
    /// evaluations into `result`. Unlike the function above, this function can evaluate constraints
    /// at any out-of-domain point, but it is much slower than the previous function.
    pub fn evaluate_at(&self, current: &TraceState, next: &TraceState, x: u128, result: &mut [u128])
    {
        // map x to the corresponding coordinate in constant cycles
        let num_cycles = (self.trace_length / BASE_CYCLE_LENGTH) as u128;
        let x = field::exp(x, num_cycles);

        // determine round constants at the specified x coordinate
        let mut ark = [field::ZERO; 2 * HASH_STATE_WIDTH];
        for i in 0..ark.len() {
            ark[i] = polynom::eval(&self.ark_polys[i], x);
        }

        // evaluate transition constraints for the stack
        enforce_constraints(current, next, &ark, result);
    }
}

// HELPER FUNCTIONS
// ================================================================================================
fn enforce_constraints(current: &TraceState, next: &TraceState, ark: &[u128], result: &mut [u128])
{
    // split constraint evaluation result into aux constraints and stack constraints
    // 这里 aux constraint 占 2 位，result 占 10 位
    // aux用来确定参与运算的值是二进制的。
    let (aux, result) = result.split_at_mut(NUM_AUX_CONSTRAINTS);

    // get user stack registers from current and next steps
    let old_stack = current.user_stack();
    let new_stack = next.user_stack();

    // initialize a vector to hold stack constraint evaluations; this is needed because
    // constraint evaluator functions assume that the stack is at least 8 items deep; while
    // it may actually be smaller than that

    // 新建一个全0的数组，长度是old_stack
    let mut evaluations = vec![field::ZERO; old_stack.len()];

    // 1 ----- enforce constraints for low-degree operations --------------------------------------
    let ld_flags = current.ld_op_flags(); // ld_flags 表明这是哪一个low degree 操作（这里ld_flags 是index ，比如00..0010000，第16个是1，则是read操作）

    // assertion operations
    enforce_assert  (&mut evaluations, aux, old_stack, new_stack, ld_flags[OpCode::Assert.ld_index()]);
    enforce_asserteq(&mut evaluations, aux, old_stack, new_stack, ld_flags[OpCode::AssertEq.ld_index()]);

    // input operations
    enforce_read    (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Read.ld_index()]);
    enforce_read2   (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Read2.ld_index()]);

    // stack manipulation operations
    enforce_dup     (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Dup.ld_index()]);
    enforce_dup2    (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Dup2.ld_index()]);
    enforce_dup4    (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Dup4.ld_index()]);
    enforce_pad2    (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Pad2.ld_index()]);

    enforce_drop    (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Drop.ld_index()]);
    enforce_drop4   (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Drop4.ld_index()]);
    
    enforce_swap    (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Swap.ld_index()]);
    enforce_swap2   (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Swap2.ld_index()]);
    enforce_swap4   (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Swap4.ld_index()]);

    enforce_roll4   (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Roll4.ld_index()]);
    enforce_roll8   (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Roll8.ld_index()]);

    // arithmetic and boolean operations
    enforce_add     (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Add.ld_index()]);
    enforce_mul     (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Mul.ld_index()]);
    enforce_inv     (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Inv.ld_index()]);
    enforce_neg     (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Neg.ld_index()]);
    enforce_not     (&mut evaluations, aux, old_stack, new_stack, ld_flags[OpCode::Not.ld_index()]);
    enforce_and     (&mut evaluations, aux, old_stack, new_stack, ld_flags[OpCode::And.ld_index()]);
    enforce_or      (&mut evaluations, aux, old_stack, new_stack, ld_flags[OpCode::Or.ld_index()]);
    enforce_khash   (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Khash.ld_index()]);
    // enforce_khash   (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::Kash.ld_index()]);

    // comparison operations
    enforce_eq      (&mut evaluations, aux, old_stack, new_stack, ld_flags[OpCode::Eq.ld_index()]);
    enforce_binacc  (&mut evaluations,      old_stack, new_stack, ld_flags[OpCode::BinAcc.ld_index()]);

    // conditional selection operations
    enforce_choose  (&mut evaluations, aux, old_stack, new_stack, ld_flags[OpCode::Choose.ld_index()]);
    enforce_choose2 (&mut evaluations, aux, old_stack, new_stack, ld_flags[OpCode::Choose2.ld_index()]);
    enforce_cswap2  (&mut evaluations, aux, old_stack, new_stack, ld_flags[OpCode::CSwap2.ld_index()]);

    // 2 ----- enforce constraints for high-degree operations --------------------------------------
    let hd_flags = current.hd_op_flags();

    enforce_push    (&mut evaluations,      old_stack, new_stack,      hd_flags[OpCode::Push.hd_index() ]);
    enforce_cmp     (&mut evaluations,      old_stack, new_stack,      hd_flags[OpCode::Cmp.hd_index()  ]);
    enforce_rescr   (&mut evaluations,      old_stack, new_stack, ark, hd_flags[OpCode::RescR.hd_index()]);

    // 3 ----- enforce constraints for composite operations ---------------------------------------

    // BEGIN and NOOP have "composite" opcodes where all 7 opcode bits are set to either 1s or 0s;
    // thus, the flags for these operations are computed separately by multiplying all opcodes;
    // this results in flag degree of 7 for each operation, but since both operations enforce the
    // same constraints (the stack doesn't change), higher degree terms cancel out, and we
    // end up with overall constraint degree of (6 + 1 = 7) for both operations.
    enforce_stack_copy(&mut evaluations, old_stack, new_stack, 0, current.begin_flag());
    enforce_stack_copy(&mut evaluations, old_stack, new_stack, 0, current.noop_flag());
    
    // 4 ----- copy evaluations into the result ---------------------------------------------------
    result.copy_from_slice(&evaluations[..result.len()]);
}

fn transpose_ark_constants(constants: Vec<Vec<u128>>, cycle_length: usize) -> Vec<[u128; 2 * HASH_STATE_WIDTH]>
{
    let mut values = Vec::new();
    for i in 0..cycle_length {
        values.push([field::ZERO; 2 * HASH_STATE_WIDTH]);
        for j in 0..(2 * HASH_STATE_WIDTH) {
            values[i][j] = constants[j][i];
        }
    }
    return values;
}