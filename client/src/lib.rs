//! Rust client for the sBPF `calculator` program.
//!
//! The on-chain program (see `../src/calculator/calculator.s`) expects:
//!
//! ```text
//! instruction data (17 bytes): [op: u8][a: u64 LE][b: u64 LE]
//! accounts: 0. authority (signer)
//!           1. result    (writable, owned by the program, 8 bytes of data)
//! ```
//!
//! [`Calculator::calculate`] does everything in ONE transaction:
//! 1. System Program `CreateAccount`: a fresh 8-byte result account owned by the program
//! 2. the calculator instruction
//!
//! then reads the u64 the program wrote into the result account.

use {
    solana_address::Address,
    solana_commitment_config::CommitmentConfig,
    solana_instruction::{AccountMeta, Instruction},
    solana_instruction_error::InstructionError,
    solana_keypair::Keypair,
    solana_rpc_client::rpc_client::RpcClient,
    solana_signer::Signer,
    solana_transaction::Transaction,
    solana_transaction_error::TransactionError,
    std::{fmt, path::Path, str::FromStr},
};

/// Address the calculator is deployed at (public key of `deploy/calculator-keypair.json`).
pub const PROGRAM_ID: &str = "EaRMAabcmU3ospHn6T31bL2BSs7AMUkUQvJAgSG4wcbA";

/// Size of the instruction data: 1 (op) + 8 (a) + 8 (b).
pub const IX_SIZE: usize = 17;

/// Size of the result account's data: one u64.
pub const RESULT_SIZE: u64 = 8;

pub fn program_id() -> Address {
    Address::from_str(PROGRAM_ID).expect("PROGRAM_ID is a valid base58 address")
}

// ---------------------------------------------------------------------------
// Networks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    Devnet,
    Testnet,
    /// A local `surfpool start --network devnet`: a fork of devnet on your machine.
    Local,
}

impl Network {
    pub fn rpc_url(self) -> &'static str {
        match self {
            Network::Devnet => "https://api.devnet.solana.com",
            Network::Testnet => "https://api.testnet.solana.com",
            Network::Local => "http://127.0.0.1:8899",
        }
    }

    /// Solana Explorer link for a transaction (None for the local network).
    pub fn explorer_tx_url(self, signature: &str) -> Option<String> {
        let cluster = match self {
            Network::Devnet => "devnet",
            Network::Testnet => "testnet",
            Network::Local => return None,
        };
        Some(format!("https://explorer.solana.com/tx/{signature}?cluster={cluster}"))
    }
}

impl FromStr for Network {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1" | "devnet" => Ok(Network::Devnet),
            "2" | "testnet" => Ok(Network::Testnet),
            "3" | "local" | "surfpool" | "localhost" => Ok(Network::Local),
            other => Err(format!("unknown network `{other}` (expected devnet, testnet or local)")),
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Network::Devnet => "devnet",
            Network::Testnet => "testnet",
            Network::Local => "local (surfpool)",
        })
    }
}

// ---------------------------------------------------------------------------
// Operations and instruction encoding
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add = 0,
    Sub = 1,
    Mul = 2,
    Div = 3,
}

impl Op {
    /// What the program should compute, using checked math (None = overflow / div by 0).
    pub fn expected(self, a: u64, b: u64) -> Option<u64> {
        match self {
            Op::Add => a.checked_add(b),
            Op::Sub => a.checked_sub(b),
            Op::Mul => a.checked_mul(b),
            Op::Div => a.checked_div(b),
        }
    }
}

impl Op {
    /// The error the program must return when [`Op::expected`] is `None`:
    /// dividing by zero → `DivisionByZero`, anything else → `Overflow`.
    pub fn expected_error(self, a: u64, b: u64) -> Option<CalculatorError> {
        match (self.expected(a, b), self) {
            (Some(_), _) => None,
            (None, Op::Div) if b == 0 => Some(CalculatorError::DivisionByZero),
            (None, _) => Some(CalculatorError::Overflow),
        }
    }
}

/// Parses `"<op> <a> <b>"`, e.g. `"add 40 2"` or `"* 6 7"`.
pub fn parse_calculation(input: &str) -> Result<(Op, u64, u64), String> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    let [op, a, b] = parts.as_slice() else {
        return Err(format!("expected `<op> <a> <b>` (e.g. `add 40 2`), got `{}`", input.trim()));
    };
    let a = a.parse::<u64>().map_err(|e| format!("bad a `{a}`: {e}"))?;
    let b = b.parse::<u64>().map_err(|e| format!("bad b `{b}`: {e}"))?;
    Ok((op.parse()?, a, b))
}

impl FromStr for Op {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "add" | "+" => Ok(Op::Add),
            "sub" | "-" => Ok(Op::Sub),
            "mul" | "*" | "x" => Ok(Op::Mul),
            "div" | "/" => Ok(Op::Div),
            other => Err(format!("unknown op `{other}` (expected add, sub, mul or div)")),
        }
    }
}

/// Encodes the 17-byte instruction data. Takes a raw `op` byte so tests can
/// also send invalid ops; normal callers pass `Op::Add as u8` etc.
pub fn instruction_data(op: u8, a: u64, b: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(IX_SIZE);
    data.push(op);
    data.extend_from_slice(&a.to_le_bytes());
    data.extend_from_slice(&b.to_le_bytes());
    data
}

/// The calculator instruction. Account order and flags must match the program:
/// 0 = authority (signer, read-only), 1 = result (writable).
pub fn calculator_instruction(
    program_id: &Address,
    authority: &Address,
    result: &Address,
    data: Vec<u8>,
) -> Instruction {
    Instruction::new_with_bytes(
        *program_id,
        &data,
        vec![
            AccountMeta::new_readonly(*authority, true),
            AccountMeta::new(*result, false),
        ],
    )
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// The program's error codes (`r0` on exit; `ProgramError::Custom(n)` on-chain).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalculatorError {
    InvalidAccounts = 1,
    MissingSignature = 2,
    NotWritable = 3,
    InvalidResultSize = 4,
    WrongOwner = 5,
    InvalidInstructionData = 6,
    UnknownOp = 7,
    Overflow = 8,
    DivisionByZero = 9,
}

impl CalculatorError {
    pub fn from_code(code: u32) -> Option<Self> {
        use CalculatorError::*;
        Some(match code {
            1 => InvalidAccounts,
            2 => MissingSignature,
            3 => NotWritable,
            4 => InvalidResultSize,
            5 => WrongOwner,
            6 => InvalidInstructionData,
            7 => UnknownOp,
            8 => Overflow,
            9 => DivisionByZero,
            _ => return None,
        })
    }
}

impl fmt::Display for CalculatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            CalculatorError::InvalidAccounts => "wrong number or layout of accounts",
            CalculatorError::MissingSignature => "authority did not sign",
            CalculatorError::NotWritable => "result account is not writable",
            CalculatorError::InvalidResultSize => "result account is not 8 bytes",
            CalculatorError::WrongOwner => "result account is not owned by the program",
            CalculatorError::InvalidInstructionData => "instruction data is not 17 bytes",
            CalculatorError::UnknownOp => "unknown operation",
            CalculatorError::Overflow => "overflow / underflow",
            CalculatorError::DivisionByZero => "division by zero",
        };
        write!(f, "{msg} (Custom({}))", *self as u32)
    }
}

#[derive(Debug)]
pub enum ClientError {
    /// The calculator program rejected the instruction with one of its error codes.
    Program(CalculatorError),
    /// Any other on-chain failure (e.g. the System Program failed to create the account).
    Transaction(TransactionError),
    /// RPC / network problem.
    Rpc(String),
    /// The program account doesn't exist (or isn't a program) on this cluster.
    NotDeployed(Address),
    /// Reading or parsing the payer keypair failed.
    Keypair(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Program(e) => write!(f, "program error: {e}"),
            ClientError::Transaction(e) => write!(f, "transaction failed: {e}"),
            ClientError::Rpc(e) => write!(f, "rpc error: {e}"),
            ClientError::NotDeployed(id) => write!(f, "program {id} is not deployed on this network"),
            ClientError::Keypair(e) => write!(f, "keypair error: {e}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Turns an RPC send error into a [`ClientError`]. A failing instruction comes
/// back as `InstructionError(index, Custom(code))`; codes 1-9 are the
/// calculator's own errors.
fn map_send_error(err: solana_rpc_client_api::client_error::Error) -> ClientError {
    match err.get_transaction_error() {
        Some(TransactionError::InstructionError(_, InstructionError::Custom(code)))
            if CalculatorError::from_code(code).is_some() =>
        {
            ClientError::Program(CalculatorError::from_code(code).unwrap())
        }
        Some(tx_err) => ClientError::Transaction(tx_err),
        None => ClientError::Rpc(err.to_string()),
    }
}

/// Reads a Solana CLI keypair file (a JSON array of 64 bytes: secret 32 | public 32).
pub fn read_keypair(path: &Path) -> Result<Keypair, ClientError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ClientError::Keypair(format!("{}: {e}", path.display())))?;
    let bytes: Vec<u8> = serde_json::from_str(&text)
        .map_err(|e| ClientError::Keypair(format!("{}: {e}", path.display())))?;
    Keypair::try_from(bytes.as_slice())
        .map_err(|e| ClientError::Keypair(format!("{}: {e}", path.display())))
}

/// The default payer: `$KEYPAIR`, or the Solana CLI wallet `~/.config/solana/id.json`.
pub fn default_payer_path() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("KEYPAIR") {
        return path.into();
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".config/solana/id.json")
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

/// A successful call.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub value: u64,
    pub signature: String,
    pub result_account: Address,
}

/// How to set up the result account for [`Calculator::send`]. The defaults are
/// the correct setup; tests change single fields to provoke each error.
#[derive(Debug, Clone)]
pub struct CallSetup {
    /// Create a fresh result account in the same transaction. If false, a fresh
    /// address that doesn't exist on-chain is passed instead.
    pub create_result_account: bool,
    /// Data size of the created result account (correct: 8).
    pub result_space: u64,
    /// Owner of the created result account (None = the calculator program).
    pub result_owner: Option<Address>,
    /// Mark the result account writable (correct: true).
    pub result_writable: bool,
    /// Let the authority sign (correct: true). If false, a random non-signing
    /// address is used as the authority.
    pub authority_signs: bool,
    /// Pass the result account at all (correct: true).
    pub include_result_account: bool,
}

impl Default for CallSetup {
    fn default() -> Self {
        Self {
            create_result_account: true,
            result_space: RESULT_SIZE,
            result_owner: None,
            result_writable: true,
            authority_signs: true,
            include_result_account: true,
        }
    }
}

pub struct Calculator {
    rpc: RpcClient,
    payer: Keypair,
    program_id: Address,
}

impl Calculator {
    pub fn new(rpc_url: &str, payer: Keypair, program_id: Address) -> Self {
        let rpc = RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());
        Self { rpc, payer, program_id }
    }

    pub fn payer(&self) -> Address {
        self.payer.pubkey()
    }

    pub fn rpc(&self) -> &RpcClient {
        &self.rpc
    }

    /// Fails with [`ClientError::NotDeployed`] if the program isn't on this cluster.
    pub fn check_deployed(&self) -> Result<(), ClientError> {
        match self.rpc.get_account(&self.program_id) {
            Ok(account) if account.executable => Ok(()),
            Ok(_) => Err(ClientError::NotDeployed(self.program_id)),
            Err(e) if e.to_string().contains("AccountNotFound") => {
                Err(ClientError::NotDeployed(self.program_id))
            }
            Err(e) => Err(ClientError::Rpc(e.to_string())),
        }
    }

    pub fn balance(&self) -> Result<u64, ClientError> {
        self.rpc.get_balance(&self.payer.pubkey()).map_err(|e| ClientError::Rpc(e.to_string()))
    }

    /// Computes `a <op> b` on-chain and returns the value the program stored.
    pub fn calculate(&self, op: Op, a: u64, b: u64) -> Result<Outcome, ClientError> {
        self.send(instruction_data(op as u8, a, b), &CallSetup::default())
    }

    /// Sends arbitrary instruction data with a custom account setup.
    pub fn send(&self, data: Vec<u8>, setup: &CallSetup) -> Result<Outcome, ClientError> {
        let result = Keypair::new();
        let mut instructions = Vec::new();
        let mut signers: Vec<&Keypair> = vec![&self.payer];

        // 1. Create the result account (8 bytes, rent-exempt, owned by the program).
        if setup.create_result_account {
            let lamports = self
                .rpc
                .get_minimum_balance_for_rent_exemption(setup.result_space as usize)
                .map_err(|e| ClientError::Rpc(e.to_string()))?;
            instructions.push(solana_system_interface::instruction::create_account(
                &self.payer.pubkey(),
                &result.pubkey(),
                lamports,
                setup.result_space,
                &setup.result_owner.unwrap_or(self.program_id),
            ));
            signers.push(&result); // a new account must sign its own creation
        }

        // 2. The calculator instruction.
        let authority = if setup.authority_signs {
            self.payer.pubkey()
        } else {
            Keypair::new().pubkey() // never signs: the program must reject it
        };
        let mut ix = calculator_instruction(&self.program_id, &authority, &result.pubkey(), data);
        if !setup.authority_signs {
            // Also clear the signer flag. Otherwise the SDK refuses to build a
            // transaction that's missing a required signature, and the program
            // never gets to check is_signer itself.
            ix.accounts[0].is_signer = false;
        }
        if !setup.result_writable {
            ix.accounts[1].is_writable = false;
        }
        if !setup.include_result_account {
            ix.accounts.truncate(1);
        }
        instructions.push(ix);

        let blockhash = self.rpc.get_latest_blockhash().map_err(|e| ClientError::Rpc(e.to_string()))?;
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&self.payer.pubkey()),
            &signers,
            blockhash,
        );

        // The RPC node simulates the transaction first ("preflight"). A failing
        // calculator call is rejected there, so it is never sent and costs nothing.
        let signature = self.rpc.send_and_confirm_transaction(&tx).map_err(map_send_error)?;

        let account = self
            .rpc
            .get_account(&result.pubkey())
            .map_err(|e| ClientError::Rpc(e.to_string()))?;
        let value = account
            .data
            .get(..8)
            .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
            .ok_or_else(|| ClientError::Rpc("result account has fewer than 8 bytes".into()))?;

        Ok(Outcome { value, signature: signature.to_string(), result_account: result.pubkey() })
    }
}
