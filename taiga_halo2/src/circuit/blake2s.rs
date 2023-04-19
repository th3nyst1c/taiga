use ff::PrimeField;
use halo2_proofs::{
    arithmetic::Field,
    circuit::{AssignedCell, Chip, Layouter, Region},
    plonk::{Advice, Any, Column, ConstraintSystem, Error, Expression, Selector},
};
use pasta_curves::pallas;
use std::{convert::TryInto, marker::PhantomData};

// Blake2s constants
const IV: [u32; 8] = [
    0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
];

// The SIGMA constant in Blake2s is a 10x16 array that defines the message permutations in the algorithm. Each of the 10 rows corresponds to a round of the hashing process, and each of the 16 elements in the row determines the message block order.
const SIGMA: [[u8; 16]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

#[derive(Clone, Debug)]
pub struct Blake2sChip<F: Field> {
    config: Blake2sConfig,
    _marker: PhantomData<F>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Blake2sConfig {
    // Message block columns
    message: [Column<Advice>; 4],

    // Internal state columns
    v: [Column<Advice>; 4],

    // Working value columns
    t: [Column<Advice>; 2],

    // Constant columns
    constants: Column<Advice>,

    // Permutation columns
    sigma: Column<Advice>,

    // Selector columns for the S-box
    sbox: [Selector; 16],

    // Selector columns for controlling the message schedule and compression function
    round: Selector,
    message_schedule: Selector,
}

impl<F: Field> Chip<F> for Blake2sChip<F> {
    type Config = Blake2sConfig;
    type Loaded = ();

    fn config(&self) -> &Self::Config {
        &self.config
    }

    fn loaded(&self) -> &Self::Loaded {
        &()
    }
}

impl<F: Field> Blake2sChip<F> {
    pub fn construct(config: Blake2sConfig, _loaded: <Self as Chip<F>>::Loaded) -> Self {
        Self {
            config,
            _marker: PhantomData,
        }
    }

    fn xor(
        &self,
        x: &AssignedCell<pallas::Base, pallas::Base>,
        y: &AssignedCell<pallas::Base, pallas::Base>,
        region: &mut Region<'_, pallas::Base>,
        config: &Blake2sConfig,
        offset: usize,
    ) -> Result<AssignedCell<pallas::Base, pallas::Base>, Error> {
        let result_val = x
            .value()
            .zip(y.value())
            .map(|(x_val, y_val)| x_val + y_val - x_val * y_val);
        let result_cell =
            region.assign_advice(|| "xor", config.v[offset % 4], offset, || result_val)?;

        region.constrain_equal(x.cell(), result_cell.cell())?;
        region.constrain_equal(y.cell(), result_cell.cell())?;

        Ok(result_cell)
    }

    fn add(
        &self,
        x: &AssignedCell<pallas::Base, pallas::Base>,
        y: &AssignedCell<pallas::Base, pallas::Base>,
        region: &mut Region<'_, pallas::Base>,
        config: &Blake2sConfig,
        offset: usize,
    ) -> Result<AssignedCell<pallas::Base, pallas::Base>, Error> {
        let result_val = x.value().zip(y.value()).map(|(x_val, y_val)| x_val + y_val);
        let result_cell =
            region.assign_advice(|| "add", config.v[offset % 4], offset, || result_val)?;

        region.constrain_equal(x.cell(), result_cell.cell())?;
        region.constrain_equal(y.cell(), result_cell.cell())?;

        Ok(result_cell)
    }

    fn rotate(
        &self,
        cell: &AssignedCell<pallas::Base, pallas::Base>,
        region: &mut Region<'_, pallas::Base>,
        config: &Blake2sConfig,
        rotation: i32,
        offset: usize,
    ) -> Result<AssignedCell<pallas::Base, pallas::Base>, Error> {
        let rotated_value = cell.value().map(|v| {
            let pow_2: pallas::Base =
                pallas::Base::from(1u64 << (rotation as u64 % pallas::Base::NUM_BITS as u64));
            (v * pow_2) // % pallas::Base::MODULUS
        });
        let rotated_cell = region.assign_advice(
            || format!("rotate {}", rotation),
            config.v[offset % 4],
            offset,
            || rotated_value,
        )?;

        // Enforce the rotation constraint
        region.constrain_equal(cell.cell(), rotated_cell.cell())?;

        Ok(rotated_cell)
    }

    fn shift_right(
        &self,
        cell: &AssignedCell<pallas::Base, pallas::Base>,
        region: &mut Region<'_, pallas::Base>,
        config: &Blake2sConfig,
        shift: u32,
        offset: usize,
    ) -> Result<AssignedCell<pallas::Base, pallas::Base>, Error> {
        let divisor = pallas::Base::from(1u64 << shift);

        let shifted_value = cell
            .value()
            .map(|v| *v * divisor.invert().unwrap_or(pallas::Base::zero()));
        let shifted_cell = region.assign_advice(
            || format!("shift right {}", shift),
            config.v[offset % 4],
            offset,
            || shifted_value,
        )?;

        // Enforce the shift constraint
        region.constrain_equal(cell.cell(), shifted_cell.cell())?;

        Ok(shifted_cell)
    }

    fn g(
        &self,
        layouter: &mut impl Layouter<pallas::Base>,
        state: [AssignedCell<pallas::Base, pallas::Base>; 8],
        message: [AssignedCell<pallas::Base, pallas::Base>; 2],
        round: usize,
    ) -> Result<[AssignedCell<pallas::Base, pallas::Base>; 8], Error> {
        // Implement the G function
        layouter.assign_region(
            || "G function",
            |mut region| {
                // First mixing stage
                let a = self.add(&state[0], &state[4], &mut region, &self.config, round % 4)?;
                let a = self.add(&a, &message[0], &mut region, &self.config, round % 4)?;

                let d = self.xor(&state[3], &a, &mut region, &self.config, round % 4)?;
                let d = self.rotate(&d, &mut region, &self.config, -16, round % 4)?;

                let c = self.add(&state[2], &d, &mut region, &self.config, round % 4)?;

                let b = self.xor(&state[1], &c, &mut region, &self.config, round % 4)?;
                let b = self.rotate(&b, &mut region, &self.config, -12, round % 4)?;

                // Second mixing stage
                let a = self.add(&a, &b, &mut region, &self.config, round % 4)?;
                let a = self.add(&a, &message[1], &mut region, &self.config, round % 4)?;

                let d = self.xor(&d, &a, &mut region, &self.config, round % 4)?;
                let d = self.rotate(&d, &mut region, &self.config, -8, round % 4)?;

                let c = self.add(&c, &d, &mut region, &self.config, round % 4)?;

                let b = self.xor(&b, &c, &mut region, &self.config, round % 4)?;
                let b = self.rotate(&b, &mut region, &self.config, -7, round % 4)?;

                Ok([
                    a,
                    b,
                    c,
                    d,
                    state[4].clone(),
                    state[5].clone(),
                    state[6].clone(),
                    state[7].clone(),
                ])
            },
        )
    }

    fn message_schedule(
        &self,
        layouter: &mut impl Layouter<pallas::Base>,
        message_block: [AssignedCell<pallas::Base, pallas::Base>; 16],
    ) -> Result<[AssignedCell<pallas::Base, pallas::Base>; 16], Error> {
        // Implement the message schedule
        let mut message_schedule = Vec::with_capacity(64);

        // Copy the first 16 words of the message block into the message schedule
        for i in 0..16 {
            message_schedule.push(message_block[i].clone());
        }

        // Compute the remaining 48 words of the message schedule
        layouter.assign_region(
            || "message schedule",
            |mut region| {
                for i in 16..64 {
                    let s0 = self.xor(
                        &self.rotate(
                            &message_schedule[i - 15],
                            &mut region,
                            &self.config,
                            -7,
                            i % 4,
                        )?,
                        &self.rotate(
                            &message_schedule[i - 15],
                            &mut region,
                            &self.config,
                            -18,
                            i % 4,
                        )?,
                        &mut region,
                        &self.config,
                        i % 4,
                    )?;
                    let s0 = self.xor(
                        &s0,
                        &self.shift_right(
                            &message_schedule[i - 15],
                            &mut region,
                            &self.config,
                            3,
                            i % 4,
                        )?,
                        &mut region,
                        &self.config,
                        i % 4,
                    )?;

                    let s1 = self.xor(
                        &self.rotate(
                            &message_schedule[i - 2],
                            &mut region,
                            &self.config,
                            -17,
                            i % 4,
                        )?,
                        &self.rotate(
                            &message_schedule[i - 2],
                            &mut region,
                            &self.config,
                            -19,
                            i % 4,
                        )?,
                        &mut region,
                        &self.config,
                        i % 4,
                    )?;
                    let s1 = self.xor(
                        &s1,
                        &self.shift_right(
                            &message_schedule[i - 2],
                            &mut region,
                            &self.config,
                            10,
                            i % 4,
                        )?,
                        &mut region,
                        &self.config,
                        i % 4,
                    )?;

                    let sum = self.add(
                        &message_schedule[i - 16],
                        &s0,
                        &mut region,
                        &self.config,
                        i % 4,
                    )?;
                    let new_word = self.add(
                        &sum,
                        &message_schedule[i - 7],
                        &mut region,
                        &self.config,
                        i % 4,
                    )?;
                    let new_word = self.add(&new_word, &s1, &mut region, &self.config, i % 4)?;

                    message_schedule.push(new_word);
                }

                Ok(())
            },
        )?;

        // Create an array with the first 16 words of the updated message schedule
        Ok([
            message_schedule[0].clone(),
            message_schedule[1].clone(),
            message_schedule[2].clone(),
            message_schedule[3].clone(),
            message_schedule[4].clone(),
            message_schedule[5].clone(),
            message_schedule[6].clone(),
            message_schedule[7].clone(),
            message_schedule[8].clone(),
            message_schedule[9].clone(),
            message_schedule[10].clone(),
            message_schedule[11].clone(),
            message_schedule[12].clone(),
            message_schedule[13].clone(),
            message_schedule[14].clone(),
            message_schedule[15].clone(),
        ])
    }

    fn compression_function(
        &self,
        layouter: &mut impl Layouter<pallas::Base>,
        state: [AssignedCell<pallas::Base, pallas::Base>; 8],
        message_block: [AssignedCell<pallas::Base, pallas::Base>; 16],
    ) -> Result<[AssignedCell<pallas::Base, pallas::Base>; 8], Error> {
        // 1. Compute the message schedule
        let message_schedule = self.message_schedule(layouter, message_block)?;

        // 2. Perform the 10 rounds of the Blake2s compression function
        let mut current_state = state.clone();
        for round in 0..10 {
            for g_index in 0..8 {
                let idx = SIGMA[round][2 * g_index];
                let idx1 = SIGMA[round][2 * g_index + 1];

                current_state = self.g(
                    layouter,
                    current_state,
                    [
                        message_schedule[idx as usize].clone(),
                        message_schedule[idx1 as usize].clone(),
                    ],
                    round,
                )?;
            }
        }

        // 3. Finalize the state
        let final_state = layouter.assign_region(
            || "Finalize state",
            |mut region| {
                let mut final_state = Vec::with_capacity(8);
                for i in 0..8 {
                    final_state.push(self.add(
                        &state[i],
                        &current_state[i],
                        &mut region,
                        &self.config,
                        i % 4,
                    )?);
                }
                Ok([
                    final_state[0].clone(),
                    final_state[1].clone(),
                    final_state[2].clone(),
                    final_state[3].clone(),
                    final_state[4].clone(),
                    final_state[5].clone(),
                    final_state[6].clone(),
                    final_state[7].clone(),
                ])
            },
        )?;

        Ok(final_state)
    }

    //     fn compression_function(
    //         &self,
    //         layouter: &mut impl Layouter<pallas::Base>,
    //         initial_state: [AssignedCell<pallas::Base, pallas::Base>; 8],
    //         message_blocks: &[[AssignedCell<pallas::Base, pallas::Base>; 16]],
    //     ) -> Result<[AssignedCell<pallas::Base, pallas::Base>; 8], Error> {
    //         let mut state = initial_state;

    //         for (round_idx, message_block) in message_blocks.iter().enumerate() {
    //             // 1. Apply the message schedule
    //             let scheduled_message = self.message_schedule(layouter, *message_block)?;

    //             // 2. Execute the G function for each column
    //             for col_idx in 0..4 {
    //                 let input_state = [
    //                     state[col_idx * 2],
    //                     state[col_idx * 2 + 1],
    //                     state[(col_idx * 2 + 2) % 8],
    //                     state[(col_idx * 2 + 3) % 8],
    //                 ];

    //                 state = self.g(layouter, input_state, scheduled_message, round_idx)?;
    //             }
    //             // 3. Finalize the state
    //             let mut final_state = [];
    //             for i in 0..8 {
    //                 layouter.assign_region(
    //                     || "Finalize state",
    //                     |mut region| {
    //                         let row_offset = 0;

    //                         let lc_initial_state = region.assign_advice(
    //                             || format!("LC initial_state[{}]", i),
    //                             self.config.v[i % 4],
    //                             row_offset,
    //                             || initial_state[i].value(),
    //                         )?;

    //                         let lc_state = region.assign_advice(
    //                             || format!("LC state[{}]", i),
    //                             self.config.v[(i + 1) % 4],
    //                             row_offset,
    //                             || state[i].value(),
    //                         )?;

    //                         region.constrain_equal(initial_state[i].cell(), lc_initial_state.cell())?;
    //                         region.constrain_equal(state[i].cell(), lc_state.cell())?;

    //                         let final_val = Expression::from(lc_initial_state)
    //                             + Expression::from(lc_state.value())
    //                             - (Expression::from(initial_state[i].value())
    //                                 * Expression::from(state[i].value()));

    //                         let final_cell = region.assign_advice(
    //                             || format!("final_state[{}]", i),
    //                             self.config.v[(i + 2) % 4],
    //                             row_offset,
    //                             || {
    //                                 final_val.evaluate(
    //                                     &|_| pallas::Base::zero(),
    //                                     &|_| pallas::Base::zero(),
    //                                     &|_| pallas::Base::zero(),
    //                                     &|query| {
    //                                         if let Some(value) =
    //                                             region.get_assigned_value(query.column, query.at)
    //                                         {
    //                                             value
    //                                         } else {
    //                                             pallas::Base::zero()
    //                                         }
    //                                     },
    //                                     &|_| pallas::Base::zero(),
    //                                     &|value| -value,
    //                                     &|a, b| a + b,
    //                                     &|a, b| a * b,
    //                                     &|a, _| a,
    //                                 )
    //                             },
    //                         )?;

    //                         final_state[i] = AssignedCell {
    //                             cell: final_cell,
    //                             value: region.get_assigned_value(final_cell),
    //                             _marker: Default::default()
    //                         };

    //                         Ok(())
    //                     },
    //                 )?;
    //             }

    //             Ok(final_state)
    //         }
    //     }
}
// const BLOCK_SIZE: usize = 64; // block size in bytes
// const ROUND_COUNT: usize = 10; // number of rounds

// pub(crate) struct Blake2sCircuit {
//     message: [u8; BLOCK_SIZE],
// }

// pub(crate) struct Blake2sConfig {
//     message_column: Column<Advice>,
//     state_columns: [Column<Advice>; 8],
//     round_constants: Column<Fixed>,
//     sbox_selector: Selector,
// }

// impl Circuit<Fp> for Blake2sCircuit {
//     fn configure(meta: &mut ConstraintSystem<Fp>) -> Self::Config {
//         // Define columns and selectors
//         // ... (implementation depends on the design of the circuit)

//         // Define constraints
//         // ... (implementation depends on the design of the circuit)
//     }

//     fn synthesize(
//         &self,
//         cs: &mut impl plonk::Assignment<Fp>,
//         config: Self::Config,
//     ) -> Result<(), Error> {
//         // Load the message into the circuit
//     }
// }
