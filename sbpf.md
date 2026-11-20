# sBPF: everything you need to write Solana programs at the lowest level

This guide assumes you know a little Solana (accounts, instructions, programs) and a little Rust. It does **not** assume you know assembly, BPF, ELF, or how the Solana VM works. Everything here comes from reading the two repos in this folder (`sbpf/` and `sbpf-linker/`). File references look like `sbpf/crates/vm/src/memory.rs` so you can open the code yourself.

The worked example in Part 9 was **built and tested**, not just written: 18/18 Mollusk tests pass. The project is in `practice/calculator/`.

---

## Table of contents

- [Part 0. A two-minute summary](#part-0-a-two-minute-summary)
- [Part 1. The big picture: what a Solana program really is](#part-1-the-big-picture-what-a-solana-program-really-is)
- [Part 2. Why `sbpf` and `sbpf-linker` exist](#part-2-why-sbpf-and-sbpf-linker-exist)
- [Part 3. v0, v1, v2, v3: the versions explained](#part-3-v0-v1-v2-v3-the-versions-explained)
- [Part 4. The virtual machine: registers, memory, stack, compute](#part-4-the-virtual-machine-registers-memory-stack-compute)
- [Part 5. Instructions: encoding and the full instruction set](#part-5-instructions-encoding-and-the-full-instruction-set)
- [Part 6. Assembly syntax: everything the assembler understands](#part-6-assembly-syntax-everything-the-assembler-understands)
- [Part 7. The input buffer: how your program sees accounts](#part-7-the-input-buffer-how-your-program-sees-accounts)
- [Part 8. Syscalls: talking to the runtime](#part-8-syscalls-talking-to-the-runtime)
- [Part 9. Worked example: a checked calculator (full code and tests)](#part-9-worked-example-a-checked-calculator-full-code-and-tests)
- [Part 10. The `sbpf` toolchain, command by command](#part-10-the-sbpf-toolchain-command-by-command)
- [Part 11. What the assembler produces: the ELF file](#part-11-what-the-assembler-produces-the-elf-file)
- [Part 12. `sbpf-linker` in depth](#part-12-sbpf-linker-in-depth)
- [Part 13. How the `sbpf` codebase is organized](#part-13-how-the-sbpf-codebase-is-organized)
- [Part 14. Gotchas and mistakes to avoid](#part-14-gotchas-and-mistakes-to-avoid)
- [Part 15. Practice exercises](#part-15-practice-exercises)
- [Appendix A. Cheat sheet](#appendix-a-cheat-sheet)

---

## Part 0. A two-minute summary

- **sBPF** (Solana BPF) is the **instruction set** that every Solana program compiles down to. Validators run that bytecode inside a small virtual machine. Whether you write Anchor, native Rust, Pinocchio, or C, the end result is sBPF bytecode in a `.so` (ELF) file.
- **`sbpf`** (the repo in `sbpf/`, by Blueshift) is a **toolchain for writing sBPF by hand in assembly**: an assembler, disassembler, debugger, VM, and project CLI (`sbpf init/build/test/deploy/...`). It skips Rust and LLVM entirely.
- **`sbpf-linker`** (the repo in `sbpf-linker/`) lets you build Solana programs with **ordinary upstream nightly Rust and upstream LLVM** instead of Solana's custom forked compiler. It takes the generic eBPF object that upstream LLVM produces and **relinks it into a valid sBPF program**. Under the hood it reuses the `sbpf` assembler crates.
- **Versions**: the toolchains here target **v0** (legacy, works everywhere) and **v3** (newer, smaller and simpler binaries, default in both tools). v1 and v2 are intermediate versions that these tools do not emit. **Learn the instruction set once.** Your assembly source is nearly identical across v0 and v3; the `--arch` flag mostly changes how the file is packaged and how syscalls are encoded.

---

## Part 1. The big picture: what a Solana program really is

### 1.1 From your code to the chain

```
 your code (Rust / C / assembly)
        │
        ▼  compiler / assembler
 sBPF bytecode, packaged as an ELF file  (program.so)
        │
        ▼  solana program deploy
 stored in an account on-chain
        │
        ▼  a transaction calls the program
 validator loads the ELF, verifies it, and runs the bytecode in the sBPF VM
 (interpreted or JIT-compiled to native machine code)
```

A Solana program is not a Rust binary or a WASM module. It is a list of very simple 8-byte instructions like "load 8 bytes from memory into register 2" or "jump if register 2 equals 0". This list lives inside an ELF file (the same container format Linux uses for executables and `.so` libraries).

### 1.2 Where BPF comes from

**BPF** (Berkeley Packet Filter) started in the Linux kernel as a tiny VM for filtering network packets. It grew into **eBPF** (extended BPF), a general-purpose 64-bit instruction set that the kernel runs safely in a sandbox. eBPF has properties a blockchain wants:

| Property | Why a blockchain cares |
|---|---|
| Tiny instruction set (~100 opcodes) | Easy to verify, easy to implement identically on every validator |
| Fixed 8-byte instruction size | Fast to decode, easy to JIT-compile to x86/ARM |
| Registers instead of a stack machine | Maps directly onto real CPUs, so it runs fast |
| No direct access to the machine | Programs can only touch memory the VM gives them |
| Deterministic | Every validator must get exactly the same result |

Solana took eBPF and modified it. The modified version is called **sBPF**. The main changes from Linux eBPF:

- Memory is split into fixed **regions** (program data, stack, heap, input) at fixed virtual addresses (Part 4).
- **Syscalls** (helper functions provided by the runtime, like logging and hashing) replace kernel helpers (Part 8).
- **Compute metering**: every instruction costs compute units, and the program aborts when it runs out.
- Some instruction semantics differ. For example, in sBPF `add32`, `sub32` and `mul32` **sign-extend** their 32-bit result to 64 bits, while Linux eBPF **zero-extends** (see the comment at the top of `sbpf/crates/analyzer/src/alu32_extension.rs`).

### 1.3 Two meanings of "sbpf"

People say "sbpf" for two different things:

1. **sBPF the instruction set**: the bytecode format and VM rules defined by Solana's runtime (Agave).
2. **`sbpf` the tool**: Blueshift's assembler/CLI in `sbpf/`. Its crates are named `sbpf-assembler`, `sbpf-vm`, and so on.

In this guide, **sBPF** means the instruction set and **`sbpf`** (in code font) means the tool.

---

## Part 2. Why `sbpf` and `sbpf-linker` exist

### 2.1 The traditional way, and its problems

Normally you run `cargo build-sbf`. That uses Solana's **platform-tools**: a forked Rust compiler and a forked LLVM with a custom "SBF" backend.

```
Rust source ──► Solana's forked rustc + forked LLVM (sbf target) ──► program.so
```

This works, but it has real costs:

- **The fork lags upstream.** New Rust versions and LLVM improvements arrive late, because someone has to port them to the fork.
- **It's heavy.** You download a large custom toolchain.
- **You don't control the output.** The compiler decides which instructions you get. If you care about every compute unit, you're limited to "write Rust and hope".

### 2.2 What `sbpf` (the assembler toolchain) solves

From `sbpf/CONTRIBUTING.md`:

> sBPF is the first fully independent Solana BPF assembler. It enables users to quickly get started developing sBPF Assembly projects, skipping the heavy build requirements of solana-rust and solana-llvm and translating assembly code directly into SVM-compatible bytecode.

In practice:

```
program.s (hand-written assembly) ──► sbpf build (milliseconds, no LLVM) ──► program.so
```

- **Total control**: every instruction in the output is one you wrote. That gives you the smallest possible programs and the lowest possible compute usage.
- **Instant builds**: my test program assembled in about 3 ms.
- **Tiny binaries**: the calculator in Part 9 is **760 bytes** as v3 (1.6 KB as v0).
- **A full toolkit**: `init` scaffolds a project, `build` assembles, `test` runs Rust tests in Mollusk, `deploy` wraps `solana program deploy`, `disassemble` turns any `.so` back into readable assembly, `debug` is a step debugger.
- **Learning**: it is the best way to understand what every Solana program actually does underneath.

### 2.3 What `sbpf-linker` solves

`sbpf-linker` solves a different problem: **writing Rust without Solana's forked compiler**.

Upstream Rust and LLVM already support generic eBPF (target `bpfel-unknown-none`), because Linux kernel developers use it. But plain eBPF output is **not** a valid Solana program: wrong ELF layout, wrong syscall mechanism, different semantics for some instructions, unsupported relocations. `sbpf-linker` bridges that gap:

```
Rust source
   │  upstream nightly rustc, target bpfel-unknown-none, -C linker=sbpf-linker
   ▼
LLVM bitcode for each crate
   │  sbpf-linker step 1: bpf-linker (from the Aya project) links all the bitcode
   │  and runs upstream LLVM's BPF backend → a generic eBPF object file (.o)
   ▼
generic eBPF object
   │  sbpf-linker step 2 (src/byteparser.rs): decode every instruction into the
   │  sbpf assembler's AST, resolve relocations, fix semantic differences
   ▼
sbpf assembler (the same crates as the `sbpf` tool) emits a v0 or v3 ELF
   ▼
target/deploy/<name>.so   ← deployable Solana program
```

From the README: *"An upstream BPF linker to relink upstream BPF binaries into an SBPF V0/V3 compatible binary format."*

So the two repos fit together like this:

| | `sbpf` | `sbpf-linker` |
|---|---|---|
| You write | Assembly (`.s`) | Rust (`no_std`) |
| Compiler | None (it is the assembler) | Upstream nightly rustc + LLVM |
| Output | v0 or v3 `.so` | v0 or v3 `.so` |
| Relationship | Provides `sbpf-assembler` and `sbpf-common` | Depends on them (`Cargo.toml`: `sbpf-assembler = "0.3.4"`) |

`sbpf/CONTRIBUTING.md` says this explicitly: every PR to `sbpf` is tested against `sbpf-linker`, because the linker depends on it.

---

## Part 3. v0, v1, v2, v3: the versions explained

### 3.1 Why there are versions at all

Once programs are deployed on-chain, they must keep working forever. When Solana wants to change the instruction set (fix a design mistake, add instructions, simplify the loader), it cannot change the meaning of existing bytecode. So it defines a **new version** instead. Each program declares its version in the ELF header field **`e_flags`**:

| `e_flags` | Version |
|---|---|
| `0` | v0 |
| `1` | v1 |
| `2` | v2 |
| `3` | v3 |

You can see this in `sbpf/crates/assembler/src/lib.rs` (`SbpfArch::e_flags()` returns `0` for V0 and `3` for V3) and in `sbpf/crates/disassembler/src/program.rs`, which checks `e_flags == 0x02` for v2 and `0x03` for v3.

### 3.2 What each version is

**v0: the original, "legacy" format**
- What almost every program deployed so far uses.
- Syscalls are **dynamic**: the bytecode contains `call -1` placeholders, plus relocation entries and a dynamic symbol table naming each syscall. The loader patches each call when the program is loaded.
- Addresses of read-only data (strings, tables) are fixed up through **relocations** (`R_SBF_64_RELATIVE`) at load time.
- Needs extra ELF sections: `.dynamic`, `.dynsym`, `.dynstr`, `.rel.dyn` (built in `sbpf/crates/assembler/src/program.rs`).
- Fixed 4 KB stack frame per function call.

**v1: dynamic stack frames (SIMD-0166; not emitted by these tools)**
- Relaxed the fixed 4 KB stack frame so that functions could adjust the stack pointer themselves.
- Neither `sbpf` nor `sbpf-linker` produces v1.

**v2: instruction-set rework (SIMD-0173 and SIMD-0174; not emitted by these tools)**
- Moved and repurposed opcodes. `sbpf/crates/common/src/instruction.rs` (`from_bytes_sbpf_v2`) shows the remapping: for example, opcode `0x2C` meant `mul32 dst, src` in v0 but `ldxb` in v2, and `0x9F` meant `mod64 dst, src` in v0 but `stxdw` in v2. `lddw` was replaced by a pair of instructions, and `callx` moved its register operand.
- Added "PQR" arithmetic (Product, Quotient, Remainder): `lmul`, `uhmul`, `shmul`, `udiv`, `urem`, `sdiv`, `srem`. `sbpf/docs/opcodes.md` marks these **"v2 only"**.
- The `sbpf` **disassembler** can read v2 binaries. The assembler cannot produce them.

**v3: the modern format (SIMD-0178, SIMD-0189 and SIMD-0377; default in both tools)**

Everything below is visible in the code:

| v3 feature | Where in the code |
|---|---|
| **Static syscalls**: `call` carries a 32-bit **murmur3 hash** of the syscall name. No relocations, no symbol table. | `ast.rs`: `inst.imm = murmur3_32(&syscall_name)`, `src = 0` |
| **Fixed virtual addresses**: read-only data at `0x0`, bytecode at `0x1_0000_0000` | `header.rs`: `V3_RODATA_VADDR = 0 << 32`, `V3_BYTECODE_VADDR = 1 << 32` |
| **Minimal ELF**: 1 program header (code) or 2 (rodata + code). **No section headers** unless built with debug info. | `program.rs`: "v3 programs are loaded entirely through program headers; the loader never reads the section header table" |
| **No load-time relocations**: `lddw r1, label` resolves directly to the final address | `ast.rs`: relocations added `only for v0` |
| **`jmp32` instructions** (`jeq32`, `jgt32`, ...) compare only the low 32 bits | `opcodes.md`: marked "v3 only" |
| **Internal calls** are PC-relative with `src = 1`; `callx` puts its register in the `dst` field | `ast.rs`, `instruction.rs` |
| **Closer to upstream eBPF**, which is why `sbpf-linker` can map LLVM's BPF "cpu v4" output to it | `sbpf-linker/tests/tests.rs`: `V0 => "v2"`, `V3 => "v4"` |

Note that v3 does **not** include the v2 PQR instructions in this toolchain. v3 went back toward upstream eBPF opcodes rather than building on v2's opcode changes.

### 3.3 Side by side

| | v0 | v3 |
|---|---|---|
| `e_flags` | 0 | 3 |
| Syscall encoding | `call -1` + relocation + `.dynsym` entry | `call <murmur3 hash>` (static) |
| Rodata address | File offset, patched by relocation | Fixed `0x0 + offset` |
| Code address | File offset | Fixed `0x1_0000_0000 + offset` |
| ELF sections needed | `.text .rodata .dynamic .dynsym .dynstr .rel.dyn .shstrtab` | None (program headers only) |
| `jmp32` family | ✗ (assembler error) | ✓ |
| Calculator example size | 1.6 KB | 760 bytes |
| `sbpf build` flag | `--arch v0` | `--arch v3` (default) |
| LLVM CPU used by `sbpf-linker` | `v2` (also accepts `generic`, `v1`) | `v4` |

### 3.4 Which should you learn?

**Write v3, understand v0.**

- The **assembly you write is about 95% the same** for both. The calculator in Part 9 assembles unchanged with `--arch v0` and `--arch v3`. The only source-level difference is that `jeq32`-style instructions are v3-only.
- v3 is the default in both tools and produces smaller, simpler binaries.
- **v3 is live on every public cluster**, so you can deploy v3 programs anywhere (see the table below).
- You still need v0 knowledge because **most existing programs you'll disassemble are v0**.

### 3.5 Where each version is enabled

Each version is switched on per cluster by a **feature gate**: an on-chain flag that activates at a specific epoch. Queried with `solana feature status -u <cluster> --display-all <feature-id>` (in November 2026):

| Version | SIMDs | Feature id | mainnet-beta | testnet | devnet |
|---|---|---|---|---|---|
| v1 | 0166 | `JE86WkYvTrzW8HgNmrHY7dFYpCmSptUpKupbo2AdQ9cG` | active since epoch 809 | — | active since epoch 902 |
| v2 | 0173, 0174 | `F6UVKh1ujTEFK3en2SyAL3cdVnqko1FVEXWhmdLRu6WP` | active since epoch 825 | — | active since epoch 918 |
| v3 | 0178, 0189, 0377 | `5cC3foj77CWun58pC51ebHFUWavHWKarWyR5UUik7dnC` | active since epoch 993 | active since epoch 954 | active since epoch 1069 |

("—" = not queried.)

Coming next, still **inactive** on devnet: SIMD-0500 ("Disable deployment of SBPF v0, v1 and v2 programs", feature `B8JJXCy5amZyWG9r7EnUYLwzXSXTxG7GZ1qZ1qggo83g`). Once that activates on a cluster, **new deployments there must be v3**. Programs already deployed as v0 keep running, since separate gates control execution. That's one more reason to write v3.

Older features are hidden by default, so the `--display-all` flag matters. Without it, these gates don't show up in `solana feature status` at all.

---

## Part 4. The virtual machine: registers, memory, stack, compute

`sbpf/crates/vm/` is a readable, simplified sBPF VM (used by the `sbpf debug` debugger). It's the best place to see the rules in code.

### 4.1 Registers

There are eleven 64-bit registers (`registers: [u64; 11]` in `vm.rs`):

| Register | Role | Who preserves it across `call`? |
|---|---|---|
| `r0` | **Return value.** Your program's exit code; also each syscall's result. `0` = success. | Overwritten by calls |
| `r1` | **1st argument.** At program start: a pointer to the **input buffer** (`0x4_0000_0000`). | Clobbered by calls |
| `r2`–`r5` | 2nd–5th arguments / scratch | Clobbered by calls |
| `r6`–`r9` | General purpose, **callee-saved** | Preserved: the VM saves and restores them on `call`/`exit` |
| `r10` | **Frame pointer**: the top of the current stack frame. Read-only. | Managed by the VM |

Code proof (`sbpf/crates/common/src/execute/call.rs`): on `call`, the VM saves `r6`–`r9` and `r10`, then adds the frame size to `r10`. On `exit`, it restores them. If the call stack is empty, `exit` ends the program with `r0` as the result.

**Rule of thumb:** anything you need after a `call` (including a syscall) must live in `r6`–`r9` or on the stack. The most common beginner bug is keeping the input pointer in `r1` and losing it at the first syscall.

### 4.2 The memory map

There is no single flat memory. The VM translates **virtual addresses** into separate regions (`sbpf/crates/vm/src/memory.rs`):

```
 virtual address          region      access       what lives there
 ────────────────────────────────────────────────────────────────────────
 0x0_0000_0000            rodata      read-only    strings, constants, tables (.rodata)
 0x1_0000_0000            bytecode    (v3)         your instructions (not data-readable)
 0x2_0000_0000            stack       read/write   4 KB per call frame
 0x3_0000_0000            heap        read/write   32 KB by default
 0x4_0000_0000            input       read/write*  accounts + instruction data + program id
```

\* The VM lets you write anywhere in the input region, but the **runtime checks afterwards** that you only changed what you're allowed to (for example, only writable accounts that your program owns). An illegal change fails the transaction.

Reads or writes outside a region (or past its end) abort the program with an access violation. Writing to rodata is always an error.

### 4.3 The stack

- At program start, `r10 = 0x2_0000_0000 + 4096`: the **top** of the first 4 KB frame (`initial_frame_pointer()`).
- You use the stack with **negative offsets** from `r10`:
  ```asm
  stxdw [r10 - 8], r2     # store 8 bytes just below the frame top
  ldxdw r3, [r10 - 8]     # load them back
  ```
  You can't move `r10` itself, so copy it to make a pointer you can pass to a syscall:
  ```asm
  mov64 r1, r10
  sub64 r1, 32            # r1 = pointer to a 32-byte buffer on the stack
  ```
- On `call`, the new frame starts 4 KB higher (`r10 += 4096`). Frames are isolated: each function gets its own 4 KB.
- Max call depth is 64 (`SbpfVmConfig::default()`), so the stack is 64 × 4 KB.

### 4.4 The heap

32 KB starting at `0x3_0000_0000`. Most assembly programs don't need it; the stack is usually enough. It is there if you need larger scratch space.

### 4.5 Compute units

Every instruction costs 1 compute unit (CU) (`compute_meter.consume(1)` in `vm.rs`). Syscalls cost more; for example, logging costs about 100 CUs. The default VM limit is 1,400,000. When the meter hits zero, the program aborts.

Real numbers from the calculator tests (Part 9): a successful `add` uses **~144 CUs**, of which ~100 are the logging syscall. The error paths use **5–41 CUs**. A typical Anchor instruction uses thousands. Low CU usage is the main reason people write assembly.

---

## Part 5. Instructions: encoding and the full instruction set

### 5.1 The 8-byte instruction format

Every instruction is exactly **8 bytes**. The one exception is `lddw`, which is **16 bytes** because it carries a full 64-bit constant.

```
 byte:   0          1                 2      3      4      5      6      7
       ┌────────┬────────┬────────┬──────────────┬────────────────────────────┐
       │ opcode │ src(4) │ dst(4) │  offset (16) │      immediate (32)        │
       └────────┴────────┴────────┴──────────────┴────────────────────────────┘
                  high      low      signed, LE        signed, little-endian
                  nibble    nibble
```

From `instruction.rs` (`to_bytes_inner`):
```rust
let mut b = vec![self.opcode.into(), src_val << 4 | dst_val];
b.extend_from_slice(&off_val.to_le_bytes());       // 2 bytes
b.extend_from_slice(&(imm_val as i32).to_le_bytes()); // 4 bytes
```

**Worked example**, `ldxdw r2, [r6 + 0x58]`:
- opcode `ldxdw` = `0x79`
- dst = 2, src = 6 → byte 1 = `0x62`
- offset = `0x58` → `58 00`
- imm unused → `00 00 00 00`
- result: `79 62 58 00 00 00 00 00`

### 5.2 How opcodes are built

The opcode byte isn't arbitrary. Its low 3 bits are the **class**:

| Low 3 bits | Class | Meaning |
|---|---|---|
| `0` | LD | 64-bit immediate load (`lddw`) |
| `1` | LDX | load from memory |
| `2` | ST | store an immediate to memory |
| `3` | STX | store a register to memory |
| `4` | ALU32 | 32-bit arithmetic |
| `5` | JMP | 64-bit jumps, call, exit |
| `6` | JMP32 | 32-bit jumps (v3) |
| `7` | ALU64 | 64-bit arithmetic |

For ALU and JMP classes, bit 3 (`0x08`) is the **source** flag: `0` means the operand is the immediate, `1` means it's the `src` register. That's why every register form is the immediate form plus 8:

- `add64 dst, imm` = `0x07`, `add64 dst, src` = `0x0f` (`0x07 | 0x08`)
- `jeq dst, imm` = `0x15`, `jeq dst, src` = `0x1d`

The assembler relies on this: `process_jump_reg` in `parser/default.rs` computes `reg_opcode = opcode | BPF_X`.

### 5.3 The full instruction set

This is `sbpf/docs/opcodes.md` (generated from `crates/common/src/opcode.rs`) reorganized with explanations. "v2 only" instructions aren't usable in v0/v3 and are left out of the tables, apart from a note.

#### Load a 64-bit constant

| Instruction | Opcode | Meaning |
|---|---|---|
| `lddw dst, imm64` | `0x18` | `dst = imm64` (full 64 bits; 16-byte instruction) |

Use it for numbers that don't fit in 32 bits, and for addresses of labels: `lddw r1, message` loads the address of `message` in rodata.

#### Memory loads (always zero-extended into the 64-bit register)

| Instruction | Opcode | Meaning |
|---|---|---|
| `ldxb dst, [src + off]` | `0x71` | load 1 byte |
| `ldxh dst, [src + off]` | `0x69` | load 2 bytes (half word) |
| `ldxw dst, [src + off]` | `0x61` | load 4 bytes (word) |
| `ldxdw dst, [src + off]` | `0x79` | load 8 bytes (double word) |

Memory is **little-endian**. Unaligned access is allowed: the calculator loads a `u64` from offset `0x50d9`.

#### Memory stores

| Instruction | Opcode | Meaning |
|---|---|---|
| `stb / sth / stw / stdw [dst + off], imm` | `0x72 0x6a 0x62 0x7a` | store an immediate (1/2/4/8 bytes) |
| `stxb / stxh / stxw / stxdw [dst + off], src` | `0x73 0x6b 0x63 0x7b` | store a register (1/2/4/8 bytes) |

Note the operand order: for stores, the **memory location comes first**.

#### 64-bit arithmetic and logic (ALU64)

All take either `dst, imm` or `dst, src`:

| Instruction | Imm / Reg opcode | Meaning |
|---|---|---|
| `add64` | `0x07 / 0x0f` | `dst += x` |
| `sub64` | `0x17 / 0x1f` | `dst -= x` |
| `mul64` | `0x27 / 0x2f` | `dst *= x` |
| `div64` | `0x37 / 0x3f` | `dst /= x` (unsigned; **dividing by 0 aborts**) |
| `mod64` | `0x97 / 0x9f` | `dst %= x` (unsigned) |
| `or64` | `0x47 / 0x4f` | `dst \|= x` |
| `and64` | `0x57 / 0x5f` | `dst &= x` |
| `xor64` | `0xa7 / 0xaf` | `dst ^= x` |
| `lsh64` | `0x67 / 0x6f` | `dst <<= x` |
| `rsh64` | `0x77 / 0x7f` | `dst >>= x` (logical: fills with 0s) |
| `arsh64` | `0xc7 / 0xcf` | `dst >>= x` (arithmetic: keeps the sign bit) |
| `mov64` | `0xb7 / 0xbf` | `dst = x` |
| `neg64 dst` | `0x87` | `dst = -dst` |

The immediate is 32-bit **signed** and gets sign-extended to 64 bits. `mov64 r1, -1` gives `0xFFFFFFFFFFFFFFFF`. `mov64 r1, 0x80000000` also gives `0xFFFFFFFF80000000`, which is probably not what you meant. For any constant above `0x7FFFFFFF`, use `lddw`.

#### 32-bit arithmetic (ALU32)

Same operations with a `32` suffix: `add32 sub32 mul32 div32 mod32 or32 and32 xor32 lsh32 rsh32 arsh32 mov32 neg32`. They operate on the low 32 bits.

How the upper 32 bits are filled is where sBPF differs from Linux eBPF: `add32`, `sub32` and `mul32` **sign-extend** the result, while the others zero-extend. This is why `sbpf-linker` runs a fix-up pass (Part 12). In hand-written code, use 64-bit ops unless you specifically want 32-bit behavior.

#### Byte order

| Instruction | Opcode | Meaning |
|---|---|---|
| `be16 / be32 / be64 dst` | `0xdc` | convert to big-endian (byte-swap on Solana's little-endian machine) |
| `le16 / le32 / le64 dst` | `0xd4` | convert to little-endian (truncates to that width) |

#### Jumps

The jump offset counts **instructions** (8-byte slots), relative to the *next* instruction. You never compute it yourself; write a label and the assembler computes `(target - current) / 8 - 1` (`ast.rs`).

| Instruction | Meaning |
|---|---|
| `ja label` | always jump |
| `jeq dst, x, label` | jump if `dst == x` |
| `jne dst, x, label` | jump if `dst != x` |
| `jgt` / `jge` / `jlt` / `jle` | unsigned `>` `>=` `<` `<=` |
| `jsgt` / `jsge` / `jslt` / `jsle` | **signed** `>` `>=` `<` `<=` |
| `jset dst, x, label` | jump if `dst & x != 0` (test bits) |

`x` is an immediate or a register. Add the `32` suffix (`jeq32`, `jne32`, ...) to compare only the low 32 bits. That form is **v3 only**; v0 rejects it with an error (`check_arch_v3` in `parser/common.rs`).

Signed versus unsigned matters. `0xFFFFFFFFFFFFFFFF` is a huge number to `jgt` but `-1` to `jsgt`. Token amounts and lamports are unsigned, so use `jgt`/`jlt` for them.

#### Calls and exit

| Instruction | Opcode | Meaning |
|---|---|---|
| `call label` | `0x85` | call a function in your program |
| `call sol_log_` | `0x85` | call a **syscall** (any name in the list in Part 8) |
| `callx reg` | `0x8d` | call the address stored in a register (function pointers) |
| `exit` | `0x95` | return from a function; at the top level, **end the program** with `r0` as the result |

The assembler tells syscalls apart from your own functions by name. If the name is in `REGISTERED_SYSCALLS` (`crates/common/src/syscalls.rs`), it's a syscall.

---

## Part 6. Assembly syntax: everything the assembler understands

The grammar is in `sbpf/crates/assembler/src/sbpf.pest`. Here's all of it in plain words.

### 6.1 Basics

```asm
# comment          ; also a comment          // also a comment

.globl entrypoint      # REQUIRED: marks where execution starts (.global also works)

entrypoint:            # a label: a name for the address of the next instruction
  mov64 r0, 0          # instructions are indented by convention (not required)
  exit
```

- **Registers**: `r0` to `r10`.
- **Numbers**: decimal `42`, hex `0x2a`, negative `-8`, underscores for readability `1_000_000`, `0xff_ff`.
- **Labels**: letters, digits and `_`, not starting with a digit.
- **Numeric labels** (reusable local labels): define `1:`, then refer to `1f` (next `1:` forward) or `1b` (previous `1:` backward). Handy for small loops:
  ```asm
  1:
    sub64 r2, 1
    jne r2, 0, 1b      # loop back to the nearest previous "1:"
  ```

### 6.2 Constants and expressions

```asm
.equ ACCOUNT_DATA, 0x0060
.equ SEED_LEN, 8 * 4

  ldxdw r2, [r1 + ACCOUNT_DATA + 8]   # memory refs allow several + / - terms
  mov64 r3, (SEED_LEN + 1) * 2        # operands allow + - * / and parentheses
```

`.equ` is the main tool for readable assembly. Name every offset.

### 6.3 Sections and data

```asm
.rodata                       # switch to read-only data
  message:   .ascii "Hello, Solana!"   # raw bytes of a string (no trailing 0)
  bytes:     .byte 1, 2, 3
  halves:    .short 1, 2               # 2 bytes each (.half also works)
  words:     .word 1, 2                # .word / .int / .long are 4 bytes each
  quads:     .quad 1, 2, some_label    # 8 bytes each; may contain label addresses
.text                         # switch back to code
```

Load a rodata address with `lddw`:

```asm
  lddw r1, message      # r1 = address of message
  mov64 r2, 14          # length
  call sol_log_         # prints "Hello, Solana!"
```

This is exactly the program `sbpf init` generates (`src/commands/common.rs`).

### 6.4 The preprocessor: includes, macros, loops, conditionals

`sbpf build` runs a preprocessor first (`crates/assembler/src/preprocessor/`). Errors still point at your original file and line, even inside macros.

```asm
.include "offsets.s"          # paste another file here (path relative to this file)

.macro CHECK_SIGNER offset, err=err_signer   # macro with a default argument
  ldxb r2, [r6 + \offset]
  jeq r2, 0, \err
.endm

  CHECK_SIGNER 0x9            # expands to the two lines above

.macro LOOP_LABEL
loop_\@:                      # \@ = unique number per expansion (avoids duplicate labels)
.endm

.macro DEF_STR name, text
\name:
  .ascii \text
\name\()_end:                 # \() separates an argument from the following text
.endm

.rept 4                       # repeat a block 4 times
  add64 r2, 1
.endr

.irp reg, r2, r3, r4          # repeat once per value
  mov64 \reg, 0
.endr

.if SOME_CONST == 1           # conditional assembly (== != < > <= >=)
  ...
.else
  ...
.endif
```

### 6.5 The LLVM dialect

The assembler also accepts LLVM's BPF syntax. This is what `llvm-objdump` prints and what you'll see from Rust/C compilers:

| Default syntax | LLVM syntax |
|---|---|
| `add64 r1, 5` | `r1 += 5` |
| `add32 r1, 5` | `w1 += 5` (`w` = 32-bit view of the register) |
| `mov64 r1, r2` | `r1 = r2` |
| `lddw r1, 0x1234567890` | `r1 = 0x1234567890 ll` |
| `ldxdw r2, [r1 + 8]` | `r2 = *(u64 *)(r1 + 8)` |
| `stxw [r1 + 4], r2` | `*(u32 *)(r1 + 4) = w2` |
| `jeq r1, 0, done` | `if r1 == 0 goto done` |
| `jsgt r1, r2, done` | `if r1 s> r2 goto done` |
| `ja done` | `goto done` |
| `neg64 r1` | `r1 = -r1` |
| `be64 r1` | `r1 = be64 r1` |

Each file uses one dialect throughout. The rest of this guide uses the default syntax.

---

## Part 7. The input buffer: how your program sees accounts

This is the most important part to get right. At entry, **`r1` points to a byte buffer** the runtime built from your instruction. There is no struct and no deserializer. You read raw bytes at offsets.

### 7.1 The layout

From `sbpf/crates/runtime/src/serialize.rs` (`serialize_parameters`):

```
u64                 number of accounts
── for each account ─────────────────────────────────────────────────
  if this account is NOT a duplicate of an earlier one:
    u8              0xff   (the "not a duplicate" marker)
    u8              is_signer       (1 / 0)
    u8              is_writable     (1 / 0)
    u8              is_executable   (1 / 0)
    [u8; 4]         padding
    [u8; 32]        pubkey
    [u8; 32]        owner
    u64             lamports
    u64             data_len
    [u8; data_len]  data
    [u8; 10240]     spare space (so the program can grow the account by up to 10 KB)
    padding         up to the next multiple of 8
    u64             rent_epoch
  if it IS a duplicate (same pubkey appeared earlier):
    u8              index of the earlier account
    [u8; 7]         padding
──────────────────────────────────────────────────────────────────────
u64                 instruction data length
[u8; len]           instruction data
[u8; 32]            program id
```

### 7.2 Offsets inside one account

If an account record starts at offset `A`:

| Field | Offset |
|---|---|
| dup marker | `A + 0` |
| is_signer | `A + 1` |
| is_writable | `A + 2` |
| is_executable | `A + 3` |
| pubkey | `A + 8` |
| owner | `A + 40` (`+0x28`) |
| lamports | `A + 72` (`+0x48`) |
| data_len | `A + 80` (`+0x50`) |
| data | `A + 88` (`+0x58`) |
| rent_epoch | `A + 88 + round_up_8(data_len + 10240)` |
| **next account starts at** | `A + 96 + round_up_8(data_len + 10240)` |

The first account starts at `A = 8`, right after the `u64` count.

### 7.3 Worked example: computing offsets by hand

Say account 0 has 0 bytes of data and account 1 has 8 bytes.

```
Account 0: A = 0x8
  is_signer  = 0x8 + 1      = 0x0009
  pubkey     = 0x8 + 8      = 0x0010
  owner      = 0x8 + 40     = 0x0030
  lamports   = 0x8 + 72     = 0x0050
  data_len   = 0x8 + 80     = 0x0058
  data       = 0x8 + 88     = 0x0060
  next       = 0x8 + 96 + round_up_8(0 + 10240) = 0x8 + 10336 = 0x2868

Account 1: A = 0x2868
  is_writable = 0x286a,  owner = 0x2890,  data_len = 0x28b8,  data = 0x28c0
  next        = 0x2868 + 96 + round_up_8(8 + 10240) = 0x2868 + 10344 = 0x50d0

Instruction data length = 0x50d0
Instruction data        = 0x50d8
Program id              = 0x50d8 + instruction_data_len
```

These match the `.equ` constants in `sbpf/examples/sbpf-asm-counter/` (where the first account is `OWNER_* = 0x0008...`) and the calculator in Part 9.

### 7.4 The key lesson: fixed offsets are assumptions

Fixed offsets are only correct if every earlier account has exactly the `data_len` you assumed, no account is a duplicate, and (for the program id) the instruction data has exactly the length you assumed. **A program that uses fixed offsets must check those assumptions before trusting them**, or an attacker can pass a different layout and make you read the wrong bytes.

Two strategies:

1. **Fixed layout + validation** (fast, what the examples do): check `num_accounts`, each earlier `data_len`, the dup markers, and the instruction-data length *before* using later offsets.
2. **Walk the buffer** (flexible): compute each account's start from the previous one's `data_len`. That's one of the exercises in Part 15.

I made exactly this mistake while writing the calculator. See section 9.5.

### 7.5 Returning results and errors

- Set `r0 = 0` and `exit` for success.
- Set `r0 = n` (small number) and `exit` to fail with **`ProgramError::Custom(n)`**. The tests in Part 9 confirm this.
- Changes to writable accounts' data and lamports (in the input region) are saved if the program succeeds, after the runtime checks they're allowed. If the program fails, all changes are discarded.

---

## Part 8. Syscalls: talking to the runtime

Your program can't do I/O on its own. Everything else (logging, hashing, PDAs, cross-program invocation, sysvars) is a **syscall**: a function provided by the validator.

### 8.1 Calling convention

- Arguments go in `r1`–`r5`, the result comes back in `r0`.
- `r1`–`r5` are **clobbered**: their values after the call are undefined. Keep what you need in `r6`–`r9` or on the stack.
- Pointers you pass must point into valid VM memory (stack, heap, input, or rodata).

### 8.2 How a syscall name becomes bytecode

`call sol_log_` is encoded differently per version (`resolve_label_references` in `ast.rs`):
- **v3**: `src = 0`, `imm = murmur3_32("sol_log_")`. The VM looks up the hash directly.
- **v0**: `src = 1`, `imm = -1`, plus a relocation that tells the loader "patch this call with `sol_log_`".

You can see a raw hash in `sbpf-linker/tests/assembly/input_loads.rs`, which calls `sol_memcmp_` by its hash `0x5FDCDE31`.

### 8.3 The full syscall list

From `sbpf/crates/common/src/syscalls.rs`. The signatures below are the commonly used ones, as used in the examples in `sbpf/examples/`.

| Syscall | Arguments (`r1`, `r2`, ...) | What it does |
|---|---|---|
| `sol_log_` | `ptr, len` | Log a UTF-8 string |
| `sol_log_64_` | `a, b, c, d, e` | Log five u64s as hex |
| `sol_log_pubkey` | `ptr_to_32_bytes` | Log a pubkey in base58 |
| `sol_log_compute_units_` | (none) | Log remaining compute units |
| `sol_log_data` | `ptr_to_slices, n` | Log binary data (base64), used for events |
| `sol_memcpy_` | `dst, src, n` | Copy n bytes (no overlap) |
| `sol_memmove_` | `dst, src, n` | Copy n bytes (overlap OK) |
| `sol_memset_` | `dst, byte, n` | Fill memory |
| `sol_memcmp_` | `a, b, n, result_ptr` | Compare; writes an i32 result (0 = equal) to `result_ptr` |
| `sol_sha256` / `sol_keccak256` / `sol_blake3` | `slices_ptr, n, out_ptr` | Hash a list of slices into 32 bytes |
| `sol_create_program_address` | `seeds_ptr, n_seeds, program_id_ptr, out_ptr` | PDA from seeds (including the bump); `r0 != 0` if the result is on the curve |
| `sol_try_find_program_address` | `seeds_ptr, n_seeds, program_id_ptr, out_ptr, bump_out_ptr` | Find PDA + bump (expensive) |
| `sol_invoke_signed_c` | `instruction_ptr, account_infos_ptr, n_infos, signer_seeds_ptr, n_signers` | Cross-program invocation (CPI) with C-layout structs |
| `sol_invoke_signed_rust` | same idea, Rust layout | CPI (used by Rust programs) |
| `sol_get_rent_sysvar`, `sol_get_clock_sysvar`, `sol_get_epoch_schedule_sysvar`, `sol_get_epoch_rewards_sysvar`, `sol_get_last_restart_slot`, `sol_get_fees_sysvar` | `out_ptr` | Copy a sysvar into your buffer |
| `sol_get_sysvar` | `sysvar_id_ptr, out_ptr, offset, length` | Read part of any sysvar |
| `sol_set_return_data` / `sol_get_return_data` | `ptr, len` / ... | Return data to the caller of a CPI |
| `sol_get_stack_height` | (none) | Current CPI depth |
| `sol_remaining_compute_units` | (none) | Returns the remaining CUs in `r0` |
| `sol_get_processed_sibling_instruction` | ... | Inspect earlier instructions |
| `sol_get_epoch_stake` | ... | Stake info |
| `sol_secp256k1_recover`, `sol_curve_*`, `sol_alt_bn128_*`, `sol_big_mod_exp`, `sol_poseidon`, `sol_sha512` | ... | Cryptography |
| `abort` / `sol_panic_` | ... | Abort the program |

**The "slice" format.** Many syscalls take an array of `{ ptr: u64, len: u64 }` pairs (16 bytes each), for example PDA seeds and hash inputs. The counter example builds three of them on the stack:

```asm
  # seeds array at r5: [ {ptr,len}, {ptr,len}, {ptr,len} ]
  mov64 r2, r5
  stxdw [r2 + 0], r9          # seed 0 ptr  -> "counter"
  lddw r3, 7
  stxdw [r2 + 8], r3          # seed 0 len  = 7
  add64 r2, 16
  ...                         # seed 1 = owner key (32 bytes), seed 2 = bump (1 byte)
  call sol_create_program_address
```

Read `sbpf/examples/sbpf-asm-counter/src/sbpf-asm-counter/sbpf-asm-counter.s` for a complete PDA + CPI (create account) program, and `sbpf-asm-vault` / `sbpf-asm-cpi` for more.

---

## Part 9. Worked example: a checked calculator (full code and tests)

This is exercise 3 from Part 15, solved completely. It's small but touches most of the skills: offsets, validation, signer/owner/writable checks, branching, arithmetic with overflow detection, writing account data, a syscall, and error codes.

**Location:** `practice/calculator/`
**Status:** `sbpf build` succeeds (760 bytes, v3) and **18/18 tests pass** with both `cargo test` and `sbpf test`.

### 9.1 The spec

```
Instruction data (17 bytes):  [op: u8][a: u64 LE][b: u64 LE]
   op: 0 = add, 1 = sub, 2 = mul, 3 = div

Accounts:
   0. authority  - must sign, must have 0 bytes of data
   1. result     - must be writable, owned by this program, exactly 8 bytes

Effect: result.data = a <op> b   (and logs the value)

Errors: 1 bad accounts | 2 not signer | 3 not writable | 4 bad data len
        5 wrong owner  | 6 bad ix len | 7 unknown op   | 8 overflow | 9 div by 0
```

### 9.2 Project layout

```
practice/calculator/
├── Cargo.toml                     # test dependencies (Mollusk etc.)
├── Cargo.lock                     # pinned versions (see section 9.6)
├── deploy/calculator-keypair.json # program keypair; build output goes here too
└── src/
    ├── calculator/calculator.s    # THE PROGRAM (sbpf build finds src/<name>/<name>.s)
    └── lib.rs                     # tests
```

### 9.3 The program: `src/calculator/calculator.s`

```asm
# =============================================================================
#  calculator.s  -  a checked u64 calculator written in raw sBPF assembly
# =============================================================================
#
#  Instruction data (17 bytes):
#     [0]      op   (u8)   0 = add, 1 = sub, 2 = mul, 3 = div
#     [1..9]   a    (u64, little-endian)
#     [9..17]  b    (u64, little-endian)
#
#  Accounts:
#     0. authority  - signer, must have 0 bytes of data
#     1. result     - writable, owned by this program, exactly 8 bytes of data
#
#  Effect: result.data = a <op> b   (and logs the result with sol_log_64_)
#
#  Error codes (returned in r0, seen by clients as ProgramError::Custom(n)):
#     1 wrong account count / layout     6 bad instruction data length
#     2 authority did not sign           7 unknown op
#     3 result account not writable      8 overflow / underflow
#     4 result account data len != 8     9 division by zero
#     5 result account not owned by us
# =============================================================================

# ---------------------------------------------------------------------------
#  Input buffer offsets (relative to r1 at entry).
#
#  The runtime serializes every account as:
#     u8  dup marker (0xff = not a duplicate)
#     u8  is_signer
#     u8  is_writable
#     u8  is_executable
#     4   padding
#     32  pubkey
#     32  owner
#     u64 lamports
#     u64 data_len
#     ..  data  (data_len bytes) + 10240 bytes of realloc space, padded to 8
#     u64 rent_epoch
#
#  Because the data region size depends on data_len, these constants are
#  ONLY correct when account 0 has 0 bytes and account 1 has 8 bytes.
#  That is why the program checks both lengths before trusting any offset
#  that comes after them.
# ---------------------------------------------------------------------------
.equ NUM_ACCOUNTS,       0x0000

# account 0: authority (data_len 0)
.equ AUTH_IS_SIGNER,     0x0009         # 0x0008 header + 1
.equ AUTH_DATA_LEN,      0x0058
# its data (0 bytes) + 10240 padding ends at 0x2860, rent_epoch = 0x2860..0x2868

# account 1: result (data_len 8)
.equ RES_DUP_MARKER,     0x2868
.equ RES_IS_WRITABLE,    0x286a         # 0x2868 header + 2
.equ RES_OWNER,          0x2890
.equ RES_DATA_LEN,       0x28b8
.equ RES_DATA,           0x28c0
# its data (8 bytes) + 10240 padding ends at 0x50c8, rent_epoch = 0x50c8..0x50d0

# instruction
.equ IX_DATA_LEN,        0x50d0
.equ IX_OP,              0x50d8
.equ IX_A,               0x50d9         # unaligned u64 - sBPF allows this
.equ IX_B,               0x50e1
.equ PROGRAM_ID,         0x50e9         # always right after instruction data

.equ IX_SIZE,            17
.equ RESULT_SIZE,        8
.equ NOT_A_DUPLICATE,    0xff

.globl entrypoint

entrypoint:
  # r1 holds the input pointer, but r1-r5 are clobbered by every call.
  # Keep it in r6 (callee-saved) so it survives the syscall at the end.
  mov64 r6, r1

  ###########################################################################
  ##  1. Validate the account layout BEFORE trusting any fixed offset      ##
  ###########################################################################
  ldxdw r2, [r6 + NUM_ACCOUNTS]
  jne r2, 2, err_accounts

  ldxdw r2, [r6 + AUTH_DATA_LEN]          # if authority had data, every
  jne r2, 0, err_accounts                 # offset below would shift

  ldxb r2, [r6 + AUTH_IS_SIGNER]
  jeq r2, 0, err_signer

  ldxb r2, [r6 + RES_DUP_MARKER]          # a duplicate is serialized as
  jne r2, NOT_A_DUPLICATE, err_accounts   # 8 bytes, not a full account

  ldxb r2, [r6 + RES_IS_WRITABLE]
  jeq r2, 0, err_not_writable

  ldxdw r2, [r6 + RES_DATA_LEN]
  jne r2, RESULT_SIZE, err_data_len

  # The program id is serialized right AFTER the instruction data, so
  # PROGRAM_ID is only correct once we know the data is exactly 17 bytes.
  # (Checking the owner first was a real bug: a 16-byte payload shifted
  # PROGRAM_ID by one byte and produced "wrong owner" instead.)
  ldxdw r2, [r6 + IX_DATA_LEN]
  jne r2, IX_SIZE, err_ix_data

  ###########################################################################
  ##  2. Owner check: result.owner == program_id (32 bytes = 4 x u64)      ##
  ###########################################################################
  ldxdw r2, [r6 + RES_OWNER + 0]
  ldxdw r3, [r6 + PROGRAM_ID + 0]
  jne r2, r3, err_owner
  ldxdw r2, [r6 + RES_OWNER + 8]
  ldxdw r3, [r6 + PROGRAM_ID + 8]
  jne r2, r3, err_owner
  ldxdw r2, [r6 + RES_OWNER + 16]
  ldxdw r3, [r6 + PROGRAM_ID + 16]
  jne r2, r3, err_owner
  ldxdw r2, [r6 + RES_OWNER + 24]
  ldxdw r3, [r6 + PROGRAM_ID + 24]
  jne r2, r3, err_owner

  ###########################################################################
  ##  3. Parse instruction data (length already checked above)             ##
  ###########################################################################
  ldxdw r7, [r6 + IX_A]                   # r7 = a
  ldxdw r8, [r6 + IX_B]                   # r8 = b
  ldxb r2, [r6 + IX_OP]                   # r2 = op

  jeq r2, 0, op_add
  jeq r2, 1, op_sub
  jeq r2, 2, op_mul
  jeq r2, 3, op_div
  ja err_unknown_op

  ###########################################################################
  ##  4. The operations. Each leaves its result in r9.                     ##
  ###########################################################################
op_add:
  mov64 r9, r7
  add64 r9, r8
  jlt r9, r7, err_overflow                # unsigned wrap-around => sum < a
  ja store

op_sub:
  jlt r7, r8, err_overflow                # a < b would underflow
  mov64 r9, r7
  sub64 r9, r8
  ja store

op_mul:
  mov64 r9, r7
  mul64 r9, r8                            # r9 = a * b (wrapping)
  jeq r7, 0, store                        # 0 * b never overflows
  mov64 r2, r9
  div64 r2, r7                            # if (a*b)/a != b it wrapped
  jne r2, r8, err_overflow
  ja store

op_div:
  jeq r8, 0, err_div_zero                 # div by 0 would abort the VM
  mov64 r9, r7
  div64 r9, r8

  ###########################################################################
  ##  5. Write the result and log it                                       ##
  ###########################################################################
store:
  stxdw [r6 + RES_DATA], r9               # result.data[0..8] = r9 (LE)

  mov64 r1, r9                            # sol_log_64_ prints r1..r5
  mov64 r2, 0
  mov64 r3, 0
  mov64 r4, 0
  mov64 r5, 0
  call sol_log_64_

  mov64 r0, 0                             # 0 = success
  exit

  ###########################################################################
  ##  Errors: put a code in r0 and exit                                    ##
  ###########################################################################
err_accounts:
  mov64 r0, 1
  exit
err_signer:
  mov64 r0, 2
  exit
err_not_writable:
  mov64 r0, 3
  exit
err_data_len:
  mov64 r0, 4
  exit
err_owner:
  mov64 r0, 5
  exit
err_ix_data:
  mov64 r0, 6
  exit
err_unknown_op:
  mov64 r0, 7
  exit
err_overflow:
  mov64 r0, 8
  exit
err_div_zero:
  mov64 r0, 9
  exit
```

### 9.4 Walking through the tricky parts

**`mov64 r6, r1` as the first instruction.** At the end we call `sol_log_64_`, and before that we need `r1` for its first argument. Every syscall can clobber `r1`–`r5`. `r6` is callee-saved, so the input pointer is safe there for the whole program.

**Validation order matters.** Each check only reads offsets that are already proven valid:
1. `NUM_ACCOUNTS` (offset 0) is always valid.
2. `AUTH_DATA_LEN` (0x58) is valid because account 0 always starts at 8.
3. Once account 0's data length is known to be 0, everything up to account 1 (`0x2868`) is known.
4. Once account 1's dup marker is `0xff` and its data length is 8, everything up to the instruction data (`0x50d0`) is known.
5. Once the instruction data length is 17, `PROGRAM_ID` (`0x50e9`) is known.

**Unsigned overflow detection without special instructions.**
- Add: if `a + b` wrapped around, the result is smaller than `a`. So `jlt r9, r7` catches it.
- Sub: underflow happens exactly when `a < b`. Check *before* subtracting.
- Mul: compute the wrapping product, then check `(a*b)/a == b`. This is valid for `a != 0`; `a == 0` can't overflow.
- Div: `div64` by zero aborts the VM with an unhelpful error. Checking first gives a clean `Custom(9)`.

**The owner check compares 32 bytes as four 8-byte loads.** That's 12 instructions and no syscall. Using `sol_memcmp_` would cost more.

**Error paths are separate tiny blocks.** Each one is `mov64 r0, N` then `exit`. The disassembly shows the assembler turns each label into a jump offset:
```
$ sbpf disassemble deploy/calculator.so
entrypoint:
  mov64 r6, r1
  ldxdw r2, [r6+0x0]
  jne r2, 0x2, jmp_01f0
  ...
```

### 9.5 The bug I hit, and why it's the most useful lesson here

The first version validated the owner **before** checking the instruction data length. The test `short_instruction_data` sends 16 bytes instead of 17 and expected `Custom(6)`. It got **`Custom(5)` (wrong owner)**:

```
CHECK FAILED: program_result
  Expected: `Failure(Custom(6))`,
 Got: `Failure(Custom(5))`
```

The reason: the program id comes **right after** the instruction data. With 16 bytes of data, the real program id starts at `0x50e8`, not `0x50e9`, so the owner comparison read shifted bytes. The fix was to move the length check above the owner check.

Here it only produced the wrong error code. In a real program, the same mistake (trusting an offset whose position depends on attacker-controlled lengths) can be a security hole. **Always validate a length before using any offset that comes after it.**

### 9.6 The tests: `src/lib.rs`

Tests use **Mollusk**, a lightweight Solana VM that runs your `.so` in-process. No validator needed.

```rust
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
```

**`Cargo.toml`** (what `sbpf init` generates, plus `solana-program-error` and `serde_json`):

```toml
[package]
name = "calculator"
version = "0.1.0"
edition = "2021"

[dependencies]

[dev-dependencies]
mollusk-svm = "0.14.0"
solana-account = "4.4.0"
solana-address = "2.7.0"
solana-instruction = "3.5.0"
solana-program-error = "3.0.1"
serde_json = "1"

[features]
test-sbf = []
```

**About the `Cargo.lock`.** With a fresh lockfile, cargo picked `solana-program-runtime 4.4.1`, which `mollusk-svm 0.14.0` doesn't compile against (`error[E0624]: method 'process_instruction' is private`). The fix was to copy `sbpf/Cargo.lock` into the project, which pins compatible versions. The practice project already includes this lockfile. If you `sbpf init` a new project and hit the same error, do the same.

### 9.7 Run it

```sh
cd practice/calculator
sbpf build            # assembles src/calculator/calculator.s -> deploy/calculator.so
sbpf test             # builds if needed, then runs cargo test-sbf
# or simply:
cargo test            # same tests (needs deploy/calculator.so to exist)

# Look at what you built:
sbpf disassemble deploy/calculator.so
sbpf build --arch v0 -d /tmp/v0   # compare: same source, v0 packaging (~1.6 KB)
```

Expected result: `test result: ok. 18 passed; 0 failed`.

---

## Part 10. The `sbpf` toolchain, command by command

Install: `cargo install sbpf` (or `cargo install --git https://github.com/blueshift-gg/sbpf.git sbpf`).

### `sbpf init [name] [--ts-tests]`
`src/commands/init.rs`. Creates:
```
name/
├── src/name/name.s           # "Hello, Solana!" program
├── src/lib.rs                # Mollusk test (default)  OR  tests/name.test.ts (--ts-tests)
├── deploy/name-keypair.json  # a fresh random program keypair
├── Cargo.toml / package.json
├── README.md, .gitignore
```

### `sbpf build [-a v0|v3] [-g] [-d DIR] [-i FILE]`
`src/commands/build.rs`.
- With no `-i`: for **every directory `src/X/`** that contains `src/X/X.s`, assembles it to `deploy/X.so`. So a project can hold several programs. It also creates a keypair in `deploy/` if there is none.
- `-i path/to/file.s`: assemble one file (output in the current directory unless you pass `-d`).
- `-a`, `--arch`: `v3` (default) or `v0`.
- `-g`, `--debug`: include DWARF debug info (line tables) for debuggers.
- Errors are printed with the file, line, and macro expansion chain.

### `sbpf test`
`src/commands/test.rs`. Builds first if `deploy/` has no `.so` files. Then:
- If `Cargo.toml` exists: runs `cargo test-sbf -- --nocapture`.
- Else if `package.json` exists: deploys, then runs `yarn test` (TypeScript tests against a local validator at `127.0.0.1:8899`).

### `sbpf deploy [name] [url]`
`src/commands/deploy.rs`. Runs `solana program deploy deploy/<name>.so --program-id deploy/<name>-keypair.json -u <url>` for one program or all of them. The default URL is `localhost`, so start `solana-test-validator` first.

### `sbpf e2e`
Build, then deploy, then test.

### `sbpf clean`
Removes build and deploy artifacts.

### `sbpf disassemble <file.so>`
`src/commands/disassemble.rs`. Turns any program into readable assembly. It handles v0, v2 and v3, and also works on programs built with Rust. Try it on programs you've built with Anchor or Pinocchio to see what they compile to.

### `sbpf debug --asm FILE.s | --elf FILE.so [--input JSON] [...]`
`src/commands/debug.rs`, `crates/debugger/`. An interactive step debugger on the built-in VM:

| REPL command | Action |
|---|---|
| `step` / `s` | execute one instruction |
| `next` / `n` | step over calls |
| `finish` / `f` | run until the current function returns |
| `continue` / `c` | run to the next breakpoint or the end |
| `break N` / `b N` | breakpoint at line N |
| `delete N` / `d N` | remove a breakpoint |
| `info breakpoints`, `info line` | show breakpoints / current line |
| `regs`, `reg N`, `setreg N V` | inspect or modify registers |
| `compute` | show compute units used |
| `help`, `quit` / `q` | |

Options: `--input` (a JSON file or string describing the instruction and accounts, base58 data; format in `sbpf/README.md`), `--compute-unit-limit` (default 1,400,000), `--max-call-depth` (64), `--heap-size` (32768), `--adapter` (machine-readable mode for editor integration).

### Custom linker script
`sbpf/README.md` mentions that you can put `src/X/X.ld` next to `X.s` to override the default layout. The default script is in `src/commands/common.rs` (`DEFAULT_LINKER`).

### WebAssembly / npm
The assembler compiles to WASM (`Makefile` targets `node`, `bundler`, `web`; see `npm/README.md`), so you can assemble programs in a browser or Node.

---

## Part 11. What the assembler produces: the ELF file

`sbpf/crates/assembler/src/program.rs` builds the file. The pipeline:

```
source.s
  → preprocessor   (.include, .macro, .rept, .irp, .if)   crates/assembler/src/preprocessor/
  → parser (pest grammar → AST)                            parser/, sbpf.pest
  → [optional optimizer: CFG passes]                       optimizer/ (off by default for .s files)
  → label resolution: jumps → offsets, syscalls → hash/reloc, lddw label → address   ast.rs
  → validation of every instruction's operands             ast.rs validate_instructions
  → ELF layout                                             program.rs
  → bytes
```

### v3 layout (what `sbpf build` emits by default)

```
┌───────────────────────────────┐ 0
│ ELF header (64 B)  e_flags=3  │  e_entry = 0x1_0000_0000 + entry offset
├───────────────────────────────┤ 64
│ PH[0] PT_LOAD  PF_R  vaddr 0  │  (only if there is rodata)
│ PH[1] PT_LOAD  PF_X  vaddr 1<<32
├───────────────────────────────┤
│ .rodata bytes                 │
│ .text bytes (instructions)    │
└───────────────────────────────┘   no section headers (unless -g)
```

### v0 layout

```
┌───────────────────────────────┐
│ ELF header       e_flags=0    │
│ PH: PT_LOAD (R+X) text+rodata │
│ PH: PT_LOAD (R)   dynsym...   │
│ PH: PT_DYNAMIC                │
├───────────────────────────────┤
│ .text  .rodata                │
│ .dynamic .dynsym .dynstr      │  ← syscall names live here
│ .rel.dyn                      │  ← "patch offset X with syscall Y" / "relocate address"
│ .shstrtab                     │
│ section headers               │
└───────────────────────────────┘
```

A v0 program with no syscalls and no rodata addresses is "static" and skips the dynamic sections (`program_is_static` in `ast.rs`).

---

## Part 12. `sbpf-linker` in depth

### 12.1 Using it

From `sbpf-linker/Readme.md`:

```sh
cargo binstall sbpf-linker                 # recommended install (prebuilt)
cargo generate --git https://github.com/blueshift-gg/solana-upstream-bpf-template
cargo +nightly build-bpf
```

The README warns that `cargo install sbpf-linker` (building from source) is for linker developers. It loads the Rust toolchain's LLVM shared library at runtime, which breaks when the toolchain's LLVM changes. The repo pins `nightly-2026-11-18` in `rust-toolchain.toml`.

Add compiler builtins (soft-float, 128-bit math, `memcpy` and friends, tuned for Solana):

```sh
cargo add solana-compiler-builtins
```
```rust
// src/lib.rs - referencing it is required, or rustc may not link it
use solana_compiler_builtins as _;
```

The linker warns if it isn't linked (`warn_if_builtins_missing`).

### 12.2 How rustc calls it

Rustc invokes `sbpf-linker` as the linker. The test harness (`tests/tests.rs`) shows the flags:

```
-C linker=sbpf-linker
-C target-cpu=v4                         (v2 for --arch v0)
-C target-feature=+allows-misaligned-mem-access
-C link-arg=--arch=v3
-C link-arg=--llvm-args=--bpf-stack-size=4096   (REQUIRED: the linker errors without it)
-C link-arg=--llvm-args=--disable-movsx         (sBPF has no sign-extending mov)
-C link-arg=--llvm-args=--disable-ldsx          (sBPF has no sign-extending loads)
--target bpfel-unknown-none
```

### 12.3 What it does, step by step

1. **Link bitcode** with `bpf-linker` (the Aya project's linker): merge all crates' LLVM bitcode, run the optimizer (`-O 0-3/s/z`), and generate a generic eBPF object. It exports `entrypoint` plus 47 math builtins (`MATH_BUILTINS`: `__multi3`, `__udivti3`, `__adddf3`, ...) so they aren't removed as dead code.
2. **Parse the object** (`src/byteparser.rs`):
   - Decodes every instruction in `.text*` into the `sbpf` assembler's AST.
   - Turns symbols into labels, and the `entrypoint` symbol into the program's entry.
   - Collects `.rodata*` and `.data.rel.ro*` sections, filling gaps with synthetic `__anon` entries (for compiler-generated lookup tables).
   - Resolves **relocations**: `lddw` of a data address becomes a label; `call` to a section-relative address becomes a call label; `call` to a named function becomes a call label, and if the name is a syscall it becomes a syscall.
   - Maps LLVM's `__bpf_trap` (from `core::intrinsics::abort` / `llvm.trap`) to the `abort` syscall.
   - Rejects 32-bit relocations in read-only data (relative lookup tables) with a clear message: *compile with `-C relocation-model=static`*.
3. **Fix stack arguments** (`src/fuse_args_stack.rs`). Upstream LLVM passes a function's 6th and later arguments through a pseudo-register `r11`. sBPF has no `r11`. Because each sBPF call frame sits exactly `stack_frame_size` bytes above the caller's, the linker rewrites:
   - caller's `stx [r11 - k]` → `stx [r10 + k]` (writes into the callee's future frame)
   - callee's `ldx [r11 + k]` → `ldx [r10 + k - frame_size]`

   It also checks that no local variable overlaps the incoming-argument area (`StackArgOverlap` error).
4. **Run sBPF optimization passes** (`sbpf/crates/analyzer/`, enabled by default with `--sbpf-optimize`):
   - **Dead function removal**: drops functions not reachable from `entrypoint`.
   - **ALU32 extension fix-up**: in eBPF every 32-bit ALU op zero-extends, but in sBPF `add32`, `sub32` and `mul32` sign-extend. For each one, the pass keeps it if the difference can't matter, promotes it to 64-bit if that's provably equal, or inserts a `mov32 rX, rX` to zero the upper half.
   - **Signed division fix-up**: LLVM cpu v4 marks signed `div`/`mod` with `off = 1`. sBPF v0/v3 only have unsigned division, so the pass expands it into sign handling plus unsigned division (`arsh64 r, 63` masks, see `tests/assembly/signed_div.rs`).
   - `--dump-cfg-dir DIR` writes `opt-before.dot` / `opt-after.dot` control-flow graphs.
5. **Emit** a v0 or v3 ELF with the same `Program::from_parse_result` the assembler uses, written to the output path and to `target/deploy/<name>.so` (with the `lib` prefix removed).

### 12.4 Useful flags

| Flag | Meaning |
|---|---|
| `--arch v0\|v3` (hidden, default `v3`) | sBPF version to emit. v0 only accepts `--cpu generic/v1/v2`. |
| `--cpu` | LLVM BPF CPU (`generic`, `probe`, `v1`–`v4`). Rustc passes it from `-C target-cpu`. |
| `-O` | LLVM optimization level (`0-3`, `s`, `z`) |
| `--emit llvm-ir\|asm\|llvm-bc\|obj` | Intermediate output for inspection |
| `--dump-module PATH` | Dump final LLVM IR |
| `--sbpf-optimize=false` | Disable the sBPF passes |
| `--export sym1,sym2` / `--export-symbols FILE` | Keep these symbols |
| `--btf`, `--allow-bpf-trap`, `--unroll-loops`, `--ignore-inline-never` | Pass-throughs to bpf-linker |
| `--log-level debug --log-file PATH` | Tracing |

### 12.5 How its tests work (good learning material)

`sbpf-linker/tests/assembly/*.rs` are small Rust programs with `// CHECK:` lines describing the expected assembly output. They're a great way to see **how Rust constructs become sBPF**: signed division, u128 division, function pointers in rodata, more than 5 arguments, `memcmp` lowering, float conversions. Run them with `cargo test --test tests`.

---

## Part 13. How the `sbpf` codebase is organized

```
sbpf/
├── src/                      the CLI (main.rs + commands/*.rs)
├── crates/
│   ├── common/               the instruction set: Opcode table (opcode.rs), Instruction
│   │   │                     encode/decode (instruction.rs, decode.rs), validation, the
│   │   │                     execution semantics of every instruction (execute/*.rs),
│   │   │                     syscall names + murmur3 hashes (syscalls.rs)
│   │   └── macros/tablegen   proc-macro that generates opcode tables + docs from #[opcode(...)]
│   ├── assembler/            preprocessor → pest parser → AST → label resolution → ELF
│   ├── disassembler/         ELF → instructions/rodata/relocations → assembly text
│   ├── ir/                   control-flow graph (CFG), dataflow, graph traversal engines
│   ├── analyzer/             optimization/fix-up passes on the CFG (used by sbpf-linker)
│   ├── vm/                   a simple interpreter: registers, memory regions, call frames
│   ├── runtime/              Solana-like runtime around the VM: input serialization,
│   │                         syscalls (log, memory, PDA, crypto, sysvars), CPI to System Program
│   ├── debugger/             REPL + adapter on top of runtime/vm
│   └── syscall-map/          hash → syscall lookup tables
├── examples/                 sbpf-asm-counter, sbpf-asm-vault, sbpf-asm-cpi (+ Mollusk tests)
├── docs/opcodes.md           auto-generated opcode reference
└── npm/                      WASM build of the assembler
```

**Suggested reading order:**
1. `docs/opcodes.md`
2. `crates/common/src/execute/alu64.rs`, `jump.rs`, `call.rs` (exact semantics)
3. `crates/vm/src/memory.rs`, `vm.rs`
4. `crates/runtime/src/serialize.rs` (the input buffer)
5. `examples/sbpf-asm-counter` (a real program)
6. `crates/assembler/src/ast.rs`, `program.rs` (how assembly becomes ELF)
7. `sbpf-linker/src/byteparser.rs` (how upstream eBPF becomes sBPF)

---

## Part 14. Gotchas and mistakes to avoid

1. **Losing the input pointer.** `r1`–`r5` are clobbered by every `call`. Save `r1` in `r6`–`r9` first.
2. **Trusting offsets before checking lengths.** Account offsets depend on earlier accounts' `data_len`. The program id offset depends on the instruction data length (section 9.5).
3. **Ignoring duplicate accounts.** If the same pubkey is passed twice, the second entry is only 8 bytes. Check the `0xff` marker.
4. **Missing signer, writable, or owner checks.** The runtime won't stop you from reading a non-signer's "signature flag" and continuing. You must check it yourself.
5. **Large immediates.** `mov64 r1, 0x80000000` sign-extends to `0xFFFFFFFF80000000`. Use `lddw` for anything above `0x7FFFFFFF`.
6. **Signed versus unsigned jumps.** Use `jgt/jlt/...` for amounts; `jsgt/jslt/...` treat the top bit as a sign.
7. **32-bit ALU surprises.** `add32/sub32/mul32` sign-extend in sBPF. Prefer 64-bit ops.
8. **Division by zero** aborts the program. Check the divisor first.
9. **Writing to rodata** (strings, constants) is an access violation. Copy to the stack first.
10. **Using `jeq32` and friends with `--arch v0`** gives an assembler error. They are v3 only.
11. **Deploying v3 on a cluster without the v3 gate.** v3 is active on mainnet, testnet and devnet (Part 3.5), but a local or custom validator might not have the feature active. If the loader rejects the ELF there, rebuild with `--arch v0`. A local `solana-test-validator` (Agave 4.1.2) accepted the v3 calculator.
12. **The init template's program id in tests.** The generated test does `std::fs::read("deploy/...-keypair.json")[..32]`, which takes the first 32 bytes of the *JSON text*, not the public key. That's fine for Mollusk (any id works), but it isn't your real deployed address. The calculator tests parse the JSON and take bytes `32..64` (the real public key).
13. **Mollusk/Agave version mismatches** (`method ... is private` compile errors). Reuse `sbpf/Cargo.lock` (section 9.6).
14. **The stack is 4 KB per frame.** Large buffers belong in the heap or need to be split.
15. **Stores take memory first:** `stxdw [r1 + 8], r2`, not `stxdw r2, [r1 + 8]`.

---

## Part 15. Practice exercises

Do them in order. Each builds on the previous one. For every exercise: write Mollusk tests for the success path **and** every error path, then run `sbpf disassemble` and `sbpf debug` on your result.

### Level 1: get comfortable

1. **Hello, modified.** Start from `sbpf init`. Log your own string, then call `sol_log_compute_units_`. Change the string length and see what happens if it's wrong (too long goes out of bounds).
2. **Echo.** Log the instruction data length with `sol_log_64_`. Then log the first 8 bytes as a u64, but only if the length is at least 8 (otherwise return error 1).
3. **Checked calculator.** ✅ **Solved in Part 9** (`practice/calculator/`). Extension: add `op 4 = mod` and `op 5 = pow` (a loop with overflow checks).

### Level 2: accounts

4. **Signer-gated counter (no PDA).** Account 0 is the authority (signer). Account 1 is an 8-byte account owned by the program that stores `u64 count` and an increment. Check signer, writable, owner, and data length. Extension: store the authority's pubkey in the account (40 bytes) and only allow that authority to increment it (a 32-byte compare).
5. **Account walker.** Accept **any number** of accounts with **any data lengths**. Walk the buffer in a loop. For each account, compute the next offset as `start + 96 + round_up_8(data_len + 10240)` (round up with `add64 rX, 7` then `and64 rX, -8`). Handle duplicates (8-byte records). Log the number of signers and the total lamports. This is the most important exercise: after it, offsets will never scare you again.
6. **Lamport mover (no CPI).** Move `amount` lamports from account 1 to account 2. Both must be owned by your program (the runtime only allows you to debit accounts you own). Check for underflow and overflow. Test that total lamports are conserved.

### Level 3: syscalls and functions

7. **Write a function.** Implement `memeq32(a_ptr, b_ptr) -> r0` as a separate label you `call`. It should compare 32 bytes using four `ldxdw`. Use it for owner checks. Confirm that `r6`–`r9` survive the call by using them before and after.
8. **Hashing.** Compute `sha256(instruction_data)` with `sol_sha256` (build one `{ptr, len}` slice on the stack) and write the 32-byte result into an account.
9. **PDA verification.** Re-implement the PDA part of `examples/sbpf-asm-counter`: seeds `["counter", owner_pubkey, bump]` with `sol_create_program_address`, then compare to account 1's key.

### Level 4: CPI

10. **System transfer via CPI.** Study `examples/sbpf-asm-cpi`. Then write your own program that transfers lamports from a signer to a recipient by building a System Program `Transfer` instruction (discriminator `2u32` + `u64 lamports`) and calling `sol_invoke_signed_c` with no signer seeds.
11. **PDA-signed transfer (vault).** Study `examples/sbpf-asm-vault`. Write a vault that a user deposits into, and that only the original depositor can withdraw from (CPI signed with the PDA's seeds).

### Level 5: the bigger picture

12. **Rust versus assembly.** Write the calculator in `no_std` Rust using the upstream template + `sbpf-linker` (Part 12). Compare the `.so` size and CUs with your assembly version. Disassemble both and read the differences.
13. **Read real programs.** Dump a deployed program (`solana program dump <PROGRAM_ID> out.so`), run `sbpf disassemble out.so`, and annotate its entrypoint: where does it read the account count? Where does it dispatch on the instruction discriminator?
14. **v0 versus v3.** Build the same program with both `--arch` values. Disassemble both, and use a hex viewer (`xxd`) to find the `e_flags` byte and the encoding of a `call sol_log_` in each.

---

## Appendix A. Cheat sheet

```asm
# ── skeleton ──────────────────────────────────────────────
.equ SOME_OFFSET, 0x58
.globl entrypoint
entrypoint:
  mov64 r6, r1                 # save input pointer
  ...
  mov64 r0, 0                  # success
  exit
.rodata
  msg: .ascii "hi"

# ── registers ─────────────────────────────────────────────
# r0 return | r1-r5 args (clobbered by call) | r6-r9 saved | r10 frame ptr (read-only)

# ── memory ────────────────────────────────────────────────
ldxdw r2, [r6 + 8]     # load u64     (ldxb/ldxh/ldxw for 1/2/4 bytes)
stxdw [r6 + 8], r2     # store reg    (stxb/stxh/stxw)
stdw  [r10 - 8], 5     # store imm    (stb/sth/stw)
lddw  r1, msg          # 64-bit constant or label address

# ── math (64-bit; append 32 for 32-bit) ──────────────────
add64 sub64 mul64 div64 mod64 and64 or64 xor64 lsh64 rsh64 arsh64 mov64 neg64

# ── branches ──────────────────────────────────────────────
ja L | jeq jne jgt jge jlt jle jset (unsigned) | jsgt jsge jslt jsle (signed)
# v3 only: jeq32 ... (compare low 32 bits)

# ── calls ─────────────────────────────────────────────────
call my_function | call sol_log_ | callx r3 | exit

# ── memory map ────────────────────────────────────────────
# 0x0 rodata | 0x1_0000_0000 code (v3) | 0x2_0000_0000 stack | 0x3_0000_0000 heap
# 0x4_0000_0000 input (r1)

# ── input buffer (first account starts at 8) ─────────────
# A+0 dup(0xff) A+1 signer A+2 writable A+3 exec A+8 key A+40 owner
# A+72 lamports A+80 data_len A+88 data
# next = A + 96 + round_up_8(data_len + 10240)
# after accounts: u64 ix_len, ix data, 32-byte program id

# ── commands ──────────────────────────────────────────────
# sbpf init NAME | sbpf build [-a v0|v3] [-g] | sbpf test | sbpf deploy
# sbpf disassemble X.so | sbpf debug --asm X.s --input in.json
```
