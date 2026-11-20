# =============================================================================
#  calculator.s  -  a checked u64 calculator written in raw sBPF assembly
# =============================================================================
#
#  WHAT IT DOES
#  ------------
#  A client sends:  op (1 byte) + a (8 bytes) + b (8 bytes)  = 17 bytes.
#  The program computes  a <op> b  and stores the 8-byte answer inside an
#  account that this program owns.
#
#     op = 0 -> a + b        op = 2 -> a * b
#     op = 1 -> a - b        op = 3 -> a / b
#
#  Accounts the client must pass, in this order:
#     account 0 "authority" : must sign the transaction, must hold 0 bytes of data
#     account 1 "result"    : must be writable, owned by this program, 8 bytes of data
#
#  ERROR CODES (the number we leave in r0; clients see ProgramError::Custom(n))
#     1 wrong number/layout of accounts   6 instruction data is not 17 bytes
#     2 authority did not sign            7 unknown op
#     3 result account not writable       8 overflow / underflow
#     4 result account is not 8 bytes     9 division by zero
#     5 result account not owned by us
#
#  HOW TO READ THIS FILE
#  ---------------------
#  - Anything after `#` is a comment. The assembler ignores it.
#  - `r0` .. `r10` are the VM's 11 registers, each holds a 64-bit number.
#  - `[r6 + X]` means "the memory at address (value of r6) + X".
#  - `name:` is a label: a name for the address of the next instruction.
#  - `.equ NAME, VALUE` defines a constant; the assembler replaces NAME with
#    VALUE everywhere. It produces no code at all.
# =============================================================================


# =============================================================================
#  PART 1: WHERE THE 0x.... NUMBERS COME FROM
# =============================================================================
#
#  When the program starts, register r1 holds the address of ONE big byte
#  buffer that the Solana runtime built from the transaction. There are no
#  structs: we read raw bytes at fixed distances ("offsets") from r1.
#
#  The runtime writes the buffer in this exact order
#  (source: sbpf/crates/runtime/src/serialize.rs):
#
#    u64   number of accounts                                   <- offset 0
#    then, for EACH account, one "account record":
#       +0   u8   0xff  (means "not a duplicate of an earlier account")
#       +1   u8   is_signer      (1 = yes, 0 = no)
#       +2   u8   is_writable    (1 = yes, 0 = no)
#       +3   u8   is_executable  (1 = it is a program)
#       +4   4 bytes of padding  (unused zeros, keeps the next field aligned)
#       +8   32 bytes  the account's address (pubkey)
#       +40  32 bytes  the owner program's address
#       +72  u64       lamports
#       +80  u64       data_len  (how many bytes of data the account holds)
#       +88  the data itself, then 10240 spare zero bytes (room for the
#            program to grow the account), then zeros up to a multiple of 8
#       ...  u64       rent_epoch
#    u64   instruction data length
#    ..    instruction data (our 17 bytes)
#    32    program id (this program's own address)
#
#  So each account record is:
#       88 + round_up_to_8(data_len + 10240) + 8   bytes long.
#
#  Working it out for THIS program (authority has 0 bytes, result has 8):
#
#    0x0000 = 0      number of accounts (u64)
#    0x0008 = 8      authority record starts (right after the 8-byte count)
#      +1  -> 0x0009 authority.is_signer
#      +80 -> 0x0058 authority.data_len
#      record size = 88 + round8(0 + 10240) + 8 = 88 + 10240 + 8 = 10336
#    0x2868 = 8 + 10336 = 10344        result record starts
#      +0  -> 0x2868 result dup marker
#      +2  -> 0x286a result.is_writable
#      +40 -> 0x2890 result.owner
#      +80 -> 0x28b8 result.data_len
#      +88 -> 0x28c0 result.data          (our 8-byte answer goes here)
#      record size = 88 + round8(8 + 10240) + 8 = 88 + 10248 + 8 = 10344
#    0x50d0 = 10344 + 10344 = 20688   instruction data length (u64)
#    0x50d8 = 20688 + 8                instruction data starts:
#       0x50d8  op  (1 byte)
#       0x50d9  a   (8 bytes)   = 0x50d8 + 1
#       0x50e1  b   (8 bytes)   = 0x50d9 + 8
#    0x50e9 = 0x50d8 + 17              program id (32 bytes)
#
#  You don't need to do this by hand: run
#       python3 practice/offsets.py auth:0 result:8 --ix 17
#  and it prints all of these as ready-to-paste .equ lines.
#
#  IMPORTANT: these numbers are only correct if the accounts really have those
#  data lengths. If the authority had 3 bytes of data, everything after it
#  would move. That's why the program CHECKS every length below before it
#  trusts any offset that comes after it.
# =============================================================================

.equ NUM_ACCOUNTS,       0x0000         # u64: how many accounts were passed

# ---- account 0: authority (we require data_len == 0) -----------------------
.equ AUTH_IS_SIGNER,     0x0009         # 0x0008 (record start) + 1
.equ AUTH_DATA_LEN,      0x0058         # 0x0008 (record start) + 80

# ---- account 1: result (we require data_len == 8) --------------------------
.equ RES_DUP_MARKER,     0x2868         # record start + 0   (0xff = not a duplicate)
.equ RES_IS_WRITABLE,    0x286a         # record start + 2
.equ RES_OWNER,          0x2890         # record start + 40  (32 bytes)
.equ RES_DATA_LEN,       0x28b8         # record start + 80
.equ RES_DATA,           0x28c0         # record start + 88  (our 8-byte answer)

# ---- instruction data --------------------------------------------------------
.equ IX_DATA_LEN,        0x50d0         # u64: length of the instruction data
.equ IX_OP,              0x50d8         # byte 0 of the instruction data
.equ IX_A,               0x50d9         # bytes 1..9  (not a multiple of 8: sBPF allows that)
.equ IX_B,               0x50e1         # bytes 9..17
.equ PROGRAM_ID,         0x50e9         # right after the 17 instruction-data bytes

# ---- plain constants (just numbers, not addresses) ---------------------------
.equ IX_SIZE,            17             # 1 (op) + 8 (a) + 8 (b)
.equ RESULT_SIZE,        8              # the result account holds one u64
.equ NOT_A_DUPLICATE,    0xff           # dup-marker value for a normal account


# `.globl entrypoint` tells the assembler: execution starts at the label
# called `entrypoint`. Every program needs exactly one.
.globl entrypoint

entrypoint:
  # On entry:  r1 = address of the input buffer described above
  #            r10 = top of our 4 KB stack frame (we don't use the stack here)
  #
  # Copy r1 into r6. r1-r5 get overwritten by every `call` (including the
  # syscall at the end), but r6-r9 are preserved across calls. r6 holds the
  # buffer address for the rest of the program.
  mov64 r6, r1                            # r6 = r1  (input buffer address)

  # ===========================================================================
  #  STEP 1: validate the layout BEFORE trusting any offset
  #  Each check only reads offsets already proven valid by the checks above it.
  # ===========================================================================

  # --- exactly 2 accounts? ----------------------------------------------------
  ldxdw r2, [r6 + NUM_ACCOUNTS]           # r2 = 8 bytes at buffer+0 (account count)
  jne r2, 2, err_accounts                 # if r2 != 2, jump to err_accounts

  # --- authority holds 0 bytes of data? --------------------------------------
  # Offset 0x58 is always valid: account 0 always starts at byte 8.
  ldxdw r2, [r6 + AUTH_DATA_LEN]          # r2 = authority.data_len
  jne r2, 0, err_accounts                 # any data would shift every later offset

  # --- authority signed the transaction? ------------------------------------
  ldxb r2, [r6 + AUTH_IS_SIGNER]          # r2 = 1 byte (ldxb = load one byte)
  jeq r2, 0, err_signer                   # 0 means "did not sign"

  # From here on we know account 1 starts at 0x2868, because account 0 had 0 bytes.

  # --- account 1 is a full record, not a duplicate? -------------------------
  # If the client passed the same address twice, the runtime writes only an
  # 8-byte "see account #N" stub instead of a full record, and every offset
  # after it would be wrong.
  ldxb r2, [r6 + RES_DUP_MARKER]          # r2 = first byte of account 1's record
  jne r2, NOT_A_DUPLICATE, err_accounts   # must be 0xff

  # --- result account writable? ----------------------------------------------
  ldxb r2, [r6 + RES_IS_WRITABLE]         # r2 = 1 if writable
  jeq r2, 0, err_not_writable             # we're about to write to it

  # --- result account holds exactly 8 bytes? --------------------------------
  ldxdw r2, [r6 + RES_DATA_LEN]           # r2 = result.data_len
  jne r2, RESULT_SIZE, err_data_len       # must be 8 (room for one u64)

  # From here on we know the instruction data starts at 0x50d8.

  # --- instruction data exactly 17 bytes? -----------------------------------
  # PROGRAM_ID sits right AFTER the instruction data, so its offset is only
  # right once we know the data is 17 bytes. (An earlier version did the
  # owner check first. A 16-byte payload then moved the program id by one
  # byte and gave the wrong error. A test caught it.)
  ldxdw r2, [r6 + IX_DATA_LEN]            # r2 = instruction data length
  jne r2, IX_SIZE, err_ix_data            # must be 17

  # ===========================================================================
  #  STEP 2: owner check. Is the result account owned by THIS program?
  #  Without it, an attacker could pass an account owned by another program.
  #  (The runtime would reject the write anyway, but checking gives a clean
  #  error code.)
  #  An address is 32 bytes = four 8-byte chunks, so we compare 4 times.
  # ===========================================================================
  ldxdw r2, [r6 + RES_OWNER + 0]          # r2 = owner bytes 0..8
  ldxdw r3, [r6 + PROGRAM_ID + 0]         # r3 = program id bytes 0..8
  jne r2, r3, err_owner                   # different -> not ours
  ldxdw r2, [r6 + RES_OWNER + 8]          # bytes 8..16
  ldxdw r3, [r6 + PROGRAM_ID + 8]
  jne r2, r3, err_owner
  ldxdw r2, [r6 + RES_OWNER + 16]         # bytes 16..24
  ldxdw r3, [r6 + PROGRAM_ID + 16]
  jne r2, r3, err_owner
  ldxdw r2, [r6 + RES_OWNER + 24]         # bytes 24..32
  ldxdw r3, [r6 + PROGRAM_ID + 24]
  jne r2, r3, err_owner
  # (calling sol_memcmp_ would also work, but a syscall costs ~100 compute
  #  units; these 12 instructions cost 12.)

  # ===========================================================================
  #  STEP 3: read the instruction data
  #  Kept in r7/r8 (callee-saved) so they would survive a call if we made one.
  # ===========================================================================
  ldxdw r7, [r6 + IX_A]                   # r7 = a  (8 bytes, little-endian u64)
  ldxdw r8, [r6 + IX_B]                   # r8 = b
  ldxb r2, [r6 + IX_OP]                   # r2 = op (1 byte)

  # A "switch" in assembly is just a chain of compare-and-jump.
  jeq r2, 0, op_add                       # op == 0 -> add
  jeq r2, 1, op_sub                       # op == 1 -> subtract
  jeq r2, 2, op_mul                       # op == 2 -> multiply
  jeq r2, 3, op_div                       # op == 3 -> divide
  ja err_unknown_op                       # `ja` = jump always: none matched

  # ===========================================================================
  #  STEP 4: the operations. Each one leaves its answer in r9 and jumps to
  #  `store`. The math wraps around silently on overflow (like Rust's
  #  wrapping_add), so we detect overflow ourselves.
  # ===========================================================================
op_add:
  mov64 r9, r7                            # r9 = a
  add64 r9, r8                            # r9 = a + b  (wraps on overflow)
  jlt r9, r7, err_overflow                # if it wrapped, the sum is SMALLER than a
                                          # (jlt = unsigned "less than")
  ja store                                # done, go save it

op_sub:
  jlt r7, r8, err_overflow                # a < b would go below zero: check BEFORE subtracting
  mov64 r9, r7                            # r9 = a
  sub64 r9, r8                            # r9 = a - b  (safe now)
  ja store

op_mul:
  mov64 r9, r7                            # r9 = a
  mul64 r9, r8                            # r9 = a * b  (keeps only the low 64 bits if it overflowed)
  jeq r7, 0, store                        # 0 * anything = 0, can't overflow (and we can't divide by a=0 below)
  mov64 r2, r9                            # r2 = the product
  div64 r2, r7                            # r2 = product / a
  jne r2, r8, err_overflow                # if (a*b)/a != b, the product wrapped
  ja store

op_div:
  jeq r8, 0, err_div_zero                 # dividing by zero aborts the whole VM, so catch it first
  mov64 r9, r7                            # r9 = a
  div64 r9, r8                            # r9 = a / b  (unsigned, rounds down)
                                          # no jump needed: `store` is the very next line

  # ===========================================================================
  #  STEP 5: save the answer into the result account and log it
  # ===========================================================================
store:
  stxdw [r6 + RES_DATA], r9               # write r9's 8 bytes into result.data[0..8]
                                          # (stxdw = store a register as 8 bytes)
                                          # The runtime saves this change because
                                          # the account is writable and owned by us.

  # Syscall arguments go in r1..r5. sol_log_64_ prints all five as hex,
  # e.g. "Program log: 0x2a, 0x0, 0x0, 0x0, 0x0".
  mov64 r1, r9                            # 1st value to print: the answer
  mov64 r2, 0                             # the other four: zero
  mov64 r3, 0
  mov64 r4, 0
  mov64 r5, 0
  call sol_log_64_                        # ask the runtime to log; overwrites r0-r5

  mov64 r0, 0                             # r0 = 0 means SUCCESS
  exit                                    # end the program; the runtime reads r0

  # ===========================================================================
  #  ERRORS. Each one puts its code in r0 and exits. Any non-zero r0 fails the
  #  transaction, and the runtime throws away every change we made.
  # ===========================================================================
err_accounts:
  mov64 r0, 1                             # Custom(1): wrong account count/layout
  exit
err_signer:
  mov64 r0, 2                             # Custom(2): authority didn't sign
  exit
err_not_writable:
  mov64 r0, 3                             # Custom(3): result not writable
  exit
err_data_len:
  mov64 r0, 4                             # Custom(4): result isn't 8 bytes
  exit
err_owner:
  mov64 r0, 5                             # Custom(5): result not owned by us
  exit
err_ix_data:
  mov64 r0, 6                             # Custom(6): instruction data isn't 17 bytes
  exit
err_unknown_op:
  mov64 r0, 7                             # Custom(7): op not 0-3
  exit
err_overflow:
  mov64 r0, 8                             # Custom(8): overflow / underflow
  exit
err_div_zero:
  mov64 r0, 9                             # Custom(9): division by zero
  exit
