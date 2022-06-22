use ark_poly::univariate::DensePolynomial;
use ark_poly_commit::PolynomialCommitment;
use plonk_core::{
    constraint_system::StandardComposer,
    prelude::Proof,
    proof_system::{pi::PublicInputs, Prover, Verifier},
};

use crate::circuit::circuit_parameters::CircuitParameters;

pub struct BlindingCircuit<CP: CircuitParameters> {
    pub public_input: PublicInputs<CP::CurveBaseField>,
    pub proof: Proof<CP::CurveBaseField, CP::OuterCurvePC>,
    pub verifier: Verifier<CP::CurveBaseField, CP::Curve, CP::OuterCurvePC>,
    pub vk: <CP::OuterCurvePC as PolynomialCommitment<
        CP::CurveBaseField,
        DensePolynomial<CP::CurveBaseField>,
    >>::VerifierKey,
}

impl<CP: CircuitParameters> BlindingCircuit<CP> {
    pub fn precompute_prover(
        setup: &<<CP as CircuitParameters>::OuterCurvePC as PolynomialCommitment<
            CP::CurveBaseField,
            DensePolynomial<CP::CurveBaseField>,
        >>::UniversalParams,
        gadget: fn(
            &mut StandardComposer<CP::CurveBaseField, CP::Curve>,
            private_inputs: &[CP::CurveBaseField],
            public_inputs: &[CP::CurveBaseField],
        ),
        private_inputs: &[CP::CurveBaseField],
        public_inputs: &[CP::CurveBaseField],
    ) -> (
        // Prover
        Prover<CP::CurveBaseField, CP::Curve, CP::OuterCurvePC>,
        // CommitterKey
        <CP::OuterCurvePC as PolynomialCommitment<
            CP::CurveBaseField,
            DensePolynomial<CP::CurveBaseField>,
        >>::CommitterKey,
        // VerifierKey
        <CP::OuterCurvePC as PolynomialCommitment<
            CP::CurveBaseField,
            DensePolynomial<CP::CurveBaseField>,
        >>::VerifierKey,
        // PublicInput
        PublicInputs<CP::CurveBaseField>,
    ) {
        // Create a `Prover`
        // Set the circuit using `gadget`
        // Output `prover`, `vk`, `public_input`.

        let mut prover = Prover::<CP::CurveBaseField, CP::Curve, CP::OuterCurvePC>::new(b"demo");
        prover.key_transcript(b"key", b"additional seed information");
        gadget(prover.mut_cs(), private_inputs, public_inputs);
        let (ck, vk) = CP::OuterCurvePC::trim(
            setup,
            prover.circuit_bound().next_power_of_two() + 6,
            0,
            None,
        )
        .unwrap();
        let public_input = prover.mut_cs().get_pi().clone();

        (prover, ck, vk, public_input)
    }

    pub fn precompute_verifier(
        gadget: fn(
            &mut StandardComposer<CP::CurveBaseField, CP::Curve>,
            private_inputs: &[CP::CurveBaseField],
            public_inputs: &[CP::CurveBaseField],
        ),
        private_inputs: &[CP::CurveBaseField],
        public_inputs: &[CP::CurveBaseField],
    ) -> Verifier<CP::CurveBaseField, CP::Curve, CP::OuterCurvePC> {
        let mut verifier: Verifier<CP::CurveBaseField, CP::Curve, CP::OuterCurvePC> =
            Verifier::new(b"demo");
        verifier.key_transcript(b"key", b"additional seed information");
        gadget(verifier.mut_cs(), private_inputs, public_inputs);
        verifier
    }

    pub fn preprocess(
        prover: &mut Prover<CP::CurveBaseField, CP::Curve, CP::OuterCurvePC>,
        verifier: &mut Verifier<CP::CurveBaseField, CP::Curve, CP::OuterCurvePC>,
        ck: &<CP::OuterCurvePC as PolynomialCommitment<
            CP::CurveBaseField,
            DensePolynomial<CP::CurveBaseField>,
        >>::CommitterKey,
    ) {
        prover.preprocess(ck).unwrap();
        verifier.preprocess(ck).unwrap();
    }

    pub fn new(
        setup: &<<CP as CircuitParameters>::OuterCurvePC as PolynomialCommitment<
            CP::CurveBaseField,
            DensePolynomial<CP::CurveBaseField>,
        >>::UniversalParams,
        gadget: fn(
            &mut StandardComposer<CP::CurveBaseField, CP::Curve>,
            &[CP::CurveBaseField],
            &[CP::CurveBaseField],
        ),
        private_inputs: &[CP::CurveBaseField],
        public_inputs: &[CP::CurveBaseField],
    ) -> Self {
        // Given a gadget corresponding to a circuit, create all the computations for PBC related to the VP

        // Prover desc_vp
        let (mut prover, ck, vk, public_input) = Self::precompute_prover(setup, gadget, private_inputs, public_inputs);
        let mut verifier = Self::precompute_verifier(gadget, private_inputs, public_inputs);
        Self::preprocess(&mut prover, &mut verifier, &ck);

        // proof
        let proof = prover.prove(&ck).unwrap();

        Self {
            public_input,
            proof,
            verifier,
            vk,
        }
    }

    pub fn verify(&self) {
        self.verifier
            .verify(&self.proof, &self.vk, &self.public_input)
            .unwrap();
    }
}
