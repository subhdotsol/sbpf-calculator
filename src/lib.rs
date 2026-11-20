// Tests for calculator.s, run inside Mollusk (a lightweight, in-process SVM).
// `sbpf test` builds deploy/calculator.so and then runs `cargo test-sbf`.
#[cfg(test)]
mod tests {
    use {
        mollusk_svm::{result::Check, Mollusk},
        solana_account::Account,
        solana_address::Address,
        solana_instruction::{AccountMeta, Instruction},
        solana_program_error::ProgramError,
    };

    const OP_ADD: u8 = 0;
    const OP_SUB: u8 = 1;
    const OP_MUL: u8 = 2;
    const OP_DIV: u8 = 3;

    // The program id is the public half of the keypair `sbpf init` generated.
    // A Solana keypair file is 64 bytes: [secret 32 | public 32].
    fn program_id() -> Address {
        let bytes: Vec<u8> =
            serde_json::from_str(&std::fs::read_to_string("deploy/calculator-keypair.json").unwrap())
                .unwrap();
        Address::new_from_array(bytes[32..64].try_into().unwrap())
    }

    // Instruction data: [op | a (u64 LE) | b (u64 LE)] = 17 bytes
    fn ix_data(op: u8, a: u64, b: u64) -> Vec<u8> {
        let mut data = vec![op];
        data.extend_from_slice(&a.to_le_bytes());
        data.extend_from_slice(&b.to_le_bytes());
        data
    }

    // Everything a test needs: the VM, the ids, and the two accounts in
    // their "happy path" state. Individual tests mutate what they need.
    struct Setup {
        mollusk: Mollusk,
        program_id: Address,
        authority: Address,
        authority_account: Account,
        result: Address,
        result_account: Account,
    }

    fn setup() -> Setup {
        let program_id = program_id();
        let mollusk = Mollusk::new(&program_id, "deploy/calculator");
        let system_program = Address::default(); // 1111...1111
        Setup {
            authority: Address::new_unique(),
            authority_account: Account::new(1_000_000_000, 0, &system_program),
            result: Address::new_unique(),
            // 8 bytes of data, owned by our program, rent-exempt.
            result_account: Account::new(
                mollusk.sysvars.rent.minimum_balance(8),
                8,
                &program_id,
            ),
            mollusk,
            program_id,
        }
    }

    fn instruction(s: &Setup, data: Vec<u8>) -> Instruction {
        Instruction::new_with_bytes(
            s.program_id,
            &data,
            vec![
                AccountMeta::new_readonly(s.authority, true), // signer
                AccountMeta::new(s.result, false),           // writable
            ],
        )
    }

    // Runs the instruction and checks the result account now holds `expected`.
    fn assert_result(op: u8, a: u64, b: u64, expected: u64) {
        let s = setup();
        s.mollusk.process_and_validate_instruction(
            &instruction(&s, ix_data(op, a, b)),
            &[
                (s.authority, s.authority_account.clone()),
                (s.result, s.result_account.clone()),
            ],
            &[
                Check::success(),
                Check::account(&s.result).data(&expected.to_le_bytes()).build(),
            ],
        );
    }

    // Runs the instruction and checks it failed with Custom(code).
    fn assert_error(s: &Setup, ix: &Instruction, accounts: &[(Address, Account)], code: u32) {
        s.mollusk.process_and_validate_instruction(
            ix,
            accounts,
            &[Check::err(ProgramError::Custom(code))],
        );
    }

    fn default_accounts(s: &Setup) -> Vec<(Address, Account)> {
        vec![
            (s.authority, s.authority_account.clone()),
            (s.result, s.result_account.clone()),
        ]
    }

    // ----------------------------------------------------------------- happy

    #[test]
    fn add() {
        assert_result(OP_ADD, 40, 2, 42);
    }

    #[test]
    fn sub() {
        assert_result(OP_SUB, 50, 8, 42);
    }

    #[test]
    fn mul() {
        assert_result(OP_MUL, 6, 7, 42);
    }

    #[test]
    fn mul_by_zero() {
        assert_result(OP_MUL, 0, u64::MAX, 0);
    }

    #[test]
    fn div() {
        assert_result(OP_DIV, 85, 2, 42); // integer division rounds down
    }

    #[test]
    fn large_values_use_all_64_bits() {
        // Catches mistakes like using 32-bit ops or 4-byte loads.
        assert_result(OP_ADD, u64::MAX - 1, 1, u64::MAX);
    }

    // -------------------------------------------------------- math errors

    #[test]
    fn add_overflow() {
        let s = setup();
        let ix = instruction(&s, ix_data(OP_ADD, u64::MAX, 1));
        assert_error(&s, &ix, &default_accounts(&s), 8);
    }

    #[test]
    fn sub_underflow() {
        let s = setup();
        let ix = instruction(&s, ix_data(OP_SUB, 1, 2));
        assert_error(&s, &ix, &default_accounts(&s), 8);
    }

    #[test]
    fn mul_overflow() {
        let s = setup();
        let ix = instruction(&s, ix_data(OP_MUL, 1 << 32, 1 << 32));
        assert_error(&s, &ix, &default_accounts(&s), 8);
    }

    #[test]
    fn div_by_zero() {
        let s = setup();
        let ix = instruction(&s, ix_data(OP_DIV, 1, 0));
        assert_error(&s, &ix, &default_accounts(&s), 9);
    }

    #[test]
    fn unknown_op() {
        let s = setup();
        let ix = instruction(&s, ix_data(4, 1, 1));
        assert_error(&s, &ix, &default_accounts(&s), 7);
    }

    #[test]
    fn short_instruction_data() {
        let s = setup();
        let mut data = ix_data(OP_ADD, 1, 1);
        data.pop(); // 16 bytes instead of 17
        let ix = instruction(&s, data);
        assert_error(&s, &ix, &default_accounts(&s), 6);
    }

    // ---------------------------------------------------- account errors

    #[test]
    fn authority_must_sign() {
        let s = setup();
        let mut ix = instruction(&s, ix_data(OP_ADD, 1, 1));
        ix.accounts[0].is_signer = false;
        assert_error(&s, &ix, &default_accounts(&s), 2);
    }

    #[test]
    fn result_must_be_writable() {
        let s = setup();
        let mut ix = instruction(&s, ix_data(OP_ADD, 1, 1));
        ix.accounts[1].is_writable = false;
        assert_error(&s, &ix, &default_accounts(&s), 3);
    }

    #[test]
    fn result_must_be_8_bytes() {
        let s = setup();
        let ix = instruction(&s, ix_data(OP_ADD, 1, 1));
        let mut result_account = s.result_account.clone();
        result_account.data = vec![0; 16];
        let accounts = vec![(s.authority, s.authority_account.clone()), (s.result, result_account)];
        assert_error(&s, &ix, &accounts, 4);
    }

    #[test]
    fn result_must_be_owned_by_program() {
        let s = setup();
        let ix = instruction(&s, ix_data(OP_ADD, 1, 1));
        let mut result_account = s.result_account.clone();
        result_account.owner = Address::new_unique(); // someone else's account
        let accounts = vec![(s.authority, s.authority_account.clone()), (s.result, result_account)];
        assert_error(&s, &ix, &accounts, 5);
    }

    #[test]
    fn authority_with_data_is_rejected() {
        // If the authority had data, every hard-coded offset after it shifts.
        let s = setup();
        let ix = instruction(&s, ix_data(OP_ADD, 1, 1));
        let mut authority_account = s.authority_account.clone();
        authority_account.data = vec![1, 2, 3];
        let accounts = vec![(s.authority, authority_account), (s.result, s.result_account.clone())];
        assert_error(&s, &ix, &accounts, 1);
    }

    #[test]
    fn wrong_number_of_accounts() {
        let s = setup();
        let mut ix = instruction(&s, ix_data(OP_ADD, 1, 1));
        ix.accounts.pop();
        let accounts = vec![(s.authority, s.authority_account.clone())];
        assert_error(&s, &ix, &accounts, 1);
    }
}
