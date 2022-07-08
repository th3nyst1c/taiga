use crate::circuit::circuit_parameters::CircuitParameters;
use crate::circuit::gadgets::field_addition::field_addition_gadget;
use crate::circuit::integrity::{
    ValidityPredicateInputNoteVariables, ValidityPredicateOuputNoteVariables,
};
use crate::circuit::validity_predicate::{ValidityPredicate, NUM_NOTE};
use crate::note::Note;
use plonk_core::{circuit::Circuit, constraint_system::StandardComposer, prelude::Error};

// BalanceValidityPredicate have a custom constraint with a + b = c,
// in which a, b are private inputs and c is a public input.
pub struct BalanceValidityPredicate<CP: CircuitParameters> {
    // basic "private" inputs to the VP
    pub input_notes: [Note<CP>; NUM_NOTE],
    pub output_notes: [Note<CP>; NUM_NOTE],
}

impl<CP> ValidityPredicate<CP> for BalanceValidityPredicate<CP>
where
    CP: CircuitParameters,
{
    fn get_input_notes(&self) -> &[Note<CP>; NUM_NOTE] {
        &self.input_notes
    }

    fn get_output_notes(&self) -> &[Note<CP>; NUM_NOTE] {
        &self.output_notes
    }

    fn custom_constraints(
        &self,
        composer: &mut StandardComposer<CP::CurveScalarField, CP::InnerCurve>,
        input_note_variables: &[ValidityPredicateInputNoteVariables],
        output_note_variables: &[ValidityPredicateOuputNoteVariables],
    ) -> Result<(), Error> {
        // sum of the input note values
        let mut balance_input_var = composer.zero_var();
        for note_var in input_note_variables {
            balance_input_var =
                field_addition_gadget::<CP>(composer, balance_input_var, note_var.value);
        }
        // sum of the output note values
        let mut balance_output_var = composer.zero_var();
        for note_var in output_note_variables {
            balance_output_var =
                field_addition_gadget::<CP>(composer, balance_output_var, note_var.value);
        }
        composer.assert_equal(balance_input_var, balance_output_var);
        Ok(())
    }
}

impl<CP> Circuit<CP::CurveScalarField, CP::InnerCurve> for BalanceValidityPredicate<CP>
where
    CP: CircuitParameters,
{
    const CIRCUIT_ID: [u8; 32] = [0x00; 32];

    // Default implementation
    fn gadget(
        &mut self,
        composer: &mut StandardComposer<CP::CurveScalarField, CP::InnerCurve>,
    ) -> Result<(), Error> {
        self.gadget_vp(composer)
    }

    fn padded_circuit_size(&self) -> usize {
        1 << 17
    }
}

#[test]
fn test_balance_vp_example() {
    use crate::circuit::circuit_parameters::PairingCircuitParameters as CP;
    type Fr = <CP as CircuitParameters>::CurveScalarField;
    type P = <CP as CircuitParameters>::InnerCurve;
    type PC = <CP as CircuitParameters>::CurvePC;
    use ark_poly_commit::PolynomialCommitment;
    use ark_std::test_rng;
    use plonk_core::circuit::{verify_proof, VerifierData};

    let mut rng = test_rng();
    let input_notes = [(); NUM_NOTE].map(|_| Note::<CP>::dummy(&mut rng));
    let output_notes = input_notes; // for a right balance
    let mut balance_vp = BalanceValidityPredicate {
        input_notes,
        output_notes,
    };

    // Generate CRS
    let pp = PC::setup(balance_vp.padded_circuit_size(), None, &mut rng).unwrap();

    // Compile the circuit
    let (pk_p, vk) = balance_vp.compile::<PC>(&pp).unwrap();

    // Prover
    let (proof, pi) = balance_vp.gen_proof::<PC>(&pp, pk_p, b"Test").unwrap();

    // Verifier
    let verifier_data = VerifierData::new(vk, pi);
    verify_proof::<Fr, P, PC>(&pp, verifier_data.key, &proof, &verifier_data.pi, b"Test").unwrap();
}