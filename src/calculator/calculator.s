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
