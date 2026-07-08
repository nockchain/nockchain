use p3_baby_bear::BabyBear;
use p3_circuit::builder::CircuitBuilder;
use p3_circuit::ops::poseidon1_perm::{
    KoalaBearD1Width16 as P1KoalaBearD1Width16, Poseidon1PermCallBase,
};
use p3_circuit::ops::poseidon2_perm::{GoldilocksD2Width8, Poseidon2PermCallBase};
use p3_circuit::ops::{
    KoalaBearD1Width16, Poseidon1Config, Poseidon2Config, generate_poseidon1_trace,
    generate_poseidon2_trace, generate_recompose_trace,
};
use p3_field::extension::QuinticTrinomialExtensionField;
use p3_field::{BasedVectorSpace, PrimeCharacteristicRing};
use p3_goldilocks::{Goldilocks, Poseidon2Goldilocks};
use p3_koala_bear::{KoalaBear, default_koalabear_poseidon1_16, default_koalabear_poseidon2_16};
use p3_symmetric::{CryptographicHasher, PaddingFreeSponge, Permutation};
use p3_test_utils::LiftPermToQuintic;
use p3_tip5_circuit_air::Tip5CircuitAir;

use super::*;
use crate::ConstraintProfile;
use crate::air::{ConstAir, PublicAir, RecomposeAir};
use crate::batch_stark_prover::{
    BABY_BEAR_MODULUS, KOALA_BEAR_MODULUS, Poseidon1Preprocessor, Poseidon2Preprocessor,
    poseidon1_air_builders_d5, poseidon1_table_provers_d5, poseidon2_air_builders,
    poseidon2_air_builders_d5, poseidon2_table_provers_d5, recompose_air_builders,
};
use crate::common::{NpoPreprocessor, get_airs_and_degrees_with_prep};
use crate::config::{
    self, BabyBearConfig, GoldilocksConfig, GoldilocksTipsConfig, KoalaBearConfig,
};

#[test]
fn circuit_table_air_forwards_next_row_declarations() {
    type SC = GoldilocksTipsConfig;
    type F = Goldilocks;

    let const_air = CircuitTableAir::<SC, 2>::Const(ConstAir::<F, 2>::new_with_preprocessed(
        1,
        vec![F::ZERO; 2],
    ));
    assert!(const_air.main_next_row_columns().is_empty());
    assert!(const_air.preprocessed_next_row_columns().is_empty());

    let public_air = CircuitTableAir::<SC, 2>::Public(PublicAir::<F, 2>::new_with_preprocessed(
        1,
        1,
        vec![F::ZERO; 2],
    ));
    assert!(public_air.main_next_row_columns().is_empty());
    assert!(public_air.preprocessed_next_row_columns().is_empty());

    let recompose_air = RecomposeAir::<F, 2>::new_with_preprocessed(1, vec![F::ZERO; 2], 1, false);
    let recompose_dynamic = DynamicAirEntry::<SC>::new(Box::new(recompose_air.clone()));
    assert!(recompose_dynamic.main_next_row_columns().is_empty());
    assert!(recompose_dynamic.preprocessed_next_row_columns().is_empty());
    let recompose_wrapped = CircuitTableAir::<SC, 2>::Dynamic(recompose_dynamic);
    assert!(recompose_wrapped.main_next_row_columns().is_empty());
    assert!(recompose_wrapped.preprocessed_next_row_columns().is_empty());

    let tip5_dynamic = DynamicAirEntry::<SC>::new(Box::new(
        Tip5CircuitAir::<F, 2>::new_with_preprocessed(Vec::new(), 1),
    ));
    assert_eq!(
        tip5_dynamic.main_next_row_columns().len(),
        tip5_dynamic.width()
    );
    assert!(tip5_dynamic.preprocessed_next_row_columns().is_empty());
    let tip5_wrapped = CircuitTableAir::<SC, 2>::Dynamic(tip5_dynamic);
    assert_eq!(
        tip5_wrapped.main_next_row_columns().len(),
        tip5_wrapped.width()
    );
    assert!(tip5_wrapped.preprocessed_next_row_columns().is_empty());
}

fn prove_babybear_public_plus_const(
    constant: u64,
    extra_addend: Option<u64>,
) -> (
    BatchStarkProver<BabyBearConfig>,
    BatchStarkProof<BabyBearConfig>,
    CircuitProverData<BabyBearConfig>,
) {
    let mut builder = CircuitBuilder::<BabyBear>::new();
    let x = builder.public_input();
    let expected = builder.public_input();
    let c = builder.define_const(BabyBear::from_u64(constant));
    let mut sum = builder.add(x, c);
    let mut expected_output = 7 + constant;
    if let Some(extra) = extra_addend {
        let c_extra = builder.define_const(BabyBear::from_u64(extra));
        sum = builder.add(sum, c_extra);
        expected_output += extra;
    }
    let diff = builder.sub(sum, expected);
    builder.assert_zero(diff);

    let circuit = builder.build().unwrap();
    let cfg = config::baby_bear();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<BabyBearConfig, _, 1>(
            &circuit,
            &TablePacking::default(),
            &[],
            &[],
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, log_degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &log_degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);

    let mut runner = circuit.runner();
    runner
        .set_public_inputs(&[BabyBear::from_u64(7), BabyBear::from_u64(expected_output)])
        .unwrap();
    let traces = runner.run().unwrap();

    let prover = BatchStarkProver::new(cfg);
    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    prover.verify_all_tables(&proof).unwrap();
    (prover, proof, circuit_prover_data)
}

fn prove_goldilocks_tip5_ext2_public_plus_const(
    extra_addend: Option<u64>,
) -> (
    BatchStarkProver<GoldilocksTipsConfig>,
    BatchStarkProof<GoldilocksTipsConfig>,
    CircuitProverData<GoldilocksTipsConfig>,
    GoldilocksTip5FriShape,
) {
    let (prover, proof, circuit_prover_data, fri_shape, _) =
        prove_goldilocks_tip5_ext2_public_plus_const_with_public_binding(extra_addend, 0);
    (prover, proof, circuit_prover_data, fri_shape)
}

fn prove_goldilocks_tip5_ext2_public_plus_const_with_public_binding(
    extra_addend: Option<u64>,
    public_binding_lanes: usize,
) -> (
    BatchStarkProver<GoldilocksTipsConfig>,
    BatchStarkProof<GoldilocksTipsConfig>,
    CircuitProverData<GoldilocksTipsConfig>,
    GoldilocksTip5FriShape,
    Vec<Goldilocks>,
) {
    const D: usize = 2;
    type Ext2 = BinomialExtensionField<Goldilocks, D>;

    let fri_shape = GoldilocksTip5FriShape::recursive_pure_query_60bit();
    let cfg = config::goldilocks_tip5_pure_query_60bit_with_fri_shape(
        fri_shape.log_blowup,
        fri_shape.num_queries,
        fri_shape.log_final_poly_len,
        fri_shape.max_log_arity,
        fri_shape.cap_height,
    );
    let table_packing = TablePacking::default()
        .with_public_binding_lanes(public_binding_lanes)
        .with_fri_params(fri_shape.log_final_poly_len, fri_shape.log_blowup);

    let mut builder = CircuitBuilder::<Ext2>::new();
    let x = builder.public_input();
    let expected = builder.public_input();
    let c =
        Ext2::from_basis_coefficients_slice(&[Goldilocks::from_u64(10), Goldilocks::from_u64(3)])
            .unwrap();
    let c_const = builder.define_const(c);
    let mut sum = builder.add(x, c_const);
    let mut expected_value =
        Ext2::from_basis_coefficients_slice(&[Goldilocks::from_u64(17), Goldilocks::from_u64(5)])
            .unwrap();
    if let Some(extra) = extra_addend {
        let extra_const = Ext2::from_basis_coefficients_slice(&[
            Goldilocks::from_u64(extra),
            Goldilocks::from_u64(extra + 1),
        ])
        .unwrap();
        let extra_target = builder.define_const(extra_const);
        sum = builder.add(sum, extra_target);
        expected_value += extra_const;
    }
    let diff = builder.sub(sum, expected);
    builder.assert_zero(diff);

    let circuit = builder.build().unwrap();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<GoldilocksTipsConfig, _, D>(
            &circuit,
            &table_packing,
            &[],
            &[],
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, log_degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &log_degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);

    let mut runner = circuit.runner();
    let x_value =
        Ext2::from_basis_coefficients_slice(&[Goldilocks::from_u64(7), Goldilocks::from_u64(2)])
            .unwrap();
    runner
        .set_public_inputs(&[x_value, expected_value])
        .unwrap();
    let public_values = [x_value, expected_value]
        .iter()
        .take(public_binding_lanes)
        .flat_map(<Ext2 as BasedVectorSpace<Goldilocks>>::as_basis_coefficients_slice)
        .copied()
        .collect::<Vec<_>>();
    let traces = runner.run().unwrap();

    let prover = BatchStarkProver::new(cfg).with_table_packing(table_packing);
    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    prover
        .verify_all_tables_with_public_values(&proof, &public_values)
        .unwrap();
    (prover, proof, circuit_prover_data, fri_shape, public_values)
}

#[test]
fn test_babybear_batch_stark_base_field() {
    let mut builder = CircuitBuilder::<BabyBear>::new();

    // x + 5*2 - 3 + (-1) == expected
    let x = builder.public_input();
    let expected = builder.public_input();
    let c5 = builder.define_const(BabyBear::from_u64(5));
    let c2 = builder.define_const(BabyBear::from_u64(2));
    let c3 = builder.define_const(BabyBear::from_u64(3));
    let neg_one = builder.define_const(BabyBear::NEG_ONE);

    let mul_result = builder.mul(c5, c2); // 10
    let add_result = builder.add(x, mul_result); // x + 10
    let sub_result = builder.sub(add_result, c3); // x + 7
    let final_result = builder.add(sub_result, neg_one); // x + 6

    let diff = builder.sub(final_result, expected);
    builder.assert_zero(diff);

    let circuit = builder.build().unwrap();
    let cfg = config::baby_bear();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<BabyBearConfig, _, 1>(
            &circuit,
            &TablePacking::default(),
            &[],
            &[],
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, log_degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &log_degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);

    let mut runner = circuit.runner();

    let x_val = BabyBear::from_u64(7);
    let expected_val = BabyBear::from_u64(13); // 7 + 10 - 3 - 1 = 13
    runner.set_public_inputs(&[x_val, expected_val]).unwrap();
    let traces = runner.run().unwrap();

    let prover = BatchStarkProver::new(cfg);

    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    assert_eq!(proof.ext_degree, 1);
    assert!(proof.w_binomial.is_none());

    assert!(prover.verify_all_tables(&proof).is_ok());
}

#[test]
fn test_table_lookups() {
    let mut builder = CircuitBuilder::<BabyBear>::new();
    let cfg = config::baby_bear();

    // x + 5*2 - 3 + (-1) == expected
    let x = builder.public_input();
    let expected = builder.public_input();
    let c5 = builder.define_const(BabyBear::from_u64(5));
    let c2 = builder.define_const(BabyBear::from_u64(2));
    let c3 = builder.define_const(BabyBear::from_u64(3));
    let neg_one = builder.define_const(BabyBear::NEG_ONE);

    let mul_result = builder.mul(c5, c2); // 10
    let add_result = builder.add(x, mul_result); // x + 10
    let sub_result = builder.sub(add_result, c3); // x + 7
    let final_result = builder.add(sub_result, neg_one); // x + 6

    let diff = builder.sub(final_result, expected);
    builder.assert_zero(diff);

    let circuit = builder.build().unwrap();
    let default_packing = TablePacking::default();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<BabyBearConfig, _, 1>(
            &circuit,
            &default_packing,
            &[],
            &[],
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, log_degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();

    let mut runner = circuit.runner();

    let x_val = BabyBear::from_u64(7);
    let expected_val = BabyBear::from_u64(13); // 7 + 10 - 3 - 1 = 13
    runner.set_public_inputs(&[x_val, expected_val]).unwrap();
    let traces = runner.run().unwrap();
    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &log_degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);

    let prover = BatchStarkProver::new(cfg);

    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    assert_eq!(proof.ext_degree, 1);
    assert!(proof.w_binomial.is_none());

    assert!(prover.verify_all_tables(&proof).is_ok());

    // Check that the generated lookups are correct and consistent across tables.
    for air in airs.iter() {
        let lookups = crate::batch_stark_prover::lookups_for_circuit_table_air(air);

        match air {
            CircuitTableAir::Const(_) => {
                assert_eq!(lookups.len(), 1, "Const table should have one lookup");
            }
            CircuitTableAir::Public(_) => {
                assert_eq!(lookups.len(), 1, "Public table should have one lookup");
            }
            CircuitTableAir::Alu(_) => {
                // ALU table sends 4 lookups per lane + 2 extra for double-step Horner a1/c1
                let expected_num_lookups = default_packing.alu_lanes() * 4
                    + 2 * (default_packing.horner_packed_steps() - 1);
                assert_eq!(
                    lookups.len(),
                    expected_num_lookups,
                    "ALU table should have {} lookups, found {}",
                    expected_num_lookups,
                    lookups.len()
                );
            }
            CircuitTableAir::Dynamic(_dynamic_air) => {
                assert!(
                    lookups.is_empty(),
                    "There is no dynamic table in this test, so no lookups expected"
                );
            }
        }
    }
}

#[test]
fn test_extension_field_batch_stark() {
    const D: usize = 4;
    type Ext4 = BinomialExtensionField<BabyBear, D>;
    let cfg = config::baby_bear();

    let mut builder = CircuitBuilder::<Ext4>::new();
    let x = builder.public_input();
    let y = builder.public_input();
    let z = builder.public_input();
    let expected = builder.public_input();
    let xy = builder.mul(x, y);
    let res = builder.add(xy, z);
    let diff = builder.sub(res, expected);
    builder.assert_zero(diff);

    let circuit = builder.build().unwrap();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<BabyBearConfig, _, D>(
            &circuit,
            &TablePacking::default(),
            &[],
            &[],
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();

    let mut runner = circuit.runner();
    let xv = Ext4::from_basis_coefficients_slice(&[
        BabyBear::from_u64(2),
        BabyBear::from_u64(3),
        BabyBear::from_u64(5),
        BabyBear::from_u64(7),
    ])
    .unwrap();
    let yv = Ext4::from_basis_coefficients_slice(&[
        BabyBear::from_u64(11),
        BabyBear::from_u64(13),
        BabyBear::from_u64(17),
        BabyBear::from_u64(19),
    ])
    .unwrap();
    let zv = Ext4::from_basis_coefficients_slice(&[
        BabyBear::from_u64(23),
        BabyBear::from_u64(29),
        BabyBear::from_u64(31),
        BabyBear::from_u64(37),
    ])
    .unwrap();
    let expected_v = xv * yv + zv;
    runner.set_public_inputs(&[xv, yv, zv, expected_v]).unwrap();
    let traces = runner.run().unwrap();

    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);
    let prover = BatchStarkProver::new(cfg);

    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    assert_eq!(proof.ext_degree, 4);
    // Ensure W was captured
    let expected_w = <Ext4 as ExtractBinomialW<BabyBear>>::extract_w().unwrap();
    assert_eq!(proof.w_binomial, Some(expected_w));
    prover.verify_all_tables(&proof).unwrap();
}

#[test]
fn test_extension_field_table_lookups() {
    const D: usize = 4;
    type Ext4 = BinomialExtensionField<BabyBear, D>;
    let cfg = config::baby_bear();

    let mut builder = CircuitBuilder::<Ext4>::new();
    let x = builder.public_input();
    let y = builder.public_input();
    let z = builder.public_input();
    let expected = builder.public_input();
    let xy = builder.mul(x, y);
    let res = builder.add(xy, z);
    let diff = builder.sub(res, expected);
    builder.assert_zero(diff);

    let circuit = builder.build().unwrap();
    let default_packing = TablePacking::default();
    let mut air_builders_ext4 = poseidon2_air_builders::<BabyBearConfig, 4>();
    air_builders_ext4.extend(recompose_air_builders::<BabyBearConfig, 4>(1, false));
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<BabyBearConfig, _, D>(
            &circuit,
            &default_packing,
            &[],
            &air_builders_ext4,
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, log_degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();

    let mut runner = circuit.runner();

    let xv = Ext4::from_basis_coefficients_slice(&[
        BabyBear::from_u64(2),
        BabyBear::from_u64(3),
        BabyBear::from_u64(5),
        BabyBear::from_u64(7),
    ])
    .unwrap();
    let yv = Ext4::from_basis_coefficients_slice(&[
        BabyBear::from_u64(11),
        BabyBear::from_u64(13),
        BabyBear::from_u64(17),
        BabyBear::from_u64(19),
    ])
    .unwrap();
    let zv = Ext4::from_basis_coefficients_slice(&[
        BabyBear::from_u64(23),
        BabyBear::from_u64(29),
        BabyBear::from_u64(31),
        BabyBear::from_u64(37),
    ])
    .unwrap();
    let expected_v = xv * yv + zv;
    runner.set_public_inputs(&[xv, yv, zv, expected_v]).unwrap();
    let traces = runner.run().unwrap();

    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &log_degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);

    let prover = BatchStarkProver::new(cfg);

    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    assert_eq!(proof.ext_degree, 4);
    // Ensure W was captured
    let expected_w = <Ext4 as ExtractBinomialW<BabyBear>>::extract_w().unwrap();
    assert_eq!(proof.w_binomial, Some(expected_w));

    assert!(prover.verify_all_tables(&proof).is_ok());

    // Check that the generated lookups are correct and consistent across tables.
    for air in airs.iter() {
        let lookups = crate::batch_stark_prover::lookups_for_circuit_table_air(air);

        match air {
            CircuitTableAir::Const(_) => {
                assert_eq!(lookups.len(), 1, "Const table should have one lookup");
            }
            CircuitTableAir::Public(_) => {
                assert_eq!(lookups.len(), 1, "Public table should have one lookup");
            }
            CircuitTableAir::Alu(_) => {
                // ALU table sends 4 lookups per lane + 2 extra for double-step Horner a1/c1
                let expected_num_lookups = default_packing.alu_lanes() * 4
                    + 2 * (default_packing.horner_packed_steps() - 1);
                assert_eq!(
                    lookups.len(),
                    expected_num_lookups,
                    "ALU table should have {} lookups, found {}",
                    expected_num_lookups,
                    lookups.len()
                );
            }
            CircuitTableAir::Dynamic(_dynamic_air) => {
                assert!(
                    lookups.is_empty(),
                    "There is no dynamic table in this test, so no lookups expected"
                );
            }
        }
    }
}

#[test]
fn test_koalabear_batch_stark_base_field() {
    let mut builder = CircuitBuilder::<KoalaBear>::new();
    let cfg = config::koala_bear();

    // a * b + 100 - (-1) == expected
    let a = builder.public_input();
    let b = builder.public_input();
    let expected = builder.public_input();
    let c = builder.define_const(KoalaBear::from_u64(100));
    let d = builder.define_const(KoalaBear::NEG_ONE);

    let ab = builder.mul(a, b);
    let add = builder.add(ab, c);
    let final_res = builder.sub(add, d);
    let diff = builder.sub(final_res, expected);
    builder.assert_zero(diff);

    let circuit = builder.build().unwrap();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<KoalaBearConfig, _, 1>(
            &circuit,
            &TablePacking::default(),
            &[],
            &[],
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let mut runner = circuit.runner();

    let a_val = KoalaBear::from_u64(42);
    let b_val = KoalaBear::from_u64(13);
    let expected_val = KoalaBear::from_u64(647); // 42*13 + 100 - (-1)
    runner
        .set_public_inputs(&[a_val, b_val, expected_val])
        .unwrap();
    let traces = runner.run().unwrap();

    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);
    let prover = BatchStarkProver::new(cfg);

    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    assert_eq!(proof.ext_degree, 1);
    assert!(proof.w_binomial.is_none());
    prover.verify_all_tables(&proof).unwrap();
}

#[test]
fn test_koalabear_batch_stark_extension_field_d8() {
    const D: usize = 8;
    type KBExtField = BinomialExtensionField<KoalaBear, D>;
    let mut builder = CircuitBuilder::<KBExtField>::new();
    let cfg = config::koala_bear();

    // x * y * z == expected
    let x = builder.public_input();
    let y = builder.public_input();
    let expected = builder.public_input();
    let z = builder.define_const(
        KBExtField::from_basis_coefficients_slice(&[
            KoalaBear::from_u64(1),
            KoalaBear::NEG_ONE,
            KoalaBear::from_u64(2),
            KoalaBear::from_u64(3),
            KoalaBear::from_u64(4),
            KoalaBear::from_u64(5),
            KoalaBear::from_u64(6),
            KoalaBear::from_u64(7),
        ])
        .unwrap(),
    );

    let xy = builder.mul(x, y);
    let xyz = builder.mul(xy, z);
    let diff = builder.sub(xyz, expected);
    builder.assert_zero(diff);

    let circuit = builder.build().unwrap();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<KoalaBearConfig, _, D>(
            &circuit,
            &TablePacking::default(),
            &[],
            &[],
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let mut runner = circuit.runner();

    let x_val = KBExtField::from_basis_coefficients_slice(&[
        KoalaBear::from_u64(4),
        KoalaBear::from_u64(6),
        KoalaBear::from_u64(8),
        KoalaBear::from_u64(10),
        KoalaBear::from_u64(12),
        KoalaBear::from_u64(14),
        KoalaBear::from_u64(16),
        KoalaBear::from_u64(18),
    ])
    .unwrap();
    let y_val = KBExtField::from_basis_coefficients_slice(&[
        KoalaBear::from_u64(12),
        KoalaBear::from_u64(14),
        KoalaBear::from_u64(16),
        KoalaBear::from_u64(18),
        KoalaBear::from_u64(20),
        KoalaBear::from_u64(22),
        KoalaBear::from_u64(24),
        KoalaBear::from_u64(26),
    ])
    .unwrap();
    let z_val = KBExtField::from_basis_coefficients_slice(&[
        KoalaBear::from_u64(1),
        KoalaBear::NEG_ONE,
        KoalaBear::from_u64(2),
        KoalaBear::from_u64(3),
        KoalaBear::from_u64(4),
        KoalaBear::from_u64(5),
        KoalaBear::from_u64(6),
        KoalaBear::from_u64(7),
    ])
    .unwrap();

    let expected_val = x_val * y_val * z_val;
    runner
        .set_public_inputs(&[x_val, y_val, expected_val])
        .unwrap();
    let traces = runner.run().unwrap();

    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);
    let prover = BatchStarkProver::new(cfg);

    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    assert_eq!(proof.ext_degree, 8);
    let expected_w = <KBExtField as ExtractBinomialW<KoalaBear>>::extract_w().unwrap();
    assert_eq!(proof.w_binomial, Some(expected_w));
    prover.verify_all_tables(&proof).unwrap();
}

#[test]
fn test_goldilocks_batch_stark_binomial_ext2() {
    const D: usize = 2;
    type Ext2 = BinomialExtensionField<Goldilocks, D>;
    let mut builder = CircuitBuilder::<Ext2>::new();
    let cfg = config::goldilocks();

    // x * y + z == expected
    let x = builder.public_input();
    let y = builder.public_input();
    let z = builder.public_input();
    let expected = builder.public_input();

    let xy = builder.mul(x, y);
    let res = builder.add(xy, z);
    let diff = builder.sub(res, expected);
    builder.assert_zero(diff);

    let circuit = builder.build().unwrap();
    let mut air_builders_ext2 = poseidon2_air_builders::<GoldilocksConfig, 2>();
    air_builders_ext2.extend(recompose_air_builders::<GoldilocksConfig, 2>(1, false));
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<GoldilocksConfig, _, D>(
            &circuit,
            &TablePacking::default(),
            &[],
            &air_builders_ext2,
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let mut runner = circuit.runner();

    let x_val =
        Ext2::from_basis_coefficients_slice(&[Goldilocks::from_u64(3), Goldilocks::NEG_ONE])
            .unwrap();
    let y_val =
        Ext2::from_basis_coefficients_slice(&[Goldilocks::from_u64(7), Goldilocks::from_u64(11)])
            .unwrap();
    let z_val =
        Ext2::from_basis_coefficients_slice(&[Goldilocks::from_u64(13), Goldilocks::from_u64(17)])
            .unwrap();
    let expected_val = x_val * y_val + z_val;

    runner
        .set_public_inputs(&[x_val, y_val, z_val, expected_val])
        .unwrap();
    let traces = runner.run().unwrap();

    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);
    let prover = BatchStarkProver::new(cfg);

    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    assert_eq!(proof.ext_degree, 2);
    let expected_w = <Ext2 as ExtractBinomialW<Goldilocks>>::extract_w().unwrap();
    assert_eq!(proof.w_binomial, Some(expected_w));
    prover.verify_all_tables(&proof).unwrap();
}

#[test]
fn test_goldilocks_poseidon2_circuit_build_and_run() {
    const D: usize = 2;
    type Ext2 = BinomialExtensionField<Goldilocks, D>;
    let mut rng = <rand::rngs::SmallRng as rand::SeedableRng>::seed_from_u64(0);
    let perm = Poseidon2Goldilocks::<8>::new_from_rng_128(&mut rng);
    let perm_for_hash = perm.clone();
    let mut builder = CircuitBuilder::<Ext2>::new();
    builder.enable_poseidon2_perm_width_8::<GoldilocksD2Width8, _>(
        generate_poseidon2_trace::<Ext2, GoldilocksD2Width8>,
        perm,
    );
    builder.enable_recompose::<Goldilocks>(generate_recompose_trace::<Goldilocks, Ext2>);
    let poseidon2_config = Poseidon2Config::GOLDILOCKS_D2_W8;
    let inputs = [builder.public_input(), builder.public_input()];
    let hash_outputs = builder
        .add_hash_slice(&poseidon2_config, &inputs, true)
        .unwrap();
    let expected0 = builder.public_input();
    let expected1 = builder.public_input();
    let sub0 = builder.sub(hash_outputs[0], expected0);
    builder.assert_zero(sub0);
    let sub1 = builder.sub(hash_outputs[1], expected1);
    builder.assert_zero(sub1);
    let circuit = builder.build().unwrap();
    let mut runner = circuit.runner();
    let in0 =
        Ext2::from_basis_coefficients_slice(&[Goldilocks::from_u64(1), Goldilocks::ZERO]).unwrap();
    let in1 =
        Ext2::from_basis_coefficients_slice(&[Goldilocks::from_u64(2), Goldilocks::ZERO]).unwrap();
    let hasher = PaddingFreeSponge::<Poseidon2Goldilocks<8>, 8, 4, 4>::new(perm_for_hash);
    let base_inputs = [
        Goldilocks::from_u64(1),
        Goldilocks::ZERO,
        Goldilocks::from_u64(2),
        Goldilocks::ZERO,
    ];
    let expected_hash = hasher.hash_iter(base_inputs);
    let out0 = Ext2::from_basis_coefficients_slice(&expected_hash[0..2]).unwrap();
    let out1 = Ext2::from_basis_coefficients_slice(&expected_hash[2..4]).unwrap();
    runner.set_public_inputs(&[in0, in1, out0, out1]).unwrap();
    let _traces = runner.run().unwrap();
}

#[test]
fn test_koalabear_modulus_constant() {
    // Verify KOALA_BEAR_MODULUS matches the actual KoalaBear field modulus.
    // The modulus p satisfies: from_u64(p) == 0 in the field.
    assert_eq!(
        KoalaBear::from_u64(KOALA_BEAR_MODULUS),
        KoalaBear::ZERO,
        "KOALA_BEAR_MODULUS (0x{:x}) does not match KoalaBear's actual modulus",
        KOALA_BEAR_MODULUS
    );

    // Verify the exact hex value (2130706433 = 0x7f000001).
    assert_eq!(KOALA_BEAR_MODULUS, 0x7f000001);
    assert_eq!(KOALA_BEAR_MODULUS, 2130706433);

    // Verify arithmetic at the modulus boundary with hardcoded expected values.
    // (p - 1) + 2 = 1 in the field
    let p_minus_1 = KoalaBear::from_u64(KOALA_BEAR_MODULUS - 1);
    assert_eq!(p_minus_1, KoalaBear::NEG_ONE);
    assert_eq!(p_minus_1 + KoalaBear::TWO, KoalaBear::ONE);

    // (p - 1) * (p - 1) = 1 in the field (since (-1) * (-1) = 1)
    assert_eq!(p_minus_1 * p_minus_1, KoalaBear::ONE);

    // Verify from_u64(p + 1) == 1
    assert_eq!(KoalaBear::from_u64(KOALA_BEAR_MODULUS + 1), KoalaBear::ONE);
}

#[test]
fn test_babybear_modulus_constant() {
    // Verify BABY_BEAR_MODULUS matches the actual BabyBear field modulus.
    assert_eq!(
        BabyBear::from_u64(BABY_BEAR_MODULUS),
        BabyBear::ZERO,
        "BABY_BEAR_MODULUS (0x{:x}) does not match BabyBear's actual modulus",
        BABY_BEAR_MODULUS
    );

    // Verify the exact hex value (2013265921 = 0x78000001).
    assert_eq!(BABY_BEAR_MODULUS, 0x78000001);
    assert_eq!(BABY_BEAR_MODULUS, 2013265921);

    // Verify arithmetic at the modulus boundary.
    let p_minus_1 = BabyBear::from_u64(BABY_BEAR_MODULUS - 1);
    assert_eq!(p_minus_1, BabyBear::NEG_ONE);
    assert_eq!(p_minus_1 + BabyBear::TWO, BabyBear::ONE);
    assert_eq!(BabyBear::from_u64(BABY_BEAR_MODULUS + 1), BabyBear::ONE);
}

#[test]
fn test_mul_only_circuit_padding() {
    // Circuit with only mul operations; ALU table still needs correct padding/lanes handling.
    let mut builder = CircuitBuilder::<BabyBear>::new();
    let cfg = config::baby_bear();

    let x = builder.public_input();
    let y = builder.public_input();

    // Only multiplication, no addition
    builder.mul(x, y);

    let circuit = builder.build().unwrap();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<BabyBearConfig, _, 1>(
            &circuit,
            &TablePacking::default(),
            &[],
            &[],
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let mut runner = circuit.runner();

    let x_val = BabyBear::from_u64(7);
    let y_val = BabyBear::from_u64(11);
    runner.set_public_inputs(&[x_val, y_val]).unwrap();
    let traces = runner.run().unwrap();

    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);

    let prover = BatchStarkProver::new(cfg);

    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    prover.verify_all_tables(&proof).unwrap();
}

#[test]
fn test_add_only_circuit_padding() {
    // Circuit with only add operations; ALU table still needs correct padding/lanes handling.
    let mut builder = CircuitBuilder::<BabyBear>::new();
    let cfg = config::baby_bear();

    let x = builder.public_input();
    let y = builder.public_input();
    let expected = builder.public_input();

    // Only addition, no multiplication
    let sum = builder.add(x, y);
    let diff = builder.sub(sum, expected);
    builder.assert_zero(diff);

    let circuit = builder.build().unwrap();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<BabyBearConfig, _, 1>(
            &circuit,
            &TablePacking::default(),
            &[],
            &[],
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let mut runner = circuit.runner();

    let x_val = BabyBear::from_u64(42);
    let y_val = BabyBear::from_u64(13);
    let expected_val = x_val + y_val;
    runner
        .set_public_inputs(&[x_val, y_val, expected_val])
        .unwrap();
    let traces = runner.run().unwrap();

    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);

    let prover = BatchStarkProver::new(cfg);

    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    prover.verify_all_tables(&proof).unwrap();
}

fn koala_ef5_lift(b: KoalaBear) -> QuinticTrinomialExtensionField<KoalaBear> {
    QuinticTrinomialExtensionField::<KoalaBear>::from_basis_coefficients_slice(&[
        b,
        KoalaBear::ZERO,
        KoalaBear::ZERO,
        KoalaBear::ZERO,
        KoalaBear::ZERO,
    ])
    .expect("basis slice")
}

#[test]
fn test_koalabear_quintic_trinomial_batch_stark_with_poseidon_d1() {
    const D: usize = 5;
    type EF5 = QuinticTrinomialExtensionField<KoalaBear>;

    // Must match KoalaBearD1Width16::round_constants() in poseidon2-circuit-air (not RNG-derived).
    let inner_perm = default_koalabear_poseidon2_16();
    let mut sponge0 = [KoalaBear::ZERO; 16];
    sponge0[0] = KoalaBear::from_u64(11);
    sponge0[1] = KoalaBear::from_u64(13);
    let sponge_out = inner_perm.permute(sponge0);
    let lift_perm = LiftPermToQuintic::new(inner_perm);

    let in0 = koala_ef5_lift(KoalaBear::from_u64(11));
    let in1 = koala_ef5_lift(KoalaBear::from_u64(13));
    let exp0 = koala_ef5_lift(sponge_out[0]);
    let exp1 = koala_ef5_lift(sponge_out[1]);

    let mut builder = CircuitBuilder::<EF5>::new();
    builder.enable_poseidon2_perm_base::<KoalaBearD1Width16, _>(
        generate_poseidon2_trace::<EF5, KoalaBearD1Width16>,
        lift_perm,
    );

    let in_a = builder.public_input();
    let in_b = builder.public_input();
    let mut perm_inputs: [Option<_>; 16] = [None; 16];
    perm_inputs[0] = Some(in_a);
    perm_inputs[1] = Some(in_b);
    let (_pid, hash_outputs) = builder
        .add_poseidon2_perm_base(&Poseidon2PermCallBase {
            config: Poseidon2Config::KOALA_BEAR_D1_W16,
            new_start: true,
            inputs: perm_inputs,
            // Only CTL-expose rate limbs that are wired into the rest of the circuit; unused
            // exposed outputs would leave WitnessChecks Receive contributions unmatched.
            out_ctl: [true; 8],
            return_all_outputs: false,
        })
        .unwrap();
    let e0 = builder.public_input();
    let e1 = builder.public_input();
    let h0_diff = builder.sub(hash_outputs[0].unwrap(), e0);
    let h1_diff = builder.sub(hash_outputs[1].unwrap(), e1);
    builder.assert_zero(h0_diff);
    builder.assert_zero(h1_diff);

    let circuit = builder.build().unwrap();
    let cfg = config::koala_bear();

    let npo_prep: Vec<Box<dyn NpoPreprocessor<KoalaBear>>> = vec![Box::new(Poseidon2Preprocessor)];
    let air_builders = poseidon2_air_builders_d5::<KoalaBearConfig>();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<KoalaBearConfig, _, D>(
            &circuit,
            &TablePacking::default(),
            &npo_prep,
            &air_builders,
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let mut runner = circuit.runner();

    runner.set_public_inputs(&[in0, in1, exp0, exp1]).unwrap();
    let traces = runner.run().unwrap();

    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);

    let mut prover = BatchStarkProver::new(cfg);
    for p in poseidon2_table_provers_d5(Poseidon2Config::KOALA_BEAR_D1_W16) {
        prover.register_table_prover(p);
    }

    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    assert_eq!(proof.ext_degree, D);
    assert!(proof.w_binomial.is_none());
    assert!(proof.alu_quintic_trinomial);
    prover.verify_all_tables(&proof).unwrap();
}

/// Two D=1 Poseidon rows in an EF5 circuit: the second row uses `new_start=false` so the full
/// 16-wide state chains through the compact D=1 preprocessed layout (sponge selectors, not Merkle).
#[test]
fn test_koalabear_quintic_trinomial_batch_stark_poseidon_d1_sponge_chain() {
    const D: usize = 5;
    type EF5 = QuinticTrinomialExtensionField<KoalaBear>;

    let inner_perm = default_koalabear_poseidon2_16();
    let mut sponge0 = [KoalaBear::ZERO; 16];
    sponge0[0] = KoalaBear::from_u64(11);
    sponge0[1] = KoalaBear::from_u64(13);
    let sponge_out0 = inner_perm.permute(sponge0);
    let sponge_out1 = inner_perm.permute(sponge_out0);
    let lift_perm = LiftPermToQuintic::new(inner_perm);

    let in0 = koala_ef5_lift(KoalaBear::from_u64(11));
    let in1 = koala_ef5_lift(KoalaBear::from_u64(13));
    let exp0 = koala_ef5_lift(sponge_out1[0]);
    let exp1 = koala_ef5_lift(sponge_out1[1]);

    let mut builder = CircuitBuilder::<EF5>::new();
    builder.enable_poseidon2_perm_base::<KoalaBearD1Width16, _>(
        generate_poseidon2_trace::<EF5, KoalaBearD1Width16>,
        lift_perm,
    );

    let in_a = builder.public_input();
    let in_b = builder.public_input();
    let mut perm0_inputs: [Option<_>; 16] = [None; 16];
    perm0_inputs[0] = Some(in_a);
    perm0_inputs[1] = Some(in_b);
    let (_pid0, _hash0) = builder
        .add_poseidon2_perm_base(&Poseidon2PermCallBase {
            config: Poseidon2Config::KOALA_BEAR_D1_W16,
            new_start: true,
            inputs: perm0_inputs,
            out_ctl: [false; 8],
            return_all_outputs: false,
        })
        .unwrap();

    let perm1_inputs: [Option<_>; 16] = [None; 16];
    let (_pid1, hash1_outputs) = builder
        .add_poseidon2_perm_base(&Poseidon2PermCallBase {
            config: Poseidon2Config::KOALA_BEAR_D1_W16,
            new_start: false,
            inputs: perm1_inputs,
            out_ctl: [true; 8],
            return_all_outputs: false,
        })
        .unwrap();
    let e0 = builder.public_input();
    let e1 = builder.public_input();
    let h0_diff = builder.sub(hash1_outputs[0].unwrap(), e0);
    let h1_diff = builder.sub(hash1_outputs[1].unwrap(), e1);
    builder.assert_zero(h0_diff);
    builder.assert_zero(h1_diff);

    let circuit = builder.build().unwrap();
    let cfg = config::koala_bear();

    let npo_prep: Vec<Box<dyn NpoPreprocessor<KoalaBear>>> = vec![Box::new(Poseidon2Preprocessor)];
    let air_builders = poseidon2_air_builders_d5::<KoalaBearConfig>();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<KoalaBearConfig, _, D>(
            &circuit,
            &TablePacking::default(),
            &npo_prep,
            &air_builders,
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let mut runner = circuit.runner();

    runner.set_public_inputs(&[in0, in1, exp0, exp1]).unwrap();
    let traces = runner.run().unwrap();

    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);

    let mut prover = BatchStarkProver::new(cfg);
    for p in poseidon2_table_provers_d5(Poseidon2Config::KOALA_BEAR_D1_W16) {
        prover.register_table_prover(p);
    }

    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    assert_eq!(proof.ext_degree, D);
    assert!(proof.w_binomial.is_none());
    assert!(proof.alu_quintic_trinomial);
    prover.verify_all_tables(&proof).unwrap();
}

#[test]
fn test_stark_serialization_round_trip() {
    let mut builder = CircuitBuilder::<BabyBear>::new();

    let x = builder.public_input();
    let expected = builder.public_input();
    let c5 = builder.define_const(BabyBear::from_u64(5));
    let c2 = builder.define_const(BabyBear::from_u64(2));
    let mul_result = builder.mul(c5, c2);
    let add_result = builder.add(x, mul_result);
    let diff = builder.sub(add_result, expected);
    builder.assert_zero(diff);

    let circuit = builder.build().unwrap();
    let cfg = config::baby_bear();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<BabyBearConfig, _, 1>(
            &circuit,
            &TablePacking::default(),
            &[],
            &[],
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, log_degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &log_degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);

    let mut runner = circuit.runner();
    let x_val = BabyBear::from_u64(7);
    let expected_val = BabyBear::from_u64(17); // 7 + 5*2 = 17
    runner.set_public_inputs(&[x_val, expected_val]).unwrap();
    let traces = runner.run().unwrap();

    let prover = BatchStarkProver::new(cfg);
    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();

    let original_preprocessed = proof
        .stark_common
        .preprocessed
        .as_ref()
        .expect("preprocessed binding must be present");
    let original_matrix_to_instance = original_preprocessed.matrix_to_instance.clone();
    let original_instances_len = original_preprocessed.instances.len();

    let bytes = postcard::to_allocvec(&proof).expect("serialize proof");
    let deserialized: BatchStarkProof<BabyBearConfig> =
        postcard::from_bytes(&bytes).expect("deserialize proof");

    let restored_preprocessed = deserialized
        .stark_common
        .preprocessed
        .as_ref()
        .expect("preprocessed binding must survive (de)serialization");
    assert_eq!(
        restored_preprocessed.matrix_to_instance,
        original_matrix_to_instance
    );
    assert_eq!(
        restored_preprocessed.instances.len(),
        original_instances_len
    );

    // Verification must succeed against the deserialized proof, relying only on the
    // proof's own `stark_common` for the preprocessed binding.
    prover
        .verify_all_tables(&deserialized)
        .expect("verification uses proof.stark_common");
}

#[test]
fn test_compact_preprocessed_ood_round_trip_uses_canonical_setup() {
    let (prover, proof, circuit_prover_data) = prove_babybear_public_plus_const(10, None);
    let full_bytes = postcard::to_allocvec(&proof).expect("serialize full proof");
    let compact = PreprocessedOodCompactBatchStarkProof::from_full(proof);
    let compact_bytes = postcard::to_allocvec(&compact).expect("serialize compact proof");
    assert!(
        compact_bytes.len() < full_bytes.len(),
        "compact proof should omit preprocessed OOD bytes: full={}, compact={}",
        full_bytes.len(),
        compact_bytes.len()
    );

    let compact: PreprocessedOodCompactBatchStarkProof<BabyBearConfig> =
        postcard::from_bytes(&compact_bytes).expect("deserialize compact proof");
    prover
        .verify_all_tables(&compact.proof)
        .expect_err("normal verifier must reject missing preprocessed OOD openings");
    prover
        .verify_compact_preprocessed_ood(compact, &circuit_prover_data)
        .expect("compact verifier restores omitted preprocessed OOD openings");
}

#[test]
fn test_compact_preprocessed_ood_rejects_wrong_setup_binding() {
    let (prover, proof, _) = prove_babybear_public_plus_const(10, None);
    let (_, _, wrong_setup) = prove_babybear_public_plus_const(10, Some(1));
    let compact = PreprocessedOodCompactBatchStarkProof::from_full(proof);

    let err = prover
        .verify_compact_preprocessed_ood(compact, &wrong_setup)
        .expect_err("compact verifier must bind restoration to canonical setup");
    assert!(
        format!("{err:?}").contains("canonical setup binding"),
        "unexpected error: {err:?}"
    );
}

#[test]
fn test_goldilocks_tip5_compact_preprocessed_fri_round_trip_uses_canonical_setup() {
    let (prover, proof, circuit_prover_data, fri_shape) =
        prove_goldilocks_tip5_ext2_public_plus_const(None);
    let full_input_batches =
        goldilocks_tip5_full_fri_input_batch_count(&proof.proof, &proof.stark_common);
    assert!(
        full_input_batches > goldilocks_tip5_preprocessed_trace_idx(),
        "full proof must include a preprocessed FRI input batch"
    );
    let preprocessed_idx = goldilocks_tip5_preprocessed_trace_idx();
    let original_preprocessed_input_batches = proof
        .proof
        .opening_proof
        .query_proofs
        .iter()
        .map(|query| {
            postcard::to_allocvec(&query.input_proof[preprocessed_idx])
                .expect("serialize original preprocessed FRI input batch")
        })
        .collect::<Vec<_>>();
    let original_opened_values_bytes =
        postcard::to_allocvec(&proof.proof.opened_values).expect("serialize opened values");

    let full_bytes = postcard::to_allocvec(&proof).expect("serialize full proof");
    let compact = GoldilocksTip5PreprocessedCompactBatchStarkProof::try_from_full(proof, fri_shape)
        .expect("compact Goldilocks/Tip5 proof");
    for (query, query_proof) in compact
        .proof
        .proof
        .opening_proof
        .query_proofs
        .iter()
        .enumerate()
    {
        assert_eq!(
            query_proof.input_proof.len(),
            full_input_batches - 1,
            "query {query} should omit exactly the preprocessed FRI input batch"
        );
    }
    let compact_bytes = postcard::to_allocvec(&compact).expect("serialize compact proof");
    assert!(
        compact_bytes.len() < full_bytes.len(),
        "compact proof should omit preprocessed OOD and FRI input-batch bytes: full={}, compact={}",
        full_bytes.len(),
        compact_bytes.len()
    );

    let debug_compact: GoldilocksTip5PreprocessedCompactBatchStarkProof =
        postcard::from_bytes(&compact_bytes).expect("deserialize compact proof");
    let mut debug_proof = debug_compact.into_inner();
    let (airs, pvs, effective_common) = prover
        .rebuild_verifier_statement::<2>(
            &debug_proof,
            debug_proof.w_binomial,
            &debug_proof.stark_common,
            &[],
        )
        .expect("rebuild verifier statement");
    prover
        .restore_goldilocks_tip5_preprocessed_ood_openings::<2>(
            &mut debug_proof.proof,
            &airs,
            &pvs,
            &effective_common,
            &circuit_prover_data,
            fri_shape,
        )
        .expect("restore OOD openings");
    let restored_opened_values_bytes =
        postcard::to_allocvec(&debug_proof.proof.opened_values).expect("serialize opened values");
    assert_eq!(
        restored_opened_values_bytes, original_opened_values_bytes,
        "restored OOD openings should exactly match the full proof"
    );
    let (query_indices, log_global_max_height) = prover
        .derive_goldilocks_tip5_fri_query_indices::<2>(
            &debug_proof.proof,
            &airs,
            &pvs,
            &effective_common,
            fri_shape,
        )
        .expect("derive FRI query indices");
    let val_mmcs = goldilocks_tip5_val_mmcs(fri_shape.cap_height);
    let preprocessed_prover_data = circuit_prover_data
        .prover_data
        .prover_only
        .preprocessed_prover_data
        .as_ref()
        .expect("preprocessed prover data");
    let preprocessed_log_max_height =
        p3_util::log2_strict_usize(val_mmcs.get_max_height(preprocessed_prover_data));
    for (query, (query_index, expected_bytes)) in query_indices
        .into_iter()
        .zip(original_preprocessed_input_batches.iter())
        .enumerate()
    {
        let reduced_index = query_index >> (log_global_max_height - preprocessed_log_max_height);
        let regenerated = val_mmcs.open_batch(reduced_index, preprocessed_prover_data);
        let regenerated_bytes =
            postcard::to_allocvec(&regenerated).expect("serialize regenerated input batch");
        assert_eq!(
            regenerated_bytes, *expected_bytes,
            "regenerated preprocessed FRI input batch should match full proof for query {query}"
        );
    }

    let ood_only_compact: GoldilocksTip5PreprocessedCompactBatchStarkProof =
        postcard::from_bytes(&compact_bytes).expect("deserialize compact proof");
    prover
        .verify_compact_preprocessed_ood(
            PreprocessedOodCompactBatchStarkProof {
                proof: ood_only_compact.proof,
            },
            &circuit_prover_data,
        )
        .expect_err("OOD-only compact verifier must reject missing preprocessed FRI input batch");

    let compact: GoldilocksTip5PreprocessedCompactBatchStarkProof =
        postcard::from_bytes(&compact_bytes).expect("deserialize compact proof");
    prover
        .verify_all_tables(&compact.proof)
        .expect_err("normal verifier must reject compact proof with omitted openings");
    prover
        .verify_goldilocks_tip5_preprocessed_compact(compact, &circuit_prover_data)
        .expect("compact verifier restores preprocessed OOD and FRI input-batch openings");
}

#[test]
fn test_goldilocks_tip5_compact_preprocessed_fri_rejects_wrong_setup_binding() {
    let (prover, proof, _, fri_shape) = prove_goldilocks_tip5_ext2_public_plus_const(None);
    let (_, _, wrong_setup, _) = prove_goldilocks_tip5_ext2_public_plus_const(Some(1));
    let compact = GoldilocksTip5PreprocessedCompactBatchStarkProof::try_from_full(proof, fri_shape)
        .expect("compact Goldilocks/Tip5 proof");

    let err = prover
        .verify_goldilocks_tip5_preprocessed_compact(compact, &wrong_setup)
        .expect_err("compact verifier must bind restoration to canonical setup");
    assert!(
        format!("{err:?}").contains("canonical setup binding"),
        "unexpected error: {err:?}"
    );
}

#[test]
fn test_goldilocks_tip5_path_pruned_compact_round_trip_restores_full_proof() {
    let (prover, proof, circuit_prover_data, fri_shape) =
        prove_goldilocks_tip5_ext2_public_plus_const(None);
    let full_bytes = postcard::to_allocvec(&proof).expect("serialize full proof");

    let compact = prover
        .compact_goldilocks_tip5_path_pruned_preprocessed(proof, &circuit_prover_data, fri_shape)
        .expect("path-prune compact Goldilocks/Tip5 proof");
    for (query, query_proof) in compact
        .proof
        .proof
        .opening_proof
        .query_proofs
        .iter()
        .enumerate()
    {
        for (batch, opening) in query_proof.input_proof.iter().enumerate() {
            assert!(
                opening.opening_proof.is_empty(),
                "query {query} input batch {batch} should omit Merkle paths"
            );
        }
        for (round, opening) in query_proof.commit_phase_openings.iter().enumerate() {
            assert!(
                opening.opening_proof.is_empty(),
                "query {query} commit phase {round} should omit Merkle paths"
            );
        }
    }
    assert!(
        compact
            .input_batch_paths
            .iter()
            .chain(compact.commit_phase_paths.iter())
            .any(|path_set| path_set
                .paths
                .paths
                .iter()
                .any(|path| !path.siblings.is_empty())),
        "fixture should include at least one pruned Merkle sibling"
    );

    let compact_bytes = postcard::to_allocvec(&compact).expect("serialize compact proof");
    assert!(
        compact_bytes.len() < full_bytes.len(),
        "path-pruned compact proof should be smaller: full={}, compact={}",
        full_bytes.len(),
        compact_bytes.len()
    );

    let debug_compact: GoldilocksTip5PathPrunedCompactBatchStarkProof =
        postcard::from_bytes(&compact_bytes).expect("deserialize compact proof");
    let GoldilocksTip5PathPrunedCompactBatchStarkProof {
        proof: mut debug_proof,
        fri_shape,
        input_batch_paths,
        commit_phase_paths,
    } = debug_compact;
    let (airs, pvs, effective_common) = prover
        .rebuild_verifier_statement::<2>(
            &debug_proof,
            debug_proof.w_binomial,
            &debug_proof.stark_common,
            &[],
        )
        .expect("rebuild verifier statement");
    prover
        .restore_goldilocks_tip5_preprocessed_ood_openings::<2>(
            &mut debug_proof.proof,
            &airs,
            &pvs,
            &effective_common,
            &circuit_prover_data,
            fri_shape,
        )
        .expect("restore OOD openings");
    prover
        .restore_goldilocks_tip5_path_pruned_fri_openings::<2>(
            &mut debug_proof.proof,
            &airs,
            &pvs,
            &effective_common,
            fri_shape,
            &input_batch_paths,
            &commit_phase_paths,
        )
        .expect("restore pruned Merkle paths");
    prover
        .restore_goldilocks_tip5_preprocessed_fri_input_batches::<2>(
            &mut debug_proof.proof,
            &airs,
            &pvs,
            &effective_common,
            &circuit_prover_data,
            fri_shape,
        )
        .expect("restore preprocessed FRI input batch");
    let restored_bytes = postcard::to_allocvec(&debug_proof).expect("serialize restored proof");
    assert_eq!(
        restored_bytes, full_bytes,
        "path-pruned compact restoration should exactly reproduce the original full proof"
    );

    let compact: GoldilocksTip5PathPrunedCompactBatchStarkProof =
        postcard::from_bytes(&compact_bytes).expect("deserialize compact proof");
    prover
        .verify_all_tables(&compact.proof)
        .expect_err("normal verifier must reject path-pruned compact proof");
    let compact: GoldilocksTip5PathPrunedCompactBatchStarkProof =
        postcard::from_bytes(&compact_bytes).expect("deserialize compact proof");
    prover
        .verify_goldilocks_tip5_path_pruned_preprocessed_compact(compact, &circuit_prover_data)
        .expect("path-pruned compact verifier restores full proof before upstream verification");
}

#[test]
fn test_goldilocks_tip5_path_pruned_compact_body_uses_canonical_metadata() {
    let (prover, proof, circuit_prover_data, fri_shape) =
        prove_goldilocks_tip5_ext2_public_plus_const(None);
    let metadata = GoldilocksTip5BatchStarkProofMetadata::from_proof(&proof);
    let full_bytes = postcard::to_allocvec(&proof).expect("serialize full proof");

    let compact_body = prover
        .compact_goldilocks_tip5_path_pruned_preprocessed_body(
            proof,
            &circuit_prover_data,
            fri_shape,
        )
        .expect("path-prune compact Goldilocks/Tip5 proof body");
    let body_bytes = postcard::to_allocvec(&compact_body).expect("serialize compact body");
    assert!(
        body_bytes.len() < full_bytes.len(),
        "metadata-free compact body should be smaller than full proof: full={}, body={}",
        full_bytes.len(),
        body_bytes.len()
    );

    let compact_body: GoldilocksTip5PathPrunedCompactBatchStarkProofBody =
        postcard::from_bytes(&body_bytes).expect("deserialize compact body");
    prover
        .verify_goldilocks_tip5_path_pruned_preprocessed_compact_body(
            compact_body,
            &metadata,
            &circuit_prover_data,
        )
        .expect("compact body verifier restores metadata and proof openings");
}

#[test]
fn test_goldilocks_tip5_path_pruned_compact_body_rejects_wrong_metadata() {
    let (prover, proof, circuit_prover_data, fri_shape) =
        prove_goldilocks_tip5_ext2_public_plus_const(None);
    let (_, wrong_proof, wrong_setup, _) = prove_goldilocks_tip5_ext2_public_plus_const(Some(1));
    let wrong_metadata = GoldilocksTip5BatchStarkProofMetadata::from_proof(&wrong_proof);

    let compact_body = prover
        .compact_goldilocks_tip5_path_pruned_preprocessed_body(
            proof,
            &circuit_prover_data,
            fri_shape,
        )
        .expect("path-prune compact Goldilocks/Tip5 proof body");

    prover
        .verify_goldilocks_tip5_path_pruned_preprocessed_compact_body(
            compact_body,
            &wrong_metadata,
            &wrong_setup,
        )
        .expect_err("compact body must not verify under a different canonical metadata template");
}

#[test]
fn test_goldilocks_tip5_path_pruned_compact_body_rejects_wrong_public_values() {
    let (prover, proof, circuit_prover_data, fri_shape, public_values) =
        prove_goldilocks_tip5_ext2_public_plus_const_with_public_binding(None, 1);
    let metadata = GoldilocksTip5BatchStarkProofMetadata::from_proof(&proof);

    let compact_body = prover
        .compact_goldilocks_tip5_path_pruned_preprocessed_body_with_public_values(
            proof,
            &public_values,
            &circuit_prover_data,
            fri_shape,
        )
        .expect("path-prune compact Goldilocks/Tip5 proof body");

    let mut wrong_public_values = public_values.clone();
    wrong_public_values[0] += Goldilocks::ONE;
    prover
        .verify_goldilocks_tip5_path_pruned_preprocessed_compact_body_with_public_values(
            compact_body,
            &wrong_public_values,
            &metadata,
            &circuit_prover_data,
        )
        .expect_err("compact body must bind caller-supplied public values");
}

#[test]
fn test_goldilocks_tip5_path_pruned_compact_body_context_binds_setup_shape_and_public_values() {
    let (prover, proof, circuit_prover_data, fri_shape, public_values) =
        prove_goldilocks_tip5_ext2_public_plus_const_with_public_binding(None, 1);
    let metadata = GoldilocksTip5BatchStarkProofMetadata::from_proof(&proof);
    let compact_body = prover
        .compact_goldilocks_tip5_path_pruned_preprocessed_body_with_public_values(
            proof,
            &public_values,
            &circuit_prover_data,
            fri_shape,
        )
        .expect("path-prune compact Goldilocks/Tip5 proof body");
    let compact_body_bytes = postcard::to_allocvec(&compact_body).expect("serialize compact body");
    let decode_body = || -> GoldilocksTip5PathPrunedCompactBatchStarkProofBody {
        postcard::from_bytes(&compact_body_bytes).expect("deserialize compact body")
    };

    let context = GoldilocksTip5PathPrunedCompactVerifierContext::new(
        &metadata,
        &circuit_prover_data,
        fri_shape,
        &public_values,
    );
    prover
        .verify_goldilocks_tip5_path_pruned_preprocessed_compact_body_with_context(
            decode_body(),
            context,
        )
        .expect("compact verifier context should verify the honest compact body");

    let mut wrong_shape = fri_shape;
    wrong_shape.num_queries += 1;
    let err = prover
        .verify_goldilocks_tip5_path_pruned_preprocessed_compact_body_with_context(
            decode_body(),
            GoldilocksTip5PathPrunedCompactVerifierContext::new(
                &metadata,
                &circuit_prover_data,
                wrong_shape,
                &public_values,
            ),
        )
        .expect_err("compact verifier context must pin the expected FRI shape");
    assert!(
        format!("{err:?}").contains("FRI shape mismatch"),
        "unexpected wrong-shape error: {err:?}"
    );

    let (_, _, wrong_setup, _, _) =
        prove_goldilocks_tip5_ext2_public_plus_const_with_public_binding(Some(1), 1);
    let err = prover
        .verify_goldilocks_tip5_path_pruned_preprocessed_compact_body_with_context(
            decode_body(),
            GoldilocksTip5PathPrunedCompactVerifierContext::new(
                &metadata,
                &wrong_setup,
                fri_shape,
                &public_values,
            ),
        )
        .expect_err("compact verifier context must bind metadata to canonical setup");
    assert!(
        format!("{err:?}").contains("metadata/setup binding mismatch"),
        "unexpected wrong-setup error: {err:?}"
    );

    let mut wrong_public_values = public_values.clone();
    wrong_public_values[0] += Goldilocks::ONE;
    prover
        .verify_goldilocks_tip5_path_pruned_preprocessed_compact_body_with_context(
            decode_body(),
            GoldilocksTip5PathPrunedCompactVerifierContext::new(
                &metadata,
                &circuit_prover_data,
                fri_shape,
                &wrong_public_values,
            ),
        )
        .expect_err("compact verifier context must bind caller-supplied public values");
}

#[test]
fn test_goldilocks_tip5_path_pruned_compact_rejects_tampered_merkle_path() {
    let (prover, proof, circuit_prover_data, fri_shape) =
        prove_goldilocks_tip5_ext2_public_plus_const(None);
    let compact = prover
        .compact_goldilocks_tip5_path_pruned_preprocessed(proof, &circuit_prover_data, fri_shape)
        .expect("path-prune compact Goldilocks/Tip5 proof");
    let compact_bytes = postcard::to_allocvec(&compact).expect("serialize compact proof");

    let mut leaf_bad: GoldilocksTip5PathPrunedCompactBatchStarkProof =
        postcard::from_bytes(&compact_bytes).expect("deserialize compact proof");
    let mut leaf_tampered = false;
    for path_set in leaf_bad
        .input_batch_paths
        .iter_mut()
        .chain(leaf_bad.commit_phase_paths.iter_mut())
    {
        if let Some(path) = path_set.paths.paths.first_mut() {
            path.leaf_index ^= 1;
            leaf_tampered = true;
            break;
        }
    }
    assert!(leaf_tampered, "fixture should include a pruned path");
    let err = prover
        .verify_goldilocks_tip5_path_pruned_preprocessed_compact(leaf_bad, &circuit_prover_data)
        .expect_err("tampered pruned Merkle leaf index must fail compact verification");
    assert!(
        format!("{err:?}").contains("leaf") || format!("{err:?}").contains("increasing"),
        "unexpected error for leaf-index tamper: {err:?}"
    );

    let mut compact: GoldilocksTip5PathPrunedCompactBatchStarkProof =
        postcard::from_bytes(&compact_bytes).expect("deserialize compact proof");
    let mut tampered = false;
    for path_set in compact.input_batch_paths.iter_mut() {
        for path in path_set.paths.paths.iter_mut() {
            if let Some(first) = path.siblings.first_mut() {
                first[0] += Goldilocks::from_u64(1);
                tampered = true;
                break;
            }
        }
        if tampered {
            break;
        }
    }
    if !tampered {
        for path_set in compact.commit_phase_paths.iter_mut() {
            for path in path_set.paths.paths.iter_mut() {
                if let Some(first) = path.siblings.first_mut() {
                    first[0] += Goldilocks::from_u64(1);
                    tampered = true;
                    break;
                }
            }
            if tampered {
                break;
            }
        }
    }
    assert!(
        tampered,
        "fixture should include a serialized pruned sibling"
    );

    prover
        .verify_goldilocks_tip5_path_pruned_preprocessed_compact(compact, &circuit_prover_data)
        .expect_err("tampered pruned Merkle path must fail upstream verification");
}

// --- Proof-metadata validation after deserialization ---------------------------
//
// `#[derive(Deserialize)]` bypasses the constructors that enforce structural
// invariants (non-zero row counts, lane clamping, power-of-two minimum height,
// `horner_packed_steps >= 2`). These tests deserialize/construct the invalid
// states a malicious or corrupt serialized proof could carry and assert that
// `validate()` rejects them before verification.

/// Field-compatible mirror of `TablePacking` (same field order/types) used to
/// forge invalid serialized packings. `postcard` is non-self-describing, so a
/// structurally identical struct round-trips into the real `TablePacking`.
#[derive(serde::Serialize)]
struct PackingMirror {
    public_lanes: usize,
    alu_lanes: usize,
    npo_lanes: Vec<(p3_circuit::ops::NpoTypeId, usize)>,
    min_trace_height: usize,
    horner_packed_steps: usize,
    public_binding_lanes: usize,
}

impl PackingMirror {
    fn valid() -> Self {
        Self {
            public_lanes: 1,
            alu_lanes: 1,
            npo_lanes: Vec::new(),
            min_trace_height: 1,
            horner_packed_steps: 2,
            public_binding_lanes: 0,
        }
    }

    fn into_table_packing(self) -> TablePacking {
        let bytes = postcard::to_allocvec(&self).expect("serialize packing mirror");
        postcard::from_bytes(&bytes).expect("deserialize into TablePacking")
    }
}

#[test]
fn validate_rejects_zero_serialized_row_count() {
    // A `RowCounts` is a newtype over `[usize; N]`; derived `Deserialize` bypasses
    // `RowCounts::new`'s non-zero assertion.
    let bytes = postcard::to_allocvec(&[0usize, 1, 1]).expect("serialize raw row counts");
    let rows: RowCounts = postcard::from_bytes(&bytes).expect("deserialize RowCounts");
    assert_eq!(rows.validate(), Err(ProofMetadataError::ZeroRowCount));

    let ok = postcard::to_allocvec(&[1usize, 1, 1]).expect("serialize raw row counts");
    let rows: RowCounts = postcard::from_bytes(&ok).expect("deserialize RowCounts");
    assert_eq!(rows.validate(), Ok(()));
}

#[test]
fn validate_rejects_invalid_serialized_table_packing() {
    // Sanity: a valid mirror round-trips and validates.
    assert_eq!(
        PackingMirror::valid().into_table_packing().validate(),
        Ok(())
    );

    let zero_public = PackingMirror {
        public_lanes: 0,
        ..PackingMirror::valid()
    };
    assert_eq!(
        zero_public.into_table_packing().validate(),
        Err(ProofMetadataError::ZeroLanes("public_lanes"))
    );

    let zero_alu = PackingMirror {
        alu_lanes: 0,
        ..PackingMirror::valid()
    };
    assert_eq!(
        zero_alu.into_table_packing().validate(),
        Err(ProofMetadataError::ZeroLanes("alu_lanes"))
    );

    let op = p3_circuit::ops::NpoTypeId::new("test_op");
    let zero_npo = PackingMirror {
        npo_lanes: vec![(op.clone(), 0)],
        ..PackingMirror::valid()
    };
    assert_eq!(
        zero_npo.into_table_packing().validate(),
        Err(ProofMetadataError::ZeroNpoLanes(op))
    );

    let bad_height = PackingMirror {
        min_trace_height: 24, // not a power of two
        ..PackingMirror::valid()
    };
    assert_eq!(
        bad_height.into_table_packing().validate(),
        Err(ProofMetadataError::BadMinTraceHeight(24))
    );

    let zero_height = PackingMirror {
        min_trace_height: 0,
        ..PackingMirror::valid()
    };
    assert_eq!(
        zero_height.into_table_packing().validate(),
        Err(ProofMetadataError::BadMinTraceHeight(0))
    );

    let bad_horner = PackingMirror {
        horner_packed_steps: 1,
        ..PackingMirror::valid()
    };
    assert_eq!(
        bad_horner.into_table_packing().validate(),
        Err(ProofMetadataError::BadHornerPackedSteps(1))
    );
}

#[test]
fn validate_rejects_zero_lane_npo_entry() {
    // `NonPrimitiveTableEntry` has public fields, so deserialization can produce
    // a zero-lane entry directly.
    let op = p3_circuit::ops::NpoTypeId::new("test_op");
    let entry = NonPrimitiveTableEntry::<BabyBearConfig> {
        op_type: op.clone(),
        rows: 4,
        lanes: 0,
        public_values: Vec::new(),
        air_variant: AirVariant::Baseline,
    };
    assert_eq!(entry.validate(), Err(ProofMetadataError::ZeroNpoLanes(op)));
}

#[test]
fn verify_all_tables_rejects_tampered_serialized_row_counts() {
    // End-to-end: a real proof whose deserialized `rows` metadata was corrupted
    // to a zero count must be rejected before any AIR is reconstructed from it.
    let mut builder = CircuitBuilder::<BabyBear>::new();
    let x = builder.public_input();
    let expected = builder.public_input();
    let c5 = builder.define_const(BabyBear::from_u64(5));
    let c2 = builder.define_const(BabyBear::from_u64(2));
    let mul_result = builder.mul(c5, c2);
    let add_result = builder.add(x, mul_result);
    let diff = builder.sub(add_result, expected);
    builder.assert_zero(diff);

    let circuit = builder.build().unwrap();
    let cfg = config::baby_bear();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<BabyBearConfig, _, 1>(
            &circuit,
            &TablePacking::default(),
            &[],
            &[],
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, log_degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &log_degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);

    let mut runner = circuit.runner();
    runner
        .set_public_inputs(&[BabyBear::from_u64(7), BabyBear::from_u64(17)])
        .unwrap();
    let traces = runner.run().unwrap();

    let prover = BatchStarkProver::new(cfg);
    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();

    // Forge a deserialized proof with a zero primitive row count by serializing
    // raw counts and deserializing into `RowCounts` (a state `RowCounts::new`
    // would have rejected, but derived `Deserialize` accepts).
    let public_rows = proof.rows[PrimitiveTable::Public];
    let alu_rows = proof.rows[PrimitiveTable::Alu];
    let raw =
        postcard::to_allocvec(&[0usize, public_rows, alu_rows]).expect("serialize raw row counts");
    let tampered_rows: RowCounts =
        postcard::from_bytes(&raw).expect("deserialize tampered RowCounts");
    let tampered = BatchStarkProof {
        rows: tampered_rows,
        ..proof
    };

    let err = prover
        .verify_all_tables(&tampered)
        .expect_err("tampered row counts must be rejected before verification");
    assert!(
        matches!(
            err,
            BatchStarkProverError::InvalidMetadata(ProofMetadataError::ZeroRowCount)
        ),
        "unexpected error: {err:?}"
    );
}

/// Full prove/verify round-trip of a single D=1 Poseidon1 permutation in an EF5 circuit.
#[test]
fn test_koalabear_quintic_trinomial_batch_stark_with_poseidon1_d1() {
    const D: usize = 5;
    type EF5 = QuinticTrinomialExtensionField<KoalaBear>;

    // Must match `KoalaBearD1Width16::round_constants()` in poseidon1-circuit-air.
    let inner_perm = default_koalabear_poseidon1_16();
    let mut sponge0 = [KoalaBear::ZERO; 16];
    sponge0[0] = KoalaBear::from_u64(11);
    sponge0[1] = KoalaBear::from_u64(13);
    let sponge_out = inner_perm.permute(sponge0);
    let lift_perm = LiftPermToQuintic::new(inner_perm);

    let in0 = koala_ef5_lift(KoalaBear::from_u64(11));
    let in1 = koala_ef5_lift(KoalaBear::from_u64(13));
    let exp0 = koala_ef5_lift(sponge_out[0]);
    let exp1 = koala_ef5_lift(sponge_out[1]);

    let mut builder = CircuitBuilder::<EF5>::new();
    builder.enable_poseidon1_perm_base::<P1KoalaBearD1Width16, _>(
        generate_poseidon1_trace::<EF5, P1KoalaBearD1Width16>,
        lift_perm,
    );

    let in_a = builder.public_input();
    let in_b = builder.public_input();
    let mut perm_inputs: [Option<_>; 16] = [None; 16];
    perm_inputs[0] = Some(in_a);
    perm_inputs[1] = Some(in_b);
    let (_pid, hash_outputs) = builder
        .add_poseidon1_perm_base(&Poseidon1PermCallBase {
            config: Poseidon1Config::KOALA_BEAR_D1_W16,
            new_start: true,
            inputs: perm_inputs,
            out_ctl: [true; 8],
            return_all_outputs: false,
        })
        .unwrap();
    let e0 = builder.public_input();
    let e1 = builder.public_input();
    let h0_diff = builder.sub(hash_outputs[0].unwrap(), e0);
    let h1_diff = builder.sub(hash_outputs[1].unwrap(), e1);
    builder.assert_zero(h0_diff);
    builder.assert_zero(h1_diff);

    let circuit = builder.build().unwrap();
    let cfg = config::koala_bear();

    let npo_prep: Vec<Box<dyn NpoPreprocessor<KoalaBear>>> = vec![Box::new(Poseidon1Preprocessor)];
    let air_builders = poseidon1_air_builders_d5::<KoalaBearConfig>();
    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<KoalaBearConfig, _, D>(
            &circuit,
            &TablePacking::default(),
            &npo_prep,
            &air_builders,
            ConstraintProfile::Standard,
        )
        .unwrap();
    let (airs, degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let mut runner = circuit.runner();

    runner.set_public_inputs(&[in0, in1, exp0, exp1]).unwrap();
    let traces = runner.run().unwrap();

    let prover_data = ProverData::from_airs_and_degrees(&cfg, &airs, &degrees);
    let circuit_prover_data =
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns);

    let mut prover = BatchStarkProver::new(cfg);
    for p in poseidon1_table_provers_d5(Poseidon1Config::KOALA_BEAR_D1_W16) {
        prover.register_table_prover(p);
    }

    let proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .unwrap();
    assert_eq!(proof.ext_degree, D);
    assert!(proof.w_binomial.is_none());
    assert!(proof.alu_quintic_trinomial);
    prover.verify_all_tables(&proof).unwrap();
}
