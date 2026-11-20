//! Offline tests: no network, no SOL. Run with `cargo test`.
//! They check that the client encodes exactly what calculator.s expects.

use calculator_client::{
    calculator_instruction, instruction_data, program_id, CalculatorError, Network, Op, IX_SIZE,
    PROGRAM_ID,
};
use solana_address::Address;

#[test]
fn instruction_data_is_17_bytes_op_then_two_little_endian_u64s() {
    let data = instruction_data(Op::Mul as u8, 0x0102030405060708, 42);
    assert_eq!(data.len(), IX_SIZE);
    assert_eq!(data[0], 2, "byte 0 is the op (mul = 2)");
    assert_eq!(&data[1..9], &[8, 7, 6, 5, 4, 3, 2, 1], "a is little-endian at bytes 1..9");
    assert_eq!(u64::from_le_bytes(data[9..17].try_into().unwrap()), 42, "b at bytes 9..17");
}

#[test]
fn op_byte_values_match_the_program() {
    // calculator.s: jeq r2, 0, op_add / 1 op_sub / 2 op_mul / 3 op_div
    assert_eq!(Op::Add as u8, 0);
    assert_eq!(Op::Sub as u8, 1);
    assert_eq!(Op::Mul as u8, 2);
    assert_eq!(Op::Div as u8, 3);
}

#[test]
fn accounts_are_authority_signer_then_result_writable() {
    let authority = Address::new_unique();
    let result = Address::new_unique();
    let ix = calculator_instruction(&program_id(), &authority, &result, instruction_data(0, 1, 2));

    assert_eq!(ix.program_id.to_string(), PROGRAM_ID);
    assert_eq!(ix.accounts.len(), 2);
    assert_eq!(ix.accounts[0].pubkey, authority);
    assert!(ix.accounts[0].is_signer && !ix.accounts[0].is_writable, "authority: signer, read-only");
    assert_eq!(ix.accounts[1].pubkey, result);
    assert!(!ix.accounts[1].is_signer && ix.accounts[1].is_writable, "result: writable, not signer");
}

#[test]
fn parses_ops_and_networks() {
    assert_eq!("add".parse::<Op>().unwrap(), Op::Add);
    assert_eq!("*".parse::<Op>().unwrap(), Op::Mul);
    assert!("pow".parse::<Op>().is_err());

    assert_eq!("devnet".parse::<Network>().unwrap(), Network::Devnet);
    assert_eq!("2".parse::<Network>().unwrap(), Network::Testnet);
    assert_eq!("surfpool".parse::<Network>().unwrap(), Network::Local);
    assert!("mainnet".parse::<Network>().is_err());
}

#[test]
fn every_program_error_code_maps_back() {
    for code in 1..=9u32 {
        let err = CalculatorError::from_code(code).expect("codes 1-9 are defined");
        assert_eq!(err as u32, code);
    }
    assert_eq!(CalculatorError::from_code(0), None);
    assert_eq!(CalculatorError::from_code(10), None);
}

#[test]
fn expected_results_use_checked_math() {
    assert_eq!(Op::Add.expected(40, 2), Some(42));
    assert_eq!(Op::Add.expected(u64::MAX, 1), None);
    assert_eq!(Op::Sub.expected(1, 2), None);
    assert_eq!(Op::Div.expected(85, 2), Some(42));
    assert_eq!(Op::Div.expected(1, 0), None);
}

#[test]
fn parses_calculations_and_predicts_errors() {
    use calculator_client::parse_calculation;
    assert_eq!(parse_calculation("add 40 2").unwrap(), (Op::Add, 40, 2));
    assert_eq!(parse_calculation("  * 6   7 ").unwrap(), (Op::Mul, 6, 7));
    assert!(parse_calculation("add 40").is_err());
    assert!(parse_calculation("add -1 2").is_err(), "u64 only");

    assert_eq!(Op::Add.expected_error(40, 2), None);
    assert_eq!(Op::Div.expected_error(1, 0), Some(CalculatorError::DivisionByZero));
    assert_eq!(Op::Sub.expected_error(1, 2), Some(CalculatorError::Overflow));
    assert_eq!(Op::Mul.expected_error(u64::MAX, 2), Some(CalculatorError::Overflow));
}

#[test]
fn explorer_links() {
    assert_eq!(
        Network::Devnet.explorer_tx_url("sig").unwrap(),
        "https://explorer.solana.com/tx/sig?cluster=devnet"
    );
    assert!(Network::Local.explorer_tx_url("sig").is_none());
}
