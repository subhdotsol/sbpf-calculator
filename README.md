# calculator: a checked u64 calculator in raw sBPF assembly

A small Solana program written directly in sBPF assembly. A client sends an operation and two numbers; the program does the math with overflow checks and writes the 8-byte answer into an account it owns.

```
instruction data (17 bytes):  [op: u8][a: u64 little-endian][b: u64 little-endian]
                               op: 0 = add, 1 = sub, 2 = mul, 3 = div

accounts (in this order):
  0. authority : must sign, must hold 0 bytes of data
  1. result    : writable, owned by this program, exactly 8 bytes of data
```

| Error | Meaning | Error | Meaning |
|---|---|---|---|
| `Custom(1)` | wrong number/layout of accounts | `Custom(6)` | instruction data isn't 17 bytes |
| `Custom(2)` | authority didn't sign | `Custom(7)` | unknown op |
| `Custom(3)` | result account not writable | `Custom(8)` | overflow / underflow |
| `Custom(4)` | result account isn't 8 bytes | `Custom(9)` | division by zero |
| `Custom(5)` | result account not owned by this program | | |

## Files

```
calculator/
├── src/calculator/calculator.s   the program (every line is commented)
├── src/lib.rs                    18 Mollusk tests
├── deploy/calculator-keypair.json  the program's keypair = its address (keep it private)
├── deploy/calculator.so          the built program (created by `sbpf build`)
├── Cargo.toml / Cargo.lock       test dependencies (the lock file matters, see Testing)
├── client/                       Rust CLI client + devnet tests
├── web/                          browser UI (bun + Vite + React + shadcn/ui)
└── ../offsets.py                 prints input-buffer offsets for any program
```

## Prerequisites

| Tool | Install | Check |
|---|---|---|
| Rust | https://rustup.rs | `cargo --version` |
| sbpf | `cargo install sbpf` | `sbpf --version` |
| Solana CLI | https://solana.com/docs/intro/installation | `solana --version` |

---

## How the program reads its input (the `0x....` numbers)

When the program starts, register `r1` holds the address of one long byte buffer that the Solana runtime builds from the transaction. There are no structs: the program reads raw bytes at fixed distances (**offsets**) from `r1`. For example, `ldxdw r2, [r6 + 0x58]` means "read 8 bytes starting 88 bytes into the buffer".

### The buffer layout

The runtime always writes it in this order (source: `sbpf/crates/runtime/src/serialize.rs`):

```
u64   number of accounts                              offset 0
then, for each account, an "account record":
   +0    u8   0xff  ("not a duplicate of an earlier account")
   +1    u8   is_signer       (1 = yes)
   +2    u8   is_writable     (1 = yes)
   +3    u8   is_executable   (1 = it's a program)
   +4    4 bytes padding
   +8    32 bytes  the account's address
   +40   32 bytes  the owner program's address
   +72   u64       lamports
   +80   u64       data_len
   +88   the data, then 10240 spare bytes, then zeros up to a multiple of 8
   ...   u64       rent_epoch
u64   instruction data length
..    instruction data
32    program id
```

The field positions inside a record never change. Only the size of a record depends on its data:

```
record size = 88 + round_up_to_8(data_len + 10240) + 8
```

If the same account is passed twice, the second record is just 8 bytes (the index of the earlier one plus padding), which is why the program checks the `0xff` marker.

### Worked out for this program

| Offset | What | How it's computed |
|---|---|---|
| `0x0000` | number of accounts | start of buffer |
| `0x0008` | authority record starts | after the 8-byte count |
| `0x0009` | authority.is_signer | `0x0008 + 1` |
| `0x0058` | authority.data_len | `0x0008 + 80` |
| `0x2868` | result record starts | `8 + (88 + 10240 + 8)` = `8 + 10336` = `10344` |
| `0x286a` | result.is_writable | `0x2868 + 2` |
| `0x2890` | result.owner | `0x2868 + 40` |
| `0x28b8` | result.data_len | `0x2868 + 80` |
| `0x28c0` | result.data (the answer goes here) | `0x2868 + 88` |
| `0x50d0` | instruction data length | `10344 + (88 + 10248 + 8)` = `20688` |
| `0x50d8` | `op` (1 byte) | `0x50d0 + 8` |
| `0x50d9` | `a` (8 bytes) | `0x50d8 + 1` |
| `0x50e1` | `b` (8 bytes) | `0x50d9 + 8` |
| `0x50e9` | program id | `0x50d8 + 17` |

(The result account holds 8 bytes, so `8 + 10240 = 10248`, which is already a multiple of 8.)

### These offsets are assumptions, so the program checks them

The numbers above are only right if the authority holds 0 bytes, the result account holds 8 bytes and isn't a duplicate, and the instruction data is 17 bytes. If an attacker passed different sizes, every offset after the change would point at the wrong bytes. So `calculator.s` checks each length **before** using any offset that comes after it. The first version checked the owner before the instruction length, and a test caught it (see the comment in `calculator.s`).

### Computing offsets for your own program

Don't do it by hand. `practice/offsets.py` prints ready-to-paste `.equ` lines. Pass each account as `name:data_len` in instruction order, then the instruction data length:

```sh
python3 ../offsets.py auth:0 result:8 --ix 17                     # this program
python3 ../offsets.py payer:0 vault:40 system_program:14 --ix 9   # an example of your own
```

- The System Program account always holds 14 bytes (the text `"system_program"`).
- Use `name:dup` for an account that repeats an earlier one.
- The tool matches the hand-written offsets in this program and in Blueshift's `sbpf-asm-counter` example.

---

## Build

```sh
cd practice/calculator
sbpf build                  # assembles src/calculator/calculator.s -> deploy/calculator.so (sBPF v3)
sbpf build --arch v0        # same program as legacy sBPF v0 (only needed for a cluster without v3)
```

Look at what you built:
```sh
sbpf disassemble deploy/calculator.so
```

## Test (locally, no network)

The tests run the real `.so` inside Mollusk, an in-process Solana VM, so no validator or SOL is needed.

```sh
sbpf test                   # builds if needed, then runs `cargo test-sbf`
# or
cargo test                  # needs deploy/calculator.so to exist (run `sbpf build` first)
```

Expected: `test result: ok. 18 passed; 0 failed`.

What the 18 tests cover:
- **Math:** add, sub, mul, div, multiply by zero, full 64-bit values.
- **Math errors:** add overflow, sub underflow, mul overflow, divide by zero, unknown op, short instruction data.
- **Account checks:** missing signature, result not writable, result wrong size, result owned by someone else, authority with data, wrong number of accounts.

Add `-- --nocapture` to see the program logs and compute units (about 144 CU for a successful call).

> **If `cargo test` fails with `method ... is private`:** your `Cargo.lock` lost the pinned versions. `mollusk-svm 0.14` doesn't compile against newer `solana-program-runtime` releases. Copy `sbpf/Cargo.lock` into this folder again.

---

## Deploy to devnet

Devnet is Solana's free public test network. Its SOL has no value and comes from a faucet.

### 1. Point the Solana CLI at devnet and check your wallet

```sh
solana config set --url devnet
solana config get            # "RPC URL" should be https://api.devnet.solana.com
solana address               # your wallet address (it pays for the deploy and controls upgrades)
```

If you have no wallet yet: `solana-keygen new` (creates `~/.config/solana/id.json`).

### 2. Get devnet SOL

```sh
solana airdrop 1
solana balance
```

The airdrop is rate-limited. If it fails, use https://faucet.solana.com with your wallet address.

**Cost:** about **0.0065 SOL** for this 760-byte program (measured on a local validator), plus small transaction fees. During the upload the CLI temporarily holds some SOL in a buffer account and refunds it at the end. 1 SOL is plenty.

### 3. Build and deploy

```sh
cd practice/calculator
sbpf build
sbpf deploy calculator devnet
```

`sbpf deploy` runs this command for you; you can also run it directly:
```sh
solana program deploy deploy/calculator.so --program-id deploy/calculator-keypair.json -u devnet
```

On success it prints:
```
Program Id: EaRMAabcmU3ospHn6T31bL2BSs7AMUkUQvJAgSG4wcbA
```

That address comes from `deploy/calculator-keypair.json`; check it with `solana address -k deploy/calculator-keypair.json`. Anyone with that file could deploy to this address, so keep it out of git (`.gitignore` already ignores `deploy/`).

### 4. Check it's on-chain

```sh
solana program show EaRMAabcmU3ospHn6T31bL2BSs7AMUkUQvJAgSG4wcbA -u devnet
```

Or open https://explorer.solana.com/address/EaRMAabcmU3ospHn6T31bL2BSs7AMUkUQvJAgSG4wcbA?cluster=devnet

You should see `Data Length: 760`, and `Authority` = your wallet address. That wallet is the only one allowed to upgrade or close the program.

### Updating the program later

Edit `calculator.s`, then run `sbpf test` to make sure the tests still pass, then `sbpf build` and `sbpf deploy calculator devnet` again. It upgrades in place at the same address.

### Getting your devnet SOL back (optional)

```sh
solana program close EaRMAabcmU3ospHn6T31bL2BSs7AMUkUQvJAgSG4wcbA -u devnet --bypass-warning
```

**This is permanent:** a closed program's address can never be deployed to again. You'd need a new keypair (delete `deploy/calculator-keypair.json` and `sbpf build` makes a fresh one).

### If something goes wrong

| Problem | Fix |
|---|---|
| `airdrop request failed` / rate limited | Use https://faucet.solana.com, or wait and retry |
| `insufficient funds` | `solana balance -u devnet`; get more devnet SOL |
| The deploy is rejected with an ELF / program-format error | Shouldn't happen on devnet: sBPF v3 is enabled there (feature `5cC3foj77CWun58pC51ebHFUWavHWKarWyR5UUik7dnC`, SIMD-0178/0189/0377, active since epoch 1069). Check with `solana feature status -u devnet --display-all 5cC3foj77CWun58pC51ebHFUWavHWKarWyR5UUik7dnC`. On a cluster without it, use `sbpf build --arch v0` |
| `Program file ./deploy/calculator.so not found` | Run `sbpf build` first, from inside `practice/calculator` |
| `Error: ... upgrade authority` | You're using a different wallet than the one that first deployed. Use that wallet (`solana config set --keypair <path>`) |
| Deploy stopped halfway | Rerun the same command. To recover the SOL held in the leftover buffer: `solana program show --buffers -u devnet`, then `solana program close --buffers -u devnet` |

---

## The Rust client (`client/`)

Deploying only uploads the program. To use it, a client sends one transaction that:
1. creates an 8-byte result account owned by the program (System Program `CreateAccount`), and
2. calls the program with `[op | a | b]` and the accounts `[authority (signer), result (writable)]`.

Then it reads the u64 the program wrote. The payer and authority is your Solana CLI wallet (`~/.config/solana/id.json`, or set `KEYPAIR=/path/to/keypair.json`).

```
client/
├── src/lib.rs          the library: instruction encoding, Calculator::calculate, error mapping
├── src/main.rs         the interactive CLI
├── tests/offline.rs    8 tests, no network: encoding, account order, parsing, error codes
└── tests/network.rs    17 tests with real transactions: 5 succeed, 11 fail, 1 with your own input (CALC)
```

### Choosing a network

| Choice | RPC | Notes |
|---|---|---|
| `devnet` | api.devnet.solana.com | Where the program is deployed |
| `testnet` | api.testnet.solana.com | Deploy there first: `solana airdrop 1 -u testnet && sbpf deploy calculator testnet` |
| `local` | 127.0.0.1:8899 | **surfpool**: a local fork of devnet. Start it in another terminal with `surfpool start --network devnet`. It fetches your deployed program from devnet automatically, so no deploy is needed and every transaction is free. |

If you pick `local` and surfpool isn't running, the client prints the command and waits. If the program isn't deployed on the chosen network, it tells you how to deploy it.

### Run it

```sh
cd practice/calculator/client

cargo run                        # asks for the network, then:  calc> add 40 2
cargo run -- devnet add 40 2     # one-shot
cargo run -- local div 1 0       # → failed: program error: division by zero (Custom(9))
```

Example (devnet):
```
network: devnet (https://api.devnet.solana.com)
program: EaRMAabcmU3ospHn6T31bL2BSs7AMUkUQvJAgSG4wcbA (deployed)
payer:   5GHn… (20.6756 SOL, from ~/.config/solana/id.json)
  Add(40, 2) = 42
  result account: 9pyW…
  signature:      3RZ6…
  explorer:       https://explorer.solana.com/tx/3RZ6…?cluster=devnet
```

### Test it

```sh
cd practice/calculator/client

# offline (free, instant): 8 tests
cargo test

# real transactions: 16 fixed tests + custom_calculation (your own input via CALC)
CALCULATOR_NETWORK=local  cargo test --test network -- --ignored --test-threads=1   # surfpool, free
CALCULATOR_NETWORK=devnet cargo test --test network -- --ignored --test-threads=1   # devnet
```

| Passing cases (must succeed) | Failing cases (must fail with this exact error) |
|---|---|
| `add` 40+2=42 | add overflow → `Custom(8)` |
| `sub` 50−8=42 | sub underflow → `Custom(8)` |
| `mul` 6×7=42 | mul overflow → `Custom(8)` |
| `div` 85÷2=42 | divide by zero → `Custom(9)` |
| full 64 bits: (2⁶⁴−2)+1 | unknown op 4 → `Custom(7)` |
| | 16-byte instruction data → `Custom(6)` |
| | result owned by the System Program → `Custom(5)` |
| | 16-byte result account → `Custom(4)` |
| | read-only result account → `Custom(3)` |
| | authority didn't sign → `Custom(2)` |
| | result account missing → `Custom(1)` |

#### Test your own numbers

`custom_calculation` takes the operation and numbers from the `CALC` variable. Separate several calculations with `;`:

```sh
CALC="add 40 2" CALCULATOR_NETWORK=devnet cargo test --test network custom -- --ignored --nocapture

CALC="add 40 2; mul 123456789 1000; sub 5 9; div 1 0" \
  CALCULATOR_NETWORK=local cargo test --test network custom -- --ignored --nocapture
```

The test works out the correct answer itself (Rust's checked math) and compares it with what the program did:
- **If the answer exists,** the program must store exactly that value.
- **If it doesn't,** the program must fail with the matching error: dividing by zero → `Custom(9)`, overflow or going below zero → `Custom(8)`.

```
✓ Add(40, 2) = 42   (tx 1YCh…)
✓ Mul(123456789, 1000) = 123456789000   (tx 5hnd…)
✓ Sub(5, 9) rejected as expected: overflow / underflow (Custom(8))
✓ Div(1, 0) rejected as expected: division by zero (Custom(9))
```

Ops: `add sub mul div` (or `+ - * /`); numbers are u64 (`0` to `18446744073709551615`). Without `CALC`, the test prints a hint and passes, so it never breaks a full `--ignored` run. `--nocapture` makes the `✓` lines visible.

The network tests are marked `#[ignore]` so a plain `cargo test` never touches the network; `--ignored` runs them. `--test-threads=1` avoids public RPC rate limits.

**Cost on devnet:** a full run spent **0.0035 devnet SOL**. Each passing test creates an 8-byte account (about 0.00095 SOL of rent). Failing tests are rejected while the RPC node simulates the transaction ("preflight"), so they're never sent and cost nothing.

All 16 pass on both surfpool and devnet.

## The web app (`web/`)

A browser UI for the same program, built with **bun**, Vite, React, Tailwind and **shadcn/ui**. You sign with a browser wallet (Phantom, Solflare or Backpack) instead of `~/.config/solana/id.json`.

```sh
cd web
bun install
bun dev          # http://localhost:5173
bun run build    # production build in web/dist
```

1. Pick a network: **Devnet**, **Testnet**, or **Local (surfpool)**. For local, run `surfpool start --network devnet` first; the app tells you if it can't reach it. The badge shows whether the program is deployed there.
2. Click **Connect wallet**. Your wallet must hold SOL **on the network you picked**: the app sends the transaction itself, so the network selected inside the wallet doesn't matter (`solana airdrop 1 <your-wallet-address> -u devnet`).
3. Enter `a`, an op and `b`. The line under the inputs previews the answer with the same checked u64 math, so you see `will fail: overflow` before sending anything.
4. Click **Send to the program**. Each result lands in the history with an explorer link.

How a call works (`web/src/lib/calculator.ts`, which mirrors `client/src/lib.rs`):

- It builds one transaction: create a fresh 8-byte result account owned by the calculator, then the calculator instruction (`[op][a u64 LE][b u64 LE]`, accounts authority=you as signer, result as writable).
- It **simulates first**. If the program rejects the call, you see the exact code (for example `division by zero (Custom(9))`), and the wallet is never asked to sign, so it costs nothing.
- Otherwise the wallet signs, the result account's keypair signs, it's sent and confirmed, and the stored u64 is read back.

shadcn components live in `web/src/components/ui/`. Add more with `bunx --bun shadcn@latest add <name>`.
