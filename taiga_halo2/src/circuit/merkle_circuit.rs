use halo2_gadgets::{
    poseidon::{
        primitives as poseidon, primitives::ConstantLength, Hash as PoseidonHash,
        Pow5Chip as PoseidonChip, Pow5Config as PoseidonConfig,
    },
    utilities::cond_swap::{CondSwapChip, CondSwapConfig, CondSwapInstructions},
};
use halo2_proofs::{
    circuit::{AssignedCell, Chip, Layouter, Value},
    plonk::{Advice, Column, ConstraintSystem, Error},
};
use pasta_curves::pallas;

/// MerkleTreeChip based on poseidon hash.
#[derive(Clone, Debug)]
pub struct MerklePoseidonConfig {
    advices: [Column<Advice>; 5],
    cond_swap_config: CondSwapConfig,
    poseidon_config: PoseidonConfig<CP::CurveScalarField, 3, 2>,
}

#[derive(Clone, Debug)]
pub struct MerklePoseidonChip {
    config: MerklePoseidonConfig,
}

impl Chip<CP::CurveScalarField> for MerklePoseidonChip {
    type Config = MerklePoseidonConfig;
    type Loaded = ();

    fn config(&self) -> &Self::Config {
        &self.config
    }

    fn loaded(&self) -> &Self::Loaded {
        &()
    }
}

impl MerklePoseidonChip {
    pub fn configure(
        meta: &mut ConstraintSystem<CP::CurveScalarField>,
        advices: [Column<Advice>; 5],
        poseidon_config: PoseidonConfig<CP::CurveScalarField, 3, 2>,
    ) -> MerklePoseidonConfig {
        let cond_swap_config = CondSwapChip::configure(meta, advices);

        MerklePoseidonConfig {
            advices,
            cond_swap_config,
            poseidon_config,
        }
    }

    pub fn construct(config: MerklePoseidonConfig) -> Self {
        MerklePoseidonChip { config }
    }
}

#[allow(clippy::type_complexity)]
pub fn merkle_poseidon_gadget(
    mut layouter: impl Layouter<CP::CurveScalarField>,
    chip: MerklePoseidonChip,
    note_x: AssignedCell<CP::CurveScalarField, CP::CurveScalarField>,
    merkle_path: &[(CP::CurveScalarField, bool)],
) -> Result<AssignedCell<CP::CurveScalarField, CP::CurveScalarField>, Error> {
    fn swap(
        merkle_chip: &MerklePoseidonChip,
        layouter: impl Layouter<CP::CurveScalarField>,
        pair: (
            AssignedCell<CP::CurveScalarField, CP::CurveScalarField>,
            Value<CP::CurveScalarField>,
        ),
        swap: Value<bool>,
    ) -> Result<
        (
            AssignedCell<CP::CurveScalarField, CP::CurveScalarField>,
            AssignedCell<CP::CurveScalarField, CP::CurveScalarField>,
        ),
        Error,
    > {
        let config = merkle_chip.config().cond_swap_config.clone();
        let chip = CondSwapChip::<CP::CurveScalarField>::construct(config);
        chip.swap(layouter, pair, swap)
    }

    let mut cur = note_x;
    for e in merkle_path.iter() {
        let pair = {
            let pair = (cur, Value::known(e.0));
            swap(
                &chip,
                layouter.namespace(|| "merkle swap"),
                pair,
                Value::known(e.1),
            )?
        };
        let poseidon_message = [pair.0, pair.1];

        let poseidon_chip = PoseidonChip::construct(chip.config().poseidon_config.clone());
        let poseidon_hasher =
            PoseidonHash::<_, _, poseidon::P128Pow5T3, ConstantLength<2>, 3, 2>::init(
                poseidon_chip,
                layouter.namespace(|| "Poseidon init"),
            )?;
        cur = poseidon_hasher.hash(
            layouter.namespace(|| "merkle poseidon hash"),
            poseidon_message,
        )?;
    }

    Ok(cur)
}

#[test]
fn test_halo2_merkle_circuit() {
    use crate::circuit::gadgets::assign_free_advice;
    use crate::constant::TAIGA_COMMITMENT_TREE_DEPTH;
    use crate::merkle_tree::{MerklePath, Node};
    use ff::Field;
    use halo2_proofs::{
        circuit::{Layouter, SimpleFloorPlanner, Value},
        dev::MockProver,
        plonk::{Circuit, ConstraintSystem, Error},
    };
    use rand::rngs::OsRng;

    #[derive(Default)]
    struct MyCircuit {
        leaf: CP::CurveScalarField,
        merkle_path: MerklePath,
    }

    impl Circuit<CP::CurveScalarField> for MyCircuit {
        type Config = MerklePoseidonConfig;
        type FloorPlanner = SimpleFloorPlanner;

        fn without_witnesses(&self) -> Self {
            Self::default()
        }

        fn configure(meta: &mut ConstraintSystem<CP::CurveScalarField>) -> Self::Config {
            let advices = [
                meta.advice_column(),
                meta.advice_column(),
                meta.advice_column(),
                meta.advice_column(),
                meta.advice_column(),
            ];
            for advice in advices.iter() {
                meta.enable_equality(*advice);
            }
            let cond_swap_config = CondSwapChip::configure(meta, advices);

            let state = (0..3).map(|_| meta.advice_column()).collect::<Vec<_>>();
            let partial_sbox = meta.advice_column();
            let rc_a = (0..3).map(|_| meta.fixed_column()).collect::<Vec<_>>();
            let rc_b = (0..3).map(|_| meta.fixed_column()).collect::<Vec<_>>();
            meta.enable_constant(rc_b[0]);
            let poseidon_config = PoseidonChip::configure::<poseidon::P128Pow5T3>(
                meta,
                state.try_into().unwrap(),
                partial_sbox,
                rc_a.try_into().unwrap(),
                rc_b.try_into().unwrap(),
            );

            Self::Config {
                advices,
                cond_swap_config,
                poseidon_config,
            }
        }

        fn synthesize(
            &self,
            config: Self::Config,
            mut layouter: impl Layouter<CP::CurveScalarField>,
        ) -> Result<(), Error> {
            // Witness leaf
            let leaf = assign_free_advice(
                layouter.namespace(|| "witness leaf"),
                config.advices[0],
                Value::known(self.leaf),
            )?;

            let merkle_chip = MerklePoseidonChip::construct(config.clone());

            let root = merkle_poseidon_gadget(
                layouter.namespace(|| "poseidon merkle"),
                merkle_chip,
                leaf,
                &self.merkle_path.get_path(),
            )?;

            let expected_root = {
                let root = self.merkle_path.root(Node::new(self.leaf)).inner();
                assign_free_advice(
                    layouter.namespace(|| "witness leaf"),
                    config.advices[0],
                    Value::known(root),
                )?
            };
            layouter.assign_region(
                || "constrain result",
                |mut region| region.constrain_equal(root.cell(), expected_root.cell()),
            )
        }
    }

    let mut rng = OsRng;

    let leaf = CP::CurveScalarField::random(rng);
    let merkle_path = MerklePath::dummy(&mut rng, TAIGA_COMMITMENT_TREE_DEPTH);

    let circuit = MyCircuit { leaf, merkle_path };

    let prover = MockProver::run(11, &circuit, vec![]).unwrap();
    assert_eq!(prover.verify(), Ok(()))
}
