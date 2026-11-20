#!/usr/bin/env python3
"""
offsets.py - print the input-buffer offsets for an sBPF program.

When a Solana program starts, register r1 points at one big byte buffer that
the runtime builds from your instruction (accounts + instruction data +
program id). Your assembly reads fields with `ldxdw r2, [r1 + OFFSET]`.
This script computes every OFFSET for you and prints ready-to-paste `.equ`
lines.

Usage:
    python3 offsets.py <name>:<data_len> [<name>:<data_len> ...] --ix <ix_data_len>

    Each account is "name:data_len" in the SAME ORDER as the instruction's
    account list. Use "name:dup" for an account that repeats an earlier one.

Examples:
    # the calculator: authority (0 bytes), result (8 bytes), 17 bytes of ix data
    python3 offsets.py auth:0 result:8 --ix 17

    # the counter example in sbpf/examples: owner, counter, system program
    # (the System Program account holds 14 bytes: the string "system_program")
    python3 offsets.py owner:0 counter:0 system_program:14 --ix 2

The layout comes from sbpf/crates/runtime/src/serialize.rs:

    u64                  number of accounts
    for each account:
      not a duplicate:
        u8   0xff (dup marker)    u8 is_signer    u8 is_writable    u8 is_executable
        [4]  padding
        [32] pubkey               [32] owner
        u64  lamports             u64 data_len
        [data_len] data, then 10240 spare bytes, then zero padding up to a multiple of 8
        u64  rent_epoch
      duplicate:
        u8   index of the earlier account, then [7] padding      (8 bytes total)
    u64                  instruction data length
    [len]                instruction data
    [32]                 program id
"""

import argparse

MAX_PERMITTED_DATA_INCREASE = 10240  # spare room so a program can grow an account


def round_up_8(n: int) -> int:
    """Round n up to the next multiple of 8 (the runtime keeps u64s 8-aligned)."""
    return (n + 7) // 8 * 8


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("accounts", nargs="+", help='"name:data_len" or "name:dup", in instruction order')
    parser.add_argument("--ix", type=int, required=True, help="instruction data length in bytes")
    args = parser.parse_args()

    lines = []  # (name, offset, explanation)

    def emit(name: str, offset: int, why: str) -> None:
        lines.append((name, offset, why))

    emit("NUM_ACCOUNTS", 0, "u64 at the very start")
    cursor = 8  # the first account starts right after the u64 count

    for spec in args.accounts:
        name, _, length = spec.partition(":")
        prefix = name.upper()

        if length == "dup":
            emit(f"{prefix}_DUP_INDEX", cursor, "duplicate: 1-byte index of the earlier account")
            cursor += 8  # a duplicate takes exactly 8 bytes
            continue

        data_len = int(length)
        start = cursor
        emit(f"{prefix}_HEADER", start + 0, "dup marker, must be 0xff")
        emit(f"{prefix}_IS_SIGNER", start + 1, "1 = signed the transaction")
        emit(f"{prefix}_IS_WRITABLE", start + 2, "1 = marked writable")
        emit(f"{prefix}_IS_EXECUTABLE", start + 3, "1 = is a program")
        emit(f"{prefix}_KEY", start + 8, "32-byte pubkey  (start + 8)")
        emit(f"{prefix}_OWNER", start + 40, "32-byte owner   (start + 40)")
        emit(f"{prefix}_LAMPORTS", start + 72, "u64             (start + 72)")
        emit(f"{prefix}_DATA_LEN", start + 80, "u64             (start + 80)")
        emit(f"{prefix}_DATA", start + 88, f"{data_len} bytes of data (start + 88)")
        data_region = round_up_8(data_len + MAX_PERMITTED_DATA_INCREASE)
        rent_epoch = start + 88 + data_region
        emit(f"{prefix}_RENT_EPOCH", rent_epoch,
             f"u64 after data({data_len}) + 10240 spare, rounded up to 8 = {data_region}")
        cursor = rent_epoch + 8  # next account starts after the u64 rent_epoch

    emit("INSTRUCTION_DATA_LEN", cursor, "u64 right after the last account")
    emit("INSTRUCTION_DATA", cursor + 8, f"{args.ix} bytes")
    emit("PROGRAM_ID", cursor + 8 + args.ix, "32 bytes, right after the instruction data")

    width = max(len(n) for n, _, _ in lines)
    for name, offset, why in lines:
        print(f".equ {name + ',':<{width + 1}} 0x{offset:04x}    # {offset:>6}  {why}")


if __name__ == "__main__":
    main()
