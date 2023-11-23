use crate::circuit::gadgets::poseidon_hash::poseidon_hash_gadget;
use group::ff::PrimeField;
use halo2_gadgets::{
    poseidon::Pow5Config as PoseidonConfig,
    utilities::{bool_check, lookup_range_check::LookupRangeCheckConfig},
};
use halo2_proofs::{
    circuit::{AssignedCell, Layouter},
    plonk::{Advice, Column, ConstraintSystem, Constraints, Error, Selector},
    poly::Rotation,
};
use pasta_curves::pallas;

/// compose = is_merkle_checked(bool) * 2^128 + quantity(64 bits)
#[derive(Clone, Debug)]
struct ComposeMerkleCheckQuantity {
    q_compose: Selector,
    col_l: Column<Advice>,
    col_m: Column<Advice>,
    col_r: Column<Advice>,
}

impl ComposeMerkleCheckQuantity {
    fn configure(
        meta: &mut ConstraintSystem<pallas::Base>,
        col_l: Column<Advice>,
        col_m: Column<Advice>,
        col_r: Column<Advice>,
        two_pow_128: pallas::Base,
    ) -> Self {
        let q_compose = meta.selector();

        meta.create_gate("Compose is_merkle_checked and quantity", |meta| {
            let q_compose = meta.query_selector(q_compose);

            let compose_is_merkle_checked_and_quantity = meta.query_advice(col_l, Rotation::cur());
            let is_merkle_checked = meta.query_advice(col_m, Rotation::cur());
            let quantity = meta.query_advice(col_r, Rotation::cur());

            // e = quantity + (2^128) * is_merkle_checked
            let composition_check = compose_is_merkle_checked_and_quantity
                - (quantity + is_merkle_checked.clone() * two_pow_128);

            Constraints::with_selector(
                q_compose,
                [
                    (
                        "bool_check is_merkle_checked",
                        bool_check(is_merkle_checked),
                    ),
                    ("composition", composition_check),
                ],
            )
        });

        Self {
            q_compose,
            col_l,
            col_m,
            col_r,
        }
    }

    fn assign(
        &self,
        layouter: &mut impl Layouter<pallas::Base>,
        is_merkle_checked: &AssignedCell<pallas::Base, pallas::Base>,
        quantity: &AssignedCell<pallas::Base, pallas::Base>,
    ) -> Result<AssignedCell<pallas::Base, pallas::Base>, Error> {
        layouter.assign_region(
            || "Compose is_merkle_checked and quantity",
            |mut region| {
                self.q_compose.enable(&mut region, 0)?;

                let compose = is_merkle_checked.value().zip(quantity.value()).map(
                    |(is_merkle_checked, quantity)| {
                        quantity + is_merkle_checked * pallas::Base::from_u128(1 << 64).square()
                    },
                );
                is_merkle_checked.copy_advice(
                    || "is_merkle_checked",
                    &mut region,
                    self.col_m,
                    0,
                )?;
                quantity.copy_advice(|| "quantity", &mut region, self.col_r, 0)?;

                region.assign_advice(|| "compose", self.col_l, 0, || compose)
            },
        )
    }
}

#[derive(Clone, Debug)]
pub struct ResourceCommitConfig {
    compose_config: ComposeMerkleCheckQuantity,
    poseidon_config: PoseidonConfig<pallas::Base, 3, 2>,
    lookup_config: LookupRangeCheckConfig<pallas::Base, 10>,
}

#[derive(Clone, Debug)]
pub struct ResourceCommitChip {
    config: ResourceCommitConfig,
}

impl ResourceCommitChip {
    pub fn configure(
        meta: &mut ConstraintSystem<pallas::Base>,
        advices: [Column<Advice>; 3],
        poseidon_config: PoseidonConfig<pallas::Base, 3, 2>,
        lookup_config: LookupRangeCheckConfig<pallas::Base, 10>,
    ) -> ResourceCommitConfig {
        let two_pow_128 = pallas::Base::from_u128(1 << 64).square();
        let compose_config = ComposeMerkleCheckQuantity::configure(
            meta,
            advices[0],
            advices[1],
            advices[2],
            two_pow_128,
        );

        ResourceCommitConfig {
            compose_config,
            poseidon_config,
            lookup_config,
        }
    }

    pub fn construct(config: ResourceCommitConfig) -> Self {
        ResourceCommitChip { config }
    }

    pub fn get_poseidon_config(&self) -> PoseidonConfig<pallas::Base, 3, 2> {
        self.config.poseidon_config.clone()
    }

    pub fn get_lookup_config(&self) -> &LookupRangeCheckConfig<pallas::Base, 10> {
        &self.config.lookup_config
    }
}

#[allow(clippy::too_many_arguments)]
pub fn resource_commit(
    mut layouter: impl Layouter<pallas::Base>,
    chip: ResourceCommitChip,
    app_vp: AssignedCell<pallas::Base, pallas::Base>,
    label: AssignedCell<pallas::Base, pallas::Base>,
    value: AssignedCell<pallas::Base, pallas::Base>,
    npk: AssignedCell<pallas::Base, pallas::Base>,
    nonce: AssignedCell<pallas::Base, pallas::Base>,
    psi: AssignedCell<pallas::Base, pallas::Base>,
    quantity: AssignedCell<pallas::Base, pallas::Base>,
    is_merkle_checked: AssignedCell<pallas::Base, pallas::Base>,
    rcm: AssignedCell<pallas::Base, pallas::Base>,
) -> Result<AssignedCell<pallas::Base, pallas::Base>, Error> {
    // Compose the quantity and is_merkle_checked to one field in order to save one poseidon absorb
    let compose_is_merkle_checked_and_quantity =
        chip.config
            .compose_config
            .assign(&mut layouter, &is_merkle_checked, &quantity)?;

    // resource commitment
    let poseidon_message = [
        app_vp,
        label,
        value,
        npk,
        nonce,
        psi,
        compose_is_merkle_checked_and_quantity,
        rcm,
    ];
    poseidon_hash_gadget(
        chip.config.poseidon_config,
        layouter.namespace(|| "resource commitment"),
        poseidon_message,
    )
}
