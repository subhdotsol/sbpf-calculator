//! Network tests: real transactions against a cluster.
//!
//! They're `#[ignore]`d so a plain `cargo test` never touches the network.
//! Run them with:
//!
//!   CALCULATOR_NETWORK=devnet cargo test --test network -- --ignored --test-threads=1
//!   CALCULATOR_NETWORK=local  cargo test --test network -- --ignored --test-threads=1   # surfpool
//!
//! Cost: each PASSING test creates an 8-byte account (~0.00095 SOL rent + fee).
//! FAILING tests are rejected during the RPC node's simulation ("preflight"),
//! so they are never sent and cost nothing.
//! `--test-threads=1` keeps public RPC rate limits happy.

use calculator_client::{
    default_payer_path, instruction_data, parse_calculation, program_id, read_keypair,
    CalculatorError, Calculator, CallSetup, ClientError, Network, Op,
};
use solana_address::Address;

fn client() -> Calculator {
    let network: Network = std::env::var("CALCULATOR_NETWORK")
        .unwrap_or_else(|_| "devnet".into())
        .parse()
        .expect("CALCULATOR_NETWORK must be devnet, testnet or local");
    let payer = read_keypair(&default_payer_path()).expect("payer keypair");
    let client = Calculator::new(network.rpc_url(), payer, program_id());
    client.check_deployed().unwrap_or_else(|e| panic!("{network}: {e}"));
    client
}

/// Asserts that the call succeeded and stored `expected`.
fn assert_ok(op: Op, a: u64, b: u64, expected: u64) {
    let outcome = client().calculate(op, a, b).unwrap_or_else(|e| panic!("{op:?}({a}, {b}) failed: {e}"));
    assert_eq!(outcome.value, expected, "{op:?}({a}, {b})");
    assert_eq!(Some(outcome.value), op.expected(a, b), "client-side checked math agrees");
}

/// Asserts that the program rejected the call with exactly `expected`.
fn assert_program_error(result: Result<calculator_client::Outcome, ClientError>, expected: CalculatorError) {
    match result {
        Err(ClientError::Program(err)) => assert_eq!(err, expected),
        Err(other) => panic!("expected {expected}, got a different error: {other}"),
        Ok(outcome) => panic!("expected {expected}, but the call succeeded with {}", outcome.value),
    }
}

// ============================================================ passing cases

#[test]
#[ignore = "network: run with --ignored"]
fn add() {
    assert_ok(Op::Add, 40, 2, 42);
}

#[test]
#[ignore = "network: run with --ignored"]
fn sub() {
    assert_ok(Op::Sub, 50, 8, 42);
}

#[test]
#[ignore = "network: run with --ignored"]
fn mul() {
    assert_ok(Op::Mul, 6, 7, 42);
}

#[test]
#[ignore = "network: run with --ignored"]
fn div_rounds_down() {
    assert_ok(Op::Div, 85, 2, 42);
}

#[test]
#[ignore = "network: run with --ignored"]
fn uses_all_64_bits() {
    assert_ok(Op::Add, u64::MAX - 1, 1, u64::MAX);
}

// ============================================================ your own input

/// Send your own calculation(s) and check the program's answer.
///
///   CALC="add 40 2" cargo test --test network custom -- --ignored --nocapture
///   CALC="add 40 2; div 1 0; mul 4294967296 4294967296" cargo test --test network custom -- --ignored --nocapture
///
/// The expected answer is computed here with Rust's checked math:
/// - if the answer exists, the program must return exactly it;
/// - if not, the program must fail with the matching error
///   (divide by zero -> Custom(9), overflow/underflow -> Custom(8)).
#[test]
#[ignore = "network: run with --ignored"]
fn custom_calculation() {
    let Ok(input) = std::env::var("CALC") else {
        println!("custom_calculation: set CALC to try your own input, e.g. CALC=\"add 40 2\"");
        return;
    };
    let client = client();

    for calculation in input.split(';').filter(|c| !c.trim().is_empty()) {
        let (op, a, b) = parse_calculation(calculation).unwrap_or_else(|e| panic!("CALC: {e}"));
        let result = client.calculate(op, a, b);

        match (op.expected(a, b), op.expected_error(a, b)) {
            (Some(expected), _) => {
                let outcome = result.unwrap_or_else(|e| panic!("{op:?}({a}, {b}) should succeed, got: {e}"));
                assert_eq!(outcome.value, expected, "{op:?}({a}, {b})");
                println!("✓ {op:?}({a}, {b}) = {}   (tx {})", outcome.value, outcome.signature);
            }
            (None, Some(expected_error)) => {
                assert_program_error(result, expected_error);
                println!("✓ {op:?}({a}, {b}) rejected as expected: {expected_error}");
            }
            (None, None) => unreachable!("expected_error is Some whenever expected is None"),
        }
    }
}

// ============================================================ failing cases
// Each one checks the EXACT error code from calculator.s.

#[test]
#[ignore = "network: run with --ignored"]
fn add_overflow_is_rejected() {
    assert_program_error(client().calculate(Op::Add, u64::MAX, 1), CalculatorError::Overflow);
}

#[test]
#[ignore = "network: run with --ignored"]
fn sub_underflow_is_rejected() {
    assert_program_error(client().calculate(Op::Sub, 1, 2), CalculatorError::Overflow);
}

#[test]
#[ignore = "network: run with --ignored"]
fn mul_overflow_is_rejected() {
    assert_program_error(client().calculate(Op::Mul, 1 << 32, 1 << 32), CalculatorError::Overflow);
}

#[test]
#[ignore = "network: run with --ignored"]
fn division_by_zero_is_rejected() {
    assert_program_error(client().calculate(Op::Div, 1, 0), CalculatorError::DivisionByZero);
}

#[test]
#[ignore = "network: run with --ignored"]
fn unknown_op_is_rejected() {
    let data = instruction_data(4, 1, 1); // op 4 doesn't exist
    assert_program_error(client().send(data, &CallSetup::default()), CalculatorError::UnknownOp);
}

#[test]
#[ignore = "network: run with --ignored"]
fn short_instruction_data_is_rejected() {
    let mut data = instruction_data(Op::Add as u8, 1, 1);
    data.pop(); // 16 bytes instead of 17
    assert_program_error(client().send(data, &CallSetup::default()), CalculatorError::InvalidInstructionData);
}

#[test]
#[ignore = "network: run with --ignored"]
fn result_owned_by_another_program_is_rejected() {
    // An 8-byte account owned by the System Program instead of the calculator.
    let setup = CallSetup { result_owner: Some(Address::default()), ..CallSetup::default() };
    assert_program_error(client().send(instruction_data(0, 1, 1), &setup), CalculatorError::WrongOwner);
}

#[test]
#[ignore = "network: run with --ignored"]
fn result_of_wrong_size_is_rejected() {
    let setup = CallSetup { result_space: 16, ..CallSetup::default() };
    assert_program_error(client().send(instruction_data(0, 1, 1), &setup), CalculatorError::InvalidResultSize);
}

#[test]
#[ignore = "network: run with --ignored"]
fn read_only_result_is_rejected() {
    // Not created in this transaction: creating it would force it writable.
    let setup = CallSetup { create_result_account: false, result_writable: false, ..CallSetup::default() };
    assert_program_error(client().send(instruction_data(0, 1, 1), &setup), CalculatorError::NotWritable);
}

#[test]
#[ignore = "network: run with --ignored"]
fn authority_that_did_not_sign_is_rejected() {
    let setup = CallSetup { authority_signs: false, ..CallSetup::default() };
    assert_program_error(client().send(instruction_data(0, 1, 1), &setup), CalculatorError::MissingSignature);
}

#[test]
#[ignore = "network: run with --ignored"]
fn missing_result_account_is_rejected() {
    let setup = CallSetup {
        create_result_account: false,
        include_result_account: false,
        ..CallSetup::default()
    };
    assert_program_error(client().send(instruction_data(0, 1, 1), &setup), CalculatorError::InvalidAccounts);
}
