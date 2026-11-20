// Everything the UI needs to talk to calculator.s. Mirrors client/src/lib.rs.
//
// Instruction data (17 bytes): [op u8][a u64 LE][b u64 LE]
// Accounts: 0 authority (signer), 1 result (writable, owned by the program, 8 bytes)

import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from '@solana/web3.js'

export const PROGRAM_ID = new PublicKey('EaRMAabcmU3ospHn6T31bL2BSs7AMUkUQvJAgSG4wcbA')
export const IX_SIZE = 17
export const RESULT_SIZE = 8
export const U64_MAX = (1n << 64n) - 1n

// ------------------------------------------------------------------ networks

export type Network = 'devnet' | 'testnet' | 'local'

export const NETWORKS: Record<Network, { label: string; rpc: string }> = {
  devnet: { label: 'Devnet', rpc: 'https://api.devnet.solana.com' },
  testnet: { label: 'Testnet', rpc: 'https://api.testnet.solana.com' },
  local: { label: 'Local (surfpool)', rpc: 'http://127.0.0.1:8899' },
}

export function explorerTxUrl(network: Network, signature: string): string | null {
  return network === 'local' ? null : `https://explorer.solana.com/tx/${signature}?cluster=${network}`
}

// ------------------------------------------------------------------ ops

export const OPS = [
  { value: 0, name: 'add', symbol: '+' },
  { value: 1, name: 'sub', symbol: '−' },
  { value: 2, name: 'mul', symbol: '×' },
  { value: 3, name: 'div', symbol: '÷' },
] as const

export type Op = (typeof OPS)[number]['value']

/** The answer calculator.s should store, or null when it must reject (checked u64 math). */
export function expected(op: Op, a: bigint, b: bigint): bigint | null {
  let r: bigint
  switch (op) {
    case 0: r = a + b; break
    case 1: r = a - b; break
    case 2: r = a * b; break
    case 3: if (b === 0n) return null; r = a / b; break // BigInt division truncates, like u64
  }
  return r < 0n || r > U64_MAX ? null : r
}

/** Parses a decimal u64; throws a readable message otherwise. */
export function parseU64(input: string, name: string): bigint {
  const s = input.trim()
  if (!/^\d+$/.test(s)) throw new Error(`${name} must be a whole number from 0 to ${U64_MAX}`)
  const n = BigInt(s)
  if (n > U64_MAX) throw new Error(`${name} is bigger than u64::MAX (${U64_MAX})`)
  return n
}

// ------------------------------------------------------------------ errors (calculator.s codes 1-9)

export const PROGRAM_ERRORS: Record<number, string> = {
  1: 'wrong number of accounts',
  2: 'authority did not sign',
  3: 'result account is not writable',
  4: 'result account is not 8 bytes',
  5: 'result account is not owned by the calculator',
  6: 'instruction data is not 17 bytes',
  7: 'unknown op',
  8: 'overflow / underflow',
  9: 'division by zero',
}

export class ProgramError extends Error {
  code: number
  logs: string[]
  constructor(code: number, logs: string[]) {
    super(`${PROGRAM_ERRORS[code] ?? 'unknown error'} (Custom(${code}))`)
    this.code = code
    this.logs = logs
  }
}

// ------------------------------------------------------------------ encoding

export function instructionData(op: number, a: bigint, b: bigint): Uint8Array {
  const data = new Uint8Array(IX_SIZE)
  const view = new DataView(data.buffer)
  data[0] = op
  view.setBigUint64(1, a, true) // little-endian, like the sBPF ldxdw
  view.setBigUint64(9, b, true)
  return data
}

export function calculatorInstruction(authority: PublicKey, result: PublicKey, data: Uint8Array) {
  return new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: authority, isSigner: true, isWritable: false },
      { pubkey: result, isSigner: false, isWritable: true },
    ],
    data: Buffer.from(data),
  })
}

// ------------------------------------------------------------------ calling the program

export type Outcome = { value: bigint; signature: string; resultAccount: PublicKey }

type SignTransaction = <T extends VersionedTransaction>(tx: T) => Promise<T>

/**
 * One transaction: create a fresh 8-byte result account owned by the calculator,
 * then call the calculator. The payer (your wallet) is also the authority.
 *
 * It's simulated first, so a rejected calculation shows the program's error code
 * without asking the wallet to sign (and costs nothing).
 */
export async function calculate(
  connection: Connection,
  payer: PublicKey,
  signTransaction: SignTransaction,
  op: Op,
  a: bigint,
  b: bigint,
): Promise<Outcome> {
  const result = Keypair.generate()
  const rent = await connection.getMinimumBalanceForRentExemption(RESULT_SIZE)
  const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash()

  const message = new TransactionMessage({
    payerKey: payer,
    recentBlockhash: blockhash,
    instructions: [
      SystemProgram.createAccount({
        fromPubkey: payer,
        newAccountPubkey: result.publicKey,
        lamports: rent,
        space: RESULT_SIZE,
        programId: PROGRAM_ID,
      }),
      calculatorInstruction(payer, result.publicKey, instructionData(op, a, b)),
    ],
  }).compileToV0Message()

  // 1. Simulate (no signatures needed) to surface program errors early.
  const sim = await connection.simulateTransaction(new VersionedTransaction(message), {
    sigVerify: false,
    replaceRecentBlockhash: true,
  })
  if (sim.value.err) throw toError(sim.value.err, sim.value.logs ?? [])

  // 2. Wallet signs as payer + authority, then the new result account signs.
  const tx = await signTransaction(new VersionedTransaction(message))
  tx.sign([result])

  // 3. Send, confirm, read the u64 the program stored.
  const signature = await connection.sendRawTransaction(tx.serialize())
  const confirmation = await connection.confirmTransaction(
    { signature, blockhash, lastValidBlockHeight },
    'confirmed',
  )
  if (confirmation.value.err) throw toError(confirmation.value.err, [])

  const account = await connection.getAccountInfo(result.publicKey, 'confirmed')
  if (!account) throw new Error('result account not found after confirmation')
  const value = new DataView(account.data.buffer, account.data.byteOffset, 8).getBigUint64(0, true)
  return { value, signature, resultAccount: result.publicKey }
}

/** Turns `{ InstructionError: [1, { Custom: 8 }] }` into a ProgramError. */
function toError(err: unknown, logs: string[]): Error {
  const ix = (err as { InstructionError?: [number, unknown] })?.InstructionError
  const custom = (ix?.[1] as { Custom?: number } | undefined)?.Custom
  if (typeof custom === 'number' && ix?.[0] === 1) return new ProgramError(custom, logs)
  if (JSON.stringify(err).includes('AccountNotFound') || logs.some((l) => l.includes('insufficient')))
    return new Error('not enough SOL in your wallet on this network (airdrop some first)')
  return new Error(`transaction failed: ${JSON.stringify(err)}`)
}

export async function isDeployed(connection: Connection): Promise<boolean> {
  const info = await connection.getAccountInfo(PROGRAM_ID)
  return !!info?.executable
}
